use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Claude,
    Codex,
    Grok,
    DeepSeek,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RateWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// Projected exhaustion time from a linear burn-rate fit over the last
    /// two polls (see `quota_math::exhaust_eta`). Providers always set this
    /// to `None` at parse time — the shell computes it post-poll from
    /// persisted samples.
    pub exhaust_eta: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QuotaSnapshot {
    pub plan: Option<String>,
    pub windows: Vec<RateWindow>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Green,
    Amber,
    Red,
    Unavailable,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("credentials not found")]
    NoCredentials,
    #[error("token expired")]
    TokenExpired,
    #[error("network error: {0}")]
    Network(String),
    #[error("schema changed: {0}")]
    SchemaChanged(String),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DayUsage {
    pub date: NaiveDate,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub est_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UsageStats {
    pub days: Vec<DayUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    pub kind: ProviderKind,
    pub health: Health,
    pub remaining_percent: Option<f64>,
    pub quota: Option<QuotaSnapshot>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
    pub usage: Option<UsageStats>,
    pub updated_at: Option<DateTime<Utc>>,
}
