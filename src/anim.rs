/// Fast start, gentle finish. Input is clamped to 0..=1 (NaN counts as done).
pub fn ease_out_cubic(t: f64) -> f64 {
    if t.is_nan() {
        return 1.0;
    }
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The spinner advances one frame every two ticks and wraps.
pub fn spinner_frame(tick: u64, frames: usize) -> usize {
    if frames == 0 {
        return 0;
    }
    ((tick / 2) % frames as u64) as usize
}

/// Animate a number from `from` to `to`, eased. Reaches `to` exactly at
/// `duration_ticks` (or at once when the duration is zero).
pub fn count_up(from: u64, to: u64, elapsed_ticks: u64, duration_ticks: u64) -> u64 {
    if duration_ticks == 0 || elapsed_ticks >= duration_ticks {
        return to;
    }
    let eased = ease_out_cubic(elapsed_ticks as f64 / duration_ticks as f64);
    let delta = to.abs_diff(from) as u128;
    let part = ((delta as f64 * eased) as u128).min(delta);
    let value = if to >= from {
        from as u128 + part
    } else {
        from as u128 - part
    };
    value as u64
}

/// A value that glides toward a target a fraction at a time.
#[derive(Clone, Copy, Debug)]
pub struct Smooth {
    pub shown: f64,
}

impl Smooth {
    pub fn new(value: f64) -> Self {
        Self { shown: value }
    }

    /// Move a fraction `rate` (0..=1) toward `target`; snaps when within 0.5.
    pub fn step(&mut self, target: f64, rate: f64) {
        if !target.is_finite() {
            return;
        }
        let rate = if rate.is_finite() {
            rate.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let next = self.shown + (target - self.shown) * rate;
        self.shown = if (target - next).abs() < 0.5 {
            target
        } else {
            next
        };
    }

    pub fn value(&self) -> f64 {
        self.shown
    }
}

/// How many of `width` columns a left-to-right wipe has revealed.
pub fn wipe_columns(progress: f64, width: u16) -> u16 {
    let p = if progress.is_nan() {
        1.0
    } else {
        progress.clamp(0.0, 1.0)
    };
    (p * width as f64).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_bounds_and_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(-3.0), 0.0);
        assert_eq!(ease_out_cubic(9.0), 1.0);
        assert_eq!(ease_out_cubic(f64::NAN), 1.0);
        let mut prev = 0.0;
        for i in 0..=100 {
            let v = ease_out_cubic(i as f64 / 100.0);
            assert!((0.0..=1.0).contains(&v));
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn spinner_wraps_and_advances_slowly() {
        let frames: Vec<usize> = (0..9).map(|t| spinner_frame(t, 4)).collect();
        assert_eq!(frames, [0, 0, 1, 1, 2, 2, 3, 3, 0]);
        assert_eq!(spinner_frame(123, 0), 0);
        assert!(spinner_frame(u64::MAX, 10) < 10);
    }

    #[test]
    fn count_up_endpoints() {
        assert_eq!(count_up(100, 900, 0, 20), 100);
        assert_eq!(count_up(100, 900, 20, 20), 900);
        assert_eq!(count_up(100, 900, 500, 20), 900);
        assert_eq!(count_up(100, 900, 5, 0), 900);
        let mid = count_up(0, 1000, 10, 20);
        assert!(mid > 0 && mid < 1000);
    }

    #[test]
    fn count_up_descending() {
        assert_eq!(count_up(900, 100, 0, 20), 900);
        assert_eq!(count_up(900, 100, 20, 20), 100);
        let mid = count_up(900, 100, 10, 20);
        assert!(mid > 100 && mid < 900);
    }

    #[test]
    fn count_up_no_overflow() {
        assert_eq!(count_up(0, u64::MAX, 20, 20), u64::MAX);
        assert_eq!(count_up(u64::MAX, 0, 0, 20), u64::MAX);
        let mid = count_up(0, u64::MAX, 7, 20);
        assert!(mid > 0 && mid < u64::MAX);
        let mid = count_up(u64::MAX, 0, 7, 20);
        assert!(mid > 0 && mid < u64::MAX);
    }

    #[test]
    fn count_up_is_monotonic() {
        let mut prev = 0;
        for t in 0..=20 {
            let v = count_up(0, 12_345, t, 20);
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn smooth_converges_and_snaps() {
        let mut s = Smooth::new(0.0);
        for _ in 0..200 {
            s.step(100.0, 0.25);
        }
        assert_eq!(s.value(), 100.0);
        let mut s = Smooth::new(10.0);
        s.step(10.2, 0.0);
        assert_eq!(s.value(), 10.2, "snaps when already close");
        let mut s = Smooth::new(5.0);
        s.step(f64::NAN, 0.5);
        assert_eq!(s.value(), 5.0, "ignores non-finite targets");
    }

    #[test]
    fn wipe_columns_bounds() {
        assert_eq!(wipe_columns(0.0, 80), 0);
        assert_eq!(wipe_columns(1.0, 80), 80);
        assert_eq!(wipe_columns(0.5, 80), 40);
        assert_eq!(wipe_columns(-1.0, 80), 0);
        assert_eq!(wipe_columns(2.0, 80), 80);
        assert_eq!(wipe_columns(f64::NAN, 80), 80);
        assert_eq!(wipe_columns(0.5, 0), 0);
    }
}
