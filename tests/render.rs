use std::fs;
use std::path::Path;
use std::time::Duration;

use maccleanup::accounting::Accounting;
use maccleanup::app::{Action, App, Dialog, GraphMode, Screen};
use maccleanup::fmt::human_size;
use maccleanup::glyphs::{GlyphMode, Glyphs};
use maccleanup::safety::Policy;
use maccleanup::scan::Scanner;
use maccleanup::softdelete::Holding;
use maccleanup::ui;
use maccleanup::volume;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

struct Env {
    _dir: tempfile::TempDir,
    app: App,
}

/// Runs nothing and finds nothing: tool discovery is deterministic in tests.
struct NoTools;

impl maccleanup::catalog::CommandRunner for NoTools {
    fn run(&self, _: &maccleanup::catalog::CommandSpec) -> maccleanup::catalog::CommandResult {
        maccleanup::catalog::CommandResult {
            ok: true,
            output_tail: String::new(),
        }
    }
    fn query(&self, _: &maccleanup::catalog::CommandSpec) -> Option<String> {
        None
    }
    fn available(&self, _: &str) -> bool {
        false
    }
}

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn make_app(files: &[(&str, usize)], mode: GlyphMode) -> Env {
    make_app_with(files, mode, |_| {})
}

fn make_app_with(files: &[(&str, usize)], mode: GlyphMode, setup: impl FnOnce(&Path)) -> Env {
    make_app_full(files, mode, false, setup)
}

fn make_app_full(
    files: &[(&str, usize)],
    mode: GlyphMode,
    animations: bool,
    setup: impl FnOnce(&Path),
) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    for (rel, size) in files {
        write(&home.join(rel), *size);
    }
    setup(&home);
    let hold = base.join("hold");
    let scanner = Scanner::new(2);
    let root = scanner.scan_root(&home);
    assert!(scanner.wait_idle(Duration::from_secs(20)));
    let policy = Policy::new(&home, &hold);
    let holding = Holding::open(hold).unwrap();
    let free = volume::free_space(&home).unwrap();
    let app = App::new(
        scanner,
        root,
        policy,
        holding,
        Accounting::new(free),
        Glyphs::new(mode),
        animations,
    )
    .with_runner(std::sync::Arc::new(NoTools));
    Env { _dir: dir, app }
}

const FILES: &[(&str, usize)] = &[
    ("big/f", 5_000_000),
    ("small.txt", 1500),
    ("mid/f", 250_000),
];

fn explorer(mode: GlyphMode) -> Env {
    let mut e = make_app(FILES, mode);
    e.app.update(Action::GoTo(Screen::Explorer));
    e
}

fn render_cells(app: &mut App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    term.backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect()
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let cells = render_cells(app, w, h);
    let mut out = String::new();
    for row in cells.chunks(w.max(1) as usize) {
        out.push_str(&row.concat());
        out.push('\n');
    }
    out
}

/// Open the review and wait until its space figures are worked out.
fn open_review(app: &mut App) {
    app.update(Action::OpenReview);
    for _ in 0..400 {
        match &app.dialog {
            Some(Dialog::Review(r)) if !r.space_ready => {
                app.update(Action::Tick);
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => return,
        }
    }
}

fn mark(app: &mut App, name: &str) {
    let idx = app
        .rows
        .iter()
        .position(|r| r.name.to_string_lossy() == name)
        .unwrap();
    app.cursor = idx;
    app.update(Action::ToggleMark);
}

#[test]
fn too_small_shows_notice_and_does_not_panic() {
    for (w, h) in [(20, 5), (59, 30), (100, 13), (0, 0), (1, 1)] {
        let mut e = explorer(GlyphMode::Nerd);
        let text = render(&mut e.app, w, h);
        if w >= 20 && h >= 5 {
            assert!(text.contains("too small"), "{w}x{h}:\n{text}");
        }
    }
}

#[test]
fn minimum_size_renders_the_real_ui() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, ui::MIN_WIDTH, ui::MIN_HEIGHT);
    assert!(!text.contains("too small"), "{text}");
    assert!(text.contains("Explorer"));
}

#[test]
fn home_shows_free_space_and_pending() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    let free = e.app.free_now;
    let text = render(&mut e.app, 100, 30);
    assert!(text.contains("Free"), "{text}");
    assert!(text.contains(&human_size(free)), "{text}");
    assert!(text.contains("pending"), "{text}");
}

#[test]
fn home_lists_biggest_folders() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    let text = render(&mut e.app, 100, 30);
    assert!(text.contains("Biggest folders"), "{text}");
    assert!(text.contains("big"), "{text}");
}

#[test]
fn explorer_lists_rows_with_sizes() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("big"), "{text}");
    assert!(text.contains("small.txt"), "{text}");
    assert!(text.contains(&human_size(5_000_000)), "{text}");
    assert!(text.contains("▸"), "cursor glyph missing:\n{text}");
}

#[test]
fn marked_footer_text() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "big");
    let text = render(&mut e.app, 130, 30);
    assert!(text.contains("marked: 1 items"), "{text}");
    assert!(text.contains("not deleted yet"), "{text}");
}

#[test]
fn graph_modes_render() {
    let mut e = explorer(GlyphMode::Nerd);
    e.app.graph = GraphMode::Bars;
    let bars = render(&mut e.app, 120, 30);
    assert!(bars.contains("selected:"), "{bars}");

    e.app.graph = GraphMode::Treemap;
    let tree = render(&mut e.app, 120, 30);
    assert!(tree.contains("selected:"), "{tree}");
    assert!(tree.contains("Treemap"), "{tree}");

    e.app.graph = GraphMode::Hidden;
    let hidden = render(&mut e.app, 120, 30);
    assert!(!hidden.contains("selected:"), "{hidden}");
    assert!(!hidden.contains("Treemap"), "{hidden}");
    assert!(!hidden.contains("Distribution"), "{hidden}");
}

#[test]
fn treemap_resize_changes_panel_width() {
    let mut e = explorer(GlyphMode::Nerd);
    e.app.graph = GraphMode::Treemap;
    let narrow = render(&mut e.app, 120, 30);
    for _ in 0..4 {
        e.app.update(Action::GrowGraph);
    }
    let wide = render(&mut e.app, 120, 30);
    assert_ne!(narrow, wide);
}

#[test]
fn ascii_mode_has_only_ascii_cells() {
    let assert_ascii = |label: &str, cells: Vec<String>| {
        for c in &cells {
            assert!(c.is_ascii(), "{label}: non-ascii cell {c:?}");
        }
    };

    let mut e = make_app(FILES, GlyphMode::Ascii);
    assert_ascii("home", render_cells(&mut e.app, 100, 30));

    e.app.update(Action::GoTo(Screen::Explorer));
    for mode in [GraphMode::Bars, GraphMode::Treemap, GraphMode::Hidden] {
        e.app.graph = mode;
        assert_ascii(
            &format!("explorer {mode:?}"),
            render_cells(&mut e.app, 120, 30),
        );
    }
    mark(&mut e.app, "big");
    e.app.graph = GraphMode::Treemap;
    assert_ascii("treemap with a mark", render_cells(&mut e.app, 120, 30));

    open_review(&mut e.app);
    assert!(matches!(e.app.dialog, Some(Dialog::Review(_))));
    assert_ascii("review", render_cells(&mut e.app, 120, 30));
    e.app.update(Action::ConfirmIrreversible);
    assert_ascii("irreversible", render_cells(&mut e.app, 120, 30));
    e.app.update(Action::Cancel);

    e.app.update(Action::GoTo(Screen::Holding));
    assert_ascii("holding", render_cells(&mut e.app, 100, 30));
}

