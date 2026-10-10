use std::{
    env,
    path::{Path, PathBuf},
};

const DEFAULT_DIR: &str = "/etc/openwrt-clash-lite";
const DEFAULT_RUN_DIR: &str = "/tmp/openwrt-clash-lite";
const MIHOMO_FALLBACKS: [&str; 2] = ["/usr/bin/mihomo", "/etc/openclash/core/clash_meta"];

/// Persistent files live in `dir` (flash); logs and the generated runtime config in `run_dir` (RAM).
/// Both can be overridden by environment variables, which is also how tests stay off the real system.
#[derive(Clone)]
pub struct Paths {
    pub dir: PathBuf,
    pub run_dir: PathBuf,
}

fn env_path(name: &str, default: &str) -> PathBuf {
    env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

impl Paths {
    pub fn from_env() -> Self {
        Self {
            dir: env_path("OCLITE_DIR", DEFAULT_DIR),
            run_dir: env_path("OCLITE_RUN_DIR", DEFAULT_RUN_DIR),
        }
    }

    pub fn state_file(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.dir.join("profiles")
    }

    pub fn profile(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(format!("{id}.yaml"))
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub fn ui_dir(&self) -> PathBuf {
        self.dir.join("ui")
    }

    pub fn runtime_config(&self) -> PathBuf {
        self.run_dir.join("runtime.yaml")
    }

    pub fn log_file(&self) -> PathBuf {
        self.run_dir.join("mihomo.log")
    }

    /// `OCLITE_MIHOMO`, then our own dir, then common install locations (incl. OpenClash's core).
    pub fn mihomo_bin(&self) -> Option<PathBuf> {
        let mut candidates: Vec<PathBuf> = env::var_os("OCLITE_MIHOMO")
            .map(PathBuf::from)
            .into_iter()
            .collect();
        candidates.push(self.dir.join("mihomo"));
        candidates.extend(MIHOMO_FALLBACKS.iter().map(PathBuf::from));
        candidates.into_iter().find(|p| is_file(p))
    }
}

fn is_file(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_derived_from_the_two_roots() {
        let p = Paths {
            dir: "/a".into(),
            run_dir: "/r".into(),
        };
        assert_eq!(p.profile("R1"), Path::new("/a/profiles/R1.yaml"));
        assert_eq!(p.runtime_config(), Path::new("/r/runtime.yaml"));
        assert_eq!(p.log_file(), Path::new("/r/mihomo.log"));
    }

    #[test]
    fn mihomo_bin_is_none_when_nothing_exists() {
        let p = Paths {
            dir: "/definitely/not/here".into(),
            run_dir: "/r".into(),
        };
        if !MIHOMO_FALLBACKS.iter().any(|f| Path::new(f).is_file())
            && env::var_os("OCLITE_MIHOMO").is_none()
        {
            assert!(p.mihomo_bin().is_none());
        }
    }
}
