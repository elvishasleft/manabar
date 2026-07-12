use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A single quota reading for one provider+window, persisted so the next
/// poll can compute a burn rate even across app restarts or long gaps
/// between on-demand polls.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub t_ms: i64,
    pub used: f64,
}

/// Last-seen quota sample per provider+window, keyed by provider kind
/// (lowercase, e.g. `"claude"`) then by window label (e.g. `"5h"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SampleStore {
    pub samples: HashMap<String, HashMap<String, Sample>>,
}

/// `%APPDATA%\quotabar\state.json` — next to `config_path()`'s
/// `config.json`, but written by the shell after every poll rather than
/// edited by hand.
pub fn state_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("quotabar")
        .join("state.json")
}

/// Loads the sample store, falling back to an empty default when the file
/// is missing or its contents don't parse — a corrupt state file must never
/// crash the app, it just means burn-rate ETAs start cold again.
pub fn load(path: &Path) -> SampleStore {
    match std::fs::read_to_string(path) {
        Err(_) => SampleStore::default(), // missing file: normal, stay quiet
        Ok(text) => {
            let text = text.trim_start_matches('\u{feff}'); // strip UTF-8 BOM
            match serde_json::from_str(text) {
                Ok(store) => store,
                Err(e) => {
                    log::warn!("state.json is invalid, falling back to defaults: {e}");
                    SampleStore::default()
                }
            }
        }
    }
}

pub fn save(path: &Path, store: &SampleStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(store).expect("state serializes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = load(&dir.path().join("nope.json"));
        assert!(store.samples.is_empty());
    }

    #[test]
    fn roundtrips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut store = SampleStore::default();
        store.samples.entry("claude".into()).or_default().insert(
            "5h".into(),
            Sample {
                t_ms: 1_786_600_000_000,
                used: 42.5,
            },
        );
        save(&path, &store).unwrap();
        assert_eq!(load(&path), store);
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "garbage{{{").unwrap();
        assert_eq!(load(&path), SampleStore::default());
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("state.json");
        save(&path, &SampleStore::default()).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn multiple_windows_and_providers_roundtrip_independently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut store = SampleStore::default();
        store.samples.entry("claude".into()).or_default().insert(
            "5h".into(),
            Sample {
                t_ms: 1,
                used: 10.0,
            },
        );
        store.samples.entry("claude".into()).or_default().insert(
            "Weekly".into(),
            Sample {
                t_ms: 2,
                used: 20.0,
            },
        );
        store.samples.entry("codex".into()).or_default().insert(
            "30d".into(),
            Sample {
                t_ms: 3,
                used: 30.0,
            },
        );
        save(&path, &store).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.samples["claude"]["5h"].used, 10.0);
        assert_eq!(loaded.samples["claude"]["Weekly"].used, 20.0);
        assert_eq!(loaded.samples["codex"]["30d"].used, 30.0);
    }
}
