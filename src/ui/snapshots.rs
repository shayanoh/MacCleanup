//! The Time Machine page. It looks like the Docker page: a colored heading
//! bar that is also the select-all row, then one row per local snapshot with
//! a mark box, an icon, a safety dot, its date, what deleting it would free
//! (a range, once measured) and its kind. The total held by all snapshots is
//! the scan's exact figure.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{
    ACCENT, CAUTION, DANGER, DIM, SAFE, SELECTED_BG, fit, fit_pad, pad, panel, right, safety_color,
    safety_dot,
};
use crate::app::App;
use crate::fmt::human_size;
use crate::safety::Safety;
use crate::snapshots::{Held, Snapshot};

/// `730 MB - 1.2 GB`, or one size when both ends read the same.
fn range(h: &Held) -> String {
    let (lo, hi) = (human_size(h.low), human_size(h.high));
    if lo == hi { lo } else { format!("{lo} - {hi}") }
}

/// `950`, `12k`, `3.1M`: a file count in a few characters.
fn count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// What a snapshot's size cell says while it is being measured: the walk's
/// count of files and bytes so far for the one running, "waiting" for the rest.
fn measuring_text(app: &App, date: &str) -> String {
    let (index, _) = app.snaps.progress.position();
    if app.snaps.measuring.get(index as usize).map(String::as_str) != Some(date) {
        return "waiting...".to_string();
    }
    let files = app.snaps.progress.files();
    if files == 0 {
        return "mounting...".to_string();
    }
    format!(
        "{} files, {}",
        count(files),
        human_size(app.snaps.progress.held().low)
    )
}

