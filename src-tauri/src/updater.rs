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

#[cfg(windows)]
static UPDATE_IN_PROGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set once `apply_inner` hits a double fault (swap rename failed AND the
/// rollback rename failed). `apply_inner` starts with an unconditional
/// `remove_file(&old)`, so re-entering it after a double fault would delete
/// the just-preserved `.old` recovery backup and then fail again (the
/// canonical exe path is gone). Poisoning permanently blocks retries in this
/// process; a restarted process starts fresh, and `cleanup_old_exe()` only
/// discards `.old` on a healthy start.
#[cfg(windows)]
static UPDATE_POISONED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Thin reentrancy + poison guard around [`apply_inner`]. Rejects concurrent
/// calls while an update is in flight, and — once a prior attempt has hit a
/// double fault — permanently rejects further calls in this process so the
/// preserved on-disk `.old` backup can never be clobbered by a retry.
#[cfg(windows)]
pub async fn apply(app: AppHandle, info: UpdateInfo) -> Result<(), String> {
    use std::sync::atomic::Ordering;

    if UPDATE_POISONED.load(Ordering::SeqCst) {
        return Err(
            "previous update attempt failed critically — restart the app or reinstall; \
             on-disk backup (.old) was preserved"
                .into(),
        );
    }

    if UPDATE_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("update already in progress".into());
    }
    let result = apply_inner(app, info).await;
    if result.is_err() {
        // On success the process exits/restarts, so there is no one left to
        // observe the flag; only clear it on the error paths.
        UPDATE_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
    result
}

/// Windows: download → size-check → rename dance → relaunch. Every failure
/// rolls back to the pre-step state and returns Err for the panel to show.
#[cfg(windows)]
async fn apply_inner(app: AppHandle, info: UpdateInfo) -> Result<(), String> {
    let (Some(url), Some(size)) = (info.asset_url.clone(), info.asset_size) else {
        return Err("this release has no portable build".into());
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");

    let client = reqwest::Client::builder()
        .user_agent(ua())
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if resp.status() != 200 {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() as u64 != size {
        return Err(format!(
            "size mismatch: got {} bytes, expected {size}",
            bytes.len()
        ));
    }
    if let Err(e) = std::fs::write(&new, &bytes) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("write failed: {e}"));
    }
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(&exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("rename current exe failed: {e}"));
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        // roll back: put the running image's name back
        if let Err(re) = std::fs::rename(&old, &exe) {
            // Double fault: exe path is empty; leave .old and .new on disk
            // for manual recovery instead of deleting evidence. Poison the
            // process so a retry can't run apply_inner's unconditional
            // `remove_file(&old)` and destroy the preserved backup.
            UPDATE_POISONED.store(true, std::sync::atomic::Ordering::SeqCst);
            log::error!("update swap failed ({e}) and rollback also failed ({re})");
            return Err(format!(
                "swap failed ({e}) and rollback also failed ({re}) — reinstall needed, old exe kept as .old"
            ));
        }
        let _ = std::fs::remove_file(&new);
        return Err(format!("swap failed: {e}"));
    }
    std::process::Command::new(&exe)
        .spawn()
        .map_err(|e| format!("relaunch failed (update IS installed): {e}"))?;
    app.exit(0);
    Ok(())
}

/// Non-Windows: no in-place swap; open the release page instead.
#[cfg(not(windows))]
pub async fn apply(_app: AppHandle, info: UpdateInfo) -> Result<(), String> {
    tauri_plugin_opener::open_url(info.notes_url, None::<&str>).map_err(|e| e.to_string())
}
