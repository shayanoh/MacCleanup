//! Drawing. Everything here reads `App` and paints a frame; state changes
//! live in `app.rs`. The only thing written back is `app.layout`, so mouse
//! clicks can be mapped to rows.

pub mod dialogs;
pub mod explorer;
pub mod graph;
pub mod holding;
pub mod home;
pub mod sidebar;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::anim;
use crate::app::{App, Focus, Screen};
use crate::glyphs::{GlyphMode, Glyphs};
use crate::safety::Safety;

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 14;
pub const SIDEBAR_WIDTH: u16 = 16;

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Color = Color::DarkGray;
pub const SAFE: Color = Color::Green;
pub const CAUTION: Color = Color::Yellow;
pub const DANGER: Color = Color::Red;
pub const SELECTED_BG: Color = Color::Rgb(44, 50, 72);

const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

pub fn safety_color(s: Safety) -> Color {
    match s {
        Safety::Safe => SAFE,
        Safety::Caution => CAUTION,
        Safety::Protected => DANGER,
    }
}

pub fn safety_dot(g: &Glyphs, s: Safety) -> &'static str {
    match s {
        Safety::Safe => g.safe(),
        Safety::Caution => g.caution(),
        Safety::Protected => g.protected(),
    }
}

pub fn safety_word(s: Safety) -> &'static str {
    match s {
        Safety::Safe => "safe",
        Safety::Caution => "caution",
        Safety::Protected => "protected",
    }
}

/// A bordered panel: rounded in Nerd mode, plain ASCII otherwise.
pub fn panel<'a>(g: &Glyphs, title: impl Into<Line<'a>>) -> Block<'a> {
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(DIM));
    match g.mode {
        GlyphMode::Nerd => block.border_type(BorderType::Rounded),
        GlyphMode::Ascii => block.border_set(ASCII_BORDER),
    }
}

/// Make arbitrary file names safe to print: control characters (which could
/// move the cursor or inject escape codes) and, in ASCII mode, anything
/// non-ASCII become `?`.
pub fn clean(s: &str, g: &Glyphs) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() || (g.mode == GlyphMode::Ascii && !c.is_ascii()) {
                '?'
            } else {
                c
            }
        })
        .collect()
}

pub fn text_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Clean `s` and cut it to `width` display columns, ending in an ellipsis.
pub fn fit(s: &str, width: usize, g: &Glyphs) -> String {
    let s = clean(s, g);
    if width == 0 {
        return String::new();
    }
    if text_width(&s) <= width {
        return s;
    }
    let ell = g.ellipsis();
    let ell_w = text_width(ell);
    if width <= ell_w {
        return ".".repeat(width);
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > width - ell_w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push_str(ell);
    out
}

/// Like [`fit`], but cuts from the start so the end (the file name) stays.
pub fn fit_start(s: &str, width: usize, g: &Glyphs) -> String {
    let s = clean(s, g);
    if width == 0 {
        return String::new();
    }
    if text_width(&s) <= width {
        return s;
    }
    let ell = g.ellipsis();
    let ell_w = text_width(ell);
    if width <= ell_w {
        return ".".repeat(width);
    }
    let mut tail: Vec<char> = Vec::new();
    let mut used = 0;
    for c in s.chars().rev() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + cw > width - ell_w {
            break;
        }
        tail.push(c);
        used += cw;
    }
    let mut out = String::from(ell);
    out.extend(tail.into_iter().rev());
    out
}

pub fn pad(mut s: String, width: usize) -> String {
    let w = text_width(&s);
    if w < width {
        s.push_str(&" ".repeat(width - w));
    }
    s
}

pub fn fit_pad(s: &str, width: usize, g: &Glyphs) -> String {
    pad(fit(s, width, g), width)
}

/// Right-align `s` within `width` columns.
pub fn right(s: &str, width: usize) -> String {
    let w = text_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{s}", " ".repeat(width - w))
    }
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    app.too_small = area.width < MIN_WIDTH || area.height < MIN_HEIGHT;
    if area.width == 0 || area.height == 0 {
        return;
    }
    if app.too_small {
        too_small(frame, area);
        return;
    }

    let [body, footer] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    let [side, main] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Fill(1)]).areas(body);
    app.layout.sidebar = side;
    app.layout.main = main;
    app.layout.list_top = 0;
    app.layout.list_rows = 0;

    sidebar::render(frame, side, app);
    match app.screen {
        Screen::Home => home::render(frame, main, app),
        Screen::Explorer => explorer::render(frame, main, app),
        Screen::Holding => holding::render(frame, main, app),
    }
    wipe(frame, main, app);
    render_footer(frame, footer, app);
    dialogs::render(frame, area, app);
}

