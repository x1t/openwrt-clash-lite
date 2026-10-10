use std::{env, fs, path::PathBuf, time::UNIX_EPOCH};

const DEFAULT_DIR: &str = "/etc/openwrt-clash-lite";
const DEFAULT_RUN_DIR: &str = "/tmp/openwrt-clash-lite";
const DEFAULT_UCI: &str = "/etc/config/openwrt-clash-lite";
/// Yacd as installed by the ipk (read-only); `<dir>/ui` is checked first so it can be replaced.
const PACKAGED_UI: &str = "/usr/share/openwrt-clash-lite/ui";
const MIHOMO_FALLBACKS: [&str; 2] = ["/usr/bin/mihomo", "/etc/openclash/core/clash_meta"];

/// Persistent files live in `dir` (flash); logs and the generated runtime config in `run_dir` (RAM);
/// settings and subscriptions in the UCI file. Environment variables override all three, which is
/// also how tests stay off the real system.
#[derive(Clone)]
pub struct Paths {
    pub dir: PathBuf,
    pub run_dir: PathBuf,
    pub uci_file: PathBuf,
}

fn env_path(name: &str, default: &str) -> PathBuf {
    env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

impl Paths {
    pub fn from_env() -> Self {
        Self {
            dir: env_path("OCLITE_DIR", DEFAULT_DIR),
            run_dir: env_path("OCLITE_RUN_DIR", DEFAULT_RUN_DIR),
            uci_file: env_path("OCLITE_UCI", DEFAULT_UCI),
        }
    }

    /// Everything below one directory (tests).
    #[cfg(test)]
    pub fn under(base: &std::path::Path) -> Self {
        Self {
            dir: base.join("etc"),
            run_dir: base.join("run"),
            uci_file: base.join("config").join("openwrt-clash-lite"),
        }
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.dir.join("profiles")
    }

    pub fn profile(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(format!("{id}.yaml"))
    }

    /// Unix time of the last download of a subscription (the profile file's mtime).
    pub fn profile_updated(&self, id: &str) -> Option<u64> {
        let modified = fs::metadata(self.profile(id)).ok()?.modified().ok()?;
        modified
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub fn ui_dir(&self) -> Option<PathBuf> {
        [self.dir.join("ui"), PathBuf::from(PACKAGED_UI)]
            .into_iter()
            .find(|d| d.join("index.html").is_file())
    }

    pub fn runtime_config(&self) -> PathBuf {
        self.run_dir.join("runtime.yaml")
    }

    pub fn log_file(&self) -> PathBuf {
        self.run_dir.join("mihomo.log")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.run_dir.join("mihomo.pid")
    }

    /// `OCLITE_MIHOMO`, then our own dir, then common install locations (incl. OpenClash's core).
    pub fn mihomo_bin(&self) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = env::var_os("OCLITE_MIHOMO")
            .map(PathBuf::from)
            .into_iter()
            .collect();
        candidates.push(self.own_mihomo());
        candidates.extend(MIHOMO_FALLBACKS.iter().map(PathBuf::from));
        candidates.into_iter().find(|p| p.is_file())
    }

    /// Where `update-core` installs mihomo.
    pub fn own_mihomo(&self) -> PathBuf {
        self.dir.join("mihomo")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, time::SystemTime};

    #[test]
    fn paths_are_derived_from_the_roots() {
        let p = Paths {
            dir: "/a".into(),
            run_dir: "/r".into(),
            uci_file: "/c/x".into(),
        };
        assert_eq!(p.profile("R1"), Path::new("/a/profiles/R1.yaml"));
        assert_eq!(p.runtime_config(), Path::new("/r/runtime.yaml"));
        assert_eq!(p.log_file(), Path::new("/r/mihomo.log"));
        assert_eq!(p.own_mihomo(), Path::new("/a/mihomo"));
    }

    #[test]
    fn mihomo_bin_is_none_when_nothing_exists() {
        let p = Paths::under(Path::new("/definitely/not/here"));
        if !MIHOMO_FALLBACKS.iter().any(|f| Path::new(f).is_file())
            && env::var_os("OCLITE_MIHOMO").is_none()
        {
            assert!(p.mihomo_bin().is_none());
        }
    }

    #[test]
    fn our_own_ui_dir_wins_and_the_update_time_is_the_file_mtime() {
        let base = env::temp_dir().join(format!("oclite-cfg-{}", std::process::id()));
        let p = Paths::under(&base);
        fs::create_dir_all(p.dir.join("ui")).unwrap();
        fs::write(p.dir.join("ui").join("index.html"), "x").unwrap();
        assert_eq!(p.ui_dir(), Some(p.dir.join("ui")));
        assert_eq!(p.profile_updated("R1"), None);
        fs::create_dir_all(p.profiles_dir()).unwrap();
        fs::write(p.profile("R1"), "proxies: []\n").unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let age = now - p.profile_updated("R1").unwrap();
        assert!(age < 60, "mtime is now");
        let _ = fs::remove_dir_all(&base);
    }
}
