use crate::credentials::grok_token;
use crate::http::get_json;
use crate::model::{ProviderError, QuotaSnapshot, RateWindow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct PeriodRaw {
    end: Option<String>,
}

#[derive(Deserialize)]
struct ValRaw {
    val: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigRaw {
    credit_usage_percent: Option<f64>,
    current_period: Option<PeriodRaw>,
    on_demand_cap: Option<ValRaw>,
    on_demand_used: Option<ValRaw>,
}

#[derive(Deserialize)]
struct BillingRaw {
    config: Option<ConfigRaw>,
}

pub fn parse_billing(
    body: &str,
    plan: Option<String>,
    fetched_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, ProviderError> {
    let raw: BillingRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("grok billing: {e}")))?;
    let config = raw
        .config
        .ok_or_else(|| ProviderError::SchemaChanged("grok billing: missing config".into()))?;
    let used = config.credit_usage_percent.ok_or_else(|| {
        ProviderError::SchemaChanged("grok billing: missing creditUsagePercent".into())
    })?;
    let resets_at = config
        .current_period
        .and_then(|p| p.end)
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let mut windows = vec![RateWindow {
        label: "Weekly credits".into(),
        used_percent: used,
        resets_at,
        exhaust_eta: None,
    }];
    let cap = config.on_demand_cap.and_then(|v| v.val).unwrap_or(0.0);
    if cap > 0.0 {
        let od_used = config.on_demand_used.and_then(|v| v.val).unwrap_or(0.0);
        windows.push(RateWindow {
            label: "On-demand".into(),
            used_percent: (od_used / cap) * 100.0,
            resets_at: None,
            exhaust_eta: None,
        });
    }
    Ok(QuotaSnapshot {
        plan,
        windows,
        fetched_at,
    })
}

pub struct GrokProvider {
    pub base_url: String,
    pub home: PathBuf,
}

impl GrokProvider {
    pub fn new(home: PathBuf) -> Self {
        Self {
            base_url: "https://cli-chat-proxy.grok.com".into(),
            home,
        }
    }

    pub async fn fetch_quota(
        &self,
        http: &reqwest::Client,
    ) -> Result<QuotaSnapshot, ProviderError> {
        let token = grok_token(&self.home)?;
        let now = Utc::now();
        if let Some(exp) = token.expires_at {
            if exp <= now {
                return Err(ProviderError::TokenExpired);
            }
        }
        let auth = [("authorization", format!("Bearer {}", token.bearer))];
        // best-effort plan label; failure must not fail the quota fetch
        let plan = match get_json(http, &format!("{}/v1/settings", self.base_url), &auth).await {
            Ok((200, body)) => serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("subscription_tier_display")
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                }),
            _ => None,
        };
        let (status, body) = get_json(
            http,
            &format!("{}/v1/billing?format=credits", self.base_url),
            &auth,
        )
        .await?;
        match status {
            200 => parse_billing(&body, plan, now),
            401 | 403 => Err(ProviderError::TokenExpired),
            s => Err(ProviderError::Network(format!("grok billing HTTP {s}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderError;
    use chrono::Utc;

    const FIXTURE: &str = include_str!("../../tests/fixtures/grok_billing.json");

    #[test]
    fn parses_real_billing_shape() {
        let snap = parse_billing(FIXTURE, Some("X Premium+".into()), Utc::now()).unwrap();
        assert_eq!(snap.plan.as_deref(), Some("X Premium+"));
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].label, "Weekly credits");
        assert_eq!(snap.windows[0].used_percent, 4.0);
        assert!(snap.windows[0].resets_at.is_some());
    }

    #[test]
    fn adds_on_demand_window_when_cap_positive() {
        let body = r#"{"config":{"creditUsagePercent":50.0,
            "currentPeriod":{"end":"2026-07-17T23:13:16+00:00"},
            "onDemandCap":{"val":200},"onDemandUsed":{"val":50}}}"#;
        let snap = parse_billing(body, None, Utc::now()).unwrap();
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[1].label, "On-demand");
        assert_eq!(snap.windows[1].used_percent, 25.0);
    }

    #[test]
    fn missing_config_is_schema_changed() {
        assert!(matches!(
            parse_billing(r#"{"other":1}"#, None, Utc::now()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }
}
