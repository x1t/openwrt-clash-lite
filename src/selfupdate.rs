//! `self-update`: installs the latest openwrt-clash-lite release with opkg, the way OpenClash
//! updates itself: download the ipk for this router's architecture, check its size, dry-run
//! `opkg install --noaction`, then install. The final opkg replaces this process (exec), so the
//! package scripts' `pidof openwrt-clash-lite` only ever sees the service.
use crate::{state::Res, sys};
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use ureq::Agent;

const LATEST: &str = "https://api.github.com/repos/x1t/openwrt-clash-lite/releases/latest";
const DOWNLOAD_DIR: &str = "/tmp/openwrt-clash-lite-update";
const LUCI_MENU: &str = "/usr/share/luci/menu.d/luci-app-openwrt-clash-lite.json";
/// Printed instead of installing; the LuCI page looks for it.
pub const UP_TO_DATE: &str = "already up to date";

fn version_tuple(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

fn is_newer(tag: &str, current: &str) -> bool {
    matches!((version_tuple(tag), version_tuple(current)), (Some(t), Some(c)) if t > c)
}

/// Architecture names from `opkg print-architecture` ("arch <name> <priority>"), best first.
fn opkg_archs(text: &str) -> Vec<String> {
    let mut archs: Vec<(u32, String)> = text
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next()? == "arch").then_some(())?;
            let name = w.next()?.to_string();
            Some((w.next()?.parse().ok()?, name))
        })
        .collect();
    archs.sort_by_key(|a| std::cmp::Reverse(a.0));
    archs.into_iter().map(|(_, n)| n).collect()
}

struct Package {
    name: String,
    url: String,
    size: u64,
}

fn asset(release: &Value, name: &str) -> Option<Package> {
    let a = release["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(name))?;
    Some(Package {
        name: name.to_string(),
        url: a["browser_download_url"].as_str()?.to_string(),
        size: a["size"].as_u64()?,
    })
}

/// The daemon package for the best matching architecture, plus the LuCI page if it is installed.
fn pick(release: &Value, version: &str, archs: &[String], luci: bool) -> Res<Vec<Package>> {
    let core = archs
        .iter()
        .find_map(|a| asset(release, &format!("openwrt-clash-lite_{version}_{a}.ipk")))
        .ok_or(format!(
            "release {version} has no package for {}",
            archs.join(", ")
        ))?;
    let mut packages = vec![core];
    if luci {
        let name = format!("luci-app-openwrt-clash-lite_{version}_all.ipk");
        packages.push(asset(release, &name).ok_or(format!("release {version} has no {name}"))?);
    }
    Ok(packages)
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into()
}

fn download(p: &Package, dir: &Path) -> Res<PathBuf> {
    let dest = dir.join(&p.name);
    let resp = agent().get(&p.url).call()?;
    io::copy(
        &mut resp.into_body().into_reader(),
        &mut fs::File::create(&dest)?,
    )?;
    let got = fs::metadata(&dest)?.len();
    if got != p.size {
        return Err(format!("{}: downloaded {got} bytes, expected {}", p.name, p.size).into());
    }
    Ok(dest)
}

/// Returns only when there is nothing to do or something failed; a successful install replaces
/// this process with `opkg install`.
pub fn run() -> Res<String> {
    let current = env!("CARGO_PKG_VERSION");
    let release: Value = agent().get(LATEST).call()?.body_mut().read_json()?;
    let tag = release["tag_name"].as_str().ok_or("no release found")?;
    if !is_newer(tag, current) {
        return Ok(format!("{UP_TO_DATE}: {current}"));
    }
    let version = tag.trim_start_matches('v');
    let (_, arch_text) = sys::run_cmd("opkg", &["print-architecture"]);
    let packages = pick(
        &release,
        version,
        &opkg_archs(&arch_text),
        Path::new(LUCI_MENU).exists(),
    )?;
    let dir = Path::new(DOWNLOAD_DIR);
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir)?;
    let files: Vec<String> = packages
        .iter()
        .map(|p| download(p, dir).map(|f| f.to_string_lossy().into_owned()))
        .collect::<Res<_>>()?;
    let mut args: Vec<&str> = vec!["install", "--noaction"];
    args.extend(files.iter().map(String::as_str));
    let (ok, out) = sys::run_cmd("opkg", &args);
    if !ok {
        return Err(format!("opkg dry run failed, nothing installed: {}", out.trim()).into());
    }
    println!("installing {current} -> {version}: {}", files.join(" "));
    args.remove(1);
    Err(sys::exec("opkg", &args).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("v0.2.0", "0.1.6"));
        assert!(!is_newer("v0.1.6", "0.1.6"));
        assert!(!is_newer("v0.1.5", "0.1.6"));
        assert!(!is_newer("nightly", "0.1.6"));
    }

    /// Real `opkg print-architecture` output of the author's armv7 router.
    #[test]
    fn opkg_architectures_are_ordered_by_priority() {
        let text = "arch all 1\narch noarch 1\narch arm_cortex-a5_vfpv4 10\n";
        assert_eq!(opkg_archs(text), ["arm_cortex-a5_vfpv4", "all", "noarch"]);
        assert!(opkg_archs("").is_empty());
    }

    fn release() -> Value {
        let names = [
            "luci-app-openwrt-clash-lite_0.1.7_all.ipk",
            "openwrt-clash-lite_0.1.7_aarch64_generic.ipk",
            "openwrt-clash-lite_0.1.7_arm_cortex-a5_vfpv4.ipk",
        ];
        let assets: Vec<Value> = names
            .iter()
            .map(|n| json!({"name": n, "size": 3_022_668, "browser_download_url": format!("https://github.com/x1t/openwrt-clash-lite/releases/download/v0.1.7/{n}")}))
            .collect();
        json!({"tag_name": "v0.1.7", "assets": assets})
    }

    #[test]
    fn the_package_for_this_architecture_and_the_luci_page_are_picked() {
        let archs = opkg_archs("arch all 1\narch noarch 1\narch aarch64_generic 10\n");
        let p = pick(&release(), "0.1.7", &archs, true).unwrap();
        assert_eq!(p[0].name, "openwrt-clash-lite_0.1.7_aarch64_generic.ipk");
        assert_eq!(p[1].name, "luci-app-openwrt-clash-lite_0.1.7_all.ipk");
        assert_eq!(p[0].size, 3_022_668);
        let without_luci = pick(&release(), "0.1.7", &archs, false).unwrap();
        assert_eq!(without_luci.len(), 1);
    }

    #[test]
    fn an_unsupported_architecture_is_an_error_before_downloading() {
        let archs = opkg_archs("arch all 1\narch mipsel_24kc 10\n");
        let err = pick(&release(), "0.1.7", &archs, true)
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("mipsel_24kc"), "{err}");
    }
}
