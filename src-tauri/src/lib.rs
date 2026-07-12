mod commands;
mod config;
mod icon;
mod state;
mod tray;

use chrono::{DateTime, Utc};
use quotabar_core::model::{Health, ProviderKind, ProviderView, RateWindow};
use quotabar_core::pricing::PriceTable;
use quotabar_core::providers::{default_providers, initial_view, update_view, QuotaProvider};
use quotabar_core::usage_logs::LogCache;
use quotabar_core::{exhaust_eta, http};
use std::collections::HashMap;
use std::time::Duration;
use tauri::{Emitter, Manager};

pub struct AppShared {
    views: tokio::sync::RwLock<Vec<ProviderView>>,
    providers: Vec<Box<dyn QuotaProvider>>,
    http: reqwest::Client,
    cfg: config::Config,
    prices: PriceTable,
    thresholds: quotabar_core::Thresholds,
    log_cache: tokio::sync::Mutex<LogCache>,
    refresh: tokio::sync::Notify,
    sample_store: tokio::sync::Mutex<state::SampleStore>,
    last_health: tokio::sync::Mutex<HashMap<ProviderKind, Health>>,
}

/// Whether `kind` is enabled per `cfg` — the single source of truth for the
/// `Enabled` struct's per-field mapping, shared by `AppShared::enabled` and
/// the initial per-view `enabled` flag set in `AppShared::new` so the two
/// never drift apart.
fn provider_enabled(cfg: &config::Config, kind: ProviderKind) -> bool {
    match kind {
        ProviderKind::Claude => cfg.enabled.claude,
        ProviderKind::Codex => cfg.enabled.codex,
        ProviderKind::Grok => cfg.enabled.grok,
        ProviderKind::DeepSeek => cfg.enabled.deepseek,
    }
}

