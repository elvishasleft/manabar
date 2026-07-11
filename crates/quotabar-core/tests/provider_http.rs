use quotabar_core::http::{client, get_json};
use quotabar_core::model::ProviderError;
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
    let (status, _) = get_json(&client(), &format!("{}/x", server.uri()), &[]).await.unwrap();
    assert_eq!(status, 401);
}

#[tokio::test]
async fn get_json_maps_connect_failure_to_network() {
    let err = get_json(&client(), "http://127.0.0.1:1/x", &[]).await.unwrap_err();
    assert!(matches!(err, ProviderError::Network(_)));
}

use quotabar_core::providers::claude::ClaudeProvider;
use std::fs;

fn fake_home_with_claude_creds(expires_in_mins: i64) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let creds_dir = dir.path().join(".claude");
    fs::create_dir_all(&creds_dir).unwrap();
    let expires_ms = (chrono::Utc::now() + chrono::Duration::minutes(expires_in_mins))
        .timestamp_millis();
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
        .respond_with(ResponseTemplate::new(200).set_body_string(include_str!("fixtures/claude_usage.json")))
        .mount(&server)
        .await;
    let home = fake_home_with_claude_creds(60);
    let p = ClaudeProvider { base_url: server.uri(), home: home.path().to_path_buf() };
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
    let p = ClaudeProvider { base_url: server.uri(), home: home.path().to_path_buf() };
    assert!(matches!(p.fetch_quota(&client()).await, Err(ProviderError::TokenExpired)));
}

#[tokio::test]
async fn claude_expired_file_token_skips_http() {
    let server = MockServer::start().await;
    // no mock mounted: a request would return 404 -> Network, so TokenExpired proves no call happened
    let home = fake_home_with_claude_creds(-5);
    let p = ClaudeProvider { base_url: server.uri(), home: home.path().to_path_buf() };
    assert!(matches!(p.fetch_quota(&client()).await, Err(ProviderError::TokenExpired)));
}

#[tokio::test]
async fn claude_malformed_body_is_schema_changed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let home = fake_home_with_claude_creds(60);
    let p = ClaudeProvider { base_url: server.uri(), home: home.path().to_path_buf() };
    assert!(matches!(p.fetch_quota(&client()).await, Err(ProviderError::SchemaChanged(_))));
}
