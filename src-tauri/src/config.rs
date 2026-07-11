use quotabar_core::pricing::{Price, PriceTable};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Enabled {
    pub claude: bool,
    pub codex: bool,
    pub grok: bool,
}

impl Default for Enabled {
    fn default() -> Self {
        Self {
            claude: true,
            codex: true,
            grok: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceOverride {
    pub model_contains: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub enabled: Enabled,
    pub price_overrides: Vec<PriceOverride>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 1800,
            enabled: Enabled::default(),
            price_overrides: vec![],
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("quotabar")
        .join("config.json")
}

pub fn load(path: &Path) -> Config {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(cfg).expect("config serializes"),
    )
}

pub fn price_table(cfg: &Config) -> PriceTable {
    let mut table = PriceTable::default_table();
    for o in cfg.price_overrides.iter().rev() {
        table.entries.insert(
            0,
            (
                o.model_contains.clone(),
                Price {
                    input: o.input,
                    output: o.output,
                    cache_read: o.cache_read,
                    cache_write: o.cache_write,
                },
            ),
        );
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(&dir.path().join("nope.json"));
        assert_eq!(cfg.poll_interval_secs, 1800);
        assert!(cfg.enabled.claude && cfg.enabled.codex && cfg.enabled.grok);
        assert!(cfg.price_overrides.is_empty());
    }

    #[test]
    fn roundtrips_and_survives_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config {
            poll_interval_secs: 600,
            enabled: Enabled {
                codex: false,
                ..Enabled::default()
            },
            ..Config::default()
        };
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path), cfg);
        std::fs::write(&path, "garbage{{{").unwrap();
        assert_eq!(load(&path), Config::default());
    }

    #[test]
    fn overrides_win_over_default_table() {
        let mut cfg = Config::default();
        cfg.price_overrides.push(PriceOverride {
            model_contains: "claude-opus".into(),
            input: 1.0,
            output: 2.0,
            cache_read: 0.1,
            cache_write: 0.2,
        });
        let t = price_table(&cfg);
        assert_eq!(t.price_for("claude-opus-4-8").unwrap().output, 2.0);
    }
}