#[test]
fn review_dialog_shows_total_and_keys() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "big");
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Enter"), "{text}");
    assert!(text.contains("Esc"), "{text}");
    assert!(text.contains("[ x ]"), "{text}");
    assert!(text.contains(&human_size(5_001_500)), "{text}");
    assert!(text.contains("not freed"), "{text}");
}

#[test]
fn irreversible_dialog_warns() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "big");
    open_review(&mut e.app);
    e.app.update(Action::ConfirmIrreversible);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("cannot be undone"), "{text}");
    assert!(text.contains("[ y ]"), "{text}");
}

#[test]
fn deleting_dialog_shows_progress() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "big");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    // Render before ticking so the dialog is still in the working state.
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("0 / 1") || text.contains("1 / 1"), "{text}");
    // Let the job finish so the sandbox can be removed cleanly.
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn finish_dialog_shows_congratulation() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "big");
    open_review(&mut e.app);
    e.app.update(Action::ConfirmIrreversible);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(e.app.dialog, Some(Dialog::Finish(_))));
    // Let the count-up finish, then pretend the volume reported 7 MB more
    // free space (ticks re-read the real volume, so set it afterwards).
    for _ in 0..60 {
        e.app.update(Action::Tick);
    }
    e.app.free_now = e.app.accounting.free_at_start() + 7_000_000;
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Congratulations"), "{text}");
    assert!(text.contains("7.0 MB"), "{text}");
}

#[test]
fn finish_dialog_explains_when_nothing_was_freed() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::ConfirmIrreversible);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..60 {
        e.app.update(Action::Tick);
    }
    e.app.free_now = e.app.accounting.free_at_start();
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Nothing freed yet"), "{text}");
    assert!(!text.contains("Congratulations"), "{text}");
}

#[test]
fn holding_screen_lists_items() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    e.app.update(Action::Cancel);
    e.app.update(Action::GoTo(Screen::Holding));
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("small.txt"), "{text}");
    assert!(text.contains("restore"), "{text}");
    // Long paths keep their tail, so the file name stays visible.
    assert!(text.contains("home/small.txt"), "{text}");
}

#[test]
fn holding_screen_empty_state() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Holding));
    let text = render(&mut e.app, 100, 30);
    assert!(text.contains("empty"), "{text}");
}

#[test]
fn fda_banner_when_flagged() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    e.app.fda_missing = true;
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Full Disk Access"), "{text}");
}

#[test]
fn no_fda_banner_by_default() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    let text = render(&mut e.app, 120, 30);
    assert!(!text.contains("Full Disk Access"), "{text}");
}

#[test]
fn sidebar_shows_entries_and_pending() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 100, 30);
    for label in ["Home", "Explorer", "Leftovers", "ToBeDeleted", "pending"] {
        assert!(text.contains(label), "missing {label}:\n{text}");
    }
}

#[test]
fn layout_is_published_for_mouse_mapping() {
    let mut e = explorer(GlyphMode::Nerd);
    render(&mut e.app, 100, 30);
    assert!(e.app.layout.sidebar.width > 0);
    assert!(e.app.layout.main.width > 0);
    assert!(e.app.layout.list_rows > 0);
    assert!(e.app.layout.list_top > e.app.layout.main.y);
}

#[test]
fn notice_text_replaces_footer_hints() {
    let mut e = explorer(GlyphMode::Nerd);
    open_review(&mut e.app); // nothing marked: sets a notice
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Nothing marked yet"), "{text}");
}

#[test]
fn long_and_wide_names_do_not_break_layout() {
    let long = "x".repeat(200);
    let files: Vec<(String, usize)> = vec![
        (format!("{long}/f"), 100),
        ("日本語のフォルダ/f".to_string(), 200),
    ];
    let refs: Vec<(&str, usize)> = files.iter().map(|(n, s)| (n.as_str(), *s)).collect();
    let mut e = make_app(&refs, GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Explorer));
    for mode in [GraphMode::Bars, GraphMode::Treemap, GraphMode::Hidden] {
        e.app.graph = mode;
        let cells = render_cells(&mut e.app, 90, 20);
        assert_eq!(cells.len(), 90 * 20);
    }
}

#[test]
fn drawing_records_whether_the_terminal_is_too_small() {
    let mut e = explorer(GlyphMode::Nerd);
    render(&mut e.app, 20, 5);
    assert!(e.app.too_small);
    render(&mut e.app, 100, 30);
    assert!(!e.app.too_small);
    render(&mut e.app, 0, 0);
    assert!(e.app.too_small);
}

fn find_text(app: &mut App, w: u16, h: u16, needle: &str) -> Option<ratatui::style::Style> {
    find_cell(app, w, h, needle, 0)
}

/// The style of the cell `offset` characters into the first occurrence of
/// `needle` (scanning top to bottom, left to right).
fn find_cell(
    app: &mut App,
    w: u16,
    h: u16,
    needle: &str,
    offset: usize,
) -> Option<ratatui::style::Style> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let needle_chars: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(needle_chars.len() as u16) {
            if needle_chars
                .iter()
                .enumerate()
                .all(|(i, c)| buf[(x + i as u16, y)].symbol() == c)
            {
                let cell = &buf[(x + offset as u16, y)];
                return Some(Style::default().fg(cell.fg).add_modifier(cell.modifier));
            }
        }
    }
    None
}

use ratatui::style::{Color, Style};

#[test]
fn explorer_header_marks_the_sorted_column() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 120, 30);
    assert!(
        text.contains("On disk ▼"),
        "default is on-disk, descending:\n{text}"
    );
    assert!(
        !text.contains("Size ▼") && !text.contains("Name ▲"),
        "{text}"
    );

    e.app.update(Action::CycleSort);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Frees ▼"), "{text}");
    assert!(!text.contains("On disk ▼"), "{text}");

    e.app.update(Action::CycleSort);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Name ▲"), "{text}");
    assert!(!text.contains("Frees ▼"), "{text}");

    e.app.update(Action::CycleSort);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Size ▼"), "{text}");
    assert!(!text.contains("Name ▲"), "{text}");
}

#[test]
fn explorer_header_arrows_are_ascii_in_ascii_mode() {
    let mut e = explorer(GlyphMode::Ascii);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("On disk v"), "{text}");
    e.app.update(Action::CycleSort);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Frees v"), "{text}");
    e.app.update(Action::CycleSort);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Name ^"), "{text}");
}

