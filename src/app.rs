use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use ratatui::layout::Rect;

use crate::accounting::Accounting;
use crate::anim::Smooth;
use crate::bg::Latest;
use crate::catalog::{CommandRunner, CommandSpec, SystemRunner, ToolEntry, discover_tools};
use crate::filter;
use crate::fmt::both_usage;
use crate::glyphs::Glyphs;
use crate::jobs::{CommandEvent, CommandJob, DeleteEvent, DeleteJob, DeleteMode, Phase};
use crate::safety::{Policy, Safety};
use crate::scan::{Node, Row, Scanner, Space, file_space, locate};
use crate::softdelete::{HeldItem, Holding};
use crate::targets::{Line, SectionKind, TRow, Targets};
use crate::usage::Usage;
use crate::volume;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Explorer,
    Targets,
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
    /// What deleting the entry frees right now (clone- and snapshot-aware).
    Frees,
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

pub const SIDEBAR: [SidebarEntry; 7] = [
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
        label: "Caches & Projects",
        screen: Some(Screen::Targets),
    },
    SidebarEntry {
        label: "Docker",
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
    /// The review dialog cannot show every tool command at this terminal size.
    pub review_clipped: bool,
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
    /// Mark every folder that shares data with the selected row (Caches & Projects).
    MarkRelated,
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

/// A tool's own cleanup command in the review.
#[derive(Clone, Debug)]
pub struct ReviewCommand {
    pub id: String,
    pub tool: String,
    /// `pnpm store prune`
    pub line: String,
    pub spec: CommandSpec,
    /// The most it might free (what deleting its folder alone would).
    pub up_to: u64,
}

/// How one cleanup command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    pub line: String,
    pub ok: bool,
    /// The last line the command printed.
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct Review {
    pub items: Vec<ReviewItem>,
    /// Tool commands to run after the folders are moved.
    pub commands: Vec<ReviewCommand>,
    pub total: Usage,
    /// False while the set-wise numbers are still being worked out.
    pub space_ready: bool,
    /// Where the marked items' disk space would go: freed now, held by
    /// snapshots, or shared with clones and hard links that stay.
    pub space: Space,
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
    /// How each tool command ended (empty when none ran).
    pub commands: Vec<CommandOutcome>,
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

/// Where the selected row's disk space would go.
#[derive(Clone, Debug)]
pub struct Detail {
    pub path: PathBuf,
    pub space: Space,
    /// False while the folder is still being scanned (numbers still growing).
    pub complete: bool,
}

/// Whether a path is in the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkState {
    Unmarked,
    Marked,
    /// A folder above it is marked, so it goes with that one.
    Covered(PathBuf),
}

/// Another folder that shares data with the selected row.
#[derive(Clone, Debug)]
pub struct Holder {
    pub path: PathBuf,
    pub usage: Usage,
    pub state: MarkState,
}

/// Where the space of everything marked would go, as a set.
#[derive(Clone, Copy, Debug, Default)]
pub struct JointSpace {
    /// The marked folders and files only.
    pub base: Space,
    /// The same plus the folders behind marked tool commands.
    pub with_tools: Space,
    /// Just the marked folders that belong to the Projects section.
    pub projects: Space,
    /// Just the marked folders that belong to the Tools & Caches section.
    pub tools: Space,
}

/// What a Caches & Projects line stands for (survives a rebuild of the list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineKey {
    Section(SectionKind),
    Group(String),
    Row(PathBuf),
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
    /// The current folder's own space numbers, known once the scan is done.
    pub current_space: Option<Space>,
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
    /// The Caches & Projects list: built groups, the visible lines and the cursor.
    pub targets: Targets,
    pub tlines: Vec<Line>,
    pub tcursor: usize,
    pub toffset: usize,
    /// Tool caches found by asking the installed tools (empty until looked up).
    pub tool_entries: Vec<ToolEntry>,
    tools_rx: Option<mpsc::Receiver<Vec<ToolEntry>>>,
    tools_started: bool,
    /// Keys of the groups the user folded (kept across rebuilds).
    closed_groups: HashSet<String>,
    targets_synced: bool,
    /// The list being built on a thread (a full walk of the scan tree).
    targets_job: Latest<(Targets, bool)>,
    /// Tool commands marked for the review (the second kind of selection).
    pub tool_marks: BTreeSet<String>,
    /// Set-wise space of the whole selection, worked out on a thread.
    pub set_space: Latest<JointSpace>,
    set_exact: bool,
    /// Folders sharing data with the selected row, worked out on a thread.
    pub holders: Latest<(PathBuf, Vec<PathBuf>)>,
    holders_key: Option<PathBuf>,
    pub runner: Arc<dyn CommandRunner>,
    job: Option<DeleteJob>,
    cmd_job: Option<CommandJob>,
    /// Ids and commands waiting to run (or running) after the moves.
    pending_commands: Vec<(String, CommandSpec)>,
    cmd_outcomes: Vec<CommandOutcome>,
    last_moves: (Usage, Usage, usize),
    job_mode: DeleteMode,
    job_free_before: u64,
    job_failure: Option<String>,
    quit_after_job: bool,
}

/// The order the explorer (and every screen like it) puts rows in: biggest
/// first for the size columns, A to Z for names; ties fall back to the rest.
pub fn cmp_rows(by: SortBy, a: &Row, b: &Row) -> std::cmp::Ordering {
    match by {
        SortBy::Disk => b
            .disk
            .cmp(&a.disk)
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.name.cmp(&b.name)),
        SortBy::Frees => b
            .frees
            .unwrap_or(0)
            .cmp(&a.frees.unwrap_or(0))
            .then_with(|| b.disk.cmp(&a.disk))
            .then_with(|| a.name.cmp(&b.name)),
        SortBy::Size => b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)),
        SortBy::Name => a
            .name
            .to_string_lossy()
            .to_lowercase()
            .cmp(&b.name.to_string_lossy().to_lowercase()),
    }
}

