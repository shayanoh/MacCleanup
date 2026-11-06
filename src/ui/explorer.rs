use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::{
    ACCENT, DIM, SELECTED_BG, fit, fit_pad, fit_start, graph, pad, panel, right, safety_color,
    safety_dot, text_width,
};
use crate::anim;
use crate::app::{App, GraphMode, SortBy};
use crate::filter::{self, Query};
use crate::fmt::{both_usage, eighths_bar, human_size};
use crate::glyphs::Glyphs;
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
        app.graph_hits = graph::render(frame, graph_area, app);
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
    // The filter box takes one row above the header while it is shown.
    let filter_h = u16::from(app.filter.visible());
    if inner.height < 8 + filter_h || inner.width < 24 {
        return;
    }
    app.layout.list_top = inner.y + 3 + filter_h;
    app.layout.list_rows = inner.height - 7 - filter_h;
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
            Span::raw(format!(" scanning{} {totals} ", g.ellipsis())),
            Span::styled(
                crate::fmt::marquee(app.tick, 10, &g),
                Style::default().fg(ACCENT),
            ),
        ])
    };
    frame.render_widget(
        Paragraph::new(folder_totals(app, inner.width as usize)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(status),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // Filter box.
    if filter_h > 0 {
        frame.render_widget(
            Paragraph::new(filter_line(app, inner.width as usize)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
    }
    let query = filter::parse(&app.filter.query);

    // Column layout.
    let c = cols(inner.width, &g);
    let (icon_w, mark_w, name_w, show_share) = (c.icon_w, c.mark_w, c.name_w, c.show_share);
    frame.render_widget(
        Paragraph::new(header_line(app.sort, &c, &g)),
        Rect::new(inner.x, inner.y + 2 + filter_h, inner.width, 1),
    );

    // Rows.
    let list_rows = app.layout.list_rows as usize;
    let list_area = Rect::new(
        inner.x,
        inner.y + 3 + filter_h,
        inner.width,
        app.layout.list_rows,
    );
    let total = app.weight_total().max(1);
    let start = app.offset.min(app.rows.len());
    let end = (start + list_rows).min(app.rows.len());
    let mut lines: Vec<Line> = app.rows[start..end]
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row_line(
                app,
                &query,
                start + i,
                row,
                total,
                name_w,
                icon_w,
                mark_w,
                show_share,
            )
        })
        .collect();
    if app.rows.is_empty() && app.filter.active() {
        lines.push(Line::from(Span::styled(
            "(nothing matches the filter)",
            Style::default().fg(DIM),
        )));
    } else if app.rows.is_empty() {
        let msg = if app.current().is_done() {
            "(empty)".to_string()
        } else {
            format!("(scanning{})", g.ellipsis())
        };
        lines.push(Line::from(Span::styled(msg, Style::default().fg(DIM))));
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    if app.rows.len() > list_rows {
        let mut state = ScrollbarState::new(app.rows.len() - list_rows).position(app.offset);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(app.glyphs.scroll_track()))
            .thumb_symbol(app.glyphs.scroll_thumb())
            .style(Style::default().fg(ACCENT));
        frame.render_stateful_widget(bar, list_area, &mut state);
    }

    // Where the selected row's space would go.
    let detail_y = inner.y + inner.height - 4;
    let detail_lines = detail_lines(app, inner.width as usize);
    frame.render_widget(
        Paragraph::new(detail_lines),
        Rect::new(inner.x, detail_y, inner.width, 3),
    );

    // Marked summary.
    let mut marked = format!(
        "marked: {} items, {} (not deleted yet)",
        app.marks.len(),
        both_usage(app.marked())
    );
    if !app.tool_marks.is_empty() {
        marked.push_str(&format!(" + {} tool cleanup(s)", app.tool_marks.len()));
    }
    let style = if app.marks.is_empty() && app.tool_marks.is_empty() {
        Style::default().fg(DIM)
    } else {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    };
    let pos = position_text(app);
    let pos_w = text_width(&pos) as u16;
    let marked_w = inner.width.saturating_sub(pos_w + 1) as usize;
    frame.render_widget(
        Paragraph::new(Span::styled(fit(&marked, marked_w, &g), style)),
        Rect::new(inner.x, inner.y + inner.height - 1, marked_w as u16, 1),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(pos, Style::default().fg(Color::White))),
        Rect::new(
            inner.x + inner.width - pos_w,
            inner.y + inner.height - 1,
            pos_w,
            1,
        ),
    );
}