#[test]
fn sorted_column_is_white_and_the_others_gray() {
    let mut e = explorer(GlyphMode::Nerd);
    // Sorted by on-disk size: that header is white, the Size header is gray.
    let disk = find_text(&mut e.app, 120, 30, "On disk").unwrap();
    let size = find_text(&mut e.app, 120, 30, "Size").unwrap();
    assert_eq!(disk.fg, Some(Color::White));
    assert_eq!(size.fg, Some(Color::DarkGray));

    e.app.update(Action::CycleSort); // by Frees
    let disk = find_text(&mut e.app, 120, 30, "On disk").unwrap();
    let frees = find_text(&mut e.app, 120, 30, "Frees").unwrap();
    assert_eq!(frees.fg, Some(Color::White));
    assert_eq!(disk.fg, Some(Color::DarkGray));

    e.app.update(Action::CycleSort); // by Name
    e.app.update(Action::CycleSort); // by Size
    let frees = find_text(&mut e.app, 120, 30, "Frees").unwrap();
    let size = find_text(&mut e.app, 120, 30, "Size").unwrap();
    assert_eq!(size.fg, Some(Color::White));
    assert_eq!(frees.fg, Some(Color::DarkGray));
}

#[test]
fn sorted_column_values_are_white_and_the_other_size_column_gray() {
    let mut e = explorer(GlyphMode::Nerd);
    // small.txt is 1500 bytes but occupies 4096 on disk, so the two columns
    // print different strings we can find on screen.
    let small = e
        .app
        .rows
        .iter()
        .find(|r| r.name == "small.txt")
        .unwrap()
        .clone();
    let big_disk = human_size(small.disk);
    let big_size = human_size(small.size);
    assert_ne!(big_disk, big_size, "need distinct strings for this check");
    let disk = find_text(&mut e.app, 120, 30, &big_disk).unwrap();
    let size = find_text(&mut e.app, 120, 30, &big_size).unwrap();
    assert_eq!(disk.fg, Some(Color::White));
    assert_eq!(size.fg, Some(Color::DarkGray));
}

#[test]
fn home_scan_line_shows_on_disk_size_too() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    let stats = e.app.scanner.stats();
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains(&human_size(stats.bytes)), "{text}");
    assert!(text.contains(&human_size(stats.disk)), "{text}");
    assert!(text.contains("on disk"), "{text}");
}

#[test]
fn review_dialog_shows_on_disk_total() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    let text = render(&mut e.app, 120, 30);
    // small.txt: 1.5 KB logical, 4.1 KB on disk.
    assert!(text.contains("1.5 KB"), "{text}");
    assert!(text.contains("4.1 KB"), "{text}");
    assert!(text.contains("on disk"), "{text}");
}

#[test]
fn marked_footer_shows_on_disk_size() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    let text = render(&mut e.app, 140, 30);
    assert!(
        text.contains("marked: 1 items, 1.5 KB (4.1 KB on disk)"),
        "{text}"
    );
}

#[test]
fn sidebar_pending_shows_on_disk_size() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    e.app.update(Action::Cancel);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("pending"), "{text}");
    assert!(text.contains("4.1 KB disk"), "{text}");
}

#[test]
fn home_pending_and_folders_show_on_disk_size() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("pending 0 B (0 B on disk)"), "{text}");
    // Biggest folders: logical and on-disk columns for big/ (5.0 MB).
    let line = text.lines().find(|l| l.contains("big")).unwrap_or("");
    assert!(
        line.matches("MB").count() >= 2,
        "size and disk columns: {line}"
    );
}

#[test]
fn holding_screen_shows_on_disk_size() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    e.app.update(Action::Cancel);
    e.app.update(Action::GoTo(Screen::Holding));
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("1.5 KB (4.1 KB on disk)"), "summary:\n{text}");
    let row = text.lines().find(|l| l.contains("small.txt")).unwrap();
    assert!(row.contains("1.5 KB") && row.contains("4.1 KB"), "{row}");
}

#[test]
fn finish_dialog_shows_on_disk_amounts() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::ConfirmIrreversible);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("moved 1.5 KB (4.1 KB on disk)"), "{text}");
    assert!(
        text.contains("deleted for good 1.5 KB (4.1 KB on disk)"),
        "{text}"
    );
}

#[test]
fn explorer_and_home_footers_mention_rescan() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("r/R rescan"), "{text}");
    e.app.update(Action::GoTo(Screen::Home));
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("r/R rescan"), "{text}");
}

fn shared_app(mode: GlyphMode) -> Env {
    let mut e = make_app_with(&[("store/pkg", 8192), ("proj/own", 4096)], mode, |home| {
        fs::hard_link(home.join("store/pkg"), home.join("proj/pkg")).unwrap()
    });
    e.app.update(Action::GoTo(Screen::Explorer));
    e
}

fn select_row(app: &mut App, name: &str) {
    app.cursor = app
        .rows
        .iter()
        .position(|r| r.name.to_string_lossy() == name)
        .unwrap();
    app.refresh_detail();
}

#[test]
fn shared_detail_is_ascii_in_ascii_mode() {
    let mut e = shared_app(GlyphMode::Ascii);
    select_row(&mut e.app, "proj");
    for c in render_cells(&mut e.app, 130, 30) {
        assert!(c.is_ascii(), "non-ascii cell {c:?}");
    }
}

#[test]
fn home_starts_with_menu_hints_and_a_focused_menu() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    assert_eq!(e.app.focus, maccleanup::app::Focus::Sidebar);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("j/k choose"), "{text}");
    assert!(text.contains("enter open"), "{text}");
}

#[test]
fn dialogs_grow_in_when_animations_are_on() {
    let mut e = make_app_full(FILES, GlyphMode::Nerd, true, |_| {});
    e.app.update(Action::GoTo(Screen::Explorer));
    for _ in 0..12 {
        e.app.update(Action::Tick); // let the screen-switch wipe finish
    }
    let before = render(&mut e.app, 120, 30).matches('╭').count();
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);

    let first = render(&mut e.app, 120, 30);
    assert!(
        !first.contains("Review before"),
        "content waits for the window:\n{first}"
    );
    assert!(!first.contains("Total"), "{first}");
    assert_eq!(
        first.matches('╭').count(),
        before + 1,
        "a small frame is already visible"
    );

    for _ in 0..3 {
        e.app.update(Action::Tick);
    }
    let mid = render(&mut e.app, 120, 30);
    assert!(!mid.contains("Review before"), "still growing:\n{mid}");

    for _ in 0..10 {
        e.app.update(Action::Tick);
    }
    let done = render(&mut e.app, 120, 30);
    assert!(done.contains("Review before moving"), "{done}");
    assert!(done.contains("Total"), "{done}");
}

#[test]
fn no_grow_animation_when_animations_are_off() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Review before moving"), "{text}");
}

const CONFETTI: [&str; 6] = ["✦", "✧", "◆", "▴", "▪", "✶"];

fn finished_with_freed_space(animations: bool, freed: bool) -> Env {
    let mut e = make_app_full(FILES, GlyphMode::Nerd, animations, |_| {});
    e.app.update(Action::GoTo(Screen::Explorer));
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::ConfirmIrreversible);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..20 {
        e.app.update(Action::Tick); // the window finishes growing
    }
    e.app.free_now = e.app.accounting.free_at_start() + if freed { 7_000_000 } else { 0 };
    e
}

#[test]
fn congratulations_has_confetti_when_animated() {
    let mut e = finished_with_freed_space(true, true);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Congratulations"), "{text}");
    assert!(
        CONFETTI.iter().any(|c| text.contains(c)),
        "no confetti:\n{text}"
    );
}

