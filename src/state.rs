//! Settings and subscriptions, stored in UCI (`/etc/config/openwrt-clash-lite`) so LuCI forms,
//! `uci set` and sysupgrade backups work with them like with any other OpenWrt package.
use crate::{
    config::Paths,
    sys,
    uci::{self, Section},
};
use serde::Serialize;
use std::{
    collections::hash_map::RandomState,
    fs,
    hash::{BuildHasher, Hasher},
};

pub type Res<T> = Result<T, Box<dyn std::error::Error>>;

const DEFAULT_PORT: u16 = 19097;
const DEFAULT_LOG_LEVEL: &str = "warning";
const DEFAULT_UPDATE_TIME: &str = "06:10";

#[derive(Serialize, Clone, Default, Debug, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, PartialEq)]
pub struct State {
    pub secret: String,
    /// Port of the mihomo REST API and Yacd; it listens on all interfaces (LAN access to the
    /// panel), protected by `secret`, and OpenWrt's firewall keeps the WAN side closed.
    pub port: u16,
    /// mihomo log level: silent, error, warning, info or debug.
    pub log_level: String,
    /// Re-download the active subscription every day at `update_time` (HH:MM, cron job).
    pub auto_update: bool,
    pub update_time: String,
    pub active: Option<String>,
    pub profiles: Vec<Profile>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            secret: String::new(),
            port: DEFAULT_PORT,
            log_level: DEFAULT_LOG_LEVEL.into(),
            auto_update: false,
            update_time: DEFAULT_UPDATE_TIME.into(),
            active: None,
            profiles: Vec::new(),
        }
    }
}

fn random_hex() -> String {
    let a = RandomState::new().build_hasher().finish();
    let b = RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")
}

fn main_section(state: &mut State, s: &Section) -> Res<()> {
    let text = |key: &str| s.get(key).filter(|v| !v.is_empty()).map(str::to_string);
    state.secret = text("secret").unwrap_or_default();
    if let Some(port) = text("port") {
        state.port = port
            .parse()
            .map_err(|_| format!("port {port:?} is not a port number"))?;
    }
    state.log_level = text("log_level").unwrap_or(state.log_level.clone());
    state.auto_update = text("auto_update").as_deref() == Some("1");
    state.update_time = text("update_time").unwrap_or(state.update_time.clone());
    state.active = text("active");
    Ok(())
}

fn from_sections(sections: &[Section]) -> Res<State> {
    let mut state = State::default();
    for s in sections {
        match (s.kind.as_str(), s.name.as_deref()) {
            ("main", _) => main_section(&mut state, s)?,
            ("subscription", Some(id)) => state.profiles.push(Profile {
                id: id.to_string(),
                name: s.get("name").unwrap_or(id).to_string(),
                url: s.get("url").unwrap_or_default().to_string(),
            }),
            _ => {}
        }
    }
    Ok(state)
}

fn to_sections(state: &State) -> Vec<Section> {
    let opt = |k: &str, v: &str| (k.to_string(), v.to_string());
    let mut main = vec![
        opt("secret", &state.secret),
        opt("port", &state.port.to_string()),
        opt("log_level", &state.log_level),
        opt("auto_update", if state.auto_update { "1" } else { "0" }),
        opt("update_time", &state.update_time),
    ];
    main.extend(state.active.as_deref().map(|a| opt("active", a)));
    let mut sections = vec![Section {
        kind: "main".into(),
        name: Some("main".into()),
        options: main,
    }];
    sections.extend(state.profiles.iter().map(|p| Section {
        kind: "subscription".into(),
        name: Some(p.id.clone()),
        options: vec![opt("name", &p.name), opt("url", &p.url)],
    }));
    sections
}

