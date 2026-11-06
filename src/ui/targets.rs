//! The Caches & Projects screen: the Explorer's columns, graph and keys on
//! a list of project build output and tool caches, plus a pane that says
//! what the highlighted row is, how it comes back, and which other folders
//! hold the same data.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use super::explorer::{Cols, cols, header_line, number_spans};
use super::graph::{self, GraphData};
use super::{
    ACCENT, DIM, SELECTED_BG, fit, fit_pad, fit_start, panel, right, safety_color, safety_dot,
    safety_word, text_width,
};
use crate::anim;
use crate::app::{App, GraphMode, MarkState, SortBy};
use crate::filter::{self, Query};
use crate::fmt::{eighths_bar, human_size};
use crate::glyphs::Glyphs;
use crate::scan::Row;
use crate::targets::{Line, RowKind, SectionKind, TRow};

/// Below this width the side pane would be unreadable, so only the list shows.
const SIDE_MIN_TOTAL: u16 = 76;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width >= SIDE_MIN_TOTAL {
        let [list, side] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Percentage(app.graph_pct.max(30)),
        ])
        .areas(area);
        render_list(frame, list, app);
        render_side(frame, side, app);
    } else {
        render_list(frame, area, app);
    }
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let title = format!(" {} Caches & Projects ", g.folder());
    let block = panel(&g, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let filter_h = u16::from(app.filter.visible());
    if inner.height < 7 + filter_h || inner.width < 24 {
        return;
    }
    app.layout.list_top = inner.y + 3 + filter_h;
    app.layout.list_rows = inner.height - 4 - filter_h;
    app.ensure_visible();
    let width = inner.width as usize;

    frame.render_widget(
        Paragraph::new(totals_line(app, width)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(status_line(app, width)),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    if filter_h > 0 {
        frame.render_widget(
            Paragraph::new(filter_line(app, width)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
    }
    let c = cols(inner.width, &g);
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
    let query = filter::parse(&app.filter.query);
    let start = app.toffset.min(app.tlines.len());
    let end = (start + list_rows).min(app.tlines.len());
    let mut lines: Vec<TextLine> = app.tlines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let selected = start + i == app.tcursor;
            let text = match *line {
                Line::Section(kind) => section_line(app, kind, width),
                Line::Group(gi) => group_line(app, gi, &c),
                Line::Row { group, row } => row_line(app, &query, group, row, &c),
            };
            if selected {
                text.style(Style::default().bg(SELECTED_BG))
            } else {
                text
            }
        })
        .collect();
    if app.tlines.is_empty() {
        let msg = if app.filter.active() {
            "(nothing matches the filter)".to_string()
        } else if app.targets_building() {
            format!("(building the list{})", g.ellipsis())
        } else if app.scanner.is_idle() {
            "(nothing to clean was found in the scan)".to_string()
        } else {
            format!("(scanning{})", g.ellipsis())
        };
        lines.push(TextLine::from(Span::styled(msg, Style::default().fg(DIM))));
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    if app.tlines.len() > list_rows {
        let mut state = ScrollbarState::new(app.tlines.len() - list_rows).position(app.toffset);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(g.scroll_track()))
            .thumb_symbol(g.scroll_thumb())
            .style(Style::default().fg(ACCENT));
        frame.render_stateful_widget(bar, list_area, &mut state);
    }

    // The selection summary and the position in the list.
    let pos = if app.tlines.is_empty() {
        "0/0".to_string()
    } else {
        format!(
            "{}/{}",
            app.tcursor.min(app.tlines.len() - 1) + 1,
            app.tlines.len()
        )
    };
    let pos_w = text_width(&pos) as u16;
    let summary_w = inner.width.saturating_sub(pos_w + 1) as usize;
    let y = inner.y + inner.height - 1;
    frame.render_widget(
        Paragraph::new(summary_line(app, summary_w)),
        Rect::new(inner.x, y, summary_w as u16, 1),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(pos, Style::default().fg(Color::White))),
        Rect::new(inner.x + inner.width - pos_w, y, pos_w, 1),
    );
}

/// Totals for everything listed, above the status line.
fn totals_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let (mut size, mut disk, mut rows) = (0u64, 0u64, 0usize);
    for grp in &app.targets.groups {
        size += grp.size();
        disk += grp.disk();
        rows += grp.rows.len();
    }
    let text = format!(
        "{} {} items {} {} ({} on disk)",
        g.folder(),
        rows,
        g.dot(),
        human_size(size),
        human_size(disk)
    );
    TextLine::from(Span::styled(
        fit(&text, width, g),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Scan activity, the tool lookup, and a warning when the scan root is not
/// the home folder (then caches outside it are not listed).
fn status_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let idx = anim::spinner_frame(app.tick, g.spinner().len());
    let mut spans: Vec<Span> = Vec::new();
    if app.scanner.is_idle() {
        spans.push(Span::styled(
            g.check().to_string(),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::raw(" scan done"));
    } else {
        spans.push(Span::styled(
            g.spinner()[idx].to_string(),
            Style::default().fg(ACCENT),
        ));
        spans.push(Span::raw(format!(" scanning{}", g.ellipsis())));
    }
    if app.tools_looking() {
        spans.push(Span::raw(format!(
            " {} looking for installed tools{}",
            g.dot(),
            g.ellipsis()
        )));
    }
    if app.targets_building() && app.targets.groups.is_empty() {
        spans.push(Span::raw(format!(
            " {} building the list{}",
            g.dot(),
            g.ellipsis()
        )));
    }
    if app.fda_missing {
        spans.push(Span::styled(
            format!(
                " {} Full Disk Access is missing: some caches cannot be listed",
                g.warn()
            ),
            Style::default().fg(Color::Yellow),
        ));
    }
    if app.stack[0].path != app.policy.home() {
        spans.push(Span::styled(
            format!(
                " {} scan root is {}: only what is under it is listed",
                g.warn(),
                fit_start(&app.stack[0].path.to_string_lossy(), 30, g)
            ),
            Style::default().fg(Color::Yellow),
        ));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used > width {
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        return TextLine::from(fit(&text, width, g));
    }
    TextLine::from(spans)
}

/// The filter box (same look as the Explorer's).
fn filter_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let f = &app.filter;
    let shown = app
        .tlines
        .iter()
        .filter(|l| matches!(l, Line::Row { .. }))
        .count();
    let total: usize = app.targets.groups.iter().map(|x| x.rows.len()).sum();
    let mut right_text = format!("{shown} shown");
    if !f.editing {
        right_text.push_str("  esc clears");
    }
    let avail = width.saturating_sub(text_width(&right_text) + 3).max(1);
    let chars: Vec<char> = f.query.chars().collect();
    let cursor = f.cursor.min(chars.len());
    let start = (cursor + 1).saturating_sub(avail).min(chars.len());
    let visible: Vec<char> = chars[start..].to_vec();
    let at = cursor - start;
    let mut spans = vec![Span::styled(
        "/ ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    let text_style = Style::default().fg(Color::White);
    if f.editing {
        let before: String = visible[..at.min(visible.len())].iter().collect();
        let under = visible.get(at).copied().unwrap_or(' ');
        let after: String = visible.iter().skip(at + 1).collect();
        spans.push(Span::styled(before, text_style));
        spans.push(Span::styled(
            under.to_string(),
            text_style.add_modifier(Modifier::REVERSED),
        ));
        spans.push(Span::styled(after, text_style));
    } else {
        spans.push(Span::styled(visible.iter().collect::<String>(), text_style));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    let gap = width.saturating_sub(used + text_width(&right_text));
    spans.push(Span::raw(" ".repeat(gap)));
    let color = if f.active() && shown == 0 {
        Color::Red
    } else {
        DIM
    };
    spans.push(Span::styled(
        fit(
            &format!("{right_text} of {total}"),
            width.saturating_sub(used),
            g,
        ),
        Style::default().fg(color),
    ));
    TextLine::from(spans)
}

/// A full-width band: blue PROJECTS, purple TOOLS & CACHES.
fn section_line(app: &App, kind: SectionKind, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let (title, bg) = match kind {
        SectionKind::Projects => (" PROJECTS", Color::Blue),
        SectionKind::Tools => (" TOOLS & CACHES", Color::Magenta),
    };
    let (mut disk, mut marked) = (0u64, 0usize);
    for grp in app.targets.groups.iter().filter(|x| x.section == kind) {
        disk += grp.disk();
        marked += grp
            .rows
            .iter()
            .filter(|r| app.row_mark(r) == MarkState::Marked)
            .count();
    }
    let frees = if marked == 0 {
        String::new()
    } else {
        match app.section_frees(kind) {
            Some(f) => format!("   {marked} marked {} frees {}", g.dot(), human_size(f)),
            None => format!("   {marked} marked {} frees {}", g.dot(), g.ellipsis()),
        }
    };
    let text = format!("{title}   {} on disk{frees}", human_size(disk));
    let padded = fit_pad(&text, width, g);
    TextLine::from(Span::styled(
        padded,
        Style::default()
            .fg(Color::Black)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    ))
}

/// A project or a tool: bold title with its totals in the number columns.
fn group_line(app: &App, gi: usize, c: &Cols) -> TextLine<'static> {
    let g = &app.glyphs;
    let grp = &app.targets.groups[gi];
    let fold = if grp.open { "v" } else { ">" };
    let marked = grp
        .rows
        .iter()
        .filter(|r| app.row_mark(r) == MarkState::Marked)
        .count();
    let mut title = format!("{fold} {}", grp.title);
    if !grp.subtitle.is_empty() {
        title.push_str(&format!("  {}", grp.subtitle));
    }
    if marked > 0 {
        title.push_str(&format!("  {marked}/{} marked", grp.rows.len()));
    }
    let name_cell = format!(
        "{}{}",
        " ".repeat(c.prefix_w.saturating_sub(2)),
        fit_pad(&title, c.name_w + 2, g)
    );
    let mut spans = vec![Span::styled(
        fit_pad(&name_cell, c.prefix_w + c.name_w, g),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];
    spans.extend(number_spans(&grp.as_row(), false, app.sort, None, g));
    TextLine::from(spans)
}

fn weight(sort: SortBy, r: &Row) -> u64 {
    match sort {
        SortBy::Size => r.size,
        SortBy::Frees if r.frees.is_some_and(|f| f > 0) => r.frees.unwrap_or(0),
        _ => r.disk,
    }
}

fn row_line(app: &App, query: &Query, gi: usize, ri: usize, c: &Cols) -> TextLine<'static> {
    let g = &app.glyphs;
    let t = app.targets.row(gi, ri);
    let r = t.row.live();
    let selected = app.tlines.get(app.tcursor) == Some(&Line::Row { group: gi, row: ri });
    let state = app.row_mark(t);
    let is_command = t.is_command();

    let cursor = if selected { g.cursor() } else { " " };
    let (mark, mark_style) = match &state {
        MarkState::Marked => (g.marked(), Style::default().fg(Color::Magenta)),
        MarkState::Covered(_) => (g.marked(), Style::default().fg(DIM)),
        MarkState::Unmarked => (g.unmarked(), Style::default().fg(Color::Magenta)),
    };
    let icon = if is_command { g.command() } else { g.folder() };
    let name_style = match &state {
        MarkState::Marked => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        MarkState::Covered(_) => Style::default().fg(DIM),
        MarkState::Unmarked => Style::default().fg(Color::Blue),
    };
    let name = r.name.to_string_lossy().into_owned();
    let queued = r.queued();
    let group_total: u64 = app.targets.groups[gi]
        .rows
        .iter()
        .map(|x| weight(app.sort, &x.row))
        .sum::<u64>()
        .max(1);
    let share = if queued {
        super::pad("queued".to_string(), 8)
    } else if r.scanning {
        let idx = anim::spinner_frame(app.tick, g.spinner().len());
        super::pad(format!("{} {}", g.spinner()[idx], r.files), 8)
    } else {
        eighths_bar(weight(app.sort, &r) as f64 / group_total as f64, 8, g)
    };

    let mut spans = vec![
        Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!("{} ", super::pad(mark.to_string(), c.mark_w)),
            mark_style,
        ),
        Span::styled(
            format!("{} ", super::pad(icon.to_string(), c.icon_w)),
            Style::default().fg(ACCENT),
        ),
        Span::styled(
            format!("{} ", safety_dot(g, t.safety)),
            Style::default().fg(safety_color(t.safety)),
        ),
    ];
    spans.extend(highlight(
        &fit_pad(&format!("  {name}"), c.name_w, g),
        &query
            .matches(&name)
            .unwrap_or_default()
            .iter()
            .map(|p| p + 2)
            .collect::<Vec<_>>(),
        name_style,
        name_style.fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    // A tool command's Frees is an upper bound, so it says so.
    let frees_text = is_command.then(|| {
        right(
            &format!(
                "{}{}",
                if g.mode == crate::glyphs::GlyphMode::Ascii {
                    "<="
                } else {
                    "≤"
                },
                human_size(r.frees.unwrap_or(0))
            ),
            9,
        )
    });
    spans.extend(number_spans(&r, queued, app.sort, frees_text, g));
    if c.show_share {
        let color = if queued { DIM } else { Color::Yellow };
        spans.push(Span::styled(
            format!(" {share}"),
            Style::default().fg(color),
        ));
    }
    TextLine::from(spans)
}

fn highlight(text: &str, positions: &[usize], base: Style, hit: Style) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_hit = false;
    for (i, ch) in text.chars().enumerate() {
        let is_hit = positions.contains(&i);
        if is_hit != run_hit && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_hit { hit } else { base },
            ));
        }
        run_hit = is_hit;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_hit { hit } else { base }));
    }
    spans
}

