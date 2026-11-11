//! Self-update from GitHub Releases. Each release carries `Fanhuaji-<os>-<arch>.zip`, holding
//! one entry (the `.app` bundle on macOS, the executable elsewhere), and a minisign signature
//! `<zip>.minisig` made with the key whose public half is [`PUBLIC_KEY`]. A verified zip
//! replaces what is installed by renaming, then the app restarts.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Signs release zips; the secret half is the `MINISIGN_SECRET_KEY` Actions secret.
pub const PUBLIC_KEY: &str = "RWSkiiW8p5dRcfBLoIJvwNFriEcYRTysv8NSQn+Ppzsv8qz8OKRIghKI";

pub const RELEASES_API: &str =
    "https://api.github.com/repos/fanhuaji-desktop/fanhuaji-gpui/releases/latest";

#[derive(Debug, PartialEq, Clone)]
pub struct Release {
    pub version: String,
    /// The release page, for platforms without an asset or when installing fails.
    pub url: String,
    pub notes: String,
    /// This platform's zip, when the release has one with its signature.
    pub asset: Option<Asset>,
}

#[derive(Debug, PartialEq, Clone)]
pub struct Asset {
    pub zip: String,
    pub sig: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// This platform's release asset, e.g. `Fanhuaji-macos-aarch64.zip`.
pub fn asset_name() -> String {
    format!(
        "Fanhuaji-{}-{}.zip",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// `1.2.3` from `v1.2.3` or `1.2.3`; None for anything else (pre-releases included).
fn parse(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.strip_prefix('v').unwrap_or(version).split('.');
    let mut out = [0; 3];
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Whether `tag` names a release newer than `current`. Unparsable tags never are.
pub fn is_newer(current: &str, tag: &str) -> bool {
    matches!((parse(current), parse(tag)), (Some(c), Some(t)) if t > c)
}

/// The newer release in a `releases/latest` response body, if there is one, with the
/// asset called `asset` (and its signature) if both are attached.
pub fn newer_release(current: &str, body: &str, asset: &str) -> Result<Option<Release>, String> {
    let latest: LatestRelease =
        serde_json::from_str(body).map_err(|e| format!("RESPONSE_PARSE_FAILED:{e}"))?;
    let url_of = |name: &str| {
        latest
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
    };
    let asset = url_of(asset)
        .zip(url_of(&format!("{asset}.minisig")))
        .map(|(zip, sig)| Asset { zip, sig });
    Ok(is_newer(current, &latest.tag_name).then(|| Release {
        version: latest
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&latest.tag_name)
            .to_string(),
        url: latest.html_url,
        notes: latest.body.unwrap_or_default(),
        asset,
    }))
}

/// Ask GitHub for the latest release. A repo without releases answers 404: no update.
pub async fn check(client: &reqwest::Client, current: &str) -> Result<Option<Release>, String> {
    let resp = client
        .get(RELEASES_API)
        // GitHub rejects API requests without a User-Agent.
        .header("User-Agent", "fanhuaji-gpui")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("NET_REQUEST_FAILED:{e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("HTTP_ERROR:{}", resp.status().as_u16()));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| format!("RESPONSE_PARSE_FAILED:{e}"))?;
    newer_release(current, &body, &asset_name())
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client
        .get(url)
        .header("User-Agent", "fanhuaji-gpui")
        // The 30 s client timeout covers the whole body; a release zip needs longer.
        .timeout(std::time::Duration::from_secs(600))
        .send()
        .await
        .map_err(|e| format!("NET_REQUEST_FAILED:{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP_ERROR:{}", resp.status().as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("NET_REQUEST_FAILED:{e}"))?;
    Ok(bytes.to_vec())
}

/// Download the asset's zip and return it once its signature checks out.
pub async fn download(client: &reqwest::Client, asset: &Asset) -> Result<Vec<u8>, String> {
    let zip = fetch(client, &asset.zip).await?;
    let sig = fetch(client, &asset.sig).await?;
    verify(&zip, &String::from_utf8_lossy(&sig), PUBLIC_KEY)?;
    Ok(zip)
}

/// Whether `sig` (a `.minisig` file) signs `data` with the key `public_key`.
pub fn verify(data: &[u8], sig: &str, public_key: &str) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(public_key)
        .map_err(|e| format!("UPDATE_SIGNATURE_INVALID:{e}"))?;
    let sig = minisign_verify::Signature::decode(sig)
        .map_err(|e| format!("UPDATE_SIGNATURE_INVALID:{e}"))?;
    key.verify(data, &sig, false)
        .map_err(|e| format!("UPDATE_SIGNATURE_INVALID:{e}"))
}

/// What an update replaces: the `.app` bundle the executable runs from, else the executable.
pub fn install_target(exe: &Path) -> PathBuf {
    let bundle = exe.ancestors().nth(3).filter(|app| {
        app.extension().is_some_and(|e| e == "app")
            && exe.parent().is_some_and(|p| p.ends_with("Contents/MacOS"))
    });
    bundle.unwrap_or(exe).to_path_buf()
}

/// Siblings of `target` used while installing; hidden so the user doesn't trip over them.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    target.with_file_name(format!(".{name}.{suffix}"))
}

fn remove(path: &Path) {
    let _ = std::fs::remove_dir_all(path).or_else(|_| std::fs::remove_file(path));
}

/// Remove what an earlier update left behind: Windows can't delete a running executable.
pub fn cleanup(target: &Path) {
    remove(&sibling(target, "old"));
    remove(&sibling(target, "update"));
}

/// Put the one entry in `zip` in place of `target`, keeping `target` if anything fails.
pub fn install(zip: &[u8], target: &Path) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("UPDATE_INSTALL_FAILED:{e}");
    let staging = sibling(target, "update");
    let old = sibling(target, "old");
    cleanup(target);
    // Extract next to the target so the swap below is a rename on one file system.
    zip::ZipArchive::new(std::io::Cursor::new(zip))
        .and_then(|mut a| a.extract(&staging))
        .map_err(|e| {
            remove(&staging);
            fail(&e)
        })?;
    let entries: Vec<_> = std::fs::read_dir(&staging)
        .map_err(|e| fail(&e))?
        .flatten()
        .map(|e| e.path())
        .collect();
    let new = match entries.as_slice() {
        [one] if one.is_dir() == target.is_dir() => one.clone(),
        _ => {
            remove(&staging);
            return Err(fail(&"unexpected archive layout"));
        }
    };
    #[cfg(unix)]
    if new.is_file() {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| fail(&e))?;
    }
    // A running executable can be renamed (Windows too), just not deleted.
    std::fs::rename(target, &old).map_err(|e| fail(&e))?;
    if let Err(e) = std::fs::rename(&new, target) {
        let _ = std::fs::rename(&old, target);
        return Err(fail(&e));
    }
    cleanup(target);
    Ok(())
}

/// Start the installed app; the caller quits this one.
pub fn relaunch(target: &Path) -> std::io::Result<()> {
    let mut cmd = if target.is_dir() {
        // `-n`: a new instance, even though this one is still running.
        let mut open = std::process::Command::new("open");
        open.arg("-n").arg(target);
        open
    } else {
        std::process::Command::new(target)
    };
    cmd.spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.1.0", "v0.2.0"));
        assert!(is_newer("0.9.0", "0.10.0"), "10 > 9, not string order");
        assert!(is_newer("1.2.3", "v2.0.0"));
        assert!(!is_newer("0.2.0", "v0.2.0"));
        assert!(!is_newer("0.3.0", "v0.2.9"));
    }

