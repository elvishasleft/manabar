use crate::model::{Health, QuotaSnapshot, RateWindow};
use chrono::{DateTime, Duration, Utc};

/// Minimum elapsed time between two samples for a burn rate to be considered
/// meaningful — below this, clock skew or duplicate polls dominate the
/// signal.
const MIN_DELTA_SECS: i64 = 60;
/// Maximum elapsed time between two samples for a burn rate to still be
/// treated as "current" — beyond this the usage pattern has likely reset or
/// drifted too far to extrapolate from.
const MAX_DELTA_SECS: i64 = 48 * 3_600;
/// Minimum burn rate (percent used per minute) required to project an ETA.
/// At or below this, the projected exhaustion time would be so far out (or
/// negative/undefined for a non-positive rate) that it isn't actionable.
const MIN_RATE_PERCENT_PER_MIN: f64 = 0.01;

pub fn health_for(remaining_percent: f64) -> Health {
    if remaining_percent > 30.0 {
        Health::Green
    } else if remaining_percent > 10.0 {
        Health::Amber
    } else {
        Health::Red
    }
}

/// Projects when a quota window will hit 100% used, from a linear fit
/// between two consecutive samples.
///
/// Returns `None` when the samples don't support a meaningful projection:
/// the elapsed time is too short (< 60s, likely clock noise) or too long
/// (> 48h, the trend is stale), the fitted rate is at or below
/// `0.01`%/min (flat, negative, or barely-moving usage never reaches 100%
/// in any useful timeframe), or the window is already at/over 100% used.
pub fn exhaust_eta(
    prev_t: DateTime<Utc>,
    prev_used: f64,
    curr_t: DateTime<Utc>,
    curr_used: f64,
) -> Option<DateTime<Utc>> {
    if curr_used >= 100.0 {
        return None;
    }
    let delta_secs = (curr_t - prev_t).num_seconds();
    if !(MIN_DELTA_SECS..=MAX_DELTA_SECS).contains(&delta_secs) {
        return None;
    }
    let delta_minutes = delta_secs as f64 / 60.0;
    let rate = (curr_used - prev_used) / delta_minutes;
    if rate <= MIN_RATE_PERCENT_PER_MIN {
        return None;
    }
    let minutes_left = (100.0 - curr_used) / rate;
    Some(curr_t + Duration::milliseconds((minutes_left * 60_000.0).round() as i64))
}

impl QuotaSnapshot {
    /// The window with the least remaining quota (the constraint that
    /// determines this provider's overall health/remaining-percent). When
    /// multiple windows tie for the minimum, the first one in `windows` wins
    /// — matching `Iterator::min_by`'s documented tie-break behavior.
    pub fn binding_window(&self) -> Option<&RateWindow> {
        self.windows.iter().min_by(|a, b| {
            let remaining_a = 100.0 - a.used_percent;
            let remaining_b = 100.0 - b.used_percent;
            remaining_a
                .partial_cmp(&remaining_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    pub fn binding_remaining_percent(&self) -> Option<f64> {
        self.binding_window().map(|w| 100.0 - w.used_percent)
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
                    exhaust_eta: None,
                },
                RateWindow {
                    label: "Weekly".into(),
                    used_percent: 65.0,
                    resets_at: None,
                    exhaust_eta: None,
                },
            ],
            fetched_at: Utc::now(),
        };
        assert_eq!(snap.binding_remaining_percent(), Some(35.0));
    }

    #[test]
    fn binding_window_returns_the_min_remaining_window() {
        let snap = QuotaSnapshot {
            plan: None,
            windows: vec![
                RateWindow {
                    label: "5h".into(),
                    used_percent: 20.0,
                    resets_at: None,
                    exhaust_eta: None,
                },
                RateWindow {
                    label: "Weekly".into(),
                    used_percent: 65.0,
                    resets_at: None,
                    exhaust_eta: None,
                },
            ],
            fetched_at: Utc::now(),
        };
        assert_eq!(
            snap.binding_window().map(|w| w.label.as_str()),
            Some("Weekly")
        );
    }

    #[test]
    fn binding_window_is_none_when_no_windows() {
        let snap = QuotaSnapshot {
            plan: None,
            windows: vec![],
            fetched_at: Utc::now(),
        };
        assert!(snap.binding_window().is_none());
    }

    #[test]
    fn binding_window_keeps_first_on_tie() {
        let snap = QuotaSnapshot {
            plan: None,
            windows: vec![
                RateWindow {
                    label: "first".into(),
                    used_percent: 50.0,
                    resets_at: None,
                    exhaust_eta: None,
                },
                RateWindow {
                    label: "second".into(),
                    used_percent: 50.0,
                    resets_at: None,
                    exhaust_eta: None,
                },
            ],
            fetched_at: Utc::now(),
        };
        assert_eq!(
            snap.binding_window().map(|w| w.label.as_str()),
            Some("first")
        );
    }

    #[test]
    fn exhaust_eta_happy_path_projects_linear_rate() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::minutes(30);
        // rate = (50 - 40) / 30 = 1/3 %/min -> (100 - 50) / (1/3) = 150 min
        let eta = exhaust_eta(prev_t, 40.0, curr_t, 50.0).unwrap();
        assert_eq!(eta, curr_t + chrono::Duration::minutes(150));
    }

    #[test]
    fn exhaust_eta_none_when_delta_too_short() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::seconds(59);
        assert!(exhaust_eta(prev_t, 40.0, curr_t, 50.0).is_none());
    }

    #[test]
    fn exhaust_eta_none_when_delta_too_long() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::hours(48) + chrono::Duration::seconds(1);
        assert!(exhaust_eta(prev_t, 40.0, curr_t, 50.0).is_none());
    }

    #[test]
    fn exhaust_eta_at_max_delta_boundary_is_accepted() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::hours(48);
        // rate = (50 - 10) / 2880 min = ~0.0139 %/min, comfortably above the
        // 0.01 threshold, isolating this test to the Δt boundary itself.
        assert!(exhaust_eta(prev_t, 10.0, curr_t, 50.0).is_some());
    }

    #[test]
    fn exhaust_eta_none_when_rate_is_negative() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::minutes(30);
        assert!(exhaust_eta(prev_t, 50.0, curr_t, 40.0).is_none());
    }

    #[test]
    fn exhaust_eta_none_when_rate_is_zero() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::minutes(30);
        assert!(exhaust_eta(prev_t, 40.0, curr_t, 40.0).is_none());
    }

    #[test]
    fn exhaust_eta_none_when_rate_at_or_below_threshold() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::minutes(100);
        // rate = 1.0 / 100 = 0.01 %/min exactly -> must be strictly > threshold
        assert!(exhaust_eta(prev_t, 40.0, curr_t, 41.0).is_none());
    }

    #[test]
    fn exhaust_eta_none_when_already_at_100() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::minutes(30);
        assert!(exhaust_eta(prev_t, 90.0, curr_t, 100.0).is_none());
    }

    #[test]
    fn exhaust_eta_none_when_over_100() {
        let prev_t = Utc.with_ymd_and_hms(2026, 7, 12, 10, 0, 0).unwrap();
        let curr_t = prev_t + chrono::Duration::minutes(30);
        assert!(exhaust_eta(prev_t, 90.0, curr_t, 103.0).is_none());
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