/// One line under the list: how much is marked and what it will really free.
fn summary_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let count = app.marks.len() + app.tool_marks.len();
    if count == 0 {
        return TextLine::from(Span::styled(
            fit("nothing marked (space marks, d reviews)", width, g),
            Style::default().fg(DIM),
        ));
    }
    let idx = anim::spinner_frame(app.tick, g.spinner().len());
    let frees = match app.joint_frees() {
        Some(f) => human_size(f),
        None => g.spinner()[idx].to_string(),
    };
    let mut text = format!("marked {count} {} frees {frees} now", g.dot());
    let up_to = app.tools_up_to();
    if up_to > 0 {
        text.push_str(&format!(
            " {} + up to {} by tool commands",
            g.dot(),
            human_size(up_to)
        ));
    }
    text.push_str(&format!(" {} d review", g.dot()));
    TextLine::from(Span::styled(
        fit(&text, width, g),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ))
}

/// The graph (unless hidden) over the detail pane.
fn render_side(frame: &mut Frame, area: Rect, app: &App) {
    let g = app.glyphs;
    if app.graph == GraphMode::Hidden || area.height < 16 {
        render_detail(frame, area, app, &g);
        return;
    }
    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(45), Constraint::Fill(1)]).areas(area);
    graph::render_data(frame, top, &g, app.graph, &graph_data(app));
    render_detail(frame, bottom, app, &g);
}

