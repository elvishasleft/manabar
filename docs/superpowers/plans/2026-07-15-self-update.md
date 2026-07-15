# ManaBar Self-Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In-panel update notice + one-click portable-exe self-update, per `docs/superpowers/specs/2026-07-15-self-update-design.md`.

**Architecture:** All parse/validate/compare logic lives in `manabar-core` (its tests run locally; the `src-tauri` app crate's test binary cannot load on this dev machine — STATUS_ENTRYPOINT_NOT_FOUND — so app-crate tests are compiled locally but only *run* in CI). `src-tauri/src/updater.rs` owns the daily check task, shared state, and the swap IO. The panel renders through the existing `esc()`-escaped pure render layer.

**Tech Stack:** Rust (reqwest, serde, tokio via tauri async runtime), tauri-plugin-opener 2, TypeScript + vitest.

## Global Constraints (from spec — every task inherits these)

- Zero telemetry: only anonymous `GET https://api.github.com/repos/elvishasleft/manabar/releases/latest`, once per day; nothing POSTed, no identifiers beyond IP.
- Updater HTTP client carries **no auth headers**; never touches credential code paths.
- Download URLs must come from the pinned repo's API response and match prefix `https://github.com/elvishasleft/manabar/releases/download/`.
- Notes URL must match prefix `https://github.com/elvishasleft/manabar/`.
- Version strings validated as numeric dotted segments before any use; rendered only through `esc()`.
- Byte count of download must equal the API-reported asset `size`.
- `update_check: false` disables the check task entirely; nothing installs without a click.
- Run `cargo fmt` before each commit; `cargo clippy -p manabar-core -p manabar --all-targets` must stay clean.

---

### Task 1: `manabar-core::updater` — version comparison

**Files:**
- Create: `crates/manabar-core/src/updater.rs`
- Modify: `crates/manabar-core/src/lib.rs` (add `pub mod updater;` alongside existing `pub mod` lines)
- Test: inline `#[cfg(test)]` in `crates/manabar-core/src/updater.rs`

**Interfaces:**
- Produces: `pub fn is_newer_version(current: &str, candidate_tag: &str) -> bool` and `pub(crate) fn parse_version(tag: &str) -> Option<Vec<u64>>` (used by Task 2).

- [ ] **Step 1: Write the failing tests** — create `crates/manabar-core/src/updater.rs`:

```rust
//! Self-update support: GitHub release parsing and version comparison.
//! Pure logic only — all IO lives in the app crate.

/// Parses `"v1.2.3"` / `"1.2.3"` into numeric segments. Returns `None` for
/// anything that is not purely dotted decimal segments — callers treat that
/// as "not a valid release tag", never as an error to retry.
pub(crate) fn parse_version(tag: &str) -> Option<Vec<u64>> {
    todo!()
}

/// True when `candidate_tag` is a valid version strictly newer than
/// `current`. Malformed input is never newer.
pub fn is_newer_version(current: &str, candidate_tag: &str) -> bool {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_and_minor_and_major_are_newer() {
        assert!(is_newer_version("0.6.1", "v0.6.2"));
        assert!(is_newer_version("0.6.1", "0.7.0"));
        assert!(is_newer_version("0.6.1", "v1.0.0"));
    }

    #[test]
    fn equal_and_older_are_not_newer() {
        assert!(!is_newer_version("0.6.1", "v0.6.1"));
        assert!(!is_newer_version("0.6.1", "0.6.0"));
        assert!(!is_newer_version("0.6.1", "v0.5.9"));
    }

    #[test]
    fn shorter_segments_compare_as_zero_padded() {
        assert!(is_newer_version("0.6", "0.6.1"));
        assert!(!is_newer_version("0.6.0", "0.6"));
    }

    #[test]
    fn malformed_tags_are_never_newer() {
        assert!(!is_newer_version("0.6.1", "banana"));
        assert!(!is_newer_version("0.6.1", "v0.6.2-alpha"));
        assert!(!is_newer_version("0.6.1", ""));
        assert!(!is_newer_version("0.6.1", "0.6.2; rm -rf /"));
    }

    #[test]
    fn parse_version_strips_v_and_rejects_junk() {
        assert_eq!(parse_version("v1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("10.0"), Some(vec![10, 0]));
        assert_eq!(parse_version("v1.2.3-rc1"), None);
        assert_eq!(parse_version("<img src=x>"), None);
    }
}
```

Add `pub mod updater;` to `crates/manabar-core/src/lib.rs`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p manabar-core updater`
Expected: panics at `todo!()` (RED).

- [ ] **Step 3: Implement**

```rust
pub(crate) fn parse_version(tag: &str) -> Option<Vec<u64>> {
    let bare = tag.strip_prefix('v').unwrap_or(tag);
    if bare.is_empty() {
        return None;
    }
    bare.split('.').map(|s| s.parse::<u64>().ok()).collect()
}

pub fn is_newer_version(current: &str, candidate_tag: &str) -> bool {
    let (Some(cur), Some(cand)) = (parse_version(current), parse_version(candidate_tag)) else {
        return false;
    };
    let len = cur.len().max(cand.len());
    for i in 0..len {
        let a = cand.get(i).copied().unwrap_or(0);
        let b = cur.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p manabar-core updater`
Expected: 5 tests PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p manabar-core
git add crates/manabar-core/src/updater.rs crates/manabar-core/src/lib.rs
git commit -m "feat: version comparison for self-update checks"
```

---

### Task 2: `manabar-core::updater` — release parsing and validation

**Files:**
- Create: `crates/manabar-core/tests/fixtures/github_release.json`
- Modify: `crates/manabar-core/src/updater.rs`

**Interfaces:**
- Consumes: `is_newer_version`, `parse_version` from Task 1; `ProviderError` from `crate::model`.
- Produces (used by Tasks 4/5/6):

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateInfo {
    pub version: String,            // no leading 'v'
    pub notes_url: String,          // validated github.com/elvishasleft/manabar/ prefix
    pub asset_url: Option<String>,  // validated .../releases/download/ prefix
    pub asset_size: Option<u64>,
}
pub fn parse_latest_release(body: &str, current_version: &str)
    -> Result<Option<UpdateInfo>, crate::model::ProviderError>;
pub const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/elvishasleft/manabar/releases/latest";
```

- [ ] **Step 1: Create the synthetic fixture** `crates/manabar-core/tests/fixtures/github_release.json`:

```json
{
  "tag_name": "v9.9.9",
  "html_url": "https://github.com/elvishasleft/manabar/releases/tag/v9.9.9",
  "prerelease": false,
  "assets": [
    {
      "name": "ManaBar_9.9.9_aarch64.dmg",
      "size": 1111,
      "browser_download_url": "https://github.com/elvishasleft/manabar/releases/download/v9.9.9/ManaBar_9.9.9_aarch64.dmg"
    },
    {
      "name": "ManaBar_9.9.9_portable.exe",
      "size": 2222,
      "browser_download_url": "https://github.com/elvishasleft/manabar/releases/download/v9.9.9/ManaBar_9.9.9_portable.exe"
    }
  ]
}
```

- [ ] **Step 2: Write the failing tests** — append to the `updater.rs` tests module:

```rust
const FIXTURE: &str = include_str!("../../tests/fixtures/github_release.json");

#[test]
fn parses_release_and_picks_portable_asset() {
    let info = parse_latest_release(FIXTURE, "0.6.1").unwrap().unwrap();
    assert_eq!(info.version, "9.9.9");
    assert_eq!(
        info.notes_url,
        "https://github.com/elvishasleft/manabar/releases/tag/v9.9.9"
    );
    assert!(info.asset_url.as_deref().unwrap().ends_with("_portable.exe"));
    assert_eq!(info.asset_size, Some(2222));
}

#[test]
fn same_or_older_version_is_none() {
    assert!(parse_latest_release(FIXTURE, "9.9.9").unwrap().is_none());
    assert!(parse_latest_release(FIXTURE, "10.0.0").unwrap().is_none());
}

#[test]
fn missing_portable_asset_gives_no_asset_url() {
    let body = r#"{"tag_name":"v9.9.9","html_url":"https://github.com/elvishasleft/manabar/releases/tag/v9.9.9","assets":[]}"#;
    let info = parse_latest_release(body, "0.1.0").unwrap().unwrap();
    assert!(info.asset_url.is_none());
    assert!(info.asset_size.is_none());
}

#[test]
fn foreign_notes_url_is_schema_changed() {
    let body = r#"{"tag_name":"v9.9.9","html_url":"https://evil.example.com/x","assets":[]}"#;
    assert!(parse_latest_release(body, "0.1.0").is_err());
}

#[test]
fn foreign_asset_url_is_dropped_not_fetched() {
    let body = r#"{"tag_name":"v9.9.9","html_url":"https://github.com/elvishasleft/manabar/releases/tag/v9.9.9","assets":[{"name":"ManaBar_9.9.9_portable.exe","size":5,"browser_download_url":"https://evil.example.com/ManaBar_9.9.9_portable.exe"}]}"#;
    let info = parse_latest_release(body, "0.1.0").unwrap().unwrap();
    assert!(info.asset_url.is_none());
}

#[test]
fn malformed_tag_is_schema_changed() {
    let body = r#"{"tag_name":"nightly","html_url":"https://github.com/elvishasleft/manabar/releases/tag/nightly","assets":[]}"#;
    assert!(parse_latest_release(body, "0.1.0").is_err());
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p manabar-core updater`
Expected: compile error (missing items). Add `todo!()` stubs matching the Produces signatures if you want a running RED first.

- [ ] **Step 4: Implement** — append to `updater.rs`:

```rust
use crate::model::ProviderError;
use serde::Deserialize;

pub const RELEASES_LATEST_URL: &str =
    "https://api.github.com/repos/elvishasleft/manabar/releases/latest";
const NOTES_PREFIX: &str = "https://github.com/elvishasleft/manabar/";
const ASSET_PREFIX: &str = "https://github.com/elvishasleft/manabar/releases/download/";

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes_url: String,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
}

#[derive(Deserialize)]
struct AssetRaw {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct ReleaseRaw {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<AssetRaw>,
}

/// Parses `releases/latest`. `Ok(None)` = up to date. Any field that fails
/// its allowlist (numeric tag, pinned-repo URLs) is rejected — a bad notes
/// URL fails the whole parse, a bad asset URL just drops one-click support.
pub fn parse_latest_release(
    body: &str,
    current_version: &str,
) -> Result<Option<UpdateInfo>, ProviderError> {
    let raw: ReleaseRaw = serde_json::from_str(body)
        .map_err(|e| ProviderError::SchemaChanged(format!("github release: {e}")))?;
    if parse_version(&raw.tag_name).is_none() {
        return Err(ProviderError::SchemaChanged(format!(
            "github release: unexpected tag {:?}",
            raw.tag_name
        )));
    }
    if !raw.html_url.starts_with(NOTES_PREFIX) {
        return Err(ProviderError::SchemaChanged(
            "github release: html_url outside pinned repo".into(),
        ));
    }
    if !is_newer_version(current_version, &raw.tag_name) {
        return Ok(None);
    }
    let asset = raw.assets.iter().find(|a| {
        a.name.ends_with("_portable.exe") && a.browser_download_url.starts_with(ASSET_PREFIX)
    });
    Ok(Some(UpdateInfo {
        version: raw.tag_name.trim_start_matches('v').to_string(),
        notes_url: raw.html_url,
        asset_url: asset.map(|a| a.browser_download_url.clone()),
        asset_size: asset.map(|a| a.size),
    }))
}
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p manabar-core updater`
Expected: all 11 updater tests PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt -p manabar-core
git add crates/manabar-core/src/updater.rs crates/manabar-core/tests/fixtures/github_release.json
git commit -m "feat: parse and validate github releases/latest for updates"
```

---

### Task 3: config flag `update_check`

**Files:**
- Modify: `src-tauri/src/config.rs` (struct `Config` ~line 55, `impl Default` ~line 70, tests at bottom)

**Interfaces:**
- Produces: `Config.update_check: bool` (default `true`), consumed by Task 4.

- [ ] **Step 1: Write the test** — in the existing `#[cfg(test)]` module of `config.rs` add:

```rust
#[test]
fn update_check_defaults_true_and_parses_false() {
    assert!(Config::default().update_check);
    let cfg: Config = serde_json::from_str(r#"{"update_check":false}"#).unwrap();
    assert!(!cfg.update_check);
}
```

- [ ] **Step 2: Implement** — add to the `Config` struct:

```rust
    /// Daily anonymous check of the GitHub releases feed. `false` disables
    /// the network call entirely (privacy opt-out; see the spec's security
    /// section).
    pub update_check: bool,
```

and `update_check: true,` in `impl Default for Config`. The struct-level `#[serde(default)]` fills missing fields from `Config::default()`, so existing config files without the key deserialize to `true`.

- [ ] **Step 3: Verify it compiles (app-crate tests execute in CI only)**

Run: `cargo check -p manabar && cargo clippy -p manabar 2>&1 | tail -3`
Expected: clean. (App-crate test binaries do not load on this machine.)

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add src-tauri/src/config.rs
git commit -m "feat: update_check config flag (default on)"
```

---

### Task 4: app-side check task, shared state, startup cleanup

**Files:**
- Create: `src-tauri/src/updater.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod updater;`; in `.setup()` after `let cfg = config::load(&cfg_path);` add the three wiring lines shown in Step 2)

**Interfaces:**
- Consumes: `manabar_core::updater::{parse_latest_release, UpdateInfo, RELEASES_LATEST_URL}`.
- Produces: `pub struct UpdateShared(pub tokio::sync::Mutex<Option<UpdateInfo>>)` managed in tauri state; event `"update"` with payload `Option<UpdateInfo>`; `pub fn spawn(app: AppHandle, enabled: bool)`; `pub fn cleanup_old_exe()`.

- [ ] **Step 1: Implement** `src-tauri/src/updater.rs`:

```rust
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
```

- [ ] **Step 2: Wire into `lib.rs`** — add `mod updater;` near the other `mod` lines; in `.setup()` right after `let cfg = config::load(&cfg_path);` add:

```rust
            updater::cleanup_old_exe();
            app.manage(updater::UpdateShared::default());
            updater::spawn(app.handle().clone(), cfg.update_check);
```

- [ ] **Step 3: Verify**

Run: `cargo check -p manabar && cargo clippy -p manabar --all-targets 2>&1 | tail -3 && cargo test -p manabar-core`
Expected: clean; core tests PASS.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add src-tauri/src/updater.rs src-tauri/src/lib.rs
git commit -m "feat: daily update check task with panel event"
```

---

### Task 5: commands — `update_status`, `open_release_notes`, `apply_update`

**Files:**
- Modify: `src-tauri/Cargo.toml` (add `tauri-plugin-opener = "2"` to `[dependencies]`)
- Modify: `src-tauri/src/lib.rs` (add `.plugin(tauri_plugin_opener::init())` beside the other plugins; extend `generate_handler![...]` with `commands::update_status, commands::open_release_notes, commands::apply_update`)
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/updater.rs` (swap implementation; commands delegate)

**Interfaces:**
- Consumes: `UpdateShared` from Task 4.
- Produces (frontend contract, Task 7): `invoke<UpdateInfo | null>("update_status")`, `invoke("open_release_notes")`, `invoke("apply_update")` — `apply_update` returns `Err(String)` on failure; on success the process restarts and never resolves.

- [ ] **Step 1: Add commands** to `commands.rs`:

```rust
#[tauri::command]
pub async fn update_status(
    upd: tauri::State<'_, crate::updater::UpdateShared>,
) -> Result<Option<manabar_core::updater::UpdateInfo>, String> {
    Ok(upd.0.lock().await.clone())
}

/// Opens the stored, already-validated release page. The URL is never taken
/// from the frontend — the webview cannot pass an arbitrary URL here.
#[tauri::command]
pub async fn open_release_notes(
    app: tauri::AppHandle,
    upd: tauri::State<'_, crate::updater::UpdateShared>,
) -> Result<(), String> {
    let url = upd
        .0
        .lock()
        .await
        .as_ref()
        .map(|u| u.notes_url.clone())
        .ok_or("no update available")?;
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn apply_update(
    app: tauri::AppHandle,
    upd: tauri::State<'_, crate::updater::UpdateShared>,
) -> Result<(), String> {
    let info = upd.0.lock().await.clone().ok_or("no update available")?;
    crate::updater::apply(app, info).await
}
```

(If `OpenerExt` differs in the shipped `tauri-plugin-opener` 2.x, use the free function `tauri_plugin_opener::open_url(url, None::<&str>)` — check the crate docs at the pinned version and use whichever compiles. No capability change is needed for Rust-side calls.)

- [ ] **Step 2: Add swap implementation** to `src-tauri/src/updater.rs`:

```rust
/// Windows: download → size-check → rename dance → relaunch. Every failure
/// rolls back to the pre-step state and returns Err for the panel to show.
#[cfg(windows)]
pub async fn apply(app: AppHandle, info: UpdateInfo) -> Result<(), String> {
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
        let _ = std::fs::rename(&old, &exe);
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
pub async fn apply(app: AppHandle, info: UpdateInfo) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(info.notes_url, None::<&str>)
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 3: Verify + Commit**

Run: `cargo check -p manabar && cargo clippy -p manabar --all-targets 2>&1 | tail -3 && cargo fmt`
Expected: clean.

```bash
git add src-tauri/Cargo.toml src-tauri/src/commands.rs src-tauri/src/updater.rs src-tauri/src/lib.rs Cargo.lock
git commit -m "feat: apply_update swap command and release-notes opener"
```

---

### Task 6: panel render layer (pure, tested)

**Files:**
- Modify: `src/render.ts`
- Test: `src/render.test.ts` (vitest — runs locally with `npm test`)

**Interfaces:**
- Produces (consumed by Task 7):

```ts
export interface UpdateInfo { version: string; notes_url: string }
export type UpdatePhase = "idle" | "busy" | "error";
export function updateNotice(u: UpdateInfo | null, phase: UpdatePhase, error?: string): string;
```

- [ ] **Step 1: Write failing tests** — append to `src/render.test.ts` (match the file's existing import style):

```ts
import { updateNotice } from "./render";

describe("updateNotice", () => {
  it("renders nothing when no update", () => {
    expect(updateNotice(null, "idle")).toBe("");
  });

  it("renders version, update button, and notes link", () => {
    const html = updateNotice({ version: "9.9.9", notes_url: "https://github.com/elvishasleft/manabar/releases/tag/v9.9.9" }, "idle");
    expect(html).toContain("v9.9.9 available");
    expect(html).toContain('class="update-btn"');
    expect(html).toContain("update-notes");
  });

  it("escapes hostile version strings", () => {
    const html = updateNotice({ version: "<img src=x onerror=1>", notes_url: "https://github.com/elvishasleft/manabar/x" }, "idle");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });

  it("busy phase disables the button and shows progress", () => {
    const html = updateNotice({ version: "9.9.9", notes_url: "https://github.com/elvishasleft/manabar/x" }, "busy");
    expect(html).toContain("disabled");
    expect(html).toContain("downloading…");
  });

  it("error phase renders the escaped error", () => {
    const html = updateNotice({ version: "9.9.9", notes_url: "https://github.com/elvishasleft/manabar/x" }, "error", "<b>boom</b>");
    expect(html).toContain("&lt;b&gt;boom&lt;/b&gt;");
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npm test`
Expected: FAIL — `updateNotice` is not exported.

- [ ] **Step 3: Implement** — in `src/render.ts`, reusing the module's existing `esc()` helper:

```ts
export interface UpdateInfo {
  version: string;
  notes_url: string;
}
export type UpdatePhase = "idle" | "busy" | "error";

// Update notice line for the footer. Version and error text are attacker-ish
// inputs (compromised-release scenario) — everything goes through esc().
export function updateNotice(
  u: UpdateInfo | null,
  phase: UpdatePhase,
  error?: string,
): string {
  if (!u) return "";
  const btn =
    phase === "busy"
      ? `<button class="update-btn" disabled>downloading…</button>`
      : `<button class="update-btn">Update</button>`;
  const err = phase === "error" && error ? ` <span class="update-err">${esc(error)}</span>` : "";
  return `<div class="update-line">v${esc(u.version)} available · ${btn} · <button class="update-notes">notes ↗</button>${err}</div>`;
}
```

- [ ] **Step 4: Run to verify pass**

Run: `npm test`
Expected: all vitest tests PASS (existing + 5 new).

- [ ] **Step 5: Commit**

```bash
git add src/render.ts src/render.test.ts
git commit -m "feat: escaped update-notice renderer for panel footer"
```

---

### Task 7: panel wiring + styles

**Files:**
- Modify: `src/main.ts`
- Modify: `src/style.css`

**Interfaces:**
- Consumes: `updateNotice`, `UpdateInfo`, `UpdatePhase` (Task 6); event `"update"` (Task 4); commands (Task 5).

- [ ] **Step 1: Wire in `src/main.ts`:**

Extend the render import with `updateNotice, type UpdateInfo, type UpdatePhase`. Add module state below `let current`:

```ts
let updateInfo: UpdateInfo | null = null;
let updatePhase: UpdatePhase = "idle";
let updateError: string | undefined;
```

In `render()`, change the innerHTML line to include the notice inside the footer:

```ts
  document.querySelector<HTMLElement>("#app")!.innerHTML = asRenderedHtml(
    visible.map(card).join("") +
      `<footer>updated ${age(updated) || "—"}${updateNotice(updateInfo, updatePhase, updateError)}</footer>`,
  );
```

Add listeners/bootstrap next to the existing `state` listener:

```ts
listen<UpdateInfo | null>("update", (e) => {
  updateInfo = e.payload;
  updatePhase = "idle";
  updateError = undefined;
  if (current.length) render(current);
});
invoke<UpdateInfo | null>("update_status")
  .then((u) => {
    updateInfo = u;
    if (current.length) render(current);
  })
  .catch(() => {});
```

Extend the existing delegated `#app` click listener with two branches before the `.refresh-btn` branch:

```ts
  const notes = (e.target as HTMLElement).closest<HTMLButtonElement>(".update-notes");
  if (notes) {
    invoke("open_release_notes").catch((err) => console.error("open_release_notes failed", err));
    return;
  }
  const upd = (e.target as HTMLElement).closest<HTMLButtonElement>(".update-btn");
  if (upd && !upd.disabled && updatePhase !== "busy") {
    updatePhase = "busy";
    if (current.length) render(current);
    invoke("apply_update").catch((err) => {
      updatePhase = "error";
      updateError = String(err);
      if (current.length) render(current);
    });
    return;
  }
```

- [ ] **Step 2: Styles** — read `src/style.css` first and reuse its existing color tokens; append:

```css
.update-line {
  margin-top: 6px;
  font-size: 12px;
}
.update-btn,
.update-notes {
  background: none;
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 6px;
  color: inherit;
  cursor: pointer;
  font-size: 11px;
  padding: 1px 8px;
}
.update-btn:disabled {
  opacity: 0.6;
  cursor: default;
}
.update-err {
  color: #e5534b;
}
```

(Substitute the repo's actual CSS custom properties for the border and red literals if `style.css` defines them.)

- [ ] **Step 3: Verify**

Run: `npm test && npm run build && cargo check -p manabar`
Expected: vitest green, `tsc && vite build` clean, cargo clean.

- [ ] **Step 4: Commit**

```bash
git add src/main.ts src/style.css
git commit -m "feat: update notice and one-click update in panel footer"
```

---

### Task 8: docs, release v0.7.0, install

**Files:**
- Modify: `README.md` (new `## Updates` section)
- Modify: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `Cargo.lock` (0.6.1 → 0.7.0)

- [ ] **Step 1: README section** (place after the Configuration section):

```markdown
## Updates

Once a day ManaBar makes a single anonymous request to the GitHub
releases feed of this repository to see whether a newer version exists.
Nothing is sent beyond the request itself — no identifiers, no usage
data, no telemetry — and nothing is ever installed without you clicking
**Update**. When a new version exists the panel footer shows
`vX.Y.Z available · Update · notes ↗`; on Windows, **Update** downloads
the portable exe from this repository's GitHub Releases (byte count
verified against the API-reported size), swaps it in place, and
restarts; on macOS it opens the Releases page. Set
`"update_check": false` in `config.json` to disable the check entirely.
```

- [ ] **Step 2: Bump versions** — `sed` 0.6.1 → 0.7.0 in `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, run `cargo check -p manabar` to sync `Cargo.lock`.

- [ ] **Step 3: Full local gate**

Run: `cargo test -p manabar-core && npm test && npm run build && cargo clippy -p manabar-core -p manabar --all-targets 2>&1 | tail -3 && cargo fmt`
Expected: all green.

- [ ] **Step 4: Commit, push, watch CI**

```bash
git add README.md src-tauri/Cargo.toml src-tauri/tauri.conf.json Cargo.lock
git commit -m "chore: bump version to 0.7.0"
git push
```

Wait for the `CI` workflow on the pushed commit to be green (it runs the app-crate tests this machine cannot).

- [ ] **Step 5: Tag + release + install**

```bash
git tag v0.7.0 && git push origin v0.7.0
# wait for release workflows to publish ManaBar_0.7.0_portable.exe
# then: stop manabar.exe, download the asset over
#   C:\Users\Elvis\AppData\Local\ManaBar\manabar.exe, relaunch
```

Verify: the running panel is v0.7.0 and shows no update line (nothing newer exists — correct idle state). The one-click path gets its real-world test on the next release.
