//! Subscription download and validation (same rules as the Windows clash-lite, UA fixed to `xctcc`).
use crate::state::Res;
use std::time::Duration;
use ureq::Agent;

const USER_AGENT: &str = "xctcc";
const MAX_BODY: u64 = 32 * 1024 * 1024;

pub struct Fetched {
    pub body: String,
    pub name: String,
}

/// Fixes `https://host/path&token=x`, where the query was glued onto the path.
fn fix_dirty_url(url: &str) -> String {
    let host_end = url.find("://").map_or(0, |i| i + 3);
    let has_path_amp = !url.contains('?') && url[host_end..].contains('&');
    if has_path_amp {
        url.replacen('&', "?", 1)
    } else {
        url.to_string()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn filename(disposition: &str) -> Option<String> {
    let find = |key: &str| {
        disposition
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix(key))
    };
    if let Some(v) = find("filename*=") {
        let encoded = v.rsplit("''").next().unwrap_or(v);
        return Some(percent_decode(encoded.trim_matches('"')));
    }
    find("filename=").map(|v| v.trim_matches('"').to_string())
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?'])
        .next()
        .unwrap_or("subscription")
        .to_string()
}

pub fn validate(body: &str) -> Res<()> {
    let ok = body
        .lines()
        .any(|l| l.starts_with("proxies:") || l.starts_with("proxy-providers:"));
    if ok {
        Ok(())
    } else {
        Err("not a Clash YAML (no top-level proxies / proxy-providers)".into())
    }
}

pub fn fetch(url: &str) -> Res<Fetched> {
    let url = fix_dirty_url(url.trim());
    let agent: Agent = Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut resp = agent.get(&url).call()?;
    let name = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .and_then(filename)
        .unwrap_or_else(|| host_of(&url));
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()?;
    let body = text.trim_start_matches('\u{feff}').to_string();
    validate(&body)?;
    Ok(Fetched { body, name })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_query_glued_to_path() {
        assert_eq!(
            fix_dirty_url("https://a.com/sub&token=1&f=clash"),
            "https://a.com/sub?token=1&f=clash"
        );
        assert_eq!(
            fix_dirty_url("https://a.com/sub?token=1&x=2"),
            "https://a.com/sub?token=1&x=2"
        );
        assert_eq!(fix_dirty_url("https://a.com/sub"), "https://a.com/sub");
    }

    #[test]
    fn extracts_filename_from_disposition() {
        assert_eq!(
            filename("attachment; filename=\"plain.yaml\"").as_deref(),
            Some("plain.yaml")
        );
        let rfc5987 = "attachment; filename*=UTF-8''%E6%9C%BA%E5%9C%BA%20A.yaml";
        assert_eq!(filename(rfc5987).as_deref(), Some("机场 A.yaml"));
        assert_eq!(filename("inline"), None);
    }

    #[test]
    fn host_is_the_fallback_name() {
        assert_eq!(
            host_of("https://sub.example.com/api/v1?token=x"),
            "sub.example.com"
        );
    }

    #[test]
    fn validates_clash_yaml() {
        assert!(validate("port: 1\nproxies:\n  - name: a\n").is_ok());
        assert!(validate("proxy-providers:\n  p: {}\n").is_ok());
        assert!(validate("c3M6Ly9hYmM=").is_err());
        assert!(validate("  proxies:\n").is_err());
    }
}
