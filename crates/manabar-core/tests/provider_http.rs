use manabar_core::http::{client, get_json};
use manabar_core::model::ProviderError;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn get_json_returns_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/x"))
        .and(header("authorization", "Bearer t"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .mount(&server)
        .await;
    let (status, body) = get_json(
        &client(),
        &format!("{}/x", server.uri()),
        &[("authorization", "Bearer t".to_string())],
    )
    .await
    .unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"ok":true}"#);
}

#[tokio::test]
async fn get_json_passes_through_error_statuses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let (status, _) = get_json(&client(), &format!("{}/x", server.uri()), &[])
        .await
        .unwrap();
    assert_eq!(status, 401);
}

#[tokio::test]
async fn get_json_maps_connect_failure_to_network() {
    let err = get_json(&client(), "http://127.0.0.1:1/x", &[])
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Network(_)));
}

use manabar_core::providers::claude::ClaudeProvider;
use std::fs;

fn fake_home_with_claude_creds(expires_in_mins: i64) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let creds_dir = dir.path().join(".claude");
    fs::create_dir_all(&creds_dir).unwrap();
    let expires_ms =
        (chrono::Utc::now() + chrono::Duration::minutes(expires_in_mins)).timestamp_millis();
    fs::write(
        creds_dir.join(".credentials.json"),
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"tok","expiresAt":{expires_ms},"subscriptionType":"max"}}}}"#
        ),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn claude_fetch_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(include_str!("fixtures/claude_usage.json")),
        )
        .mount(&server)
        .await;
    let home = fake_home_with_claude_creds(60);
    let p = ClaudeProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    let snap = p.fetch_quota(&client()).await.unwrap();
    assert_eq!(snap.windows.len(), 3);
}

#[tokio::test]
async fn claude_401_maps_to_token_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let home = fake_home_with_claude_creds(60);
    let p = ClaudeProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    assert!(matches!(
        p.fetch_quota(&client()).await,
        Err(ProviderError::TokenExpired)
    ));
}

#[tokio::test]
async fn claude_expired_file_token_skips_http() {
    let server = MockServer::start().await;
    // no mock mounted: a request would return 404 -> Network, so TokenExpired proves no call happened
    let home = fake_home_with_claude_creds(-5);
    let p = ClaudeProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    assert!(matches!(
        p.fetch_quota(&client()).await,
        Err(ProviderError::TokenExpired)
    ));
}

#[tokio::test]
async fn claude_malformed_body_is_schema_changed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let home = fake_home_with_claude_creds(60);
    let p = ClaudeProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    assert!(matches!(
        p.fetch_quota(&client()).await,
        Err(ProviderError::SchemaChanged(_))
    ));
}

use manabar_core::providers::codex::CodexProvider;

fn fake_home_with_codex_creds() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join(".codex");
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("auth.json"),
        r#"{"auth_mode":"chatgpt","tokens":{"access_token":"tok","account_id":"acct-1"}}"#,
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn codex_fetch_happy_path_sends_account_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/backend-api/wham/usage"))
        .and(header("chatgpt-account-id", "acct-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(include_str!("fixtures/codex_usage.json")),
        )
        .mount(&server)
        .await;
    let home = fake_home_with_codex_creds();
    let p = CodexProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    let snap = p.fetch_quota(&client()).await.unwrap();
    assert_eq!(snap.plan.as_deref(), Some("Free"));
}

#[tokio::test]
async fn codex_401_maps_to_token_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let home = fake_home_with_codex_creds();
    let p = CodexProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    assert!(matches!(
        p.fetch_quota(&client()).await,
        Err(ProviderError::TokenExpired)
    ));
}

use manabar_core::providers::grok::GrokProvider;

fn fake_home_with_grok_creds() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join(".grok");
    fs::create_dir_all(&d).unwrap();
    let expires = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    fs::write(
        d.join("auth.json"),
        format!(r#"{{"https://auth.x.ai::client-1":{{"key":"tok-g","expires_at":"{expires}"}}}}"#),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn grok_fetch_happy_path_with_plan_from_settings() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/settings"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"subscription_tier_display":"X Premium+"}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/billing"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(include_str!("fixtures/grok_billing.json")),
        )
        .mount(&server)
        .await;
    let home = fake_home_with_grok_creds();
    let p = GrokProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    let snap = p.fetch_quota(&client()).await.unwrap();
    assert_eq!(snap.plan.as_deref(), Some("X Premium+"));
    assert_eq!(snap.windows[0].used_percent, 4.0);
}

#[tokio::test]
async fn grok_settings_failure_does_not_fail_quota() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/settings"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/billing"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(include_str!("fixtures/grok_billing.json")),
        )
        .mount(&server)
        .await;
    let home = fake_home_with_grok_creds();
    let p = GrokProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    let snap = p.fetch_quota(&client()).await.unwrap();
    assert!(snap.plan.is_none());
    assert_eq!(snap.windows.len(), 1);
}

#[tokio::test]
async fn grok_401_maps_to_token_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let home = fake_home_with_grok_creds();
    let p = GrokProvider {
        base_url: server.uri(),
        home: home.path().to_path_buf(),
    };
    assert!(matches!(
        p.fetch_quota(&client()).await,
        Err(ProviderError::TokenExpired)
    ));
}

use manabar_core::providers::deepseek::DeepSeekProvider;

#[tokio::test]
async fn deepseek_fetch_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user/balance"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("fixtures/deepseek_balance.json")),
        )
        .mount(&server)
        .await;
    let p = DeepSeekProvider {
        base_url: server.uri(),
        key_override: Some("test-key".into()),
        budget: None,
    };
    let snap = p.fetch_quota(&client()).await.unwrap();
    assert_eq!(snap.windows.len(), 1);
    assert_eq!(snap.windows[0].label, "Balance");
    assert_eq!(snap.windows[0].used_percent, 0.0);
    assert_eq!(snap.plan.as_deref(), Some("¥42.50"));
}

#[tokio::test]
async fn deepseek_401_maps_to_token_expired() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let p = DeepSeekProvider {
        base_url: server.uri(),
        key_override: Some("test-key".into()),
        budget: None,
    };
    assert!(matches!(
        p.fetch_quota(&client()).await,
        Err(ProviderError::TokenExpired)
    ));
}
