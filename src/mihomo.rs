//! `update-core`: installs the latest mihomo release for this CPU as `<dir>/mihomo`.
//! Checked before anything is replaced: enough free flash, a complete gzip stream, `-v` runs,
//! and the running config passes `-t` with the new core. Only then is the temp file renamed
//! over the old binary (atomic, same directory), so a failed update leaves the old core working.
use crate::{config::Paths, state::Res, sys};
use serde_json::Value;
use std::{fs, io, path::Path, process::Command, time::Duration};
use ureq::Agent;

const LATEST: &str = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";

/// mihomo's name for this build's CPU.
fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "arm") {
        "armv7"
    } else {
        "amd64"
    }
}

struct Asset {
    tag: String,
    url: String,
    gz_size: u64,
}

/// The plain `mihomo-linux-<arch>-<tag>.gz` asset of a release.
fn pick_asset(release: &Value, arch: &str) -> Option<Asset> {
    let tag = release["tag_name"].as_str()?;
    let want = format!("mihomo-linux-{arch}-{tag}.gz");
    let asset = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(want.as_str()))?;
    Some(Asset {
        tag: tag.to_string(),
        url: asset["browser_download_url"].as_str()?.to_string(),
        gz_size: asset["size"].as_u64().unwrap_or(0),
    })
}

/// Free flash needed: the core unpacks to about 2.7x the download (v1.19.32 armv7: 21 MB -> 56 MB);
/// 3x plus 32 MB keeps the overlay from filling up while the old core still exists.
fn space_needed(gz_size: u64) -> u64 {
    gz_size * 3 + 32 * 1024 * 1024
}

fn check_space(dir: &Path, gz_size: u64) -> Res<()> {
    let (Some(free), need) = (sys::free_bytes(dir), space_needed(gz_size)) else {
        return Ok(());
    };
    if free < need {
        return Err(format!(
            "not enough free space in {}: {} MB free, {} MB needed; nothing was changed",
            dir.display(),
            free / 1_048_576,
            need / 1_048_576
        )
        .into());
    }
    Ok(())
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .build()
        .into()
}

fn download_gz(url: &str, dest: &Path) -> Res<()> {
    let resp = agent().get(url).call()?;
    let mut gz = flate2::read::GzDecoder::new(resp.into_body().into_reader());
    io::copy(&mut gz, &mut fs::File::create(dest)?)?;
    sys::make_executable(dest)?;
    Ok(())
}

/// First line of `mihomo -v`, which must mention Mihomo for the file to count as a working core.
fn version_of(bin: &Path) -> Res<String> {
    let (ok, out) = sys::run_cmd(&bin.to_string_lossy(), &["-v"]);
    let line = out.lines().next().unwrap_or_default().to_string();
    if ok && line.contains("Mihomo") {
        Ok(line)
    } else {
        Err(format!("downloaded core does not run: {line}").into())
    }
}

/// When the service runs, the new core must accept its config (`-t`), or it would not start.
fn test_running_config(bin: &Path, paths: &Paths) -> Res<()> {
    let config = paths.runtime_config();
    if !config.is_file() {
        return Ok(());
    }
    let mut cmd = Command::new(bin);
    cmd.arg("-t")
        .arg("-d")
        .arg(paths.data_dir())
        .arg("-f")
        .arg(&config);
    if let Some(ui) = paths.ui_dir() {
        cmd.env("SAFE_PATHS", ui);
    }
    let out = cmd.output()?;
    if out.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    let last = text.lines().last().unwrap_or_default().to_string();
    Err(format!("the new core rejects the current config, keeping the old one: {last}").into())
}

pub fn update(paths: &Paths) -> Res<String> {
    let release: Value = agent().get(LATEST).call()?.body_mut().read_json()?;
    let asset = pick_asset(&release, arch()).ok_or(format!(
        "no mihomo-linux-{} asset in the latest release",
        arch()
    ))?;
    fs::create_dir_all(&paths.dir)?;
    check_space(&paths.dir, asset.gz_size)?;
    let target = paths.own_mihomo();
    let tmp = target.with_extension("new");
    let result = download_gz(&asset.url, &tmp)
        .and_then(|()| version_of(&tmp))
        .and_then(|v| test_running_config(&tmp, paths).map(|()| v));
    match result {
        Ok(version) => {
            fs::rename(&tmp, &target)?;
            Ok(format!("{}: {version}", asset.tag))
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Asset names as published in MetaCubeX/mihomo release v1.19.32.
    fn release() -> Value {
        let names = [
            "mihomo-linux-arm64-v1.19.32.deb",
            "mihomo-linux-arm64-v1.19.32.gz",
            "mihomo-linux-armv7-v1.19.32.gz",
            "mihomo-linux-armv7-v1.19.32.rpm",
            "mihomo-linux-amd64-v1.19.32.gz",
            "mihomo-linux-amd64-v3-v1.19.32.gz",
        ];
        let assets: Vec<Value> = names
            .iter()
            .map(|n| json!({"name": n, "size": 21_167_899, "browser_download_url": format!("https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/{n}")}))
            .collect();
        json!({"tag_name": "v1.19.32", "assets": assets})
    }

    #[test]
    fn picks_the_plain_gz_for_each_cpu() {
        for arch in ["arm64", "armv7", "amd64"] {
            let a = pick_asset(&release(), arch).unwrap();
            assert_eq!(a.tag, "v1.19.32");
            assert_eq!(a.gz_size, 21_167_899);
            let want = format!("/mihomo-linux-{arch}-v1.19.32.gz");
            assert!(a.url.ends_with(&want), "{}", a.url);
        }
    }

    #[test]
    fn the_space_check_covers_the_real_unpacked_size() {
        // v1.19.32 armv7: 21_167_899 bytes download, 56_164_478 bytes unpacked
        assert!(space_needed(21_167_899) > 56_164_478 + 8 * 1024 * 1024);
    }

    #[test]
    fn too_little_free_space_is_refused_before_downloading() {
        let dir = std::env::temp_dir();
        if sys::free_bytes(&dir).is_some() {
            let err = check_space(&dir, u64::MAX / 4).unwrap_err().to_string();
            assert!(err.contains("nothing was changed"), "{err}");
        }
        assert!(check_space(&dir, 1).is_ok());
    }

    #[test]
    fn without_a_running_config_there_is_nothing_to_test() {
        let paths = Paths::under(&std::env::temp_dir().join("oclite-no-runtime"));
        assert!(test_running_config(Path::new("/nonexistent/mihomo"), &paths).is_ok());
    }

    #[test]
    fn a_release_without_our_asset_gives_none() {
        assert!(pick_asset(&release(), "mips").is_none());
        assert!(pick_asset(&json!({"message": "rate limited"}), "arm64").is_none());
    }

    #[test]
    fn this_build_targets_a_known_cpu() {
        assert!(["arm64", "armv7", "amd64"].contains(&arch()));
    }
}