/// `2026-11-07-032842` shown as `2026-11-07 03:28:42`.
fn pretty(date: &str) -> String {
    match date.split('-').collect::<Vec<_>>().as_slice() {
        [y, m, d, t] if t.len() == 6 => {
            format!("{y}-{m}-{d} {}:{}:{}", &t[0..2], &t[2..4], &t[4..6])
        }
        _ => date.to_string(),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let block = panel(&g, format!(" {} Time Machine snapshots ", g.nav(6)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 5 || inner.width < 30 {
        return;
    }
    let width = inner.width as usize;
    app.layout.list_top = inner.y + 3;
    app.layout.list_rows = inner.height - 4;
    app.ensure_visible();

    let held = app.stack.first().map_or(0, |n| n.snapshot_held());
    let (measured, measured_count) = app.snaps.measured_total();
    let mut totals = format!(
        "held by snapshots {} {} (the scan's total)",
        g.dot(),
        human_size(held)
    );
    if measured_count > 0 {
        totals.push_str(&format!(
            " {} measured {} in {} snapshot(s)",
            g.dot(),
            range(&measured),
            measured_count
        ));
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&totals, width, &g),
            Style::default().fg(ACCENT),
        )),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let settling = if app.snaps.settle.active(app.tick) {
        " (space is being released, updating)"
    } else {
        ""
    };
    let free = Line::from(vec![
        Span::styled("free now ", Style::default().fg(DIM)),
        Span::styled(
            human_size(app.free_now),
            Style::default().fg(SAFE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                " {} at start {}{settling}",
                g.dot(),
                human_size(app.accounting.free_at_start())
            ),
            Style::default().fg(DIM),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(free),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // Columns: cursor, mark box, icon, safety dot, name, size, kind.
    let size_w = 20;
    let kind_w = 14;
    let prefix_w = 2 + 2 + 2 + 2;
    let name_w = width.saturating_sub(prefix_w + size_w + kind_w + 2).max(8);
    let header = format!(
        "{}{} {} {}",
        " ".repeat(prefix_w),
        pad("Snapshot".to_string(), name_w),
        right("Frees (about)", size_w),
        pad("Kind".to_string(), kind_w)
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&header, width, &g),
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );

    let list = Rect::new(inner.x, inner.y + 3, inner.width, app.layout.list_rows);
    if app.snaps.items.is_empty() {
        let (text, color) = match (&app.snaps.error, app.snaps.loaded) {
            (Some(e), _) => (e.clone(), DANGER),
            (None, true) => (
                "No local snapshots. macOS makes them hourly while Time Machine is on.".to_string(),
                DIM,
            ),
            (None, false) => ("Reading the snapshots...".to_string(), DIM),
        };
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(color)),
            list,
        );
    } else {
        // Row 0 is the heading, row n the n-th snapshot.
        let start = app.snaps.offset.min(app.snaps.rows());
        let end = (start + list.height as usize).min(app.snaps.rows());
        let lines: Vec<Line> = (start..end)
            .map(|row| {
                let selected = row == app.snaps.cursor;
                let line = if row == 0 {
                    heading_line(app, width, selected)
                } else {
                    snapshot_line(app, row - 1, selected, [name_w, size_w, kind_w])
                };
                if selected && row > 0 {
                    line.style(
                        Style::default()
                            .bg(SELECTED_BG)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    line
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), list);
    }

    let hints = format!(
        "space mark {d} a mark all {d} m/M measure {d} d review & delete {d} r reread",
        d = g.dot()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&hints, width, &g),
            Style::default().fg(DIM),
        )),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

/// The colored bar over the snapshots. Its box is "all marked": space on it
/// marks or unmarks every Time Machine snapshot. Underlined while selected,
/// since its background is already a color.
fn heading_line(app: &App, width: usize, selected: bool) -> Line<'static> {
    let g = &app.glyphs;
    let ids: Vec<String> = app.snaps.items.iter().filter_map(Snapshot::id).collect();
    let marked = ids.iter().filter(|id| app.tool_marks.contains(*id)).count();
    let all = !ids.is_empty() && marked == ids.len();
    let cursor = if selected { g.cursor() } else { " " };
    let boxed = if all { g.marked() } else { g.unmarked() };
    let mut text = format!(
        "{cursor} {boxed} TIME MACHINE   {} {}",
        ids.len(),
        if ids.len() == 1 {
            "snapshot"
        } else {
            "snapshots"
        }
    );
    if marked > 0 {
        text.push_str(&format!("   {marked} marked"));
    }
    let mut style = Style::default()
        .fg(Color::Black)
        .bg(ACCENT)
        .add_modifier(Modifier::BOLD);
    if selected {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    Line::from(Span::styled(fit_pad(&text, width, g), style))
}

fn snapshot_line(
    app: &App,
    i: usize,
    selected: bool,
    [name_w, size_w, kind_w]: [usize; 3],
) -> Line<'static> {
    let g = &app.glyphs;
    let snap = &app.snaps.items[i];
    let ours = snap.date.is_some();
    let marked = snap.id().is_some_and(|id| app.tool_marks.contains(&id));
    let measuring = snap
        .date
        .as_ref()
        .is_some_and(|d| app.snaps.measuring.contains(d));
    let (size, size_style) = match (&snap.date, measuring) {
        (Some(d), true) => (measuring_text(app, d), Style::default().fg(CAUTION)),
        (Some(d), false) => match app.snaps.sizes.get(d) {
            Some(h) => (range(h), Style::default()),
            None => ("press m".to_string(), Style::default().fg(DIM)),
        },
        (None, _) => ("-".to_string(), Style::default().fg(DIM)),
    };
    let label = snap
        .date
        .as_deref()
        .map_or_else(|| snap.name.clone(), pretty);
    let safety = if ours {
        Safety::Safe
    } else {
        Safety::Protected
    };
    let dim = Style::default().fg(DIM);
    let (mark_glyph, mark_style) = if !ours {
        (" ", dim)
    } else if marked {
        (g.marked(), Style::default().fg(Color::Magenta))
    } else {
        (g.unmarked(), Style::default().fg(Color::Magenta))
    };
    let name_style = if marked {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else if ours {
        Style::default().fg(Color::Blue)
    } else {
        dim
    };
    let cursor = if selected { g.cursor() } else { " " };
    let icon = if ours { g.snapshot() } else { g.locked() };
    let kind = if ours { "Time Machine" } else { "system, kept" };
    Line::from(vec![
        Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
        Span::styled(format!("{mark_glyph} "), mark_style),
        Span::styled(format!("{icon} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!("{} ", safety_dot(g, safety)),
            Style::default().fg(safety_color(safety)),
        ),
        Span::styled(pad(fit(&label, name_w, g), name_w), name_style),
        Span::styled(format!(" {}", right(&size, size_w)), size_style),
        Span::styled(format!(" {}", pad(kind.to_string(), kind_w)), dim),
    ])
}
