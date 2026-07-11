use quotabar_core::http::client;
use quotabar_core::providers::{claude::ClaudeProvider, codex::CodexProvider, grok::GrokProvider};
use std::path::PathBuf;

fn real_home() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").expect("USERPROFILE not set"))
}

#[tokio::test]
#[ignore = "hits real endpoints with local credentials; run manually"]
async fn live_claude() {
    let p = ClaudeProvider::new(real_home());
    let snap = p.fetch_quota(&client()).await.expect(
        "claude live fetch failed - if TokenExpired, open Claude Code once to refresh sign-in, then re-run",
    );
    println!("claude: {snap:#?}");
    assert!(!snap.windows.is_empty());
}

#[tokio::test]
#[ignore = "hits real endpoints with local credentials; run manually"]
async fn live_codex() {
    let p = CodexProvider::new(real_home());
    let snap = p
        .fetch_quota(&client())
        .await
        .expect("codex live fetch failed");
    println!("codex: {snap:#?}");
    assert!(!snap.windows.is_empty());
}

#[tokio::test]
#[ignore = "hits real endpoints with local credentials; run manually"]
async fn live_grok() {
    let p = GrokProvider::new(real_home());
    let snap = p
        .fetch_quota(&client())
        .await
        .expect("grok live fetch failed");
    println!("grok: {snap:#?}");
    assert!(!snap.windows.is_empty());
}
