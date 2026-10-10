use crate::state::Res;
use serde_json::Value;
use std::time::Duration;
use ureq::Agent;

fn agent() -> Agent {
    Agent::config_builder()
        .proxy(None) // never route the control channel through any proxy
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .into()
}

fn get(controller: &str, secret: &str, path: &str) -> Res<Value> {
    let mut resp = agent()
        .get(format!("http://{controller}{path}"))
        .header("Authorization", format!("Bearer {secret}"))
        .call()?;
    Ok(resp.body_mut().read_json()?)
}

/// mihomo version string when its REST API answers, e.g. `v1.19.32`.
pub fn version(controller: &str, secret: &str) -> Res<String> {
    let v = get(controller, secret, "/version")?;
    Ok(v["version"].as_str().unwrap_or("unknown").to_string())
}

pub fn ready(controller: &str, secret: &str) -> bool {
    version(controller, secret).is_ok()
}

/// Whether mihomo's TUN is enabled, from `/configs`.
pub fn tun_enabled(controller: &str, secret: &str) -> Res<bool> {
    Ok(get(controller, secret, "/configs")?["tun"]["enable"]
        .as_bool()
        .unwrap_or(false))
}

/// Switches the TUN on; mihomo keeps the rest of the `tun:` block from the config file.
pub fn enable_tun(controller: &str, secret: &str) -> Res<()> {
    agent()
        .patch(format!("http://{controller}/configs"))
        .header("Authorization", format!("Bearer {secret}"))
        .send_json(serde_json::json!({ "tun": { "enable": true } }))?;
    Ok(())
}

/// Every downloaded (HTTP) or file provider has content; a failed first download keeps the
/// zero time `0001-01-01...`. Built-in `Compatible`/`Inline` entries are skipped.
fn all_loaded(resp: &Value) -> bool {
    resp["providers"].as_object().is_some_and(|providers| {
        providers
            .values()
            .filter(|p| matches!(p["vehicleType"].as_str(), Some("HTTP" | "File")))
            .all(|p| {
                p["updatedAt"]
                    .as_str()
                    .is_some_and(|t| !t.starts_with("0001-"))
            })
    })
}

/// mihomo drops every connection until its proxy and rule providers have loaded.
pub fn providers_loaded(controller: &str, secret: &str) -> Res<bool> {
    Ok(all_loaded(&get(controller, secret, "/providers/proxies")?)
        && all_loaded(&get(controller, secret, "/providers/rules")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn providers_count_as_loaded_only_when_every_fetched_one_has_a_time() {
        let loaded = json!({"providers": {
            "apple": {"vehicleType": "HTTP", "updatedAt": "2026-10-10T09:28:46.661900578+08:00"},
            "local": {"vehicleType": "File", "updatedAt": "2026-10-09T01:00:00+08:00"},
            "PROXY": {"vehicleType": "Compatible", "proxies": []}
        }});
        assert!(all_loaded(&loaded));
        let pending = json!({"providers": {
            "apple": {"vehicleType": "HTTP", "updatedAt": "2026-10-10T09:28:46+08:00"},
            "google": {"vehicleType": "HTTP", "updatedAt": "0001-01-01T00:00:00Z"}
        }});
        assert!(!all_loaded(&pending));
        let missing_time = json!({"providers": {"x": {"vehicleType": "HTTP"}}});
        assert!(!all_loaded(&missing_time));
    }

    #[test]
    fn no_providers_is_loaded_but_an_unexpected_reply_is_not() {
        assert!(all_loaded(&json!({"providers": {}})));
        assert!(!all_loaded(&json!({"message": "Unauthorized"})));
    }
}
