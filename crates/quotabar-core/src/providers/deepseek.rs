use crate::credentials::deepseek_key;
use crate::http::get_json;
use crate::model::{ProviderError, QuotaSnapshot, RateWindow};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
struct BalanceInfoRaw {
    currency: String,
    total_balance: String,
}

#[derive(Deserialize)]
struct BalanceRaw {
    is_available: bool,
    balance_infos: Vec<BalanceInfoRaw>,
}

/// Maps a currency code to the prefix used in the plan pill: known symbols
/// for CNY/USD, otherwise the raw code itself as a prefix (e.g. `EUR `).
fn currency_prefix(currency: &str) -> String {
    match currency {
        "CNY" => "¥".to_string(),
        "USD" => "$".to_string(),
        other => format!("{other} "),
    }
}

/// Parses the official `GET /user/balance` response into a single `Balance`
/// window. Balances arrive as strings, so a parse failure (or a missing
/// `balance_infos[0]`) is `SchemaChanged`, matching how the other providers
/// treat an unparseable-but-2xx response.
///
/// Semantics are balance-based, not window-based: with a `budget` configured
/// and strictly positive, `used_percent = (1 - balance/budget) * 100`,
/// clamped to 0-100. Without a usable budget (`None`, zero, or negative —
/// dividing by a non-positive budget is meaningless and would otherwise
/// yield `NaN`/nonsensical ratios), it's a binary green/red signal from
/// `is_available` alone: 0% used (green) while DeepSeek reports the account
/// usable, 100% (red) once it doesn't. `resets_at` is always `None` — a
/// balance has no reset schedule.
pub fn parse_balance(
    body: &str,
    budget: Option<f64>,
    fetched_at: DateTime<Utc>,
) -> Result<QuotaSnapshot, ProviderError> {
    let raw: BalanceRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("deepseek balance: {e}")))?;
    let info = raw.balance_infos.first().ok_or_else(|| {
        ProviderError::SchemaChanged("deepseek balance: missing balance_infos".into())
    })?;
    let balance: f64 = info.total_balance.parse().map_err(|e| {
        ProviderError::SchemaChanged(format!("deepseek balance: bad total_balance: {e}"))
    })?;
    let used_percent = match budget {
        Some(b) if b > 0.0 => ((1.0 - balance / b) * 100.0).clamp(0.0, 100.0),
        _ => {
            if raw.is_available {
                0.0
            } else {
                100.0
            }
        }
    };
    let plan = Some(format!("{}{:.2}", currency_prefix(&info.currency), balance));
    Ok(QuotaSnapshot {
        plan,
        windows: vec![RateWindow {
            label: "Balance".into(),
            used_percent,
            resets_at: None,
        }],
        fetched_at,
    })
}

pub struct DeepSeekProvider {
    pub base_url: String,
    pub key_override: Option<String>,
    pub budget: Option<f64>,
}

impl DeepSeekProvider {
    pub fn new(key_override: Option<String>, budget: Option<f64>) -> Self {
        Self {
            base_url: "https://api.deepseek.com".into(),
            key_override,
            budget,
        }
    }

    pub async fn fetch_quota(
        &self,
        http: &reqwest::Client,
    ) -> Result<QuotaSnapshot, ProviderError> {
        let key = deepseek_key(self.key_override.as_deref())?;
        let now = Utc::now();
        let auth = [("authorization", format!("Bearer {key}"))];
        let (status, body) =
            get_json(http, &format!("{}/user/balance", self.base_url), &auth).await?;
        match status {
            200 => parse_balance(&body, self.budget, now),
            401 | 403 => Err(ProviderError::TokenExpired),
            s => Err(ProviderError::Network(format!("deepseek balance HTTP {s}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderError;
    use chrono::Utc;

    const FIXTURE: &str = include_str!("../../tests/fixtures/deepseek_balance.json");

    #[test]
    fn no_budget_available_is_zero_percent_with_plan() {
        let snap = parse_balance(FIXTURE, None, Utc::now()).unwrap();
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].label, "Balance");
        assert_eq!(snap.windows[0].used_percent, 0.0);
        assert!(snap.windows[0].resets_at.is_none());
        assert_eq!(snap.plan.as_deref(), Some("¥97.97"));
    }

    #[test]
    fn no_budget_unavailable_is_hundred_percent() {
        let body = r#"{"is_available":false,"balance_infos":[{"currency":"CNY","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
        let snap = parse_balance(body, None, Utc::now()).unwrap();
        assert_eq!(snap.windows[0].used_percent, 100.0);
    }

    #[test]
    fn budget_computes_used_percent_from_ratio() {
        let snap = parse_balance(FIXTURE, Some(200.0), Utc::now()).unwrap();
        assert!(
            (snap.windows[0].used_percent - 51.015).abs() < 0.001,
            "expected ~51.015, got {}",
            snap.windows[0].used_percent
        );
    }

    #[test]
    fn zero_budget_with_zero_balance_falls_back_to_binary_unavailable() {
        // budget = Some(0.0), balance = "0.00": naive (1 - 0/0)*100 is NaN.
        // Must fall back to the is_available binary instead of NaN/100.
        let body = r#"{"is_available":false,"balance_infos":[{"currency":"CNY","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
        let snap = parse_balance(body, Some(0.0), Utc::now()).unwrap();
        assert_eq!(snap.windows[0].used_percent, 100.0);
    }

    #[test]
    fn zero_budget_with_zero_balance_and_available_falls_back_to_binary_available() {
        // Same invalid budget, but is_available=true: fallback binary is 0%,
        // not the NaN the naive ratio math would have produced.
        let body = r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
        let snap = parse_balance(body, Some(0.0), Utc::now()).unwrap();
        assert_eq!(snap.windows[0].used_percent, 0.0);
    }

    #[test]
    fn negative_budget_falls_back_to_binary_logic() {
        let body = r#"{"is_available":false,"balance_infos":[{"currency":"CNY","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
        let snap = parse_balance(body, Some(-5.0), Utc::now()).unwrap();
        assert_eq!(snap.windows[0].used_percent, 100.0);
    }

    #[test]
    fn positive_budget_with_zero_balance_uses_real_math() {
        // A genuinely positive budget with zero balance is fully used: real
        // ratio math applies here, not the fallback binary.
        let body = r#"{"is_available":false,"balance_infos":[{"currency":"CNY","total_balance":"0.00","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
        let snap = parse_balance(body, Some(100.0), Utc::now()).unwrap();
        assert_eq!(snap.windows[0].used_percent, 100.0);
    }

    #[test]
    fn usd_currency_gets_dollar_prefix() {
        let body = r#"{"is_available":true,"balance_infos":[{"currency":"USD","total_balance":"12.50","granted_balance":"0.00","topped_up_balance":"12.50"}]}"#;
        let snap = parse_balance(body, None, Utc::now()).unwrap();
        assert_eq!(snap.plan.as_deref(), Some("$12.50"));
    }

    #[test]
    fn garbage_is_schema_changed() {
        assert!(matches!(
            parse_balance("not json", None, Utc::now()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }

    #[test]
    fn missing_balance_infos_is_schema_changed() {
        assert!(matches!(
            parse_balance(
                r#"{"is_available":true,"balance_infos":[]}"#,
                None,
                Utc::now()
            ),
            Err(ProviderError::SchemaChanged(_))
        ));
    }

    #[test]
    fn unparseable_total_balance_is_schema_changed() {
        let body = r#"{"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"not-a-number","granted_balance":"0.00","topped_up_balance":"0.00"}]}"#;
        assert!(matches!(
            parse_balance(body, None, Utc::now()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }
}
