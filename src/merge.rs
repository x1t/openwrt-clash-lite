//! Builds the config handed to mihomo: our takeover settings in front of the untouched subscription.
//!
//! The subscription is processed line by line (no YAML parser): top-level blocks we own are dropped,
//! everything else is kept verbatim. `---` document separators are removed because mihomo only reads
//! the first YAML document, which silently ignores proxies, rules and dns (found the hard way).

/// Top-level keys we set ourselves; a subscription copy of them would conflict or break the takeover.
const OWNED_KEYS: [&str; 11] = [
    "external-controller",
    "external-controller-tls",
    "external-controller-unix",
    "external-ui",
    "external-ui-name",
    "external-ui-url",
    "secret",
    "tun",
    "log-level",
    "interface-name",
    "routing-mark",
];

/// No `listen`: the TUN's dns-hijack answers through mihomo's resolver without a listening socket.
const FALLBACK_DNS: &str = "dns:
  enable: true
  enhanced-mode: redir-host
  default-nameserver: [223.5.5.5, 119.29.29.29]
  nameserver: [223.5.5.5, 119.29.29.29]
";

pub struct Overrides<'a> {
    pub controller: &'a str,
    pub secret: &'a str,
    pub log_level: &'a str,
    pub ui_dir: Option<&'a str>,
}

/// `key` of a line that starts a top-level mapping entry, e.g. `tun:` or `log-level: info`.
fn top_level_key(line: &str) -> Option<&str> {
    let first = line.chars().next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let key = line.split_once(':')?.0;
    key.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        .then_some(key)
}

fn is_document_marker(line: &str) -> bool {
    matches!(line.trim_end(), "---" | "...")
}

/// Leading spaces of a line, or None for blank and comment lines.
fn indent_of(line: &str) -> Option<usize> {
    let t = line.trim_start();
    (!t.is_empty() && !t.starts_with('#')).then(|| line.len() - t.len())
}

/// Handles one entry of the subscription's `dns:` block; returns false when the line must be dropped.
/// - `listen` goes: subscriptions often say `0.0.0.0:53`, which collides with dnsmasq, and the TUN's
///   dns-hijack needs no listening socket (`listeners:` entries elsewhere keep their `listen`).
/// - `enhanced-mode` is always `redir-host` (never fake-ip): we write it before the block's first
///   entry and drop whatever the subscription says.
fn filter_dns_line(
    line: &str,
    indent: usize,
    dns_indent: &mut Option<usize>,
    out: &mut String,
) -> bool {
    if dns_indent.is_none() {
        *dns_indent = Some(indent);
        out.push_str(&format!(
            "{}enhanced-mode: redir-host\n",
            " ".repeat(indent)
        ));
    }
    let entry = line.trim_start();
    let is_ours_to_drop = entry.starts_with("listen:") || entry.starts_with("enhanced-mode:");
    !(is_ours_to_drop && Some(indent) == *dns_indent)
}

