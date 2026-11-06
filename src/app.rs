use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::layout::Rect;

use crate::accounting::Accounting;
use crate::anim::Smooth;
use crate::filter;
use crate::fmt::both_usage;
use crate::glyphs::Glyphs;
use crate::jobs::{DeleteEvent, DeleteJob, DeleteMode, Phase};
use crate::safety::{Policy, Safety};
use crate::scan::{Breakdown, Node, Row, Scanner, file_breakdown, locate};
use crate::softdelete::{HeldItem, Holding};
use crate::usage::Usage;
use crate::volume;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Explorer,
    Holding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Main,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphMode {
    Bars,
    Treemap,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    /// Allocated bytes on disk (the default; this is what a delete frees).
    Disk,
    /// Logical size.
    Size,
    Name,
}

/// One entry in the left sidebar. Entries without a screen are planned for
/// later releases and are shown dimmed.
pub struct SidebarEntry {
    pub label: &'static str,
    pub screen: Option<Screen>,
}

pub const SIDEBAR: [SidebarEntry; 8] = [
    SidebarEntry {
        label: "Home",
        screen: Some(Screen::Home),
    },
    SidebarEntry {
        label: "Explorer",
        screen: Some(Screen::Explorer),
    },
    SidebarEntry {
        label: "Leftovers",
        screen: None,
    },
    SidebarEntry {
        label: "Uninstall",
        screen: None,
    },
    SidebarEntry {
        label: "Caches",
        screen: None,
    },
    SidebarEntry {
        label: "Docker",
        screen: None,
    },
    SidebarEntry {
        label: "Projects",
        screen: None,
    },
    SidebarEntry {
        label: "ToBeDeleted",
        screen: Some(Screen::Holding),
    },
];

/// Screen geometry, written by the UI each frame so clicks can be mapped.
#[derive(Clone, Copy, Debug, Default)]
pub struct UiLayout {
    pub sidebar: Rect,
    pub main: Rect,
    /// First screen row of the list in the active pane.
    pub list_top: u16,
    /// How many list rows are visible.
    pub list_rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Into,
    Out,
    ToggleMark,
    OpenReview,
    CycleGraph,
    GrowGraph,
    ShrinkGraph,
    CycleSort,
    FocusToggle,
    GoTo(Screen),
    Click {
        x: u16,
        y: u16,
    },
    Scroll(i16),
    Tick,
    /// Rescan the folder being shown.
    Rescan,
    /// Rescan everything from the scan root.
    RescanAll,
    /// Open the filter box (`/`) on the explorer.
    FilterOpen,
    FilterChar(char),
    FilterBackspace,
    FilterDelete,
    FilterLeft,
    FilterRight,
    FilterHome,
    FilterEnd,
    FilterClearWord,
    FilterClearLine,
    /// Enter: stop typing but keep the filter applied.
    FilterAccept,
    /// Esc: remove the filter.
    FilterClear,
    /// From the quit warning: go and look at ToBeDeleted instead.
    ReviewHeld,
    Confirm,
    Cancel,
    ConfirmIrreversible,
    RestoreSelected,
    DeleteSelectedForever,
    EmptyAll,
}

#[derive(Clone, Debug)]
pub struct ReviewItem {
    pub path: PathBuf,
    pub usage: Usage,
    pub safety: Safety,
}

#[derive(Clone, Debug)]
pub struct Review {
    pub items: Vec<ReviewItem>,
    pub total: Usage,
    /// Of the total, what deleting actually frees (every hard link is in the
    /// marked items)...
    pub unique: Usage,
    /// ...and what also has a hard link elsewhere and so stays on disk.
    pub shared: Usage,
}

#[derive(Clone, Debug)]
pub struct DeleteProgress {
    pub done: usize,
    pub total: usize,
    /// Bytes processed so far, and the expected grand total.
    pub usage: Usage,
    pub of: Usage,
    pub phase: Phase,
    pub current: PathBuf,
}

#[derive(Clone, Debug)]
pub struct FinishInfo {
    pub moved: Usage,
    pub emptied: Usage,
    pub free_before: u64,
    pub free_after: u64,
    pub failures: usize,
    /// Why the first item failed, if any did.
    pub first_failure: Option<String>,
    /// App tick when the finish screen opened (drives the count-up).
    pub started_tick: u64,
}

/// What the quit warning tells the user is still waiting in ToBeDeleted.
#[derive(Clone, Debug)]
pub struct QuitWarning {
    pub count: usize,
    pub usage: Usage,
}

/// The explorer's filter box.
#[derive(Clone, Debug, Default)]
pub struct FilterState {
    pub query: String,
    /// Cursor position in characters.
    pub cursor: usize,
    /// True while the user is typing into the box.
    pub editing: bool,
}

impl FilterState {
    /// A non-empty filter is applied to the list.
    pub fn active(&self) -> bool {
        !self.query.is_empty()
    }

    /// The box is drawn while typing or while a filter is applied.
    pub fn visible(&self) -> bool {
        self.active() || self.editing
    }
}

/// What a "delete for good" confirmation applies to.
#[derive(Clone, Debug)]
pub enum ForeverTarget {
    Item {
        id: String,
        name: String,
        usage: Usage,
    },
    All {
        count: usize,
        usage: Usage,
    },
}

#[derive(Clone, Debug)]
pub enum Dialog {
    /// Quit was requested while ToBeDeleted still holds something.
    QuitWarning(QuitWarning),
    Review(Review),
    Irreversible(Review),
    ConfirmForever(ForeverTarget),
    Deleting(DeleteProgress),
    Finish(FinishInfo),
    Notice(String),
}

/// Unique vs shared space for the selected row.
#[derive(Clone, Debug)]
pub struct Detail {
    pub path: PathBuf,
    pub breakdown: Breakdown,
    /// False while the folder is still being scanned (numbers still growing).
    pub complete: bool,
}

pub struct App {
    pub screen: Screen,
    pub focus: Focus,
    pub graph: GraphMode,
    pub graph_pct: u16,
    pub sort: SortBy,
    /// The rows shown: `all_rows` narrowed by the filter.
    pub rows: Vec<Row>,
    /// Everything in the current folder, before filtering.
    pub all_rows: Vec<Row>,
    pub filter: FilterState,
    pub detail: Option<Detail>,
    pub cursor: usize,
    pub offset: usize,
    /// Marked paths with their size and on-disk size (kept live while scanning).
    pub marks: BTreeMap<PathBuf, Usage>,
    pub dialog: Option<Dialog>,
    /// The tick the current dialog appeared on (drives its grow-in animation).
    pub dialog_since: u64,
    dialog_kind: Option<std::mem::Discriminant<Dialog>>,
    pub should_quit: bool,
    /// Directories entered so far; the last one is the one being shown.
    pub stack: Vec<Arc<Node>>,
    pub held: Vec<HeldItem>,
    pub held_cursor: usize,
    pub held_offset: usize,
    pub accounting: Accounting,
    pub free_now: u64,
    /// `free_now` glided toward, for the animated gauge.
    pub free_shown: Smooth,
    pub total_space: u64,
    pub tick: u64,
    pub notice: Option<String>,
    pub layout: UiLayout,
    pub animations: bool,
    pub glyphs: Glyphs,
    pub transition_start: u64,
    pub fda_missing: bool,
    /// Set by `ui::draw`: the terminal is below the minimum size, so input
    /// is limited to quitting (the user cannot see what a key would do).
    pub too_small: bool,
    pub scanner: Scanner,
    pub policy: Arc<Policy>,
    pub holding: Arc<Holding>,
    rows_synced_done: bool,
    pending_select: Option<PathBuf>,
    job: Option<DeleteJob>,
    job_mode: DeleteMode,
    job_free_before: u64,
    job_failure: Option<String>,
    quit_after_job: bool,
}

fn sort_rows(rows: &mut [Row], by: SortBy) {
    match by {
        SortBy::Disk => rows.sort_by(|a, b| {
            b.disk
                .cmp(&a.disk)
                .then_with(|| b.size.cmp(&a.size))
                .then_with(|| a.name.cmp(&b.name))
        }),
        SortBy::Size => rows.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
        SortBy::Name => rows.sort_by_key(|r| r.name.to_string_lossy().to_lowercase()),
    }
}

/// Scroll window: keep `cursor` inside `[offset, offset + visible)`.
fn window(cursor: usize, offset: usize, visible: usize) -> usize {
    let visible = visible.max(1);
    if cursor < offset {
        cursor
    } else if cursor >= offset + visible {
        cursor + 1 - visible
    } else {
        offset
    }
}

impl App {
    pub fn new(
        scanner: Scanner,
        root: Arc<Node>,
        policy: Policy,
        holding: Holding,
        mut accounting: Accounting,
        glyphs: Glyphs,
        animations: bool,
    ) -> App {
        // Items left in ToBeDeleted by earlier sessions are pending too.
        accounting.set_pending(holding.total().unwrap_or_default());
        let free_now = volume::free_space(&root.path).unwrap_or(0);
        let total_space = volume::total_space(&root.path).unwrap_or(0);
        let mut app = App {
            screen: Screen::Home,
            // The menu starts focused so j/k and Enter work straight away.
            focus: Focus::Sidebar,
            graph: GraphMode::Bars,
            graph_pct: 35,
            sort: SortBy::Disk,
            rows: Vec::new(),
            all_rows: Vec::new(),
            filter: FilterState::default(),
            detail: None,
            cursor: 0,
            offset: 0,
            marks: BTreeMap::new(),
            dialog: None,
            dialog_since: 0,
            dialog_kind: None,
            should_quit: false,
            stack: vec![root],
            held: Vec::new(),
            held_cursor: 0,
            held_offset: 0,
            accounting,
            free_now,
            free_shown: Smooth::new(free_now as f64),
            total_space,
            tick: 0,
            notice: None,
            layout: UiLayout::default(),
            animations,
            glyphs,
            transition_start: 0,
            fda_missing: false,
            too_small: false,
            scanner,
            policy: Arc::new(policy),
            holding: Arc::new(holding),
            rows_synced_done: false,
            pending_select: None,
            job: None,
            job_mode: DeleteMode::SoftOnly,
            job_free_before: 0,
            job_failure: None,
            quit_after_job: false,
        };
        app.refresh_rows();
        app.refresh_held();
        app
    }

