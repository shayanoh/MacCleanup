use crate::usage::Usage;

/// One thing that happened this session, for the finish screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub label: String,
    pub usage: Usage,
}

/// Tracks pending (soft-deleted) bytes and what was truly freed.
/// "Freed" is always measured as the volume's free space now minus at start.
#[derive(Clone, Debug)]
pub struct Accounting {
    free_at_start: u64,
    pending: Usage,
    events: Vec<Event>,
}

impl Accounting {
    pub fn new(free_at_start: u64) -> Self {
        Self {
            free_at_start,
            pending: Usage::default(),
            events: Vec::new(),
        }
    }

    fn push(&mut self, label: &str, usage: Usage) {
        self.events.push(Event {
            label: label.to_string(),
            usage,
        });
    }

    pub fn record_soft_delete(&mut self, usage: Usage) {
        self.pending += usage;
        self.push("moved to ToBeDeleted", usage);
    }

    pub fn record_restore(&mut self, usage: Usage) {
        self.pending = self.pending.saturating_sub(usage);
        self.push("restored", usage);
    }

    pub fn record_emptied(&mut self, usage: Usage) {
        self.pending = self.pending.saturating_sub(usage);
        self.push("emptied ToBeDeleted", usage);
    }

    /// Soft-deleted but not yet freed.
    pub fn pending(&self) -> Usage {
        self.pending
    }

    pub fn freed(&self, free_now: u64) -> u64 {
        free_now.saturating_sub(self.free_at_start)
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn free_at_start(&self) -> u64 {
        self.free_at_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(size: u64, disk: u64) -> Usage {
        Usage::new(size, disk)
    }

    #[test]
    fn soft_delete_is_pending_not_freed() {
        let mut a = Accounting::new(100);
        a.record_soft_delete(u(40, 64));
        assert_eq!(a.pending(), u(40, 64));
        assert_eq!(a.freed(100), 0);
    }

    #[test]
    fn emptied_reduces_pending() {
        let mut a = Accounting::new(100);
        a.record_soft_delete(u(40, 64));
        a.record_emptied(u(40, 64));
        assert_eq!(a.pending(), u(0, 0));
        assert_eq!(a.freed(140), 40);
    }

    #[test]
    fn freed_never_negative() {
        let a = Accounting::new(100);
        assert_eq!(a.freed(50), 0);
    }

    #[test]
    fn restore_reduces_pending() {
        let mut a = Accounting::new(100);
        a.record_soft_delete(u(40, 64));
        a.record_restore(u(15, 16));
        assert_eq!(a.pending(), u(25, 48));
    }

    #[test]
    fn pending_saturates() {
        let mut a = Accounting::new(0);
        a.record_soft_delete(u(10, 10));
        a.record_emptied(u(50, 50));
        assert_eq!(a.pending(), u(0, 0));
    }

    #[test]
    fn events_are_recorded_in_order() {
        let mut a = Accounting::new(7);
        a.record_soft_delete(u(3, 4));
        a.record_restore(u(1, 2));
        a.record_emptied(u(2, 3));
        let labels: Vec<&str> = a.events().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            ["moved to ToBeDeleted", "restored", "emptied ToBeDeleted"]
        );
        assert_eq!(a.events()[0].usage, u(3, 4));
        assert_eq!(a.free_at_start(), 7);
    }
}
