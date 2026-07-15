# ManaBar Self-Update — Design

Date: 2026-07-15
Status: approved

## Goal

ManaBar has no way to tell users a new version exists. Add an in-panel
update notice and a one-click updater for the Windows portable build.

## Update check

- New module `src-tauri/src/updater.rs`.
- 30 s after startup, then every 24 h: `GET
  https://api.github.com/repos/elvishasleft/manabar/releases/latest`
  (anonymous; requires a `User-Agent` header; one call per day is far
  below the 60/h anonymous limit).
- Compare `tag_name` (strip leading `v`) against `CARGO_PKG_VERSION`
  using a small hand-rolled numeric segment comparison — no new
  dependency. `releases/latest` already excludes prereleases and
  drafts.
- Result is carried on the existing panel snapshot payload as
  `update: { version: string, notes_url: string } | null`.
- New config key `update_check: bool`, default `true`. When `false`,
  the check task never runs.

## Panel notice (src/render.ts)

- When `update` is non-null the footer shows one extra line:
  `vX.Y.Z available · [Update] · notes ↗`.
- `notes ↗` opens the release page in the default browser via the
  official `tauri-plugin-opener` plugin (new dependency, also used by
  the macOS update path).
- No update → no extra UI. Never blocks or interrupts.
- While `apply_update` runs the button label switches to
  `downloading…` and the button is disabled to prevent double clicks.
- Errors from `apply_update` render inline in the same footer line.

## One-click update (`apply_update` command)

Windows (portable swap):

1. From the same `releases/latest` response, pick the asset whose name
   ends in `_portable.exe`. Only URLs returned by `api.github.com` for
   the hardcoded `elvishasleft/manabar` repo are ever fetched.
2. Download to `<exe_dir>\manabar.exe.new`.
3. Verify the byte count matches the asset `size` reported by the API.
4. Rename running `manabar.exe` → `manabar.exe.old` (Windows allows
   renaming a running image), rename `.new` → `manabar.exe`.
5. Spawn the new exe, exit the old process.
6. On startup, best-effort delete a leftover `manabar.exe.old`.

Failure handling: every step rolls back to the pre-step state (delete
partial `.new`, rename `.old` back if the final rename fails) and
reports the error to the panel. The app keeps running on the old
version — no half-updated state.

macOS: the Update button opens the Releases page (opener plugin). A
DMG cannot be swapped in place gracefully; not attempted.

## Security & privacy (hard requirements)

- **Zero telemetry.** The update check is a single anonymous `GET` to
  GitHub's public API once per day. It carries no user identifier, no
  machine identifier, no usage data — nothing beyond what any HTTPS
  request exposes (client IP to GitHub). Nothing is ever POSTed.
- **Credential isolation.** The updater never touches provider
  credentials. It uses a bare HTTP client with no auth headers; the
  token-loading code paths are not reachable from the updater module.
- **Pinned source.** Only `https://api.github.com/repos/elvishasleft/
  manabar/releases/latest` is queried, and only asset URLs returned by
  that response are downloaded. No redirects to non-GitHub hosts are
  followed (custom reqwest redirect policy on both updater clients:
  https-only, hosts limited to github.com, api.github.com, and
  *.githubusercontent.com — GitHub asset downloads legitimately redirect
  to githubusercontent.com object hosts).
- **Integrity check.** Downloaded byte count must equal the asset
  `size` from the API response; mismatch → delete the partial file and
  abort. Nothing downloaded is ever executed without the user having
  clicked Update. When the API response carries a sha256 `digest` for
  the asset, the downloaded bytes' SHA-256 must additionally match it;
  size-only verification is the floor for releases predating the digest
  field. Assets larger than 100 MB are rejected at parse time as a
  sanity cap.
- **Injection safety.** `tag_name` and release URL are attacker-ish
  inputs (repo compromise scenario). They are rendered in the panel
  through the same escaping helpers covered by the existing CSP
  regression tests; the version string is additionally validated
  against `^v?[0-9]+(\.[0-9]+)*$` before use, and the notes URL must
  have prefix `https://github.com/elvishasleft/manabar/`.
- **Opener scope.** `tauri-plugin-opener` capability is restricted to
  opening URLs; only the validated release-page URL is ever passed.
- **User control.** `update_check: false` disables the network check
  entirely. No background installs — updating always requires a click.

## Accepted trade-offs

- No signature verification: the portable build has no signing-key
  infrastructure. Trust boundary is GitHub HTTPS + hardcoded repo +
  size check.
- No delta updates, no silent auto-install. The user must click.
- NSIS-installed copies live in the same `%LOCALAPPDATA%\ManaBar`
  directory and are swapped the same way; the uninstaller registry
  entry keeps the old version string until the next installer run.
  Accepted (cosmetic).

## Testing

- Unit: version comparison (equal / newer / older / malformed tags),
  release JSON parsing (synthetic fixture `github_release.json`),
  portable-asset selection (present / absent / multiple).
- The rename dance is factored into a pure "plan" function (input:
  paths + current state, output: ordered file operations) and unit
  tested; the thin IO wrapper is validated manually and by the next
  real release.
- Accepted deviation (v0.7.0): the swap sequence keeps its IO inline in
  `apply_inner` rather than the originally sketched pure "plan function"
  extraction; compensated by three review rounds on that path, the
  poison/reentrancy guards, and a manual first-release verification of
  the one-click path when v0.7.1 ships.