    pub fn current(&self) -> &Arc<Node> {
        &self.stack[self.stack.len() - 1]
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.cursor)
    }

    /// The number the graphs and share bars use for a row: the size you are
    /// sorting by (on-disk size unless sorted by logical size).
    pub fn weight(&self, row: &Row) -> u64 {
        match self.sort {
            SortBy::Size => row.size,
            SortBy::Disk | SortBy::Name => row.disk,
        }
    }

    /// The matching total for the folder being shown.
    pub fn weight_total(&self) -> u64 {
        match self.sort {
            SortBy::Size => self.current().size(),
            SortBy::Disk | SortBy::Name => self.current().disk(),
        }
    }

    /// Total of everything marked, logical and on disk.
    pub fn marked(&self) -> Usage {
        self.marks.values().copied().sum()
    }

    fn visible_rows(&self) -> usize {
        if self.layout.list_rows == 0 {
            10
        } else {
            self.layout.list_rows as usize
        }
    }

    pub fn ensure_visible(&mut self) {
        let vis = self.visible_rows();
        self.offset = window(self.cursor, self.offset, vis);
        self.held_offset = window(self.held_cursor, self.held_offset, vis);
    }

    /// Re-list the current directory, keeping the cursor on the same path.
    pub fn refresh_rows(&mut self) {
        let keep = self
            .pending_select
            .take()
            .or_else(|| self.selected_row().map(|r| r.path.clone()));
        let node = Arc::clone(self.current());
        let mut rows = self.scanner.list_children(&node);
        sort_rows(&mut rows, self.sort);
        self.rows_synced_done = node.is_done();
        for r in &rows {
            if let Some(usage) = self.marks.get_mut(&r.path) {
                *usage = Usage::new(r.size, r.disk);
            }
        }
        self.all_rows = rows;
        self.rows = self.matching_rows();
        let found = keep.and_then(|p| self.rows.iter().position(|r| r.path == p));
        self.cursor = found
            .unwrap_or(self.cursor)
            .min(self.rows.len().saturating_sub(1));
        self.ensure_visible();
        self.refresh_detail();
    }

    // --- filter -------------------------------------------------------

    /// `all_rows` narrowed by the current query (the sort order is kept).
    fn matching_rows(&self) -> Vec<Row> {
        let query = filter::parse(&self.filter.query);
        if query.is_empty() {
            return self.all_rows.clone();
        }
        self.all_rows
            .iter()
            .filter(|r| query.matches(&r.name.to_string_lossy()).is_some())
            .cloned()
            .collect()
    }

    /// Re-apply the query. While typing the selection jumps to the first
    /// match; `keep_selection` keeps the same row selected when it survives.
    fn refilter(&mut self, keep_selection: bool) {
        let keep = keep_selection
            .then(|| self.selected_row().map(|r| r.path.clone()))
            .flatten();
        self.rows = self.matching_rows();
        self.cursor = keep
            .and_then(|p| self.rows.iter().position(|r| r.path == p))
            .unwrap_or(0);
        self.offset = 0;
        self.ensure_visible();
        self.refresh_detail();
    }

    fn reset_filter(&mut self) {
        self.filter = FilterState::default();
    }

    fn filter_open(&mut self) {
        if self.screen != Screen::Explorer {
            return;
        }
        self.focus = Focus::Main;
        self.filter.editing = true;
        self.filter.cursor = self.filter.query.chars().count();
    }

    fn filter_clear(&mut self) {
        let was_active = self.filter.active();
        self.filter.query.clear();
        self.filter.cursor = 0;
        self.filter.editing = false;
        if was_active {
            self.refilter(true);
        }
    }

    fn filter_byte(&self, chars: usize) -> usize {
        self.filter
            .query
            .char_indices()
            .nth(chars)
            .map_or(self.filter.query.len(), |(b, _)| b)
    }

    fn filter_edit(&mut self, action: Action) {
        if !self.filter.editing {
            return;
        }
        let len = self.filter.query.chars().count();
        let cursor = self.filter.cursor.min(len);
        match action {
            Action::FilterChar(c) if !c.is_control() => {
                let at = self.filter_byte(cursor);
                self.filter.query.insert(at, c);
                self.filter.cursor = cursor + 1;
                self.refilter(false);
            }
            Action::FilterBackspace if cursor > 0 => {
                let at = self.filter_byte(cursor - 1);
                self.filter.query.remove(at);
                self.filter.cursor = cursor - 1;
                self.refilter(false);
            }
            Action::FilterDelete if cursor < len => {
                let at = self.filter_byte(cursor);
                self.filter.query.remove(at);
                self.refilter(false);
            }
            Action::FilterLeft => self.filter.cursor = cursor.saturating_sub(1),
            Action::FilterRight => self.filter.cursor = (cursor + 1).min(len),
            Action::FilterHome => self.filter.cursor = 0,
            Action::FilterEnd => self.filter.cursor = len,
            Action::FilterClearLine if cursor > 0 => {
                let at = self.filter_byte(cursor);
                self.filter.query.drain(..at);
                self.filter.cursor = 0;
                self.refilter(false);
            }
            Action::FilterClearWord if cursor > 0 => {
                let chars: Vec<char> = self.filter.query.chars().collect();
                let mut start = cursor;
                while start > 0 && chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                self.filter.query = chars[..start].iter().chain(&chars[cursor..]).collect();
                self.filter.cursor = start;
                self.refilter(false);
            }
            _ => {}
        }
    }

    pub fn refresh_held(&mut self) {
        self.held = self.holding.list().unwrap_or_default();
        self.held_cursor = self.held_cursor.min(self.held.len().saturating_sub(1));
        self.ensure_visible();
    }

    fn refresh_free(&mut self) {
        if let Ok(free) = volume::free_space(&self.current().path) {
            self.free_now = free;
        }
    }

    pub fn update(&mut self, action: Action) {
        self.apply(action);
        // A new kind of dialog starts its own grow-in animation.
        let kind = self.dialog.as_ref().map(std::mem::discriminant);
        if kind != self.dialog_kind {
            if kind.is_some() {
                self.dialog_since = self.tick;
            }
            self.dialog_kind = kind;
        }
    }

    fn apply(&mut self, action: Action) {
        if !matches!(action, Action::Tick) {
            self.notice = None;
        }
        match action {
            Action::Quit => self.request_quit(),
            Action::Tick => self.on_tick(),
            Action::Cancel => self.cancel(),
            Action::Confirm => self.confirm(),
            Action::ConfirmIrreversible => match self.dialog.take() {
                Some(Dialog::Review(r)) => self.dialog = Some(Dialog::Irreversible(r)),
                other => self.dialog = other,
            },
            Action::ReviewHeld => {
                if matches!(self.dialog, Some(Dialog::QuitWarning(_))) {
                    self.dialog = None;
                    self.goto(Screen::Holding);
                    self.focus = Focus::Main;
                }
            }
            // Everything below is blocked while a dialog is open.
            _ if self.dialog.is_some() => {}
            Action::FilterOpen => self.filter_open(),
            Action::FilterAccept => self.filter.editing = false,
            Action::FilterClear => self.filter_clear(),
            Action::FilterChar(_)
            | Action::FilterBackspace
            | Action::FilterDelete
            | Action::FilterLeft
            | Action::FilterRight
            | Action::FilterHome
            | Action::FilterEnd
            | Action::FilterClearWord
            | Action::FilterClearLine => self.filter_edit(action),
            Action::Up => self.move_by(-1),
            Action::Down => self.move_by(1),
            Action::PageUp => self.move_by(-(self.visible_rows() as i64)),
            Action::PageDown => self.move_by(self.visible_rows() as i64),
            Action::Scroll(n) => self.move_by(n as i64),
            Action::Top => self.move_by(i64::MIN / 2),
            Action::Bottom => self.move_by(i64::MAX / 2),
            Action::Into => self.enter(),
            Action::Out => self.leave(),
            Action::ToggleMark => self.toggle_mark(),
            Action::OpenReview => self.open_review(),
            Action::CycleGraph => {
                self.graph = match self.graph {
                    GraphMode::Bars => GraphMode::Treemap,
                    GraphMode::Treemap => GraphMode::Hidden,
                    GraphMode::Hidden => GraphMode::Bars,
                }
            }
            Action::GrowGraph => self.graph_pct = (self.graph_pct + 5).min(70),
            Action::ShrinkGraph => self.graph_pct = self.graph_pct.saturating_sub(5).max(20),
            Action::CycleSort => {
                self.sort = match self.sort {
                    SortBy::Disk => SortBy::Size,
                    SortBy::Size => SortBy::Name,
                    SortBy::Name => SortBy::Disk,
                };
                self.refresh_rows();
            }
            Action::FocusToggle => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Main,
                    Focus::Main => Focus::Sidebar,
                }
            }
            // Jumping straight to a screen means "work there": focus its list.
            Action::GoTo(screen) => {
                self.goto(screen);
                self.focus = Focus::Main;
            }
            Action::Rescan => self.rescan(false),
            Action::RescanAll => self.rescan(true),
            Action::Click { x, y } => self.click(x, y),
            Action::RestoreSelected => self.restore_selected(),
            Action::DeleteSelectedForever => self.ask_delete_forever(),
            Action::EmptyAll => self.ask_empty_all(),
        }
    }

    /// Quitting while a delete is running would kill it half-way, so wait
    /// for it to finish first.
    fn request_quit(&mut self) {
        if self.job.is_some() {
            self.quit_after_job = true;
            self.notice = Some("Finishing the delete before quitting...".to_string());
        } else if matches!(self.dialog, Some(Dialog::QuitWarning(_))) || self.held.is_empty() {
            // Nothing waiting in ToBeDeleted, or the user asked twice.
            self.should_quit = true;
        } else {
            self.dialog = Some(Dialog::QuitWarning(QuitWarning {
                count: self.held.len(),
                usage: self.held.iter().map(HeldItem::usage).sum(),
            }));
        }
    }

    fn on_tick(&mut self) {
        self.tick += 1;
        self.poll_job();
        if self.animations {
            self.free_shown.step(self.free_now as f64, 0.15);
        } else {
            self.free_shown = Smooth::new(self.free_now as f64);
        }
        if self.tick.is_multiple_of(20) {
            self.refresh_free();
        }
        if self.screen == Screen::Explorer {
            let done = self.current().is_done();
            let busy_refresh = !done && self.tick.is_multiple_of(10);
            if busy_refresh || (done && !self.rows_synced_done) {
                self.refresh_rows();
            }
        }
    }

    fn cancel(&mut self) {
        // A running delete cannot be cancelled.
        if let Some(Dialog::Deleting(p)) = self.dialog.take() {
            self.dialog = Some(Dialog::Deleting(p));
        }
    }

    fn confirm(&mut self) {
        match self.dialog.take() {
            Some(Dialog::QuitWarning(_)) => self.should_quit = true,
            Some(Dialog::Review(r)) => self.run_delete(r, DeleteMode::SoftOnly),
            Some(Dialog::Irreversible(r)) => self.run_delete(r, DeleteMode::SoftThenEmpty),
            Some(Dialog::ConfirmForever(t)) => self.run_forever(t),
            Some(Dialog::Deleting(p)) => self.dialog = Some(Dialog::Deleting(p)),
            Some(Dialog::Finish(_)) | Some(Dialog::Notice(_)) | None => {}
        }
    }

    fn goto(&mut self, screen: Screen) {
        self.filter.editing = false;
        self.screen = screen;
        self.transition_start = self.tick;
        match screen {
            Screen::Holding => self.refresh_held(),
            Screen::Explorer => self.refresh_rows(),
            Screen::Home => {}
        }
        self.refresh_detail();
    }

    fn move_by(&mut self, delta: i64) {
        match (self.focus, self.screen) {
            (Focus::Sidebar, _) => {
                let order = [Screen::Home, Screen::Explorer, Screen::Holding];
                let idx = order.iter().position(|s| *s == self.screen).unwrap_or(0) as i64;
                let next = (idx + delta.signum()).clamp(0, order.len() as i64 - 1) as usize;
                if order[next] != self.screen {
                    self.goto(order[next]);
                }
            }
            (_, Screen::Explorer) => {
                self.cursor = step(self.cursor, delta, self.rows.len());
                self.ensure_visible();
                self.refresh_detail();
            }
            (_, Screen::Holding) => {
                self.held_cursor = step(self.held_cursor, delta, self.held.len());
                self.ensure_visible();
            }
            _ => {}
        }
    }

    /// Rescan the current folder (or everything). Refused while a scan runs.
    fn rescan(&mut self, all: bool) {
        if self.screen == Screen::Holding {
            return;
        }
        if !self.scanner.is_idle() {
            self.notice = Some("A scan is still running. Wait for it to finish.".to_string());
            return;
        }
        let whole = all || self.screen == Screen::Home || self.stack.len() == 1;
        if whole {
            let root_path = self.stack[0].path.clone();
            let Some(root) = self.scanner.rescan_all(&root_path) else {
                return;
            };
            // Going back to the root changes the folder shown, so the filter
            // (which belongs to one folder's list) goes; a plain refresh of the
            // same folder keeps it.
            if self.stack.len() > 1 {
                self.reset_filter();
            }
            self.stack = vec![root];
            self.cursor = 0;
            self.offset = 0;
            self.rows.clear();
            self.pending_select = None;
            self.notice = Some("Rescanning everything...".to_string());
        } else {
            let node = Arc::clone(self.current());
            let Some(fresh) = self.scanner.rescan_dir(&node) else {
                return;
            };
            let name = fresh.name.to_string_lossy().into_owned();
            let last = self.stack.len() - 1;
            self.stack[last] = fresh;
            self.notice = Some(format!("Rescanning {name}..."));
        }
        self.refresh_rows();
    }

    fn enter(&mut self) {
        if self.focus == Focus::Sidebar {
            self.focus = Focus::Main;
            return;
        }
        if self.screen != Screen::Explorer {
            return;
        }
        let Some(node) = self
            .selected_row()
            .filter(|r| r.is_dir)
            .and_then(|r| r.node.clone())
        else {
            return;
        };
        self.stack.push(node);
        self.reset_filter();
        self.cursor = 0;
        self.offset = 0;
        self.rows.clear();
        self.refresh_rows();
    }

    fn leave(&mut self) {
        if self.focus != Focus::Main || self.screen != Screen::Explorer || self.stack.len() <= 1 {
            return;
        }
        if let Some(from) = self.stack.pop() {
            self.pending_select = Some(from.path.clone());
        }
        self.reset_filter();
        self.refresh_rows();
    }

    fn toggle_mark(&mut self) {
        if self.screen != Screen::Explorer {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        let (path, usage) = (row.path.clone(), Usage::new(row.size, row.disk));
        let name = row.name.to_string_lossy().into_owned();
        if self.marks.remove(&path).is_some() {
            return;
        }
        if !self.policy.can_mark(&path) {
            self.notice = Some(format!("{name} is protected and cannot be deleted"));
            return;
        }
        if self.marks.keys().any(|m| path.starts_with(m)) {
            self.notice = Some(format!("{name} is already covered by a marked folder"));
            return;
        }
        self.marks.retain(|m, _| !m.starts_with(&path));
        self.marks.insert(path, usage);
    }

    fn open_review(&mut self) {
        if self.screen != Screen::Explorer {
            return;
        }
        if self.marks.is_empty() {
            self.notice = Some("Nothing marked yet. Press space to mark items.".to_string());
            return;
        }
        let mut items: Vec<ReviewItem> = self
            .marks
            .iter()
            .map(|(path, usage)| ReviewItem {
                path: path.clone(),
                usage: *usage,
                safety: self.policy.classify_real(path),
            })
            .collect();
        items.sort_by(|a, b| {
            b.usage
                .disk
                .cmp(&a.usage.disk)
                .then_with(|| a.path.cmp(&b.path))
        });
        let total = items.iter().map(|i| i.usage).sum();
        let (mut unique, mut shared) = (Usage::default(), Usage::default());
        for item in &items {
            let split = self.breakdown_of(&item.path, item.usage);
            unique += split.unique;
            shared += split.shared;
        }
        self.dialog = Some(Dialog::Review(Review {
            items,
            total,
            unique,
            shared,
        }));
    }

    /// Unique vs shared space for the selected row (explorer only).
    pub fn refresh_detail(&mut self) {
        self.detail = if self.screen == Screen::Explorer {
            self.selected_row().and_then(|row| self.detail_for(row))
        } else {
            None
        };
    }

    fn detail_for(&self, row: &Row) -> Option<Detail> {
        if row.is_dir {
            let node = row.node.as_ref()?;
            Some(Detail {
                path: row.path.clone(),
                breakdown: self.scanner.breakdown(node),
                complete: node.is_done(),
            })
        } else {
            Some(Detail {
                path: row.path.clone(),
                breakdown: file_breakdown(&row.path).ok()?,
                complete: true,
            })
        }
    }

    /// Unique/shared for a marked path; unknown paths count as all unique.
    fn breakdown_of(&self, path: &Path, usage: Usage) -> Breakdown {
        let is_dir = std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir());
        if is_dir {
            if let Some(node) = locate(&self.stack[0], path) {
                return self.scanner.breakdown(&node);
            }
        } else if let Ok(split) = file_breakdown(path) {
            return split;
        }
        Breakdown {
            unique: usage,
            shared: Usage::default(),
        }
    }

    fn click(&mut self, x: u16, y: u16) {
        let l = self.layout;
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if inside(l.sidebar) {
            if y <= l.sidebar.y {
                return;
            }
            let idx = (y - l.sidebar.y - 1) as usize;
            if let Some(entry) = SIDEBAR.get(idx) {
                match entry.screen {
                    Some(screen) => {
                        self.goto(screen);
                        self.focus = Focus::Main;
                    }
                    None => {
                        self.notice = Some(format!("{} is coming in a later release", entry.label));
                    }
                }
            }
        } else if inside(l.main) && y >= l.list_top && y < l.list_top + l.list_rows {
            let i = (y - l.list_top) as usize;
            self.focus = Focus::Main;
            self.filter.editing = false;
            match self.screen {
                Screen::Explorer => {
                    let idx = self.offset + i;
                    if idx < self.rows.len() {
                        if idx == self.cursor {
                            self.enter();
                        } else {
                            self.cursor = idx;
                        }
                        self.refresh_detail();
                    }
                }
                Screen::Holding => {
                    let idx = self.held_offset + i;
                    if idx < self.held.len() {
                        self.held_cursor = idx;
                    }
                }
                Screen::Home => {}
            }
        }
    }

    // --- soft delete -------------------------------------------------

    /// Start the delete on a background thread and show its progress.
    fn run_delete(&mut self, review: Review, mode: DeleteMode) {
        let items: Vec<(PathBuf, Usage)> = review
            .items
            .iter()
            .map(|i| (i.path.clone(), i.usage))
            .collect();
        let phases = if mode == DeleteMode::SoftThenEmpty {
            2
        } else {
            1
        };
        self.job_mode = mode;
        self.job_free_before = self.free_now;
        self.job_failure = None;
        let items_total: Usage = items.iter().map(|(_, u)| *u).sum();
        self.dialog = Some(Dialog::Deleting(DeleteProgress {
            done: 0,
            total: items.len() * phases,
            usage: Usage::default(),
            of: (0..phases).fold(Usage::default(), |acc, _| acc + items_total),
            phase: Phase::Moving,
            current: PathBuf::new(),
        }));
        self.job = Some(DeleteJob::start(
            Arc::clone(&self.holding),
            Arc::clone(&self.policy),
            items,
            mode,
        ));
    }

    /// Drain the running job's events: update the progress dialog and, when
    /// it finishes, record the outcome.
    fn poll_job(&mut self) {
        let events = self.job.as_ref().map(DeleteJob::poll).unwrap_or_default();
        let mut finished = None;
        for event in events {
            match event {
                DeleteEvent::Progress {
                    done,
                    total,
                    usage,
                    of,
                    phase,
                    current,
                } => {
                    if let Some(Dialog::Deleting(p)) = &mut self.dialog {
                        *p = DeleteProgress {
                            done,
                            total,
                            usage,
                            of,
                            phase,
                            current,
                        };
                    }
                }
                DeleteEvent::Failed { error, .. } => {
                    if self.job_failure.is_none() {
                        self.job_failure = Some(error);
                    }
                }
                DeleteEvent::Finished {
                    moved,
                    emptied,
                    failures,
                } => finished = Some((moved, emptied, failures)),
            }
        }
        if let Some((moved, emptied, failures)) = finished {
            self.job = None;
            let (mode, free_before) = (self.job_mode, self.job_free_before);
            self.after_delete(mode, moved, emptied, failures, free_before);
            if self.quit_after_job {
                self.quit_after_job = false;
                self.request_quit();
            }
        }
    }

    /// Record the outcome of a delete and show the right dialog.
    pub(crate) fn after_delete(
        &mut self,
        mode: DeleteMode,
        moved: Usage,
        emptied: Usage,
        failures: usize,
        free_before: u64,
    ) {
        if moved != Usage::default() {
            self.accounting.record_soft_delete(moved);
        }
        if emptied != Usage::default() {
            self.accounting.record_emptied(emptied);
        }
        // Marks of items that no longer exist were moved; failed ones stay.
        self.marks
            .retain(|p, _| std::fs::symlink_metadata(p).is_ok());
        self.refresh_free();
        self.refresh_held();
        self.refresh_rows();
        let reason = self.job_failure.take();
        self.dialog = Some(match mode {
            DeleteMode::SoftThenEmpty | DeleteMode::EmptyHeld => Dialog::Finish(FinishInfo {
                moved,
                emptied,
                free_before,
                free_after: self.free_now,
                failures,
                first_failure: reason,
                started_tick: self.tick,
            }),
            DeleteMode::SoftOnly => {
                let mut msg = format!(
                    "Moved {} to ToBeDeleted (pending, not freed yet)",
                    both_usage(moved)
                );
                if failures > 0 {
                    msg.push_str(&format!(". {failures} item(s) could not be moved"));
                    if let Some(reason) = reason {
                        msg.push_str(&format!(" ({reason})"));
                    }
                }
                Dialog::Notice(msg)
            }
        });
    }

    fn restore_selected(&mut self) {
        if self.screen != Screen::Holding {
            return;
        }
        let Some(item) = self.held.get(self.held_cursor).cloned() else {
            return;
        };
        match self.holding.restore(&item.id) {
            Ok(_) => {
                self.accounting.record_restore(item.usage());
                self.notice = Some(format!("Restored {}", item.held_name));
            }
            Err(e) => self.notice = Some(e.to_string()),
        }
        self.refresh_held();
        self.refresh_rows();
    }

    fn ask_delete_forever(&mut self) {
        if self.screen != Screen::Holding {
            return;
        }
        match self.held.get(self.held_cursor) {
            Some(item) => {
                self.dialog = Some(Dialog::ConfirmForever(ForeverTarget::Item {
                    id: item.id.clone(),
                    name: item.held_name.clone(),
                    usage: item.usage(),
                }));
            }
            None => self.notice = Some("ToBeDeleted is empty".to_string()),
        }
    }

    fn ask_empty_all(&mut self) {
        if self.screen != Screen::Holding {
            return;
        }
        if self.held.is_empty() {
            self.notice = Some("ToBeDeleted is empty".to_string());
            return;
        }
        self.dialog = Some(Dialog::ConfirmForever(ForeverTarget::All {
            count: self.held.len(),
            usage: self.held.iter().map(HeldItem::usage).sum(),
        }));
    }

    /// Permanently delete held items on a background thread, with progress.
    fn run_forever(&mut self, target: ForeverTarget) {
        let items: Vec<HeldItem> = match &target {
            ForeverTarget::Item { id, .. } => {
                self.held.iter().filter(|i| &i.id == id).cloned().collect()
            }
            ForeverTarget::All { .. } => self.held.clone(),
        };
        if items.is_empty() {
            self.refresh_held();
            return;
        }
        self.job_mode = DeleteMode::EmptyHeld;
        self.job_free_before = self.free_now;
        self.job_failure = None;
        self.dialog = Some(Dialog::Deleting(DeleteProgress {
            done: 0,
            total: items.len(),
            usage: Usage::default(),
            of: items.iter().map(HeldItem::usage).sum(),
            phase: Phase::Deleting,
            current: PathBuf::new(),
        }));
        self.job = Some(DeleteJob::empty_held(Arc::clone(&self.holding), items));
    }
}