fn sort_rows(rows: &mut [Row], by: SortBy) {
    rows.sort_by(|a, b| cmp_rows(by, a, b));
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
            graph: GraphMode::Treemap,
            graph_pct: 35,
            sort: SortBy::Disk,
            rows: Vec::new(),
            all_rows: Vec::new(),
            filter: FilterState::default(),
            detail: None,
            current_space: None,
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
            targets: Targets::default(),
            tlines: Vec::new(),
            tcursor: 0,
            toffset: 0,
            tool_entries: Vec::new(),
            tools_rx: None,
            tools_started: false,
            closed_groups: HashSet::new(),
            targets_synced: false,
            targets_job: Latest::new(),
            tool_marks: BTreeSet::new(),
            set_space: Latest::new(),
            set_exact: true,
            holders: Latest::new(),
            holders_key: None,
            runner: Arc::new(SystemRunner),
            job: None,
            cmd_job: None,
            pending_commands: Vec::new(),
            cmd_outcomes: Vec::new(),
            last_moves: (Usage::default(), Usage::default(), 0),
            job_mode: DeleteMode::SoftOnly,
            job_free_before: 0,
            job_failure: None,
            quit_after_job: false,
        };
        app.set_space.set_now(JointSpace::default());
        app.refresh_rows();
        app.refresh_held();
        app
    }

    /// Use another way of running tool commands (tests pass a fake).
    pub fn with_runner(mut self, runner: Arc<dyn CommandRunner>) -> App {
        self.runner = runner;
        self
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
            SortBy::Frees if self.frees_total() > 0 => row.frees.unwrap_or(0),
            SortBy::Frees | SortBy::Disk | SortBy::Name => row.disk,
        }
    }

    /// What deleting the whole folder shown frees now (0 while unknown).
    fn frees_total(&self) -> u64 {
        self.current_space.map_or(0, |s| s.frees_now)
    }

    /// The matching total for the folder being shown.
    pub fn weight_total(&self) -> u64 {
        match self.sort {
            SortBy::Size => self.current().size(),
            SortBy::Frees if self.frees_total() > 0 => self.frees_total(),
            SortBy::Frees | SortBy::Disk | SortBy::Name => self
                .current_space
                .map_or_else(|| self.current().disk(), |s| s.footprint),
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
        self.toffset = window(self.tcursor, self.toffset, vis);
    }

    /// Re-list the current directory, keeping the cursor on the same path.
    pub fn refresh_rows(&mut self) {
        let keep = self
            .pending_select
            .take()
            .or_else(|| self.selected_row().map(|r| r.path.clone()));
        let node = Arc::clone(self.current());
        let mut rows = self.scanner.list_children(&node);
        self.current_space = self.scanner.is_idle().then(|| self.scanner.space(&node));
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
        if self.screen == Screen::Targets {
            let keep = keep_selection.then(|| self.sel_of(self.tcursor)).flatten();
            self.rebuild_tlines(keep);
            if !keep_selection {
                self.tcursor = 0;
                self.toffset = 0;
            }
            return;
        }
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
        if !matches!(self.screen, Screen::Explorer | Screen::Targets) {
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
            Action::MarkRelated => self.mark_related(),
            Action::OpenReview => self.open_review(),
            Action::CycleGraph => {
                self.graph = match self.graph {
                    GraphMode::Treemap => GraphMode::Hidden,
                    GraphMode::Hidden => GraphMode::Bars,
                    GraphMode::Bars => GraphMode::Treemap,
                }
            }
            Action::GrowGraph => self.graph_pct = (self.graph_pct + 5).min(70),
            Action::ShrinkGraph => self.graph_pct = self.graph_pct.saturating_sub(5).max(20),
            Action::CycleSort => {
                self.sort = match self.sort {
                    // Same order as the column headers: Name, Size, On disk, Frees.
                    SortBy::Name => SortBy::Size,
                    SortBy::Size => SortBy::Disk,
                    SortBy::Disk => SortBy::Frees,
                    SortBy::Frees => SortBy::Name,
                };
                if self.screen == Screen::Targets {
                    // Only the order changes: no need to build the list again.
                    let keep = self.sel_of(self.tcursor);
                    self.targets.sort(self.sort);
                    self.rebuild_tlines(keep);
                } else {
                    self.refresh_rows();
                }
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
        if self.job.is_some() || self.cmd_job.is_some() {
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
        self.poll_commands();
        if self.animations {
            self.free_shown.step(self.free_now as f64, 0.15);
        } else {
            self.free_shown = Smooth::new(self.free_now as f64);
        }
        if self.tick.is_multiple_of(20) {
            self.refresh_free();
        }
        self.poll_discovery();
        self.poll_targets();
        if self.set_space.poll() || !self.set_space.pending {
            // A review opened while the numbers were being worked out.
            if let (Some(Dialog::Review(r)), Some(j)) = (&mut self.dialog, self.set_space.value)
                && !r.space_ready
                && !self.set_space.pending
            {
                r.space = j.base;
                r.space_ready = true;
            }
        }
        self.holders.poll();
        if self.scanner.is_idle() && !self.set_exact && !self.set_space.pending {
            // The scan finished since the numbers were worked out: redo them.
            self.marks_changed();
            self.holders_key = None;
            self.request_holders();
        }
        if self.screen == Screen::Targets {
            let idle = self.scanner.is_idle();
            if (!idle && self.tick.is_multiple_of(10)) || (idle && !self.targets_synced) {
                self.rebuild_targets();
            }
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
            Some(Dialog::Review(r)) => {
                if !r.commands.is_empty() && self.layout.review_clipped {
                    // Never run a command the review could not show.
                    self.notice = Some(
                        "Some commands do not fit on screen. Make the terminal taller, or Esc."
                            .to_string(),
                    );
                    self.dialog = Some(Dialog::Review(r));
                } else {
                    self.run_delete(r, DeleteMode::SoftOnly);
                }
            }
            Some(Dialog::Irreversible(r)) => self.run_delete(r, DeleteMode::SoftThenEmpty),
            Some(Dialog::ConfirmForever(t)) => self.run_forever(t),
            Some(Dialog::Deleting(p)) => self.dialog = Some(Dialog::Deleting(p)),
            Some(Dialog::Finish(_)) | Some(Dialog::Notice(_)) | None => {}
        }
    }

    fn goto(&mut self, screen: Screen) {
        self.filter.editing = false;
        // The filter belongs to one list; it does not follow to another screen.
        if (self.screen == Screen::Targets || screen == Screen::Targets) && self.screen != screen {
            self.reset_filter();
        }
        self.screen = screen;
        self.transition_start = self.tick;
        match screen {
            Screen::Holding => self.refresh_held(),
            Screen::Explorer => self.refresh_rows(),
            Screen::Targets => {
                self.start_discovery();
                self.rebuild_targets();
            }
            Screen::Home => {}
        }
        self.refresh_detail();
    }

    fn move_by(&mut self, delta: i64) {
        match (self.focus, self.screen) {
            (Focus::Sidebar, _) => {
                let order = [
                    Screen::Home,
                    Screen::Explorer,
                    Screen::Targets,
                    Screen::Holding,
                ];
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
            (_, Screen::Targets) => {
                self.tcursor = step(self.tcursor, delta, self.tlines.len());
                self.ensure_visible();
                self.request_holders();
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
        let whole =
            all || matches!(self.screen, Screen::Home | Screen::Targets) || self.stack.len() == 1;
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
        if self.screen == Screen::Targets {
            self.target_into();
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
        if self.focus == Focus::Main && self.screen == Screen::Targets {
            self.target_out();
            return;
        }
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
        match self.screen {
            Screen::Explorer => {
                let Some(row) = self.selected_row() else {
                    return;
                };
                let (path, usage) = (row.path.clone(), Usage::new(row.size, row.disk));
                let name = row.name.to_string_lossy().into_owned();
                self.mark_path(path, usage, &name);
            }
            Screen::Targets => self.toggle_target_mark(),
            _ => {}
        }
    }

    /// Toggle one path in the shared selection, with the reasons it may be
    /// refused. Returns true if the selection changed.
    fn mark_path(&mut self, path: PathBuf, usage: Usage, name: &str) -> bool {
        if self.marks.remove(&path).is_some() {
            self.marks_changed();
            return true;
        }
        if !self.policy.can_mark(&path) {
            self.notice = Some(format!("{name} is protected and cannot be deleted"));
            return false;
        }
        if self.marks.keys().any(|m| path.starts_with(m)) {
            self.notice = Some(format!("{name} is already covered by a marked folder"));
            return false;
        }
        self.insert_mark(path, usage);
        self.marks_changed();
        true
    }

    /// Put a path in the selection (a marked folder swallows marks inside it).
    fn insert_mark(&mut self, path: PathBuf, usage: Usage) {
        self.marks.retain(|m, _| !m.starts_with(&path));
        self.marks.insert(path, usage);
    }

    /// Is this path in the selection, covered by it, or not?
    pub fn mark_state(&self, path: &Path) -> MarkState {
        if self.marks.contains_key(path) {
            return MarkState::Marked;
        }
        match self.marks.keys().find(|m| path.starts_with(m)) {
            Some(parent) => MarkState::Covered(parent.clone()),
            None => MarkState::Unmarked,
        }
    }

    /// The same for a Caches & Projects row (command rows are marked by id).
    pub fn row_mark(&self, t: &TRow) -> MarkState {
        match &t.kind {
            crate::targets::RowKind::Folder => self.mark_state(&t.row.path),
            crate::targets::RowKind::Command { id, .. } => {
                if self.tool_marks.contains(id) {
                    MarkState::Marked
                } else {
                    match self.mark_state(&t.row.path) {
                        MarkState::Covered(p) => MarkState::Covered(p),
                        _ => MarkState::Unmarked,
                    }
                }
            }
        }
    }

    fn toggle_target_mark(&mut self) {
        match self.tlines.get(self.tcursor).copied() {
            Some(Line::Row { group, row }) => {
                let t = self.targets.row(group, row).clone();
                let name = t.row.name.to_string_lossy().into_owned();
                match &t.kind {
                    crate::targets::RowKind::Folder => {
                        self.mark_path(
                            t.row.path.clone(),
                            Usage::new(t.row.size, t.row.disk),
                            &name,
                        );
                    }
                    crate::targets::RowKind::Command { id, .. } => {
                        if !self.tool_marks.remove(id) {
                            if let MarkState::Covered(_) = self.mark_state(&t.row.path) {
                                self.notice =
                                    Some(format!("{name} is already covered by a marked folder"));
                                return;
                            }
                            self.tool_marks.insert(id.clone());
                        }
                        self.marks_changed();
                    }
                }
            }
            Some(Line::Group(g)) => {
                let items = self.folder_rows_of(|gi, _| gi == g);
                self.mark_many(items);
            }
            Some(Line::Section(kind)) => {
                let items = self.folder_rows_of(|_, grp| grp.section == kind);
                self.mark_many(items);
            }
            None => {}
        }
    }

    /// The folder rows (never command rows) of the groups `keep` accepts.
    fn folder_rows_of(
        &self,
        keep: impl Fn(usize, &crate::targets::Group) -> bool,
    ) -> Vec<(PathBuf, Usage)> {
        self.targets
            .groups
            .iter()
            .enumerate()
            .filter(|(i, g)| keep(*i, g))
            .flat_map(|(_, g)| g.rows.iter())
            .filter(|r| !r.is_command())
            .map(|r| (r.row.path.clone(), Usage::new(r.row.size, r.row.disk)))
            .collect()
    }

    /// Bulk toggle: if everything is already in, take it out; otherwise put
    /// in whatever is allowed and not yet covered.
    fn mark_many(&mut self, items: Vec<(PathBuf, Usage)>) {
        if items.is_empty() {
            return;
        }
        let all_in = items
            .iter()
            .all(|(p, _)| self.mark_state(p) != MarkState::Unmarked);
        if all_in {
            for (p, _) in &items {
                self.marks.remove(p);
            }
        } else {
            for (p, u) in items {
                if self.mark_state(&p) == MarkState::Unmarked && self.policy.can_mark(&p) {
                    self.insert_mark(p, u);
                }
            }
        }
        self.marks_changed();
    }

    /// Ask for fresh set-wise numbers after the selection changed.
    pub fn marks_changed(&mut self) {
        if self.marks.is_empty() && self.tool_marks.is_empty() {
            self.set_space.set_now(JointSpace::default());
            self.set_exact = true;
            return;
        }
        let inputs = self.joint_inputs();
        let view = self.scanner.view();
        self.set_exact = self.scanner.is_idle();
        self.set_space.spawn(move || joint_of(&view, inputs));
    }

    /// What the set-wise calculation needs, copied so a thread can use it.
    fn joint_inputs(&self) -> JointInputs {
        let root = &self.stack[0];
        let (mut nodes, mut files) = (Vec::new(), Vec::new());
        for (p, u) in &self.marks {
            match locate(root, p) {
                Some(n) => nodes.push(n),
                None => files.push((p.clone(), *u)),
            }
        }
        // Folders behind marked tool commands (unless a marked folder covers them).
        let tool_nodes = self
            .targets
            .groups
            .iter()
            .flat_map(|g| g.rows.iter())
            .filter(|r| match &r.kind {
                crate::targets::RowKind::Command { id, .. } => {
                    self.tool_marks.contains(id)
                        && !self.marks.keys().any(|m| r.row.path.starts_with(m))
                }
                _ => false,
            })
            .filter_map(|r| r.row.node.clone())
            .collect();
        // Which section each marked folder belongs to (by the rows it covers).
        let (mut projects, mut tools) = (Vec::new(), Vec::new());
        for n in &nodes {
            let section = self
                .targets
                .groups
                .iter()
                .find(|g| {
                    g.rows
                        .iter()
                        .any(|r| r.row.path.starts_with(&n.path) || n.path.starts_with(&r.row.path))
                })
                .map(|g| g.section);
            match section {
                Some(SectionKind::Projects) => projects.push(Arc::clone(n)),
                Some(SectionKind::Tools) => tools.push(Arc::clone(n)),
                None => {}
            }
        }
        JointInputs {
            projects,
            tools,
            nodes,
            files,
            tool_nodes,
        }
    }

    /// What deleting everything marked frees, as a set; `None` while it is
    /// being worked out.
    pub fn joint_frees(&self) -> Option<u64> {
        if self.set_space.pending {
            return None;
        }
        self.set_space.value.map(|j| j.base.frees_all())
    }

    /// The joint figure for the marked folders of one section.
    pub fn section_frees(&self, section: SectionKind) -> Option<u64> {
        if self.set_space.pending {
            return None;
        }
        self.set_space.value.map(|j| match section {
            SectionKind::Projects => j.projects.frees_all(),
            SectionKind::Tools => j.tools.frees_all(),
        })
    }

    /// What the marked tool commands might add on top (an upper bound).
    pub fn tools_up_to(&self) -> u64 {
        if self.tool_marks.is_empty() {
            return 0;
        }
        self.set_space.value.map_or(0, |j| {
            j.with_tools.frees_all().saturating_sub(j.base.frees_all())
        })
    }

    /// Look up which other folders share data with the selected row.
    pub fn request_holders(&mut self) {
        let Some(t) = self.selected_target() else {
            return;
        };
        let path = t.row.path.clone();
        if self.holders_key.as_ref() == Some(&path) {
            return;
        }
        let Some(node) = t.row.node.clone() else {
            return;
        };
        self.holders_key = Some(path.clone());
        let candidates: Vec<PathBuf> = self
            .targets
            .groups
            .iter()
            .flat_map(|g| g.rows.iter().map(|r| r.row.path.clone()))
            .filter(|p| *p != path)
            .collect();
        let view = self.scanner.view();
        self.holders.spawn(move || {
            let mut out: BTreeSet<PathBuf> = BTreeSet::new();
            for dir in view.sharers(&node) {
                // Name the listed row that holds it, or the folder itself.
                let owner = candidates.iter().find(|c| dir.starts_with(c));
                out.insert(owner.cloned().unwrap_or(dir));
            }
            out.remove(&path);
            (path, out.into_iter().take(50).collect())
        });
    }

    /// The related folders of the selected row, with their sizes and marks.
    pub fn holders_view(&self) -> Vec<Holder> {
        let (Some(t), Some((key, paths))) = (self.selected_target(), &self.holders.value) else {
            return Vec::new();
        };
        if *key != t.row.path {
            return Vec::new();
        }
        paths
            .iter()
            .map(|p| {
                let usage = match self.targets.find(p) {
                    Some((g, r)) => {
                        let row = &self.targets.row(g, r).row;
                        Usage::new(row.size, row.disk)
                    }
                    None => locate(&self.stack[0], p)
                        .map_or_else(Usage::default, |n| Usage::new(n.size(), n.disk())),
                };
                Holder {
                    path: p.clone(),
                    usage,
                    state: self.mark_state(p),
                }
            })
            .collect()
    }

    /// `m`: mark the selected folder and every folder that shares its data.
    fn mark_related(&mut self) {
        if self.screen != Screen::Targets {
            return;
        }
        let mut changed = 0;
        let mut todo: Vec<(PathBuf, Usage)> = Vec::new();
        if let Some(t) = self.selected_target()
            && !t.is_command()
        {
            todo.push((t.row.path.clone(), Usage::new(t.row.size, t.row.disk)));
        }
        todo.extend(self.holders_view().into_iter().map(|h| (h.path, h.usage)));
        for (p, u) in todo {
            if self.mark_state(&p) == MarkState::Unmarked && self.policy.can_mark(&p) {
                self.insert_mark(p, u);
                changed += 1;
            }
        }
        if changed > 0 {
            self.marks_changed();
            self.notice = Some(format!("Marked {changed} related folder(s)"));
        } else {
            self.notice = Some("Nothing more to mark here".to_string());
        }
    }

    fn open_review(&mut self) {
        if !matches!(self.screen, Screen::Explorer | Screen::Targets) {
            return;
        }
        let commands = self.review_commands();
        if self.marks.is_empty() && commands.is_empty() {
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
        // The same figure the footer shows, unless it is still being worked out.
        // (never worked out here: that can take long on a big tree, so a
        // review opened mid-calculation fills the figure in when it lands).
        let (space, space_ready) = match (self.set_space.pending, self.set_space.value) {
            (false, Some(j)) => (j.base, true),
            _ => (Space::default(), false),
        };
        self.dialog = Some(Dialog::Review(Review {
            items,
            commands,
            total,
            space_ready,
            space,
        }));
    }

    /// The marked tool commands, as the review lists them.
    fn review_commands(&self) -> Vec<ReviewCommand> {
        self.targets
            .groups
            .iter()
            .flat_map(|g| g.rows.iter().map(move |r| (g, r)))
            .filter_map(|(g, r)| match &r.kind {
                crate::targets::RowKind::Command { id, spec }
                    if self.tool_marks.contains(id)
                        && !self.marks.keys().any(|m| r.row.path.starts_with(m)) =>
                {
                    Some(ReviewCommand {
                        id: id.clone(),
                        tool: g.title.clone(),
                        line: spec.display(),
                        spec: spec.clone(),
                        up_to: r.row.frees.unwrap_or(0),
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Where the selected row's disk space would go (explorer only).
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
            let (space, complete) = match row.space {
                Some(space) => (space, true),
                // Still scanning: only the additive numbers are known yet.
                None => (
                    Space {
                        footprint: node.disk(),
                        frees_now: node.private(),
                        snapshots: node.snapshot_held(),
                        ..Space::default()
                    },
                    false,
                ),
            };
            Some(Detail {
                path: row.path.clone(),
                space,
                complete,
            })
        } else {
            Some(Detail {
                path: row.path.clone(),
                space: file_space(&row.path).ok()?,
                complete: true,
            })
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
                Screen::Targets => {
                    let idx = self.toffset + i;
                    if idx < self.tlines.len() {
                        if idx == self.tcursor {
                            self.target_into();
                        } else {
                            self.tcursor = idx;
                            self.request_holders();
                        }
                    }
                }
                Screen::Home => {}
            }
        }
    }

    // --- Caches & Projects ---------------------------------------------

    /// Is the look for installed tools still running?
    pub fn tools_looking(&self) -> bool {
        self.tools_rx.is_some()
    }

    /// Ask the installed tools where their caches are, once, on a thread.
    fn start_discovery(&mut self) {
        if self.tools_started {
            return;
        }
        self.tools_started = true;
        let (tx, rx) = mpsc::channel();
        let runner = Arc::clone(&self.runner);
        let home = self.policy.home().to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(discover_tools(&home, runner.as_ref()));
        });
        self.tools_rx = Some(rx);
    }

    fn poll_discovery(&mut self) {
        let Some(rx) = &self.tools_rx else { return };
        match rx.try_recv() {
            Ok(entries) => {
                self.tool_entries = entries;
                self.tools_rx = None;
                self.rebuild_targets();
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.tools_rx = None,
        }
    }

    /// Build the list again from the scan on a thread (it walks the whole
    /// scan tree and reads the clone tables). The result is picked up by the
    /// next tick; the cursor stays on the same line.
    pub fn rebuild_targets(&mut self) {
        let root = Arc::clone(&self.stack[0]);
        let home = self.policy.home().to_path_buf();
        let view = self.scanner.view();
        let idle = self.scanner.is_idle();
        let policy = Arc::clone(&self.policy);
        let entries = self.tool_entries.clone();
        let closed = self.closed_groups.clone();
        self.targets_job.spawn(move || {
            (
                Targets::build(&root, &home, &view, idle, &policy, &entries, &closed),
                idle,
            )
        });
    }

    /// Take in a finished list build, if there is one.
    fn poll_targets(&mut self) {
        if !self.targets_job.poll() {
            return;
        }
        let Some((mut built, idle)) = self.targets_job.value.take() else {
            return;
        };
        let keep = self.sel_of(self.tcursor);
        // Folds and sorting may have changed while it was being built.
        for g in &mut built.groups {
            g.open = !self.closed_groups.contains(&g.key());
        }
        built.sort(self.sort);
        self.targets = built;
        self.targets_synced = idle;
        self.rebuild_tlines(keep);
    }

    /// Wait for the list build to land. Blocks: for tests and tools, never
    /// for the UI loop.
    pub fn settle_targets(&mut self) {
        for _ in 0..4000 {
            self.poll_targets();
            if !self.targets_job.pending {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Is the list still being built?
    pub fn targets_building(&self) -> bool {
        self.targets_job.pending
    }

    fn rebuild_tlines(&mut self, keep: Option<LineKey>) {
        self.tlines = self.targets.lines(&filter::parse(&self.filter.query));
        let found = keep.and_then(|k| self.index_of(&k));
        self.tcursor = found
            .unwrap_or(self.tcursor)
            .min(self.tlines.len().saturating_sub(1));
        self.ensure_visible();
        self.request_holders();
    }

    /// What a line stands for, so the cursor can follow it across rebuilds.
    fn sel_of(&self, idx: usize) -> Option<LineKey> {
        Some(match *self.tlines.get(idx)? {
            Line::Section(k) => LineKey::Section(k),
            Line::Group(g) => LineKey::Group(self.targets.groups.get(g)?.key()),
            Line::Row { group, row } => LineKey::Row(
                self.targets
                    .groups
                    .get(group)?
                    .rows
                    .get(row)?
                    .row
                    .path
                    .clone(),
            ),
        })
    }

    fn index_of(&self, key: &LineKey) -> Option<usize> {
        self.tlines.iter().position(|l| match (*l, key) {
            (Line::Section(a), LineKey::Section(b)) => a == *b,
            (Line::Group(g), LineKey::Group(k)) => {
                self.targets.groups.get(g).is_some_and(|x| x.key() == *k)
            }
            (Line::Row { group, row }, LineKey::Row(p)) => self
                .targets
                .groups
                .get(group)
                .and_then(|g| g.rows.get(row))
                .is_some_and(|r| r.row.path == *p),
            _ => false,
        })
    }

    /// The row under the cursor on the Caches & Projects screen.
    pub fn selected_target(&self) -> Option<&TRow> {
        match *self.tlines.get(self.tcursor)? {
            Line::Row { group, row } => self.targets.groups.get(group)?.rows.get(row),
            _ => None,
        }
    }

    /// `l`, Enter or a second click: open a folded group, or show a row in the Explorer.
    fn target_into(&mut self) {
        match self.tlines.get(self.tcursor).copied() {
            Some(Line::Group(g)) => self.set_fold(g, true),
            Some(Line::Row { .. }) => {
                if let Some(path) = self.selected_target().map(|t| t.row.path.clone()) {
                    self.show_in_explorer(&path);
                }
            }
            _ => {}
        }
    }

    /// `h`: fold the group the cursor is in (and move to its header).
    fn target_out(&mut self) {
        match self.tlines.get(self.tcursor).copied() {
            Some(Line::Group(g)) | Some(Line::Row { group: g, .. }) => self.set_fold(g, false),
            _ => {}
        }
    }

    fn set_fold(&mut self, group: usize, open: bool) {
        let Some(key) = self.targets.groups.get(group).map(|g| g.key()) else {
            return;
        };
        if open {
            self.closed_groups.remove(&key);
        } else {
            self.closed_groups.insert(key.clone());
        }
        if let Some(g) = self.targets.groups.get_mut(group) {
            g.open = open;
        }
        self.rebuild_tlines(Some(LineKey::Group(key)));
    }

    /// Go to the Explorer showing `path`'s folder with `path` selected.
    fn show_in_explorer(&mut self, path: &Path) {
        let Some(stack) = path.parent().and_then(|dir| self.stack_to(dir)) else {
            self.notice = Some("That folder is not in the scan".to_string());
            return;
        };
        self.stack = stack;
        self.pending_select = Some(path.to_path_buf());
        self.cursor = 0;
        self.offset = 0;
        self.rows.clear();
        self.goto(Screen::Explorer);
        self.focus = Focus::Main;
    }

    /// The chain of scanned folders from the scan root down to `dir`.
    fn stack_to(&self, dir: &Path) -> Option<Vec<Arc<Node>>> {
        let root = &self.stack[0];
        let rel = dir.strip_prefix(&root.path).ok()?;
        let mut chain = vec![Arc::clone(root)];
        for c in rel.components() {
            let cur = chain.last()?;
            let next = cur.subdir(&cur.path.join(c))?;
            chain.push(next);
        }
        Some(chain)
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
        self.last_moves = (Usage::default(), Usage::default(), 0);
        self.cmd_outcomes.clear();
        self.pending_commands = review
            .commands
            .iter()
            .map(|c| (c.id.clone(), c.spec.clone()))
            .collect();
        if items.is_empty() {
            self.start_commands();
            return;
        }
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
            if !self.pending_commands.is_empty() {
                // The folders are moved; now the tools' own commands run.
                self.moves_done(moved, emptied);
                self.last_moves = (moved, emptied, failures);
                self.start_commands();
                return;
            }
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
        self.moves_done(moved, emptied);
        let reason = self.job_failure.take();
        self.dialog = Some(match mode {
            DeleteMode::SoftThenEmpty | DeleteMode::EmptyHeld => Dialog::Finish(FinishInfo {
                moved,
                emptied,
                free_before,
                free_after: self.free_now,
                failures,
                first_failure: reason,
                commands: Vec::new(),
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

    /// Book what a move job did and refresh everything that shows it.
    fn moves_done(&mut self, moved: Usage, emptied: Usage) {
        if moved != Usage::default() {
            self.accounting.record_soft_delete(moved);
        }
        if emptied != Usage::default() {
            self.accounting.record_emptied(emptied);
        }
        // Marks of items that no longer exist were moved; failed ones stay.
        self.marks
            .retain(|p, _| std::fs::symlink_metadata(p).is_ok());
        self.marks_changed();
        self.refresh_free();
        self.refresh_held();
        self.refresh_rows();
    }

    /// Run the marked tool commands on a thread and show their progress.
    fn start_commands(&mut self) {
        let specs: Vec<CommandSpec> = self
            .pending_commands
            .iter()
            .map(|(_, spec)| spec.clone())
            .collect();
        self.dialog = Some(Dialog::Deleting(DeleteProgress {
            done: 0,
            total: specs.len(),
            usage: Usage::default(),
            of: Usage::default(),
            phase: Phase::Command,
            current: PathBuf::new(),
        }));
        self.cmd_job = Some(CommandJob::start(Arc::clone(&self.runner), specs));
    }

    fn poll_commands(&mut self) {
        let events = self
            .cmd_job
            .as_ref()
            .map(CommandJob::poll)
            .unwrap_or_default();
        for event in events {
            match event {
                CommandEvent::Started { index, total, line } => {
                    if let Some(Dialog::Deleting(p)) = &mut self.dialog {
                        p.done = index;
                        p.total = total;
                        p.phase = Phase::Command;
                        p.current = PathBuf::from(line);
                    }
                }
                CommandEvent::Done {
                    line,
                    ok,
                    output_tail,
                } => {
                    let note = output_tail
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .trim()
                        .chars()
                        .take(100)
                        .collect();
                    self.cmd_outcomes.push(CommandOutcome { line, ok, note });
                    if let Some(Dialog::Deleting(p)) = &mut self.dialog {
                        p.done = self.cmd_outcomes.len();
                    }
                }
                CommandEvent::Finished => self.commands_done(),
            }
        }
    }

    /// Every command has run: forget their marks and show how it went.
    fn commands_done(&mut self) {
        self.cmd_job = None;
        for (id, _) in std::mem::take(&mut self.pending_commands) {
            self.tool_marks.remove(&id);
        }
        self.marks_changed();
        self.refresh_free();
        self.refresh_held();
        self.refresh_rows();
        self.rebuild_targets();
        let (moved, emptied, failures) = self.last_moves;
        self.dialog = Some(Dialog::Finish(FinishInfo {
            moved,
            emptied,
            free_before: self.job_free_before,
            free_after: self.free_now,
            failures,
            first_failure: self.job_failure.take(),
            commands: std::mem::take(&mut self.cmd_outcomes),
            started_tick: self.tick,
        }));
        if self.quit_after_job {
            self.quit_after_job = false;
            self.request_quit();
        }
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
/// The folders and files whose space is worked out together.
struct JointInputs {
    /// The marked folders in each section (other marks belong to neither).
    projects: Vec<Arc<Node>>,
    tools: Vec<Arc<Node>>,
    nodes: Vec<Arc<Node>>,
    files: Vec<(PathBuf, Usage)>,
    tool_nodes: Vec<Arc<Node>>,
}

fn joint_of(view: &crate::scan::SpaceView, inputs: JointInputs) -> JointSpace {
    let add_files = |mut sp: Space| {
        for (f, usage) in &inputs.files {
            sp = sp
                + file_space(f).unwrap_or(Space {
                    footprint: usage.disk,
                    frees_now: usage.disk,
                    ..Space::default()
                });
        }
        sp
    };
    let base = add_files(view.space_of_set(&inputs.nodes));
    let projects = view.space_of_set(&inputs.projects);
    let tools = view.space_of_set(&inputs.tools);
    let mut all = inputs.nodes.clone();
    all.extend(inputs.tool_nodes.iter().cloned());
    let with_tools = add_files(view.space_of_set(&outermost(all)));
    JointSpace {
        base,
        with_tools,
        projects,
        tools,
    }
}

/// Keep only the outermost of a set of folders (drop any inside another).
fn outermost(mut nodes: Vec<Arc<Node>>) -> Vec<Arc<Node>> {
    nodes.sort_by_key(|n| n.path.components().count());
    let mut kept: Vec<Arc<Node>> = Vec::new();
    for n in nodes {
        if !kept.iter().any(|k| n.path.starts_with(&k.path)) {
            kept.push(n);
        }
    }
    kept
}

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
        assert_eq!(app.graph, GraphMode::Treemap, "the treemap is the default");
        assert_eq!(app.graph_pct, 35);
        app.update(Action::CycleGraph);
        assert_eq!(app.graph, GraphMode::Hidden);
        app.update(Action::CycleGraph);
        assert_eq!(app.graph, GraphMode::Bars);
        app.update(Action::CycleGraph);
        assert_eq!(app.graph, GraphMode::Treemap);
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
    fn sort_cycles_in_the_order_of_the_column_headers() {
        // Header order is Name, Size, On disk, Frees; `s` walks it left to right.
        let mut e = sort_env();
        let app = &mut e.app;
        assert_eq!(app.sort, SortBy::Disk, "on disk is the default");
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Frees);
        // Plain files free what they occupy, so this order matches on-disk.
        assert_eq!(names(app), ["real.bin", "a.bin", "sparse.bin"]);
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Name);
        assert_eq!(names(app), ["a.bin", "real.bin", "sparse.bin"]);
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Size);
        assert_eq!(names(app), ["sparse.bin", "real.bin", "a.bin"]);
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
        app.update(Action::CycleSort); // by what it frees
        app.update(Action::CycleSort); // by name
        app.update(Action::CycleSort); // by size
        assert_eq!(app.sort, SortBy::Size);
        assert_eq!(app.weight(&sparse), sparse.size);
        assert_eq!(app.weight_total(), app.current().size());
    }

    fn set_layout(app: &mut App) {
        app.layout = UiLayout {
            sidebar: Rect::new(0, 0, 16, 20),
            main: Rect::new(16, 0, 80, 20),
            list_top: 3,
            list_rows: 10,
            ..UiLayout::default()
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
        assert_eq!(app.screen, Screen::Targets);
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
        for _ in 0..4 {
            // frees, size, name, and back to on-disk: the filter stays on.
            app.update(Action::CycleSort);
            assert_eq!(names(app), ["node_modules", "notes.txt"]);
        }
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

    fn clone_env() -> crate::testutil::Env {
        // store/orig (8192) is cloned into proj/pkg; proj/own is plain (4096).
        crate::testutil::env_with(&[("store/orig", 8192), ("proj/own", 4096)], |home| {
            crate::testutil::clone_file(&home.join("store/orig"), &home.join("proj/pkg"))
        })
    }

    #[test]
    fn detail_splits_the_space_of_the_selected_folder() {
        let mut e = shared_env(); // hard link store/pkg <-> proj/pkg
        let app = &mut e.app;
        select(app, "proj");
        let d = app.detail.as_ref().expect("a detail for the selected row");
        assert!(d.path.ends_with("proj"));
        assert!(d.complete);
        let sp = d.space;
        assert_eq!((sp.footprint, sp.shared_outside), (12288, 8192));
        assert_eq!(sp.frees_now + sp.snapshots, 4096);
        select(app, "store");
        let sp = app.detail.as_ref().unwrap().space;
        assert_eq!(
            (sp.footprint, sp.shared_outside, sp.frees_now),
            (8192, 8192, 0)
        );
    }

    #[test]
    fn detail_counts_clones_as_shared_too() {
        let mut e = clone_env();
        let app = &mut e.app;
        select(app, "proj");
        let sp = app.detail.as_ref().unwrap().space;
        assert_eq!((sp.footprint, sp.shared_outside), (12288, 8192));
        assert_eq!(sp.frees_now + sp.snapshots, 4096);
        select(app, "store");
        let sp = app.detail.as_ref().unwrap().space;
        assert_eq!(
            (sp.footprint, sp.shared_outside, sp.frees_now),
            (8192, 8192, 0)
        );
    }

    #[test]
    fn detail_for_single_files_knows_links_and_clones() {
        let mut e = clone_env();
        let app = &mut e.app;
        select(app, "proj");
        app.update(Action::Into);
        select(app, "pkg"); // the clone
        let sp = app.detail.as_ref().unwrap().space;
        assert_eq!(
            (sp.footprint, sp.shared_outside, sp.frees_now),
            (8192, 8192, 0)
        );
        select(app, "own"); // plain
        let sp = app.detail.as_ref().unwrap().space;
        assert_eq!((sp.footprint, sp.shared_outside), (4096, 0));
        assert_eq!(sp.frees_now + sp.snapshots, 4096);
    }

    #[test]
    fn folders_are_sorted_and_sized_by_their_real_footprint() {
        // dup holds a file and its clone: 16384 allocated, 8192 really.
        // solo holds one 12288 file. By footprint solo is bigger.
        let mut e = crate::testutil::env_with(&[("dup/orig", 8192), ("solo/f", 12288)], |home| {
            crate::testutil::clone_file(&home.join("dup/orig"), &home.join("dup/clone"))
        });
        let app = &mut e.app;
        assert_eq!(names(app), ["solo", "dup"]);
        let dup = app.rows.iter().find(|r| r.name == "dup").unwrap();
        assert_eq!(dup.disk, 8192, "the row shows the footprint");
        assert!(dup.space.is_some());
    }

    #[test]
    fn shares_are_relative_to_the_folders_footprint() {
        let mut e = crate::testutil::env_with(&[("dup/orig", 8192), ("solo/f", 12288)], |home| {
            crate::testutil::clone_file(&home.join("dup/orig"), &home.join("dup/clone"))
        });
        let app = &mut e.app;
        let sp = app.current_space.expect("the scan is done");
        assert_eq!(sp.footprint, 8192 + 12288);
        assert_eq!(app.weight_total(), 8192 + 12288);
        app.update(Action::CycleSort); // by what frees
        app.update(Action::CycleSort); // by name
        app.update(Action::CycleSort); // by size: the logical total
        assert_eq!(app.sort, SortBy::Size);
        assert_eq!(app.weight_total(), app.current().size());
    }

    #[test]
    fn review_reports_what_frees_now_what_snapshots_hold_and_what_stays() {
        let mut e = clone_env();
        let app = &mut e.app;
        select(app, "proj");
        app.update(Action::ToggleMark);
        crate::testutil::open_review_ready(app);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!(r.space.shared_outside, 8192);
                assert_eq!(r.space.frees_now + r.space.snapshots, 4096);
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
        crate::testutil::open_review_ready(app);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!(r.space.shared_outside, 0);
                assert_eq!(r.space.footprint, 12288);
                assert_eq!(r.space.frees_now + r.space.snapshots, 12288);
            }
            other => panic!("expected review, got {}", other.is_some()),
        }
    }

    #[test]
    fn review_of_a_single_hard_linked_file_frees_nothing() {
        let mut e = shared_env();
        let app = &mut e.app;
        select(app, "proj");
        app.update(Action::Into);
        select(app, "pkg");
        app.update(Action::ToggleMark);
        crate::testutil::open_review_ready(app);
        match &app.dialog {
            Some(Dialog::Review(r)) => {
                assert_eq!((r.space.frees_now, r.space.shared_outside), (0, 8192));
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

    fn frees_env() -> crate::testutil::Env {
        // orig.bin and its clone share their data; plain.bin is on its own.
        crate::testutil::env_with(&[("plain.bin", 8192), ("orig.bin", 20480)], |home| {
            crate::testutil::clone_file(&home.join("orig.bin"), &home.join("clone.bin"))
        })
    }

    fn frees_of(app: &App, name: &str) -> Option<u64> {
        app.rows.iter().find(|r| r.name == name).unwrap().frees
    }

    #[test]
    fn rows_know_what_deleting_them_frees() {
        let mut e = frees_env();
        let app = &mut e.app;
        assert_eq!(
            frees_of(app, "orig.bin"),
            Some(0),
            "a clone's data stays shared"
        );
        assert_eq!(frees_of(app, "clone.bin"), Some(0));
        let plain = frees_of(app, "plain.bin").expect("known once the scan is done");
        assert!(plain <= 8192);
    }

    #[test]
    fn sorting_by_frees_puts_what_really_frees_first() {
        let mut e = frees_env();
        let app = &mut e.app;
        // By on-disk size the two cloned files (20 KB each) come first.
        assert_ne!(names(app)[0], "plain.bin");
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Frees);
        let frees: Vec<u64> = app.rows.iter().map(|r| r.frees.unwrap_or(0)).collect();
        assert!(
            frees.windows(2).all(|w| w[0] >= w[1]),
            "descending: {frees:?}"
        );
        if frees_of(app, "plain.bin").unwrap_or(0) > 0 {
            assert_eq!(
                names(app)[0],
                "plain.bin",
                "it is the only thing that frees"
            );
        }
    }

    #[test]
    fn unknown_values_sort_as_zero_and_never_panic() {
        let mut e = env(FILES);
        let app = &mut e.app;
        app.scanner.set_busy(true);
        app.refresh_rows();
        app.scanner.set_busy(false);
        assert!(
            app.rows.iter().all(|r| r.frees.is_none()),
            "unknown while scanning"
        );
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Frees);
        assert_eq!(app.rows.len(), 3);
        app.refresh_rows();
        assert!(
            app.rows.iter().all(|r| r.frees.is_some()),
            "known once idle"
        );
    }

    #[test]
    fn graphs_follow_what_frees_but_fall_back_when_nothing_does() {
        // Only a clone pair: nothing frees, so a frees-weighted graph would be empty.
        let mut e = crate::testutil::env_with(&[("orig.bin", 8192)], |home| {
            crate::testutil::clone_file(&home.join("orig.bin"), &home.join("clone.bin"))
        });
        let app = &mut e.app;
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Frees);
        assert_eq!(app.current_space.unwrap().frees_now, 0);
        assert_eq!(app.weight_total(), app.current_space.unwrap().footprint);
        let row = app.rows[0].clone();
        assert_eq!(app.weight(&row), row.disk);

        // With something that does free, the graph weighs by what frees.
        let mut e = frees_env();
        let app = &mut e.app;
        app.update(Action::CycleSort);
        let frees_total = app.current_space.unwrap().frees_now;
        if frees_total > 0 {
            assert_eq!(app.weight_total(), frees_total);
            let plain = app
                .rows
                .iter()
                .find(|r| r.name == "plain.bin")
                .unwrap()
                .clone();
            assert_eq!(app.weight(&plain), plain.frees.unwrap());
        }
    }

    // --- Caches & Projects screen ------------------------------------

    use crate::targets::{Line, SectionKind};
    use crate::testutil::{env_with, wait_tools};

    const TARGET_FILES: &[(&str, usize)] = &[
        ("shop/package.json", 10),
        ("shop/node_modules/a/i.js", 30_000),
        ("shop/.next/c", 10_000),
        ("api/Cargo.toml", 10),
        ("api/target/x", 90_000),
        ("Library/Caches/Spotify/blob", 50_000),
    ];

    fn targets_app() -> crate::testutil::Env {
        let mut e = env_with(TARGET_FILES, |_| {});
        e.app.update(Action::GoTo(Screen::Targets));
        wait_tools(&mut e.app);
        e
    }

    fn line_names(app: &App) -> Vec<String> {
        app.tlines
            .iter()
            .map(|l| match *l {
                Line::Section(SectionKind::Projects) => "PROJECTS".to_string(),
                Line::Section(SectionKind::Tools) => "TOOLS".to_string(),
                Line::Group(g) => app.targets.groups[g].title.clone(),
                Line::Row { group, row } => app
                    .targets
                    .row(group, row)
                    .row
                    .name
                    .to_string_lossy()
                    .into_owned(),
            })
            .collect()
    }

    fn put_cursor_on(app: &mut App, name: &str) {
        let at = line_names(app)
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no line {name} in {:?}", line_names(app)));
        app.tcursor = at;
    }

    #[test]
    fn targets_screen_lists_projects_before_tools() {
        let e = targets_app();
        let names = line_names(&e.app);
        assert_eq!(names[0], "PROJECTS");
        let tools = names.iter().position(|n| n == "TOOLS").unwrap();
        assert!(names[..tools].contains(&"~/shop".to_string()));
        assert!(names[..tools].contains(&"target".to_string()));
        assert!(names[tools..].contains(&"App caches".to_string()));
        assert!(names[tools..].contains(&"Spotify".to_string()));
    }

    #[test]
    fn tool_discovery_never_blocks_the_screen_switch() {
        let mut e = env_with(TARGET_FILES, |_| {});
        e.app.update(Action::GoTo(Screen::Targets));
        assert!(e.app.tools_looking(), "discovery runs in the background");
        // Project artifacts are listed once the list is built, even while
        // the installed tools are still being looked up.
        e.app.settle_targets();
        assert!(line_names(&e.app).contains(&"node_modules".to_string()));
        wait_tools(&mut e.app);
        assert!(!e.app.tools_looking());
    }

    #[test]
    fn h_folds_a_group_and_l_unfolds_it() {
        let mut e = targets_app();
        let app = &mut e.app;
        put_cursor_on(app, "target");
        app.focus = Focus::Main;
        app.update(Action::Out);
        assert_eq!(
            line_names(app)[app.tcursor],
            "~/api",
            "cursor moves to the group"
        );
        assert!(!line_names(app).contains(&"target".to_string()));
        app.update(Action::Into);
        assert!(line_names(app).contains(&"target".to_string()));
    }

    #[test]
    fn a_fold_survives_a_rebuild() {
        let mut e = targets_app();
        let app = &mut e.app;
        put_cursor_on(app, "~/api");
        app.focus = Focus::Main;
        app.update(Action::Out);
        app.update(Action::Tick);
        for _ in 0..30 {
            app.update(Action::Tick);
        }
        assert!(!line_names(app).contains(&"target".to_string()));
    }

    #[test]
    fn enter_on_a_row_opens_it_in_the_explorer() {
        let mut e = targets_app();
        let app = &mut e.app;
        put_cursor_on(app, "node_modules");
        app.focus = Focus::Main;
        app.update(Action::Into);
        assert_eq!(app.screen, Screen::Explorer);
        assert_eq!(app.current().path, e.home.join("shop"));
        assert_eq!(app.selected_row().unwrap().name, "node_modules");
    }

    #[test]
    fn sort_cycle_on_targets_follows_the_header_order() {
        let mut e = targets_app();
        let app = &mut e.app;
        assert_eq!(app.sort, SortBy::Disk);
        let mut seen = Vec::new();
        for _ in 0..4 {
            app.update(Action::CycleSort);
            seen.push(app.sort);
        }
        assert_eq!(
            seen,
            [SortBy::Frees, SortBy::Name, SortBy::Size, SortBy::Disk]
        );
        app.update(Action::CycleSort);
        app.update(Action::CycleSort);
        assert_eq!(app.sort, SortBy::Name);
        let names = line_names(app);
        let api = names.iter().position(|n| n == "~/api").unwrap();
        let shop = names.iter().position(|n| n == "~/shop").unwrap();
        assert!(api < shop, "A to Z by name");
    }

    #[test]
    fn the_sidebar_has_one_caches_and_projects_entry() {
        let labels: Vec<_> = SIDEBAR.iter().map(|s| s.label).collect();
        assert_eq!(labels.iter().filter(|l| l.contains("Caches")).count(), 1);
        assert!(!labels.contains(&"Projects"));
        let entry = SIDEBAR
            .iter()
            .find(|s| s.label == "Caches & Projects")
            .unwrap();
        assert_eq!(entry.screen, Some(Screen::Targets));
    }

    #[test]
    fn the_filter_narrows_target_rows_and_clear_restores_them() {
        let mut e = targets_app();
        let app = &mut e.app;
        app.focus = Focus::Main;
        app.update(Action::FilterOpen);
        assert!(app.filter.editing);
        for c in "spot".chars() {
            app.update(Action::FilterChar(c));
        }
        let names = line_names(app);
        assert_eq!(names, ["TOOLS", "App caches", "Spotify"]);
        app.update(Action::FilterClear);
        assert!(line_names(app).len() > 3);
    }

    #[test]
    fn leaving_targets_drops_its_filter() {
        let mut e = targets_app();
        let app = &mut e.app;
        app.focus = Focus::Main;
        app.update(Action::FilterOpen);
        app.update(Action::FilterChar('x'));
        app.update(Action::GoTo(Screen::Explorer));
        assert!(!app.filter.active());
        assert_eq!(app.rows.len(), app.all_rows.len());
    }

    #[test]
    fn rescan_on_targets_rescans_everything() {
        let mut e = targets_app();
        let app = &mut e.app;
        app.focus = Focus::Main;
        app.update(Action::Rescan);
        assert_eq!(app.stack.len(), 1);
        assert!(
            app.notice
                .as_deref()
                .unwrap_or("")
                .contains("Rescanning everything")
        );
    }

    // --- shared selection and joint frees ------------------------------

    use crate::testutil::{clone_file, wait_sets};

    /// A cache folder `Store` and two projects whose node_modules clone it.
    fn shared_app() -> crate::testutil::Env {
        let mut e = env_with(
            &[
                ("Library/Caches/Store/f", 200_000),
                ("p1/package.json", 10),
                ("p2/package.json", 10),
            ],
            |home| {
                clone_file(
                    &home.join("Library/Caches/Store/f"),
                    &home.join("p1/node_modules/f"),
                );
                clone_file(
                    &home.join("Library/Caches/Store/f"),
                    &home.join("p2/node_modules/f"),
                );
            },
        );
        e.app.update(Action::GoTo(Screen::Targets));
        wait_tools(&mut e.app);
        e.app.focus = Focus::Main;
        e
    }

    fn cursor_on_path(app: &mut App, path: &Path) {
        let at = app
            .tlines
            .iter()
            .position(|l| match *l {
                Line::Row { group, row } => app.targets.row(group, row).row.path == path,
                _ => false,
            })
            .unwrap_or_else(|| panic!("no row for {}", path.display()));
        app.tcursor = at;
    }

    fn mark_target(app: &mut App, path: &Path) {
        cursor_on_path(app, path);
        app.update(Action::ToggleMark);
    }

    #[test]
    fn a_mark_made_on_targets_is_a_mark_in_the_explorer_selection() {
        let mut e = shared_app();
        let nm = e.home.join("p1/node_modules");
        mark_target(&mut e.app, &nm);
        assert!(e.app.marks.contains_key(&nm));
        assert_eq!(e.app.mark_state(&nm), MarkState::Marked);
    }

    #[test]
    fn a_mark_made_in_the_explorer_shows_on_targets() {
        let mut e = shared_app();
        let (p1, nm) = (e.home.join("p1"), e.home.join("p1/node_modules"));
        e.app.update(Action::GoTo(Screen::Explorer));
        select(&mut e.app, "p1");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::GoTo(Screen::Targets));
        assert_eq!(e.app.mark_state(&nm), MarkState::Covered(p1));
    }

    #[test]
    fn marking_a_parent_in_the_explorer_replaces_a_child_marked_on_targets() {
        let mut e = shared_app();
        let nm = e.home.join("p1/node_modules");
        mark_target(&mut e.app, &nm);
        e.app.update(Action::GoTo(Screen::Explorer));
        select(&mut e.app, "p1");
        e.app.update(Action::ToggleMark);
        let keys: Vec<_> = e.app.marks.keys().cloned().collect();
        assert_eq!(keys, vec![e.home.join("p1")]);
    }

    #[test]
    fn marking_inside_a_marked_folder_is_refused_on_targets() {
        let mut e = shared_app();
        e.app.update(Action::GoTo(Screen::Explorer));
        select(&mut e.app, "p1");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::GoTo(Screen::Targets));
        let nm = e.home.join("p1/node_modules");
        mark_target(&mut e.app, &nm);
        assert!(
            e.app
                .notice
                .as_deref()
                .unwrap_or("")
                .contains("already covered")
        );
        assert_eq!(e.app.marks.len(), 1);
    }

    fn with_command_entries(e: &mut crate::testutil::Env) {
        use crate::catalog::{CommandSpec, Mode, ToolEntry};
        let mk = |id: &str, rel: &str, mode: Mode| ToolEntry {
            id: id.to_string(),
            tool: "T".to_string(),
            label: id.to_string(),
            path: e.home.join(rel),
            regen: "again".to_string(),
            mode,
            note: None,
        };
        e.app.tool_entries = vec![
            mk(
                "cmd",
                "Library/Caches/Store",
                Mode::Command(CommandSpec::new("/x/tool", &["clean"])),
            ),
            mk("folder", "p1/node_modules", Mode::Folder),
        ];
        e.app.rebuild_targets();
        e.app.settle_targets();
    }

    #[test]
    fn bulk_group_mark_marks_folder_rows_only() {
        let mut e = shared_app();
        with_command_entries(&mut e);
        put_cursor_on(&mut e.app, "T");
        e.app.update(Action::ToggleMark);
        assert!(e.app.marks.contains_key(&e.home.join("p1/node_modules")));
        assert!(
            e.app.tool_marks.is_empty(),
            "a command is never marked in bulk"
        );
        // A second press unmarks the group's folder rows.
        e.app.update(Action::ToggleMark);
        assert!(e.app.marks.is_empty());
    }

    #[test]
    fn a_section_mark_skips_command_rows_too() {
        let mut e = shared_app();
        with_command_entries(&mut e);
        let at = e
            .app
            .tlines
            .iter()
            .position(|l| matches!(l, Line::Section(SectionKind::Tools)))
            .unwrap();
        e.app.tcursor = at;
        e.app.update(Action::ToggleMark);
        assert!(e.app.tool_marks.is_empty());
        assert!(!e.app.marks.is_empty());
    }

    #[test]
    fn a_command_row_is_marked_by_marking_that_row() {
        let mut e = shared_app();
        with_command_entries(&mut e);
        let store = e.home.join("Library/Caches/Store");
        mark_target(&mut e.app, &store);
        assert!(e.app.tool_marks.contains("cmd"));
        assert!(e.app.marks.is_empty());
        mark_target(&mut e.app, &store);
        assert!(e.app.tool_marks.is_empty());
    }

    #[test]
    fn joint_frees_rises_only_when_every_holder_is_marked() {
        let mut e = shared_app();
        let (store, n1, n2) = (
            e.home.join("Library/Caches/Store"),
            e.home.join("p1/node_modules"),
            e.home.join("p2/node_modules"),
        );
        mark_target(&mut e.app, &n1);
        wait_sets(&mut e.app);
        assert_eq!(e.app.joint_frees(), Some(0));
        mark_target(&mut e.app, &n2);
        wait_sets(&mut e.app);
        assert_eq!(e.app.joint_frees(), Some(0), "the store still holds it");
        mark_target(&mut e.app, &store);
        wait_sets(&mut e.app);
        assert!(e.app.joint_frees().unwrap() >= 200_000);
        // Keeping one holder: the figure drops again.
        mark_target(&mut e.app, &n2);
        wait_sets(&mut e.app);
        assert_eq!(e.app.joint_frees(), Some(0));
    }

    #[test]
    fn nothing_marked_frees_nothing_at_once() {
        let e = shared_app();
        assert_eq!(e.app.joint_frees(), Some(0));
        assert!(!e.app.set_space.pending);
    }

    #[test]
    fn related_folders_show_their_mark_state() {
        let mut e = shared_app();
        let (n1, n2) = (
            e.home.join("p1/node_modules"),
            e.home.join("p2/node_modules"),
        );
        mark_target(&mut e.app, &n2);
        cursor_on_path(&mut e.app, &n1);
        e.app.request_holders();
        wait_sets(&mut e.app);
        let h = e.app.holders_view();
        let paths: Vec<_> = h.iter().map(|h| h.path.clone()).collect();
        assert!(
            paths.contains(&e.home.join("Library/Caches/Store")),
            "{paths:?}"
        );
        let other = h.iter().find(|h| h.path == n2).unwrap();
        assert_eq!(other.state, MarkState::Marked);
        let store = h.iter().find(|h| h.path.ends_with("Store")).unwrap();
        assert_eq!(store.state, MarkState::Unmarked);
        assert!(
            h.iter().all(|h| h.path != n1),
            "the row itself is not listed"
        );
    }

    #[test]
    fn m_marks_every_related_holder_and_the_total_follows() {
        let mut e = shared_app();
        let n1 = e.home.join("p1/node_modules");
        cursor_on_path(&mut e.app, &n1);
        e.app.request_holders();
        wait_sets(&mut e.app);
        e.app.update(Action::MarkRelated);
        wait_sets(&mut e.app);
        for p in ["p1/node_modules", "p2/node_modules", "Library/Caches/Store"] {
            assert!(e.app.marks.contains_key(&e.home.join(p)), "{p}");
        }
        assert!(e.app.joint_frees().unwrap() >= 200_000);
    }

    // --- tool commands in the shared review -----------------------------

    use crate::catalog::command::FakeRunner;
    use crate::catalog::{CommandSpec, Mode, ToolEntry};
    use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
    use std::time::Duration;

    /// Two command rows (`Store`, `Other`) and two projects with node_modules.
    fn command_app(runner: Arc<FakeRunner>) -> crate::testutil::Env {
        let mut e = env_with(
            &[
                ("Library/Caches/Store/f", 200_000),
                ("Library/Caches/Other/f", 100_000),
                ("p1/package.json", 10),
                ("p1/node_modules/f", 50_000),
            ],
            |_| {},
        );
        e.app.runner = runner;
        let mk = |id: &str, rel: &str| ToolEntry {
            id: id.to_string(),
            tool: "Tool".to_string(),
            label: id.to_string(),
            path: e.home.join(rel),
            regen: "again".to_string(),
            mode: Mode::Command(CommandSpec::new(&format!("/x/{id}"), &["clean"])),
            note: None,
        };
        e.app.tool_entries = vec![
            mk("store", "Library/Caches/Store"),
            mk("other", "Library/Caches/Other"),
        ];
        e.app.update(Action::GoTo(Screen::Targets));
        wait_tools(&mut e.app);
        // Discovery replaced the entries with what it found; use ours.
        e.app.tool_entries = vec![
            mk("store", "Library/Caches/Store"),
            mk("other", "Library/Caches/Other"),
        ];
        e.app.rebuild_targets();
        e.app.settle_targets();
        e.app.focus = Focus::Main;
        e
    }

    fn review_of(app: &App) -> &Review {
        match &app.dialog {
            Some(Dialog::Review(r)) => r,
            other => panic!("expected the review, got {other:?}"),
        }
    }

    fn finish_of(app: &App) -> &FinishInfo {
        match &app.dialog {
            Some(Dialog::Finish(f)) => f,
            other => panic!("expected the finish screen, got {other:?}"),
        }
    }

    #[test]
    fn the_review_lists_folders_and_commands_together() {
        let mut e = command_app(Arc::new(FakeRunner::new(&[])));
        let (nm, store) = (
            e.home.join("p1/node_modules"),
            e.home.join("Library/Caches/Store"),
        );
        mark_target(&mut e.app, &nm);
        mark_target(&mut e.app, &store);
        e.app.update(Action::OpenReview);
        let r = review_of(&e.app);
        assert_eq!(r.items.len(), 1);
        assert_eq!(r.items[0].path, nm);
        assert_eq!(r.commands.len(), 1);
        assert_eq!(r.commands[0].line, "store clean");
        assert_eq!(r.commands[0].spec.program, "/x/store");
    }

    #[test]
    fn a_review_opens_from_the_explorer_with_marks_made_on_targets() {
        let mut e = command_app(Arc::new(FakeRunner::new(&[])));
        let nm = e.home.join("p1/node_modules");
        mark_target(&mut e.app, &nm);
        e.app.update(Action::GoTo(Screen::Explorer));
        e.app.update(Action::OpenReview);
        assert_eq!(review_of(&e.app).items.len(), 1);
    }

    #[test]
    fn review_total_matches_the_joint_figure() {
        let mut e = shared_app();
        for p in ["p1/node_modules", "p2/node_modules", "Library/Caches/Store"] {
            mark_target(&mut e.app, &e.home.join(p));
        }
        wait_sets(&mut e.app);
        let joint = e.app.joint_frees().unwrap();
        assert!(joint >= 200_000);
        e.app.update(Action::OpenReview);
        assert_eq!(review_of(&e.app).space.frees_all(), joint);
    }

    #[test]
    fn nothing_runs_without_the_confirmation() {
        let fake = Arc::new(FakeRunner::new(&[]));
        let mut e = command_app(fake.clone());
        let store = e.home.join("Library/Caches/Store");
        mark_target(&mut e.app, &store);
        e.app.update(Action::OpenReview);
        e.app.update(Action::Cancel);
        std::thread::sleep(Duration::from_millis(50));
        assert!(fake.calls().is_empty());
        assert!(e.app.tool_marks.contains("store"), "the mark stays");
    }

    #[test]
    fn commands_run_after_the_folders_are_moved() {
        let moved_first = Arc::new(AtomicBool::new(false));
        let watch = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let (flag, w) = (Arc::clone(&moved_first), Arc::clone(&watch));
        let fake = Arc::new(FakeRunner::new(&[]).probing(move |_| {
            if let Some(p) = w.lock().unwrap().as_ref() {
                flag.store(!p.exists(), AtomicOrdering::SeqCst);
            }
        }));
        let mut e = command_app(fake.clone());
        let nm = e.home.join("p1/node_modules");
        *watch.lock().unwrap() = Some(nm.clone());
        mark_target(&mut e.app, &nm);
        mark_target(&mut e.app, &e.home.join("Library/Caches/Store"));
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        assert!(
            moved_first.load(AtomicOrdering::SeqCst),
            "the folder was already moved when the command started"
        );
        assert_eq!(fake.calls().len(), 1);
        assert!(!nm.exists());
    }

    #[test]
    fn a_failing_command_neither_stops_the_next_nor_undoes_the_moves() {
        let fake = Arc::new(FakeRunner::new(&[]).failing(&["store clean"]));
        let mut e = command_app(fake.clone());
        let nm = e.home.join("p1/node_modules");
        for p in [
            nm.clone(),
            e.home.join("Library/Caches/Store"),
            e.home.join("Library/Caches/Other"),
        ] {
            mark_target(&mut e.app, &p);
        }
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        assert_eq!(fake.calls().len(), 2, "both commands ran");
        assert!(!nm.exists(), "the folder stays moved");
        let f = finish_of(&e.app);
        let oks: Vec<_> = f.commands.iter().map(|c| (c.line.as_str(), c.ok)).collect();
        assert_eq!(oks, [("store clean", false), ("other clean", true)]);
    }

    #[test]
    fn x_moves_then_empties_the_folders_and_still_runs_the_commands() {
        let fake = Arc::new(FakeRunner::new(&[]));
        let mut e = command_app(fake.clone());
        let nm = e.home.join("p1/node_modules");
        mark_target(&mut e.app, &nm);
        mark_target(&mut e.app, &e.home.join("Library/Caches/Store"));
        e.app.update(Action::OpenReview);
        e.app.update(Action::ConfirmIrreversible);
        assert!(matches!(e.app.dialog, Some(Dialog::Irreversible(_))));
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        assert!(e.app.holding.list().unwrap().is_empty(), "emptied for good");
        assert!(!nm.exists());
        assert_eq!(fake.calls().len(), 1);
        assert!(finish_of(&e.app).emptied.size > 0);
    }

    #[test]
    fn a_review_of_only_commands_runs_and_clears_their_marks() {
        let fake = Arc::new(FakeRunner::new(&[]));
        let mut e = command_app(fake.clone());
        mark_target(&mut e.app, &e.home.join("Library/Caches/Store"));
        e.app.update(Action::OpenReview);
        assert!(review_of(&e.app).items.is_empty());
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        assert_eq!(fake.calls().len(), 1);
        assert!(e.app.tool_marks.is_empty());
        assert_eq!(finish_of(&e.app).commands.len(), 1);
    }

    #[test]
    fn the_ui_does_not_wait_for_a_slow_command() {
        let fake = Arc::new(FakeRunner::new(&[]).slow(Duration::from_millis(400)));
        let mut e = command_app(fake);
        mark_target(&mut e.app, &e.home.join("Library/Caches/Store"));
        e.app.update(Action::OpenReview);
        let started = std::time::Instant::now();
        e.app.update(Action::Confirm);
        e.app.update(Action::Tick);
        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(matches!(e.app.dialog, Some(Dialog::Deleting(_))));
        settle(&mut e.app);
    }

    #[test]
    fn quitting_waits_for_a_running_command() {
        let fake = Arc::new(FakeRunner::new(&[]).slow(Duration::from_millis(300)));
        let mut e = command_app(fake);
        mark_target(&mut e.app, &e.home.join("Library/Caches/Store"));
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        e.app.update(Action::Quit);
        assert!(!e.app.should_quit, "the command is still running");
        settle(&mut e.app);
        for _ in 0..20 {
            e.app.update(Action::Tick);
        }
        assert!(e.app.should_quit);
    }

    #[test]
    fn a_review_opened_mid_calculation_fills_its_figures_in_later() {
        let mut e = shared_app();
        let nm = e.home.join("p1/node_modules");
        mark_target(&mut e.app, &nm);
        assert!(e.app.set_space.pending, "the figures are being worked out");
        e.app.update(Action::OpenReview);
        assert!(
            !review_of(&e.app).space_ready,
            "nothing is computed on the UI thread"
        );
        wait_sets(&mut e.app);
        assert!(review_of(&e.app).space_ready);
    }

    #[test]
    fn the_list_is_built_off_the_ui_thread() {
        let mut e = env_with(TARGET_FILES, |_| {});
        e.app.update(Action::GoTo(Screen::Targets));
        assert!(e.app.targets_building());
        assert!(
            e.app.tlines.is_empty(),
            "the first paint does not wait for the walk"
        );
        e.app.settle_targets();
        assert!(!e.app.tlines.is_empty());
    }
}
