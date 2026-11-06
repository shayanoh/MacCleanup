use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, SELECTED_BG, fit, fit_pad, panel, right, safety_color};
use crate::app::{App, GraphMode};
use crate::fmt::{eighths_bar, human_size};
use crate::safety::Safety;
use crate::scan::Row;
use crate::treemap::squarify;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let g = &app.glyphs;
    let title = match app.graph {
        GraphMode::Treemap => " Treemap ",
        _ => " Distribution ",
    };
    let block = panel(g, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 8 || inner.height < 3 {
        return;
    }
    let rows: Vec<Row> = app.rows.iter().map(Row::live).collect();
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing to show", Style::default().fg(DIM))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    }
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    match app.graph {
        GraphMode::Treemap => treemap(frame, body, app, &rows),
        _ => bars(frame, body, app, &rows),
    }
    let caption = caption(app, &rows);
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&caption, inner.width as usize, g),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
}

fn pct_of(size: u64, total: u64) -> u64 {
    (size as u128 * 100 / total.max(1) as u128) as u64
}

fn caption(app: &App, rows: &[Row]) -> String {
    let total = app.current().size().max(1);
    match rows.get(app.cursor) {
        Some(r) => format!(
            "selected: {} {}% of {}",
            r.name.to_string_lossy(),
            pct_of(r.size, total),
            app.current().name.to_string_lossy()
        ),
        None => "selected: nothing".to_string(),
    }
}

fn bars(frame: &mut Frame, area: Rect, app: &App, rows: &[Row]) {
    let g = &app.glyphs;
    let total = app.current().size().max(1);
    let name_w = (area.width as usize / 3).clamp(6, 16);
    let pct_w = 5;
    let bar_w = (area.width as usize)
        .saturating_sub(name_w + pct_w + 2)
        .max(1);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .take(area.height as usize)
        .map(|(i, r)| {
            let safety = app.policy.classify(&r.path);
            let line = Line::from(vec![
                Span::raw(fit_pad(&r.name.to_string_lossy(), name_w, g)),
                Span::raw(" "),
                Span::styled(
                    eighths_bar(r.size as f64 / total as f64, bar_w, g),
                    Style::default().fg(safety_color(safety)),
                ),
                Span::raw(right(&format!("{}%", pct_of(r.size, total)), pct_w)),
            ]);
            if i == app.cursor {
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
    frame.render_widget(Paragraph::new(lines), area);
}

fn treemap(frame: &mut Frame, area: Rect, app: &App, rows: &[Row]) {
    let g = &app.glyphs;
    if area.width < 4 || area.height < 1 {
        return;
    }
    let weights: Vec<u64> = rows.iter().map(|r| r.size).collect();
    let rects = squarify(&weights, area.width, area.height);
    for (i, cell) in rects.iter().enumerate() {
        let Some(c) = cell else { continue };
        let r = &rows[i];
        let rect = Rect::new(area.x + c.x, area.y + c.y, c.w, c.h);
        let safety = app.policy.classify(&r.path);
        let selected = i == app.cursor;
        let marked = app.marks.contains_key(&r.path);
        let bg = fill_color(safety, selected);
        let fg = if selected { Color::White } else { Color::Gray };
        let style = Style::default().bg(bg).fg(fg);
        let w = rect.width as usize;

        let mut lines: Vec<Line> = Vec::new();
        for y in 0..rect.height {
            let text = if y == 0 {
                let prefix = if selected {
                    format!("{} ", g.cursor())
                } else {
                    String::new()
                };
                fit_pad(&format!("{prefix}{}", r.name.to_string_lossy()), w, g)
            } else if y == 1 && rect.height >= 3 {
                fit_pad(&human_size(r.size), w, g)
            } else if marked {
                g.hatch().repeat(w)
            } else {
                " ".repeat(w)
            };
            let mut st = style;
            if y == 0 {
                st = st.add_modifier(Modifier::BOLD);
            }
            lines.push(Line::from(Span::styled(text, st)));
        }
        frame.render_widget(Paragraph::new(lines), rect);
    }
}

fn fill_color(safety: Safety, selected: bool) -> Color {
    match (safety, selected) {
        (Safety::Safe, false) => Color::Rgb(28, 84, 52),
        (Safety::Safe, true) => Color::Rgb(40, 140, 80),
        (Safety::Caution, false) => Color::Rgb(104, 84, 28),
        (Safety::Caution, true) => Color::Rgb(176, 140, 40),
        (Safety::Protected, false) => Color::Rgb(104, 40, 40),
        (Safety::Protected, true) => Color::Rgb(176, 64, 64),
    }
}
