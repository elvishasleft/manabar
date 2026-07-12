mod commands;
mod config;
mod icon;
mod tray;

use chrono::Utc;
use quotabar_core::http;
use quotabar_core::model::{ProviderKind, ProviderView};
use quotabar_core::pricing::PriceTable;
use quotabar_core::providers::{default_providers, initial_view, update_view, QuotaProvider};
use quotabar_core::usage_logs::LogCache;
use std::time::Duration;
use tauri::{Emitter, Manager};

pub struct AppShared {
    views: tokio::sync::RwLock<Vec<ProviderView>>,
    providers: Vec<Box<dyn QuotaProvider>>,
    http: reqwest::Client,
    cfg: config::Config,
    prices: PriceTable,
    log_cache: tokio::sync::Mutex<LogCache>,
    refresh: tokio::sync::Notify,
}

impl AppShared {
    fn new(cfg: config::Config, home: std::path::PathBuf) -> Self {
        let providers = default_providers(home);
        let views = providers.iter().map(|p| initial_view(p.kind())).collect();
        Self {
            views: tokio::sync::RwLock::new(views),
            providers,
            http: http::client(),
            prices: config::price_table(&cfg),
            cfg,
            log_cache: tokio::sync::Mutex::new(LogCache::default()),
            refresh: tokio::sync::Notify::new(),
        }
    }

    fn enabled(&self, kind: ProviderKind) -> bool {
        match kind {
            ProviderKind::Claude => self.cfg.enabled.claude,
            ProviderKind::Codex => self.cfg.enabled.codex,
            ProviderKind::Grok => self.cfg.enabled.grok,
        }
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
                if self.enabled(p.kind()) {
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
            views[i] = update_view(&views[i], result);
        }
    }
}

async fn poll_and_publish(app: &tauri::AppHandle) {
    let shared = app.state::<AppShared>();
    shared.poll_quotas().await;
    let views = shared.views().await;
    tray::update_tray(app, &views);
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
            app.manage(AppShared::new(cfg, home));
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
}
