use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use std::sync::Arc;

use super::{ACCENT, DIM, SELECTED_BG, fit, fit_pad, panel, right, safety_color};
use crate::app::{App, GraphMode, GraphTip, SortBy};
use crate::fmt::{eighths_bar, human_size};
use crate::glyphs::Glyphs;
use crate::safety::Safety;
use crate::scan::{Node, Row};
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
    /// What a click on each row selects: `(group, row)` on Caches & Projects,
    /// `(0, row)` in the Explorer.
    pub keys: Vec<(usize, usize)>,
    /// True when the weights are logical sizes, false for on-disk sizes;
    /// nested boxes inside a folder are weighed the same way.
    pub by_size: bool,
}

/// Where a click lands: the area drawn for a row, and what it selects.
pub type GraphHit = (Rect, (usize, usize));

/// What a graph drew: where a click lands, and what each box or bar is (for
/// the hover tooltip; nested boxes come after the box that holds them).
#[derive(Default)]
pub struct GraphOut {
    pub hits: Vec<GraphHit>,
    pub tips: Vec<GraphTip>,
}

impl GraphOut {
    /// Remember it for this frame's clicks and hovers.
    pub fn store(self, app: &mut App) {
        app.graph_hits = self.hits;
        app.graph_tips = self.tips;
    }
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
            keys: (0..rows.len()).map(|i| (0, i)).collect(),
            by_size: app.sort == SortBy::Size,
            rows,
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) -> GraphOut {
    render_data(
        frame,
        area,
        &app.glyphs,
        app.graph,
        &GraphData::from_explorer(app),
        app.hover,
    )
}

/// Draw the graph and return where each row can be clicked and hovered.
/// `hover` is the mouse position, if known: the box under it is highlighted.
pub fn render_data(
    frame: &mut Frame,
    area: Rect,
    g: &Glyphs,
    mode: GraphMode,
    data: &GraphData,
    hover: Option<(u16, u16)>,
) -> GraphOut {
    let title = match mode {
        GraphMode::Treemap => " Treemap ",
        _ => " Distribution ",
    };
    let block = panel(g, title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 8 || inner.height < 3 {
        return GraphOut::default();
    }
    if data.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing to show", Style::default().fg(DIM))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return GraphOut::default();
    }
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    let out = match mode {
        GraphMode::Treemap => treemap(frame, body, g, data, hover),
        _ => bars(frame, body, g, data),
    };
    let caption = caption(data);
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(&caption, inner.width as usize, g),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
    );
    out
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

fn bars(frame: &mut Frame, area: Rect, g: &Glyphs, data: &GraphData) -> GraphOut {
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
    let shown = data.rows.len().min(area.height as usize);
    let line = |i: usize| Rect::new(area.x, area.y + i as u16, area.width, 1);
    GraphOut {
        hits: (0..shown).map(|i| (line(i), data.keys[i])).collect(),
        tips: (0..shown)
            .map(|i| GraphTip {
                rect: line(i),
                name: data.rows[i].name.to_string_lossy().into_owned(),
                size: data.rows[i].size,
                disk: data.rows[i].disk,
                pct: pct_of(data.weights[i], total),
            })
            .collect(),
    }
}

/// A box needs at least this many cells to hold the boxes of its contents
/// (one cell of border all around, the top one carrying the name).
const NEST_MIN_W: u16 = 14;
const NEST_MIN_H: u16 = 6;
/// Nesting stops here even in a huge terminal.
const NEST_MAX_DEPTH: usize = 6;

/// One box of the treemap: a row of the graph, a sub-folder, or the loose
/// files of a folder (`node` is None for those).
struct Item {
    name: String,
    size: u64,
    disk: u64,
    node: Option<Arc<Node>>,
}

struct Ctx<'a> {
    g: &'a Glyphs,
    by_size: bool,
    total: u64,
    hover: Option<(u16, u16)>,
    tips: Vec<GraphTip>,
}

/// How a box is coloured and marked.
#[derive(Clone, Copy)]
struct Look {
    safety: Safety,
    selected: bool,
    marked: bool,
    /// Position among its siblings (picks the shade).
    index: usize,
    depth: usize,
}

