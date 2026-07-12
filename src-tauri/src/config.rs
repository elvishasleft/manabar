use quotabar_core::pricing::{Price, PriceTable};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Enabled {
    pub claude: bool,
    pub codex: bool,
    pub grok: bool,
    pub deepseek: bool,
}

impl Default for Enabled {
    fn default() -> Self {
        Self {
            claude: true,
            codex: true,
            grok: true,
            deepseek: true,
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

/// User-facing config shape for health-color thresholds. Deliberately
/// separate from `quotabar_core::Thresholds`: this one is raw, possibly
/// invalid, user-editable JSON; `sanitized_thresholds` is the only path
/// from here to the validated core type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThresholdsConfig {
    pub amber: f64,
    pub red: f64,
}

impl Default for ThresholdsConfig {
    fn default() -> Self {
        Self {
            amber: 30.0,
            red: 10.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub enabled: Enabled,
    pub price_overrides: Vec<PriceOverride>,
    pub deepseek_api_key: Option<String>,
    pub deepseek_budget: Option<f64>,
    pub thresholds: ThresholdsConfig,
    /// macOS-only: shows `"{letter} {percent}%"` next to the tray icon in
    /// the menu bar (see `tray::menubar_title`). Ignored on Windows, where
    /// there is no menu bar text concept. Defaults to `true`.
    pub menubar_text: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 1800,
            enabled: Enabled::default(),
            price_overrides: vec![],
            deepseek_api_key: None,
            deepseek_budget: None,
            thresholds: ThresholdsConfig::default(),
            menubar_text: true,
        }
    }
}

/// Validates `cfg.thresholds` and converts to the core `Thresholds` type
/// consumed by `health_for`. Valid iff `0.0 <= red < amber <= 100.0` — any
/// other combination (inverted/equal boundaries, negative, or over 100)
/// falls back to the built-in defaults rather than producing a `Health`
/// classification that could never show green (or never show red).
pub fn sanitized_thresholds(cfg: &Config) -> quotabar_core::Thresholds {
    let t = cfg.thresholds;
    if (0.0..t.amber).contains(&t.red) && t.amber <= 100.0 {
        quotabar_core::Thresholds {
            amber: t.amber,
            red: t.red,
        }
    } else {
        log::warn!(
            "invalid thresholds in config (amber={}, red={}); must satisfy 0.0 <= red < amber <= 100.0 — using defaults",
            t.amber,
            t.red
        );
        quotabar_core::Thresholds::default()
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
        assert!(
            cfg.enabled.claude && cfg.enabled.codex && cfg.enabled.grok && cfg.enabled.deepseek
        );
        assert!(cfg.price_overrides.is_empty());
        assert!(cfg.deepseek_api_key.is_none());
        assert!(cfg.deepseek_budget.is_none());
        assert_eq!(cfg.thresholds, ThresholdsConfig::default());
        assert!(cfg.menubar_text, "menubar_text defaults to true");
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
            deepseek_api_key: Some("sk-test".into()),
            deepseek_budget: Some(200.0),
            ..Config::default()
        };
        save(&path, &cfg).unwrap();
        assert_eq!(load(&path), cfg);
        std::fs::write(&path, "garbage{{{").unwrap();
        assert_eq!(load(&path), Config::default());
    }

    #[test]
    fn old_config_without_deepseek_fields_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"poll_interval_secs":900,"enabled":{"claude":true,"codex":true,"grok":true},"price_overrides":[]}"#,
        )
        .unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.poll_interval_secs, 900);
        assert!(cfg.enabled.deepseek, "deepseek enabled defaults to true");
        assert!(cfg.deepseek_api_key.is_none());
        assert!(cfg.deepseek_budget.is_none());
        assert_eq!(
            cfg.thresholds,
            ThresholdsConfig::default(),
            "old config without a thresholds key must still load, defaulting to 30/10"
        );
    }

    #[test]
    fn old_config_without_thresholds_key_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"poll_interval_secs":1800,"enabled":{"claude":true,"codex":true,"grok":true,"deepseek":true},"price_overrides":[],"deepseek_api_key":null,"deepseek_budget":null}"#,
        )
        .unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.thresholds, ThresholdsConfig::default());
    }

    #[test]
    fn old_config_without_menubar_text_key_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"poll_interval_secs":1800,"enabled":{"claude":true,"codex":true,"grok":true,"deepseek":true},"price_overrides":[],"deepseek_api_key":null,"deepseek_budget":null,"thresholds":{"amber":30.0,"red":10.0}}"#,
        )
        .unwrap();
        let cfg = load(&path);
        assert!(
            cfg.menubar_text,
            "pre-v0.4 config without a menubar_text key must still load, defaulting to true"
        );
    }

    #[test]
    fn partial_thresholds_object_fills_in_missing_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"thresholds":{"amber":50.0}}"#).unwrap();
        let cfg = load(&path);
        assert_eq!(cfg.thresholds.amber, 50.0);
        assert_eq!(
            cfg.thresholds.red, 10.0,
            "missing red falls back to default"
        );
    }

    #[test]
    fn sanitized_thresholds_accepts_valid_custom_values() {
        let cfg = Config {
            thresholds: ThresholdsConfig {
                amber: 50.0,
                red: 20.0,
            },
            ..Config::default()
        };
        let t = sanitized_thresholds(&cfg);
        assert_eq!(t.amber, 50.0);
        assert_eq!(t.red, 20.0);
    }

    #[test]
    fn sanitized_thresholds_falls_back_when_red_gte_amber() {
        let cfg = Config {
            thresholds: ThresholdsConfig {
                amber: 20.0,
                red: 20.0,
            },
            ..Config::default()
        };
        let t = sanitized_thresholds(&cfg);
        assert_eq!(t, quotabar_core::Thresholds::default());
    }

    #[test]
    fn sanitized_thresholds_falls_back_when_red_negative() {
        let cfg = Config {
            thresholds: ThresholdsConfig {
                amber: 30.0,
                red: -5.0,
            },
            ..Config::default()
        };
        let t = sanitized_thresholds(&cfg);
        assert_eq!(t, quotabar_core::Thresholds::default());
    }

    #[test]
    fn sanitized_thresholds_falls_back_when_amber_over_100() {
        let cfg = Config {
            thresholds: ThresholdsConfig {
                amber: 150.0,
                red: 10.0,
            },
            ..Config::default()
        };
        let t = sanitized_thresholds(&cfg);
        assert_eq!(t, quotabar_core::Thresholds::default());
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
