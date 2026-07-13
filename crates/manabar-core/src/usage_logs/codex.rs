use super::UsageEvent;
use chrono::{DateTime, Utc};
use std::path::Path;

pub fn file_matches(p: &Path) -> bool {
    p.extension().map(|e| e == "jsonl").unwrap_or(false)
}

pub fn parse_file(text: &str, _mtime: DateTime<Utc>) -> Vec<UsageEvent> {
    let mut current_model = "unknown".to_string();
    let mut events = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let payload = &v["payload"];
        match payload.get("type").and_then(|t| t.as_str()) {
            Some("turn_context") => {
                if let Some(m) = payload.get("model").and_then(|t| t.as_str()) {
                    current_model = m.to_string();
                }
            }
            Some("token_count") => {
                let Some(ts) = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.with_timezone(&Utc))
                else {
                    continue;
                };
                let usage = &payload["info"]["last_token_usage"];
                if usage.is_null() {
                    continue;
                }
                let g = |k: &str| usage.get(k).and_then(|t| t.as_u64()).unwrap_or(0);
                let (input, cached) = (g("input_tokens"), g("cached_input_tokens"));
                events.push(UsageEvent {
                    timestamp: ts,
                    model: current_model.clone(),
                    input_tokens: input.saturating_sub(cached),
                    cache_read_tokens: cached,
                    cache_write_tokens: 0,
                    output_tokens: g("output_tokens"),
                });
            }
            _ => {}
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const LINES: &str = concat!(
        r#"{"timestamp":"2026-07-11T02:24:31.000Z","type":"turn_context","payload":{"type":"turn_context","model":"gpt-5.6-sol"}}"#,
        "\n",
        r#"{"timestamp":"2026-07-11T02:25:12.681Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":25161,"cached_input_tokens":10496,"output_tokens":12,"reasoning_output_tokens":0,"total_tokens":25173}}}}"#,
        "\n",
        r#"{"timestamp":"2026-07-11T02:26:00.000Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
        "\n",
        r#"{"timestamp":"2026-07-11T02:27:00.000Z","type":"event_msg","payload":{"type":"agent_message"}}"#,
        "\n",
    );

    #[test]
    fn parses_token_counts_with_model_from_turn_context() {
        let events = parse_file(LINES, Utc::now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].model, "gpt-5.6-sol");
        assert_eq!(events[0].input_tokens, 25161 - 10496);
        assert_eq!(events[0].cache_read_tokens, 10496);
        assert_eq!(events[0].output_tokens, 12);
    }

    #[test]
    fn token_count_before_any_turn_context_uses_unknown() {
        let lines = r#"{"timestamp":"2026-07-11T02:25:12.681Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":5}}}}"#;
        let events = parse_file(lines, Utc::now());
        assert_eq!(events[0].model, "unknown");
    }
}
