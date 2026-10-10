mod api;
mod config;
mod cron;
mod geo;
mod merge;
mod mihomo;
mod state;
mod subscription;
mod supervisor;
mod sys;
mod takeover;
mod uci;

use config::Paths;
use serde_json::{Value, json};
use state::{Profile, Res, State};
use std::{
    fs,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};
use subscription::Fetched;

const USAGE: &str = "openwrt-clash-lite <command>
  run              run mihomo with transparent takeover (foreground, used by procd)
  add <url>        download a subscription; it becomes active if none is
  update [id]      re-download the active (or the given) subscription
  edit <id> <name> <url>  rename a subscription or change its URL (a new URL is downloaded first)
  use <id>         make a subscription active (restart the service to apply)
  remove <id>      delete a subscription
  list [--json]    list subscriptions
  status [--json]  show whether mihomo is up
  update-core      install the latest mihomo release for this CPU
  auto-update      daily job: update the active subscription, restart only if it changed
  sync-cron        write or remove the daily job in root's crontab (UCI auto_update)
  version";

fn new_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("R{millis:x}")
}

/// Subscriptions hold node passwords: owner-only, like the UCI file.
fn write_profile(paths: &Paths, id: &str, body: &str) -> Res<()> {
    fs::create_dir_all(paths.profiles_dir())?;
    fs::write(paths.profile(id), body)?;
    sys::restrict_file(&paths.profile(id));
    Ok(())
}

/// Saves a downloaded subscription under `id`, registers it, and activates it if nothing is active.
/// A known subscription keeps its name (the user may have edited it).
fn store(paths: &Paths, state: &mut State, id: &str, url: &str, fetched: &Fetched) -> Res<()> {
    write_profile(paths, id, &fetched.body)?;
    if !state.profiles.iter().any(|p| p.id == id) {
        state.profiles.push(Profile {
            id: id.to_string(),
            name: fetched.name.clone(),
            url: url.to_string(),
        });
    }
    state.active.get_or_insert_with(|| id.to_string());
    state.save(paths)
}

fn cmd_add(paths: &Paths, url: &str) -> Res<()> {
    let fetched = subscription::fetch(url)?;
    let mut state = State::load(paths)?;
    let id = state
        .profiles
        .iter()
        .find(|p| p.url == url)
        .map_or_else(new_id, |p| p.id.clone());
    store(paths, &mut state, &id, url, &fetched)?;
    println!("saved \"{}\" as {id}", fetched.name);
    Ok(())
}

fn find<'a>(state: &'a State, id: &str) -> Res<&'a Profile> {
    Ok(state
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or(format!("unknown subscription {id}"))?)
}

fn cmd_update(paths: &Paths, id: Option<&str>) -> Res<()> {
    let mut state = State::load(paths)?;
    let id = id
        .map(str::to_string)
        .or_else(|| state.active.clone())
        .ok_or("no subscription yet")?;
    let url = find(&state, &id)?.url.clone();
    store(paths, &mut state, &id, &url, &subscription::fetch(&url)?)?;
    println!("updated {id}; restart the service to apply");
    Ok(())
}

/// Renames a subscription and/or changes its URL. A new URL is downloaded first, so a bad one
/// changes nothing.
fn cmd_edit(paths: &Paths, id: &str, name: &str, url: &str) -> Res<()> {
    let mut state = State::load(paths)?;
    let name = name.trim();
    if name.is_empty() {
        return Err("name must not be empty".into());
    }
    if find(&state, id)?.url != url {
        let fetched = subscription::fetch(url)?;
        write_profile(paths, id, &fetched.body)?;
    }
    let p = state
        .profiles
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or(format!("unknown subscription {id}"))?;
    p.name = name.to_string();
    p.url = url.to_string();
    state.save(paths)?;
    println!("saved {id}");
    Ok(())
}

/// Daily job: an unchanged subscription causes no restart, so no interruption for the LAN.
fn cmd_auto_update(paths: &Paths) -> Res<()> {
    let mut state = State::load(paths)?;
    let p = state
        .active_profile()
        .ok_or("no active subscription")?
        .clone();
    let fetched = subscription::fetch(&p.url)?;
    let unchanged = fs::read_to_string(paths.profile(&p.id)).is_ok_and(|old| old == fetched.body);
    store(paths, &mut state, &p.id, &p.url, &fetched)?;
    if unchanged {
        println!("{} unchanged", p.id);
        return Ok(());
    }
    let running = api::ready(&state.controller(), &state.secret);
    if running {
        sys::run_cmd("/etc/init.d/openwrt-clash-lite", &["restart"]);
    }
    println!(
        "{} updated{}",
        p.id,
        if running { "; service restarted" } else { "" }
    );
    Ok(())
}

fn cmd_sync_cron(paths: &Paths) -> Res<()> {
    let state = State::load(paths)?;
    let changed = cron::sync(paths, &state)?;
    let job = if state.auto_update {
        format!("daily at {}", state.update_time)
    } else {
        "off".into()
    };
    println!(
        "subscription auto-update {job}{}",
        if changed { " (crontab updated)" } else { "" }
    );
    Ok(())
}