fn too_small(frame: &mut Frame, area: Rect) {
    let text = format!("Terminal too small (need {MIN_WIDTH}x{MIN_HEIGHT})");
    let rect = centered(area, area.width, 3.min(area.height));
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(CAUTION)),
        rect,
    );
}

/// Reveal the main pane left to right for a few ticks after a screen switch.
fn wipe(frame: &mut Frame, main: Rect, app: &App) {
    if !app.animations {
        return;
    }
    const TICKS: u64 = 8;
    let elapsed = app.tick.saturating_sub(app.transition_start);
    if elapsed >= TICKS {
        return;
    }
    let progress = anim::ease_out_cubic(elapsed as f64 / TICKS as f64);
    let revealed = anim::wipe_columns(progress, main.width);
    let buf = frame.buffer_mut();
    for x in (main.x + revealed)..(main.x + main.width) {
        for y in main.y..(main.y + main.height) {
            buf[(x, y)].reset();
        }
    }
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let g = &app.glyphs;
    if let Some(notice) = &app.notice {
        let text = fit(notice, area.width as usize, g);
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(CAUTION).add_modifier(Modifier::BOLD)),
            area,
        );
        return;
    }
    let hints: &[&str] = if app.focus == Focus::Sidebar {
        &["j/k choose", "enter open", "tab back", "q quit"]
    } else {
        match app.screen {
            Screen::Home => &["2 explorer", "3 ToBeDeleted", "tab menu", "q quit"],
            Screen::Explorer => &[
                "j/k move",
                "h/l out/in",
                "space mark",
                "d review & delete",
                "g graph",
                "[ ] resize",
                "s sort",
                "tab menu",
                "q quit",
            ],
            Screen::Holding => &[
                "j/k move",
                "r restore",
                "D delete forever",
                "E empty all",
                "q quit",
            ],
        }
    };
    let sep = format!(" {} ", g.dot());
    let text = fit(&hints.join(&sep), area.width as usize, g);
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(DIM)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nerd() -> Glyphs {
        Glyphs::new(GlyphMode::Nerd)
    }

    fn ascii() -> Glyphs {
        Glyphs::new(GlyphMode::Ascii)
    }

    #[test]
    fn fit_truncates_with_ellipsis() {
        assert_eq!(fit("short", 10, &nerd()), "short");
        assert_eq!(fit("abcdefghij", 6, &nerd()), "abcde…");
        assert_eq!(fit("abcdefghij", 6, &ascii()), "abc...");
        assert_eq!(fit("anything", 0, &nerd()), "");
    }

    #[test]
    fn fit_start_keeps_the_tail() {
        assert_eq!(fit_start("/a/b/c/file.txt", 40, &nerd()), "/a/b/c/file.txt");
        assert_eq!(
            fit_start("/aaaa/bbbb/file.txt", 12, &nerd()),
            "…bb/file.txt"
        );
        let ascii_out = fit_start("/aaaa/bbbb/file.txt", 12, &ascii());
        assert!(ascii_out.ends_with("file.txt") && ascii_out.starts_with("..."));
        assert!(text_width(&ascii_out) <= 12);
        assert_eq!(fit_start("anything", 0, &nerd()), "");
    }

    #[test]
    fn fitted_text_never_exceeds_width() {
        let samples = [
            "日本語のフォルダ",
            "mixed 日本 text",
            "x".repeat(300).as_str().to_owned().leak(),
        ];
        for g in [nerd(), ascii()] {
            for s in samples {
                for w in 0..40 {
                    assert!(text_width(&fit(s, w, &g)) <= w, "fit {s:?} {w}");
                    assert!(text_width(&fit_start(s, w, &g)) <= w, "fit_start {s:?} {w}");
                }
            }
        }
    }

    #[test]
    fn clean_hides_control_chars_and_non_ascii_in_ascii_mode() {
        assert_eq!(clean("a\nb\x1b[31mc\td", &nerd()), "a?b?[31mc?d");
        assert_eq!(clean("日本", &nerd()), "日本");
        assert_eq!(clean("日本x", &ascii()), "??x");
    }

    #[test]
    fn pad_and_right_use_display_width() {
        assert_eq!(pad("日本".to_string(), 6), "日本  ");
        assert_eq!(right("ab", 5), "   ab");
        assert_eq!(right("abcdef", 3), "abcdef");
    }
}
