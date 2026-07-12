use quotabar_core::model::ProviderView;
use std::time::{Duration, Instant};

/// Windows `CREATE_NO_WINDOW` flag: suppresses the console window a spawned
/// `cmd.exe` would otherwise flash open.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Timeout for the sign-in refresh CLI spawn. The CLIs themselves complete
/// in a few seconds when healthy; 120s gives generous headroom before we
/// kill the child and surface a failure.
const SIGNIN_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the blocking waiter polls the sign-in CLI for exit.
const SIGNIN_POLL: Duration = Duration::from_millis(250);

#[tauri::command]
pub async fn get_state(
    shared: tauri::State<'_, crate::AppShared>,
) -> Result<Vec<ProviderView>, String> {
    Ok(shared.views().await)
}

#[tauri::command]
pub async fn refresh_now(shared: tauri::State<'_, crate::AppShared>) -> Result<(), String> {
    shared.request_refresh();
    Ok(())
}

#[tauri::command]
pub async fn panel_opened(
    shared: tauri::State<'_, crate::AppShared>,
) -> Result<Vec<ProviderView>, String> {
    shared.refresh_usage().await;
    shared.request_refresh();
    Ok(shared.views().await)
}

/// Fixed argv for the one-click sign-in refresh, per provider `kind`.
///
/// The CLI is invoked with a trivial prompt — exactly what a user would type
/// by hand to remediate an expired token — and nothing else. No user input
/// ever reaches this argv: `kind` is matched against a closed set of known
/// provider identifiers, so there is no injection surface. Returns `None`
/// for providers without a local refresh CLI (e.g. `codex`) or unknown
/// input.
pub(crate) fn signin_argv(kind: &str) -> Option<Vec<String>> {
    let tail: &[&str] = match kind {
        "claude" => &["claude", "-p", "hi", "--max-turns", "1"],
        "grok" => &["grok", "-p", "hi"],
        _ => return None,
    };
    let mut argv = vec!["cmd".to_string(), "/C".to_string()];
    argv.extend(tail.iter().map(|s| s.to_string()));
    Some(argv)
}

/// Result of running the sign-in CLI to completion (or giving up).
#[derive(Debug)]
enum SigninOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

/// Kills the spawned sign-in CLI and everything under it. `cmd /C` wraps the
/// real CLI in a child process of its own, so on Windows `taskkill /T` takes
/// the whole tree down rather than leaving an orphaned `claude`/`grok`
/// process behind; `Child::kill` + `wait` is the portable fallback and also
/// reaps the direct child.
fn kill_signin_cli(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs `program args` to completion off the async runtime, applying
/// `CREATE_NO_WINDOW` on Windows so no console flashes open. The wait is a
/// `try_wait` poll loop with a hard deadline: on timeout the child is
/// actively killed (not orphaned) before reporting `TimedOut`, so the
/// blocking thread is always released within ~`SIGNIN_TIMEOUT`. All stdio is
/// null — the CLIs' print modes need no input and their output is unused.
fn run_signin_cli(program: &str, args: &[String]) -> std::io::Result<SigninOutcome> {
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + SIGNIN_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(SigninOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            kill_signin_cli(&mut child);
            return Ok(SigninOutcome::TimedOut);
        }
        std::thread::sleep(SIGNIN_POLL);
    }
}

/// Spawns the provider's own sign-in CLI once with a fixed, trivial prompt —
/// exactly the manual remediation a user would perform — then triggers a
/// quota re-poll on success. QuotaBar never touches credential files
/// directly; the CLI does its own token refresh as a side effect of running.
///
/// The blocking `std::process::Command` call runs on a `spawn_blocking`
/// thread rather than `tokio::process::Command` so the async runtime doesn't
/// need Tokio's `process` feature. The closure enforces its own deadline and
/// kills the child on timeout, so no orphaned process or stuck thread
/// outlives a failed refresh.
#[tauri::command]
pub async fn refresh_signin(
    kind: String,
    shared: tauri::State<'_, crate::AppShared>,
) -> Result<(), String> {
    let argv = signin_argv(&kind).ok_or_else(|| format!("unsupported provider: {kind}"))?;
    let (program, args) = argv.split_first().expect("argv always has a program");
    let program = program.clone();
    let args = args.to_vec();

    let result = tokio::task::spawn_blocking(move || run_signin_cli(&program, &args))
        .await
        .map_err(|e| format!("CLI task failed: {e}"))?;

    match result {
        Ok(SigninOutcome::Exited(status)) if status.success() => {
            shared.request_refresh();
            Ok(())
        }
        Ok(SigninOutcome::Exited(status)) => {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown code".to_string());
            log::warn!("{kind} sign-in CLI exited with {code}");
            Err(format!("CLI exited with {code}"))
        }
        Ok(SigninOutcome::TimedOut) => {
            let secs = SIGNIN_TIMEOUT.as_secs();
            log::warn!("{kind} sign-in CLI timed out after {secs}s and was killed");
            Err(format!("timed out after {secs}s"))
        }
        Err(e) => {
            log::warn!("{kind} sign-in CLI failed to launch: {e}");
            Err(format!("failed to launch CLI: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_argv_is_fixed_and_trivial() {
        assert_eq!(
            signin_argv("claude"),
            Some(vec![
                "cmd".to_string(),
                "/C".to_string(),
                "claude".to_string(),
                "-p".to_string(),
                "hi".to_string(),
                "--max-turns".to_string(),
                "1".to_string(),
            ])
        );
    }

    #[test]
    fn grok_argv_is_fixed_and_trivial() {
        assert_eq!(
            signin_argv("grok"),
            Some(vec![
                "cmd".to_string(),
                "/C".to_string(),
                "grok".to_string(),
                "-p".to_string(),
                "hi".to_string(),
            ])
        );
    }

    #[test]
    fn codex_has_no_refresh_cli() {
        assert_eq!(signin_argv("codex"), None);
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert_eq!(signin_argv("garbage"), None);
        assert_eq!(signin_argv(""), None);
        assert_eq!(signin_argv("Claude"), None);
    }

    #[cfg(windows)]
    #[test]
    fn run_signin_cli_reports_success_exit() {
        let args: Vec<String> = ["/C", "exit", "0"].iter().map(|s| s.to_string()).collect();
        match run_signin_cli("cmd", &args).unwrap() {
            SigninOutcome::Exited(status) => assert!(status.success()),
            other => panic!("expected clean exit, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn run_signin_cli_reports_failure_exit_code() {
        let args: Vec<String> = ["/C", "exit", "3"].iter().map(|s| s.to_string()).collect();
        match run_signin_cli("cmd", &args).unwrap() {
            SigninOutcome::Exited(status) => {
                assert!(!status.success());
                assert_eq!(status.code(), Some(3));
            }
            other => panic!("expected failing exit, got {other:?}"),
        }
    }

    #[test]
    fn run_signin_cli_missing_program_is_io_error() {
        assert!(run_signin_cli("definitely-not-a-real-program-xyz", &[]).is_err());
    }
}