fn cmd_use(paths: &Paths, id: &str) -> Res<()> {
    let mut state = State::load(paths)?;
    find(&state, id)?;
    state.active = Some(id.to_string());
    state.save(paths)?;
    println!("{id} is now active; restart the service to apply");
    Ok(())
}

/// Removing the active subscription activates the first remaining one, if any.
fn cmd_remove(paths: &Paths, id: &str) -> Res<()> {
    let mut state = State::load(paths)?;
    find(&state, id)?;
    state.profiles.retain(|p| p.id != id);
    if state.active.as_deref() == Some(id) {
        state.active = state.profiles.first().map(|p| p.id.clone());
    }
    state.save(paths)?;
    let _ = fs::remove_file(paths.profile(id));
    println!("removed {id}");
    Ok(())
}

fn list_json(paths: &Paths, state: &State) -> Value {
    let items: Vec<Value> = state
        .profiles
        .iter()
        .map(|p| {
            json!({
                "id": p.id, "name": p.name, "url": p.url,
                "updated": paths.profile_updated(&p.id),
                "active": state.active.as_deref() == Some(p.id.as_str()),
            })
        })
        .collect();
    Value::Array(items)
}

fn cmd_list(paths: &Paths, as_json: bool) -> Res<()> {
    let state = State::load(paths)?;
    if as_json {
        println!("{}", list_json(paths, &state));
        return Ok(());
    }
    for p in &state.profiles {
        let mark = if state.active.as_deref() == Some(p.id.as_str()) {
            '*'
        } else {
            ' '
        };
        println!("{mark} {}  {}", p.id, p.name);
    }
    Ok(())
}

fn status_json(paths: &Paths, state: &State) -> Value {
    let (ctl, secret) = (state.controller(), state.secret.as_str());
    let version = api::version(&ctl, secret).ok();
    let pid = fs::read_to_string(paths.pid_file()).ok();
    let memory_kb = pid.and_then(|p| sys::rss_kb(p.trim().parse().ok()?));
    json!({
        "running": version.is_some(),
        "version": version,
        "tun": version.is_some() && api::tun_enabled(&ctl, secret).unwrap_or(false),
        "memory_kb": memory_kb,
        "port": state.port,
        "secret": state.secret,
        "active": state.active_profile().map(|p| json!({"id": p.id, "name": p.name})),
        "core": paths.mihomo_bin(),
    })
}

fn cmd_status(paths: &Paths, as_json: bool) -> Res<()> {
    let state = State::load(paths)?;
    let s = status_json(paths, &state);
    if as_json {
        println!("{s}");
    } else if let Some(v) = s["version"].as_str() {
        let tun = if s["tun"] == true { "on" } else { "off" };
        println!("running: mihomo {v}, tun {tun}");
    } else {
        println!(
            "not running (controller {} does not answer)",
            state.controller()
        );
    }
    Ok(())
}

fn cmd_update_core(paths: &Paths) -> Res<()> {
    let done = mihomo::update(paths)?;
    if done.starts_with(mihomo::UP_TO_DATE) {
        println!("{done}");
    } else {
        println!("installed {done}; restart the service to use it");
    }
    Ok(())
}

