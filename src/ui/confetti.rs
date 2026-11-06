//! Confetti for the congratulations window: a deterministic, tick-driven rain
//! of coloured pieces. Pure functions of the tick, so it is easy to test and
//! never needs any state.

use ratatui::style::Color;

/// Confetti lasts this many ticks (8 seconds at 20 frames per second).
pub const DURATION_TICKS: u64 = 160;

/// One piece of confetti, positioned inside the window's inner area.
pub struct Piece {
    pub x: u16,
    pub y: u16,
    pub glyph: &'static str,
    pub color: Color,
}

const NERD_GLYPHS: [&str; 6] = ["✦", "✧", "◆", "▴", "▪", "✶"];
const ASCII_GLYPHS: [&str; 6] = ["*", "+", "o", "'", "~", "^"];
const COLORS: [Color; 7] = [
    Color::Red,
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Magenta,
    Color::LightBlue,
    Color::LightRed,
];

/// A small stateless hash (splitmix64) so every piece gets stable random
/// traits from its index.
fn mix(i: u64, salt: u64) -> u64 {
    let mut z = i
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(salt.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The visible pieces `tick` ticks after the window finished opening, for a
/// `w` x `h` area. Pieces fall at different speeds, sway a little, ramp in
/// over the first 30 ticks and thin out over the last 50. Nothing appears in
/// areas too small to hold it.
pub fn pieces(tick: u64, w: u16, h: u16, ascii: bool) -> Vec<Piece> {
    if w < 10 || h < 4 || tick >= DURATION_TICKS {
        return Vec::new();
    }
    let (w64, h64) = (w as u64, h as u64);
    let most = (w64 * h64 / 14).clamp(8, 70) as f64;
    let ramp_in = 0.4 + 0.6 * (tick.min(30) as f64 / 30.0);
    let fade_out = if tick + 50 > DURATION_TICKS {
        (DURATION_TICKS - tick) as f64 / 50.0
    } else {
        1.0
    };
    let active = ((most * ramp_in * fade_out).round() as u64).max(1);
    let glyphs = if ascii { &ASCII_GLYPHS } else { &NERD_GLYPHS };

    let mut out = Vec::new();
    for i in 0..active {
        let slow = 1 + mix(i, 2) % 3; // a row every 2..=4 ticks
        let start = mix(i, 3) % (h64 + 4);
        let row = (tick / (slow + 1) + start) % (h64 + 4);
        if row >= h64 {
            continue; // still above or already below the window
        }
        let sway = [0i64, 1, 0, -1][((tick / 3 + i) % 4) as usize];
        let x = (mix(i, 1) % w64) as i64 + sway;
        out.push(Piece {
            x: x.clamp(0, w64 as i64 - 1) as u16,
            y: row as u16,
            glyph: glyphs[(mix(i, 5) % glyphs.len() as u64) as usize],
            color: COLORS[(mix(i, 6) % COLORS.len() as u64) as usize],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_stay_inside_the_area() {
        for (w, h) in [(10, 4), (40, 8), (80, 20), (200, 50)] {
            for tick in (0..DURATION_TICKS).step_by(3) {
                for p in pieces(tick, w, h, false) {
                    assert!(p.x < w && p.y < h, "{tick} {w}x{h}: {},{}", p.x, p.y);
                }
            }
        }
    }

    #[test]
    fn same_tick_gives_the_same_pieces() {
        let a: Vec<_> = pieces(37, 60, 10, false)
            .iter()
            .map(|p| (p.x, p.y, p.glyph))
            .collect();
        let b: Vec<_> = pieces(37, 60, 10, false)
            .iter()
            .map(|p| (p.x, p.y, p.glyph))
            .collect();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn pieces_fall_as_time_passes() {
        let at = |t| -> Vec<(u16, u16)> {
            pieces(t, 60, 10, false)
                .iter()
                .map(|p| (p.x, p.y))
                .collect()
        };
        assert_ne!(at(20), at(21));
        assert_ne!(at(20), at(30));
    }

    #[test]
    fn ascii_mode_uses_ascii_glyphs_only() {
        for tick in 0..DURATION_TICKS {
            for p in pieces(tick, 60, 10, true) {
                assert!(p.glyph.is_ascii(), "{:?}", p.glyph);
            }
        }
    }

    #[test]
    fn tiny_areas_get_no_confetti() {
        assert!(pieces(10, 9, 10, false).is_empty());
        assert!(pieces(10, 60, 3, false).is_empty());
        assert!(pieces(10, 0, 0, false).is_empty());
    }

    #[test]
    fn confetti_ends_after_its_duration() {
        assert!(pieces(DURATION_TICKS, 60, 10, false).is_empty());
        assert!(pieces(u64::MAX, 60, 10, false).is_empty());
    }

    #[test]
    fn confetti_ramps_in_and_fades_out() {
        let n = |t| pieces(t, 80, 20, false).len();
        assert!(n(0) > 0, "something appears at once");
        assert!(n(30) > n(0), "more pieces join");
        assert!(n(DURATION_TICKS - 1) < n(60), "it thins out at the end");
    }

    #[test]
    fn pieces_use_several_colours_and_glyphs() {
        let ps = pieces(40, 80, 20, false);
        let colours: std::collections::HashSet<_> =
            ps.iter().map(|p| format!("{:?}", p.color)).collect();
        let glyphs: std::collections::HashSet<_> = ps.iter().map(|p| p.glyph).collect();
        assert!(colours.len() >= 3, "{colours:?}");
        assert!(glyphs.len() >= 3, "{glyphs:?}");
    }
}
