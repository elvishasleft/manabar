use crate::credentials::claude_token;
use crate::http::get_json;
use crate::model::{ProviderError, QuotaSnapshot, RateWindow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct WindowRaw {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ModelScopeRaw {
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct ScopeRaw {
    #[serde(default)]
    model: Option<ModelScopeRaw>,
}

#[derive(Deserialize)]
struct LimitRaw {
    kind: String,
    percent: Option<f64>,
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<ScopeRaw>,
}

#[derive(Deserialize)]
struct UsageRaw {
    five_hour: Option<WindowRaw>,
    seven_day: Option<WindowRaw>,
    seven_day_sonnet: Option<WindowRaw>,
    seven_day_opus: Option<WindowRaw>,
    #[serde(default)]
    limits: Option<Vec<LimitRaw>>,
}

fn window(label: &str, raw: WindowRaw) -> RateWindow {
    RateWindow {
        label: label.to_string(),
        used_percent: raw.utilization,
        resets_at: parse_resets_at(raw.resets_at),
    }
}

fn parse_resets_at(resets_at: Option<String>) -> Option<DateTime<Utc>> {
    resets_at
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Label for a `limits[]` entry. Known kinds map to the legacy labels users
/// already recognize; a `weekly_scoped` entry gets the per-model display
/// name when present. Any future/unrecognized kind falls back to the kind
/// string itself so a schema addition never silently drops a window.
fn limit_label(kind: &str, scope: Option<&ScopeRaw>) -> String {
    match kind {
        "session" => "5h".to_string(),
        "weekly_all" => "Weekly".to_string(),
        "weekly_scoped" => {
            let display_name = scope
                .and_then(|s| s.model.as_ref())
                .and_then(|m| m.display_name.as_deref());
            match display_name {
                Some(name) => format!("Weekly ({name})"),
                None => "Weekly (scoped)".to_string(),
            }
        }
        other => other.to_string(),
    }
}

fn limit_window(entry: LimitRaw) -> Option<RateWindow> {
    let percent = entry.percent?;
    Some(RateWindow {
        label: limit_label(&entry.kind, entry.scope.as_ref()),
        used_percent: percent,
        resets_at: parse_resets_at(entry.resets_at),
    })
}

fn legacy_windows(raw: UsageRaw) -> Vec<RateWindow> {
    let mut windows = Vec::new();
    if let Some(w) = raw.five_hour {
        windows.push(window("5h", w));
    }
    if let Some(w) = raw.seven_day {
        windows.push(window("Weekly", w));
    }
    if let Some(w) = raw.seven_day_sonnet {
        windows.push(window("Weekly (Sonnet)", w));
    }
    if let Some(w) = raw.seven_day_opus {
        windows.push(window("Weekly (Opus)", w));
    }
    windows
}

pub fn parse_quota(
    body: &str,
    plan_hint: Option<String>,
    fetched_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, ProviderError> {
    let mut raw: UsageRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("claude usage: {e}")))?;

    // Primary path: the `limits[]` array now carries session + weekly
    // (all-model and per-model-scoped) windows. Fall back to the legacy
    // five_hour/seven_day/seven_day_sonnet/seven_day_opus fields only when
    // `limits` is absent, null, or empty (older API responses).
    let limits = raw.limits.take();
    let windows = match limits {
        Some(limits) if !limits.is_empty() => {
            limits.into_iter().filter_map(limit_window).collect()
        }
        _ => legacy_windows(raw),
    };

    if windows.is_empty() {
        return Err(ProviderError::SchemaChanged(
            "claude usage: no known rate-limit windows in response".into(),
        ));
    }
    let plan = plan_hint.map(|p| {
        let mut c = p.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => p,
        }
    });
    Ok(QuotaSnapshot {
        plan,
        windows,
        fetched_at,
    })
}

pub struct ClaudeProvider {
    pub base_url: String,
    pub home: PathBuf,
}

impl ClaudeProvider {
    pub fn new(home: PathBuf) -> Self {
        Self {
            base_url: "https://api.anthropic.com".into(),
            home,
        }
    }

    pub async fn fetch_quota(
        &self,
        http: &reqwest::Client,
    ) -> Result<QuotaSnapshot, ProviderError> {
        let token = claude_token(&self.home)?;
        let now = Utc::now();
        if let Some(exp) = token.expires_at {
            if exp <= now {
                return Err(ProviderError::TokenExpired);
            }
        }
        let (status, body) = get_json(
            http,
            &format!("{}/api/oauth/usage", self.base_url),
            &[
                ("authorization", format!("Bearer {}", token.bearer)),
                ("anthropic-beta", "oauth-2025-04-20".to_string()),
            ],
        )
        .await?;
        match status {
            200 => parse_quota(&body, token.plan_hint, now),
            401 | 403 => Err(ProviderError::TokenExpired),
            s => Err(ProviderError::Network(format!("claude usage HTTP {s}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderError;
    use chrono::Utc;

    const FIXTURE: &str = include_str!("../../tests/fixtures/claude_usage.json");

    const LEGACY_BODY: &str = r#"{
        "five_hour": { "utilization": 23.0, "resets_at": "2026-07-11T20:00:00+00:00" },
        "seven_day": { "utilization": 61.5, "resets_at": "2026-07-15T00:00:00+00:00" },
        "seven_day_sonnet": { "utilization": 12.0, "resets_at": "2026-07-15T00:00:00+00:00" },
        "seven_day_opus": null,
        "extra_usage": { "is_enabled": false }
    }"#;

    #[test]
    fn parses_limits_array_shape() {
        let snap = parse_quota(FIXTURE, Some("max".into()), Utc::now()).unwrap();
        assert_eq!(snap.plan.as_deref(), Some("Max"));
        let labels: Vec<&str> = snap.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly", "Weekly (Fable)"]);
        assert_eq!(snap.windows[0].used_percent, 4.0);
        assert_eq!(snap.windows[1].used_percent, 35.0);
        assert_eq!(snap.windows[2].used_percent, 53.0);
        assert!(snap.windows[0].resets_at.is_some());
        assert_eq!(snap.binding_remaining_percent(), Some(47.0));
    }

    #[test]
    fn falls_back_to_legacy_fields_when_limits_absent() {
        let snap = parse_quota(LEGACY_BODY, Some("max".into()), Utc::now()).unwrap();
        let labels: Vec<&str> = snap.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly", "Weekly (Sonnet)"]);
        assert_eq!(snap.windows[0].used_percent, 23.0);
        assert!(snap.windows[0].resets_at.is_some());
        assert_eq!(snap.binding_remaining_percent(), Some(38.5));
    }

    #[test]
    fn unknown_limit_kind_becomes_its_own_window() {
        let body = r#"{
            "limits": [
                { "kind": "monthly_bonus", "percent": 7, "resets_at": null, "scope": null }
            ]
        }"#;
        let snap = parse_quota(body, None, Utc::now()).unwrap();
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].label, "monthly_bonus");
        assert_eq!(snap.windows[0].used_percent, 7.0);
        assert!(snap.windows[0].resets_at.is_none());
    }

    #[test]
    fn empty_limits_and_no_legacy_windows_is_schema_changed() {
        let body = r#"{
            "five_hour": null,
            "seven_day": null,
            "seven_day_sonnet": null,
            "seven_day_opus": null,
            "limits": []
        }"#;
        let err = parse_quota(body, None, Utc::now()).unwrap_err();
        assert!(matches!(err, ProviderError::SchemaChanged(_)));
    }

    #[test]
    fn missing_all_core_windows_is_schema_changed() {
        let err = parse_quota(r#"{"unexpected":1}"#, None, Utc::now()).unwrap_err();
        assert!(matches!(err, ProviderError::SchemaChanged(_)));
    }

    #[test]
    fn non_json_is_schema_changed() {
        assert!(matches!(
            parse_quota("<html>", None, Utc::now()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }
}
