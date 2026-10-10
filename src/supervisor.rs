//! Runs mihomo under supervision: compose config, start, watch, restart on crash with back-off,
//! and on SIGTERM stop it gracefully so it removes its own routes and firewall rules.
use crate::{
    api,
    config::Paths,
    geo,
    merge::{self, Overrides},
    state::{Res, State},
    sys, takeover,
};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

const TICK: Duration = Duration::from_millis(500);
const STOP_GRACE: Duration = Duration::from_secs(10);
/// A run at least this long counts as healthy and resets the failure counter.
const STABLE_AFTER: Duration = Duration::from_secs(60);
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// Longest wait for providers before the TUN goes on anyway (a download may keep failing).
const PROVIDER_WAIT: Duration = Duration::from_secs(60);

enum Exit {
    Stop,
    Died(String),
}

fn info(msg: &str) {
    eprintln!("[openwrt-clash-lite] {msg}");
}

/// 2, 4, 8, 16, 30, 30 ... seconds for the 1st, 2nd, 3rd ... consecutive quick failure.
pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs(2u64.pow(failures.clamp(1, 5)).min(30))
}

fn open_log(path: &Path) -> Res<File> {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    Ok(OpenOptions::new().create(true).append(true).open(path)?)
}

/// Writes the merged config to RAM (it contains the subscription's node secrets).
fn write_runtime(paths: &Paths, state: &State, profile: &Path) -> Res<()> {
    let subscription = fs::read_to_string(profile)
        .map_err(|e| format!("cannot read profile {}: {e}", profile.display()))?;
    let ui_dir = paths.ui_dir().map(|d| d.to_string_lossy().into_owned());
    let config = merge::compose(
        &subscription,
        &Overrides {
            controller: &state.listen(),
            secret: &state.secret,
            log_level: &state.log_level,
            ui_dir: ui_dir.as_deref(),
        },
    );
    fs::create_dir_all(&paths.run_dir)?;
    fs::write(paths.runtime_config(), config)?;
    sys::restrict_file(&paths.runtime_config());
    Ok(())
}

fn spawn_core(paths: &Paths, bin: &Path) -> Res<Child> {
    fs::create_dir_all(paths.data_dir())?;
    let linked = geo::seed(&paths.data_dir(), &geo::seed_dirs());
    if !linked.is_empty() {
        info(&format!("reusing existing geo data: {}", linked.join(", ")));
    }
    let log = open_log(&paths.log_file())?;
    let mut cmd = Command::new(bin);
    cmd.arg("-d")
        .arg(paths.data_dir())
        .arg("-f")
        .arg(paths.runtime_config());
    // mihomo refuses an external-ui outside its home dir unless the path is in SAFE_PATHS
    if let Some(ui) = paths.ui_dir() {
        cmd.env("SAFE_PATHS", ui);
    }
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    sys::die_with_parent(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
    fs::write(paths.pid_file(), child.id().to_string())?;
    Ok(child)
}

/// SIGTERM first so mihomo cleans up after itself; SIGKILL only if it hangs.
fn stop_core(child: &mut Child) {
    sys::terminate(child.id());
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    info("mihomo ignored SIGTERM, killing it");
    let _ = child.kill();
    let _ = child.wait();
}

/// Start-up of one mihomo run: API up -> providers loaded (or PROVIDER_WAIT passed) -> TUN on.
/// Until the TUN is on, LAN traffic keeps flowing directly instead of being dropped by mihomo.
#[derive(Default)]
struct Startup {
    ready_at: Option<Instant>,
    tun_requested: bool,
    active: bool,
}

impl Startup {
    fn step(&mut self, state: &State) {
        let (ctl, secret) = (state.controller(), state.secret.as_str());
        match self.ready_at {
            None if api::ready(&ctl, secret) => {
                self.ready_at = Some(Instant::now());
                info("mihomo is ready; waiting for its providers before taking over");
            }
            Some(at) if !self.tun_requested => self.request_tun(&ctl, secret, at.elapsed()),
            Some(_) if !self.active && api::tun_enabled(&ctl, secret).unwrap_or(false) => {
                self.active = true;
                info("takeover active (TUN is up)");
            }
            _ => {}
        }
    }

    fn request_tun(&mut self, ctl: &str, secret: &str, waited: Duration) {
        let loaded = api::providers_loaded(ctl, secret).unwrap_or(false);
        if !loaded && waited < PROVIDER_WAIT {
            return;
        }
        if !loaded {
            info("providers still loading after 60s (download failing?); taking over anyway");
        }
        match api::enable_tun(ctl, secret) {
            Ok(()) => self.tun_requested = true,
            Err(e) => info(&format!("cannot switch the TUN on: {e}")),
        }
    }
}

fn monitor(child: &mut Child, state: &State, stop: &Receiver<()>) -> Exit {
    let mut startup = Startup::default();
    loop {
        match stop.recv_timeout(TICK) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return Exit::Stop,
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Exit::Died(status.to_string());
        }
        startup.step(state);
    }
}

fn supervise(
    paths: &Paths,
    state: &State,
    bin: &Path,
    profile: &Path,
    stop: &Receiver<()>,
) -> Res<()> {
    let mut failures = 0u32;
    loop {
        write_runtime(paths, state, profile)?;
        let mut child = spawn_core(paths, bin)?;
        info(&format!("started mihomo (pid {})", child.id()));
        let started = Instant::now();
        match monitor(&mut child, state, stop) {
            Exit::Stop => {
                stop_core(&mut child);
                return Ok(());
            }
            Exit::Died(status) => {
                takeover::clean_stale();
                failures = if started.elapsed() >= STABLE_AFTER {
                    1
                } else {
                    failures + 1
                };
                let wait = backoff(failures);
                info(&format!(
                    "mihomo exited ({status}); restarting in {}s",
                    wait.as_secs()
                ));
                if stop.recv_timeout(wait).is_ok() {
                    return Ok(());
                }
            }
        }
    }
}

pub fn run(paths: &Paths) -> Res<()> {
    let state = State::load(paths)?;
    state.save(paths)?; // keeps the generated secret stable across runs
    let profile = state
        .active_profile()
        .map(|p| paths.profile(&p.id))
        .filter(|p| p.is_file())
        .ok_or("no active subscription: run `openwrt-clash-lite add <subscription-url>` first")?;
    let bin = paths.mihomo_bin().ok_or(
        "mihomo binary not found: put it at <dir>/mihomo or /usr/bin/mihomo, or set OCLITE_MIHOMO",
    )?;
    let conflicts = takeover::find_conflicts(&sys::list_procs());
    if !conflicts.is_empty() {
        return Err(takeover::conflict_message(&conflicts).into());
    }
    let (tx, stop) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })?;
    let guard = takeover::prepare()?;
    let result = supervise(paths, &state, &bin, &profile, &stop);
    takeover::release(guard);
    let _ = fs::remove_file(paths.runtime_config());
    let _ = fs::remove_file(paths.pid_file());
    info("stopped; takeover rules removed");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps_at_thirty_seconds() {
        let secs: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![2, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn log_is_rotated_when_it_grows_too_large() {
        let dir = std::env::temp_dir().join(format!("oclite-log-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("mihomo.log");
        fs::write(&log, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        open_log(&log).unwrap();
        assert!(dir.join("mihomo.log.1").exists());
        assert_eq!(
            fs::metadata(&log).unwrap().len(),
            0,
            "fresh log after rotation"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
