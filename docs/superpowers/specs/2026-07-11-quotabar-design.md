# QuotaBar — Design Spec

**Date:** 2026-07-11
**Status:** Approved pending user review
**Working name:** `quotabar` (subject to rename before publishing)

## Overview

QuotaBar is a Windows system-tray application that shows, at a glance, the
remaining subscription quota for three AI coding assistants — Claude Code
(Anthropic), Codex CLI (OpenAI), and Grok CLI (xAI) — and, on demand,
cumulative token/cost consumption per provider.

No existing Windows tool covers all three providers (verified 2026-07-11:
the closest are CodeZeno/Claude-Code-Usage-Monitor and babakarto/CodexBar-Win,
both Claude + Codex only). This project fills that gap and doubles as a
portfolio piece: clean provider-plugin architecture, tested core, CI,
distributable installer.

### Goals

1. One tray icon that shows each provider's remaining quota without any click.
2. A flyout panel with per-provider detail: rate-limit windows, reset
   countdowns, and cumulative usage (tokens + estimated cost, last 7 days).
3. Portfolio-grade engineering: tested core library, CI, reproducible builds.

### Non-goals (v1)

- Providers beyond Claude / Codex / Grok (architecture allows adding them later).
- macOS / Linux support.
- Browser-cookie credential fallbacks (all three providers have CLI credential
  files on this machine; cookie decryption is out of scope).
- Refreshing OAuth tokens ourselves (see Token policy).
- Desktop notifications / low-quota alerts (future work).
- Historical usage database beyond what local logs already contain.

## 1. Product shape

- A single tray icon rendered as **three vertical bars** (fixed order:
  Claude, Codex, Grok). Bar height = remaining quota; bar color = health:
  - green: remaining > 30%
  - amber: 10% < remaining ≤ 30%
  - red: remaining ≤ 10%
  - gray (dimmed full-height bar): provider unavailable (no credentials,
    stale token, network error, or parse failure)
- Per provider, the bar reflects the **binding constraint**: the minimum
  remaining percentage across that provider's rate-limit windows
  (e.g. min(five_hour, seven_day) for Claude).
- The icon is generated at runtime as RGBA (rendered at 32×32, scaled by the
  OS), redrawn after every poll.
- Tooltip on hover: `Claude 82% · Codex 35% · Grok 95%` (unavailable
  providers shown as `Codex —`).
- Left-click: opens the flyout panel near the tray; panel auto-hides on
  focus loss.
- Right-click menu: `Refresh now` / `Start with Windows` (toggle) / `Quit`.

## 2. Architecture

Tauri v2 application. Rust owns all logic; the webview renders the panel only.

```
quotabar/
├── src-tauri/
│   ├── crates/quotabar-core/     # pure Rust library, no Tauri dependency
│   │   ├── providers/            # Provider trait + claude / codex / grok
│   │   ├── usage_logs/           # local JSONL aggregation (cumulative stats)
│   │   ├── pricing.rs            # static price table for cost estimates
│   │   └── state.rs              # ProviderStatus state machine
│   └── src/                      # Tauri shell: tray, poller, panel window,
│                                 #   icon renderer, config, autostart
├── src/                          # panel frontend (TypeScript + CSS, no heavy framework)
└── docs/
```

### Provider trait

Each provider implements one trait and produces two data types:

- `QuotaSnapshot` — per rate-limit window: used percentage, resets_at
  timestamp; plus plan label (e.g. "Max", "Plus", "SuperGrok").
- `UsageStats` — last 7 days, aggregated per day: input/output tokens and
  estimated cost in USD.

The trait exposes `fetch_quota()` and `fetch_usage_stats()`; both return
`Result<_, ProviderError>` where `ProviderError` distinguishes
`NoCredentials / TokenExpired / Network / SchemaChanged`.

### Polling

- Quota endpoints: every 30 minutes (configurable), plus immediately when the
  panel opens and on `Refresh now` — so tray colors may lag by up to the
  poll interval, but the panel is always fresh on open.
- Usage-log aggregation: on panel open, with an mtime-based incremental cache
  (only re-parse JSONL files whose mtime changed; keep per-file running
  totals so unchanged files are never re-read).
- Each provider polls independently; one provider failing never blocks or
  delays the others.

## 3. Data sources

All endpoints are unofficial (reverse-engineered) and were cross-verified on
2026-07-11 against steipete/CodexBar `docs/`, CodeZeno, and openusage
implementations. All three credential files exist on the target machine.

