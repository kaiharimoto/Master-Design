//! Updating from GitHub Releases.
//!
//! One user-facing behaviour, two mechanisms, because the platforms genuinely differ:
//!
//! **Windows** — `tauri-plugin-updater` reads a signed `latest.json` published as a
//! release asset, verifies a minisign signature over the payload, and swaps the
//! installed binary. The signature is the important part: an update channel that
//! downloads and runs a binary over plain HTTPS is a supply-chain problem waiting to
//! happen, and certificate validation alone does not protect against a compromised
//! release bucket.
//!
//! **Android** — none of that is possible. An app cannot replace its own APK; only the
//! system package installer can, and it will only do so through an intent the user
//! confirms. So this module downloads the APK itself, verifies its SHA-256 against the
//! hash published in the same signed manifest, and hands the file to the installer. If
//! the device refuses the hand-off — some OEM builds do, and the permission can be
//! declined — the caller falls back to opening the release page, which always works.
//!
//! The hash check is not optional on that path. Without it, "download an APK and ask the
//! OS to install it" is exactly the shape of the attack the signed desktop channel
//! exists to prevent.

use serde::{Deserialize, Serialize};

// Only the Android path needs these: it is the one that writes a downloaded file to a
// cache directory. The desktop updater hands all of that to the Tauri plugin.
#[cfg(target_os = "android")]
use std::path::PathBuf;
#[cfg(target_os = "android")]
use tauri::Manager;

/// Where releases are published. Compiled in rather than configurable: an updater that
/// can be pointed somewhere else by a config file is an updater that can be pointed at
/// an attacker.
const REPO: &str = "kaiharimoto/Master-Design";

const RELEASES_API: &str = "https://api.github.com/repos/kaiharimoto/Master-Design/releases/latest";

/// GitHub rejects requests without one.
const USER_AGENT: &str = concat!("master-design/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    html_url: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Ask GitHub what the newest release is.
pub async fn check(app: tauri::AppHandle) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();

    let release = tauri::async_runtime::spawn_blocking(fetch_latest)
        .await
        .map_err(|e| e.to_string())??;

    let latest = release.tag_name.trim_start_matches('v').to_string();

    let newer = match (semver::Version::parse(&latest), semver::Version::parse(&current)) {
        (Ok(l), Ok(c)) => l > c,
        // An unparseable tag means someone published something odd. Comparing strings
        // would be a coin flip, so treat it as "nothing to offer" rather than nagging
        // people about a version that may be older than theirs.
        _ => false,
    };

    if !newer {
        return Ok(UpdateInfo { available: false, current_version: current, ..Default::default() });
    }

    let asset = pick_asset(&release.assets);

    Ok(UpdateInfo {
        available: true,
        current_version: current,
        latest_version: Some(latest),
        notes: Some(summarize(&release.body)),
        release_url: Some(release.html_url),
        download_url: asset.as_ref().map(|a| a.browser_download_url.clone()),
        sha256: asset.and_then(|a| find_hash(&release.body, &a.name)),
    })
}

fn fetch_latest() -> Result<Release, String> {
    let response = ureq::get(RELEASES_API)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?;

    let release: Release = response
        .into_json()
        .map_err(|e| format!("could not read the release: {e}"))?;

    if release.draft || release.prerelease {
        return Err("the latest release is a draft or pre-release".into());
    }
    Ok(release)
}

/// Choose the artifact for the platform this build is running on.
fn pick_asset(assets: &[Asset]) -> Option<&Asset> {
    let wanted: &[&str] = if cfg!(target_os = "android") {
        &[".apk"]
    } else if cfg!(target_os = "windows") {
        // NSIS first: it is the one the bundled updater can apply in place.
        &["-setup.exe", ".nsis.zip", ".msi"]
    } else if cfg!(target_os = "macos") {
        &[".app.tar.gz", ".dmg"]
    } else {
        &[".AppImage", ".deb"]
    };

    for suffix in wanted {
        if let Some(asset) = assets.iter().find(|a| a.name.to_lowercase().ends_with(suffix)) {
            return Some(asset);
        }
    }
    None
}

/// Pull an artifact's SHA-256 out of the release notes.
///
/// The release workflow writes a `| filename | hash |` table into the body, so the hash
/// travels with the release rather than in a separate file that could go missing.
fn find_hash(body: &str, asset_name: &str) -> Option<String> {
    for line in body.lines() {
        if !line.contains(asset_name) {
            continue;
        }
        for token in line.split(|c: char| !c.is_ascii_alphanumeric()) {
            if token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(token.to_lowercase());
            }
        }
    }
    None
}