fn within(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn treemap(
    frame: &mut Frame,
    area: Rect,
    g: &Glyphs,
    data: &GraphData,
    hover: Option<(u16, u16)>,
) -> GraphOut {
    if area.width < 4 || area.height < 1 {
        return GraphOut::default();
    }
    let mut hits = Vec::new();
    let mut cx = Ctx {
        g,
        by_size: data.by_size,
        total: data.total.max(1),
        hover,
        tips: Vec::new(),
    };
    let rects = squarify(&data.weights, area.width, area.height);
    for (i, cell) in rects.iter().enumerate() {
        let Some(c) = cell else { continue };
        let r = &data.rows[i];
        let rect = Rect::new(area.x + c.x, area.y + c.y, c.w, c.h);
        hits.push((rect, data.keys[i]));
        let item = Item {
            name: r.name.to_string_lossy().into_owned(),
            size: r.size,
            disk: r.disk,
            node: r.node.clone(),
        };
        let look = Look {
            safety: data.safeties[i],
            selected: data.cursor == Some(i),
            marked: data.marked[i],
            index: i,
            depth: 0,
        };
        draw_box(frame, &mut cx, rect, &item, data.weights[i], look);
    }
    GraphOut {
        hits,
        tips: cx.tips,
    }
}

/// The boxes inside a folder box: its sub-folders plus one box for the files
/// lying directly in it, so the areas stay proportional. Empty when the box
/// is too small, or there is nothing to show inside.
fn contents(cx: &Ctx, rect: Rect, item: &Item, depth: usize) -> Vec<Item> {
    let Some(node) = &item.node else {
        return Vec::new();
    };
    if rect.width < NEST_MIN_W || rect.height < NEST_MIN_H || depth >= NEST_MAX_DEPTH {
        return Vec::new();
    }
    let weigh = |size: u64, disk: u64| if cx.by_size { size } else { disk };
    let mut items: Vec<Item> = node
        .children()
        .into_iter()
        .filter(|c| weigh(c.size(), c.disk()) > 0)
        .map(|c| Item {
            name: c.name.to_string_lossy().into_owned(),
            size: c.size(),
            disk: c.disk(),
            node: Some(c),
        })
        .collect();
    if items.is_empty() {
        return items;
    }
    let (size, disk) = items
        .iter()
        .fold((0, 0), |(s, d), i| (s + i.size, d + i.disk));
    let (loose_size, loose_disk) = (
        node.size().saturating_sub(size),
        node.disk().saturating_sub(disk),
    );
    if weigh(loose_size, loose_disk) > 0 {
        items.push(Item {
            name: format!("(files in {})", item.name),
            size: loose_size,
            disk: loose_disk,
            node: None,
        });
    }
    items
}

fn draw_box(frame: &mut Frame, cx: &mut Ctx, rect: Rect, item: &Item, weight: u64, look: Look) {
    let kids = contents(cx, rect, item, look.depth);
    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    );
    // Over the border or the name row of a box with contents counts as the
    // box itself; over the contents it belongs to the box inside.
    let hovered = cx
        .hover
        .is_some_and(|p| within(rect, p) && (kids.is_empty() || !within(inner, p)));
    let mut bg = fill_color(look.safety, look.selected, look.index, look.depth);
    if hovered {
        bg = brighten(bg);
    }
    let fg = if look.selected || hovered {
        Color::White
    } else {
        Color::Gray
    };
    let style = Style::default().bg(bg).fg(fg);
    let w = rect.width as usize;
    let g = cx.g;

    let mut lines: Vec<Line> = Vec::new();
    for y in 0..rect.height {
        let text = if y == 0 {
            let prefix = if look.selected {
                format!("{} ", g.cursor())
            } else {
                String::new()
            };
            fit_pad(&format!("{prefix}{}", item.name), w, g)
        } else if y == 1 && rect.height >= 3 && kids.is_empty() {
            fit_pad(
                &format!(
                    "{} ({} on disk)",
                    human_size(item.size),
                    human_size(item.disk)
                ),
                w,
                g,
            )
        } else if look.marked {
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
    cx.tips.push(GraphTip {
        rect,
        name: item.name.clone(),
        size: item.size,
        disk: item.disk,
        pct: pct_of(weight, cx.total),
    });

    if kids.is_empty() {
        return;
    }
    let weights: Vec<u64> = kids
        .iter()
        .map(|k| if cx.by_size { k.size } else { k.disk })
        .collect();
    for (i, cell) in squarify(&weights, inner.width, inner.height)
        .iter()
        .enumerate()
    {
        let Some(c) = cell else { continue };
        let sub = Rect::new(inner.x + c.x, inner.y + c.y, c.w, c.h);
        let look = Look {
            selected: false,
            marked: false,
            index: i,
            depth: look.depth + 1,
            ..look
        };
        draw_box(frame, cx, sub, &kids[i], weights[i], look);
    }
}

fn brighten(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            r.saturating_add(36),
            g.saturating_add(36),
            b.saturating_add(36),
        ),
        other => other,
    }
}

