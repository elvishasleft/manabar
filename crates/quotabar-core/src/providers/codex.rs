use crate::credentials::codex_token;
use crate::http::get_json;
use crate::model::{ProviderError, QuotaSnapshot, RateWindow};
use crate::quota_math::window_label_from_seconds;
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct WindowRaw {
    used_percent: f64,
    limit_window_seconds: Option<i64>,
    reset_at: Option<i64>,
}

#[derive(Deserialize)]
struct RateLimitRaw {
    primary_window: Option<WindowRaw>,
    secondary_window: Option<WindowRaw>,
}

#[derive(Deserialize)]
struct UsageRaw {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitRaw>,
}

fn window(raw: WindowRaw) -> RateWindow {
    RateWindow {
        label: raw
            .limit_window_seconds
            .map(window_label_from_seconds)
            .unwrap_or_else(|| "Window".into()),
        used_percent: raw.used_percent,
        resets_at: raw.reset_at.and_then(|s| Utc.timestamp_opt(s, 0).single()),
    }
}

pub fn parse_quota(body: &str, fetched_at: DateTime<Utc>) -> Result<QuotaSnapshot, ProviderError> {
    let raw: UsageRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("codex usage: {e}")))?;
    let rl = raw
        .rate_limit
        .ok_or_else(|| ProviderError::SchemaChanged("codex usage: missing rate_limit".into()))?;
    let mut windows = Vec::new();
    if let Some(w) = rl.primary_window {
        windows.push(window(w));
    }
    if let Some(w) = rl.secondary_window {
        windows.push(window(w));
    }
    if windows.is_empty() {
        return Err(ProviderError::SchemaChanged(
            "codex usage: no rate-limit windows in response".into(),
        ));
    }
    let plan = raw.plan_type.map(|p| {
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

pub struct CodexProvider {
    pub base_url: String,
    pub home: PathBuf,
}

impl CodexProvider {
    pub fn new(home: PathBuf) -> Self {
        Self {
            base_url: "https://chatgpt.com".into(),
            home,
        }
    }

    pub async fn fetch_quota(
        &self,
        http: &reqwest::Client,
    ) -> Result<QuotaSnapshot, ProviderError> {
        let token = codex_token(&self.home)?;
        let mut headers = vec![("authorization", format!("Bearer {}", token.bearer))];
        if let Some(acct) = &token.account_id {
            headers.push(("chatgpt-account-id", acct.clone()));
        }
        let (status, body) = get_json(
            http,
            &format!("{}/backend-api/wham/usage", self.base_url),
            &headers,
        )
        .await?;
        match status {
            200 => parse_quota(&body, Utc::now()),
            401 | 403 => Err(ProviderError::TokenExpired),
            s => Err(ProviderError::Network(format!("codex usage HTTP {s}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const FIXTURE: &str = include_str!("../../tests/fixtures/codex_usage.json");

    #[test]
    fn parses_real_free_plan_shape() {
        let snap = parse_quota(FIXTURE, Utc::now()).unwrap();
        assert_eq!(snap.plan.as_deref(), Some("Free"));
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].label, "30d");
        assert_eq!(snap.windows[0].used_percent, 7.0);
        assert_eq!(snap.windows[0].resets_at.unwrap().timestamp(), 1786328676);
    }

    #[test]
    fn parses_both_windows_when_secondary_present() {
        let body = r#"{"plan_type":"plus","rate_limit":{
            "primary_window":{"used_percent":34.5,"limit_window_seconds":18000,"reset_at":1786320000},
            "secondary_window":{"used_percent":12.0,"limit_window_seconds":604800,"reset_at":1786400000}}}"#;
        let snap = parse_quota(body, Utc::now()).unwrap();
        let labels: Vec<&str> = snap.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Weekly"]);
    }

    #[test]
    fn missing_rate_limit_is_schema_changed() {
        assert!(matches!(
            parse_quota(r#"{"plan_type":"free"}"#, Utc::now()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }
}
