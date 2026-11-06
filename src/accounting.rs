use crate::fmt::{both_usage, human_size};
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
    moved_total: Usage,
    emptied_total: Usage,
    restored_total: Usage,
    /// Most that the tool commands (Docker, pnpm, ...) that succeeded could
    /// free: an estimate, the real figure is in the free-space change.
    commands_total: u64,
    events: Vec<Event>,
}

impl Accounting {
    pub fn new(free_at_start: u64) -> Self {
        Self {
            free_at_start,
            pending: Usage::default(),
            moved_total: Usage::default(),
            emptied_total: Usage::default(),
            restored_total: Usage::default(),
            commands_total: 0,
            events: Vec::new(),
        }
    }

    fn push(&mut self, label: &str, usage: Usage) {
        self.events.push(Event {
            label: label.to_string(),
            usage,
        });
    }

    /// Start the session with what earlier sessions left in ToBeDeleted.
    /// Not an event: nothing happened this session.
    pub fn set_pending(&mut self, usage: Usage) {
        self.pending = usage;
    }

    pub fn record_soft_delete(&mut self, usage: Usage) {
        self.pending += usage;
        self.moved_total += usage;
        self.push("moved to ToBeDeleted", usage);
    }

    pub fn record_restore(&mut self, usage: Usage) {
        self.pending = self.pending.saturating_sub(usage);
        self.restored_total += usage;
        self.push("restored", usage);
    }

    pub fn record_emptied(&mut self, usage: Usage) {
        self.pending = self.pending.saturating_sub(usage);
        self.emptied_total += usage;
        self.push("emptied ToBeDeleted", usage);
    }

    /// Tool commands that ran fine, with the most they might have freed.
    pub fn record_commands(&mut self, up_to: u64) {
        self.commands_total += up_to;
        self.push("tool commands", Usage::new(up_to, up_to));
    }

    /// Soft-deleted but not yet freed.
    pub fn pending(&self) -> Usage {
        self.pending
    }

    pub fn freed(&self, free_now: u64) -> u64 {
        free_now.saturating_sub(self.free_at_start)
    }

    /// What to print when the app exits: totals for the whole session, or
    /// `None` if nothing was moved, restored or deleted.
    pub fn summary(&self, free_now: u64) -> Option<String> {
        if self.events.is_empty() {
            return None;
        }
        let mut rows: Vec<(&str, String)> = Vec::new();
        if self.moved_total != Usage::default() {
            rows.push(("moved to ToBeDeleted", both_usage(self.moved_total)));
        }
        rows.push(("deleted for good", both_usage(self.emptied_total)));
        if self.commands_total > 0 {
            rows.push((
                "tool commands",
                format!(
                    "up to {} (Docker, caches; see space freed)",
                    human_size(self.commands_total)
                ),
            ));
        }
        if self.restored_total != Usage::default() {
            rows.push(("restored", both_usage(self.restored_total)));
        }
        if self.pending != Usage::default() {
            rows.push((
                "still in ToBeDeleted",
                format!("{} (empty it to free that space)", both_usage(self.pending)),
            ));
        }
        rows.push(("free at start", human_size(self.free_at_start)));
        rows.push(("free now", human_size(free_now)));
        let change = free_now as i128 - self.free_at_start as i128;
        let sign = if change < 0 { "-" } else { "+" };
        rows.push((
            "space freed",
            format!("{sign}{}", human_size(change.unsigned_abs() as u64)),
        ));
        let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
        let mut text = String::from("MacCleanup summary");
        for (label, value) in rows {
            text.push_str(&format!("\n  {label:<width$}  {value}"));
        }
        Some(text)
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

    #[test]
    fn set_pending_starts_a_session_with_what_is_already_held() {
        let mut a = Accounting::new(0);
        a.set_pending(u(100, 128));
        assert_eq!(a.pending(), u(100, 128));
        assert!(a.events().is_empty());
        a.record_emptied(u(40, 64));
        assert_eq!(a.pending(), u(60, 64));
    }

    #[test]
    fn summary_is_none_when_nothing_happened() {
        assert_eq!(Accounting::new(100).summary(100), None);
    }

    #[test]
    fn summary_lists_totals_and_freed_space() {
        let mut a = Accounting::new(1000);
        a.record_soft_delete(u(40, 64));
        a.record_emptied(u(40, 64));
        let text = a.summary(1064).unwrap();
        assert!(text.starts_with("MacCleanup summary"), "{text}");
        assert!(text.contains("moved to ToBeDeleted"), "{text}");
        assert!(
            text.contains("deleted for good") && text.contains("40 B (64 B on disk)"),
            "{text}"
        );
        assert!(
            text.contains("space freed") && text.contains("64 B"),
            "{text}"
        );
        assert!(!text.contains("restored"), "zero rows are hidden: {text}");
        assert!(!text.contains("still in ToBeDeleted"), "{text}");
    }

    #[test]
    fn summary_mentions_what_is_still_pending() {
        let mut a = Accounting::new(1000);
        a.record_soft_delete(u(5000, 8192));
        a.record_soft_delete(u(5000, 8192));
        a.record_restore(u(5000, 8192));
        let text = a.summary(1000).unwrap();
        assert!(
            text.contains("restored") && text.contains("5.0 KB (8.2 KB on disk)"),
            "{text}"
        );
        assert!(text.contains("still in ToBeDeleted"), "{text}");
        assert!(text.contains("empty it"), "{text}");
        assert!(text.contains("space freed"), "{text}");
    }

    #[test]
    fn summary_never_reports_negative_freed_space() {
        let mut a = Accounting::new(1000);
        a.record_emptied(u(1, 1));
        let text = a.summary(500).unwrap();
        assert!(
            text.contains("space freed") && text.contains("0 B"),
            "{text}"
        );
    }
}
