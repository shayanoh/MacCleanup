//! The file browser for a Docker image or volume: the Explorer's look (name,
//! size with folders summed, modified) on a read-only listing. Nothing here
//! can be marked or changed.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::explorer::value_style;
use super::graph::{self, GraphData};
use super::targets::highlight;
use super::{ACCENT, DIM, SELECTED_BG, fit, fit_start, pad, panel, right, text_width};
use crate::anim;
use crate::app::{App, GraphMode};
use crate::docker::browse::{BrowseSort, BrowseState};
use crate::filter;
use crate::fmt::{ago, eighths_bar, human_size, now_secs};
use crate::glyphs::Glyphs;
use crate::safety::Safety;
use crate::scan::Row;

/// Below this width the graph would be unreadable, so only the list shows.
const SIDE_MIN_TOTAL: u16 = 76;

const SIZE_W: usize = 9;
const MODIFIED_W: usize = 10;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.graph != GraphMode::Hidden && area.width >= SIDE_MIN_TOTAL {
        let [list, side] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(app.graph_pct)])
                .areas(area);
        render_list(frame, list, app);
        if let Some(b) = app.docker.browse.as_ref() {
            let g = app.glyphs;
            app.graph_hits = graph::render_data(frame, side, &g, app.graph, &graph_data(b));
        }
    } else {
        render_list(frame, area, app);
    }
}

/// The graph shows the listed entries, sized by their size (folders with
/// everything inside); a click on a box selects that entry.
fn graph_data(b: &BrowseState) -> GraphData {
    let rows: Vec<Row> = b
        .shown
        .iter()
        .map(|&i| {
            let e = &b.all[i];
            Row {
                name: e.name.clone().into(),
                path: std::path::PathBuf::new(),
                is_dir: e.is_dir,
                size: e.size,
                disk: e.size,
                files: 0,
                scanning: false,
                node: None,
                space: None,
                frees: None,
                mtime: e.mtime,
            }
        })
        .collect();
    let weights: Vec<u64> = rows.iter().map(|r| r.size).collect();
    GraphData {
        total: weights.iter().sum(),
        // Read-only: nothing here is offered for deletion, so no colour
        // says "safe".
        safeties: vec![Safety::Caution; rows.len()],
        marked: vec![false; rows.len()],
        keys: (0..rows.len()).map(|i| (i, 0)).collect(),
        cursor: (!rows.is_empty()).then(|| b.cursor.min(rows.len() - 1)),
        scope: b.dir.clone(),
        weights,
        rows,
    }
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let Some(b) = app.docker.browse.as_ref() else {
        return;
    };
    let title = format!(
        " {} Docker {} {} {} ",
        g.folder(),
        g.dot(),
        b.label,
        fit_start(&b.dir, (area.width as usize).saturating_sub(40).max(8), &g)
    );
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
    let Some(b) = app.docker.browse.as_ref() else {
        return;
    };

    frame.render_widget(
        Paragraph::new(totals_line(b, width, &g)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    frame.render_widget(
        Paragraph::new(status_line(app, b, width)),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );
    if filter_h > 0 {
        frame.render_widget(
            Paragraph::new(filter_line(app, b, width)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
    }
    let prefix_w = 2 + text_width(g.folder()).max(text_width(g.file())) + 1;
    let fixed = 1 + prefix_w + 1 + SIZE_W + 1 + MODIFIED_W;
    let show_share = width >= fixed + 9 + 16;
    let name_w = width
        .saturating_sub(fixed + if show_share { 9 } else { 0 })
        .max(4);
    frame.render_widget(
        Paragraph::new(header_line(b.sort, prefix_w, name_w, show_share, &g)),
        Rect::new(inner.x, inner.y + 2 + filter_h, inner.width, 1),
    );

    let list_rows = app.layout.list_rows as usize;
    let list_area = Rect::new(
        inner.x,
        inner.y + 3 + filter_h,
        inner.width,
        app.layout.list_rows,
    );
    let query = filter::parse(&app.filter.query);
    let start = b.offset.min(b.shown.len());
    let end = (start + list_rows).min(b.shown.len());
    let now = now_secs();
    let total: u64 = b.all.iter().map(|e| e.size).sum();
    let mut lines: Vec<TextLine> = (start..end)
        .map(|i| {
            let e = &b.all[b.shown[i]];
            let selected = i == b.cursor;
            let text = entry_line(
                &g,
                &query,
                e,
                &RowCtx {
                    selected,
                    name_w,
                    now,
                    sort: b.sort,
                    share: show_share.then_some(total),
                },
            );
            if selected {
                text.style(Style::default().bg(SELECTED_BG))
            } else {
                text
            }
        })
        .collect();
    if b.shown.is_empty() {
        let msg = if let Some(e) = &b.error {
            e.clone()
        } else if b.loading() {
            format!("(reading the folder{})", g.ellipsis())
        } else if app.filter.active() {
            "(nothing matches the filter)".to_string()
        } else {
            "(empty)".to_string()
        };
        lines.push(TextLine::from(Span::styled(
            fit(&msg, width, &g),
            Style::default().fg(if b.error.is_some() {
                Color::Yellow
            } else {
                DIM
            }),
        )));
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    if b.shown.len() > list_rows {
        let mut state = ScrollbarState::new(b.shown.len() - list_rows).position(b.offset);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(g.scroll_track()))
            .thumb_symbol(g.scroll_thumb())
            .style(Style::default().fg(ACCENT));
        frame.render_stateful_widget(bar, list_area, &mut state);
    }

    let pos = if b.shown.is_empty() {
        "0/0".to_string()
    } else {
        format!("{}/{}", b.cursor.min(b.shown.len() - 1) + 1, b.shown.len())
    };
    let pos_w = text_width(&pos) as u16;
    let hint_w = inner.width.saturating_sub(pos_w + 1) as usize;
    let y = inner.y + inner.height - 1;
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(
                "read-only: nothing here can be changed (l opens a folder, h goes up, esc goes back)",
                hint_w,
                &g,
            ),
            Style::default().fg(DIM),
        )),
        Rect::new(inner.x, y, hint_w as u16, 1),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(pos, Style::default().fg(Color::White))),
        Rect::new(inner.x + inner.width - pos_w, y, pos_w, 1),
    );
}