#[test]
fn no_confetti_without_animations_or_without_freed_space() {
    let mut off = finished_with_freed_space(false, true);
    let text = render(&mut off.app, 120, 30);
    assert!(text.contains("Congratulations"));
    assert!(!CONFETTI.iter().any(|c| text.contains(c)), "{text}");

    let mut nothing = finished_with_freed_space(true, false);
    let text = render(&mut nothing.app, 120, 30);
    assert!(text.contains("Nothing freed yet"), "{text}");
    assert!(!CONFETTI.iter().any(|c| text.contains(c)), "{text}");
}

#[test]
fn confetti_never_hides_the_text() {
    let mut e = finished_with_freed_space(true, true);
    for _ in 0..40 {
        e.app.update(Action::Tick);
        e.app.free_now = e.app.accounting.free_at_start() + 7_000_000;
        let text = render(&mut e.app, 120, 30);
        assert!(text.contains("Congratulations"), "{text}");
        assert!(text.contains("deleted for good"), "{text}");
    }
}

#[test]
fn confetti_is_ascii_in_ascii_mode() {
    let mut e = make_app_full(FILES, GlyphMode::Ascii, true, |_| {});
    e.app.update(Action::GoTo(Screen::Explorer));
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::ConfirmIrreversible);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..20 {
        e.app.update(Action::Tick);
    }
    e.app.free_now = e.app.accounting.free_at_start() + 7_000_000;
    for c in render_cells(&mut e.app, 120, 30) {
        assert!(c.is_ascii(), "non-ascii cell {c:?}");
    }
}

#[test]
fn emptying_shows_a_deleting_dialog_with_a_progress_bar() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    e.app.update(Action::Cancel);
    e.app.update(Action::GoTo(Screen::Holding));
    e.app.update(Action::EmptyAll);
    e.app.update(Action::Confirm);
    // Rendered before any tick: the dialog is already up, not a frozen screen.
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Deleting for good"), "{text}");
    assert!(text.contains("0 / 1 items"), "{text}");
    assert!(text.contains("0%"), "{text}");
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
}

const FILTER_FILES: &[(&str, usize)] = &[
    ("node_modules/x", 20480),
    ("notes.txt", 8192),
    ("src/main.rs", 12288),
    ("Makefile", 4096),
];

fn filter_app(mode: GlyphMode) -> Env {
    let mut e = make_app(FILTER_FILES, mode);
    e.app.update(Action::GoTo(Screen::Explorer));
    e
}

fn type_filter(app: &mut App, text: &str) {
    app.update(Action::FilterOpen);
    for c in text.chars() {
        app.update(Action::FilterChar(c));
    }
}

fn any_reversed(app: &mut App, w: u16, h: u16) -> bool {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    term.backend()
        .buffer()
        .content()
        .iter()
        .any(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED))
}

#[test]
fn the_filter_box_shows_while_typing_with_a_match_count() {
    let mut e = filter_app(GlyphMode::Nerd);
    type_filter(&mut e.app, "no");
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("/ no"), "{text}");
    assert!(text.contains("2/4"), "{text}");
    assert!(
        text.contains("notes.txt") && text.contains("node_modules"),
        "{text}"
    );
    assert!(!text.contains("Makefile"), "filtered out:\n{text}");
    assert!(!text.contains("main.rs"), "{text}");
}

#[test]
fn the_filter_box_stays_after_enter_and_says_how_to_clear_it() {
    let mut e = filter_app(GlyphMode::Nerd);
    type_filter(&mut e.app, "no");
    e.app.update(Action::FilterAccept);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("/ no"), "{text}");
    assert!(text.contains("2/4"), "{text}");
    assert!(text.contains("esc clears"), "{text}");
    assert!(!text.contains("Makefile"), "{text}");
}

#[test]
fn there_is_no_filter_box_without_a_filter() {
    let mut e = filter_app(GlyphMode::Nerd);
    let plain = render(&mut e.app, 120, 30);
    assert!(!plain.contains("esc clears"), "{plain}");
    let rows_plain = e.app.layout.list_rows;
    type_filter(&mut e.app, "no");
    render(&mut e.app, 120, 30);
    assert_eq!(
        e.app.layout.list_rows,
        rows_plain - 1,
        "the box takes one row"
    );
    e.app.update(Action::FilterClear);
    render(&mut e.app, 120, 30);
    assert_eq!(e.app.layout.list_rows, rows_plain);
}

#[test]
fn a_filter_with_no_matches_says_so() {
    let mut e = filter_app(GlyphMode::Nerd);
    type_filter(&mut e.app, "zzz");
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("no matches"), "{text}");
    assert!(text.contains("0/4"), "{text}");
}

#[test]
fn matching_letters_are_highlighted() {
    let mut e = filter_app(GlyphMode::Nerd);
    type_filter(&mut e.app, "no");
    // The list row is "● notes.txt" (the graph panel has no dot), so anchor on
    // that: offset 2 is the 'n', 3 the 'o', 4 the 't'.
    let n = find_cell(&mut e.app, 120, 30, "● notes.txt", 2).unwrap();
    assert_eq!(n.fg, Some(Color::Yellow), "'n' matches");
    assert!(n.add_modifier.contains(ratatui::style::Modifier::BOLD));
    let o = find_cell(&mut e.app, 120, 30, "● notes.txt", 3).unwrap();
    assert_eq!(o.fg, Some(Color::Yellow), "'o' matches");
    let t = find_cell(&mut e.app, 120, 30, "● notes.txt", 4).unwrap();
    assert_ne!(t.fg, Some(Color::Yellow), "'t' does not match");
}

#[test]
fn nothing_is_highlighted_without_a_filter() {
    let mut e = filter_app(GlyphMode::Nerd);
    let n = find_cell(&mut e.app, 120, 30, "● notes.txt", 2).unwrap();
    assert_ne!(n.fg, Some(Color::Yellow));
}

#[test]
fn the_editing_cursor_is_a_reversed_cell() {
    let mut e = filter_app(GlyphMode::Nerd);
    assert!(!any_reversed(&mut e.app, 120, 30));
    type_filter(&mut e.app, "no");
    assert!(any_reversed(&mut e.app, 120, 30), "cursor while editing");
    e.app.update(Action::FilterAccept);
    assert!(
        !any_reversed(&mut e.app, 120, 30),
        "no cursor once accepted"
    );
}

#[test]
fn the_filter_is_ascii_clean_in_ascii_mode() {
    let mut e = filter_app(GlyphMode::Ascii);
    type_filter(&mut e.app, "no");
    for c in render_cells(&mut e.app, 120, 30) {
        assert!(c.is_ascii(), "non-ascii cell {c:?}");
    }
    e.app.update(Action::FilterAccept);
    for c in render_cells(&mut e.app, 120, 30) {
        assert!(c.is_ascii(), "non-ascii cell {c:?}");
    }
}

#[test]
fn the_graphs_follow_the_filter() {
    for mode in [GraphMode::Bars, GraphMode::Treemap] {
        let mut e = filter_app(GlyphMode::Nerd);
        e.app.graph = mode;
        type_filter(&mut e.app, "no");
        let text = render(&mut e.app, 130, 30);
        assert!(text.contains("selected: node_modules"), "{mode:?}\n{text}");
        assert!(!text.contains("Makefile"), "{mode:?}\n{text}");
    }
}

