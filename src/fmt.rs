use crate::glyphs::Glyphs;
use crate::usage::Usage;

const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// Decimal sizes (1 GB = 1,000,000,000 bytes), matching Finder and Docker.
pub fn human_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A bar exactly `width` characters wide, filled to `frac` (clamped, NaN = 0).
pub fn eighths_bar(frac: f64, width: usize, g: &Glyphs) -> String {
    let levels = g.eighths();
    let steps = levels.len() - 1;
    let frac = if frac.is_nan() {
        0.0
    } else {
        frac.clamp(0.0, 1.0)
    };
    let total = (frac * (width * steps) as f64).round() as usize;
    let full = (total / steps).min(width);
    let rem = if full == width { 0 } else { total % steps };
    let mut out = String::with_capacity(width * 3);
    for i in 0..width {
        let c = if i < full {
            levels[steps]
        } else if i == full {
            levels[rem]
        } else {
            levels[0]
        };
        out.push(c);
    }
    out
}

/// A bar of unknown length: a lit block moving back and forth, exactly
/// `width` characters wide. `tick` is the app tick (it advances every other tick).
pub fn marquee(tick: u64, width: usize, g: &Glyphs) -> String {
    let levels = g.eighths();
    let (off, on) = (levels[0], levels[levels.len() - 1]);
    if width == 0 {
        return String::new();
    }
    let block = (width / 4).max(1).min(width);
    let span = width - block;
    let pos = if span == 0 {
        0
    } else {
        let t = (tick / 2) as usize % (2 * span);
        if t <= span { t } else { 2 * span - t }
    };
    (0..width)
        .map(|i| if i >= pos && i < pos + block { on } else { off })
        .collect()
}

/// "1.5 KB (4.1 KB on disk)": the logical size with what it takes on disk.
pub fn both(size: u64, disk: u64) -> String {
    format!("{} ({} on disk)", human_size(size), human_size(disk))
}

pub fn both_usage(u: Usage) -> String {
    both(u.size, u.disk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyphs::{GlyphMode, Glyphs};

    #[test]
    fn the_marquee_keeps_its_width_and_moves_back_and_forth() {
        let g = Glyphs::new(GlyphMode::Nerd);
        let full = *g.eighths().last().unwrap();
        let mut seen = std::collections::HashSet::new();
        for tick in 0..80 {
            let m = marquee(tick, 12, &g);
            assert_eq!(m.chars().count(), 12, "tick {tick}");
            let lit = m.chars().filter(|c| *c == full).count();
            assert_eq!(lit, 3, "{m:?}");
            seen.insert(m);
        }
        assert!(seen.len() > 4, "it moves");
        // The block comes back: same picture after one full there-and-back.
        assert_eq!(marquee(0, 12, &g), marquee(2 * 2 * 9, 12, &g));
    }

    #[test]
    fn the_marquee_never_panics_on_tiny_widths() {
        let g = Glyphs::new(GlyphMode::Ascii);
        for w in 0..5 {
            for tick in 0..20 {
                assert_eq!(marquee(tick, w, &g).chars().count(), w);
            }
        }
    }

    #[test]
    fn human_size_cases() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1000), "1.0 KB");
        assert_eq!(human_size(1_500_000), "1.5 MB");
        assert_eq!(human_size(21_000_000_000), "21.0 GB");
    }

    #[test]
    fn bar_width_is_exact() {
        for mode in [GlyphMode::Nerd, GlyphMode::Ascii] {
            let g = Glyphs::new(mode);
            for width in [0usize, 1, 10] {
                for frac in [-1.0, 0.0, 0.33, 1.0, 5.0, f64::NAN] {
                    let s = eighths_bar(frac, width, &g);
                    assert_eq!(s.chars().count(), width, "width {width} frac {frac}");
                }
            }
        }
    }

    #[test]
    fn bar_full_and_empty() {
        let g = Glyphs::new(GlyphMode::Nerd);
        let full = *g.eighths().last().unwrap();
        let empty = g.eighths()[0];
        assert!(eighths_bar(1.0, 6, &g).chars().all(|c| c == full));
        assert!(eighths_bar(0.0, 6, &g).chars().all(|c| c == empty));
    }

    #[test]
    fn both_shows_size_and_on_disk() {
        assert_eq!(both(1500, 4096), "1.5 KB (4.1 KB on disk)");
        assert_eq!(both(0, 0), "0 B (0 B on disk)");
    }
}
