//! Everything around mihomo's own auto-route/auto-redirect that the router needs us to handle:
//! refuse to run next to another proxy core, satisfy Kwrt's fw4 quirk, and sweep up what a
//! killed mihomo leaves behind. (Findings: _notes/findings.md)
use crate::{
    state::Res,
    sys::{file_exists, run_cmd},
};
use std::fs;

/// Kwrt's `fw4 reload` exits 1 unless this file exists, and mihomo treats exit 1 as "TUN failed".
const INCLUDE: &str = "/etc/firewall.include";
/// Written by mihomo into fw4's include dir; fw4 re-reads it on every reload.
const AUTO_REDIRECT_FILE: &str = "/etc/nftables.d/0-mihomo-auto-redirect.nft";
/// ip rule priorities mihomo's auto-route uses.
const RULE_PREFS: std::ops::RangeInclusive<u32> = 9000..=9010;
const CORE_NAMES: [&str; 3] = ["clash_meta", "clash", "mihomo"];

/// Content of the include file we create. Recognising it lets a later run clean up after a run
/// that was killed, while a file with any other content (the user's own) is never touched.
const INCLUDE_MARKER: &str = "# created by openwrt-clash-lite (empty on purpose: Kwrt fw4 quirk)
";

pub struct Guard {
    owns_include: bool,
}

#[derive(Debug, PartialEq)]
enum Include {
    Missing,
    Ours,
    Foreign,
}

fn classify_include(content: Option<&str>) -> Include {
    match content {
        None => Include::Missing,
        Some(INCLUDE_MARKER) => Include::Ours,
        Some(_) => Include::Foreign,
    }
}

/// Other proxy cores already running (OpenClash, a stray mihomo). Both would fight over the same
/// firewall hooks and DNS.
pub fn find_conflicts(procs: &[(u32, String)]) -> Vec<(u32, String)> {
    procs
        .iter()
        .filter(|(_, comm)| CORE_NAMES.contains(&comm.as_str()))
        .cloned()
        .collect()
}

pub fn conflict_message(conflicts: &[(u32, String)]) -> String {
    let list: Vec<String> = conflicts
        .iter()
        .map(|(pid, name)| format!("{name}({pid})"))
        .collect();
    format!(
        "another proxy core is running: {}. Stop OpenClash or the other mihomo first.",
        list.join(", ")
    )
}

/// Priorities of leftover auto-route rules in `ip rule show` output; one entry per rule line.
pub fn parse_rule_prefs(ip_rule_show: &str) -> Vec<u32> {
    ip_rule_show
        .lines()
        .filter_map(|line| line.split_once(':')?.0.trim().parse::<u32>().ok())
        .filter(|pref| RULE_PREFS.contains(pref))
        .collect()
}

/// Removes what mihomo leaves behind when it is killed instead of stopped: routing rules, its nft
/// table, and its fw4 include file. Harmless when nothing is there.
pub fn clean_stale() {
    let (_, rules) = run_cmd("ip", &["rule", "show"]);
    for pref in parse_rule_prefs(&rules) {
        run_cmd("ip", &["rule", "del", "pref", &pref.to_string()]);
    }
    run_cmd("nft", &["delete", "table", "inet", "mihomo"]);
    if file_exists(AUTO_REDIRECT_FILE) && fs::remove_file(AUTO_REDIRECT_FILE).is_ok() {
        run_cmd("fw4", &["reload"]); // exit code is unreliable on Kwrt, so it is ignored
    }
}

pub fn prepare() -> Res<Guard> {
    clean_stale();
    // An unreadable existing file is treated as foreign, never overwritten.
    let existing =
        file_exists(INCLUDE).then(|| fs::read_to_string(INCLUDE).unwrap_or_else(|_| "?".into()));
    let owns_include = match classify_include(existing.as_deref()) {
        Include::Missing => {
            fs::write(INCLUDE, INCLUDE_MARKER)
                .map_err(|e| format!("cannot create {INCLUDE}: {e}"))?;
            true
        }
        Include::Ours => true,
        Include::Foreign => false,
    };
    Ok(Guard { owns_include })
}

pub fn release(guard: Guard) {
    clean_stale();
    if guard.owns_include {
        let _ = fs::remove_file(INCLUDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs(list: &[(u32, &str)]) -> Vec<(u32, String)> {
        list.iter().map(|(p, n)| (*p, n.to_string())).collect()
    }

    #[test]
    fn detects_openclash_style_cores_only() {
        let all = procs(&[
            (1, "procd"),
            (20, "clash_meta"),
            (21, "dnsmasq"),
            (30, "mihomo"),
            (31, "clash"),
        ]);
        let found: Vec<u32> = find_conflicts(&all).iter().map(|(p, _)| *p).collect();
        assert_eq!(found, vec![20, 30, 31]);
        assert!(find_conflicts(&procs(&[(1, "procd"), (2, "openwrt-clash-l")])).is_empty());
    }

    #[test]
    fn conflict_message_names_every_process() {
        let msg = conflict_message(&procs(&[(20, "clash_meta"), (30, "mihomo")]));
        assert!(msg.contains("clash_meta(20)") && msg.contains("mihomo(30)"));
    }

    #[test]
    fn only_auto_route_priorities_are_picked_from_ip_rule_output() {
        let out = "0:\tfrom all lookup local\n\
                   1888:\tfrom all fwmark 0x162 lookup 354\n\
                   9000:\tfrom all to 198.18.0.0/30 lookup 2022\n\
                   9001:\tnot from all dport 53 lookup main suppress_prefixlength 0\n\
                   9001:\tfrom all iif lite0 goto 9010\n\
                   9010:\tfrom all nop\n\
                   32766:\tfrom all lookup main\n";
        assert_eq!(parse_rule_prefs(out), vec![9000, 9001, 9001, 9010]);
    }

    #[test]
    fn the_include_file_is_classified_by_its_content() {
        assert_eq!(classify_include(None), Include::Missing);
        assert_eq!(classify_include(Some(INCLUDE_MARKER)), Include::Ours);
        assert_eq!(
            classify_include(Some("")),
            Include::Foreign,
            "an empty user file is not ours"
        );
        assert_eq!(
            classify_include(Some(
                "/etc/my-hook.sh
"
            )),
            Include::Foreign
        );
    }

    #[test]
    fn garbage_ip_output_yields_nothing() {
        assert!(parse_rule_prefs("").is_empty());
        assert!(parse_rule_prefs("Cannot open netlink socket: Permission denied").is_empty());
    }
}
