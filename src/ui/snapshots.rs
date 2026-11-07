//! The Time Machine page: one row per local snapshot with a mark box, its
//! date, what deleting it would free (about, once measured) and its kind.
//! The total held by all snapshots is the scan's exact figure.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{ACCENT, CAUTION, DANGER, DIM, SELECTED_BG, fit, pad, panel, right};
use crate::app::App;
use crate::fmt::human_size;
use crate::snapshots::Held;

/// `730 MB - 1.2 GB`, or one size when both ends read the same.
fn range(h: &Held) -> String {
    let (lo, hi) = (human_size(h.low), human_size(h.high));
    if lo == hi { lo } else { format!("{lo} - {hi}") }
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
    let (measured, count) = app.snaps.measured_total();
    let mut totals = format!(
        "held by snapshots {} {} (the scan's total)",
        g.dot(),
        human_size(held)
    );
    if count > 0 {
        totals.push_str(&format!(
            " {} measured {} in {} snapshot(s)",
            g.dot(),
            range(&measured),
            count
        ));
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&totals, width, &g),
            Style::default().fg(ACCENT),
        )),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let free = format!(
        "free now {} {} at start {}",
        human_size(app.free_now),
        g.dot(),
        human_size(app.accounting.free_at_start())
    );
    let settling = if app.snaps.settle.active(app.tick) {
        " (space is being released, updating)"
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&format!("{free}{settling}"), width, &g),
            Style::default().fg(DIM),
        )),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    let size_w = 20;
    let kind_w = 17;
    let name_w = width.saturating_sub(4 + size_w + kind_w + 2).max(8);
    let header = format!(
        "    {} {} {}",
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
        let start = app.snaps.offset.min(app.snaps.items.len());
        let end = (start + list.height as usize).min(app.snaps.items.len());
        let lines: Vec<Line> = app.snaps.items[start..end]
            .iter()
            .enumerate()
            .map(|(i, snap)| {
                let selected = start + i == app.snaps.cursor;
                let id = snap.id();
                let marked = id.as_ref().is_some_and(|id| app.tool_marks.contains(id));
                let measuring = snap
                    .date
                    .as_ref()
                    .is_some_and(|d| app.snaps.measuring.contains(d));
                let size = match (&snap.date, measuring) {
                    (_, true) => "measuring...".to_string(),
                    (Some(d), _) => app.snaps.sizes.get(d).map_or("press m".to_string(), range),
                    (None, _) => "-".to_string(),
                };
                let label = snap
                    .date
                    .as_deref()
                    .map_or_else(|| snap.name.clone(), pretty);
                let kind = if snap.date.is_some() {
                    "Time Machine"
                } else {
                    "system, kept"
                };
                let dim = snap.date.is_none();
                let mark = if marked { g.marked() } else { g.unmarked() };
                let cursor = if selected { g.cursor() } else { " " };
                let mut line = Line::from(vec![
                    Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
                    Span::styled(
                        format!("{} ", if dim { " " } else { mark }),
                        Style::default().fg(if marked { CAUTION } else { DIM }),
                    ),
                    Span::raw(pad(fit(&label, name_w, &g), name_w)),
                    Span::raw(format!(" {}", right(&size, size_w))),
                    Span::raw(format!(" {}", pad(kind.to_string(), kind_w))),
                ]);
                line = if dim {
                    line.style(Style::default().fg(DIM))
                } else {
                    line
                };
                if selected {
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
