//! Self-update support: GitHub release parsing and version comparison.
//! Pure logic only — all IO lives in the app crate.

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
}
