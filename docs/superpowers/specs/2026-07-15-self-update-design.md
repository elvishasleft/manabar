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
