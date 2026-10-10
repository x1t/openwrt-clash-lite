mod api;
mod config;
mod geo;
mod merge;
mod state;
mod subscription;
mod supervisor;
mod sys;
mod takeover;

use config::Paths;
use state::{Profile, Res, State, now_secs};
use std::{
    fs,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};
use subscription::Fetched;

const USAGE: &str = "openwrt-clash-lite <command>
  run             run mihomo with transparent takeover (foreground, used by procd)
  add <url>       download a subscription; it becomes active if none is
  update [id]     re-download the active (or the given) subscription
  use <id>        make a subscription active (restart the service to apply)
  list            list subscriptions
  status          show whether mihomo is up
  version";

fn new_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("R{millis:x}")
}

/// Saves a downloaded subscription under `id`, registers it, and activates it if nothing is active.
fn store(paths: &Paths, state: &mut State, id: &str, url: &str, fetched: &Fetched) -> Res<()> {
    fs::create_dir_all(paths.profiles_dir())?;
    fs::write(paths.profile(id), &fetched.body)?;
    match state.profiles.iter_mut().find(|p| p.id == id) {
        Some(p) => {
            p.name = fetched.name.clone();
            p.updated = now_secs();
        }
        None => state.profiles.push(Profile {
            id: id.to_string(),
            name: fetched.name.clone(),
            url: url.to_string(),
            updated: now_secs(),
        }),
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

fn cmd_update(paths: &Paths, id: Option<&str>) -> Res<()> {
    let mut state = State::load(paths)?;
    let id = id
        .map(str::to_string)
        .or_else(|| state.active.clone())
        .ok_or("no subscription yet")?;
    let url = state
        .profiles
        .iter()
        .find(|p| p.id == id)
        .ok_or(format!("unknown subscription {id}"))?
        .url
        .clone();
    store(paths, &mut state, &id, &url, &subscription::fetch(&url)?)?;
    println!("updated {id}; restart the service to apply");
    Ok(())
}

fn cmd_use(paths: &Paths, id: &str) -> Res<()> {
    let mut state = State::load(paths)?;
    if !state.profiles.iter().any(|p| p.id == id) {
        return Err(format!("unknown subscription {id}").into());
    }
    state.active = Some(id.to_string());
    state.save(paths)?;
    println!("{id} is now active; restart the service to apply");
    Ok(())
}

fn cmd_list(paths: &Paths) -> Res<()> {
    let state = State::load(paths)?;
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

fn cmd_status(paths: &Paths) -> Res<()> {
    let state = State::load(paths)?;
    match api::version(&state.controller, &state.secret) {
        Ok(v) => {
            let tun = api::tun_enabled(&state.controller, &state.secret).unwrap_or(false);
            println!(
                "running: mihomo {v}, tun {}",
                if tun { "on" } else { "off" }
            );
        }
        Err(_) => println!(
            "not running (controller {} does not answer)",
            state.controller
        ),
    }
    Ok(())
}

fn cmd_version() -> Res<()> {
    println!("openwrt-clash-lite {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

fn dispatch(args: &[String], paths: &Paths) -> Res<()> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("run") => supervisor::run(paths),
        Some("add") => cmd_add(paths, arg(1).ok_or("usage: add <url>")?),
        Some("update") => cmd_update(paths, arg(1)),
        Some("use") => cmd_use(paths, arg(1).ok_or("usage: use <id>")?),
        Some("list") => cmd_list(paths),
        Some("status") => cmd_status(paths),
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
        Paths {
            dir: base.join("etc"),
            run_dir: base.join("run"),
        }
    }

    fn fetched(name: &str) -> Fetched {
        Fetched {
            body: format!("proxies: []\n# {name}\n"),
            name: name.into(),
        }
    }

    #[test]
    fn the_first_subscription_becomes_active_and_the_second_does_not_steal_it() {
        let paths = temp_paths("store");
        let mut state = State::load(&paths).unwrap();
        store(&paths, &mut state, "R1", "http://a", &fetched("A")).unwrap();
        store(&paths, &mut state, "R2", "http://b", &fetched("B")).unwrap();
        assert_eq!(state.active.as_deref(), Some("R1"));
        assert_eq!(state.profiles.len(), 2);
        assert!(
            fs::read_to_string(paths.profile("R2"))
                .unwrap()
                .contains("# B")
        );
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn storing_the_same_id_again_updates_instead_of_duplicating() {
        let paths = temp_paths("update");
        let mut state = State::load(&paths).unwrap();
        store(&paths, &mut state, "R1", "http://a", &fetched("old")).unwrap();
        store(&paths, &mut state, "R1", "http://a", &fetched("new")).unwrap();
        assert_eq!(state.profiles.len(), 1);
        assert_eq!(state.profiles[0].name, "new");
        assert!(
            fs::read_to_string(paths.profile("R1"))
                .unwrap()
                .contains("# new")
        );
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn use_rejects_an_unknown_subscription_and_accepts_a_known_one() {
        let paths = temp_paths("use");
        let mut state = State::load(&paths).unwrap();
        store(&paths, &mut state, "R1", "http://a", &fetched("A")).unwrap();
        store(&paths, &mut state, "R2", "http://b", &fetched("B")).unwrap();
        assert!(cmd_use(&paths, "nope").is_err());
        cmd_use(&paths, "R2").unwrap();
        assert_eq!(State::load(&paths).unwrap().active.as_deref(), Some("R2"));
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn unknown_commands_print_the_usage() {
        let paths = temp_paths("usage");
        let err = dispatch(&["bogus".into()], &paths).unwrap_err().to_string();
        assert!(err.contains("run") && err.contains("add <url>"));
    }
}