impl AppShared {
    fn new(
        cfg: config::Config,
        home: std::path::PathBuf,
        sample_store: state::SampleStore,
    ) -> Self {
        let providers = default_providers(home, cfg.deepseek_api_key.clone(), cfg.deepseek_budget);
        let thresholds = config::sanitized_thresholds(&cfg);
        let views = providers
            .iter()
            .map(|p| {
                let mut v = initial_view(p.kind());
                v.enabled = provider_enabled(&cfg, p.kind());
                v
            })
            .collect();
        Self {
            views: tokio::sync::RwLock::new(views),
            providers,
            http: http::client(),
            prices: config::price_table(&cfg),
            thresholds,
            cfg,
            log_cache: tokio::sync::Mutex::new(LogCache::default()),
            refresh: tokio::sync::Notify::new(),
            sample_store: tokio::sync::Mutex::new(sample_store),
            last_health: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    fn enabled(&self, kind: ProviderKind) -> bool {
        provider_enabled(&self.cfg, kind)
    }

    /// Count of currently-enabled providers, used to size the placeholder
    /// tray icon shown before the first poll completes (see
    /// `tray::create_tray`). Synchronous and lock-free — it only reads
    /// config-derived state, not the polled `views`.
    pub(crate) fn enabled_count(&self) -> usize {
        self.providers
            .iter()
            .filter(|p| self.enabled(p.kind()))
            .count()
    }

    pub async fn views(&self) -> Vec<ProviderView> {
        self.views.read().await.clone()
    }

    pub fn request_refresh(&self) {
        self.refresh.notify_one();
    }

    pub async fn refresh_usage(&self) {
        let today = Utc::now().date_naive();
        let mut computed = Vec::with_capacity(self.providers.len());
        {
            let mut cache = self.log_cache.lock().await;
            for p in self.providers.iter() {
                // DeepSeek's official balance endpoint carries no usage-log
                // signal (omp logs are out of scope for v0.2), so its view's
                // `usage` stays `None` even when enabled — the panel then
                // renders no usage/sparkline block for that card. The trait
                // impl still returns zero days for signature completeness;
                // this guard is what keeps it out of the view.
                if self.enabled(p.kind()) && p.kind() != ProviderKind::DeepSeek {
                    computed.push(Some(p.fetch_usage(&mut cache, &self.prices, today)));
                } else {
                    computed.push(None);
                }
            }
        }
        let mut views = self.views.write().await;
        for (i, stats) in computed.into_iter().enumerate() {
            if let Some(s) = stats {
                views[i].usage = Some(s);
            }
        }
    }

    async fn poll_quotas(&self) {
        for (i, p) in self.providers.iter().enumerate() {
            if !self.enabled(p.kind()) {
                continue;
            }
            let result = p.fetch_quota(&self.http).await;
            if let Err(e) = &result {
                log::warn!("{:?} quota fetch failed: {e}", p.kind());
            }
            let mut views = self.views.write().await;
            views[i] = update_view(&views[i], result, &self.thresholds);
            if let Some(quota) = views[i].quota.as_mut() {
                let fetched_at = quota.fetched_at;
                let kind_key = provider_kind_key(p.kind());
                let mut store = self.sample_store.lock().await;

                // Build occurrence-qualified store keys to avoid collisions when multiple
                // windows have the same label (e.g., "Weekly (scoped)" emitted twice when
                // scope.model.display_name is missing).
                let mut seen: std::collections::HashMap<&str, u32> =
                    std::collections::HashMap::new();
                for window in quota.windows.iter_mut() {
                    let count = seen
                        .entry(window.label.as_str())
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                    let store_key = if *count == 1 {
                        window.label.clone()
                    } else {
                        format!("{}#{}", window.label, count)
                    };

                    window.exhaust_eta =
                        compute_window_eta(&store, kind_key, &store_key, window, fetched_at);
                    record_sample(&mut store, kind_key, &store_key, window, fetched_at);
                }
            }
        }
        let store = self.sample_store.lock().await;
        if let Err(e) = state::save(&state::state_path(), &store) {
            log::warn!("failed to persist quota sample state: {e}");
        }
    }
}

/// Outer `SampleStore` key for a provider — lowercase, stable identifier
/// independent of the `{:?}`/display formatting used elsewhere.
fn provider_kind_key(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "claude",
        ProviderKind::Codex => "codex",
        ProviderKind::Grok => "grok",
        ProviderKind::DeepSeek => "deepseek",
    }
}

/// Deduplicates window labels into occurrence-qualified store keys.
/// For example, ["A", "A", "B"] becomes ["A", "A#2", "B"].
/// This prevents collisions in the sample store when multiple windows
/// share the same label. This function is tested via unit tests in the tests module.
#[allow(dead_code)]
fn dedup_store_keys(labels: &[&str]) -> Vec<String> {
    let mut seen: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    labels
        .iter()
        .map(|label| {
            let count = seen.entry(label).and_modify(|c| *c += 1).or_insert(1);
            if *count == 1 {
                label.to_string()
            } else {
                format!("{}#{}", label, count)
            }
        })
        .collect()
}

/// Looks up the previous sample for `kind_key`/`store_key` and, if one
/// exists, projects an exhaustion ETA via `quota_math::exhaust_eta`. The
/// projection is suppressed (returns `None`) when the window's own
/// `resets_at` would arrive before the projected exhaustion — a window that
/// resets before it runs out isn't actually at risk.
///
/// Store keys are occurrence-qualified (e.g., "5h", "5h#2") to disambiguate
/// windows with identical labels.
fn compute_window_eta(
    store: &state::SampleStore,
    kind_key: &str,
    store_key: &str,
    window: &RateWindow,
    curr_t: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let prev = store.samples.get(kind_key)?.get(store_key)?;
    let prev_t = DateTime::from_timestamp_millis(prev.t_ms)?;
    let eta = exhaust_eta(prev_t, prev.used, curr_t, window.used_percent)?;
    window
        .resets_at
        .map_or(Some(eta), |resets_at| (eta < resets_at).then_some(eta))
}

/// Writes the current reading for `kind_key`/`store_key` into the store,
/// overwriting whatever `compute_window_eta` just read as "previous" — the
/// next poll's projection is always fit against these two most-recent
/// points.
///
/// Store keys are occurrence-qualified (e.g., "5h", "5h#2") to disambiguate
/// windows with identical labels.
fn record_sample(
    store: &mut state::SampleStore,
    kind_key: &str,
    store_key: &str,
    window: &RateWindow,
    curr_t: DateTime<Utc>,
) {
    store
        .samples
        .entry(kind_key.to_string())
        .or_default()
        .insert(
            store_key.to_string(),
            state::Sample {
                t_ms: curr_t.timestamp_millis(),
                used: window.used_percent,
            },
        );
}

/// Health severity rank used to detect a transition *into* a worse state.
/// `Unavailable` ranks alongside `Green` (0) — going unavailable never
/// itself triggers a low-quota notification; only a fresh reading that
/// lands in amber/red while ranking above the last-known health does.
fn health_rank(h: Health) -> u8 {
    match h {
        Health::Green | Health::Unavailable => 0,
        Health::Amber => 1,
        Health::Red => 2,
    }
}

/// True iff this poll's health is a *worsening* transition into amber or
/// red — never on repeat readings at the same severity, and never on
/// improvement (e.g. red -> amber, amber -> green).
fn should_notify(prev: Health, curr: Health) -> bool {
    matches!(curr, Health::Amber | Health::Red) && health_rank(curr) > health_rank(prev)
}

/// Builds the notification body for a provider whose health just worsened,
/// e.g. `"Claude: 28% left (5h window), runs out ~14:32"`. Returns `None`
/// when the view lacks enough data to describe (no remaining percent or no
/// quota snapshot) — this can't happen for a view that just triggered
/// `should_notify`, but keeps the function total rather than panicking.
fn notification_body(view: &ProviderView) -> Option<String> {
    let remaining = view.remaining_percent?;
    let window = view.quota.as_ref()?.binding_window()?;
    let eta_part = window
        .exhaust_eta
        .map(|eta| {
            format!(
                ", runs out ~{}",
                eta.with_timezone(&chrono::Local).format("%H:%M")
            )
        })
        .unwrap_or_default();
    Some(format!(
        "{}: {}% left ({}){}",
        tray::provider_name(view.kind),
        remaining.round() as i64,
        window.label,
        eta_part
    ))
}

/// Sends one notification per provider whose health worsened into amber or
/// red this cycle, then records every provider's health for the next
/// cycle's comparison — including improvements back down to green, so a
/// later re-worsening is detected as a fresh transition rather than
/// suppressed by a stale "already amber" memory.
async fn notify_health_transitions(
    app: &tauri::AppHandle,
    shared: &AppShared,
    views: &[ProviderView],
) {
    let mut last = shared.last_health.lock().await;
    for view in views {
        let prev = last.get(&view.kind).copied().unwrap_or(Health::Unavailable);
        if should_notify(prev, view.health) {
            if let Some(body) = notification_body(view) {
                use tauri_plugin_notification::NotificationExt;
                if let Err(e) = app
                    .notification()
                    .builder()
                    .title("QuotaBar")
                    .body(body)
                    .show()
                {
                    log::warn!(
                        "failed to send low-quota notification for {:?}: {e}",
                        view.kind
                    );
                }
            }
        }
        last.insert(view.kind, view.health);
    }
}

async fn poll_and_publish(app: &tauri::AppHandle) {
    let shared = app.state::<AppShared>();
    shared.poll_quotas().await;
    let views = shared.views().await;
    tray::update_tray(app, &views);
    notify_health_transitions(app, &shared, &views).await;
    let _ = app.emit("state", &views);
}

/// Maps a configured `poll_interval_secs` to the periodic-polling interval to use.
///
/// `0` means on-demand mode: no periodic background polling, refresh only on
/// panel open or manual "Refresh now". Any other value is clamped to a
/// minimum of 60 seconds, matching the existing config documentation.
pub(crate) fn effective_poll_interval(secs: u64) -> Option<Duration> {
    match secs {
        0 => None,
        s => Some(Duration::from_secs(s.max(60))),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("quotabar".into()),
                    },
                ))
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .max_file_size(1_000_000)
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::refresh_now,
            commands::panel_opened,
            commands::refresh_signin
        ])
        .setup(|app| {
            let cfg_path = config::config_path();
            let cfg = config::load(&cfg_path);
            if !cfg_path.exists() {
                if let Err(e) = config::save(&cfg_path, &cfg) {
                    log::warn!("failed to write default config: {e}");
                }
            }
            let home = dirs::home_dir().expect("home dir must exist");
            let interval = effective_poll_interval(cfg.poll_interval_secs);
            let sample_store = state::load(&state::state_path());
            app.manage(AppShared::new(cfg, home, sample_store));
            tray::create_tray(app.handle())?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    poll_and_publish(&handle).await;
                    let shared = handle.state::<AppShared>();
                    match interval {
                        Some(d) => tokio::select! {
                            _ = tokio::time::sleep(d) => {},
                            _ = shared.refresh.notified() => {},
                        },
                        None => shared.refresh.notified().await,
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "panel" {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn zero_means_on_demand_mode() {
        assert_eq!(effective_poll_interval(0), None);
    }

    #[test]
    fn below_minimum_clamps_to_60_seconds() {
        assert_eq!(effective_poll_interval(1), Some(Duration::from_secs(60)));
    }

    #[test]
    fn above_minimum_is_used_as_is() {
        assert_eq!(
            effective_poll_interval(1800),
            Some(Duration::from_secs(1800))
        );
    }

    fn window(label: &str, used_percent: f64, resets_at: Option<DateTime<Utc>>) -> RateWindow {
        RateWindow {
            label: label.into(),
            used_percent,
            resets_at,
            exhaust_eta: None,
        }
    }

    #[test]
    fn provider_kind_key_is_lowercase_and_stable() {
        assert_eq!(provider_kind_key(ProviderKind::Claude), "claude");
        assert_eq!(provider_kind_key(ProviderKind::Codex), "codex");
        assert_eq!(provider_kind_key(ProviderKind::Grok), "grok");
        assert_eq!(provider_kind_key(ProviderKind::DeepSeek), "deepseek");
    }

    #[test]
    fn compute_window_eta_none_without_prior_sample() {
        let store = state::SampleStore::default();
        let w = window("5h", 50.0, None);
        assert!(compute_window_eta(&store, "claude", "5h", &w, Utc::now()).is_none());
    }

    #[test]
    fn compute_window_eta_projects_from_prior_sample() {
        let mut store = state::SampleStore::default();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        store.samples.entry("claude".into()).or_default().insert(
            "5h".into(),
            state::Sample {
                t_ms: t0.timestamp_millis(),
                used: 40.0,
            },
        );
        let t1 = t0 + chrono::Duration::minutes(30);
        let w = window("5h", 50.0, None);
        let eta = compute_window_eta(&store, "claude", "5h", &w, t1).unwrap();
        assert_eq!(eta, t1 + chrono::Duration::minutes(150));
    }

    #[test]
    fn compute_window_eta_suppressed_when_window_resets_first() {
        let mut store = state::SampleStore::default();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        store.samples.entry("claude".into()).or_default().insert(
            "5h".into(),
            state::Sample {
                t_ms: t0.timestamp_millis(),
                used: 40.0,
            },
        );
        let t1 = t0 + chrono::Duration::minutes(30);
        // Projected eta is t1 + 150min; a reset 60min away arrives first.
        let resets_at = t1 + chrono::Duration::minutes(60);
        let w = window("5h", 50.0, Some(resets_at));
        assert!(compute_window_eta(&store, "claude", "5h", &w, t1).is_none());
    }

    #[test]
    fn compute_window_eta_kept_when_window_resets_after_exhaustion() {
        let mut store = state::SampleStore::default();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        store.samples.entry("claude".into()).or_default().insert(
            "5h".into(),
            state::Sample {
                t_ms: t0.timestamp_millis(),
                used: 40.0,
            },
        );
        let t1 = t0 + chrono::Duration::minutes(30);
        let resets_at = t1 + chrono::Duration::minutes(200);
        let w = window("5h", 50.0, Some(resets_at));
        assert!(compute_window_eta(&store, "claude", "5h", &w, t1).is_some());
    }

    #[test]
    fn record_sample_writes_current_reading_under_kind_and_store_key() {
        let mut store = state::SampleStore::default();
        let now = Utc::now();
        let w = window("30d", 33.0, None);
        record_sample(&mut store, "codex", "30d", &w, now);
        let saved = store.samples.get("codex").unwrap().get("30d").unwrap();
        assert_eq!(saved.used, 33.0);
        assert_eq!(saved.t_ms, now.timestamp_millis());
    }

    #[test]
    fn notifies_on_green_to_amber_transition() {
        assert!(should_notify(Health::Green, Health::Amber));
    }

    #[test]
    fn notifies_on_amber_to_red_transition() {
        assert!(should_notify(Health::Amber, Health::Red));
    }

    #[test]
    fn notifies_on_green_to_red_transition() {
        assert!(should_notify(Health::Green, Health::Red));
    }

    #[test]
    fn notifies_from_unavailable_into_amber_or_red() {
        assert!(should_notify(Health::Unavailable, Health::Amber));
        assert!(should_notify(Health::Unavailable, Health::Red));
    }

    #[test]
    fn does_not_notify_on_repeat_same_severity() {
        assert!(!should_notify(Health::Amber, Health::Amber));
        assert!(!should_notify(Health::Red, Health::Red));
    }

    #[test]
    fn does_not_notify_on_improvement() {
        assert!(!should_notify(Health::Red, Health::Amber));
        assert!(!should_notify(Health::Amber, Health::Green));
        assert!(!should_notify(Health::Red, Health::Green));
    }

    #[test]
    fn does_not_notify_when_current_is_green_or_unavailable() {
        assert!(!should_notify(Health::Unavailable, Health::Green));
        assert!(!should_notify(Health::Green, Health::Unavailable));
        assert!(!should_notify(Health::Amber, Health::Unavailable));
    }

    #[test]
    fn notification_body_includes_eta_when_present() {
        let mut view = initial_view(ProviderKind::Claude);
        view.remaining_percent = Some(28.0);
        let eta = Utc.with_ymd_and_hms(2026, 7, 12, 14, 32, 0).unwrap();
        view.quota = Some(quotabar_core::model::QuotaSnapshot {
            plan: None,
            windows: vec![window("5h", 72.0, None)]
                .into_iter()
                .map(|mut w| {
                    w.exhaust_eta = Some(eta);
                    w
                })
                .collect(),
            fetched_at: Utc::now(),
        });
        let body = notification_body(&view).unwrap();
        assert!(body.starts_with("Claude: 28% left (5h)"));
        assert!(body.contains("runs out ~"));
    }

    #[test]
    fn notification_body_omits_eta_part_when_absent() {
        let mut view = initial_view(ProviderKind::Codex);
        view.remaining_percent = Some(5.0);
        view.quota = Some(quotabar_core::model::QuotaSnapshot {
            plan: None,
            windows: vec![window("30d", 95.0, None)],
            fetched_at: Utc::now(),
        });
        let body = notification_body(&view).unwrap();
        assert_eq!(body, "Codex: 5% left (30d)");
    }

    #[test]
    fn notification_body_none_without_quota() {
        let view = initial_view(ProviderKind::Grok);
        assert!(notification_body(&view).is_none());
    }

    #[test]
    fn dedup_store_keys_single_unique_labels() {
        let labels = vec!["A", "B", "C"];
        let result = dedup_store_keys(&labels);
        assert_eq!(result, vec!["A", "B", "C"]);
    }

    #[test]
    fn dedup_store_keys_duplicate_labels() {
        let labels = vec!["A", "A", "B"];
        let result = dedup_store_keys(&labels);
        assert_eq!(result, vec!["A", "A#2", "B"]);
    }

    #[test]
    fn dedup_store_keys_multiple_duplicates() {
        let labels = vec!["X", "Y", "X", "Y", "X"];
        let result = dedup_store_keys(&labels);
        assert_eq!(result, vec!["X", "Y", "X#2", "Y#2", "X#3"]);
    }

    #[test]
    fn collision_handling_distinct_samples_per_occurrence() {
        let mut store = state::SampleStore::default();
        let t0 = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();

        // Simulate two windows with identical labels, stored under different keys.
        let w1 = window("Weekly", 30.0, None);
        let w2 = window("Weekly", 50.0, None);

        record_sample(&mut store, "claude", "Weekly", &w1, t0);
        record_sample(&mut store, "claude", "Weekly#2", &w2, t0);

        let samples = store.samples.get("claude").unwrap();
        assert_eq!(samples.get("Weekly").unwrap().used, 30.0);
        assert_eq!(samples.get("Weekly#2").unwrap().used, 50.0);
    }
}