impl State {
    /// A missing file means a first run; a file that exists but cannot be read or parsed is an
    /// error, never a silent reset (the next save would wipe the user's subscription list).
    pub fn load(paths: &Paths) -> Res<Self> {
        let file = &paths.uci_file;
        let mut state = match fs::read_to_string(file) {
            Ok(text) => uci::parse(&text)
                .and_then(|s| from_sections(&s))
                .map_err(|e| format!("{} is not valid ({e}); fix or delete it", file.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(format!("cannot read {}: {e}", file.display()).into()),
        };
        if state.secret.is_empty() {
            state.secret = random_hex();
        }
        Ok(state)
    }

    /// Writes to a temp file first so a power cut cannot leave a half-written config.
    pub fn save(&self, paths: &Paths) -> Res<()> {
        if let Some(dir) = paths.uci_file.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = paths.uci_file.with_extension("tmp");
        fs::write(&tmp, uci::serialize(&to_sections(self)))?;
        sys::restrict_file(&tmp); // holds the secret and subscription URLs
        fs::rename(tmp, &paths.uci_file)?;
        Ok(())
    }

    /// Where our own client reaches the API.
    pub fn controller(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// The `external-controller` handed to mihomo.
    pub fn listen(&self) -> String {
        format!("0.0.0.0:{}", self.port)
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        let id = self.active.as_deref()?;
        self.profiles.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> Paths {
        let base = std::env::temp_dir().join(format!("oclite-{tag}-{}", std::process::id()));
        Paths::under(&base)
    }

    #[test]
    fn a_missing_config_yields_defaults_with_a_secret() {
        let s = State::load(&temp_paths("missing")).unwrap();
        assert_eq!(s.secret.len(), 32);
        assert_eq!(s.port, DEFAULT_PORT);
        assert_eq!(s.log_level, "warning");
        assert!(s.active.is_none());
        assert_eq!(s.controller(), "127.0.0.1:19097");
        assert_eq!(s.listen(), "0.0.0.0:19097");
    }

    #[test]
    fn state_survives_a_save_and_load_round_trip() {
        let paths = temp_paths("roundtrip");
        let mut s = State::load(&paths).unwrap();
        s.profiles.push(Profile {
            id: "R1".into(),
            name: "it's mine".into(),
            url: "https://x/y?a=1&b=2".into(),
        });
        s.active = Some("R1".into());
        s.port = 9999;
        s.auto_update = true;
        s.update_time = "05:30".into();
        s.save(&paths).unwrap();
        let back = State::load(&paths).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            back.active_profile().map(|p| p.name.as_str()),
            Some("it's mine")
        );
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn the_file_is_what_uci_and_luci_expect() {
        let paths = temp_paths("format");
        let mut s = State::load(&paths).unwrap();
        s.secret = "abc".into();
        s.profiles.push(Profile {
            id: "R1".into(),
            name: "n".into(),
            url: "u".into(),
        });
        s.active = Some("R1".into());
        s.save(&paths).unwrap();
        let text = fs::read_to_string(&paths.uci_file).unwrap();
        assert!(
            text.starts_with("config main 'main'\n\toption secret 'abc'\n\toption port '19097'\n")
        );
        assert!(text.contains("\toption active 'R1'\n"));
        assert!(text.contains("config subscription 'R1'\n\toption name 'n'\n\toption url 'u'\n"));
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn a_corrupt_config_is_an_error_and_is_left_untouched() {
        let paths = temp_paths("corrupt");
        fs::create_dir_all(paths.uci_file.parent().unwrap()).unwrap();
        let broken = "config main 'main'\n\toption active 'R1\n";
        fs::write(&paths.uci_file, broken).unwrap();
        let err = State::load(&paths)
            .expect_err("must not silently reset")
            .to_string();
        assert!(
            err.contains("not valid") && err.contains("fix or delete"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(&paths.uci_file).unwrap(),
            broken,
            "file untouched"
        );
        let _ = fs::remove_dir_all(paths.dir.parent().unwrap());
    }

    #[test]
    fn a_bad_port_is_an_error() {
        let sections = uci::parse("config main 'main'\n\toption port 'abc'\n").unwrap();
        assert!(
            from_sections(&sections)
                .unwrap_err()
                .to_string()
                .contains("port")
        );
    }

    #[test]
    fn a_hand_written_config_with_unknown_sections_still_loads() {
        let text = "config main 'main'\n\toption active 'X'\n\toption extra '1'\n\n\
                    config other\n\toption k 'v'\n\nconfig subscription 'X'\n\toption url 'u'\n";
        let s = from_sections(&uci::parse(text).unwrap()).unwrap();
        assert_eq!(
            s.active_profile().map(|p| p.name.as_str()),
            Some("X"),
            "name falls back to id"
        );
        assert_eq!(s.port, DEFAULT_PORT);
    }
}