fn totals_line(b: &BrowseState, width: usize, g: &Glyphs) -> TextLine<'static> {
    let total: u64 = b.all.iter().map(|e| e.size).sum();
    let text = format!(
        "{} {} items {} {} (folders counted with everything inside)",
        g.folder(),
        b.all.len(),
        g.dot(),
        human_size(total)
    );
    TextLine::from(Span::styled(
        fit(&text, width, g),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))
}

fn status_line(app: &App, b: &BrowseState, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let mut spans: Vec<Span> = Vec::new();
    if b.loading() {
        let idx = anim::spinner_frame(app.tick, g.spinner().len());
        spans.push(Span::styled(
            g.spinner()[idx].to_string(),
            Style::default().fg(ACCENT),
        ));
        spans.push(Span::raw(format!(
            " reading the folder{} {}",
            g.ellipsis(),
            crate::fmt::marquee(app.tick, 8, g)
        )));
    } else if let Some(e) = &b.error {
        spans.push(Span::styled(
            format!("{} {e}", g.warn()),
            Style::default().fg(Color::Yellow),
        ));
    } else {
        spans.push(Span::styled(
            g.check().to_string(),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::raw(format!(
            " {}",
            fit_start(&b.dir, width.saturating_sub(4), g)
        )));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used > width {
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        return TextLine::from(fit(&text, width, g));
    }
    TextLine::from(spans)
}

fn filter_line(app: &App, b: &BrowseState, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let f = &app.filter;
    let mut right_text = format!("{} shown", b.shown.len());
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
    let color = if f.active() && b.shown.is_empty() {
        Color::Red
    } else {
        DIM
    };
    spans.push(Span::styled(
        fit(&right_text, width.saturating_sub(used), g),
        Style::default().fg(color),
    ));
    TextLine::from(spans)
}

fn header_line(
    sort: BrowseSort,
    prefix_w: usize,
    name_w: usize,
    show_share: bool,
    g: &Glyphs,
) -> TextLine<'static> {
    // The sorted column is white with an arrow, the others gray.
    let head = |by: BrowseSort, text: &str, w: usize| -> Span<'static> {
        let sorted = sort == by;
        let arrow = if !sorted {
            String::new()
        } else if by == BrowseSort::Name {
            format!(" {}", g.sort_asc())
        } else {
            format!(" {}", g.sort_desc())
        };
        let style = if sorted {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        };
        Span::styled(right(&format!("{text}{arrow}"), w), style)
    };
    let name_style = if sort == BrowseSort::Name {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let name_arrow = if sort == BrowseSort::Name {
        format!(" {}", g.sort_asc())
    } else {
        String::new()
    };
    let mut spans = vec![
        Span::raw(" ".repeat(prefix_w)),
        Span::styled(pad(format!("Name{name_arrow}"), name_w), name_style),
        Span::raw(" "),
        head(BrowseSort::Size, "Size", SIZE_W),
        Span::raw(" "),
        head(BrowseSort::Modified, "Modified", MODIFIED_W),
    ];
    if show_share {
        spans.push(Span::styled(" Share", Style::default().fg(DIM)));
    }
    TextLine::from(spans)
}

/// What a row needs besides its entry.
struct RowCtx {
    selected: bool,
    name_w: usize,
    now: i64,
    sort: BrowseSort,
    /// Sum of the listed sizes when the share bar is shown.
    share: Option<u64>,
}

fn entry_line(
    g: &Glyphs,
    query: &filter::Query,
    e: &crate::docker::browse::FileEntry,
    ctx: &RowCtx,
) -> TextLine<'static> {
    let cursor = if ctx.selected { g.cursor() } else { " " };
    let icon = if e.is_dir { g.folder() } else { g.file() };
    let style = if e.is_dir {
        Style::default().fg(Color::Blue)
    } else {
        Style::default()
    };
    let shown = fit(&e.name, ctx.name_w, g);
    let hits: Vec<usize> = if query.is_empty() {
        Vec::new()
    } else {
        query.matches(&shown).unwrap_or_default()
    };
    let mut spans = vec![
        Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!(
                "{} ",
                pad(
                    icon.to_string(),
                    text_width(g.folder()).max(text_width(g.file()))
                )
            ),
            Style::default().fg(ACCENT),
        ),
    ];
    spans.extend(highlight(
        &pad(shown, ctx.name_w),
        &hits,
        style,
        style.fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        format!(" {}", right(&human_size(e.size), SIZE_W)),
        value_style(ctx.sort == BrowseSort::Size),
    ));
    spans.push(Span::styled(
        format!(" {}", right(&ago(ctx.now, e.mtime), MODIFIED_W)),
        value_style(ctx.sort == BrowseSort::Modified),
    ));
    if let Some(total) = ctx.share {
        let frac = if total == 0 {
            0.0
        } else {
            e.size as f64 / total as f64
        };
        spans.push(Span::styled(
            format!(" {}", eighths_bar(frac, 8, g)),
            Style::default().fg(Color::Yellow),
        ));
    }
    TextLine::from(spans)
}
