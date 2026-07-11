use crate::model::{DayUsage, UsageStats};
use crate::pricing::{cost_usd, PriceTable};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub mod claude;
pub mod codex;
pub mod grok;

#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
}

pub type ParseFn = fn(&str, DateTime<Utc>) -> Vec<UsageEvent>;

#[derive(Debug, Clone, Copy, Default)]
struct DayTotals {
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    cost_known: bool,
}

struct FileEntry {
    mtime_ms: i64,
    len: u64,
    days: HashMap<NaiveDate, DayTotals>,
}

#[derive(Default)]
pub struct LogCache {
    files: HashMap<PathBuf, FileEntry>,
}

fn fold_events(events: &[UsageEvent], prices: &PriceTable) -> HashMap<NaiveDate, DayTotals> {
    let mut days: HashMap<NaiveDate, DayTotals> = HashMap::new();
    for e in events {
        let d = days.entry(e.timestamp.date_naive()).or_default();
        d.input_tokens += e.input_tokens + e.cache_read_tokens + e.cache_write_tokens;
        d.output_tokens += e.output_tokens;
        if let Some(p) = prices.price_for(&e.model) {
            d.cost_usd += cost_usd(p, e.input_tokens, e.cache_read_tokens, e.cache_write_tokens, e.output_tokens);
            d.cost_known = true;
        }
    }
    days
}

