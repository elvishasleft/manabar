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

/// Redirect policy for both updater HTTP clients: only follow redirects that
/// stay on `https` and land on `github.com`, `api.github.com`, or a
/// `*.githubusercontent.com` host (GitHub's release assets legitimately
/// redirect to `objects.githubusercontent.com` and similar). Anything else —
/// scheme downgrade, a foreign host, or an excessive redirect chain — is
/// rejected so a compromised or malicious redirect can never send the
/// download (or the daily check) somewhere outside the pinned GitHub hosts.
fn github_only_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let url = attempt.url();
        let host_ok = url.host_str().is_some_and(|h| {
            h == "github.com"
                || h == "api.github.com"
                || h == "githubusercontent.com"
                || h.ends_with(".githubusercontent.com")
        });
        if url.scheme() == "https" && host_ok && attempt.previous().len() <= 5 {
            attempt.follow()
        } else {
            attempt.error("redirect outside pinned GitHub hosts")
        }
    })
}

pub fn spawn(app: AppHandle, enabled: bool) {
    if !enabled {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent(ua())
            .timeout(HTTP_TIMEOUT)
            .redirect(github_only_redirects())
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

/// Startup gate + cleanup for the swap updater. If a `manabar.exe.old`
/// exists next to the current exe, a just-updated instance is starting
/// while the previous instance may still be exiting: its running image IS
/// the `.old` file, which Windows refuses to delete until the process is
/// gone. Retry the delete briefly — success doubles as both the "previous
/// instance has exited" signal (so the single-instance plugin won't forward
/// to a dying process and quit) and the cleanup itself.
pub fn wait_for_previous_instance() {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let old = exe.with_extension("exe.old");
        if !old.exists() {
            return;
        }
        for _ in 0..100 {
            if std::fs::remove_file(&old).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // Still locked after ~10s: give up; worst case matches the old
        // behavior (single-instance forward to a dying process).
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
/// process; a restarted process starts fresh, and `wait_for_previous_instance()`
/// only discards `.old` once the previous process has actually released it.
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

/// Hex-encodes a SHA-256 digest as lowercase hex, matching the format GitHub
/// uses in the asset `digest` field (`"sha256:<hex>"`). No `hex` crate
/// needed for a single fixed-width fold.
#[cfg(windows)]
fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Windows: download → size-check → digest-check → rename dance → relaunch.
/// Every failure rolls back to the pre-step state and returns Err for the
/// panel to show, except a double fault, which preserves `.old`/`.new` for
/// manual recovery.
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
        .redirect(github_only_redirects())
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
    if let Some(digest) = &info.asset_digest {
        // Already validated as `"sha256:" + 64 hex chars` by manabar-core;
        // strip_prefix here is just extracting the hex half for comparison.
        if let Some(expected_hex) = digest.strip_prefix("sha256:") {
            use sha2::Digest;
            let actual_hex = hex_encode(&sha2::Sha256::digest(&bytes));
            if !actual_hex.eq_ignore_ascii_case(expected_hex) {
                return Err("digest mismatch — download rejected".into());
            }
        }
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

    // Relaunch directly — no shell involved, so no re-parsing of the exe
    // path (the old `cmd /C "ping ... & start "" "<exe>""` trick silently
    // mis-launched when the path contained `&` or `^`). The new process may
    // start while this one is still exiting; it resolves that race itself
    // via `wait_for_previous_instance` at the top of `run()`, which blocks
    // on the `.old` file's delete-lock until this process is actually gone
    // before the single-instance plugin initializes.
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
