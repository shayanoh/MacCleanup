use std::iter::Sum;
use std::ops::{Add, AddAssign};

/// A size in both senses: logical bytes and bytes allocated on disk.
/// Deleting frees the on-disk number, so both are tracked everywhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub size: u64,
    pub disk: u64,
}

impl Usage {
    pub const fn new(size: u64, disk: u64) -> Self {
        Self { size, disk }
    }

    pub fn saturating_sub(self, other: Usage) -> Usage {
        Usage::new(
            self.size.saturating_sub(other.size),
            self.disk.saturating_sub(other.disk),
        )
    }
}

impl Add for Usage {
    type Output = Usage;
    fn add(self, other: Usage) -> Usage {
        Usage::new(
            self.size.saturating_add(other.size),
            self.disk.saturating_add(other.disk),
        )
    }
}

impl AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        *self = *self + other;
    }
}

impl Sum for Usage {
    fn sum<I: Iterator<Item = Usage>>(iter: I) -> Usage {
        iter.fold(Usage::default(), |a, b| a + b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_default_and_fields() {
        assert_eq!(Usage::default(), Usage::new(0, 0));
        let u = Usage::new(10, 4096);
        assert_eq!((u.size, u.disk), (10, 4096));
    }

    #[test]
    fn addition_and_sum_saturate() {
        let mut u = Usage::new(1, 2) + Usage::new(3, 4);
        assert_eq!(u, Usage::new(4, 6));
        u += Usage::new(1, 1);
        assert_eq!(u, Usage::new(5, 7));
        let total: Usage = [Usage::new(1, 10), Usage::new(2, 20)].into_iter().sum();
        assert_eq!(total, Usage::new(3, 30));
        let max = Usage::new(u64::MAX, u64::MAX) + Usage::new(1, 1);
        assert_eq!(max, Usage::new(u64::MAX, u64::MAX));
    }

    #[test]
    fn saturating_sub_never_goes_negative() {
        assert_eq!(
            Usage::new(10, 20).saturating_sub(Usage::new(3, 5)),
            Usage::new(7, 15)
        );
        assert_eq!(
            Usage::new(1, 1).saturating_sub(Usage::new(5, 5)),
            Usage::new(0, 0)
        );
    }
}
