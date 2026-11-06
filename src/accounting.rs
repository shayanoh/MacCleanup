/// One thing that happened this session, for the finish screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub label: String,
    pub bytes: u64,
}

/// Tracks pending (soft-deleted) bytes and what was truly freed.
/// "Freed" is always measured as the volume's free space now minus at start.
#[derive(Clone, Debug)]
pub struct Accounting {
    free_at_start: u64,
    pending: u64,
    events: Vec<Event>,
}

impl Accounting {
    pub fn new(free_at_start: u64) -> Self {
        Self {
            free_at_start,
            pending: 0,
            events: Vec::new(),
        }
    }

    fn push(&mut self, label: &str, bytes: u64) {
        self.events.push(Event {
            label: label.to_string(),
            bytes,
        });
    }

    pub fn record_soft_delete(&mut self, bytes: u64) {
        self.pending = self.pending.saturating_add(bytes);
        self.push("moved to ToBeDeleted", bytes);
    }

    pub fn record_restore(&mut self, bytes: u64) {
        self.pending = self.pending.saturating_sub(bytes);
        self.push("restored", bytes);
    }

    pub fn record_emptied(&mut self, bytes: u64) {
        self.pending = self.pending.saturating_sub(bytes);
        self.push("emptied ToBeDeleted", bytes);
    }

    pub fn pending(&self) -> u64 {
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

    #[test]
    fn soft_delete_is_pending_not_freed() {
        let mut a = Accounting::new(100);
        a.record_soft_delete(40);
        assert_eq!(a.pending(), 40);
        assert_eq!(a.freed(100), 0);
    }

    #[test]
    fn emptied_reduces_pending() {
        let mut a = Accounting::new(100);
        a.record_soft_delete(40);
        a.record_emptied(40);
        assert_eq!(a.pending(), 0);
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
        a.record_soft_delete(40);
        a.record_restore(15);
        assert_eq!(a.pending(), 25);
    }

    #[test]
    fn pending_saturates() {
        let mut a = Accounting::new(0);
        a.record_soft_delete(10);
        a.record_emptied(50);
        assert_eq!(a.pending(), 0);
    }

    #[test]
    fn events_are_recorded_in_order() {
        let mut a = Accounting::new(7);
        a.record_soft_delete(3);
        a.record_restore(1);
        a.record_emptied(2);
        let labels: Vec<&str> = a.events().iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            ["moved to ToBeDeleted", "restored", "emptied ToBeDeleted"]
        );
        assert_eq!(a.events()[0].bytes, 3);
        assert_eq!(a.free_at_start(), 7);
    }
}