#[test]
fn footers_explain_the_filter_keys() {
    let mut e = filter_app(GlyphMode::Nerd);
    let text = render(&mut e.app, 150, 30);
    assert!(text.contains("/ filter"), "{text}");
    type_filter(&mut e.app, "no");
    let text = render(&mut e.app, 150, 30);
    assert!(
        text.contains("enter accept") && text.contains("esc clear"),
        "{text}"
    );
    e.app.update(Action::FilterAccept);
    let text = render(&mut e.app, 150, 30);
    assert!(text.contains("/ edit filter"), "{text}");
}

#[test]
fn a_long_query_does_not_break_the_layout() {
    let mut e = filter_app(GlyphMode::Nerd);
    type_filter(&mut e.app, &"abc日本 ".repeat(40));
    let cells = render_cells(&mut e.app, 90, 20);
    assert_eq!(cells.len(), 90 * 20);
    let tiny = render_cells(&mut e.app, 61, 14);
    assert_eq!(tiny.len(), 61 * 14);
}

fn with_held_item() -> Env {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    e.app.update(Action::Cancel);
    e
}

#[test]
fn sidebar_says_when_tobedeleted_is_empty() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains(" empty"), "{text}");
    assert!(!text.contains("pending ("), "{text}");
    assert!(text.contains("\u{f014}"), "outline trash icon:\n{text}");
    assert!(!text.contains("\u{f1f8}"), "{text}");
}

#[test]
fn sidebar_shows_a_count_and_a_full_icon_when_it_holds_something() {
    let mut e = with_held_item();
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("pending (1)"), "{text}");
    assert!(text.contains("\u{f1f8}"), "full trash icon:\n{text}");
    assert!(!text.contains("\u{f014}"), "{text}");
    let icon = find_text(&mut e.app, 120, 30, "\u{f1f8}").unwrap();
    assert_eq!(icon.fg, Some(Color::Yellow), "stands out");
}

#[test]
fn the_sidebar_indicator_is_ascii_in_ascii_mode() {
    let mut e = explorer(GlyphMode::Ascii);
    for c in render_cells(&mut e.app, 120, 30) {
        assert!(c.is_ascii(), "{c:?}");
    }
    let empty = render(&mut e.app, 120, 30);
    assert!(empty.contains(" empty"), "{empty}");
}

#[test]
fn quitting_with_items_shows_the_warning_dialog() {
    let mut e = with_held_item();
    e.app.update(Action::Quit);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("still holds 1 item"), "{text}");
    assert!(text.contains("not freed"), "{text}");
    assert!(text.contains("1.5 KB (4.1 KB on disk)"), "{text}");
    for key in ["[ e ]", "[ q ]", "[ Esc ]"] {
        assert!(text.contains(key), "missing {key}:\n{text}");
    }
}

#[test]
fn the_quit_warning_is_ascii_in_ascii_mode() {
    let mut e = explorer(GlyphMode::Ascii);
    mark(&mut e.app, "small.txt");
    open_review(&mut e.app);
    e.app.update(Action::Confirm);
    for _ in 0..500 {
        if !matches!(e.app.dialog, Some(Dialog::Deleting(_))) {
            break;
        }
        e.app.update(Action::Tick);
        std::thread::sleep(Duration::from_millis(5));
    }
    e.app.update(Action::Cancel);
    e.app.update(Action::Quit);
    for c in render_cells(&mut e.app, 120, 30) {
        assert!(c.is_ascii(), "{c:?}");
    }
}

#[test]
fn explorer_detail_shows_where_the_space_would_go() {
    let mut e = shared_app(GlyphMode::Nerd);
    select_row(&mut e.app, "proj");
    let text = render(&mut e.app, 140, 30);
    let frees = text.lines().find(|l| l.contains("frees now")).unwrap_or("");
    let shared = text
        .lines()
        .find(|l| l.contains("shared with other places"))
        .unwrap_or("");
    assert!(
        frees.contains("held by snapshots"),
        "frees line: {frees}\n{text}"
    );
    assert!(shared.contains("8.2 KB"), "shared line: {shared}\n{text}");
    assert!(shared.contains("stays"), "{shared}");
}

#[test]
fn the_detail_for_a_plain_folder_shares_nothing() {
    let mut e = make_app(FILES, GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Explorer));
    select_row(&mut e.app, "mid");
    let text = render(&mut e.app, 140, 30);
    let shared = text
        .lines()
        .find(|l| l.contains("shared with other places"))
        .unwrap_or("");
    assert!(shared.contains("0 B"), "{shared}\n{text}");
}

#[test]
fn the_detail_says_what_cloning_inside_a_folder_saves() {
    let mut e = make_app_with(
        &[("dup/orig", 8192), ("solo/f", 4096)],
        GlyphMode::Nerd,
        |home| {
            let status = std::process::Command::new("cp")
                .arg("-c")
                .arg(home.join("dup/orig"))
                .arg(home.join("dup/clone"))
                .status()
                .unwrap();
            assert!(status.success(), "needs APFS");
        },
    );
    e.app.update(Action::GoTo(Screen::Explorer));
    select_row(&mut e.app, "dup");
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("cloning inside saves 8.2 KB"), "{text}");
    select_row(&mut e.app, "solo");
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("no clones inside"), "{text}");
}

#[test]
fn review_dialog_says_what_frees_now_and_what_stays() {
    let mut e = shared_app(GlyphMode::Nerd);
    select_row(&mut e.app, "proj");
    e.app.update(Action::ToggleMark);
    open_review(&mut e.app);
    let text = render(&mut e.app, 140, 34);
    assert!(text.contains("frees about"), "{text}");
    assert!(text.contains("right now"), "{text}");
    assert!(
        text.contains("shared with other places") && text.contains("stays"),
        "{text}"
    );
}

fn clone_pair_app() -> Env {
    let mut e = make_app_with(
        &[("dup/orig", 8192), ("solo/f", 4096)],
        GlyphMode::Nerd,
        |home| {
            let status = std::process::Command::new("cp")
                .arg("-c")
                .arg(home.join("dup/orig"))
                .arg(home.join("dup/clone"))
                .status()
                .unwrap();
            assert!(status.success(), "needs APFS");
        },
    );
    e.app.update(Action::GoTo(Screen::Explorer));
    e
}

#[test]
fn review_explains_data_shared_only_among_the_marked_files() {
    let mut e = clone_pair_app();
    select_row(&mut e.app, "dup");
    e.app.update(Action::ToggleMark);
    open_review(&mut e.app);
    let text = render(&mut e.app, 140, 36);
    assert!(
        text.contains("shared only among these files"),
        "the clone data is not private, so say where it goes:\n{text}"
    );
    assert!(text.contains("unless a snapshot holds it"), "{text}");
    assert!(
        !text.contains("frees all of it right now"),
        "must not claim everything is freed now:\n{text}"
    );
}

#[test]
fn the_detail_lines_fit_a_narrow_pane() {
    let mut e = clone_pair_app();
    select_row(&mut e.app, "dup");
    // 100 columns leaves the list pane about 50 wide.
    let text = render(&mut e.app, 100, 30);
    for needle in [
        "frees now",
        "held by snapshots",
        "shared with other places",
        "(stays)",
        "shared inside",
        "cloning inside saves",
    ] {
        let line = text.lines().find(|l| l.contains(needle));
        assert!(line.is_some(), "missing {needle:?}:\n{text}");
    }
    assert!(
        !text.contains("shared inside\u{2026}"),
        "no cut-off label:\n{text}"
    );
}

