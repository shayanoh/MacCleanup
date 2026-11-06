//! The Docker page (drawn like Caches & Projects). First a plain list so
//! the page works; the full layout comes with its own task.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

use super::{fit, panel};
use crate::app::App;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let block = panel(&g, " Docker ".to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 || inner.width < 10 {
        return;
    }
    app.layout.list_top = inner.y;
    app.layout.list_rows = inner.height;
    app.ensure_visible();
    let start = app.docker.offset.min(app.docker.lines.len());
    let end = (start + inner.height as usize).min(app.docker.lines.len());
    let text: Vec<ratatui::text::Line> = app.docker.lines[start..end]
        .iter()
        .map(|l| ratatui::text::Line::from(fit(&l.key, inner.width as usize, &g)))
        .collect();
    frame.render_widget(Paragraph::new(text), inner);
}
