use crate::model::ProviderError;
use std::time::Duration;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client build cannot fail with static config")
}

pub async fn get_json(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
) -> Result<(u16, String), ProviderError> {
    let mut last_err = None;
    for attempt in 0..2 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let mut req = client.get(url);
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp
                    .text()
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()))?;
                return Ok((status, body));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(ProviderError::Network(last_err.map(|e| e.to_string()).unwrap_or_default()))
}