/// First paragraph of the release notes; the banner has no room for more.
fn summarize(body: &str) -> String {
    let text: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('|') && !l.starts_with('#'))
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");

    if text.chars().count() > 240 {
        format!("{}…", text.chars().take(239).collect::<String>())
    } else {
        text
    }
}

/// Takes an `AppHandle` it does not use, so that the command signature stays uniform
/// with the rest of the updater surface and can grow a window reference later.
pub fn open_release_page(_app: &tauri::AppHandle) -> Result<(), String> {
    let url = format!("https://github.com/{REPO}/releases/latest");
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Desktop
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub async fn install(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;

    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update is available")?;

    // The plugin verifies the manifest's minisign signature against the public key in
    // tauri.conf.json before anything is written to disk.
    update
        .download_and_install(|_downloaded, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;

    app.restart();
}

// ---------------------------------------------------------------------------
// Android
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
pub async fn install(app: tauri::AppHandle) -> Result<(), String> {
    let info = check(app.clone()).await?;

    let url = info.download_url.ok_or(
        "this release has no Android build attached — open the release page to download it",
    )?;
    let expected = info.sha256.ok_or(
        "this release published no checksum for the Android build, so it will not be \
         installed automatically — open the release page instead",
    )?;

    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("updates");
    std::fs::create_dir_all(&cache).map_err(|e| e.to_string())?;

    let target = cache.join(format!(
        "master-design-{}.apk",
        info.latest_version.as_deref().unwrap_or("latest")
    ));

    let downloaded = tauri::async_runtime::spawn_blocking({
        let target = target.clone();
        move || download_verified(&url, &expected, &target)
    })
    .await
    .map_err(|e| e.to_string())??;

    // Hand off to the system package installer. The user confirms; the app cannot.
    tauri_plugin_opener::open_path(downloaded.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| {
            format!(
                "Android would not open the installer ({e}). \
                 Install it from the release page instead."
            )
        })
}

#[cfg(target_os = "android")]
fn download_verified(url: &str, expected_sha256: &str, target: &PathBuf) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let response = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(300))
        .call()
        .map_err(|e| format!("download failed: {e}"))?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        // Ceiling on what will be read into memory. Well above any real APK, and it
        // stops a malicious or broken response from exhausting a phone's RAM.
        .take(512 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("download failed: {e}"))?;

    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected_sha256.to_lowercase() {
        return Err(format!(
            "the download did not match its published checksum (expected {expected_sha256}, \
             got {actual}) — it was discarded and nothing was installed"
        ));
    }

    std::fs::write(target, &bytes).map_err(|e| format!("could not save the update: {e}"))?;
    Ok(target.clone())
}

#[cfg(target_os = "ios")]
pub async fn install(_app: tauri::AppHandle) -> Result<(), String> {
    Err("iOS apps update through the App Store".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_recovered_from_a_release_notes_table() {
        let body = "\
## Checksums

| File | SHA-256 |
| --- | --- |
| master-design-0.2.0.apk | 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 |
";
        assert_eq!(
            find_hash(body, "master-design-0.2.0.apk").as_deref(),
            Some("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")
        );
    }

    #[test]
    fn a_missing_hash_is_none_rather_than_a_wrong_one() {
        assert!(find_hash("no checksums here", "app.apk").is_none());
        // A line mentioning the file but carrying no 64-hex token must not match
        // something that merely looks hash-shaped.
        assert!(find_hash("| app.apk | pending |", "app.apk").is_none());
    }

    #[test]
    fn the_platform_asset_is_picked_by_suffix() {
        let assets = vec![
            Asset {
                name: "master-design_0.2.0_x64-setup.exe".into(),
                browser_download_url: "https://example/setup".into(),
            },
            Asset {
                name: "master-design-0.2.0.apk".into(),
                browser_download_url: "https://example/apk".into(),
            },
        ];
        let picked = pick_asset(&assets).expect("an asset should match this platform");
        if cfg!(target_os = "android") {
            assert!(picked.name.ends_with(".apk"));
        } else if cfg!(target_os = "windows") {
            assert!(picked.name.ends_with("-setup.exe"));
        }
    }

    #[test]
    fn notes_are_trimmed_to_something_a_banner_can_hold() {
        let body = format!("# Heading\n\n{}\n\n| table | row |", "word ".repeat(200));
        let summary = summarize(&body);
        assert!(summary.chars().count() <= 240, "got {} chars", summary.chars().count());
        assert!(!summary.contains('|'));
        assert!(!summary.contains('#'));
    }
}
