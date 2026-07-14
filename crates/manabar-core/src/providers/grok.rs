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
    monthly_limit: Option<ValRaw>,
    used: Option<ValRaw>,
    billing_period_end: Option<String>,
}

#[derive(Deserialize)]
struct BillingRaw {
    config: Option<ConfigRaw>,
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

// xAI has shipped two shapes for /v1/billing: a weekly `creditUsagePercent`
// (pre-2026-07) and a unified-billing monthly credit pool (`monthlyLimit` +
// `used`). Accept both so a server-side rollback doesn't break the gauge.
fn primary_window(config: &ConfigRaw) -> Result<RateWindow, ProviderError> {
    if let Some(used) = config.credit_usage_percent {
        let resets_at = config
            .current_period
            .as_ref()
            .and_then(|p| p.end.as_deref())
            .and_then(parse_rfc3339);
        return Ok(RateWindow {
            label: "Weekly credits".into(),
            used_percent: used,
            resets_at,
            exhaust_eta: None,
        });
    }
    let limit = config
        .monthly_limit
        .as_ref()
        .and_then(|v| v.val)
        .unwrap_or(0.0);
    if limit > 0.0 {
        let used = config.used.as_ref().and_then(|v| v.val).unwrap_or(0.0);
        return Ok(RateWindow {
            label: "Monthly credits".into(),
            used_percent: (used / limit) * 100.0,
            resets_at: config.billing_period_end.as_deref().and_then(parse_rfc3339),
            exhaust_eta: None,
        });
    }
    Err(ProviderError::SchemaChanged(
        "grok billing: neither creditUsagePercent nor monthlyLimit present".into(),
    ))
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
    let mut windows = vec![primary_window(&config)?];
    let cap = config
        .on_demand_cap
        .as_ref()
        .and_then(|v| v.val)
        .unwrap_or(0.0);
    if cap > 0.0 {
        let od_used = config
            .on_demand_used
            .as_ref()
            .and_then(|v| v.val)
            .unwrap_or(0.0);
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
        // Unified billing (2026-07) returns the credit pool on the bare
        // /v1/billing route; the old ?format=credits variant no longer
        // carries usage fields.
        let (status, body) =
            get_json(http, &format!("{}/v1/billing", self.base_url), &auth).await?;
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
    const FIXTURE_MONTHLY: &str = include_str!("../../tests/fixtures/grok_billing_monthly.json");

    #[test]
    fn parses_monthly_unified_billing_shape() {
        let snap = parse_billing(FIXTURE_MONTHLY, Some("X Premium+".into()), Utc::now()).unwrap();
        assert_eq!(snap.plan.as_deref(), Some("X Premium+"));
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].label, "Monthly credits");
        assert_eq!(snap.windows[0].used_percent, 25.0);
        assert!(snap.windows[0].resets_at.is_some());
    }

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
