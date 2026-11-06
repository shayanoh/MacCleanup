use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, SELECTED_BG, fit, fit_pad, panel, right, safety_color};
use crate::app::{App, GraphMode};
use crate::fmt::{eighths_bar, human_size};
use crate::glyphs::Glyphs;
use crate::safety::Safety;
use crate::scan::Row;
use crate::treemap::squarify;

/// What a graph shows: a list of rows with how much each weighs, so the
/// Explorer (one folder) and Caches & Projects (one group) draw the same way.
pub struct GraphData {
    pub rows: Vec<Row>,
    /// The size each row is drawn with (on disk, or what sorting uses).
    pub weights: Vec<u64>,
    pub safeties: Vec<Safety>,
    pub marked: Vec<bool>,
    /// The highlighted row, if the cursor is on one of these.
    pub cursor: Option<usize>,
    /// What 100% means.
    pub total: u64,
    /// "of <scope>" in the caption: the folder or group name.
    pub scope: String,
}

impl GraphData {
    /// The Explorer's current folder.
    pub fn from_explorer(app: &App) -> GraphData {
        let rows: Vec<Row> = app.rows.iter().map(Row::live).collect();
        GraphData {
            weights: rows.iter().map(|r| app.weight(r)).collect(),
            safeties: rows.iter().map(|r| app.policy.classify(&r.path)).collect(),
            marked: rows
                .iter()
                .map(|r| app.marks.contains_key(&r.path))
                .collect(),
            cursor: (app.cursor < rows.len()).then_some(app.cursor),
            total: app.weight_total(),
            scope: app.current().name.to_string_lossy().into_owned(),
            rows,
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    render_data(
        frame,
        area,
        &app.glyphs,
        app.graph,
        &GraphData::from_explorer(app),
    );
}

pub fn render_data(frame: &mut Frame, area: Rect, g: &Glyphs, mode: GraphMode, data: &GraphData) {
    let title = match mode {
        GraphMode::Treemap => " Treemap ",
        _ => " Distribution ",
    };
    let block = panel(g, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 8 || inner.height < 3 {
        return;
    }
    if data.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing to show", Style::default().fg(DIM))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    }
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    match mode {
        GraphMode::Treemap => treemap(frame, body, g, data),
        _ => bars(frame, body, g, data),
    }
    let caption = caption(data);
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

fn caption(data: &GraphData) -> String {
    let total = data.total.max(1);
    match data.cursor.and_then(|i| data.rows.get(i).map(|r| (i, r))) {
        Some((i, r)) => format!(
            "selected: {} {}% of {}",
            r.name.to_string_lossy(),
            pct_of(data.weights[i], total),
            data.scope
        ),
        None => "selected: nothing".to_string(),
    }
}

fn bars(frame: &mut Frame, area: Rect, g: &Glyphs, data: &GraphData) {
    let total = data.total.max(1);
    let name_w = (area.width as usize / 3).clamp(6, 16);
    let pct_w = 5;
    let bar_w = (area.width as usize)
        .saturating_sub(name_w + pct_w + 2)
        .max(1);
    let lines: Vec<Line> = data
        .rows
        .iter()
        .enumerate()
        .take(area.height as usize)
        .map(|(i, r)| {
            let weight = data.weights[i];
            let line = Line::from(vec![
                Span::raw(fit_pad(&r.name.to_string_lossy(), name_w, g)),
                Span::raw(" "),
                Span::styled(
                    eighths_bar(weight as f64 / total as f64, bar_w, g),
                    Style::default().fg(safety_color(data.safeties[i])),
                ),
                Span::raw(right(&format!("{}%", pct_of(weight, total)), pct_w)),
            ]);
            if data.cursor == Some(i) {
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

fn treemap(frame: &mut Frame, area: Rect, g: &Glyphs, data: &GraphData) {
    if area.width < 4 || area.height < 1 {
        return;
    }
    let rects = squarify(&data.weights, area.width, area.height);
    for (i, cell) in rects.iter().enumerate() {
        let Some(c) = cell else { continue };
        let r = &data.rows[i];
        let rect = Rect::new(area.x + c.x, area.y + c.y, c.w, c.h);
        let selected = data.cursor == Some(i);
        let marked = data.marked[i];
        let bg = fill_color(data.safeties[i], selected, i);
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
                fit_pad(
                    &format!("{} ({} on disk)", human_size(r.size), human_size(r.disk)),
                    w,
                    g,
                )
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

/// Box colour: the safety hue, with three alternating shades so boxes of the
/// same kind next to each other stay visually distinct. The selected box is
/// brighter and ignores the shade.
pub fn fill_color(safety: Safety, selected: bool, index: usize) -> Color {
    if selected {
        let (r, g, b) = match safety {
            Safety::Safe => (40, 140, 80),
            Safety::Caution => (176, 140, 40),
            Safety::Protected => (176, 64, 64),
        };
        return Color::Rgb(r, g, b);
    }
    let (r, g, b) = match safety {
        Safety::Safe => (28u8, 84u8, 52u8),
        Safety::Caution => (104, 84, 28),
        Safety::Protected => (104, 40, 40),
    };
    const SHADES: [f32; 3] = [0.8, 1.0, 1.25];
    let f = SHADES[index % SHADES.len()];
    let scale = |c: u8| (c as f32 * f).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(scale(r), scale(g), scale(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbouring_boxes_of_the_same_kind_get_different_shades() {
        for safety in [Safety::Safe, Safety::Caution, Safety::Protected] {
            for i in 0..12 {
                assert_ne!(
                    fill_color(safety, false, i),
                    fill_color(safety, false, i + 1),
                    "{safety:?} {i}"
                );
            }
        }
    }

    #[test]
    fn selected_box_stands_out_from_every_shade() {
        for safety in [Safety::Safe, Safety::Caution, Safety::Protected] {
            let selected = fill_color(safety, true, 0);
            for i in 0..6 {
                assert_ne!(selected, fill_color(safety, false, i));
            }
            assert_eq!(
                selected,
                fill_color(safety, true, 5),
                "selected ignores index"
            );
        }
    }

    #[test]
    fn shades_stay_recognisably_the_same_hue() {
        // Same family: the dominant channel is the same for every shade.
        let dominant = |c: Color| match c {
            Color::Rgb(r, g, b) => {
                if r >= g && r >= b {
                    'r'
                } else if g >= b {
                    'g'
                } else {
                    'b'
                }
            }
            _ => '?',
        };
        for safety in [Safety::Safe, Safety::Caution, Safety::Protected] {
            let first = dominant(fill_color(safety, false, 0));
            for i in 1..6 {
                assert_eq!(dominant(fill_color(safety, false, i)), first);
            }
        }
    }
}
