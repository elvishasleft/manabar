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
struct UsageRaw {
    five_hour: Option<WindowRaw>,
    seven_day: Option<WindowRaw>,
    seven_day_sonnet: Option<WindowRaw>,
    seven_day_opus: Option<WindowRaw>,
}

fn window(label: &str, raw: WindowRaw) -> RateWindow {
    RateWindow {
        label: label.to_string(),
        used_percent: raw.utilization,
        resets_at: raw
            .resets_at
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
    }
}

pub fn parse_quota(
    body: &str,
    plan_hint: Option<String>,
    fetched_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, ProviderError> {
    let raw: UsageRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("claude usage: {e}")))?;
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
    Ok(QuotaSnapshot { plan, windows, fetched_at })
}

pub struct ClaudeProvider {
    pub base_url: String,
    pub home: PathBuf,
}

impl ClaudeProvider {
    pub fn new(home: PathBuf) -> Self {
        Self { base_url: "https://api.anthropic.com".into(), home }
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

    #[test]
    fn parses_documented_shape() {
        let snap = parse_quota(FIXTURE, Some("max".into()), Utc::now()).unwrap();
        assert_eq!(snap.plan.as_deref(), Some("Max"));
        let labels: Vec<&str> = snap.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly", "Weekly (Sonnet)"]);
        assert_eq!(snap.windows[0].used_percent, 23.0);
        assert!(snap.windows[0].resets_at.is_some());
        assert_eq!(snap.binding_remaining_percent(), Some(38.5));
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