/// The rows of the group under the cursor (or of the whole section).
fn graph_data(app: &App) -> GraphData {
    let (rows, cursor): (Vec<&TRow>, Option<usize>) = match app.tlines.get(app.tcursor).copied() {
        Some(Line::Row { group, row }) => {
            (app.targets.groups[group].rows.iter().collect(), Some(row))
        }
        Some(Line::Group(group)) => (app.targets.groups[group].rows.iter().collect(), None),
        Some(Line::Section(kind)) => (
            app.targets
                .groups
                .iter()
                .filter(|x| x.section == kind)
                .flat_map(|x| x.rows.iter())
                .collect(),
            None,
        ),
        None => (Vec::new(), None),
    };
    let weights: Vec<u64> = rows.iter().map(|t| weight(app.sort, &t.row)).collect();
    let scope = match app.tlines.get(app.tcursor).copied() {
        Some(Line::Row { group, .. }) | Some(Line::Group(group)) => {
            app.targets.groups[group].title.clone()
        }
        Some(Line::Section(SectionKind::Projects)) => "projects".to_string(),
        Some(Line::Section(SectionKind::Tools)) => "tools & caches".to_string(),
        None => String::new(),
    };
    GraphData {
        total: weights.iter().sum(),
        safeties: rows.iter().map(|t| t.safety).collect(),
        marked: rows
            .iter()
            .map(|t| app.row_mark(t) == MarkState::Marked)
            .collect(),
        rows: rows.iter().map(|t| t.row.live()).collect(),
        weights,
        cursor,
        scope,
    }
}