#[test]
fn the_frees_column_is_in_the_header_and_shows_a_value_per_row() {
    let mut e = make_app_with(
        &[("plain.bin", 8192), ("orig.bin", 20480)],
        GlyphMode::Nerd,
        |home| {
            let status = std::process::Command::new("cp")
                .arg("-c")
                .arg(home.join("orig.bin"))
                .arg(home.join("clone.bin"))
                .status()
                .unwrap();
            assert!(status.success(), "needs APFS");
        },
    );
    e.app.update(Action::GoTo(Screen::Explorer));
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("Frees"), "{text}");
    // (the treemap beside the list also names clone.bin; pick the list row)
    let clone = text
        .lines()
        .find(|l| l.contains("clone.bin") && l.contains("20.5 KB"))
        .unwrap_or_else(|| panic!("no list row with size and disk:\n{text}"));
    assert!(clone.contains(" 0 B"), "a clone frees nothing: {clone}");
    let plain = e
        .app
        .rows
        .iter()
        .find(|r| r.name == "plain.bin")
        .unwrap()
        .frees;
    assert!(plain.is_some());
}

#[test]
fn a_narrow_pane_drops_the_share_bar_but_keeps_the_frees_column() {
    let mut e = explorer(GlyphMode::Nerd);
    let wide = render(&mut e.app, 150, 30);
    assert!(wide.contains("Frees") && wide.contains("Share"), "{wide}");
    let narrow = render(&mut e.app, 100, 30);
    assert!(
        narrow.contains("Frees") && narrow.contains("On disk"),
        "{narrow}"
    );
    assert!(
        !narrow.contains("Share"),
        "the bar gives way first:\n{narrow}"
    );
}

#[test]
fn the_treemap_is_the_default_graph() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Treemap"), "{text}");
    assert!(!text.contains("Distribution"), "{text}");
}

#[test]
fn rows_show_an_ellipsis_for_unknown_frees_and_the_ascii_form_in_ascii_mode() {
    // Known/unknown formatting lives in the explorer module; here just make
    // sure ASCII mode never prints a non-ASCII cell in the new column.
    let mut e = explorer(GlyphMode::Ascii);
    for c in render_cells(&mut e.app, 140, 30) {
        assert!(c.is_ascii(), "{c:?}");
    }
}

#[test]
fn the_explorer_shows_the_folders_own_totals_above_the_scan_totals() {
    let mut e = explorer(GlyphMode::Nerd);
    let text = render(&mut e.app, 140, 30);
    let lines: Vec<&str> = text.lines().collect();
    let folder = lines
        .iter()
        .position(|l| l.contains("this folder:"))
        .unwrap();
    let scan = lines.iter().position(|l| l.contains(" files ")).unwrap();
    assert!(folder < scan, "folder totals come first:\n{text}");
    assert!(lines[folder].contains("items") && lines[folder].contains("frees"));
}

#[test]
fn the_list_shows_where_the_cursor_is_and_a_scrollbar_when_it_overflows() {
    let names: Vec<String> = (0..60).map(|i| format!("file{i:02}.txt")).collect();
    let files: Vec<(&str, usize)> = names.iter().map(|n| (n.as_str(), 100)).collect();
    let mut e = make_app(&files, GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Explorer));
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("1/60"), "{text}");
    assert!(text.contains('█'), "a scrollbar thumb:\n{text}");
    e.app.update(Action::Bottom);
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("60/60"), "{text}");

    let mut e = make_app(&[("only.txt", 100)], GlyphMode::Ascii);
    e.app.update(Action::GoTo(Screen::Explorer));
    let text = render(&mut e.app, 140, 30);
    assert!(text.contains("1/1"), "{text}");
    assert!(!text.contains('█'), "no scrollbar when everything fits");
}

// --- Caches & Projects screen ----------------------------------------------

const TARGET_FILES: &[(&str, usize)] = &[
    ("shop/package.json", 10),
    ("shop/node_modules/a/i.js", 300_000),
    ("shop/.next/c", 100_000),
    ("api/Cargo.toml", 10),
    ("api/target/x", 900_000),
    ("Library/Caches/Spotify/blob", 500_000),
];

