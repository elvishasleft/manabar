# QuotaBar

A Windows system-tray app that shows, at a glance, how much subscription
quota you have left on three AI coding assistants — **Claude Code**
(Anthropic), **Codex CLI** (OpenAI), and **Grok CLI** (xAI) — plus
**DeepSeek** account balance, via a flyout panel with per-window detail
and estimated token/cost usage over the last 7 days.

![QuotaBar panel](docs/screenshot.png)

No click required to check quota: the tray icon itself is the status
readout, redrawn after every poll.

## Tray icon legend

The icon is four vertical bars, always in this order: **Claude, Codex,
Grok, DeepSeek**. Bar height is remaining quota; bar color is health,
based on the *binding constraint* (the lowest remaining percentage
across that provider's rate-limit windows — for DeepSeek, its single
`Balance` window):

| Color | Meaning |
|---|---|
| green | remaining > 30% |
| amber | 10% < remaining ≤ 30% |
| red | remaining ≤ 10% |
| gray (dimmed, full-height) | unavailable — no credentials, expired sign-in, network error, or the endpoint's response no longer parses |

Hovering the icon shows a tooltip, e.g.
`Claude 82% · Codex 35% · Grok 95% · DeepSeek 60%` (unavailable
providers show as `Codex —`). Left-click opens the panel; right-click
gives `Refresh now`, `Start with Windows`, and `Quit`.

## Requirements