/// The subscription without owned blocks and document markers, and whether it has a `dns:` block.
fn strip_owned(subscription: &str) -> (String, bool) {
    let text = subscription
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let (mut out, mut skipping, mut has_dns) = (String::new(), false, false);
    let (mut in_dns, mut dns_indent) = (false, None);
    for line in text.lines() {
        if is_document_marker(line) {
            continue;
        }
        if let Some(key) = top_level_key(line) {
            skipping = OWNED_KEYS.contains(&key);
            in_dns = key == "dns";
            dns_indent = None;
            has_dns |= in_dns;
        } else if let (true, Some(indent)) = (in_dns, indent_of(line))
            && !filter_dns_line(line, indent, &mut dns_indent, &mut out)
        {
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, has_dns)
}

fn header(o: &Overrides) -> String {
    let mut h = format!(
        "external-controller: {}\nsecret: {}\nlog-level: {}\n",
        o.controller, o.secret, o.log_level
    );
    if let Some(ui) = o.ui_dir {
        h.push_str(&format!("external-ui: {ui}\n"));
    }
    // Starts off: mihomo drops all connections until its providers load, so the supervisor
    // switches the TUN on afterwards instead of cutting the LAN for that time.
    // uid 65534 (nobody) bypasses the TUN, so router-side jobs can go direct as they could with
    // OpenClash (which skips gid 65534; mihomo can only match uids).
    h.push_str(
        "tun:
  enable: false
  stack: system
  device: lite0
  auto-route: true
  auto-redirect: true
  auto-detect-interface: true
  exclude-uid:
    - 65534
  dns-hijack:
    - any:53
",
    );
    h
}

pub fn compose(subscription: &str, o: &Overrides) -> String {
    let (body, has_dns) = strip_owned(subscription);
    let mut config = header(o);
    config.push_str(&body);
    if !has_dns {
        config.push_str(FALLBACK_DNS);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: Overrides = Overrides {
        controller: "127.0.0.1:19097",
        secret: "s3",
        log_level: "warning",
        ui_dir: None,
    };

    #[test]
    fn dns_listen_is_removed_but_listeners_keep_theirs() {
        let sub = "dns:\n  enable: true\n  listen: 0.0.0.0:53\n  nameserver:\n  - 1.1.1.1\n  ipv6: false\n\
                   listeners:\n- name: s\n  type: socks\n  listen: 0.0.0.0\n  port: 1\n";
        let out = compose(sub, &O);
        assert!(!out.contains("0.0.0.0:53"), "dns.listen must go");
        assert!(
            out.contains("  enable: true\n")
                && out.contains("  - 1.1.1.1\n")
                && out.contains("  ipv6: false\n")
        );
        assert!(
            out.contains("  listen: 0.0.0.0\n"),
            "the socks listener's listen must stay"
        );
        assert_eq!(out.matches("dns:").count(), 1);
    }

    #[test]
    fn only_first_level_listen_inside_dns_is_removed() {
        let sub = "dns:\n  enable: true\n  fallback-filter:\n    listen: nested-is-not-ours\n  listen: :53\n";
        let out = compose(sub, &O);
        assert!(out.contains("nested-is-not-ours") && !out.contains("listen: :53"));
    }

    #[test]
    fn the_log_level_comes_from_the_overrides() {
        let out = compose(
            "proxies: []\n",
            &Overrides {
                log_level: "info",
                ..O
            },
        );
        assert!(out.contains("log-level: info\n") && !out.contains("log-level: warning"));
    }

    fn enhanced_modes(config: &str) -> Vec<&str> {
        config
            .lines()
            .filter(|l| l.trim_start().starts_with("enhanced-mode:"))
            .collect()
    }

    #[test]
    fn fake_ip_in_the_subscription_is_replaced_by_redir_host() {
        let sub = "dns:\n  enable: true\n  enhanced-mode: fake-ip\n  fake-ip-range: 198.18.0.1/16\n  nameserver:\n  - 1.1.1.1\n";
        let out = compose(sub, &O);
        assert_eq!(enhanced_modes(&out), vec!["  enhanced-mode: redir-host"]);
        assert!(!out.contains("fake-ip\n"), "no fake-ip mode left");
        assert!(
            out.contains("  - 1.1.1.1\n"),
            "the rest of the dns block is kept"
        );
    }

    #[test]
    fn redir_host_is_added_when_the_dns_block_has_no_mode() {
        let out = compose("dns:\n  enable: true\n  ipv6: false\n", &O);
        assert_eq!(enhanced_modes(&out), vec!["  enhanced-mode: redir-host"]);
        assert!(out.contains("  enable: true\n") && out.contains("  ipv6: false\n"));
    }

    #[test]
    fn a_subscription_redir_host_is_not_duplicated() {
        let out = compose("dns:\n  enhanced-mode: redir-host\n  enable: true\n", &O);
        assert_eq!(enhanced_modes(&out).len(), 1);
    }

    #[test]
    fn a_four_space_dns_block_gets_matching_indentation() {
        let out = compose("dns:\n    enable: true\n    enhanced-mode: fake-ip\n", &O);
        assert_eq!(enhanced_modes(&out), vec!["    enhanced-mode: redir-host"]);
    }

    #[test]
    fn the_fallback_dns_has_no_listening_socket() {
        let out = compose("proxies: []\n", &O);
        assert!(out.contains("enhanced-mode: redir-host") && !out.contains("listen:"));
    }

    #[test]
    fn document_separators_and_bom_and_crlf_are_removed() {
        let out = compose("\u{feff}---\r\nmode: rule\r\n---\r\nproxies: []\r\n", &O);
        assert!(!out.lines().any(|l| l == "---"));
        assert!(!out.contains('\r') && !out.contains('\u{feff}'));
        assert!(out.contains("mode: rule\nproxies: []\n"));
    }

    #[test]
    fn owned_blocks_are_dropped_with_their_nested_content() {
        let sub = "tun:\n  enable: true\n  stack: gvisor\n  dns-hijack:\n  - 0.0.0.0:53\nmode: rule\nsecret: old\nlog-level: debug\n";
        let out = compose(sub, &O);
        assert_eq!(out.matches("tun:").count(), 1, "only our tun block remains");
        assert!(!out.contains("gvisor") && !out.contains("0.0.0.0:53"));
        assert!(!out.contains("secret: old") && !out.contains("log-level: debug"));
        assert!(out.contains("mode: rule"));
    }

    #[test]
    fn a_one_line_flow_style_tun_is_dropped_too() {
        let out = compose("tun: {enable: true, stack: gvisor}\nmode: global\n", &O);
        assert!(!out.contains("gvisor"));
        assert!(out.contains("mode: global"));
    }

    #[test]
    fn list_items_and_indented_lookalikes_are_not_top_level_keys() {
        let sub =
            "proxies:\n- name: a\n  secret: keep-me\n  tun: keep-me-too\nrules:\n- MATCH,DIRECT\n";
        let out = compose(sub, &O);
        assert!(out.contains("  secret: keep-me\n") && out.contains("  tun: keep-me-too\n"));
        assert!(out.contains("- MATCH,DIRECT"));
    }

    #[test]
    fn similarly_named_keys_are_kept() {
        let out = compose("tunnels: x\nsecrets: y\nlog-level-extra: z\n", &O);
        assert!(
            out.contains("tunnels: x")
                && out.contains("secrets: y")
                && out.contains("log-level-extra: z")
        );
    }

    #[test]
    fn our_header_comes_first_with_takeover_settings() {
        let out = compose(
            "proxies: []\n",
            &Overrides {
                ui_dir: Some("/etc/x/ui"),
                ..O
            },
        );
        assert!(out.starts_with("external-controller: 127.0.0.1:19097\nsecret: s3\n"));
        assert!(
            out.contains("tun:\n  enable: false\n"),
            "the supervisor switches the TUN on later"
        );
        for want in [
            "auto-route: true",
            "auto-redirect: true",
            "any:53",
            "external-ui: /etc/x/ui",
            "device: lite0",
            "exclude-uid:
    - 65534
",
        ] {
            assert!(out.contains(want), "missing {want}");
        }
    }

    #[test]
    fn dns_is_added_only_when_the_subscription_has_none() {
        assert!(compose("proxies: []\n", &O).contains("enhanced-mode: redir-host"));
        let own = compose("dns:\n  enable: true\n  nameserver: [1.1.1.1]\n", &O);
        assert_eq!(
            own.matches("dns:").count(),
            1,
            "the subscription's own dns block is not duplicated"
        );
        assert!(own.contains("nameserver: [1.1.1.1]"));
    }
}
