use quotabar_core::{health_for, Health};

pub const ICON_SIZE: u32 = 32;

pub const GREEN: [u8; 4] = [151, 196, 89, 255];
pub const AMBER: [u8; 4] = [239, 159, 39, 255];
pub const RED: [u8; 4] = [226, 75, 74, 255];
pub const GRAY: [u8; 4] = [110, 110, 106, 255];
pub const TRACK: [u8; 4] = [68, 68, 65, 140];

// Four 6px bars, 2px gaps, 1px margins: 1 + 6 + 2 + 6 + 2 + 6 + 2 + 6 + 1 = 32.
const BAR_X: [(usize, usize); 4] = [(1, 7), (9, 15), (17, 23), (25, 31)];
const TRACK_TOP: usize = 2;
const TRACK_BOTTOM: usize = 30; // exclusive
const TRACK_HEIGHT: usize = TRACK_BOTTOM - TRACK_TOP;

fn put(buf: &mut [u8], x: usize, y: usize, c: [u8; 4]) {
    let i = (y * 32 + x) * 4;
    buf[i..i + 4].copy_from_slice(&c);
}

fn fill_color(remaining: f64) -> [u8; 4] {
    match health_for(remaining) {
        Health::Green => GREEN,
        Health::Amber => AMBER,
        Health::Red => RED,
        Health::Unavailable => GRAY,
    }
}

pub fn render_tray_icon(remaining: [Option<f64>; 4]) -> Vec<u8> {
    let mut buf = vec![0u8; 32 * 32 * 4];
    for (bar, (x0, x1)) in BAR_X.iter().enumerate() {
        match remaining[bar] {
            None => {
                for y in TRACK_TOP..TRACK_BOTTOM {
                    for x in *x0..*x1 {
                        put(&mut buf, x, y, GRAY);
                    }
                }
            }
            Some(pct) => {
                let pct = pct.clamp(0.0, 100.0);
                let mut fill = ((TRACK_HEIGHT as f64) * pct / 100.0).ceil() as usize;
                if pct > 0.0 && fill == 0 {
                    fill = 1;
                }
                let color = fill_color(pct);
                for y in TRACK_TOP..TRACK_BOTTOM {
                    let filled = y >= TRACK_BOTTOM - fill;
                    for x in *x0..*x1 {
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
            render_tray_icon([None, None, None, None]).len(),
            32 * 32 * 4
        );
    }

    #[test]
    fn corners_are_transparent() {
        let buf = render_tray_icon([Some(50.0), Some(50.0), Some(50.0), Some(50.0)]);
        assert_eq!(px(&buf, 0, 0)[3], 0);
        assert_eq!(px(&buf, 31, 31)[3], 0);
    }

    #[test]
    fn healthy_low_and_missing_bars_have_expected_colors() {
        let buf = render_tray_icon([Some(100.0), None, Some(5.0), Some(20.0)]);
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
        let buf = render_tray_icon([Some(20.0), Some(20.0), Some(20.0), Some(20.0)]);
        assert_eq!(px(&buf, 4, 28), AMBER);
    }
}
