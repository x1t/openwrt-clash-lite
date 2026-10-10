use crate::config::Paths;
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::RandomState,
    fs,
    hash::{BuildHasher, Hasher},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

const DEFAULT_CONTROLLER: &str = "127.0.0.1:19097";
const DEFAULT_LOG_LEVEL: &str = "warning";

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub url: String,
    pub updated: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub secret: String,
    /// `host:port` the mihomo REST API listens on; edit state.json to expose the panel on the LAN.
    pub controller: String,
    /// mihomo log level: silent, error, warning, info or debug. Raise it to diagnose problems.
    pub log_level: String,
    pub active: Option<String>,
    pub profiles: Vec<Profile>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            secret: String::new(),
            controller: DEFAULT_CONTROLLER.into(),
            log_level: DEFAULT_LOG_LEVEL.into(),
            active: None,
            profiles: Vec::new(),
        }
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn random_hex() -> String {
    let a = RandomState::new().build_hasher().finish();
    let b = RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")
}

impl State {
    /// A missing file means a first run; a file that exists but cannot be read or parsed is an
    /// error, never a silent reset (the next save would wipe the user's subscription list).
    pub fn load(paths: &Paths) -> Res<Self> {
        let file = paths.state_file();
        let mut state: State = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not valid ({e}); fix or delete it", file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(format!("cannot read {}: {e}", file.display()).into()),
        };
        if state.secret.is_empty() {
            state.secret = random_hex();
        }
        Ok(state)
    }

    /// Writes to a temp file first so a power cut cannot leave a half-written state.
    pub fn save(&self, paths: &Paths) -> Res<()> {
        fs::create_dir_all(paths.profiles_dir())?;
        let tmp = paths.state_file().with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, paths.state_file())?;
        Ok(())
    }

    pub fn active_profile_path(&self, paths: &Paths) -> Option<PathBuf> {
        self.active.as_deref().map(|id| paths.profile(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> Paths {
        let base = std::env::temp_dir().join(format!("oclite-{tag}-{}", std::process::id()));
        Paths {
            dir: base.join("etc"),
            run_dir: base.join("run"),
        }
    }

    #[test]
    fn a_missing_state_file_yields_defaults_with_a_secret() {
        let s = State::load(&temp_paths("missing")).unwrap();
        assert_eq!(s.secret.len(), 32);
        assert_eq!(s.controller, DEFAULT_CONTROLLER);
        assert_eq!(s.log_level, "warning");
        assert!(s.active.is_none());
    }

    #[test]
    fn state_survives_a_save_and_load_round_trip() {
        let paths = temp_paths("roundtrip");
        let mut s = State::load(&paths).unwrap();
        s.profiles.push(Profile {
            id: "R1".into(),
            name: "n".into(),
            url: "u".into(),
            updated: 7,
        });
        s.active = Some("R1".into());
        s.save(&paths).unwrap();
        let back = State::load(&paths).unwrap();
        assert_eq!(back.secret, s.secret);
        assert_eq!(back.active.as_deref(), Some("R1"));
        assert_eq!(back.profiles[0].updated, 7);
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn a_corrupt_state_file_is_an_error_and_is_left_untouched() {
        let paths = temp_paths("corrupt");
        fs::create_dir_all(&paths.dir).unwrap();
        // duplicate key: exactly what hand-editing a pretty-printed file can produce
        let broken = r#"{"log_level":"info","log_level":"warning","active":"R1"}"#;
        fs::write(paths.state_file(), broken).unwrap();
        let err = State::load(&paths)
            .err()
            .expect("must not silently reset")
            .to_string();
        assert!(
            err.contains("not valid") && err.contains("fix or delete"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(paths.state_file()).unwrap(),
            broken,
            "file untouched"
        );
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn unknown_or_partial_json_still_loads() {
        let paths = temp_paths("partial");
        fs::create_dir_all(&paths.dir).unwrap();
        fs::write(paths.state_file(), r#"{"active":"X","extra":1}"#).unwrap();
        let s = State::load(&paths).unwrap();
        assert_eq!(s.active.as_deref(), Some("X"));
        assert_eq!(s.controller, DEFAULT_CONTROLLER);
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }
}
