use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, SELECTED_BG, panel};
use crate::app::{App, Focus, SIDEBAR, Screen};
use crate::fmt::human_size;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let g = &app.glyphs;
    let mut block = panel(g, " Menu ");
    if app.focus == Focus::Sidebar {
        block = block.border_style(Style::default().fg(ACCENT));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let holds_items = !app.held.is_empty();
    let lines: Vec<Line> = SIDEBAR
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let active = entry.screen == Some(app.screen);
            let is_holding = entry.screen == Some(Screen::Holding);
            let marker = if active { g.cursor() } else { " " };
            let icon = if is_holding {
                g.trash(holds_items)
            } else {
                g.nav(i)
            };
            let style = match (entry.screen, active) {
                (None, _) => Style::default().fg(DIM),
                (Some(_), true) => Style::default()
                    .fg(ACCENT)
                    .bg(SELECTED_BG)
                    .add_modifier(Modifier::BOLD),
                (Some(_), false) => Style::default(),
            };
            // ToBeDeleted lights up when it holds something, so it is hard to
            // forget about.
            let icon_style = if is_holding && holds_items {
                style.fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                style
            };
            Line::from(vec![
                Span::styled(marker.to_string(), style),
                Span::styled(icon.to_string(), icon_style),
                Span::styled(format!(" {}", entry.label), style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);

    // Pending (soft-deleted, not freed yet) sits at the bottom: a count and
    // sizes when ToBeDeleted holds something, "empty" when it does not.
    let h = inner.height.min(3);
    let bottom = Rect::new(inner.x, inner.y + inner.height - h, inner.width, h);
    let pending = app.accounting.pending();
    let lines = if holds_items {
        vec![
            Line::from(Span::styled(
                format!(" pending ({})", app.held.len()),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(" {} {}", g.pending(), human_size(pending.size))),
            Line::from(Span::styled(
                format!(" {} disk", human_size(pending.disk)),
                Style::default().fg(DIM),
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(" pending", Style::default().fg(DIM))),
            Line::from(Span::styled(" empty", Style::default().fg(DIM))),
        ]
    };
    frame.render_widget(Paragraph::new(lines), bottom);
}