| Provider | Credentials (read-only) | Quota endpoint | Cumulative source |
|---|---|---|---|
| Claude Code | `%USERPROFILE%\.claude\.credentials.json` | `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <access_token>` + `anthropic-beta: oauth-2025-04-20`. Fields: `five_hour`, `seven_day`, `seven_day_sonnet`, `seven_day_opus`, `extra_usage`; plan from `subscriptionType` / `rate_limit_tier`. | `%USERPROFILE%\.claude\projects\**\*.jsonl` |
| Codex CLI | `%USERPROFILE%\.codex\auth.json` (or `$CODEX_HOME`) | `GET https://chatgpt.com/backend-api/wham/usage` with Bearer token. `rate_limit.primary_window` → 5h window, `secondary_window` → weekly; `additional_rate_limits[]` → per-model limits. | `%USERPROFILE%\.codex\sessions\**\*.jsonl` |
| Grok CLI | `%USERPROFILE%\.grok\auth.json` | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` (weekly pool + pay-as-you-go cap) + `GET .../v1/settings` (plan name). | `%USERPROFILE%\.grok\logs\unified.jsonl` |

### Token policy: read-only, never refresh

Credential files are re-read from disk on every poll. On HTTP 401 the
provider enters `TokenExpired` state with a hint ("open Claude Code / run
`codex` / run `grok` to refresh sign-in"). QuotaBar never writes credential
files and never performs OAuth refresh itself — the owning CLIs refresh
their own tokens; racing them risks corrupting sign-in state (approach
validated by CodeZeno).

### Cost estimation

Cumulative cost is an **estimate**: tokens from local logs × a static price
table (per model, per direction) compiled into `pricing.rs` and overridable
via config. The panel labels these numbers "est.".

## 4. Panel UI

Frameless Tauri window (~380×560), positioned above the tray icon,
auto-hides on blur. Content: three provider cards, each with:

- Header: provider name, plan label, status (or error state + fix hint).
- Quota section: one horizontal bar per rate-limit window (5h, weekly,
  per-model weekly where present) with used %, and reset countdown
  (`resets in 2h 14m`).
- Cumulative section: today's tokens and estimated cost; 7-day sparkline.

Light/dark follows the system theme. Visual design will be produced with the
`frontend-design` skill during implementation (deliberate direction, no
default-template look); this spec constrains layout and content only.

## 5. Error handling

Per-provider state machine, rendered in both tray (gray bar) and panel:

| State | Trigger | Panel copy |
|---|---|---|
| `Ok` | 2xx + parse success | normal card |
| `NoCredentials` | credential file missing | "Not installed / not signed in" |
| `TokenExpired` | 401 | "Sign-in expired — open <CLI> to refresh" |
| `Network` | timeout / connection error | "Offline — retrying" (keeps last good data, stamped with age) |
| `SchemaChanged` | 2xx but response fails to parse | "Endpoint changed — needs an update" |

Principles: a provider failure never affects other providers or crashes the
tray; last known good snapshot is kept and displayed with its age; all HTTP
calls have timeouts (10 s) and one retry with backoff. Errors are logged to
a rotating file in the app data directory.

**Key risk:** all three quota endpoints are unofficial and may change or be
gated at any time. Mitigations: the `Provider` trait isolates the blast
radius to one module; `SchemaChanged` is a first-class visible state (never
silently wrong numbers); steipete/CodexBar `docs/` is tracked as the
upstream reference for endpoint changes.

## 6. Engineering standards

- **Repo:** new GitHub repo under `arteeeezy/`, private first, open-source
  ready (English code, commits, docs; conventional commits).
- **Layout:** `quotabar-core` is a pure library crate — no Tauri types — so
  all core logic is testable without the GUI.
- **Testing** (target ≥ 80% line coverage on `quotabar-core`):
  - Unit tests for every response parser using captured-JSON fixtures
    (real responses with tokens redacted).
  - Unit tests for quota math (binding-constraint selection, thresholds,
    countdown formatting) and JSONL aggregation (incl. mtime cache).
  - Integration tests with `wiremock`: happy path, 401, timeout, malformed
    JSON for each provider.
  - Frontend: panel is a thin renderer over serialized state; no UI test
    framework in v1.
- **CI:** GitHub Actions — `cargo fmt --check`, `clippy -D warnings`,
  `cargo test`, Windows release build.
- **Packaging:** Tauri bundler → NSIS installer; autostart via the official
  `tauri-plugin-autostart`.
- **Config:** `%APPDATA%\quotabar\config.json` — poll interval, enabled
  providers, price-table overrides, autostart flag.

## Decisions log

| Decision | Choice | Alternatives considered |
|---|---|---|
| Positioning | Portfolio project | personal-use script |
| Stack | Rust + Tauri v2 | fork CodexBar-Win (Python); pure Rust egui; C#/WinUI 3 |
| Tray icon | Three vertical bars (per-provider) | single number; ring; battery metaphor |
| Grok data path | CLI credentials (`~/.grok/auth.json`) | grok.com browser-cookie path (not needed — user runs Grok CLI) |
| Token handling | read-only, CLIs own refresh | self-refresh (rejected: write races with CLIs) |
