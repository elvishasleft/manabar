use quotabar_core::Health;

pub const ICON_SIZE: u32 = 32;

pub const GREEN: [u8; 4] = [151, 196, 89, 255];
pub const AMBER: [u8; 4] = [239, 159, 39, 255];
pub const RED: [u8; 4] = [226, 75, 74, 255];
pub const GRAY: [u8; 4] = [110, 110, 106, 255];
pub const TRACK: [u8; 4] = [68, 68, 65, 140];

/// Fixed bar geometry: 6px-wide bars with 2px gaps between them, so N bars
/// span `8*N - 2` px, centered horizontally in the 32px-wide icon. With the
/// pre-v0.3 fixed count of 4 this reduces to the original hard-coded
/// layout (margin 1, bars at x ∈ {[1,7), [9,15), [17,23), [25,31)}).
const BAR_WIDTH: usize = 6;
const BAR_GAP: usize = 2;
const TRACK_TOP: usize = 2;
const TRACK_BOTTOM: usize = 30; // exclusive
const TRACK_HEIGHT: usize = TRACK_BOTTOM - TRACK_TOP;

fn put(buf: &mut [u8], x: usize, y: usize, c: [u8; 4]) {
    let i = (y * 32 + x) * 4;
    buf[i..i + 4].copy_from_slice(&c);
}

fn fill_color(health: Health) -> [u8; 4] {
    match health {
        Health::Green => GREEN,
        Health::Amber => AMBER,
        Health::Red => RED,
        Health::Unavailable => GRAY,
    }
}

/// Renders the tray icon from one `(remaining_percent, health)` pair per
/// *enabled* provider, in display order. The bar count varies with how many
/// providers are enabled (see `tray::enabled_bars`) — disabled providers
/// contribute no bar at all rather than a placeholder gray one. Color comes
/// directly from the passed `Health` rather than being recomputed from the
/// percentage here, so the icon has no threshold knowledge of its own: the
/// shell already ran `quota_math::health_for` with the configured
/// (possibly custom) thresholds before calling this.
pub fn render_tray_icon(bars: &[(Option<f64>, Health)]) -> Vec<u8> {
    let mut buf = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let n = bars.len();
    if n == 0 {
        // All providers disabled: fully transparent icon, no bars at all.
        return buf;
    }
    let total_width = BAR_WIDTH * n + BAR_GAP * (n - 1);
    let margin = (ICON_SIZE as usize).saturating_sub(total_width) / 2;
    for (i, (remaining, health)) in bars.iter().enumerate() {
        let x0 = margin + i * (BAR_WIDTH + BAR_GAP);
        let x1 = x0 + BAR_WIDTH;
        match remaining {
            None => {
                let color = fill_color(*health);
                for y in TRACK_TOP..TRACK_BOTTOM {
                    for x in x0..x1 {
                        put(&mut buf, x, y, color);
                    }
                }
            }
            Some(pct) => {
                let pct = pct.clamp(0.0, 100.0);
                let mut fill = ((TRACK_HEIGHT as f64) * pct / 100.0).ceil() as usize;
                if fill == 0 {
                    fill = 1;
                }
                let color = fill_color(*health);
                for y in TRACK_TOP..TRACK_BOTTOM {
                    let filled = y >= TRACK_BOTTOM - fill;
                    for x in x0..x1 {
                        put(&mut buf, x, y, if filled { color } else { TRACK });
                    }
                }
            }
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * 32 + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn buffer_is_32x32_rgba() {
        assert_eq!(
            render_tray_icon(&[(None, Health::Unavailable); 4]).len(),
            32 * 32 * 4
        );
    }

    #[test]
    fn corners_are_transparent() {
        let buf = render_tray_icon(&[(Some(50.0), Health::Amber); 4]);
        assert_eq!(px(&buf, 0, 0)[3], 0);
        assert_eq!(px(&buf, 31, 31)[3], 0);
    }

    #[test]
    fn healthy_low_and_missing_bars_have_expected_colors() {
        let buf = render_tray_icon(&[
            (Some(100.0), Health::Green),
            (None, Health::Unavailable),
            (Some(5.0), Health::Red),
            (Some(20.0), Health::Amber),
        ]);
        assert_eq!(px(&buf, 4, 28), GREEN, "full bar bottom is green");
        assert_eq!(px(&buf, 4, 3), GREEN, "full bar reaches the top");
        assert_eq!(
            px(&buf, 12, 15),
            GRAY,
            "missing provider renders dim gray full bar"
        );
        assert_eq!(px(&buf, 20, 28), RED, "5% remaining renders red");
        assert_eq!(px(&buf, 20, 5), TRACK, "top of low bar is just track");
        assert_eq!(px(&buf, 28, 28), AMBER, "20% remaining renders amber");
    }

    #[test]
    fn amber_between_10_and_30() {
        let buf = render_tray_icon(&[(Some(20.0), Health::Amber); 4]);
        assert_eq!(px(&buf, 4, 28), AMBER);
    }

    #[test]
    fn zero_percent_remaining_still_shows_one_pixel_of_red() {
        // Regression: pct == 0.0 must still render a visible sliver of its
        // health color at the bottom of the bar, not an all-TRACK bar that
        // looks indistinguishable from a healthy empty gauge.
        let buf = render_tray_icon(&[
            (Some(0.0), Health::Red),
            (None, Health::Unavailable),
            (None, Health::Unavailable),
            (None, Health::Unavailable),
        ]);
        assert_eq!(
            px(&buf, 4, 29),
            RED,
            "bottom fill pixel of a 0%-remaining bar must be red"
        );
    }

    #[test]
    fn three_bars_are_centered_with_margin_5() {
        // total = 8*3 - 2 = 22, margin = (32 - 22) / 2 = 5.
        let buf = render_tray_icon(&[
            (Some(100.0), Health::Green),
            (Some(100.0), Health::Green),
            (Some(100.0), Health::Green),
        ]);
        assert_eq!(px(&buf, 4, 15)[3], 0, "just left of first bar is empty");
        assert_eq!(px(&buf, 5, 15), GREEN, "first bar starts at x=5");
        assert_eq!(px(&buf, 10, 15), GREEN, "first bar ends at x=10");
        assert_eq!(px(&buf, 11, 15)[3], 0, "gap after first bar is empty");
        assert_eq!(px(&buf, 13, 15), GREEN, "second bar starts at x=13");
    }

    #[test]
    fn one_bar_is_centered_with_margin_13() {
        // total = 8*1 - 2 = 6, margin = (32 - 6) / 2 = 13.
        let buf = render_tray_icon(&[(Some(100.0), Health::Green)]);
        assert_eq!(
            px(&buf, 12, 15)[3],
            0,
            "just left of the single bar is empty"
        );
        assert_eq!(px(&buf, 13, 15), GREEN, "bar starts at x=13");
        assert_eq!(px(&buf, 18, 15), GREEN, "bar ends at x=18");
        assert_eq!(
            px(&buf, 19, 15)[3],
            0,
            "just right of the single bar is empty"
        );
    }

    #[test]
    fn zero_bars_returns_fully_transparent_buffer() {
        let buf = render_tray_icon(&[]);
        assert_eq!(buf.len(), 32 * 32 * 4);
        assert!(
            buf.iter().all(|&b| b == 0),
            "all providers disabled must render a fully transparent icon"
        );
    }
}