fn wait_tools(app: &mut App) {
    for _ in 0..2000 {
        app.update(Action::Tick);
        if !app.tools_looking() && !app.targets_building() && app.scanner.is_idle() {
            app.update(Action::Tick);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("tool discovery did not finish");
}

fn targets_screen(mode: GlyphMode) -> Env {
    let mut e = make_app(TARGET_FILES, mode);
    e.app.update(Action::GoTo(Screen::Targets));
    wait_tools(&mut e.app);
    e
}

fn cursor_on_row(app: &mut App, name: &str) {
    use maccleanup::targets::Line;
    let at = app
        .tlines
        .iter()
        .position(|l| match *l {
            Line::Row { group, row } => app.targets.row(group, row).row.name == name,
            _ => false,
        })
        .unwrap_or_else(|| panic!("no row {name}"));
    app.tcursor = at;
}

#[test]
fn targets_screen_shows_both_section_bands_and_the_rows() {
    let mut e = targets_screen(GlyphMode::Nerd);
    let text = render(&mut e.app, 140, 36);
    assert!(text.contains("PROJECTS"), "{text}");
    assert!(text.contains("TOOLS & CACHES"), "{text}");
    for want in ["~/shop", "~/api", "node_modules", "target", "Spotify"] {
        assert!(text.contains(want), "missing {want}:\n{text}");
    }
}

#[test]
fn targets_columns_match_the_explorer_in_order() {
    let mut e = targets_screen(GlyphMode::Nerd);
    let t = render(&mut e.app, 140, 36);
    let header_of = |text: &str| -> Vec<usize> {
        let line = text
            .lines()
            .find(|l| l.contains("Name") && l.contains("Frees"))
            .unwrap();
        ["Name", "Size", "On disk", "Frees"]
            .iter()
            .map(|c| line.find(c).unwrap_or_else(|| panic!("no {c} in {line}")))
            .collect()
    };
    let cols = header_of(&t);
    assert!(cols.windows(2).all(|w| w[0] < w[1]), "{cols:?}");
    e.app.update(Action::GoTo(Screen::Explorer));
    let ex = header_of(&render(&mut e.app, 140, 36));
    // Same labels in the same order; the panes differ in width, not in order.
    assert_eq!(ex.len(), cols.len());
}

#[test]
fn a_marked_row_and_the_footer_total_show() {
    let mut e = targets_screen(GlyphMode::Nerd);
    cursor_on_row(&mut e.app, "target");
    e.app.update(Action::ToggleMark);
    for _ in 0..200 {
        e.app.update(Action::Tick);
        if !e.app.set_space.pending {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let text = render(&mut e.app, 140, 36);
    assert!(text.contains("marked 1"), "{text}");
    assert!(text.contains("frees"), "{text}");
    assert!(
        text.contains(&human_size(900_000)) || text.contains("879"),
        "{text}"
    );
}

#[test]
fn ascii_mode_has_no_non_ascii_cells() {
    let mut e = targets_screen(GlyphMode::Ascii);
    cursor_on_row(&mut e.app, "target");
    e.app.update(Action::ToggleMark);
    for c in render_cells(&mut e.app, 140, 36) {
        assert!(c.is_ascii(), "non-ascii cell {c:?}");
    }
}

#[test]
fn the_screen_renders_at_every_small_size_without_panicking() {
    let mut e = targets_screen(GlyphMode::Nerd);
    for (w, h) in [
        (ui::MIN_WIDTH, ui::MIN_HEIGHT),
        (80, 20),
        (200, 60),
        (61, 15),
    ] {
        let _ = render(&mut e.app, w, h);
    }
}

#[test]
fn an_empty_home_says_there_is_nothing_to_clean() {
    let mut e = make_app(&[("note.txt", 10)], GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Targets));
    wait_tools(&mut e.app);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("nothing to clean"), "{text}");
}

#[test]
fn the_graph_follows_the_group_and_t_switches_its_kind() {
    let mut e = targets_screen(GlyphMode::Nerd);
    cursor_on_row(&mut e.app, "target");
    let text = render(&mut e.app, 140, 36);
    assert!(text.contains("Treemap"), "{text}");
    e.app.update(Action::CycleGraph);
    e.app.update(Action::CycleGraph);
    let text = render(&mut e.app, 140, 36);
    assert!(text.contains("Distribution"), "{text}");
    assert!(text.contains("selected: target"), "{text}");
}

#[test]
fn the_detail_pane_names_what_a_row_is_and_how_it_comes_back() {
    let mut e = targets_screen(GlyphMode::Nerd);
    cursor_on_row(&mut e.app, "node_modules");
    let text = render(&mut e.app, 150, 40);
    assert!(text.contains("comes back: reinstall with npm"), "{text}");
    assert!(
        text.contains("yarn install"),
        "long text wraps, not cut:\n{text}"
    );
    assert!(text.contains("shop/node_modules"), "{text}");
}

#[test]
fn the_detail_pane_lists_related_folders_with_their_mark() {
    let mut e = make_app_with(
        &[
            ("Library/Caches/Store/f", 200_000),
            ("p1/package.json", 10),
            ("p2/package.json", 10),
        ],
        GlyphMode::Nerd,
        |home| {
            for p in ["p1", "p2"] {
                fs::create_dir_all(home.join(p).join("node_modules")).unwrap();
                let st = std::process::Command::new("cp")
                    .arg("-c")
                    .arg(home.join("Library/Caches/Store/f"))
                    .arg(home.join(p).join("node_modules/f"))
                    .status()
                    .unwrap();
                assert!(st.success(), "needs APFS");
            }
        },
    );
    e.app.update(Action::GoTo(Screen::Targets));
    wait_tools(&mut e.app);
    cursor_on_row(&mut e.app, "Store");
    e.app.update(Action::Up); // off the row and back on, so the related folders are looked up
    e.app.update(Action::Down);
    for _ in 0..200 {
        e.app.update(Action::Tick);
        if !e.app.holders.pending {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let text = render(&mut e.app, 150, 40);
    assert!(text.contains("p1/node_modules"), "{text}");
    assert!(text.contains("p2/node_modules"), "{text}");
    assert!(text.contains("also holds"), "{text}");
}

#[test]
fn the_review_shows_the_command_block_with_the_explicit_key() {
    use maccleanup::catalog::{CommandSpec, Mode, ToolEntry};
    let mut e = targets_screen(GlyphMode::Nerd);
    let home = e.app.policy.home().to_path_buf();
    e.app.tool_entries = vec![ToolEntry {
        id: "cmd".to_string(),
        tool: "Tool".to_string(),
        label: "Spotify".to_string(),
        path: home.join("Library/Caches/Spotify"),
        regen: "again".to_string(),
        mode: Mode::Command(CommandSpec::new("/opt/x/bin/tool", &["clean"])),
        note: None,
    }];
    e.app.rebuild_targets();
    e.app.settle_targets();
    cursor_on_row(&mut e.app, "Spotify");
    e.app.update(Action::ToggleMark);
    open_review(&mut e.app);
    let text = render(&mut e.app, 150, 40);
    assert!(text.contains("Tool cleanup commands"), "{text}");
    assert!(text.contains("tool clean"), "{text}");
    assert!(text.contains("/opt/x/bin/tool"), "{text}");
    assert!(text.contains("[ y ]") || text.contains(" y "), "{text}");
    assert!(
        !text.contains("Enter"),
        "Enter must not be offered:\n{text}"
    );
}

#[test]
fn tools_still_being_looked_up_are_said_so() {
    let mut e = make_app(TARGET_FILES, GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Targets));
    assert!(e.app.tools_looking());
    let text = render(&mut e.app, 140, 36);
    // The phase moves quickly; the label and a bar are always there.
    assert!(text.contains("tools:"), "{text}");
}

#[test]
fn the_explorer_footer_counts_marked_tool_cleanups() {
    use maccleanup::catalog::{CommandSpec, Mode, ToolEntry};
    let mut e = targets_screen(GlyphMode::Nerd);
    let home = e.app.policy.home().to_path_buf();
    e.app.tool_entries = vec![ToolEntry {
        id: "cmd".to_string(),
        tool: "Tool".to_string(),
        label: "Spotify".to_string(),
        path: home.join("Library/Caches/Spotify"),
        regen: "again".to_string(),
        mode: Mode::Command(CommandSpec::new("/opt/x/bin/tool", &["clean"])),
        note: None,
    }];
    e.app.rebuild_targets();
    e.app.settle_targets();
    cursor_on_row(&mut e.app, "Spotify");
    e.app.update(Action::ToggleMark);
    e.app.update(Action::GoTo(Screen::Explorer));
    let text = render(&mut e.app, 150, 40);
    assert!(text.contains("1 tool cleanup"), "{text}");
}

fn rooted_app(root_rel: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    for (rel, size) in TARGET_FILES {
        write(&home.join(rel), *size);
    }
    let hold = base.join("hold");
    let scanner = Scanner::new(2);
    let root = scanner.scan_root(&home.join(root_rel));
    assert!(scanner.wait_idle(Duration::from_secs(20)));
    let policy = Policy::new(&home, &hold);
    let holding = Holding::open(hold).unwrap();
    let free = volume::free_space(&home).unwrap();
    let app = App::new(
        scanner,
        root,
        policy,
        holding,
        Accounting::new(free),
        Glyphs::new(GlyphMode::Nerd),
        false,
    )
    .with_runner(std::sync::Arc::new(NoTools));
    Env { _dir: dir, app }
}

#[test]
fn a_narrowed_scan_root_and_missing_full_disk_access_are_announced() {
    let mut e = rooted_app("shop");
    e.app.update(Action::GoTo(Screen::Targets));
    wait_tools(&mut e.app);
    e.app.fda_missing = true;
    let text = render(&mut e.app, 190, 40);
    assert!(text.contains("scan root is"), "{text}");
    assert!(text.contains("Full Disk Access"), "{text}");
    // A full home scan says neither.
    let mut e = targets_screen(GlyphMode::Nerd);
    let text = render(&mut e.app, 190, 40);
    assert!(!text.contains("scan root is"), "{text}");
    assert!(!text.contains("Full Disk Access"), "{text}");
}

#[test]
fn y_is_refused_while_a_marked_command_is_hidden_by_a_short_terminal() {
    use maccleanup::catalog::{CommandSpec, Mode, ToolEntry};
    let mut e = targets_screen(GlyphMode::Nerd);
    let home = e.app.policy.home().to_path_buf();
    let entries: Vec<ToolEntry> = (0..8)
        .map(|i| {
            let rel = format!("Library/Caches/Cmd{i}");
            fs::create_dir_all(home.join(&rel)).unwrap();
            fs::write(home.join(&rel).join("f"), vec![b'x'; 1000]).unwrap();
            ToolEntry {
                id: format!("cmd{i}"),
                tool: "Tool".to_string(),
                label: format!("Cmd{i}"),
                path: home.join(&rel),
                regen: "again".to_string(),
                mode: Mode::Command(CommandSpec::new("/opt/x/bin/tool", &[&format!("clean{i}")])),
                note: None,
            }
        })
        .collect();
    e.app.tool_entries = entries;
    e.app.update(Action::RescanAll);
    wait_tools(&mut e.app);
    e.app.rebuild_targets();
    e.app.settle_targets();
    for i in 0..8 {
        cursor_on_row(&mut e.app, &format!("Cmd{i}"));
        e.app.update(Action::ToggleMark);
    }
    assert_eq!(e.app.tool_marks.len(), 8);
    open_review(&mut e.app);
    // A short terminal cannot show eight commands.
    let text = render(&mut e.app, 100, 20);
    assert!(text.contains("more command"), "{text}");
    e.app.update(Action::Confirm);
    assert!(
        matches!(e.app.dialog, Some(Dialog::Review(_))),
        "must stay open"
    );
    assert_eq!(e.app.tool_marks.len(), 8, "nothing ran");
    // A tall terminal shows them all and the key works again.
    let text = render(&mut e.app, 140, 60);
    assert!(text.contains("clean7"), "{text}");
    e.app.update(Action::Confirm);
    assert!(
        matches!(e.app.dialog, Some(Dialog::Deleting(_))),
        "{:?}",
        e.app.dialog
    );
}

#[test]
fn work_in_progress_shows_what_it_is_doing_with_a_bar() {
    let mut e = make_app(TARGET_FILES, GlyphMode::Nerd);
    e.app.update(Action::GoTo(Screen::Targets));
    assert!(e.app.targets_building() && e.app.tools_looking());
    let text = render(&mut e.app, 190, 40);
    // The list build has three known stages; the tool lookup has a moving bar.
    assert!(text.contains("list:") && text.contains("/3"), "{text}");
    assert!(text.contains("tools:"), "{text}");
    let status = text.lines().find(|l| l.contains("list:")).unwrap();
    assert!(
        status.chars().any(|c| "▏▎▍▌▋▊▉█".contains(c)),
        "a bar is drawn: {status}"
    );
}

#[test]
fn the_progress_readout_is_ascii_in_ascii_mode() {
    let mut e = make_app(TARGET_FILES, GlyphMode::Ascii);
    e.app.update(Action::GoTo(Screen::Targets));
    for c in render_cells(&mut e.app, 190, 40) {
        assert!(c.is_ascii(), "non-ascii cell {c:?}");
    }
}

#[test]
fn the_menu_shows_each_pages_number_and_planned_pages_keep_theirs() {
    let mut e = targets_screen(GlyphMode::Nerd);
    let text = render(&mut e.app, 140, 36);
    for (n, label) in [
        (1, "Home"),
        (2, "Explorer"),
        (3, "Leftovers"),
        (4, "Uninstall"),
        (5, "Caches & Projects"),
        (6, "Docker"),
        (7, "ToBeDeleted"),
    ] {
        let line = text
            .lines()
            .find(|l| l.contains(label) && l.starts_with(&format!("│{n}")))
            .unwrap_or_else(|| panic!("no menu line '{n} ... {label}':\n{text}"));
        assert!(line.contains(label));
    }
}

/// The middle of a graph hit area, as a click position.
fn centre(r: ratatui::layout::Rect) -> (u16, u16) {
    (r.x + r.width / 2, r.y + r.height / 2)
}

#[test]
fn clicking_a_treemap_box_selects_that_row() {
    let mut e = explorer(GlyphMode::Nerd);
    assert_eq!(e.app.graph, GraphMode::Treemap);
    let _ = render(&mut e.app, 140, 36);
    assert!(
        !e.app.graph_hits.is_empty(),
        "the treemap records its boxes"
    );
    for want in 0..e.app.rows.len() {
        let hit = e.app.graph_hits.iter().find(|(_, k)| k.1 == want).cloned();
        let Some((rect, _)) = hit else { continue };
        e.app.cursor = (want + 1) % e.app.rows.len();
        let (x, y) = centre(rect);
        e.app.update(Action::Click { x, y });
        assert_eq!(e.app.cursor, want, "box {want} at {rect:?}");
    }
}

#[test]
fn clicking_a_bar_selects_that_row() {
    let mut e = explorer(GlyphMode::Nerd);
    e.app.update(Action::CycleGraph); // treemap -> hidden
    e.app.update(Action::CycleGraph); // hidden -> bars
    assert_eq!(e.app.graph, GraphMode::Bars);
    let _ = render(&mut e.app, 140, 36);
    assert_eq!(e.app.graph_hits.len(), e.app.rows.len(), "one bar per row");
    for want in 0..e.app.rows.len() {
        let (rect, _) = e
            .app
            .graph_hits
            .iter()
            .find(|(_, k)| k.1 == want)
            .cloned()
            .unwrap();
        e.app.cursor = (want + 1) % e.app.rows.len();
        let (x, y) = centre(rect);
        e.app.update(Action::Click { x, y });
        assert_eq!(e.app.cursor, want);
    }
}

#[test]
fn the_selected_bar_moves_with_a_click() {
    let mut e = explorer(GlyphMode::Nerd);
    e.app.update(Action::CycleGraph);
    e.app.update(Action::CycleGraph);
    let _ = render(&mut e.app, 140, 36);
    let (rect, _) = e
        .app
        .graph_hits
        .iter()
        .find(|(_, k)| k.1 == 2)
        .cloned()
        .unwrap();
    let (x, y) = centre(rect);
    e.app.update(Action::Click { x, y });
    let name = e.app.rows[2].name.to_string_lossy().into_owned();
    let text = render(&mut e.app, 140, 36);
    assert!(text.contains(&format!("selected: {name}")), "{text}");
}

#[test]
fn clicking_a_box_on_caches_and_projects_selects_the_row() {
    let mut e = targets_screen(GlyphMode::Nerd);
    cursor_on_row(&mut e.app, "target");
    let _ = render(&mut e.app, 150, 40);
    // The graph shows the group under the cursor; another group's rows are
    // not in it, but this group's other rows are.
    let hits = e.app.graph_hits.clone();
    assert!(!hits.is_empty());
    for (rect, (g, r)) in hits {
        let want = e.app.targets.row(g, r).row.path.clone();
        let (x, y) = centre(rect);
        e.app.update(Action::Click { x, y });
        assert_eq!(
            e.app.selected_target().map(|t| t.row.path.clone()),
            Some(want)
        );
        let _ = render(&mut e.app, 150, 40);
    }
}

#[test]
fn nothing_is_clickable_in_a_graph_that_is_hidden() {
    let mut e = explorer(GlyphMode::Nerd);
    e.app.update(Action::CycleGraph); // hidden
    let _ = render(&mut e.app, 140, 36);
    assert!(e.app.graph_hits.is_empty());
}