pub fn aggregate_dir(
    cache: &mut LogCache,
    root: &Path,
    file_matches: fn(&Path) -> bool,
    parse: ParseFn,
    prices: &PriceTable,
    today: NaiveDate,
) -> UsageStats {
    let mut totals: HashMap<NaiveDate, DayTotals> = HashMap::new();
    if root.is_dir() {
        for entry in walkdir::WalkDir::new(root).into_iter().flatten() {
            let path = entry.path();
            if !entry.file_type().is_file() || !file_matches(path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime: DateTime<Utc> =
                meta.modified().map(DateTime::from).unwrap_or_else(|_| Utc::now());
            let (mtime_ms, len) = (mtime.timestamp_millis(), meta.len());
            let days = match cache.files.get(path) {
                Some(f) if f.mtime_ms == mtime_ms && f.len == len => f.days.clone(),
                _ => {
                    let text = std::fs::read_to_string(path).unwrap_or_default();
                    let days = fold_events(&parse(&text, mtime), prices);
                    cache
                        .files
                        .insert(path.to_path_buf(), FileEntry { mtime_ms, len, days: days.clone() });
                    days
                }
            };
            for (date, t) in days {
                let d = totals.entry(date).or_default();
                d.input_tokens += t.input_tokens;
                d.output_tokens += t.output_tokens;
                d.cost_usd += t.cost_usd;
                d.cost_known |= t.cost_known;
            }
        }
    }
    let days = (0..7)
        .map(|i| {
            let date = today - Duration::days(6 - i);
            let t = totals.get(&date).copied().unwrap_or_default();
            DayUsage {
                date,
                input_tokens: t.input_tokens,
                output_tokens: t.output_tokens,
                est_cost_usd: t.cost_known.then_some(t.cost_usd),
            }
        })
        .collect();
    UsageStats { days }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{Price, PriceTable};
    use chrono::{DateTime, NaiveDate, Utc};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // test log format: one event per line, "rfc3339|model|input|cache_read|cache_write|output"
    fn test_parse(text: &str, _mtime: DateTime<Utc>) -> Vec<UsageEvent> {
        text.lines()
            .filter_map(|l| {
                let p: Vec<&str> = l.split('|').collect();
                if p.len() != 6 {
                    return None;
                }
                Some(UsageEvent {
                    timestamp: DateTime::parse_from_rfc3339(p[0]).ok()?.with_timezone(&Utc),
                    model: p[1].to_string(),
                    input_tokens: p[2].parse().ok()?,
                    cache_read_tokens: p[3].parse().ok()?,
                    cache_write_tokens: p[4].parse().ok()?,
                    output_tokens: p[5].parse().ok()?,
                })
            })
            .collect()
    }

    static CACHE_TEST_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn counting_parse(text: &str, mtime: DateTime<Utc>) -> Vec<UsageEvent> {
        CACHE_TEST_CALLS.fetch_add(1, Ordering::SeqCst);
        test_parse(text, mtime)
    }

    fn any_log(p: &std::path::Path) -> bool {
        p.extension().map(|e| e == "log").unwrap_or(false)
    }

    fn priced() -> PriceTable {
        PriceTable {
            entries: vec![(
                "model-a".into(),
                Price { input: 3.0, output: 15.0, cache_read: 0.3, cache_write: 3.75 },
            )],
        }
    }

    #[test]
    fn aggregates_seven_zero_filled_days() {
        let dir = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 11).unwrap();
        fs::write(
            dir.path().join("a.log"),
            "2026-07-11T08:00:00Z|model-a|100|0|0|50\n2026-07-11T09:00:00Z|model-a|200|300|400|100\n2026-07-09T09:00:00Z|model-b|10|0|0|5\n2026-01-01T00:00:00Z|model-a|999|0|0|999\n",
        )
        .unwrap();
        let mut cache = LogCache::default();
        let stats = aggregate_dir(&mut cache, dir.path(), any_log, test_parse, &priced(), today);
        assert_eq!(stats.days.len(), 7);
        assert_eq!(stats.days[0].date, NaiveDate::from_ymd_opt(2026, 7, 5).unwrap());
        let d11 = &stats.days[6];
        assert_eq!(d11.date, today);
        assert_eq!(d11.input_tokens, 100 + 200 + 300 + 400);
        assert_eq!(d11.output_tokens, 150);
        assert!(d11.est_cost_usd.is_some());
        let d9 = &stats.days[4];
        assert_eq!(d9.input_tokens, 10);
        assert!(d9.est_cost_usd.is_none(), "model-b has no price");
        assert_eq!(stats.days[1].input_tokens, 0, "zero-filled day");
    }

    #[test]
    fn missing_root_gives_zero_days() {
        let mut cache = LogCache::default();
        let today = NaiveDate::from_ymd_opt(2026, 7, 11).unwrap();
        let stats = aggregate_dir(
            &mut cache,
            std::path::Path::new("Z:/definitely/missing"),
            any_log,
            test_parse,
            &priced(),
            today,
        );
        assert_eq!(stats.days.len(), 7);
        assert!(stats.days.iter().all(|d| d.input_tokens == 0 && d.output_tokens == 0));
    }

    #[test]
    fn unchanged_files_are_not_reparsed() {
        let dir = tempfile::tempdir().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 7, 11).unwrap();
        fs::write(dir.path().join("a.log"), "2026-07-11T08:00:00Z|model-a|1|0|0|1\n").unwrap();
        let mut cache = LogCache::default();
        let before = CACHE_TEST_CALLS.load(Ordering::SeqCst);
        aggregate_dir(&mut cache, dir.path(), any_log, counting_parse, &priced(), today);
        aggregate_dir(&mut cache, dir.path(), any_log, counting_parse, &priced(), today);
        assert_eq!(CACHE_TEST_CALLS.load(Ordering::SeqCst) - before, 1, "second pass must hit cache");
        fs::write(
            dir.path().join("a.log"),
            "2026-07-11T08:00:00Z|model-a|1|0|0|1\n2026-07-11T09:00:00Z|model-a|2|0|0|2\n",
        )
        .unwrap();
        let stats = aggregate_dir(&mut cache, dir.path(), any_log, counting_parse, &priced(), today);
        assert_eq!(CACHE_TEST_CALLS.load(Ordering::SeqCst) - before, 2);
        assert_eq!(stats.days[6].input_tokens, 3);
    }
}
