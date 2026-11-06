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

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn make_app(files: &[(&str, usize)], mode: GlyphMode) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    for (rel, size) in files {
        write(&home.join(rel), *size);
    }
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
        false,
    );
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
    let text = render(&mut e.app, 120, 30);
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

    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
    e.app.update(Action::ConfirmIrreversible);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("cannot be undone"), "{text}");
    assert!(text.contains("[ y ]"), "{text}");
}

#[test]
fn deleting_dialog_shows_progress() {
    let mut e = explorer(GlyphMode::Nerd);
    mark(&mut e.app, "big");
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview); // nothing marked: sets a notice
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
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let symbols: Vec<&str> = needle.chars().map(|_| "").collect();
    let _ = symbols;
    let needle_chars: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(needle_chars.len() as u16) {
            if needle_chars
                .iter()
                .enumerate()
                .all(|(i, c)| buf[(x + i as u16, y)].symbol() == c)
            {
                let cell = &buf[(x, y)];
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
    assert!(text.contains("Size ▼"), "{text}");
    assert!(!text.contains("On disk ▼"), "{text}");

    e.app.update(Action::CycleSort);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("Name ▲"), "{text}");
}

#[test]
fn explorer_header_arrows_are_ascii_in_ascii_mode() {
    let mut e = explorer(GlyphMode::Ascii);
    let text = render(&mut e.app, 120, 30);
    assert!(text.contains("On disk v"), "{text}");
    e.app.update(Action::CycleSort);
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

    e.app.update(Action::CycleSort); // by Size
    let disk = find_text(&mut e.app, 120, 30, "On disk").unwrap();
    let size = find_text(&mut e.app, 120, 30, "Size").unwrap();
    assert_eq!(size.fg, Some(Color::White));
    assert_eq!(disk.fg, Some(Color::DarkGray));
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
    e.app.update(Action::OpenReview);
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