/// How long the pointer must rest on a box before its tooltip appears.
pub const HOVER_TICKS: u64 = 10;

/// The tooltip for the box under the mouse, drawn last so it floats over
/// everything.
pub fn tooltip(frame: &mut Frame, app: &App) {
    let Some(pos) = app.hover else { return };
    if app.tick.saturating_sub(app.hover_since) < HOVER_TICKS {
        return;
    }
    // Boxes inside boxes come later in the list: the last match is the
    // innermost.
    let Some(tip) = app.graph_tips.iter().rev().find(|t| within(t.rect, pos)) else {
        return;
    };
    let screen = frame.area();
    let g = &app.glyphs;
    let name = super::clean(&tip.name, g);
    let info = [
        format!(
            "{} ({} on disk)",
            human_size(tip.size),
            human_size(tip.disk)
        ),
        format!("{}% of the graph", tip.pct),
    ];
    let widest = info
        .iter()
        .map(|l| l.chars().count())
        .chain(std::iter::once(name.chars().count()))
        .max()
        .unwrap_or(0);
    let w = (widest as u16 + 2)
        .min(48)
        .min(screen.width.saturating_sub(2))
        .max(4);
    let name_rows = Line::from(name.as_str())
        .width()
        .div_ceil((w - 2) as usize)
        .max(1) as u16;
    let h = (name_rows + info.len() as u16 + 2).min(screen.height);
    let x = (pos.0 + 2).min(screen.right().saturating_sub(w));
    let y = if pos.1 + 1 + h <= screen.bottom() {
        pos.1 + 1
    } else {
        pos.1.saturating_sub(h)
    };
    let rect = Rect::new(x.max(screen.x), y.max(screen.y), w, h);
    let mut lines = vec![Line::styled(
        name,
        Style::default().add_modifier(Modifier::BOLD),
    )];
    lines.extend(info.map(|l| Line::styled(l, Style::default().fg(Color::Gray))));
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(Color::Rgb(22, 24, 36)).fg(Color::White))
            .block(panel(g, "").border_style(Style::default().fg(ACCENT))),
        rect,
    );
}

/// Box colour: the safety hue, with three alternating shades so boxes of the
/// same kind next to each other stay visually distinct. Boxes nested inside
/// a box use a much darker set of shades on every other level, so a box never
/// blends into the one holding it. The selected box is brighter and ignores
/// the shade.
pub fn fill_color(safety: Safety, selected: bool, index: usize, depth: usize) -> Color {
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
    const SHADES: [[f32; 3]; 2] = [[0.8, 1.0, 1.25], [0.3, 0.42, 0.54]];
    let f = SHADES[depth % 2][index % 3];
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
                    fill_color(safety, false, i, 0),
                    fill_color(safety, false, i + 1, 0),
                    "{safety:?} {i}"
                );
            }
        }
    }

    #[test]
    fn selected_box_stands_out_from_every_shade() {
        for safety in [Safety::Safe, Safety::Caution, Safety::Protected] {
            let selected = fill_color(safety, true, 0, 0);
            for i in 0..6 {
                assert_ne!(selected, fill_color(safety, false, i, 0));
                assert_ne!(selected, fill_color(safety, false, i, 1));
            }
            assert_eq!(
                selected,
                fill_color(safety, true, 5, 3),
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
            let first = dominant(fill_color(safety, false, 0, 0));
            for i in 1..6 {
                assert_eq!(dominant(fill_color(safety, false, i, i)), first);
            }
        }
    }

    #[test]
    fn nested_boxes_never_share_a_shade_with_the_box_holding_them() {
        for safety in [Safety::Safe, Safety::Caution, Safety::Protected] {
            for depth in 0..4 {
                for parent in 0..3 {
                    for child in 0..3 {
                        assert_ne!(
                            fill_color(safety, false, parent, depth),
                            fill_color(safety, false, child, depth + 1),
                            "{safety:?} depth {depth} {parent}/{child}"
                        );
                    }
                }
            }
        }
    }
}
