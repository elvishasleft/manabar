use super::UsageEvent;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::Path;

pub fn file_matches(p: &Path) -> bool {
    p.extension().map(|e| e == "jsonl").unwrap_or(false)
}

pub fn parse_file(text: &str, _mtime: DateTime<Utc>) -> Vec<UsageEvent> {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut events = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(ts) = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
        else {
            continue;
        };
        let msg = &v["message"];
        let Some(usage) = msg.get("usage") else {
            continue;
        };
        let msg_id = msg
            .get("id")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let req_id = v
            .get("requestId")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if !msg_id.is_empty() && !seen.insert((msg_id, req_id)) {
            continue;
        }
        let g = |k: &str| usage.get(k).and_then(|t| t.as_u64()).unwrap_or(0);
        events.push(UsageEvent {
            timestamp: ts,
            model: msg
                .get("model")
                .and_then(|t| t.as_str())
                .unwrap_or("claude")
                .to_string(),
            input_tokens: g("input_tokens"),
            cache_read_tokens: g("cache_read_input_tokens"),
            cache_write_tokens: g("cache_creation_input_tokens"),
            output_tokens: g("output_tokens"),
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const LINES: &str = concat!(
        r#"{"type":"user","timestamp":"2026-07-11T08:00:00.000Z","message":{"role":"user"}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-07-11T08:30:19.132Z","requestId":"req_1","message":{"id":"msg_1","model":"claude-fable-5","usage":{"input_tokens":2,"cache_creation_input_tokens":5278,"cache_read_input_tokens":192046,"output_tokens":3243}}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-07-11T08:30:19.132Z","requestId":"req_1","message":{"id":"msg_1","model":"claude-fable-5","usage":{"input_tokens":2,"cache_creation_input_tokens":5278,"cache_read_input_tokens":192046,"output_tokens":3243}}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-07-10T10:00:00.000Z","requestId":"req_2","message":{"id":"msg_2","model":"claude-sonnet-5","usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":20}}}"#,
        "\n",
        "not json at all\n",
    );

    #[test]
    fn parses_assistant_lines_and_dedups() {
        let events = parse_file(LINES, Utc::now());
        assert_eq!(
            events.len(),
            2,
            "dup (msg_1, req_1) must collapse; user + garbage skipped"
        );
        assert_eq!(events[0].model, "claude-fable-5");
        assert_eq!(events[0].input_tokens, 2);
        assert_eq!(events[0].cache_write_tokens, 5278);
        assert_eq!(events[0].cache_read_tokens, 192046);
        assert_eq!(events[0].output_tokens, 3243);
        assert_eq!(events[1].model, "claude-sonnet-5");
    }

    #[test]
    fn file_matcher_wants_jsonl() {
        assert!(file_matches(std::path::Path::new("a/b/session.jsonl")));
        assert!(!file_matches(std::path::Path::new("a/b/notes.txt")));
    }
}