- Windows 10/11.
- At least one of the following, signed in locally:
  - [Claude Code](https://docs.claude.com/en/docs/claude-code) (`%USERPROFILE%\.claude\`)
  - [Codex CLI](https://github.com/openai/codex) (`%USERPROFILE%\.codex\`)
  - [Grok CLI](https://github.com/superagent-ai/grok-cli) (`%USERPROFILE%\.grok\`)
  - [DeepSeek](https://platform.deepseek.com/) API key, optional, via the
    `DEEPSEEK_API_KEY` environment variable (or the `deepseek_api_key`
    config fallback) — DeepSeek has no local CLI/credential file; it's
    a bare API key against DeepSeek's own official balance endpoint.

  A provider you haven't installed/signed into (or, for DeepSeek, don't
  have a key for) just shows as unavailable (gray) — QuotaBar doesn't
  require all four.

## Install

This repo is currently private with no published GitHub Release yet, so
build from source:

```bash
git clone https://github.com/arteeeezy/quotabar.git
cd quotabar
npm install
npm run tauri build
```

This produces two bundles under the workspace target directory:

- NSIS installer: `target/release/bundle/nsis/QuotaBar_<version>_x64-setup.exe`
- MSI installer: `target/release/bundle/msi/QuotaBar_<version>_x64_en-US.msi`

Run either one and QuotaBar starts in the tray. The NSIS installer is
per-user (no admin required, installs under `%LOCALAPPDATA%`); the MSI is
per-machine (elevation prompt, installs to `C:\Program Files\QuotaBar`).
To uninstall, use Windows' "Add or remove programs", or run
the uninstaller next to the installed exe (NSIS default:
`%LOCALAPPDATA%\QuotaBar\uninstall.exe`).

Note for GNU-toolchain builds: if you compile with
`x86_64-pc-windows-gnu` instead of the default MSVC toolchain, install
from the MSI — the NSIS bundle currently omits `WebView2Loader.dll`,
which GNU builds load dynamically, and the installed app will fail to
start without it. MSVC builds link it statically and are unaffected.
The same applies to running `target/release/quotabar.exe` directly on a
GNU toolchain ("The code execution cannot proceed because
WebView2Loader.dll was not found"): copy the DLL next to the exe once —

```bash
cp target/release/build/webview2-com-sys-*/out/x64/WebView2Loader.dll target/release/
```

Once this repo is public, a prebuilt installer will be attached to
GitHub Releases instead.

## Configuration

QuotaBar reads `%APPDATA%\quotabar\config.json` once at startup (created
with defaults on first run if missing; invalid JSON falls back to defaults
rather than crashing). There is no in-app settings UI in v1 — edit the
file and restart QuotaBar for changes (including `poll_interval_secs`) to
take effect. `Refresh now` does not reload config; it only triggers an
immediate extra poll using the interval already in memory.

```json
{
  "poll_interval_secs": 1800,
  "enabled": {
    "claude": true,
    "codex": true,
    "grok": true,
    "deepseek": true
  },
  "price_overrides": [
    {
      "model_contains": "claude-opus",
      "input": 15.0,
      "output": 75.0,
      "cache_read": 1.5,
      "cache_write": 18.75
    }
  ],
  "deepseek_api_key": null,
  "deepseek_budget": null
}
```

| Field | Type | Default | Meaning |
|---|---|---|---|
| `poll_interval_secs` | number | `1800` | Seconds between quota-endpoint polls. `0` enables **on-demand mode**: no periodic background polling — QuotaBar polls once at startup (so the tray has a baseline), then refreshes only when the panel is opened or tray `Refresh now` is clicked; tray levels stay at their last-known value between opens. Values 1-59 are clamped to 60. Opening the panel triggers an immediate usage refresh plus a quota poll; tray `Refresh now` triggers an immediate quota poll. |
| `enabled.claude` / `enabled.codex` / `enabled.grok` / `enabled.deepseek` | bool | `true` | Set to `false` to disable polling for that provider. The tray bar still renders, shown as gray/unavailable, and the panel still shows an empty card for it — fully hiding the bar/card is deferred to a future release. |
| `price_overrides` | array | `[]` | Per-model USD price overrides (per million tokens: `input`, `output`, `cache_read`, `cache_write`). `model_contains` is a substring match checked before the built-in price table, first match wins. |
| `deepseek_api_key` | string or `null` | `null` | Fallback DeepSeek API key, used only when the `DEEPSEEK_API_KEY` environment variable isn't set (or is blank). The environment variable always wins when present. |
| `deepseek_budget` | number or `null` | `null` | Optional total balance budget (same currency as your DeepSeek account, e.g. CNY) for turning the `Balance` window into a real used-percent gauge: `used% = (1 - balance/budget) × 100`, clamped 0-100. Without a budget, DeepSeek's card is a binary green/red signal from the account's own `is_available` flag — 0% used while usable, 100% once DeepSeek reports it can't serve requests. |

## How it works

QuotaBar never asks you to sign in. It reads the credential files your
CLIs already maintain (read-only, never written or refreshed) and calls
the same unofficial usage endpoints those CLIs use internally:

| Provider | Credentials (read-only) | Quota endpoint | Cumulative usage source |
|---|---|---|---|
| Claude Code | `%USERPROFILE%\.claude\.credentials.json` | `GET https://api.anthropic.com/api/oauth/usage` (Bearer token + `anthropic-beta: oauth-2025-04-20`) — primary: `limits[]` array (`session` / `weekly_all` / `weekly_scoped`, the latter carrying a per-model `scope.model.display_name` weekly window); fallback: legacy `five_hour`, `seven_day`, `seven_day_sonnet`, `seven_day_opus`, `extra_usage`; plan from `subscriptionType` | `%USERPROFILE%\.claude\projects\**\*.jsonl` |
| Codex CLI | `%USERPROFILE%\.codex\auth.json`, or `$CODEX_HOME\auth.json` when the `CODEX_HOME` environment variable is set | `GET https://chatgpt.com/backend-api/wham/usage` (Bearer token) — `rate_limit.primary_window` (5h), `secondary_window` (weekly), `additional_rate_limits[]` (per-model, paid plans) | `%USERPROFILE%\.codex\sessions\**\*.jsonl` |
| Grok CLI | `%USERPROFILE%\.grok\auth.json` (keyed by `issuer::client_id`, token field `key`) | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` (`config.creditUsagePercent`, `currentPeriod.end`) + `GET .../v1/settings` (plan label) | `%USERPROFILE%\.grok\sessions\**\signals.json` (`contextTokensUsed`, `primaryModelId`) |
| DeepSeek | `DEEPSEEK_API_KEY` environment variable, or `deepseek_api_key` in config | `GET https://api.deepseek.com/user/balance` (Bearer token) — **the one official, documented endpoint of the four**; `balance_infos[0].total_balance` (a string) becomes the `Balance` window and the plan pill (e.g. `¥97.97`) | none — DeepSeek's card has no usage/sparkline block (`usage: null`); driven internally by omp, out of scope |

Claude, Codex, and Grok's endpoints are **unofficial** —
reverse-engineered from the CLIs' own network traffic, not documented
or supported by Anthropic, OpenAI, or xAI. DeepSeek's `/user/balance`
is the exception: it's official and documented. The unofficial
endpoints can change or be gated at any time without notice. When that
happens, QuotaBar doesn't guess: a response that returns `2xx` but no
longer parses puts that provider into a distinct `SchemaChanged` state,
shown in the tray as gray and in the panel as "Endpoint changed —
needs an update," rather than showing a silently wrong number. Other
failure states (`NoCredentials`, `TokenExpired`, `Network`) are
surfaced the same way, each with a specific fix hint. One provider
failing never affects the others.

Rate-limit windows (5h / weekly / per-model) are derived from each
response's own window metadata, not hard-coded — a free-plan account
that only exposes one 30-day window is rendered as one window, not
padded out to match a paid plan's shape.

### Cost estimates

The panel's cumulative cost numbers are **estimates**: token counts
parsed from each CLI's own local session logs, multiplied by a static
per-model price table (`crates/quotabar-core/src/pricing.rs`),
overridable via `price_overrides` in the config. They are not billing
data and won't exactly match your provider invoice. Grok's usage is
credits-based rather than token-priced, so its panel shows token counts
only — cost is displayed as "—". DeepSeek has no local session-log
source at all (it's driven internally by omp), so its card omits the
usage/sparkline block entirely — its own account balance is the whole
story.

## Development

```bash
npm install                 # frontend deps
cargo test --workspace      # unit + integration tests (quotabar-core + src-tauri)
cargo fmt --check           # formatting
cargo clippy --workspace --all-targets -- -D warnings
npm run tauri dev           # run the app locally
npm run tauri build         # release build + NSIS/MSI installers
```

`quotabar-core` is a plain Rust library crate (no Tauri dependency), so
its provider parsers and quota/usage math are unit-testable without a
GUI. Provider integration tests use `wiremock` fixtures for the happy
path, 401, timeout, and malformed-JSON cases.

Three tests hit real endpoints with your local, already-signed-in
credentials and are `#[ignore]`d by default — run them manually to
smoke-test against the live services after a suspected upstream change:

```bash
cargo test --workspace -- --ignored live_
```

## License

MIT — see [LICENSE](LICENSE).
