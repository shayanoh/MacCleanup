use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{
    ACCENT, DIM, SELECTED_BG, fit, fit_pad, fit_start, graph, pad, panel, right, safety_color,
    safety_dot, text_width,
};
use crate::anim;
use crate::app::{App, GraphMode, SortBy};
use crate::fmt::{eighths_bar, human_size};
use crate::scan::Row;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let (list_area, graph_area) = match app.graph {
        GraphMode::Hidden => (area, None),
        _ => {
            let [list, graph] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(app.graph_pct)])
                    .areas(area);
            (list, Some(graph))
        }
    };
    render_list(frame, list_area, app);
    if let Some(graph_area) = graph_area {
        graph::render(frame, graph_area, app);
    }
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let path = app.current().path.to_string_lossy().into_owned();
    let title_w = (area.width as usize).saturating_sub(16);
    let title = format!(
        " {} Explorer {} {} ",
        g.folder(),
        g.dot(),
        fit_start(&path, title_w, &g)
    );
    let block = panel(&g, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 24 {
        return;
    }
    app.layout.list_top = inner.y + 2;
    app.layout.list_rows = inner.height - 3;
    app.ensure_visible();

    // Status line: live scan activity.
    let stats = app.scanner.stats();
    let totals = format!(
        "{} files {} {} ({} on disk)",
        stats.files,
        g.dot(),
        human_size(stats.bytes),
        human_size(stats.disk)
    );
    let status = if app.scanner.is_idle() {
        Line::from(vec![
            Span::styled(g.check().to_string(), Style::default().fg(Color::Green)),
            Span::raw(format!(" {totals}")),
        ])
    } else {
        let idx = anim::spinner_frame(app.tick, g.spinner().len());
        Line::from(vec![
            Span::styled(g.spinner()[idx].to_string(), Style::default().fg(ACCENT)),
            Span::raw(format!(" scanning{} {totals}", g.ellipsis())),
        ])
    };
    frame.render_widget(
        Paragraph::new(status),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Column layout.
    let icon_w = text_width(g.folder()).max(text_width(g.file()));
    let mark_w = text_width(g.marked()).max(text_width(g.unmarked()));
    let dot_w = text_width(g.safe());
    let prefix_w = 2 + mark_w + 1 + icon_w + 1 + dot_w + 1;
    let fixed = prefix_w + 1 + 9 + 1 + 9 + 1 + 8;
    let name_w = (inner.width as usize).saturating_sub(fixed).max(4);

    // Header: the sorted column is white with an arrow, the others gray.
    let sort = app.sort;
    let arrow = |by: SortBy| -> String {
        if sort != by {
            return String::new();
        }
        let a = if by == SortBy::Name {
            g.sort_asc()
        } else {
            g.sort_desc()
        };
        format!(" {a}")
    };
    let head_style = |by: SortBy| {
        if sort == by {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        }
    };
    let header = Line::from(vec![
        Span::raw(" ".repeat(prefix_w)),
        Span::styled(
            pad(format!("Name{}", arrow(SortBy::Name)), name_w),
            head_style(SortBy::Name),
        ),
        Span::raw(" "),
        Span::styled(
            right(&format!("Size{}", arrow(SortBy::Size)), 9),
            head_style(SortBy::Size),
        ),
        Span::raw(" "),
        Span::styled(
            right(&format!("On disk{}", arrow(SortBy::Disk)), 9),
            head_style(SortBy::Disk),
        ),
        Span::styled(" Share", Style::default().fg(DIM)),
    ]);
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // Rows.
    let list_rows = app.layout.list_rows as usize;
    let list_area = Rect::new(inner.x, inner.y + 2, inner.width, app.layout.list_rows);
    let total = app.weight_total().max(1);
    let start = app.offset.min(app.rows.len());
    let end = (start + list_rows).min(app.rows.len());
    let mut lines: Vec<Line> = app.rows[start..end]
        .iter()
        .enumerate()
        .map(|(i, row)| row_line(app, start + i, row, total, name_w, icon_w, mark_w))
        .collect();
    if app.rows.is_empty() {
        let msg = if app.current().is_done() {
            "(empty)".to_string()
        } else {
            format!("(scanning{})", g.ellipsis())
        };
        lines.push(Line::from(Span::styled(msg, Style::default().fg(DIM))));
    }
    frame.render_widget(Paragraph::new(lines), list_area);

    // Marked summary.
    let marked = format!(
        "marked: {} items, {} (not deleted yet)",
        app.marks.len(),
        human_size(app.marked_total())
    );
    let style = if app.marks.is_empty() {
        Style::default().fg(DIM)
    } else {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(fit(&marked, inner.width as usize, &g), style)),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

#[allow(clippy::too_many_arguments)]
fn row_line<'a>(
    app: &App,
    idx: usize,
    row: &Row,
    total: u64,
    name_w: usize,
    icon_w: usize,
    mark_w: usize,
) -> Line<'a> {
    let g = &app.glyphs;
    let r = row.live();
    let selected = idx == app.cursor;
    let marked = app.marks.contains_key(&r.path);
    let safety = app.policy.classify(&r.path);

    let cursor = if selected { g.cursor() } else { " " };
    let mark = pad(
        if marked { g.marked() } else { g.unmarked() }.to_string(),
        mark_w,
    );
    let icon = pad(
        if r.is_dir { g.folder() } else { g.file() }.to_string(),
        icon_w,
    );
    let name_style = if marked {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else if r.is_dir {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };
    let name = r.name.to_string_lossy();
    let share = if r.scanning {
        let frame_idx = anim::spinner_frame(app.tick, g.spinner().len());
        pad(format!("{} {}", g.spinner()[frame_idx], r.files), 8)
    } else {
        eighths_bar(app.weight(&r) as f64 / total as f64, 8, g)
    };

    let line = Line::from(vec![
        Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
        Span::styled(format!("{mark} "), Style::default().fg(Color::Magenta)),
        Span::styled(format!("{icon} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!("{} ", safety_dot(g, safety)),
            Style::default().fg(safety_color(safety)),
        ),
        Span::styled(fit_pad(&name, name_w, g), name_style),
        Span::styled(
            format!(" {}", right(&human_size(r.size), 9)),
            value_style(app.sort == SortBy::Size),
        ),
        Span::styled(
            format!(" {}", right(&human_size(r.disk), 9)),
            value_style(app.sort == SortBy::Disk),
        ),
        Span::styled(format!(" {share}"), Style::default().fg(Color::Yellow)),
    ]);
    if selected {
        line.style(Style::default().bg(SELECTED_BG))
    } else {
        line
    }
}

/// The sorted column's values are white; the other size column is gray.
fn value_style(sorted: bool) -> Style {
    if sorted {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(DIM)
    }
}
