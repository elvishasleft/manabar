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
