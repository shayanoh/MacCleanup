//! The Caches & Projects screen.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, SELECTED_BG, fit, panel};
use crate::app::App;
use crate::fmt::human_size;
use crate::targets::{Line, SectionKind};

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let block = panel(&g, " Caches & Projects ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 10 || inner.height < 3 {
        return;
    }
    app.layout.list_top = inner.y;
    app.layout.list_rows = inner.height;
    app.ensure_visible();
    let width = inner.width as usize;
    let start = app.toffset.min(app.tlines.len());
    let end = (start + inner.height as usize).min(app.tlines.len());
    let lines: Vec<TextLine> = app.tlines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let selected = start + i == app.tcursor;
            let (text, style) = match *line {
                Line::Section(SectionKind::Projects) => (
                    "PROJECTS".to_string(),
                    Style::default().fg(Color::Black).bg(Color::Blue),
                ),
                Line::Section(SectionKind::Tools) => (
                    "TOOLS & CACHES".to_string(),
                    Style::default().fg(Color::Black).bg(Color::Magenta),
                ),
                Line::Group(gi) => {
                    let grp = &app.targets.groups[gi];
                    let fold = if grp.open { "v" } else { ">" };
                    (
                        format!("{fold} {}", grp.title),
                        Style::default().add_modifier(Modifier::BOLD),
                    )
                }
                Line::Row { group, row } => {
                    let r = &app.targets.row(group, row).row;
                    (
                        format!(
                            "    {}  {} ({} on disk)",
                            r.name.to_string_lossy(),
                            human_size(r.size),
                            human_size(r.disk)
                        ),
                        Style::default(),
                    )
                }
            };
            let line = TextLine::from(Span::styled(fit(&text, width, &g), style));
            if selected {
                line.style(Style::default().bg(SELECTED_BG).fg(ACCENT))
            } else {
                line
            }
        })
        .collect();
    let mut lines = lines;
    if app.tlines.is_empty() {
        lines.push(TextLine::from(Span::styled(
            if app.scanner.is_idle() {
                "(nothing to clean found)"
            } else {
                "(scanning...)"
            },
            Style::default().fg(DIM),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