/// Move `pos` by `delta` within `0..len` (len 0 stays 0), without overflow.
fn step(pos: usize, delta: i64, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let target = (pos as i128 + delta as i128).clamp(0, len as i128 - 1);
    target as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{env, names, select, settle};
    use crate::usage::Usage;
    use ratatui::layout::Rect;
    use std::fs;

    // Multiples of the 4096-byte allocation unit, so size and on-disk size agree.
    const FILES: &[(&str, usize)] = &[("a/f", 12288), ("b.bin", 8192), ("c/f", 4096)];

    #[test]
    fn down_up_clamp() {
        let mut e = env(FILES);
        let app = &mut e.app;
        assert_eq!(names(app), ["a", "b.bin", "c"]);
        app.update(Action::Up);
        assert_eq!(app.cursor, 0);
        for _ in 0..5 {
            app.update(Action::Down);
        }
        assert_eq!(app.cursor, 2);
        app.update(Action::Top);
        assert_eq!(app.cursor, 0);
        app.update(Action::Bottom);
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn into_and_out_navigate() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::Into);
        assert_eq!(app.stack.len(), 2);
        assert_eq!(names(app), ["f"]);
        app.update(Action::Out);
        assert_eq!(app.stack.len(), 1);
        assert_eq!(names(app).len(), 3);
        assert_eq!(app.selected_row().unwrap().name, "a");
        app.update(Action::Out);
        assert_eq!(app.stack.len(), 1, "never leaves the scan root");
    }

    #[test]
    fn into_a_file_does_nothing() {
        let mut e = env(FILES);
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::Into);
        assert_eq!(app.stack.len(), 1);
    }

    #[test]
    fn mark_toggle_and_total() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::ToggleMark);
        assert_eq!(app.marked().size, 12288);
        app.update(Action::Down);
        app.update(Action::ToggleMark);
        assert_eq!(app.marked().size, 20480);
        assert_eq!(app.marks.len(), 2);
        app.update(Action::ToggleMark);
        assert_eq!(app.marked().size, 12288);
        assert_eq!(app.marks.len(), 1);
    }

    #[test]
    fn nested_marks_are_deduped() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        // Mark the child first.
        app.update(Action::Into);
        app.update(Action::ToggleMark);
        assert!(app.marks.contains_key(&home.join("a/f")));
        // Marking the parent replaces the child mark.
        app.update(Action::Out);
        app.update(Action::ToggleMark);
        assert_eq!(app.marks.len(), 1);
        assert!(app.marks.contains_key(&home.join("a")));
        // Marking a child of a marked parent is ignored.
        app.update(Action::Into);
        app.update(Action::ToggleMark);
        assert_eq!(app.marks.len(), 1);
        assert!(app.marks.contains_key(&home.join("a")));
        assert_eq!(app.marked().size, 12288);
    }

    #[test]
    fn protected_cannot_be_marked() {
        let mut e = env(&[("Documents/x", 100), ("other/y", 50)]);
        let app = &mut e.app;
        select(app, "Documents");
        app.update(Action::ToggleMark);
        assert!(app.marks.is_empty());
        assert!(app.notice.is_some());
    }

    #[test]
    fn open_review_requires_marks_and_sorts_by_size() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::OpenReview);
        assert!(app.dialog.is_none());
        assert!(app.notice.is_some());

        select(app, "b.bin");
        app.update(Action::ToggleMark);
        select(app, "a");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!(r.total.size, 20480);
                let sizes: Vec<u64> = r.items.iter().map(|i| i.usage.size).collect();
                assert_eq!(sizes, [12288, 8192]);
            }
            other => panic!("expected review, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn review_confirm_moves_files_to_holding() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "a");
        app.update(Action::ToggleMark);
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        settle(app);
        assert!(!home.join("a").exists());
        assert!(!home.join("b.bin").exists());
        assert!(home.join("c/f").exists());
        assert_eq!(app.held.len(), 2);
        assert_eq!(app.accounting.pending().size, 20480);
        assert!(app.marks.is_empty());
        assert!(matches!(app.dialog, Some(Dialog::Notice(_))));
        assert_eq!(names(app), ["c"]);
    }

    #[test]
    fn irreversible_flow_requires_second_confirmation() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::ConfirmIrreversible);
        assert!(matches!(app.dialog, Some(Dialog::Irreversible(_))));
        assert!(home.join("b.bin").exists(), "nothing deleted yet");
        app.update(Action::Confirm);
        settle(app);
        assert!(!home.join("b.bin").exists());
        assert!(app.held.is_empty());
        assert_eq!(app.accounting.pending().size, 0);
        assert!(matches!(app.dialog, Some(Dialog::Finish(_))));
    }

    #[test]
    fn confirm_irreversible_only_from_review() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::ConfirmIrreversible);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn cancel_closes_dialog_and_keeps_files() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Cancel);
        assert!(app.dialog.is_none());
        assert!(home.join("b.bin").exists());
        assert_eq!(app.marks.len(), 1, "marks survive a cancelled review");
    }

    #[test]
    fn dialogs_swallow_navigation() {
        let mut e = env(FILES);
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        let before = app.cursor;
        app.update(Action::Down);
        app.update(Action::ToggleMark);
        assert_eq!(app.cursor, before);
        assert_eq!(app.marks.len(), 1);
    }

    #[test]
    fn graph_cycle_and_resize_bounds() {
        let mut e = env(FILES);
        let app = &mut e.app;
        assert_eq!(app.graph, GraphMode::Bars);
        assert_eq!(app.graph_pct, 35);
        app.update(Action::CycleGraph);
        assert_eq!(app.graph, GraphMode::Treemap);
        app.update(Action::CycleGraph);
        assert_eq!(app.graph, GraphMode::Hidden);
        app.update(Action::CycleGraph);
        assert_eq!(app.graph, GraphMode::Bars);
        for _ in 0..30 {
            app.update(Action::GrowGraph);
        }
        assert_eq!(app.graph_pct, 70);
        for _ in 0..30 {
            app.update(Action::ShrinkGraph);
        }
        assert_eq!(app.graph_pct, 20);
    }

    /// A sparse file: big logical size, (almost) no space on disk.
    fn write_sparse(path: &std::path::Path, len: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = fs::File::create(path).unwrap();
        f.set_len(len).unwrap();
    }

    fn sort_env() -> crate::testutil::Env {
        // real.bin: 8192 on disk; a.bin: 4096; sparse.bin: 1 MB logical, ~0 on disk.
        let e = env(&[("real.bin", 8192), ("a.bin", 4096)]);
        write_sparse(&e.home.join("sparse.bin"), 1_000_000);
        let mut e = e;
        e.app.refresh_rows();
        e
    }

    #[test]
    fn default_sort_is_on_disk_size() {
        let e = sort_env();
        assert_eq!(e.app.sort, SortBy::Disk);
        assert_eq!(names(&e.app), ["real.bin", "a.bin", "sparse.bin"]);
    }

    #[test]
    fn sort_cycles_disk_size_name() {
        let mut e = sort_env();
        let app = &mut e.app;
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Size);
        assert_eq!(names(app), ["sparse.bin", "real.bin", "a.bin"]);
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Name);
        assert_eq!(names(app), ["a.bin", "real.bin", "sparse.bin"]);
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Disk);
        assert_eq!(names(app), ["real.bin", "a.bin", "sparse.bin"]);
    }

    #[test]
    fn cursor_follows_same_path_after_sort() {
        let mut e = sort_env();
        let app = &mut e.app;
        select(app, "a.bin");
        app.update(Action::CycleSort);
        assert_eq!(app.selected_row().unwrap().name, "a.bin");
    }

    #[test]
    fn weight_follows_the_sort_metric() {
        let mut e = sort_env();
        let app = &mut e.app;
        let sparse = app
            .rows
            .iter()
            .find(|r| r.name == "sparse.bin")
            .unwrap()
            .clone();
        assert!(sparse.size > sparse.disk);
        assert_eq!(app.weight(&sparse), sparse.disk);
        app.update(Action::CycleSort);
        assert_eq!(app.weight(&sparse), sparse.size);
        assert_eq!(app.weight_total(), app.current().size());
    }

    fn set_layout(app: &mut App) {
        app.layout = UiLayout {
            sidebar: Rect::new(0, 0, 16, 20),
            main: Rect::new(16, 0, 80, 20),
            list_top: 3,
            list_rows: 10,
        };
    }

    #[test]
    fn click_selects_then_enters() {
        let mut e = env(FILES);
        let app = &mut e.app;
        set_layout(app);
        app.update(Action::Click { x: 30, y: 4 }); // row index 1 (b.bin)
        assert_eq!(app.cursor, 1);
        app.update(Action::Click { x: 30, y: 3 }); // row index 0 (a)
        assert_eq!(app.cursor, 0);
        assert_eq!(app.stack.len(), 1);
        app.update(Action::Click { x: 30, y: 3 }); // again: enter
        assert_eq!(app.stack.len(), 2);
    }

    #[test]
    fn click_below_rows_does_nothing() {
        let mut e = env(FILES);
        let app = &mut e.app;
        set_layout(app);
        app.update(Action::Click { x: 30, y: 15 });
        assert_eq!(app.cursor, 0);
        assert_eq!(app.stack.len(), 1);
    }

    #[test]
    fn click_sidebar_switches_screen() {
        let mut e = env(FILES);
        let app = &mut e.app;
        set_layout(app);
        app.update(Action::Click { x: 2, y: 1 }); // first entry: Home
        assert_eq!(app.screen, Screen::Home);
        app.update(Action::Click { x: 2, y: 2 }); // Explorer
        assert_eq!(app.screen, Screen::Explorer);
        app.update(Action::Click { x: 2, y: 3 }); // Leftovers: not built yet
        assert_eq!(app.screen, Screen::Explorer);
        assert!(app.notice.is_some());
    }

    #[test]
    fn scroll_moves_cursor() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::Scroll(1));
        assert_eq!(app.cursor, 1);
        app.update(Action::Scroll(-5));
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn sidebar_focus_changes_screen_with_up_down() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::FocusToggle);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Up);
        assert_eq!(app.screen, Screen::Home);
        app.update(Action::Down);
        assert_eq!(app.screen, Screen::Explorer);
        app.update(Action::Down);
        assert_eq!(app.screen, Screen::Holding);
        app.update(Action::Into);
        assert_eq!(app.focus, Focus::Main);
    }

    fn hold_two(app: &mut App) {
        select(app, "a");
        app.update(Action::ToggleMark);
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        settle(app);
        app.update(Action::Cancel); // dismiss the notice
    }

    #[test]
    fn holding_screen_restore_and_delete() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::GoTo(Screen::Holding));
        assert_eq!(app.held.len(), 2);
        let first = app.held[app.held_cursor].clone();

        app.update(Action::RestoreSelected);
        assert!(first.original.exists());
        assert_eq!(app.held.len(), 1);
        assert_eq!(app.accounting.pending().size, 20480 - first.size);

        // Deleting forever asks for confirmation first.
        let payload_exists = app.held.len() == 1;
        assert!(payload_exists);
        app.update(Action::DeleteSelectedForever);
        assert!(matches!(app.dialog, Some(Dialog::ConfirmForever(_))));
        assert_eq!(app.held.len(), 1, "nothing deleted before confirmation");
        app.update(Action::Confirm);
        settle(app);
        assert!(app.held.is_empty());
        assert_eq!(app.accounting.pending().size, 0);
        assert!(home.join("c/f").exists());
    }

    #[test]
    fn holding_empty_all_needs_confirmation() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::GoTo(Screen::Holding));
        app.update(Action::EmptyAll);
        assert!(matches!(app.dialog, Some(Dialog::ConfirmForever(_))));
        app.update(Action::Cancel);
        assert_eq!(app.held.len(), 2);
        app.update(Action::EmptyAll);
        app.update(Action::Confirm);
        settle(app);
        assert!(app.held.is_empty());
        assert_eq!(app.accounting.pending().size, 0);
    }

    #[test]
    fn restore_conflict_sets_notice() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        settle(app);
        app.update(Action::Cancel);
        fs::write(home.join("b.bin"), "new").unwrap();
        app.update(Action::GoTo(Screen::Holding));
        app.update(Action::RestoreSelected);
        assert!(app.notice.is_some());
        assert_eq!(app.held.len(), 1);
    }

    #[test]
    fn holding_actions_ignored_outside_holding_screen() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::EmptyAll);
        app.update(Action::RestoreSelected);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn quit_sets_flag() {
        let mut e = env(FILES);
        e.app.update(Action::Quit);
        assert!(e.app.should_quit);
    }

    #[test]
    fn confirm_starts_background_job_and_ticks_finish_it() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        assert!(matches!(app.dialog, Some(Dialog::Deleting(_))));
        settle(app);
        assert!(matches!(app.dialog, Some(Dialog::Notice(_))));
        assert!(!home.join("b.bin").exists());
        assert_eq!(app.accounting.pending().size, 8192);
        assert_eq!(app.held.len(), 1);
    }

    #[test]
    fn deleting_dialog_cannot_be_dismissed() {
        let mut e = env(FILES);
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        app.update(Action::Cancel);
        app.update(Action::Confirm);
        assert!(matches!(app.dialog, Some(Dialog::Deleting(_))));
        settle(app);
    }

    #[test]
    fn finish_dialog_reports_amounts_for_irreversible_delete() {
        let mut e = env(FILES);
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::ConfirmIrreversible);
        app.update(Action::Confirm);
        settle(app);
        match &app.dialog {
            Some(Dialog::Finish(f)) => {
                assert_eq!(f.moved.size, 8192);
                assert_eq!(f.emptied.size, 8192);
                assert_eq!(f.failures, 0);
            }
            other => panic!("expected finish, got {}", other.is_some()),
        }
    }

    #[test]
    fn quit_waits_for_running_delete() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        assert!(matches!(app.dialog, Some(Dialog::Deleting(_))));
        app.update(Action::Quit);
        assert!(!app.should_quit, "must not quit while deleting");
        assert!(app.notice.is_some());
        settle(app);
        assert!(!home.join("b.bin").exists());
        assert_eq!(app.held.len(), 1, "the moved item is tracked");
        // The quit was deferred, and now that something sits in ToBeDeleted it
        // asks first instead of quitting behind the user's back.
        assert!(!app.should_quit);
        assert!(matches!(app.dialog, Some(Dialog::QuitWarning(_))));
        app.update(Action::Confirm);
        assert!(app.should_quit);
    }

    #[test]
    fn folder_with_protected_inside_fails_with_a_reason() {
        let mut e = env(&[(".config/gcloud/creds", 100), ("x/f", 50)]);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, ".config");
        app.update(Action::ToggleMark);
        assert_eq!(app.marks.len(), 1, "the folder itself is only Caution");
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        settle(app);
        assert!(home.join(".config/gcloud/creds").exists());
        match &app.dialog {
            Some(Dialog::Notice(msg)) => {
                assert!(msg.contains("could not be moved"), "{msg}");
                assert!(msg.contains("protected"), "{msg}");
            }
            other => panic!("expected notice, got {}", other.is_some()),
        }
        assert_eq!(app.marks.len(), 1, "failed items stay marked");
    }

    #[test]
    fn too_small_flag_defaults_to_false() {
        let e = env(FILES);
        assert!(!e.app.too_small);
    }

    #[test]
    fn marks_and_review_carry_on_disk_size() {
        let mut e = env(&[("a/f", 5000), ("b/f", 100)]);
        let app = &mut e.app;
        select(app, "a");
        let row = app.selected_row().unwrap().clone();
        assert!(row.disk >= row.size, "allocation rounds up");
        app.update(Action::ToggleMark);
        assert_eq!(app.marked(), Usage::new(row.size, row.disk));
        app.update(Action::OpenReview);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!(r.total, Usage::new(row.size, row.disk));
                assert_eq!(r.items[0].usage, Usage::new(row.size, row.disk));
            }
            other => panic!("expected review, got {}", other.is_some()),
        }
    }

    #[test]
    fn pending_and_held_items_track_on_disk_size() {
        let mut e = env(&[("a/f", 5000), ("b/f", 100)]);
        let app = &mut e.app;
        select(app, "a");
        let row = app.selected_row().unwrap().clone();
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        settle(app);
        assert_eq!(app.accounting.pending(), Usage::new(row.size, row.disk));
        assert_eq!(app.held[0].usage(), Usage::new(row.size, row.disk));
        assert!(app.held[0].disk >= app.held[0].size);
    }

    #[test]
    fn finish_info_has_on_disk_amounts() {
        let mut e = env(&[("a/f", 5000), ("b/f", 100)]);
        let app = &mut e.app;
        select(app, "a");
        let row = app.selected_row().unwrap().clone();
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::ConfirmIrreversible);
        app.update(Action::Confirm);
        settle(app);
        match &app.dialog {
            Some(Dialog::Finish(f)) => {
                assert_eq!(f.moved, Usage::new(row.size, row.disk));
                assert_eq!(f.emptied, Usage::new(row.size, row.disk));
            }
            other => panic!("expected finish, got {}", other.is_some()),
        }
    }

    #[test]
    fn rescan_folder_picks_up_new_files() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        app.update(Action::Into); // a/
        assert_eq!(names(app), ["f"]);
        crate::testutil::write(&home.join("a/new"), 20480);
        app.update(Action::Rescan);
        assert!(app.notice.is_some());
        crate::testutil::wait_scan(app);
        assert_eq!(app.stack.len(), 2, "still inside the folder");
        assert_eq!(names(app), ["new", "f"]);
        assert_eq!(app.current().size(), 12288 + 20480);
        assert_eq!(app.stack[0].size(), 12288 + 8192 + 4096 + 20480);
        assert!(app.current().is_done());
    }

    #[test]
    fn rescan_all_returns_to_the_root_and_picks_up_changes() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        app.update(Action::Into);
        crate::testutil::write(&home.join("d/g"), 16384);
        app.update(Action::RescanAll);
        crate::testutil::wait_scan(app);
        assert_eq!(app.stack.len(), 1);
        assert!(names(app).contains(&"d".to_string()), "{:?}", names(app));
        assert_eq!(app.current().size(), 12288 + 8192 + 4096 + 16384);
    }

    #[test]
    fn rescan_at_the_root_rescans_everything() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        crate::testutil::write(&home.join("d/g"), 16384);
        app.update(Action::Rescan);
        crate::testutil::wait_scan(app);
        assert!(names(app).contains(&"d".to_string()));
    }

    #[test]
    fn rescan_is_refused_while_a_scan_runs() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::Into);
        let before = std::sync::Arc::clone(app.current());
        app.scanner.set_busy(true);
        app.update(Action::Rescan);
        app.update(Action::RescanAll);
        app.scanner.set_busy(false);
        assert!(
            app.notice
                .as_deref()
                .unwrap_or("")
                .contains("still running")
        );
        assert_eq!(app.stack.len(), 2);
        assert!(std::sync::Arc::ptr_eq(app.current(), &before));
    }

    #[test]
    fn rescan_does_nothing_on_the_holding_screen() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::GoTo(Screen::Holding));
        let root = std::sync::Arc::clone(&app.stack[0]);
        app.update(Action::Rescan);
        app.update(Action::RescanAll);
        assert!(std::sync::Arc::ptr_eq(&app.stack[0], &root));
    }

    #[test]
    fn rescan_keeps_the_cursor_and_marks() {
        let mut e = env(FILES);
        let home = e.home.clone();
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::Rescan);
        crate::testutil::wait_scan(app);
        assert!(app.marks.contains_key(&home.join("b.bin")));
        assert_eq!(app.selected_row().unwrap().name, "b.bin");
    }

    fn shared_env() -> crate::testutil::Env {
        // store/pkg (8192) is hard-linked into proj/pkg; proj/own is plain (4096).
        crate::testutil::env_with(&[("store/pkg", 8192), ("proj/own", 4096)], |home| {
            fs::hard_link(home.join("store/pkg"), home.join("proj/pkg")).unwrap()
        })
    }

    #[test]
    fn detail_splits_unique_and_shared_for_the_selected_folder() {
        let mut e = shared_env();
        let app = &mut e.app;
        select(app, "proj");
        let d = app.detail.as_ref().expect("a detail for the selected row");
        assert!(d.path.ends_with("proj"));
        assert!(d.complete);
        assert_eq!(
            (d.breakdown.unique.size, d.breakdown.shared.size),
            (4096, 8192)
        );
        select(app, "store");
        let d = app.detail.as_ref().unwrap();
        assert_eq!(
            (d.breakdown.unique.size, d.breakdown.shared.size),
            (0, 8192)
        );
    }

    #[test]
    fn detail_follows_the_cursor_and_clears_off_screen() {
        let mut e = shared_env();
        let app = &mut e.app;
        let first = app.detail.as_ref().map(|d| d.path.clone());
        assert!(first.is_some(), "explorer opens with a detail");
        app.update(Action::Down);
        let second = app.detail.as_ref().map(|d| d.path.clone());
        assert_ne!(first, second);
        app.update(Action::GoTo(Screen::Home));
        // Detail is only meaningful in the explorer, but never stale-wrong.
        assert!(app.detail.is_none() || app.screen == Screen::Home);
    }

    #[test]
    fn detail_for_a_hard_linked_file_is_all_shared() {
        let mut e = shared_env();
        let app = &mut e.app;
        app.update(Action::GoTo(Screen::Explorer));
        select(app, "proj");
        app.update(Action::Into);
        select(app, "pkg");
        let d = app.detail.as_ref().unwrap();
        assert_eq!(
            (d.breakdown.unique.size, d.breakdown.shared.size),
            (0, 8192)
        );
        select(app, "own");
        let d = app.detail.as_ref().unwrap();
        assert_eq!(
            (d.breakdown.unique.size, d.breakdown.shared.size),
            (4096, 0)
        );
    }

    #[test]
    fn review_splits_what_deleting_frees_from_what_is_shared() {
        let mut e = shared_env();
        let app = &mut e.app;
        select(app, "proj");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!((r.unique.size, r.shared.size), (4096, 8192));
            }
            other => panic!("expected review, got {}", other.is_some()),
        }
    }

    #[test]
    fn review_of_a_plain_folder_has_nothing_shared() {
        let mut e = env(FILES);
        let app = &mut e.app;
        select(app, "a");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!(r.shared, Usage::default());
                assert_eq!(r.unique.size, 12288);
            }
            other => panic!("expected review, got {}", other.is_some()),
        }
    }

    fn fresh_app() -> App {
        // Same setup as `env`, but without navigating anywhere.
        use crate::accounting::Accounting;
        use crate::glyphs::{GlyphMode, Glyphs};
        use crate::safety::Policy;
        use crate::scan::Scanner;
        use crate::softdelete::Holding;
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let scanner = Scanner::new(2);
        let root = scanner.scan_root(&home);
        let hold = base.join("hold");
        let policy = Policy::new(&home, &hold);
        let holding = Holding::open(hold).unwrap();
        let app = App::new(
            scanner,
            root,
            policy,
            holding,
            Accounting::new(0),
            Glyphs::new(GlyphMode::Nerd),
            false,
        );
        std::mem::forget(dir); // keep the sandbox alive for the app's lifetime
        app
    }

    #[test]
    fn starts_on_home_with_the_menu_focused() {
        let app = fresh_app();
        assert_eq!(app.screen, Screen::Home);
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn menu_keys_work_straight_away() {
        let mut app = fresh_app();
        app.update(Action::Down); // choose Explorer without pressing tab
        assert_eq!(app.screen, Screen::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        app.update(Action::Into); // open it: focus moves to the list
        assert_eq!(app.focus, Focus::Main);
    }

    #[test]
    fn number_keys_jump_and_focus_the_screen() {
        let mut app = fresh_app();
        app.update(Action::GoTo(Screen::Explorer));
        assert_eq!(app.screen, Screen::Explorer);
        assert_eq!(app.focus, Focus::Main, "ready to move through rows");
        app.update(Action::FocusToggle);
        app.update(Action::GoTo(Screen::Holding));
        assert_eq!(app.focus, Focus::Main);
    }

    #[test]
    fn emptying_runs_in_the_background_with_a_progress_dialog() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::GoTo(Screen::Holding));
        app.update(Action::EmptyAll);
        app.update(Action::Confirm);
        match &app.dialog {
            Some(Dialog::Deleting(p)) => {
                assert_eq!(p.phase, crate::jobs::Phase::Deleting);
                assert_eq!(p.total, 2);
                assert_eq!(p.of.size, 12288 + 8192);
            }
            other => panic!("expected a progress dialog, got {}", other.is_some()),
        }
        assert_eq!(app.held.len(), 2, "still held until the job reports back");
        settle(app);
        assert!(app.held.is_empty());
        match &app.dialog {
            Some(Dialog::Finish(f)) => assert_eq!(f.emptied.size, 12288 + 8192),
            other => panic!("expected finish, got {}", other.is_some()),
        }
        assert_eq!(app.accounting.pending().size, 0);
    }

    #[test]
    fn deleting_one_item_for_good_runs_in_the_background_too() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::GoTo(Screen::Holding));
        let first = app.held[app.held_cursor].clone();
        app.update(Action::DeleteSelectedForever);
        app.update(Action::Confirm);
        assert!(matches!(app.dialog, Some(Dialog::Deleting(_))));
        settle(app);
        assert_eq!(app.held.len(), 1);
        assert!(app.held.iter().all(|i| i.id != first.id));
        assert_eq!(app.accounting.pending().size, 20480 - first.size);
    }

    #[test]
    fn quit_waits_for_an_empty_job_too() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::GoTo(Screen::Holding));
        app.update(Action::EmptyAll);
        app.update(Action::Confirm);
        app.update(Action::Quit);
        assert!(!app.should_quit);
        settle(app);
        assert!(app.should_quit);
        assert!(app.held.is_empty(), "the delete finished before quitting");
    }

    #[test]
    fn review_delete_starts_in_the_moving_phase() {
        let mut e = env(FILES);
        let app = &mut e.app;
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::Confirm);
        match &app.dialog {
            Some(Dialog::Deleting(p)) => {
                assert_eq!(p.phase, crate::jobs::Phase::Moving);
                assert_eq!(p.of.size, 8192);
            }
            other => panic!("expected a progress dialog, got {}", other.is_some()),
        }
        settle(app);
    }

    #[test]
    fn pending_starts_from_what_is_already_in_tobedeleted() {
        use crate::accounting::Accounting;
        use crate::glyphs::{GlyphMode, Glyphs};
        use crate::safety::Policy;
        use crate::scan::Scanner;
        use crate::softdelete::Holding;
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("home");
        let hold = base.join("hold");
        crate::testutil::write(&home.join("old.bin"), 5000);
        let policy = Policy::new(&home, &hold);
        // A previous session moved a file into ToBeDeleted.
        let first = Holding::open(hold.clone()).unwrap();
        first
            .move_in(&policy, &home.join("old.bin"), Usage::new(5000, 8192))
            .unwrap();
        drop(first);
        // A new session starts.
        let scanner = Scanner::new(2);
        let root = scanner.scan_root(&home);
        let app = App::new(
            scanner,
            root,
            policy,
            Holding::open(hold).unwrap(),
            Accounting::new(0),
            Glyphs::new(GlyphMode::Nerd),
            false,
        );
        assert_eq!(app.held.len(), 1);
        assert_eq!(app.accounting.pending(), Usage::new(5000, 8192));
        assert!(app.accounting.events().is_empty(), "not a session event");
    }

    #[test]
    fn dialog_clock_restarts_for_each_new_dialog() {
        let mut e = env(FILES);
        let app = &mut e.app;
        for _ in 0..5 {
            app.update(Action::Tick);
        }
        select(app, "b.bin");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        assert_eq!(app.dialog_since, app.tick, "review just opened");
        for _ in 0..4 {
            app.update(Action::Tick);
        }
        assert_ne!(app.dialog_since, app.tick);
        let review_since = app.dialog_since;
        app.update(Action::ConfirmIrreversible);
        assert!(
            app.dialog_since > review_since,
            "a different dialog animates again"
        );
        let since = app.dialog_since;
        app.update(Action::Tick);
        assert_eq!(app.dialog_since, since, "ticks alone do not restart it");
    }

    fn filter_env() -> crate::testutil::Env {
        // By on-disk size: node_modules, src, notes.txt, Makefile.
        env(&[
            ("node_modules/x", 20480),
            ("notes.txt", 8192),
            ("src/main.rs", 12288),
            ("Makefile", 4096),
        ])
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.update(Action::FilterChar(c));
        }
    }

    #[test]
    fn slash_opens_the_filter_and_typing_filters_the_list() {
        let mut e = filter_env();
        let app = &mut e.app;
        assert_eq!(names(app), ["node_modules", "src", "notes.txt", "Makefile"]);
        app.update(Action::FilterOpen);
        assert!(app.filter.editing);
        assert_eq!(app.focus, Focus::Main);
        type_text(app, "no");
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
        assert_eq!(app.all_rows.len(), 4, "the full listing is kept");
        assert_eq!((app.filter.query.as_str(), app.filter.cursor), ("no", 2));
    }

    #[test]
    fn enter_accepts_and_the_list_keeps_the_filter() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::FilterAccept);
        assert!(!app.filter.editing);
        assert!(app.filter.active());
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
        app.update(Action::Down);
        assert_eq!(app.selected_row().unwrap().name, "notes.txt");
        app.update(Action::ToggleMark);
        assert!(app.marks.keys().any(|p| p.ends_with("notes.txt")));
    }

    #[test]
    fn slash_again_edits_the_filter_and_refines_it() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::FilterAccept);
        app.update(Action::FilterOpen);
        assert!(app.filter.editing);
        assert_eq!((app.filter.query.as_str(), app.filter.cursor), ("no", 2));
        type_text(app, "d");
        assert_eq!(app.filter.query, "nod");
        assert_eq!(names(app), ["node_modules"]);
        app.update(Action::FilterBackspace);
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
    }

    #[test]
    fn clearing_the_filter_restores_the_list_and_keeps_the_selected_row() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::FilterAccept);
        app.update(Action::Down);
        app.update(Action::FilterClear);
        assert!(!app.filter.active() && !app.filter.editing);
        assert_eq!(names(app).len(), 4);
        assert_eq!(app.selected_row().unwrap().name, "notes.txt");
    }

    #[test]
    fn clearing_while_the_box_is_empty_just_closes_it() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        app.update(Action::FilterClear);
        assert!(!app.filter.editing);
        assert_eq!(names(app).len(), 4);
    }

    #[test]
    fn typing_selects_the_first_match() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "e");
        app.update(Action::Down);
        app.update(Action::Down);
        assert!(app.cursor > 0);
        type_text(app, "s");
        assert_eq!(app.cursor, 0);
        assert_eq!(
            app.selected_row().unwrap().name.to_string_lossy(),
            names(app)[0]
        );
    }

    #[test]
    fn arrows_move_through_the_filtered_list_while_editing() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::Down);
        assert_eq!(app.selected_row().unwrap().name, "notes.txt");
        app.update(Action::Down);
        assert_eq!(app.cursor, 1, "clamped to the last match");
        app.update(Action::Up);
        assert_eq!(app.selected_row().unwrap().name, "node_modules");
    }

    #[test]
    fn going_into_or_out_of_a_folder_clears_the_filter() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "node");
        app.update(Action::FilterAccept);
        app.update(Action::Into);
        assert!(!app.filter.active());
        assert_eq!(names(app), ["x"]);
        app.update(Action::Out);
        assert!(!app.filter.active());
        assert_eq!(names(app).len(), 4);
        assert_eq!(app.selected_row().unwrap().name, "node_modules");
    }

    #[test]
    fn no_matches_gives_an_empty_list_nothing_trips_on() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "zzz");
        assert!(app.rows.is_empty());
        assert!(app.selected_row().is_none());
        assert!(app.detail.is_none());
        for a in [
            Action::Down,
            Action::Up,
            Action::Top,
            Action::Bottom,
            Action::PageDown,
            Action::PageUp,
            Action::ToggleMark,
            Action::Into,
            Action::OpenReview,
            Action::CycleSort,
            Action::CycleGraph,
        ] {
            app.update(a);
        }
        assert!(app.marks.is_empty());
        for _ in 0..3 {
            app.update(Action::FilterBackspace);
        }
        assert_eq!(names(app).len(), 4);
    }

    #[test]
    fn the_filter_only_looks_at_the_current_folder() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "main");
        assert!(app.rows.is_empty(), "src/main.rs is in a subfolder");
    }

    #[test]
    fn marks_survive_filtering() {
        let mut e = filter_env();
        let app = &mut e.app;
        select(app, "Makefile");
        app.update(Action::ToggleMark);
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::FilterAccept);
        assert_eq!(app.marks.len(), 1, "hidden rows stay marked");
        assert_eq!(app.marked().size, 4096);
        app.update(Action::FilterClear);
        assert_eq!(app.marks.len(), 1);
    }

    #[test]
    fn editing_keys_move_insert_and_delete_by_character() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "héllo wörld");
        assert_eq!(app.filter.cursor, 11);
        app.update(Action::FilterHome);
        assert_eq!(app.filter.cursor, 0);
        app.update(Action::FilterLeft);
        assert_eq!(app.filter.cursor, 0, "clamped at the start");
        app.update(Action::FilterRight);
        app.update(Action::FilterRight);
        type_text(app, "X");
        assert_eq!(app.filter.query, "héXllo wörld");
        assert_eq!(app.filter.cursor, 3);
        app.update(Action::FilterDelete); // removes the l after the cursor
        assert_eq!(app.filter.query, "héXlo wörld");
        app.update(Action::FilterBackspace); // removes the X before it
        assert_eq!(app.filter.query, "hélo wörld");
        assert_eq!(app.filter.cursor, 2);
        app.update(Action::FilterEnd);
        assert_eq!(app.filter.cursor, 10);
        app.update(Action::FilterRight);
        assert_eq!(app.filter.cursor, 10, "clamped at the end");
        app.update(Action::FilterDelete);
        assert_eq!(app.filter.query, "hélo wörld", "nothing after the cursor");
        app.update(Action::FilterHome);
        app.update(Action::FilterBackspace);
        assert_eq!(app.filter.query, "hélo wörld", "nothing before the cursor");
    }

    #[test]
    fn clear_word_and_clear_line_work_like_a_shell() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "foo bar");
        app.update(Action::FilterClearWord);
        assert_eq!(app.filter.query, "foo ");
        app.update(Action::FilterClearWord);
        assert_eq!(app.filter.query, "");
        type_text(app, "foobar");
        app.update(Action::FilterHome);
        for _ in 0..3 {
            app.update(Action::FilterRight);
        }
        app.update(Action::FilterClearLine);
        assert_eq!((app.filter.query.as_str(), app.filter.cursor), ("bar", 0));
    }

    #[test]
    fn filter_actions_do_nothing_while_a_dialog_is_open() {
        let mut e = filter_env();
        let app = &mut e.app;
        select(app, "Makefile");
        app.update(Action::ToggleMark);
        app.update(Action::OpenReview);
        app.update(Action::FilterOpen);
        assert!(!app.filter.editing);
        app.update(Action::FilterChar('x'));
        assert_eq!(app.filter.query, "");
    }

    #[test]
    fn the_filter_belongs_to_the_explorer() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::GoTo(Screen::Home));
        app.update(Action::FilterOpen);
        assert!(!app.filter.editing);
        app.update(Action::GoTo(Screen::Explorer));
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::GoTo(Screen::Holding));
        assert!(!app.filter.editing, "leaving stops editing");
        assert_eq!(app.filter.query, "no", "but the filter is remembered");
    }

    #[test]
    fn a_rescan_of_the_same_folder_keeps_the_filter() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::FilterAccept);
        app.update(Action::Rescan);
        crate::testutil::wait_scan(app);
        assert_eq!(app.filter.query, "no");
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
        app.update(Action::RescanAll);
        crate::testutil::wait_scan(app);
        assert_eq!(app.filter.query, "no", "same folder, so the filter stays");
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
    }

    #[test]
    fn rescanning_everything_from_a_subfolder_drops_the_filter() {
        let mut e = filter_env();
        let app = &mut e.app;
        select(app, "node_modules");
        app.update(Action::Into);
        app.update(Action::FilterOpen);
        type_text(app, "x");
        app.update(Action::FilterAccept);
        assert!(app.filter.active());
        app.update(Action::RescanAll);
        crate::testutil::wait_scan(app);
        assert_eq!(app.stack.len(), 1, "back at the root");
        assert!(!app.filter.active(), "a different folder is shown now");
        assert_eq!(names(app).len(), 4);
    }

    #[test]
    fn clicking_a_row_accepts_the_filter() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        set_layout(app);
        app.update(Action::Click { x: 30, y: 4 });
        assert!(!app.filter.editing);
        assert_eq!(app.filter.query, "no");
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn sorting_keeps_the_filter_applied() {
        let mut e = filter_env();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        app.update(Action::FilterAccept);
        app.update(Action::CycleSort); // by size
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
        app.update(Action::CycleSort); // by name
        assert_eq!(names(app), ["node_modules", "notes.txt"]);
    }

    #[test]
    fn shares_stay_relative_to_the_whole_folder() {
        let mut e = filter_env();
        let app = &mut e.app;
        let before = app.weight_total();
        app.update(Action::FilterOpen);
        type_text(app, "no");
        assert_eq!(app.weight_total(), before);
    }

    #[test]
    fn rows_that_appear_while_scanning_respect_the_filter() {
        let mut e = filter_env();
        let home = e.home.clone();
        let app = &mut e.app;
        app.update(Action::FilterOpen);
        type_text(app, "no");
        crate::testutil::write(&home.join("nothing.bin"), 4096);
        crate::testutil::write(&home.join("other.bin"), 4096);
        app.refresh_rows();
        let n = names(app);
        assert!(n.contains(&"nothing.bin".to_string()));
        assert!(!n.contains(&"other.bin".to_string()));
    }

    #[test]
    fn quitting_with_items_in_tobedeleted_warns_first() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::Quit);
        assert!(!app.should_quit);
        match &app.dialog {
            Some(Dialog::QuitWarning(w)) => {
                assert_eq!(w.count, 2);
                assert_eq!(w.usage.size, 12288 + 8192);
            }
            other => panic!("expected the quit warning, got {}", other.is_some()),
        }
        app.update(Action::Cancel);
        assert!(app.dialog.is_none());
        assert!(!app.should_quit, "staying is the default");
        app.update(Action::Quit);
        app.update(Action::Confirm);
        assert!(app.should_quit);
    }

    #[test]
    fn a_second_quit_confirms_the_warning() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::Quit);
        assert!(!app.should_quit);
        app.update(Action::Quit); // Ctrl-C twice
        assert!(app.should_quit);
    }

    #[test]
    fn quitting_with_an_empty_tobedeleted_just_quits() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::Quit);
        assert!(app.should_quit);
        assert!(app.dialog.is_none());
    }

    #[test]
    fn the_warning_can_open_tobedeleted_instead() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::Quit);
        app.update(Action::ReviewHeld);
        assert!(app.dialog.is_none());
        assert_eq!(app.screen, Screen::Holding);
        assert_eq!(app.focus, Focus::Main);
        assert!(!app.should_quit);
    }

    #[test]
    fn review_held_does_nothing_without_the_warning() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.update(Action::ReviewHeld);
        assert_eq!(app.screen, Screen::Explorer);
    }

    #[test]
    fn emptying_tobedeleted_removes_the_warning() {
        let mut e = env(FILES);
        let app = &mut e.app;
        hold_two(app);
        app.update(Action::GoTo(Screen::Holding));
        app.update(Action::EmptyAll);
        app.update(Action::Confirm);
        settle(app);
        app.update(Action::Cancel);
        app.update(Action::Quit);
        assert!(app.should_quit, "nothing left to warn about");
    }

    #[test]
    fn items_left_by_an_earlier_run_also_warn() {
        use crate::accounting::Accounting;
        use crate::glyphs::{GlyphMode, Glyphs};
        use crate::safety::Policy;
        use crate::scan::Scanner;
        use crate::softdelete::Holding;
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let home = base.join("home");
        let hold = base.join("hold");
        crate::testutil::write(&home.join("old.bin"), 5000);
        let policy = Policy::new(&home, &hold);
        Holding::open(hold.clone())
            .unwrap()
            .move_in(&policy, &home.join("old.bin"), Usage::new(5000, 8192))
            .unwrap();
        let scanner = Scanner::new(2);
        let root = scanner.scan_root(&home);
        let mut app = App::new(
            scanner,
            root,
            policy,
            Holding::open(hold).unwrap(),
            Accounting::new(0),
            Glyphs::new(GlyphMode::Nerd),
            false,
        );
        app.update(Action::Quit);
        assert!(!app.should_quit);
        assert!(matches!(app.dialog, Some(Dialog::QuitWarning(_))));
    }
}