/// The list columns, shared with every screen that looks like the Explorer.
pub(super) struct Cols {
    pub icon_w: usize,
    pub mark_w: usize,
    /// Cursor, mark, icon and safety dot: everything left of the name.
    pub prefix_w: usize,
    pub name_w: usize,
    pub show_share: bool,
}

pub(super) fn cols(inner_width: u16, g: &Glyphs) -> Cols {
    let icon_w = text_width(g.folder()).max(text_width(g.file()));
    let mark_w = text_width(g.marked()).max(text_width(g.unmarked()));
    let dot_w = text_width(g.safe());
    let prefix_w = 2 + mark_w + 1 + icon_w + 1 + dot_w + 1;
    // Size, on disk, frees: three 9-wide columns and the 10-wide Modified one
    // ("Modified ▼" must fit); the share bar goes first when the pane is
    // narrow.
    let fixed_no_share = prefix_w + 3 * (1 + 9) + (1 + 10);
    let cw = inner_width.saturating_sub(1) as usize; // last column: scrollbar
    let show_share = cw >= fixed_no_share + 9 + 16;
    let fixed = fixed_no_share + if show_share { 9 } else { 0 };
    let name_w = cw.saturating_sub(fixed).max(4);
    Cols {
        icon_w,
        mark_w,
        prefix_w,
        name_w,
        show_share,
    }
}

/// Column titles: the sorted one is white with an arrow, the others gray.
pub(super) fn header_line(sort: SortBy, c: &Cols, g: &Glyphs) -> Line<'static> {
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
    let mut header = vec![
        Span::raw(" ".repeat(c.prefix_w)),
        Span::styled(
            pad(format!("Name{}", arrow(SortBy::Name)), c.name_w),
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
        Span::raw(" "),
        Span::styled(
            right(&format!("Frees{}", arrow(SortBy::Frees)), 9),
            head_style(SortBy::Frees),
        ),
        Span::raw(" "),
        Span::styled(
            right(&format!("Modified{}", arrow(SortBy::Modified)), 10),
            head_style(SortBy::Modified),
        ),
    ];
    if c.show_share {
        header.push(Span::styled(" Share", Style::default().fg(DIM)));
    }
    Line::from(header)
}

