//! Self-update support: GitHub release parsing and version comparison.
//! Pure logic only — all IO lives in the app crate.

use crate::model::ProviderError;
use serde::Deserialize;

pub const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/elvishasleft/manabar/releases/latest";
const NOTES_PREFIX: &str = "https://github.com/elvishasleft/manabar/";
const ASSET_PREFIX: &str = "https://github.com/elvishasleft/manabar/releases/download/";

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes_url: String,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
}

#[derive(Deserialize)]
struct AssetRaw {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct ReleaseRaw {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetRaw>,
}

/// Parses `releases/latest`. `Ok(None)` = up to date. Any field that fails
/// its allowlist (numeric tag, pinned-repo URLs) is rejected — a bad notes
/// URL fails the whole parse, a bad asset URL just drops one-click support.
pub fn parse_latest_release(
    body: &str,
    current_version: &str,
) -> Result<Option<UpdateInfo>, ProviderError> {
    let raw: ReleaseRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("github release: {e}")))?;
    if parse_version(&raw.tag_name).is_none() {
        return Err(ProviderError::SchemaChanged(format!(
            "github release: unexpected tag {:?}",
            raw.tag_name
        )));
    }
    if !raw.html_url.starts_with(NOTES_PREFIX) {
        return Err(ProviderError::SchemaChanged(
            "github release: html_url outside pinned repo".into(),
        ));
    }
    if !is_newer_version(current_version, &raw.tag_name) {
        return Ok(None);
    }
    let asset = raw.assets.iter().find(|a| {
        a.name.ends_with("_portable.exe") && a.browser_download_url.starts_with(ASSET_PREFIX)
    });
    Ok(Some(UpdateInfo {
        version: raw.tag_name.trim_start_matches('v').to_string(),
        notes_url: raw.html_url,
        asset_url: asset.map(|a| a.browser_download_url.clone()),
        asset_size: asset.map(|a| a.size),
    }))
}

/// Parses `"v1.2.3"` / `"1.2.3"` into numeric segments. Returns `None` for
/// anything that is not purely dotted decimal segments — callers treat that
/// as "not a valid release tag", never as an error to retry.
pub(crate) fn parse_version(tag: &str) -> Option<Vec<u64>> {
    let bare = tag.strip_prefix('v').unwrap_or(tag);
    if bare.is_empty() {
        return None;
    }
    bare.split('.').map(|s| s.parse::<u64>().ok()).collect()
}

/// True when `candidate_tag` is a valid version strictly newer than
/// `current`. Malformed input is never newer.
pub fn is_newer_version(current: &str, candidate_tag: &str) -> bool {
    let (Some(cur), Some(cand)) = (parse_version(current), parse_version(candidate_tag)) else {
        return false;
    };
    let len = cur.len().max(cand.len());
    for i in 0..len {
        let a = cand.get(i).copied().unwrap_or(0);
        let b = cur.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_and_minor_and_major_are_newer() {
        assert!(is_newer_version("0.6.1", "v0.6.2"));
        assert!(is_newer_version("0.6.1", "0.7.0"));
        assert!(is_newer_version("0.6.1", "v1.0.0"));
    }

    #[test]
    fn equal_and_older_are_not_newer() {
        assert!(!is_newer_version("0.6.1", "v0.6.1"));
        assert!(!is_newer_version("0.6.1", "0.6.0"));
        assert!(!is_newer_version("0.6.1", "v0.5.9"));
    }

    #[test]
    fn shorter_segments_compare_as_zero_padded() {
        assert!(is_newer_version("0.6", "0.6.1"));
        assert!(!is_newer_version("0.6.0", "0.6"));
    }

    #[test]
    fn malformed_tags_are_never_newer() {
        assert!(!is_newer_version("0.6.1", "banana"));
        assert!(!is_newer_version("0.6.1", "v0.6.2-alpha"));
        assert!(!is_newer_version("0.6.1", ""));
        assert!(!is_newer_version("0.6.1", "0.6.2; rm -rf /"));
    }

    #[test]
    fn parse_version_strips_v_and_rejects_junk() {
        assert_eq!(parse_version("v1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("10.0"), Some(vec![10, 0]));
        assert_eq!(parse_version("v1.2.3-rc1"), None);
        assert_eq!(parse_version("<img src=x>"), None);
    }

    const FIXTURE: &str = include_str!("../tests/fixtures/github_release.json");

    #[test]
    fn parses_release_and_picks_portable_asset() {
        let info = parse_latest_release(FIXTURE, "0.6.1").unwrap().unwrap();
        assert_eq!(info.version, "9.9.9");
        assert_eq!(
            info.notes_url,
            "https://github.com/elvishasleft/manabar/releases/tag/v9.9.9"
        );
        assert!(info
            .asset_url
            .as_deref()
            .unwrap()
            .ends_with("_portable.exe"));
        assert_eq!(info.asset_size, Some(2222));
    }

    #[test]
    fn same_or_older_version_is_none() {
        assert!(parse_latest_release(FIXTURE, "9.9.9").unwrap().is_none());
        assert!(parse_latest_release(FIXTURE, "10.0.0").unwrap().is_none());
    }

    #[test]
    fn missing_portable_asset_gives_no_asset_url() {
        let body = r#"{"tag_name":"v9.9.9","html_url":"https://github.com/elvishasleft/manabar/releases/tag/v9.9.9","assets":[]}"#;
        let info = parse_latest_release(body, "0.1.0").unwrap().unwrap();
        assert!(info.asset_url.is_none());
        assert!(info.asset_size.is_none());
    }

    #[test]
    fn foreign_notes_url_is_schema_changed() {
        let body = r#"{"tag_name":"v9.9.9","html_url":"https://evil.example.com/x","assets":[]}"#;
        assert!(parse_latest_release(body, "0.1.0").is_err());
    }

    #[test]
    fn foreign_asset_url_is_dropped_not_fetched() {
        let body = r#"{"tag_name":"v9.9.9","html_url":"https://github.com/elvishasleft/manabar/releases/tag/v9.9.9","assets":[{"name":"ManaBar_9.9.9_portable.exe","size":5,"browser_download_url":"https://evil.example.com/ManaBar_9.9.9_portable.exe"}]}"#;
        let info = parse_latest_release(body, "0.1.0").unwrap().unwrap();
        assert!(info.asset_url.is_none());
    }

    #[test]
    fn malformed_tag_is_schema_changed() {
        let body = r#"{"tag_name":"nightly","html_url":"https://github.com/elvishasleft/manabar/releases/tag/nightly","assets":[]}"#;
        assert!(parse_latest_release(body, "0.1.0").is_err());
    }
}
