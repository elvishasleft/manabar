pub mod claude;
pub mod codex;
pub mod grok;

use crate::model::{Health, ProviderError, ProviderKind, ProviderView, QuotaSnapshot};
use crate::pricing::PriceTable;
use crate::quota_math::health_for;
use crate::usage_logs::{aggregate_dir, LogCache};
use chrono::NaiveDate;
use std::path::PathBuf;

#[async_trait::async_trait]
pub trait QuotaProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn fetch_quota(&self, http: &reqwest::Client) -> Result<QuotaSnapshot, ProviderError>;
    fn fetch_usage(
        &self,
        cache: &mut LogCache,
        prices: &PriceTable,
        today: NaiveDate,
    ) -> crate::model::UsageStats;
}

#[async_trait::async_trait]
impl QuotaProvider for claude::ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }
    async fn fetch_quota(&self, http: &reqwest::Client) -> Result<QuotaSnapshot, ProviderError> {
        claude::ClaudeProvider::fetch_quota(self, http).await
    }
    fn fetch_usage(
        &self,
        cache: &mut LogCache,
        prices: &PriceTable,
        today: NaiveDate,
    ) -> crate::model::UsageStats {
        aggregate_dir(
            cache,
            &self.home.join(".claude").join("projects"),
            crate::usage_logs::claude::file_matches,
            crate::usage_logs::claude::parse_file,
            prices,
            today,
        )
    }
}

#[async_trait::async_trait]
impl QuotaProvider for codex::CodexProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }
    async fn fetch_quota(&self, http: &reqwest::Client) -> Result<QuotaSnapshot, ProviderError> {
        codex::CodexProvider::fetch_quota(self, http).await
    }
    fn fetch_usage(
        &self,
        cache: &mut LogCache,
        prices: &PriceTable,
        today: NaiveDate,
    ) -> crate::model::UsageStats {
        aggregate_dir(
            cache,
            &self.home.join(".codex").join("sessions"),
            crate::usage_logs::codex::file_matches,
            crate::usage_logs::codex::parse_file,
            prices,
            today,
        )
    }
}

#[async_trait::async_trait]
impl QuotaProvider for grok::GrokProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Grok
    }
    async fn fetch_quota(&self, http: &reqwest::Client) -> Result<QuotaSnapshot, ProviderError> {
        grok::GrokProvider::fetch_quota(self, http).await
    }
    fn fetch_usage(
        &self,
        cache: &mut LogCache,
        prices: &PriceTable,
        today: NaiveDate,
    ) -> crate::model::UsageStats {
        aggregate_dir(
            cache,
            &self.home.join(".grok").join("sessions"),
            crate::usage_logs::grok::file_matches,
            crate::usage_logs::grok::parse_file,
            prices,
            today,
        )
    }
}

pub fn default_providers(home: PathBuf) -> Vec<Box<dyn QuotaProvider>> {
    vec![
        Box::new(claude::ClaudeProvider::new(home.clone())),
        Box::new(codex::CodexProvider::new(home.clone())),
        Box::new(grok::GrokProvider::new(home)),
    ]
}

pub fn initial_view(kind: ProviderKind) -> ProviderView {
    ProviderView {
        kind,
        health: Health::Unavailable,
        remaining_percent: None,
        quota: None,
        error: None,
        error_kind: None,
        usage: None,
        updated_at: None,
    }
}

pub fn update_view(
    prev: &ProviderView,
    result: Result<QuotaSnapshot, ProviderError>,
) -> ProviderView {
    match result {
        Ok(snap) => {
            let remaining = snap.binding_remaining_percent();
            ProviderView {
                kind: prev.kind,
                health: remaining.map(health_for).unwrap_or(Health::Unavailable),
                remaining_percent: remaining,
                updated_at: Some(snap.fetched_at),
                quota: Some(snap),
                error: None,
                error_kind: None,
                usage: prev.usage.clone(),
            }
        }
        Err(e) => {
            let kind_str = match &e {
                ProviderError::NoCredentials => "no_credentials",
                ProviderError::TokenExpired => "token_expired",
                ProviderError::Network(_) => "network",
                ProviderError::SchemaChanged(_) => "schema_changed",
            };
            ProviderView {
                kind: prev.kind,
                health: Health::Unavailable,
                remaining_percent: None,
                quota: prev.quota.clone(),
                error: Some(e.to_string()),
                error_kind: Some(kind_str.into()),
                usage: prev.usage.clone(),
                updated_at: prev.updated_at,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use chrono::Utc;

    fn snap(used: f64) -> QuotaSnapshot {
        QuotaSnapshot {
            plan: None,
            windows: vec![RateWindow {
                label: "5h".into(),
                used_percent: used,
                resets_at: None,
            }],
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn ok_result_sets_health_and_clears_error() {
        let v0 = initial_view(ProviderKind::Claude);
        let v1 = update_view(&v0, Ok(snap(20.0)));
        assert_eq!(v1.health, Health::Green);
        assert_eq!(v1.remaining_percent, Some(80.0));
        assert!(v1.error.is_none());
        assert!(v1.quota.is_some());
        assert!(v1.updated_at.is_some());
    }

    #[test]
    fn error_keeps_last_good_quota() {
        let v0 = initial_view(ProviderKind::Codex);
        let v1 = update_view(&v0, Ok(snap(95.0)));
        assert_eq!(v1.health, Health::Red);
        let v2 = update_view(&v1, Err(ProviderError::Network("boom".into())));
        assert_eq!(v2.health, Health::Unavailable);
        assert_eq!(v2.error_kind.as_deref(), Some("network"));
        assert!(v2.quota.is_some(), "last good snapshot retained");
        assert_eq!(v2.updated_at, v1.updated_at);
    }

    #[test]
    fn error_kinds_map_to_stable_strings() {
        let v0 = initial_view(ProviderKind::Grok);
        for (err, kind) in [
            (ProviderError::NoCredentials, "no_credentials"),
            (ProviderError::TokenExpired, "token_expired"),
            (ProviderError::Network("x".into()), "network"),
            (ProviderError::SchemaChanged("x".into()), "schema_changed"),
        ] {
            assert_eq!(update_view(&v0, Err(err)).error_kind.as_deref(), Some(kind));
        }
    }

    #[test]
    fn default_providers_order_is_claude_codex_grok() {
        let ps = default_providers(std::path::PathBuf::from("C:/nonexistent"));
        let kinds: Vec<ProviderKind> = ps.iter().map(|p| p.kind()).collect();
        assert_eq!(
            kinds,
            vec![
                ProviderKind::Claude,
                ProviderKind::Codex,
                ProviderKind::Grok
            ]
        );
    }
}
