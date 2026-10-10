//! mihomo needs its GeoIP/GeoSite databases before it can open the API. On many routers fetching
//! them from GitHub is slow or blocked (a fresh start failed once after a ~1 minute timeout), so
//! reuse copies that are already on the box, e.g. the ones OpenClash ships.
use crate::sys;
use std::{
    fs,
    path::{Path, PathBuf},
};

const GEO_FILES: [&str; 2] = ["Country.mmdb", "GeoSite.dat"];
const DEFAULT_SEED_DIR: &str = "/etc/openclash";

/// `OCLITE_GEO_SEED` (for tests), else OpenClash's directory.
pub fn seed_dirs() -> Vec<PathBuf> {
    match std::env::var_os("OCLITE_GEO_SEED") {
        Some(dir) => vec![PathBuf::from(dir)],
        None => vec![PathBuf::from(DEFAULT_SEED_DIR)],
    }
}

/// Links every missing geo file in `data_dir` to the first `seed_dirs` entry that has it.
/// Returns the file names that were linked. A dangling link left by a removed seed is replaced.
pub fn seed(data_dir: &Path, seed_dirs: &[PathBuf]) -> Vec<&'static str> {
    GEO_FILES
        .iter()
        .copied()
        .filter(|name| {
            let dst = data_dir.join(name);
            if dst.exists() {
                return false;
            }
            let _ = fs::remove_file(&dst);
            seed_dirs
                .iter()
                .map(|d| d.join(name))
                .find(|src| src.is_file())
                .is_some_and(|src| sys::link_file(&src, &dst).is_ok())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("oclite-geo-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (data, seed) = (base.join("data"), base.join("seed"));
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&seed).unwrap();
        (base, data, seed)
    }

    #[test]
    fn missing_files_are_taken_from_the_seed_dir() {
        let (base, data, seed_dir) = dirs("missing");
        fs::write(seed_dir.join("Country.mmdb"), "mmdb").unwrap();
        fs::write(seed_dir.join("GeoSite.dat"), "site").unwrap();
        assert_eq!(
            seed(&data, &[seed_dir]),
            vec!["Country.mmdb", "GeoSite.dat"]
        );
        assert_eq!(
            fs::read_to_string(data.join("GeoSite.dat")).unwrap(),
            "site"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn existing_files_are_never_replaced() {
        let (base, data, seed_dir) = dirs("existing");
        fs::write(data.join("Country.mmdb"), "mine").unwrap();
        fs::write(seed_dir.join("Country.mmdb"), "theirs").unwrap();
        assert!(seed(&data, &[seed_dir]).is_empty());
        assert_eq!(
            fs::read_to_string(data.join("Country.mmdb")).unwrap(),
            "mine"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn nothing_happens_when_the_seed_has_no_such_file() {
        let (base, data, seed_dir) = dirs("none");
        assert!(seed(&data, &[seed_dir, PathBuf::from("/does/not/exist")]).is_empty());
        assert!(!data.join("Country.mmdb").exists());
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_is_replaced_by_a_working_one() {
        let (base, data, seed_dir) = dirs("dangling");
        std::os::unix::fs::symlink("/gone/Country.mmdb", data.join("Country.mmdb")).unwrap();
        fs::write(seed_dir.join("Country.mmdb"), "fresh").unwrap();
        assert_eq!(seed(&data, &[seed_dir]), vec!["Country.mmdb"]);
        assert_eq!(
            fs::read_to_string(data.join("Country.mmdb")).unwrap(),
            "fresh"
        );
        let _ = fs::remove_dir_all(base);
    }
}
