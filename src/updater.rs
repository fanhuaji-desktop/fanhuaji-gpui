//! Notify-only update check against GitHub Releases, like the Tauri app's portable flow.
//! shortcut: no self-install; add a signed updater once release builds exist.

use serde::Deserialize;

pub const RELEASES_API: &str =
    "https://api.github.com/repos/fanhuaji-desktop/fanhuaji-gpui/releases/latest";

#[derive(Debug, PartialEq, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
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

/// The newer release in a `releases/latest` response body, if there is one.
pub fn newer_release(current: &str, body: &str) -> Result<Option<Release>, String> {
    let latest: LatestRelease =
        serde_json::from_str(body).map_err(|e| format!("RESPONSE_PARSE_FAILED:{e}"))?;
    Ok(is_newer(current, &latest.tag_name).then(|| Release {
        version: latest
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&latest.tag_name)
            .to_string(),
        url: latest.html_url,
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
    newer_release(current, &body)
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
        let body = r#"{"tag_name":"v0.3.0","html_url":"https://github.com/x/y/releases/tag/v0.3.0","name":"x"}"#;
        assert_eq!(
            newer_release("0.2.0", body),
            Ok(Some(Release {
                version: "0.3.0".into(),
                url: "https://github.com/x/y/releases/tag/v0.3.0".into()
            }))
        );
        assert_eq!(newer_release("0.3.0", body), Ok(None));
        assert!(
            newer_release("0.1.0", "<html>")
                .unwrap_err()
                .starts_with("RESPONSE_PARSE_FAILED:")
        );
    }
}
