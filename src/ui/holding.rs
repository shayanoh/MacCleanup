use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{ACCENT, DIM, SELECTED_BG, fit, fit_start, pad, panel, right, text_width};
use crate::app::App;
use crate::fmt::{both_usage, human_size};
use crate::softdelete::HeldItem;
use crate::usage::Usage;

fn age(deleted_at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let secs = now.saturating_sub(deleted_at);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let block = panel(&g, format!(" {} ToBeDeleted ", g.nav(7)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 24 {
        return;
    }
    app.layout.list_top = inner.y + 2;
    app.layout.list_rows = inner.height - 3;
    app.ensure_visible();

    let total: Usage = app.held.iter().map(HeldItem::usage).sum();
    let summary = format!(
        "{} {} items {} {} {} {}",
        g.pending(),
        app.held.len(),
        g.dot(),
        both_usage(total),
        g.dot(),
        app.holding.dir().to_string_lossy()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&summary, inner.width as usize, &g),
            Style::default().fg(ACCENT),
        )),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let list_area = Rect::new(inner.x, inner.y + 2, inner.width, app.layout.list_rows);
    if app.held.is_empty() {
        frame.render_widget(
            Paragraph::new("ToBeDeleted is empty. Nothing is waiting to be deleted for good.")
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(DIM)),
            list_area,
        );
    } else {
        let size_w = 9;
        let age_w = 9;
        let prefix_w = 2 + text_width(g.folder()) + 1;
        let path_w = (inner.width as usize)
            .saturating_sub(prefix_w + size_w * 2 + age_w + 3)
            .max(6);
        let start = app.held_offset.min(app.held.len());
        let end = (start + app.layout.list_rows as usize).min(app.held.len());
        let lines: Vec<Line> = app.held[start..end]
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let selected = start + i == app.held_cursor;
                let cursor = if selected { g.cursor() } else { " " };
                let original = item.original.to_string_lossy();
                let line = Line::from(vec![
                    Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
                    Span::styled(format!("{} ", g.folder()), Style::default().fg(ACCENT)),
                    Span::raw(pad(fit_start(&original, path_w, &g), path_w)),
                    Span::raw(format!(" {}", right(&human_size(item.size), size_w))),
                    Span::styled(
                        format!(" {}", right(&human_size(item.disk), size_w)),
                        Style::default().fg(DIM),
                    ),
                    Span::styled(
                        format!(" {}", pad(age(item.deleted_at), age_w)),
                        Style::default().fg(DIM),
                    ),
                ]);
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
        frame.render_widget(Paragraph::new(lines), list_area);
    }

    let hints = format!(
        "r restore {} D delete forever {} E empty all",
        g.dot(),
        g.dot()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&hints, inner.width as usize, &g),
            Style::default().fg(DIM),
        )),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}