fn cmd_version() -> Res<()> {
    println!("openwrt-clash-lite {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

fn dispatch(args: &[String], paths: &Paths) -> Res<()> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    let json = arg(1) == Some("--json");
    match arg(0) {
        Some("run") => supervisor::run(paths),
        Some("add") => cmd_add(paths, arg(1).ok_or("usage: add <url>")?),
        Some("update") => cmd_update(paths, arg(1)),
        Some("edit") => match (arg(1), arg(2), arg(3)) {
            (Some(id), Some(name), Some(url)) => cmd_edit(paths, id, name, url),
            _ => Err("usage: edit <id> <name> <url>".into()),
        },
        Some("use") => cmd_use(paths, arg(1).ok_or("usage: use <id>")?),
        Some("remove") => cmd_remove(paths, arg(1).ok_or("usage: remove <id>")?),
        Some("list") => cmd_list(paths, json),
        Some("status") => cmd_status(paths, json),
        Some("update-core") => cmd_update_core(paths),
        Some("auto-update") => cmd_auto_update(paths),
        Some("sync-cron") => cmd_sync_cron(paths),
        Some("version") => cmd_version(),
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args, &Paths::from_env()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> Paths {
        let base = std::env::temp_dir().join(format!("oclite-main-{tag}-{}", std::process::id()));
        Paths::under(&base)
    }

    fn cleanup(paths: &Paths) {
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    fn fetched(name: &str) -> Fetched {
        Fetched {
            body: format!("proxies: []\n# {name}\n"),
            name: name.into(),
        }
    }

    fn two_profiles(paths: &Paths) -> State {
        let mut state = State::load(paths).unwrap();
        store(paths, &mut state, "R1", "http://a", &fetched("A")).unwrap();
        store(paths, &mut state, "R2", "http://b", &fetched("B")).unwrap();
        state
    }

    #[test]
    fn the_first_subscription_becomes_active_and_the_second_does_not_steal_it() {
        let paths = temp_paths("store");
        let state = two_profiles(&paths);
        assert_eq!(state.active.as_deref(), Some("R1"));
        assert_eq!(
            State::load(&paths).unwrap().profiles.len(),
            2,
            "saved to UCI"
        );
        assert!(
            fs::read_to_string(paths.profile("R2"))
                .unwrap()
                .contains("# B")
        );
        cleanup(&paths);
    }

    #[test]
    fn storing_the_same_id_again_updates_the_file_but_keeps_the_name() {
        let paths = temp_paths("update");
        let mut state = State::load(&paths).unwrap();
        store(&paths, &mut state, "R1", "http://a", &fetched("old")).unwrap();
        store(&paths, &mut state, "R1", "http://a", &fetched("new")).unwrap();
        assert_eq!(state.profiles.len(), 1);
        assert_eq!(
            state.profiles[0].name, "old",
            "an edited name survives updates"
        );
        assert!(
            fs::read_to_string(paths.profile("R1"))
                .unwrap()
                .contains("# new")
        );
        cleanup(&paths);
    }

    #[test]
    fn editing_the_name_only_needs_no_download() {
        let paths = temp_paths("editname");
        two_profiles(&paths);
        cmd_edit(&paths, "R1", "  我的机场  ", "http://a").unwrap();
        let state = State::load(&paths).unwrap();
        assert_eq!(state.profiles[0].name, "我的机场");
        assert_eq!(state.profiles[0].url, "http://a");
        assert!(
            cmd_edit(&paths, "R1", "  ", "http://a").is_err(),
            "empty name"
        );
        assert!(cmd_edit(&paths, "nope", "x", "http://a").is_err());
        cleanup(&paths);
    }

    #[test]
    fn a_new_url_that_cannot_be_downloaded_changes_nothing() {
        let paths = temp_paths("editurl");
        two_profiles(&paths);
        // nothing listens on port 1, so the download fails for real
        assert!(cmd_edit(&paths, "R1", "renamed", "http://127.0.0.1:1/sub").is_err());
        let state = State::load(&paths).unwrap();
        assert_eq!(
            (
                state.profiles[0].name.as_str(),
                state.profiles[0].url.as_str()
            ),
            ("A", "http://a")
        );
        assert!(
            fs::read_to_string(paths.profile("R1"))
                .unwrap()
                .contains("# A")
        );
        cleanup(&paths);
    }

    #[test]
    fn use_rejects_an_unknown_subscription_and_accepts_a_known_one() {
        let paths = temp_paths("use");
        two_profiles(&paths);
        assert!(cmd_use(&paths, "nope").is_err());
        cmd_use(&paths, "R2").unwrap();
        assert_eq!(State::load(&paths).unwrap().active.as_deref(), Some("R2"));
        cleanup(&paths);
    }

    #[test]
    fn removing_the_active_subscription_activates_the_next_and_deletes_the_file() {
        let paths = temp_paths("remove");
        two_profiles(&paths);
        assert!(cmd_remove(&paths, "nope").is_err());
        cmd_remove(&paths, "R1").unwrap();
        let state = State::load(&paths).unwrap();
        assert_eq!(state.active.as_deref(), Some("R2"));
        assert_eq!(state.profiles.len(), 1);
        assert!(!paths.profile("R1").exists());
        cmd_remove(&paths, "R2").unwrap();
        assert_eq!(State::load(&paths).unwrap().active, None);
        cleanup(&paths);
    }

    #[test]
    fn list_json_marks_the_active_one_and_carries_the_download_time() {
        let paths = temp_paths("listjson");
        let state = two_profiles(&paths);
        let list = list_json(&paths, &state);
        assert_eq!(list[0]["id"], "R1");
        assert_eq!(list[0]["active"], true);
        assert_eq!(list[1]["active"], false);
        assert!(list[1]["updated"].as_u64().unwrap() > 1_700_000_000);
        cleanup(&paths);
    }

    #[test]
    fn status_json_reports_not_running_when_nothing_listens() {
        let paths = temp_paths("statusjson");
        let mut state = two_profiles(&paths);
        state.port = 1; // nothing listens on port 1
        let s = status_json(&paths, &state);
        assert_eq!(s["running"], false);
        assert_eq!(s["tun"], false);
        assert_eq!(s["port"], 1);
        assert_eq!(s["active"]["name"], "A");
        cleanup(&paths);
    }

    #[test]
    fn unknown_commands_print_the_usage() {
        let paths = temp_paths("usage");
        let err = dispatch(&["bogus".into()], &paths).unwrap_err().to_string();
        for cmd in [
            "run",
            "remove <id>",
            "update-core",
            "auto-update",
            "sync-cron",
        ] {
            assert!(err.contains(cmd), "usage lists {cmd}");
        }
    }
}
