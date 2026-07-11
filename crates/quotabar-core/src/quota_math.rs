use crate::model::{Health, QuotaSnapshot};
use chrono::{DateTime, Utc};

pub fn health_for(remaining_percent: f64) -> Health {
    if remaining_percent > 30.0 {
        Health::Green
    } else if remaining_percent > 10.0 {
        Health::Amber
    } else {
        Health::Red
    }
}

impl QuotaSnapshot {
    pub fn binding_remaining_percent(&self) -> Option<f64> {
        self.windows
            .iter()
            .map(|w| 100.0 - w.used_percent)
            .fold(None, |acc, r| match acc {
                Some(a) if a <= r => Some(a),
                _ => Some(r),
            })
    }
}

pub fn format_countdown(now: DateTime<Utc>, until: DateTime<Utc>) -> String {
    let secs = (until - now).num_seconds();
    if secs <= 0 {
        return "now".into();
    }
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3_600, (secs % 3_600) / 60);
    match (d, h, m) {
        (0, 0, 0) => "under 1m".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

pub fn window_label_from_seconds(secs: i64) -> String {
    if secs == 604_800 {
        "Weekly".into()
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn health_boundaries() {
        assert_eq!(health_for(100.0), Health::Green);
        assert_eq!(health_for(30.1), Health::Green);
        assert_eq!(health_for(30.0), Health::Amber);
        assert_eq!(health_for(10.1), Health::Amber);
        assert_eq!(health_for(10.0), Health::Red);
        assert_eq!(health_for(0.0), Health::Red);
    }

    #[test]
    fn binding_is_min_remaining() {
        let snap = QuotaSnapshot {
            plan: None,
            windows: vec![
                RateWindow {
                    label: "5h".into(),
                    used_percent: 20.0,
                    resets_at: None,
                },
                RateWindow {
                    label: "Weekly".into(),
                    used_percent: 65.0,
                    resets_at: None,
                },
            ],
            fetched_at: Utc::now(),
        };
        assert_eq!(snap.binding_remaining_percent(), Some(35.0));
    }

    #[test]
    fn binding_none_when_no_windows() {
        let snap = QuotaSnapshot {
            plan: None,
            windows: vec![],
            fetched_at: Utc::now(),
        };
        assert_eq!(snap.binding_remaining_percent(), None);
    }

    #[test]
    fn countdown_formats() {
        let now = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
        assert_eq!(
            format_countdown(now, now + chrono::Duration::minutes(134)),
            "2h 14m"
        );
        assert_eq!(
            format_countdown(now, now + chrono::Duration::hours(76)),
            "3d 4h"
        );
        assert_eq!(
            format_countdown(now, now + chrono::Duration::seconds(59)),
            "under 1m"
        );
        assert_eq!(
            format_countdown(now, now - chrono::Duration::seconds(5)),
            "now"
        );
        assert_eq!(
            format_countdown(now, now + chrono::Duration::minutes(9)),
            "9m"
        );
    }

    #[test]
    fn window_labels() {
        assert_eq!(window_label_from_seconds(18_000), "5h");
        assert_eq!(window_label_from_seconds(604_800), "Weekly");
        assert_eq!(window_label_from_seconds(2_592_000), "30d");
        assert_eq!(window_label_from_seconds(3_600), "1h");
    }
}