/// The Size, On disk, Frees and Modified cells of a row. `frees_text` replaces the
/// Frees value (for example "up to 6 GB" on a tool command).
pub(super) fn number_spans(
    r: &Row,
    queued: bool,
    sort: SortBy,
    frees_text: Option<String>,
    g: &Glyphs,
) -> Vec<Span<'static>> {
    // A queued folder has no numbers yet: show that, not a misleading 0 B.
    let unknown = right(g.ellipsis(), 9);
    let cell = |v: String| if queued { unknown.clone() } else { v };
    vec![
        Span::styled(
            format!(" {}", cell(right(&human_size(r.size), 9))),
            value_style(sort == SortBy::Size),
        ),
        Span::styled(
            format!(" {}", cell(right(&human_size(r.disk), 9))),
            value_style(sort == SortBy::Disk),
        ),
        Span::styled(
            format!(
                " {}",
                if queued {
                    unknown.clone()
                } else {
                    frees_text.unwrap_or_else(|| frees_cell(r.frees, g))
                }
            ),
            value_style(sort == SortBy::Frees),
        ),
        Span::styled(
            format!(
                " {}",
                if queued {
                    right(g.ellipsis(), 10)
                } else {
                    right(&crate::fmt::ago(crate::fmt::now_secs(), r.mtime), 10)
                }
            ),
            value_style(sort == SortBy::Modified),
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn row_line<'a>(
    app: &App,
    query: &Query,
    idx: usize,
    row: &Row,
    total: u64,
    name_w: usize,
    icon_w: usize,
    mark_w: usize,
    show_share: bool,
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
    let queued = r.queued();
    let share = if queued {
        pad("queued".to_string(), 8)
    } else if r.scanning {
        let frame_idx = anim::spinner_frame(app.tick, g.spinner().len());
        pad(format!("{} {}", g.spinner()[frame_idx], r.files), 8)
    } else {
        eighths_bar(app.weight(&r) as f64 / total as f64, 8, g)
    };

    let mut spans = vec![
        Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
        Span::styled(format!("{mark} "), Style::default().fg(Color::Magenta)),
        Span::styled(format!("{icon} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!("{} ", safety_dot(g, safety)),
            Style::default().fg(safety_color(safety)),
        ),
    ];
    // The name, with the letters the filter matched picked out.
    spans.extend(highlight_spans(
        &fit_pad(&name, name_w, g),
        &query.matches(&name).unwrap_or_default(),
        name_style,
        name_style.fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    spans.extend(number_spans(&r, queued, app.sort, None, g));
    if show_share {
        let color = if queued { DIM } else { Color::Yellow };
        spans.push(Span::styled(
            format!(" {share}"),
            Style::default().fg(color),
        ));
    }
    let line = Line::from(spans);
    if selected {
        line.style(Style::default().bg(SELECTED_BG))
    } else {
        line
    }
}

/// Where the cursor is in the list: `12/340` (`0/0` when empty).
fn position_text(app: &App) -> String {
    if app.rows.is_empty() {
        "0/0".to_string()
    } else {
        format!(
            "{}/{}",
            app.cursor.min(app.rows.len() - 1) + 1,
            app.rows.len()
        )
    }
}

/// Totals for the folder being shown, above the scan-wide totals.
fn folder_totals(app: &App, width: usize) -> Line<'static> {
    let g = &app.glyphs;
    let cur = app.current();
    let mut parts = vec![format!("{} items", app.all_rows.len())];
    parts.push(format!(
        "{} ({} on disk)",
        human_size(cur.size()),
        human_size(
            app.current_space
                .map_or_else(|| cur.disk(), |s| s.footprint)
        )
    ));
    parts.push(match app.current_space {
        Some(s) => format!("frees {}", human_size(s.frees_now)),
        None => format!("frees {}", g.ellipsis()),
    });
    let text = format!(
        "{} this folder: {}",
        g.folder(),
        parts.join(&format!(" {} ", g.dot()))
    );
    Line::from(Span::styled(
        fit(&text, width, g),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))
}

/// The Frees column: a size, or an ellipsis while it is not known yet.
pub(super) fn frees_cell(frees: Option<u64>, g: &Glyphs) -> String {
    right(
        &frees.map_or_else(|| g.ellipsis().to_string(), human_size),
        9,
    )
}

/// The sorted column's values are white; the other size column is gray.
pub(super) fn value_style(sorted: bool) -> Style {
    if sorted {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(DIM)
    }
}

/// Three lines under the list saying where the selected row's disk space
/// would go: what deleting it frees now, what only snapshots hold, what is
/// shared with clones and hard links elsewhere (and stays), and what cloning
/// inside it already saves.
fn detail_lines<'a>(app: &App, width: usize) -> Vec<Line<'a>> {
    let g = &app.glyphs;
    let Some(d) = &app.detail else {
        return vec![
            Line::from(Span::styled("nothing selected", Style::default().fg(DIM))),
            Line::raw(""),
            Line::raw(""),
        ];
    };
    let sp = d.space;
    let scanning = if d.complete {
        String::new()
    } else {
        format!(" (still scanning{})", g.ellipsis())
    };
    let dot = g.dot();
    let first = format!(
        "frees now {} {dot} held by snapshots {}{scanning}",
        human_size(sp.frees_now),
        human_size(sp.snapshots)
    );
    let second = format!(
        "shared with other places {} (stays)",
        human_size(sp.shared_outside)
    );
    let third = if sp.saved > 0 {
        format!(
            "shared inside {} {dot} cloning inside saves {}",
            human_size(sp.shared_inside),
            human_size(sp.saved)
        )
    } else {
        format!(
            "shared inside {} {dot} no clones inside",
            human_size(sp.shared_inside)
        )
    };
    vec![
        Line::from(Span::styled(
            fit(&first, width, g),
            Style::default().fg(Color::Green),
        )),
        Line::from(Span::styled(
            fit(&second, width, g),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            fit(&third, width, g),
            Style::default().fg(DIM),
        )),
    ]
}

/// Split `text` into spans, styling the characters at `positions`.
fn highlight_spans(
    text: &str,
    positions: &[usize],
    base: Style,
    highlight: Style,
) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_highlighted = false;
    for (i, c) in text.chars().enumerate() {
        let highlighted = positions.binary_search(&i).is_ok();
        if highlighted != run_highlighted && !run.is_empty() {
            let style = if run_highlighted { highlight } else { base };
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_highlighted = highlighted;
        run.push(c);
    }
    if !run.is_empty() {
        let style = if run_highlighted { highlight } else { base };
        spans.push(Span::styled(run, style));
    }
    spans
}

/// The filter box: `/ query` with a reversed cursor while typing, and on the
/// right how many rows match (or that none do).
fn filter_line(app: &App, width: usize) -> Line<'static> {
    let g = &app.glyphs;
    let f = &app.filter;
    let count = format!("{}/{}", app.rows.len(), app.all_rows.len());
    let mut right_text = if f.active() && app.rows.is_empty() {
        format!("no matches {count}")
    } else {
        count
    };
    if !f.editing {
        right_text.push_str("  esc clears");
    }
    let right_color = if f.active() && app.rows.is_empty() {
        Color::Red
    } else {
        DIM
    };

    let avail = width.saturating_sub(text_width(&right_text) + 3).max(1);
    let chars: Vec<char> = f.query.chars().collect();
    // Scroll the query so the cursor stays in view.
    let cursor = f.cursor.min(chars.len());
    let start = (cursor + 1).saturating_sub(avail).min(chars.len());
    let shown: Vec<char> = chars[start..].to_vec();
    let at = cursor - start;

    let mut spans = vec![Span::styled(
        "/ ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    let text_style = Style::default().fg(Color::White);
    if f.editing {
        let before: String = shown[..at.min(shown.len())].iter().collect();
        let under = shown.get(at).copied().unwrap_or(' ');
        let after: String = shown.iter().skip(at + 1).collect();
        spans.push(Span::styled(before, text_style));
        spans.push(Span::styled(
            under.to_string(),
            text_style.add_modifier(Modifier::REVERSED),
        ));
        spans.push(Span::styled(after, text_style));
    } else {
        spans.push(Span::styled(shown.iter().collect::<String>(), text_style));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    let gap = width.saturating_sub(used + text_width(&right_text));
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(
        fit(&right_text, width.saturating_sub(used), g),
        Style::default().fg(right_color),
    ));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyphs::{GlyphMode, Glyphs};

    #[test]
    fn the_frees_cell_is_right_aligned_and_shows_unknown_as_an_ellipsis() {
        let nerd = Glyphs::new(GlyphMode::Nerd);
        let ascii = Glyphs::new(GlyphMode::Ascii);
        assert_eq!(frees_cell(Some(1500), &nerd), "   1.5 KB");
        assert_eq!(frees_cell(Some(0), &nerd), "      0 B");
        assert_eq!(frees_cell(None, &nerd), "        …");
        assert_eq!(frees_cell(None, &ascii), "      ...");
        for text in [
            frees_cell(Some(123_456_789_012), &nerd),
            frees_cell(None, &nerd),
        ] {
            assert_eq!(text_width(&text), 9);
        }
    }
}
