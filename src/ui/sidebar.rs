use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, SELECTED_BG, panel};
use crate::app::{App, Focus, SIDEBAR};
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

    let lines: Vec<Line> = SIDEBAR
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let active = entry.screen == Some(app.screen);
            let marker = if active { g.cursor() } else { " " };
            let text = format!("{marker}{} {}", g.nav(i), entry.label);
            let style = match (entry.screen, active) {
                (None, _) => Style::default().fg(DIM),
                (Some(_), true) => Style::default()
                    .fg(ACCENT)
                    .bg(SELECTED_BG)
                    .add_modifier(Modifier::BOLD),
                (Some(_), false) => Style::default(),
            };
            Line::from(Span::styled(text, style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);

    // Pending (soft-deleted, not freed yet) sits at the bottom.
    let h = inner.height.min(3);
    let bottom = Rect::new(inner.x, inner.y + inner.height - h, inner.width, h);
    let pending = app.accounting.pending();
    let lines = vec![
        Line::from(Span::styled(" pending", Style::default().fg(DIM))),
        Line::from(format!(" {} {}", g.pending(), human_size(pending.size))),
        Line::from(Span::styled(
            format!("   {} disk", human_size(pending.disk)),
            Style::default().fg(DIM),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), bottom);
}