fn render_detail(frame: &mut Frame, area: Rect, app: &App, g: &Glyphs) {
    let block = panel(g, " Details ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 10 || inner.height < 2 {
        return;
    }
    let lines = detail_lines(app, inner.width as usize, inner.height as usize);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn detail_lines(app: &App, width: usize, height: usize) -> Vec<TextLine<'static>> {
    let g = &app.glyphs;
    let dim = Style::default().fg(DIM);
    let mut out: Vec<TextLine<'static>> = Vec::new();
    let Some(t) = app.selected_target() else {
        out.push(TextLine::from(Span::styled(
            match app.tlines.get(app.tcursor) {
                Some(Line::Section(_)) => "space marks every folder in this section",
                Some(Line::Group(_)) => "space marks every folder here (h folds, l opens)",
                _ => "nothing selected",
            },
            dim,
        )));
        return out;
    };
    let r = &t.row;
    out.push(TextLine::from(vec![
        Span::styled(
            format!("{} ", safety_dot(g, t.safety)),
            Style::default().fg(safety_color(t.safety)),
        ),
        Span::styled(
            fit(&r.name.to_string_lossy(), width.saturating_sub(12), g),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", safety_word(t.safety)),
            Style::default().fg(safety_color(t.safety)),
        ),
    ]));
    out.push(TextLine::from(Span::styled(
        fit_start(&r.path.to_string_lossy(), width, g),
        dim,
    )));
    out.push(TextLine::from(format!("comes back: {}", t.regen)));
    if let RowKind::Command { spec, .. } = &t.kind {
        out.push(TextLine::from(Span::styled(
            format!("cleaned with: {}", spec.display()),
            Style::default().fg(Color::Red),
        )));
        out.push(TextLine::from(Span::styled(
            fit_start(&format!("via {}", spec.program), width, g),
            dim,
        )));
    }
    if let Some(note) = &t.note {
        out.push(TextLine::from(Span::styled(
            note.clone(),
            Style::default().fg(Color::Yellow),
        )));
    }
    out.push(TextLine::raw(""));
    if let Some(sp) = r.space {
        out.push(TextLine::from(Span::styled(
            fit(
                &format!(
                    "frees now {} {} held by snapshots {}",
                    human_size(sp.frees_now),
                    g.dot(),
                    human_size(sp.snapshots)
                ),
                width,
                g,
            ),
            Style::default().fg(Color::Green),
        )));
        out.push(TextLine::from(Span::styled(
            fit(
                &format!(
                    "shared with other places {} (stays)",
                    human_size(sp.shared_outside)
                ),
                width,
                g,
            ),
            Style::default().fg(Color::Yellow),
        )));
        out.push(TextLine::from(Span::styled(
            fit(
                &format!(
                    "shared inside {} {} cloning inside saves {}",
                    human_size(sp.shared_inside),
                    g.dot(),
                    human_size(sp.saved)
                ),
                width,
                g,
            ),
            dim,
        )));
    } else {
        out.push(TextLine::from(Span::styled(
            fit(&format!("still scanning{}", g.ellipsis()), width, g),
            dim,
        )));
    }
    if !app.marks.is_empty() || !app.tool_marks.is_empty() {
        let text = match app.joint_frees() {
            Some(f) => format!("with everything you marked: frees {}", human_size(f)),
            None => format!("with everything you marked: working it out{}", g.ellipsis()),
        };
        out.push(TextLine::from(Span::styled(
            fit(&text, width, g),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let holders = app.holders_view();
    if !holders.is_empty() {
        out.push(TextLine::raw(""));
        let unmarked = holders
            .iter()
            .filter(|h| h.state == MarkState::Unmarked)
            .count();
        out.push(TextLine::from(Span::styled(
            fit(
                &format!(
                    "also holds this data ({} not marked, m marks them)",
                    unmarked
                ),
                width,
                g,
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        let room = height.saturating_sub(out.len()).max(1);
        let home = app.policy.home();
        for h in holders.iter().take(room) {
            let (glyph, style) = match h.state {
                MarkState::Marked => (g.marked(), Style::default().fg(Color::Magenta)),
                MarkState::Covered(_) => (g.marked(), dim),
                MarkState::Unmarked => (g.unmarked(), Style::default()),
            };
            let size = right(&human_size(h.usage.disk), 9);
            let name_w = width.saturating_sub(text_width(glyph) + 2 + 9);
            let shown = crate::targets::display_path(home, &h.path);
            out.push(TextLine::from(Span::styled(
                format!(
                    "{glyph} {} {size}",
                    super::pad(fit_start(&shown, name_w, g), name_w)
                ),
                style,
            )));
        }
        if holders.len() > room {
            out.push(TextLine::from(Span::styled(
                format!("{} and {} more", g.ellipsis(), holders.len() - room),
                dim,
            )));
        }
    }
    out
}
