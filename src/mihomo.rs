//! `update-core`: installs the latest mihomo release for this CPU as `<dir>/mihomo`.
//! The download is decompressed while streaming to a temp file, checked by running `-v`,
//! and only then renamed over the old binary, so a failed update leaves the old core working.
use crate::{config::Paths, state::Res, sys};
use serde_json::Value;
use std::{fs, io, path::Path, time::Duration};
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

/// (tag, download url) of the plain `mihomo-linux-<arch>-<tag>.gz` asset.
fn pick_asset(release: &Value, arch: &str) -> Option<(String, String)> {
    let tag = release["tag_name"].as_str()?;
    let want = format!("mihomo-linux-{arch}-{tag}.gz");
    let asset = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(want.as_str()))?;
    let url = asset["browser_download_url"].as_str()?;
    Some((tag.to_string(), url.to_string()))
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

pub fn update(paths: &Paths) -> Res<String> {
    let release: Value = agent().get(LATEST).call()?.body_mut().read_json()?;
    let (tag, url) = pick_asset(&release, arch()).ok_or(format!(
        "no mihomo-linux-{} asset in the latest release",
        arch()
    ))?;
    fs::create_dir_all(&paths.dir)?;
    let target = paths.own_mihomo();
    let tmp = target.with_extension("new");
    let result = download_gz(&url, &tmp).and_then(|()| version_of(&tmp));
    match result {
        Ok(version) => {
            fs::rename(&tmp, &target)?;
            Ok(format!("{tag}: {version}"))
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
            .map(|n| json!({"name": n, "browser_download_url": format!("https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/{n}")}))
            .collect();
        json!({"tag_name": "v1.19.32", "assets": assets})
    }

    #[test]
    fn picks_the_plain_gz_for_each_cpu() {
        for arch in ["arm64", "armv7", "amd64"] {
            let (tag, url) = pick_asset(&release(), arch).unwrap();
            assert_eq!(tag, "v1.19.32");
            assert!(
                url.ends_with(&format!("/mihomo-linux-{arch}-v1.19.32.gz")),
                "{url}"
            );
        }
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