    #[test]
    fn ignores_tags_that_are_not_plain_versions() {
        assert!(!is_newer("0.1.0", "nightly"));
        assert!(!is_newer("0.1.0", "v1.0.0-beta.1"));
        assert!(!is_newer("0.1.0", "v1.0"));
        assert!(!is_newer("garbage", "v1.0.0"));
    }

    #[test]
    fn reads_the_latest_release_response() {
        let body = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/x/y/releases/tag/v0.3.0","name":"x","body":"notes",
            "assets":[{"name":"F-mac.zip","browser_download_url":"https://d/F-mac.zip"},
                      {"name":"F-mac.zip.minisig","browser_download_url":"https://d/F-mac.zip.minisig"},
                      {"name":"F-win.zip","browser_download_url":"https://d/F-win.zip"}]}"#;
        assert_eq!(
            newer_release("0.2.0", body, "F-mac.zip"),
            Ok(Some(Release {
                version: "0.3.0".into(),
                url: "https://github.com/x/y/releases/tag/v0.3.0".into(),
                notes: "notes".into(),
                asset: Some(Asset {
                    zip: "https://d/F-mac.zip".into(),
                    sig: "https://d/F-mac.zip.minisig".into()
                }),
            }))
        );
        let asset = |name| newer_release("0.2.0", body, name).unwrap().unwrap().asset;
        assert_eq!(asset("F-win.zip"), None, "unsigned assets are not offered");
        assert_eq!(asset("F-linux.zip"), None);
        assert_eq!(newer_release("0.3.0", body, "F-mac.zip"), Ok(None));
        assert!(
            newer_release("0.1.0", "<html>", "F-mac.zip")
                .unwrap_err()
                .starts_with("RESPONSE_PARSE_FAILED:")
        );
    }

    #[test]
    fn asset_names_match_the_release_workflow() {
        let name = asset_name();
        assert!(
            name.starts_with("Fanhuaji-") && name.ends_with(".zip"),
            "{name}"
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(name, "Fanhuaji-macos-aarch64.zip");
    }

    // A throwaway key pair; this signature covers b"update payload".
    const TEST_KEY: &str = "RWQW5nxFpgQM9q8HpzlUjvDpMFETpTX391nHw/A4t0ouxZN3TajzzrIG";
    const TEST_SIG: &str = "untrusted comment: signature from minisign secret key
RUQW5nxFpgQM9klO7rZxL9ZVV7/SxwXYyxn5ZCkhNLCY/MN45bNHdsWepLURdnK4FeIeHOCI7eo1MHF2s13fKrD7oC1YVZy8sQo=
trusted comment: timestamp:1794416339\tfile:payload\thashed
vCli+nJN7ziFBVf7FEwanghpmHz78e0qlzEUNdXQUbv1J/km2SjE4ZL0D2Xmsop9M31VvIByvoFnt9P8NziKAw==
";

    #[test]
    fn verifies_signatures() {
        assert_eq!(verify(b"update payload", TEST_SIG, TEST_KEY), Ok(()));
        let bad = |data: &[u8], sig: &str, key: &str| {
            verify(data, sig, key)
                .unwrap_err()
                .starts_with("UPDATE_SIGNATURE_INVALID:")
        };
        assert!(bad(b"update payload!", TEST_SIG, TEST_KEY), "tampered data");
        assert!(bad(b"update payload", TEST_SIG, PUBLIC_KEY), "another key");
        assert!(
            bad(b"update payload", "<html>", TEST_KEY),
            "not a signature"
        );
    }

    #[test]
    fn finds_what_to_replace() {
        let exe = Path::new("/Applications/繁化姬.app/Contents/MacOS/fanhuaji-gpui");
        assert_eq!(install_target(exe), Path::new("/Applications/繁化姬.app"));
        let exe = Path::new("/opt/fanhuaji/fanhuaji-gpui");
        assert_eq!(install_target(exe), exe);
        let exe = Path::new("/x/Contents/MacOS/fanhuaji-gpui");
        assert_eq!(install_target(exe), exe, "no .app above it");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fanhuaji-gpui-update-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in files {
            // Like `ditto`, which records each file's mode; a bundle's binary keeps its x bit.
            let mode = if name.contains("/MacOS/") {
                0o755
            } else {
                0o644
            };
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(mode);
            w.start_file(*name, opts).unwrap();
            std::io::Write::write_all(&mut w, body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replaces_an_executable() {
        let d = scratch("exe");
        let exe = d.join("fanhuaji.exe");
        std::fs::write(&exe, "old").unwrap();
        install(&zip_of(&[("fanhuaji-gpui.exe", "new")]), &exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(leftovers(&d), ["fanhuaji.exe"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "executable");
        }
    }

    #[test]
    fn replaces_an_app_bundle_whatever_it_is_called() {
        let d = scratch("app");
        let app = d.join("繁化姬.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/bin"), "old").unwrap();
        std::fs::write(app.join("Contents/stale"), "x").unwrap();
        let zip = zip_of(&[("Fanhuaji.app/Contents/MacOS/bin", "new")]);
        install(&zip, &app).unwrap();
        assert_eq!(
            std::fs::read_to_string(app.join("Contents/MacOS/bin")).unwrap(),
            "new"
        );
        assert!(!app.join("Contents/stale").exists(), "old bundle replaced");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin = std::fs::metadata(app.join("Contents/MacOS/bin")).unwrap();
            assert_eq!(bin.permissions().mode() & 0o111, 0o111, "still executable");
        }
        assert_eq!(leftovers(&d), ["繁化姬.app"]);
    }

    #[test]
    fn keeps_the_install_when_the_archive_does_not_fit() {
        let d = scratch("mismatch");
        let exe = d.join("fanhuaji-gpui");
        std::fs::write(&exe, "old").unwrap();
        for zip in [
            zip_of(&[("Fanhuaji.app/Contents/MacOS/bin", "new")]),
            zip_of(&[("a", "new"), ("b", "new")]),
            b"not a zip".to_vec(),
        ] {
            let err = install(&zip, &exe).unwrap_err();
            assert!(err.starts_with("UPDATE_INSTALL_FAILED:"), "{err}");
            assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
            assert_eq!(leftovers(&d), ["fanhuaji-gpui"]);
        }
    }

    #[test]
    fn cleans_up_after_an_earlier_update() {
        let d = scratch("cleanup");
        let exe = d.join("fanhuaji.exe");
        std::fs::write(&exe, "new").unwrap();
        std::fs::write(d.join(".fanhuaji.exe.old"), "old").unwrap();
        std::fs::create_dir_all(d.join(".fanhuaji.exe.update/x")).unwrap();
        cleanup(&exe);
        assert_eq!(leftovers(&d), ["fanhuaji.exe"]);
    }
}
