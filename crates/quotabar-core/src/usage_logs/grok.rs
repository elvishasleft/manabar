use super::UsageEvent;
use chrono::{DateTime, Utc};
use std::path::Path;

pub fn file_matches(p: &Path) -> bool {
    p.file_name().map(|n| n == "signals.json").unwrap_or(false)
}

pub fn parse_file(text: &str, mtime: DateTime<Utc>) -> Vec<UsageEvent> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return vec![];
    };
    let tokens = v.get("contextTokensUsed").and_then(|t| t.as_u64()).unwrap_or(0);
    if tokens == 0 {
        return vec![];
    }
    vec![UsageEvent {
        timestamp: mtime,
        model: v
            .get("primaryModelId")
            .and_then(|t| t.as_str())
            .unwrap_or("grok")
            .to_string(),
        input_tokens: tokens,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        output_tokens: 0,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn one_event_per_signals_file_stamped_with_mtime() {
        let mtime = Utc.with_ymd_and_hms(2026, 7, 10, 9, 0, 0).unwrap();
        let events = parse_file(
            r#"{"turnCount":3,"contextTokensUsed":66440,"primaryModelId":"grok-4.5"}"#,
            mtime,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].timestamp, mtime);
        assert_eq!(events[0].model, "grok-4.5");
        assert_eq!(events[0].input_tokens, 66440);
        assert_eq!(events[0].output_tokens, 0);
    }

    #[test]
    fn zero_tokens_or_garbage_yields_no_event() {
        let mtime = Utc::now();
        assert!(parse_file(r#"{"contextTokensUsed":0}"#, mtime).is_empty());
        assert!(parse_file("not json", mtime).is_empty());
    }

    #[test]
    fn file_matcher_wants_signals_json_only() {
        assert!(file_matches(std::path::Path::new("s/abc/signals.json")));
        assert!(!file_matches(std::path::Path::new("s/abc/other.json")));
    }
}
