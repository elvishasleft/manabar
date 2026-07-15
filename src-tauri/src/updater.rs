//! Daily update check + swap-in-place update for the Windows portable exe.
//! Privacy invariants (spec: Security & privacy): the HTTP client here is
//! bare — no auth headers, no credential access — and the only URLs ever
//! fetched are the pinned repo's releases/latest and asset URLs it returns.

use manabar_core::updater::{parse_latest_release, UpdateInfo, RELEASES_LATEST_URL};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(30);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct UpdateShared(pub tokio::sync::Mutex<Option<UpdateInfo>>);

fn ua() -> String {
    format!("manabar/{}", env!("CARGO_PKG_VERSION"))
}

pub fn spawn(app: AppHandle, enabled: bool) {
    if !enabled {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent(ua())
            .timeout(HTTP_TIMEOUT)
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            check_once(&app, &client).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// One check: fetch, parse, stash, notify the panel. All failures are
/// silent by design — an update check must never surface errors into the
/// quota UI or retry aggressively.
async fn check_once(app: &AppHandle, client: &reqwest::Client) {
    let Ok(resp) = client.get(RELEASES_LATEST_URL).send().await else {
        return;
    };
    if resp.status() != 200 {
        return;
    }
    let Ok(body) = resp.text().await else { return };
    let Ok(update) = parse_latest_release(&body, env!("CARGO_PKG_VERSION")) else {
        return;
    };
    let shared = app.state::<UpdateShared>();
    *shared.0.lock().await = update.clone();
    let _ = app.emit("update", &update);
}

/// Best-effort removal of the previous exe left behind by a swap update.
pub fn cleanup_old_exe() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let old = exe.with_extension("exe.old");
        let _ = std::fs::remove_file(old);
    }
}
