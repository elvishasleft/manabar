use crate::model::ProviderError;
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Token {
    pub bearer: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub plan_hint: Option<String>,
    pub account_id: Option<String>,
}

fn read_json(path: &Path) -> Result<serde_json::Value, ProviderError> {
    if !path.exists() {
        return Err(ProviderError::NoCredentials);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| ProviderError::SchemaChanged(format!("read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| ProviderError::SchemaChanged(format!("parse {}: {e}", path.display())))
}

pub fn claude_token(home: &Path) -> Result<Token, ProviderError> {
    let v = read_json(&home.join(".claude").join(".credentials.json"))?;
    let oauth = v
        .get("claudeAiOauth")
        .ok_or_else(|| ProviderError::SchemaChanged("missing claudeAiOauth".into()))?;
    let bearer = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ProviderError::SchemaChanged("missing accessToken".into()))?
        .to_string();
    let expires_at = oauth
        .get("expiresAt")
        .and_then(|t| t.as_i64())
        .and_then(|ms| Utc.timestamp_millis_opt(ms).single());
    let plan_hint = oauth
        .get("subscriptionType")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    Ok(Token {
        bearer,
        expires_at,
        plan_hint,
        account_id: None,
    })
}

fn codex_auth_path(home: &Path, codex_home_env: Option<&str>) -> std::path::PathBuf {
    match codex_home_env {
        Some(dir) if !dir.trim().is_empty() => std::path::PathBuf::from(dir).join("auth.json"),
        _ => home.join(".codex").join("auth.json"),
    }
}

pub fn codex_token(home: &Path) -> Result<Token, ProviderError> {
    let path = codex_auth_path(home, std::env::var("CODEX_HOME").ok().as_deref());
    codex_token_at(&path)
}

fn codex_token_at(path: &Path) -> Result<Token, ProviderError> {
    let v = read_json(path)?;
    let tokens = v
        .get("tokens")
        .ok_or_else(|| ProviderError::SchemaChanged("missing tokens".into()))?;
    let bearer = tokens
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ProviderError::SchemaChanged("missing access_token".into()))?
        .to_string();
    let account_id = tokens
        .get("account_id")
        .and_then(|t| t.as_str())
        .map(str::to_string);
    Ok(Token {
        bearer,
        expires_at: None,
        plan_hint: None,
        account_id,
    })
}

pub fn grok_token(home: &Path) -> Result<Token, ProviderError> {
    let v = read_json(&home.join(".grok").join("auth.json"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| ProviderError::SchemaChanged("auth.json not an object".into()))?;
    let entry = obj
        .iter()
        .find(|(k, _)| k.starts_with("https://auth.x.ai::"))
        .map(|(_, e)| e)
        .ok_or_else(|| ProviderError::SchemaChanged("no auth.x.ai entry".into()))?;
    let bearer = entry
        .get("key")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ProviderError::SchemaChanged("missing key".into()))?
        .to_string();
    let expires_at = entry
        .get("expires_at")
        .and_then(|t| t.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    Ok(Token {
        bearer,
        expires_at,
        plan_hint: None,
        account_id: None,
    })
}

/// Pure resolution rule for the DeepSeek API key: env wins when non-blank,
/// otherwise fall back to the config-supplied key, otherwise `None`. Kept
/// free of `std::env::var` so it's hermetically unit-testable — same
/// pattern as `codex_auth_path`.
pub fn resolve_deepseek_key(env: Option<&str>, cfg: Option<&str>) -> Option<String> {
    if let Some(e) = env {
        if !e.trim().is_empty() {
            return Some(e.to_string());
        }
    }
    cfg.filter(|c| !c.trim().is_empty()).map(str::to_string)
}

/// Resolves the DeepSeek API key from `DEEPSEEK_API_KEY` (preferred) or the
/// config's `deepseek_api_key` fallback. Missing/blank in both places is
/// `NoCredentials`, consistent with the other providers' missing-auth state.
pub fn deepseek_key(config_key: Option<&str>) -> Result<String, ProviderError> {
    let env = std::env::var("DEEPSEEK_API_KEY").ok();
    resolve_deepseek_key(env.as_deref(), config_key).ok_or(ProviderError::NoCredentials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn home_with(rel: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
        dir
    }

    #[test]
    fn claude_missing_file_is_no_credentials() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            claude_token(dir.path()),
            Err(ProviderError::NoCredentials)
        ));
    }

    #[test]
    fn claude_parses_token_expiry_and_plan() {
        let dir = home_with(
            ".claude/.credentials.json",
            r#"{"claudeAiOauth":{"accessToken":"tok-abc","expiresAt":1783753082456,"subscriptionType":"max"}}"#,
        );
        let t = claude_token(dir.path()).unwrap();
        assert_eq!(t.bearer, "tok-abc");
        assert_eq!(t.plan_hint.as_deref(), Some("max"));
        assert_eq!(t.expires_at.unwrap().timestamp_millis(), 1783753082456);
    }

    #[test]
    fn claude_garbage_is_schema_changed() {
        let dir = home_with(".claude/.credentials.json", "not json");
        assert!(matches!(
            claude_token(dir.path()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }

    #[test]
    fn codex_auth_path_defaults_to_home_dotcodex() {
        let p = codex_auth_path(std::path::Path::new("C:/Users/u"), None);
        assert!(p.ends_with(std::path::Path::new(".codex/auth.json")));
    }

    #[test]
    fn codex_auth_path_honors_codex_home() {
        let p = codex_auth_path(std::path::Path::new("C:/Users/u"), Some("D:/codex-home"));
        assert_eq!(p, std::path::Path::new("D:/codex-home").join("auth.json"));
        let blank = codex_auth_path(std::path::Path::new("C:/Users/u"), Some("  "));
        assert!(blank.ends_with(std::path::Path::new(".codex/auth.json")));
    }

    #[test]
    fn codex_parses_token_and_account() {
        let dir = home_with(
            ".codex/auth.json",
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"tok-x","account_id":"acct-1"}}"#,
        );
        let t = codex_token_at(&dir.path().join(".codex").join("auth.json")).unwrap();
        assert_eq!(t.bearer, "tok-x");
        assert_eq!(t.account_id.as_deref(), Some("acct-1"));
        assert!(t.expires_at.is_none());
    }

    #[test]
    fn grok_picks_xai_entry_and_reads_key_field() {
        let dir = home_with(
            ".grok/auth.json",
            r#"{"https://other::1":{"key":"wrong"},"https://auth.x.ai::b1a0":{"key":"tok-g","expires_at":"2026-07-18T12:00:00Z"}}"#,
        );
        let t = grok_token(dir.path()).unwrap();
        assert_eq!(t.bearer, "tok-g");
        assert!(t.expires_at.is_some());
    }

    #[test]
    fn grok_no_xai_entry_is_schema_changed() {
        let dir = home_with(".grok/auth.json", r#"{"https://other::1":{"key":"k"}}"#);
        assert!(matches!(
            grok_token(dir.path()),
            Err(ProviderError::SchemaChanged(_))
        ));
    }

    #[test]
    fn deepseek_key_prefers_env_over_config() {
        assert_eq!(
            resolve_deepseek_key(Some("env-key"), Some("cfg-key")),
            Some("env-key".to_string())
        );
    }

    #[test]
    fn deepseek_key_blank_env_falls_back_to_config() {
        assert_eq!(
            resolve_deepseek_key(Some(""), Some("cfg-key")),
            Some("cfg-key".to_string())
        );
        assert_eq!(
            resolve_deepseek_key(Some("   "), Some("cfg-key")),
            Some("cfg-key".to_string())
        );
        assert_eq!(
            resolve_deepseek_key(None, Some("cfg-key")),
            Some("cfg-key".to_string())
        );
    }

    #[test]
    fn deepseek_key_both_missing_is_none() {
        assert_eq!(resolve_deepseek_key(None, None), None);
        assert_eq!(resolve_deepseek_key(Some(""), Some("")), None);
        assert_eq!(resolve_deepseek_key(Some(""), None), None);
    }
}
