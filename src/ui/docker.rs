//! The Docker page, drawn like Caches & Projects: totals and status lines,
//! the filter box, coloured section bands, the Explorer's column look, a
//! graph over a Details pane. One list, no tabs: what holds what is shown
//! by indentation.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::explorer::{Cols, cols, value_style};
use super::graph::{self, GraphData};
use super::targets::{highlight, work_text};
use super::{
    ACCENT, DIM, SELECTED_BG, fit, fit_pad, fit_start, panel, right, safety_color, safety_dot,
    text_width,
};
use crate::anim;
use crate::app::{App, GraphMode};
use crate::docker::analysis::{ItemFacts, ItemKind, Refusal, frees_of_set};
use crate::docker::model::ContainerState;
use crate::docker::view::{DKind, DLine, DSection, DSort};
use crate::filter::{self, Query};
use crate::fmt::{ago, eighths_bar, human_size, now_secs};
use crate::glyphs::Glyphs;
use crate::safety::Safety;
use crate::scan::Row;

/// Below this width the side pane would be unreadable, so only the list shows.
const SIDE_MIN_TOTAL: u16 = 76;
/// "Last used ▼" must fit.
const LAST_USED_W: usize = 11;
const NUM_W: usize = 9;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width >= SIDE_MIN_TOTAL {
        let [list, side] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Percentage(app.graph_pct.max(30)),
        ])
        .areas(area);
        render_list(frame, list, app);
        app.graph_hits = render_side(frame, side, app);
    } else {
        render_list(frame, area, app);
    }
}

/// The columns: the Explorer's layout, with the Last used column a little
/// wider and no On disk column (Docker has one size).
fn docker_cols(width: u16, g: &Glyphs) -> Cols {
    let mut c = cols(width, g);
    // cols() reserves three 9-wide columns and the 10-wide Modified one;
    // Docker has 9 + 9 + 11 + 9.
    c.name_w = c.name_w.saturating_sub(1).max(4);
    c.show_share = c.show_share && c.name_w >= 12;
    c
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let g = app.glyphs;
    let block = panel(&g, format!(" {} Docker ", g.folder()));
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
    let c = docker_cols(inner.width, &g);
    frame.render_widget(
        Paragraph::new(header_line(app.docker.view.sort, &c, &g)),
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
    let lines = &app.docker.lines;
    let start = app.docker.offset.min(lines.len());
    let end = (start + list_rows).min(lines.len());
    let totals = section_totals(app);
    let now = now_secs();
    let mut out: Vec<TextLine> = (start..end)
        .map(|i| {
            let text = match lines[i].kind {
                DKind::Section(sec) => section_line(app, sec, width),
                _ => row_line(app, &query, i, &c, totals[i], now),
            };
            if i == app.docker.cursor {
                text.style(Style::default().bg(SELECTED_BG))
            } else {
                text
            }
        })
        .collect();
    if lines.is_empty() {
        out.push(TextLine::from(Span::styled(
            empty_message(app),
            Style::default().fg(if app.docker.error.is_some() {
                Color::Yellow
            } else {
                DIM
            }),
        )));
    }
    frame.render_widget(Paragraph::new(out), list_area);
    if lines.len() > list_rows {
        let mut state = ScrollbarState::new(lines.len() - list_rows).position(app.docker.offset);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(g.scroll_track()))
            .thumb_symbol(g.scroll_thumb())
            .style(Style::default().fg(ACCENT));
        frame.render_stateful_widget(bar, list_area, &mut state);
    }

    let pos = if lines.is_empty() {
        "0/0".to_string()
    } else {
        format!(
            "{}/{}",
            app.docker.cursor.min(lines.len() - 1) + 1,
            lines.len()
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

fn empty_message(app: &App) -> String {
    let g = &app.glyphs;
    if let Some(e) = &app.docker.error {
        e.message()
    } else if app.filter.active() {
        "(nothing matches the filter)".to_string()
    } else if app.docker.job.pending || !app.docker.read_once {
        format!("(reading Docker{})", g.ellipsis())
    } else {
        "(Docker holds nothing)".to_string()
    }
}

/// Counts and sizes of everything read.
fn totals_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let text = match app.docker.data() {
        Some(d) => format!(
            "{} {} images {} {} volumes {} {} cache records {} {} in use inside Docker",
            g.folder(),
            d.images.len(),
            g.dot(),
            d.volumes.len(),
            g.dot(),
            d.cache.len(),
            g.dot(),
            human_size(d.in_use())
        ),
        None => format!("{} Docker", g.folder()),
    };
    TextLine::from(Span::styled(
        fit(&text, width, g),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))
}

/// The connection (or the read in progress, or why there is none) and the
/// VM disk: what `Docker.raw` takes on the Mac against what Docker uses.
fn status_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let mut spans: Vec<Span> = Vec::new();
    if app.docker.job.pending {
        let idx = anim::spinner_frame(app.tick, g.spinner().len());
        spans.push(Span::styled(
            g.spinner()[idx].to_string(),
            Style::default().fg(ACCENT),
        ));
        spans.push(Span::raw(format!(
            " {}",
            work_text(
                &app.docker.work.snapshot(),
                "reading Docker",
                app.tick,
                8,
                g
            )
        )));
    } else if let Some(e) = &app.docker.error {
        spans.push(Span::styled(
            format!("{} {}", g.warn(), e.message()),
            Style::default().fg(Color::Yellow),
        ));
    } else {
        spans.push(Span::styled(
            g.check().to_string(),
            Style::default().fg(Color::Green),
        ));
        spans.push(Span::raw(" docker ok"));
    }
    if let Some(d) = app.docker.data()
        && let Some(raw) = d.vm_allocated
    {
        let used = d.in_use();
        let mut text = format!(
            " {} VM disk {} on disk, {} in use inside",
            g.dot(),
            human_size(raw),
            human_size(used)
        );
        if raw > used {
            text.push_str(&format!(" ({} not given back)", human_size(raw - used)));
        }
        spans.push(Span::raw(text));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used > width {
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        return TextLine::from(fit(&text, width, g));
    }
    TextLine::from(spans)
}

fn filter_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let f = &app.filter;
    let shown = app
        .docker
        .lines
        .iter()
        .filter(|l| !matches!(l.kind, DKind::Section(_)))
        .count();
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
        fit(&right_text, width.saturating_sub(used), g),
        Style::default().fg(color),
    ));
    TextLine::from(spans)
}

/// Column titles: the sorted one is white with an arrow, the others gray.
fn header_line(sort: DSort, c: &Cols, g: &Glyphs) -> TextLine<'static> {
    let label = |by: DSort, text: &str, w: usize| -> Span<'static> {
        let arrow = if sort == by {
            format!(
                " {}",
                if by == DSort::Name {
                    g.sort_asc()
                } else {
                    g.sort_desc()
                }
            )
        } else {
            String::new()
        };
        let style = if sort == by {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        };
        Span::styled(right(&format!("{text}{arrow}"), w), style)
    };
    let name = {
        let style = if sort == DSort::Name {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        };
        let arrow = if sort == DSort::Name {
            format!(" {}", g.sort_asc())
        } else {
            String::new()
        };
        Span::styled(super::pad(format!("Name{arrow}"), c.name_w), style)
    };
    let mut header = vec![
        Span::raw(" ".repeat(c.prefix_w)),
        name,
        Span::raw(" "),
        label(DSort::Size, "Size", NUM_W),
        Span::raw(" "),
        label(DSort::Frees, "Frees", NUM_W),
        Span::raw(" "),
        label(DSort::LastUsed, "Last used", LAST_USED_W),
        Span::raw(" "),
        label(DSort::Created, "Created", NUM_W),
    ];
    if c.show_share {
        header.push(Span::styled(" Share", Style::default().fg(DIM)));
    }
    TextLine::from(header)
}

fn section_title(sec: DSection) -> (&'static str, Color) {
    match sec {
        DSection::Images => (" IMAGES", Color::Blue),
        DSection::Volumes => (" VOLUMES", Color::Magenta),
        DSection::BuildCache => (" BUILD CACHE", Color::Green),
    }
}

/// The marked items of one kind of section.
fn section_marked(app: &App, sec: DSection) -> std::collections::BTreeSet<String> {
    let kinds: &[&str] = match sec {
        DSection::Images => &["image:", "container:"],
        DSection::Volumes => &["volume:"],
        DSection::BuildCache => &["cache:"],
    };
    app.docker_marked()
        .into_iter()
        .filter(|id| kinds.iter().any(|k| id.starts_with(k)))
        .collect()
}

fn section_line(app: &App, sec: DSection, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let (title, bg) = section_title(sec);
    let a = &app.docker.analysis;
    let mine: Vec<&ItemFacts> = a
        .items
        .values()
        .filter(|f| match sec {
            DSection::Images => matches!(f.kind, ItemKind::Image | ItemKind::Container),
            DSection::Volumes => f.kind == ItemKind::Volume,
            DSection::BuildCache => f.kind == ItemKind::CacheRecord,
        })
        .collect();
    let count = mine
        .iter()
        .filter(|f| f.kind != ItemKind::Container)
        .count();
    let size: u64 = mine.iter().map(|f| f.size).sum();
    let marked = section_marked(app, sec);
    let frees = if marked.is_empty() {
        String::new()
    } else {
        let f = match (app.docker.data(), app.docker.layers()) {
            (Some(d), Some(l)) => human_size(frees_of_set(a, d, l, &marked)),
            _ => g.ellipsis().to_string(),
        };
        format!("   {} marked {} frees {f}", marked.len(), g.dot())
    };
    let text = format!("{title}   {count} {} {}{frees}", g.dot(), human_size(size));
    TextLine::from(Span::styled(
        fit_pad(&text, width, g),
        Style::default()
            .fg(Color::Black)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    ))
}

/// For each line, the total its share bar is relative to: the sum of the
/// sizes of the top-level items of its section.
fn section_totals(app: &App) -> Vec<u64> {
    let lines = &app.docker.lines;
    let mut out = vec![0u64; lines.len()];
    let mut from = 0;
    for i in 0..=lines.len() {
        let boundary = i == lines.len() || matches!(lines[i].kind, DKind::Section(_));
        if boundary {
            let total: u64 = (from..i)
                .filter(|&j| lines[j].depth == 1)
                .filter_map(|j| app.docker.facts(&lines[j].key))
                .map(|f| f.size)
                .sum();
            out[from..i].fill(total);
            from = i;
        }
    }
    out
}

fn safety_of(f: &ItemFacts) -> Safety {
    match f.kind {
        ItemKind::CacheRecord => Safety::Safe,
        ItemKind::Image if f.dangling => Safety::Safe,
        _ => Safety::Caution,
    }
}

/// A build step as Docker words it, without the noise: "mount / from exec
/// /bin/sh -c apt-get update" becomes "apt-get update". Display only; the
/// details pane shows the original.
pub fn tidy_command(text: &str) -> String {
    let t = text
        .replace("mount / from exec ", "")
        .replace("/bin/sh -c ", "")
        .replace("#(nop) ", "")
        .replace(" # buildkit", "");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The other images that hold the same layer as `image`'s layer `diff`.
fn layer_shared_with(app: &App, image: &str, diff: &Option<String>) -> Vec<String> {
    let Some(diff) = diff else {
        return Vec::new();
    };
    let Some(all) = app.docker.layers() else {
        return Vec::new();
    };
    let mut ids: Vec<&String> = all
        .iter()
        .filter(|(id, ls)| {
            id.as_str() != image && ls.iter().any(|l| l.diff_id.as_ref() == Some(diff))
        })
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids.into_iter()
        .map(|id| {
            app.docker
                .facts(&format!("image:{id}"))
                .map_or_else(|| id.clone(), |f| f.name.clone())
        })
        .collect()
}

/// Is layer `n` the biggest of its image (the first one if tied)?
fn is_biggest_layer(app: &App, image: &str, n: usize) -> bool {
    let Some(layers) = app.docker.layers().and_then(|l| l.get(image)) else {
        return false;
    };
    let best = layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.size > 0)
        .max_by_key(|(i, l)| (l.size, std::cmp::Reverse(*i)))
        .map(|(i, _)| i);
    best == Some(n)
}

/// `(image id, layer number)` of a layer line's key.
fn layer_ref(key: &str) -> Option<(&str, usize)> {
    let (image, n) = key.strip_prefix("layer:")?.rsplit_once(':')?;
    Some((image, n.parse().ok()?))
}

fn row_line(
    app: &App,
    query: &Query,
    idx: usize,
    c: &Cols,
    section_total: u64,
    now: i64,
) -> TextLine<'static> {
    let g = &app.glyphs;
    let line: &DLine = &app.docker.lines[idx];
    let selected = idx == app.docker.cursor;
    let marked = app.tool_marks.contains(&line.key);
    let cursor = if selected { g.cursor() } else { " " };
    let indent = "  ".repeat(line.depth.saturating_sub(1) as usize);
    let fold = match line.open {
        Some(true) => "v ",
        Some(false) => "> ",
        None => "  ",
    };
    let dim = Style::default().fg(DIM);

    // What the row shows, by kind.
    let (label, tag, size, frees, last_used, created, safety, refused, markable) =
        if let DKind::Layer(n) = line.kind {
            let (image, _) = layer_ref(&line.key).unwrap_or(("", n));
            let layer = app
                .docker
                .layers()
                .and_then(|l| l.get(image))
                .and_then(|l| l.get(n));
            let shared =
                layer.is_some_and(|l| !layer_shared_with(app, image, &l.diff_id).is_empty());
            let biggest = is_biggest_layer(app, image, n);
            let size = layer.map_or(0, |l| l.size);
            (
                layer.map_or_else(String::new, |l| tidy_command(&l.command)),
                format!(
                    "{}{}",
                    if shared { "  shared" } else { "" },
                    if biggest { "  biggest" } else { "" }
                ),
                Some(size),
                Some(if size == 0 {
                    "-".to_string()
                } else if shared {
                    "shared".to_string()
                } else {
                    human_size(size)
                }),
                None,
                layer.map(|l| l.created),
                Safety::Safe,
                true,
                false,
            )
        } else if let Some(f) = app.docker.facts(&line.key) {
            let mut tag = String::new();
            match f.kind {
                ItemKind::Image => {
                    if f.dangling {
                        tag.push_str("  dangling");
                    } else if f.unused {
                        tag.push_str("  unused");
                    }
                }
                ItemKind::Container => {
                    let state = app.docker.data().and_then(|d| {
                        d.containers
                            .iter()
                            .find(|x| format!("container:{}", x.id) == line.key)
                            .map(|x| x.state)
                    });
                    tag.push_str(match state {
                        Some(ContainerState::Running) => "  running",
                        Some(ContainerState::Stopped) => "  stopped",
                        _ => "  paused",
                    });
                }
                ItemKind::Volume => {
                    if f.unused {
                        tag.push_str("  unused");
                    }
                }
                ItemKind::CacheRecord => {
                    if f.removable == Err(Refusal::CacheInUse) {
                        tag.push_str("  in use");
                    }
                }
                ItemKind::VmTrim => {}
            }
            let frees = if f.kind == ItemKind::Container {
                "-".to_string()
            } else {
                human_size(f.frees_alone)
            };
            (
                if f.kind == ItemKind::CacheRecord {
                    tidy_command(&f.name)
                } else {
                    f.name.clone()
                },
                tag,
                (f.kind != ItemKind::Container).then_some(f.size),
                Some(frees),
                Some((f.last_used, f.last_used_is_proxy)),
                Some(f.created),
                safety_of(f),
                f.removable.is_err(),
                true,
            )
        } else {
            (
                line.key.clone(),
                String::new(),
                None,
                None,
                None,
                None,
                Safety::Caution,
                false,
                false,
            )
        };
    let is_layer = matches!(line.kind, DKind::Layer(_));
    let (mark_glyph, mark_style) = if !markable {
        (" ", dim)
    } else if marked {
        (g.marked(), Style::default().fg(Color::Magenta))
    } else {
        (g.unmarked(), Style::default().fg(Color::Magenta))
    };
    let icon = if is_layer { g.file() } else { g.folder() };
    let name_style = if marked {
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD)
    } else if is_layer || refused {
        dim
    } else {
        Style::default().fg(Color::Blue)
    };

    let mut spans = vec![
        Span::styled(format!("{cursor} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!("{} ", super::pad(mark_glyph.to_string(), c.mark_w)),
            mark_style,
        ),
        Span::styled(
            format!("{} ", super::pad(icon.to_string(), c.icon_w)),
            Style::default().fg(ACCENT),
        ),
    ];
    if is_layer {
        spans.push(Span::styled(
            format!("{} ", " ".repeat(text_width(g.safe()))),
            dim,
        ));
    } else {
        spans.push(Span::styled(
            format!("{} ", safety_dot(g, safety)),
            Style::default().fg(safety_color(safety)),
        ));
    }
    // Name cell: indent, fold mark, the name (matches highlighted) and a tag.
    let lead = format!("{indent}{fold}");
    let room = c
        .name_w
        .saturating_sub(text_width(&lead) + text_width(&tag));
    let shown = fit(&label, room, g);
    let hits: Vec<usize> = if query.is_empty() || is_layer {
        Vec::new()
    } else {
        query
            .matches(&shown)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p + text_width(&lead))
            .collect()
    };
    let cell = super::pad(format!("{lead}{shown}"), text_width(&lead) + room);
    spans.extend(highlight(
        &cell,
        &hits,
        name_style,
        name_style.fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ));
    let tag_text = fit_pad(&tag, c.name_w.saturating_sub(text_width(&cell)), g);
    spans.push(Span::styled(tag_text, Style::default().fg(Color::Yellow)));

    let num = |text: String, w: usize, sorted: bool| {
        Span::styled(format!(" {}", right(&text, w)), value_style(sorted))
    };
    let sort = app.docker.view.sort;
    spans.push(num(
        size.map_or_else(|| "-".to_string(), human_size),
        NUM_W,
        sort == DSort::Size,
    ));
    spans.push(num(
        frees.unwrap_or_else(|| "-".to_string()),
        NUM_W,
        sort == DSort::Frees,
    ));
    spans.push(num(
        match last_used {
            Some((t, proxy)) => {
                let text = ago(now, t);
                if proxy && text != "-" {
                    format!("~{text}")
                } else {
                    text
                }
            }
            None => "-".to_string(),
        },
        LAST_USED_W,
        sort == DSort::LastUsed,
    ));
    spans.push(num(
        created.map_or_else(|| "-".to_string(), |t| ago(now, t)),
        NUM_W,
        sort == DSort::Created,
    ));
    if c.show_share {
        let bar = match size {
            Some(s) if !is_layer && section_total > 0 => {
                eighths_bar(s as f64 / section_total as f64, 8, g)
            }
            _ => " ".repeat(8),
        };
        spans.push(Span::styled(
            format!(" {bar}"),
            Style::default().fg(Color::Yellow),
        ));
    }
    TextLine::from(spans)
}

/// One line under the list: how much is marked and what it frees in Docker.
fn summary_line(app: &App, width: usize) -> TextLine<'static> {
    let g = &app.glyphs;
    let trim = app.tool_marks.contains(crate::docker::analysis::TRIM_ITEM);
    let marked = app.docker_marked().len() - usize::from(trim);
    if marked == 0 && !trim {
        return TextLine::from(Span::styled(
            fit("nothing marked (space marks, d reviews)", width, g),
            Style::default().fg(DIM),
        ));
    }
    let frees = app
        .docker_frees()
        .map_or_else(|| g.ellipsis().to_string(), human_size);
    let mut text = format!("marked {marked} {} frees {frees} in Docker", g.dot());
    if trim {
        text.push_str(&format!(" {} + VM disk trim", g.dot()));
    }
    let elsewhere = app.marks.len() + app.tool_marks.len() - marked - usize::from(trim);
    if elsewhere > 0 {
        text.push_str(&format!(" {} +{elsewhere} marked elsewhere", g.dot()));
    }
    text.push_str(&format!(" {} d review", g.dot()));
    TextLine::from(Span::styled(
        fit(&text, width, g),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ))
}

/// The graph (unless hidden) over the details pane.
fn render_side(frame: &mut Frame, area: Rect, app: &mut App) -> Vec<graph::GraphHit> {
    let g = app.glyphs;
    if app.graph == GraphMode::Hidden || area.height < 16 {
        render_detail(frame, area, app, &g);
        return Vec::new();
    }
    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(45), Constraint::Fill(1)]).areas(area);
    let hits = graph::render_data(frame, top, &g, app.graph, &graph_data(app));
    render_detail(frame, bottom, app, &g);
    hits
}

/// The graph shows the top-level items of the section holding the cursor:
/// one box or bar each, sized by their size.
fn graph_data(app: &App) -> GraphData {
    let lines = &app.docker.lines;
    let cursor = app.docker.cursor.min(lines.len().saturating_sub(1));
    let section_at = (0..=cursor.min(lines.len().saturating_sub(1)))
        .rev()
        .find(|&i| lines.get(i).is_some_and(|l| l.depth == 0))
        .unwrap_or(0);
    let items: Vec<usize> = (section_at + 1..lines.len())
        .take_while(|&i| lines[i].depth > 0)
        .filter(|&i| lines[i].depth == 1)
        .filter(|&i| app.docker.facts(&lines[i].key).is_some_and(|f| f.size > 0))
        .collect();
    // The top-level item the cursor is in (or on).
    let cursor_item = (0..=cursor)
        .rev()
        .find(|&i| lines.get(i).is_some_and(|l| l.depth == 1))
        .and_then(|i| items.iter().position(|x| *x == i));
    let rows: Vec<Row> = items
        .iter()
        .map(|&i| {
            let f = &app.docker.analysis.items[&lines[i].key];
            Row {
                name: f.name.clone().into(),
                path: std::path::PathBuf::new(),
                is_dir: true,
                size: f.size,
                disk: f.size,
                files: 0,
                scanning: false,
                node: None,
                space: None,
                frees: Some(f.frees_alone),
                mtime: f.last_used,
            }
        })
        .collect();
    let weights: Vec<u64> = rows.iter().map(|r| r.size).collect();
    let scope = match lines.get(section_at).map(|l| l.kind) {
        Some(DKind::Section(s)) => section_title(s).0.trim().to_string(),
        _ => "Docker".to_string(),
    };
    GraphData {
        total: weights.iter().sum(),
        safeties: items
            .iter()
            .map(|&i| safety_of(&app.docker.analysis.items[&lines[i].key]))
            .collect(),
        marked: items
            .iter()
            .map(|&i| app.tool_marks.contains(&lines[i].key))
            .collect(),
        // A click on a box selects that list line.
        keys: items.iter().map(|&i| (i, 0)).collect(),
        cursor: cursor_item,
        scope,
        weights,
        rows,
    }
}

fn render_detail(frame: &mut Frame, area: Rect, app: &mut App, g: &Glyphs) {
    let block = panel(g, " Details ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 10 || inner.height < 2 {
        return;
    }
    let lines = detail_lines(app, inner.width as usize);
    // Another line selected: start at the top again. The scroll can never
    // pass the point where the last line is at the bottom.
    let key = app
        .docker
        .selected()
        .map(|l| l.key.clone())
        .unwrap_or_default();
    if app.docker.detail_key != key {
        app.docker.detail_key = key;
        app.docker.detail_scroll = 0;
    }
    let max = lines.len().saturating_sub(inner.height as usize);
    app.docker.detail_max = max;
    app.docker.detail_scroll = app.docker.detail_scroll.min(max);
    let from = app.docker.detail_scroll;
    let shown: Vec<TextLine> = lines
        .into_iter()
        .skip(from)
        .take(inner.height as usize)
        .collect();
    frame.render_widget(Paragraph::new(shown), inner);
    if max > 0 {
        let mut state = ScrollbarState::new(max).position(from);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(g.scroll_track()))
            .thumb_symbol(g.scroll_thumb())
            .style(Style::default().fg(ACCENT));
        frame.render_stateful_widget(bar, inner, &mut state);
    }
}

/// Break `text` into lines of at most `width` columns: at spaces where it
/// can, in the middle of a word only when one word is wider than a line.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in text.split(' ') {
        let w = text_width(word);
        if w > width {
            // Flush, then cut the long word into pieces.
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for ch in word.chars() {
                let cw = ch.width().unwrap_or(0);
                if cur_w + cw > width {
                    out.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push(ch);
                cur_w += cw;
            }
            continue;
        }
        let needed = if cur.is_empty() { w } else { cur_w + 1 + w };
        if needed > width {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if !cur.is_empty() {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(word);
        cur_w += w;
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// A details line as one or more lines of at most `width` columns. A line
/// made of several spans is wrapped as a whole in its last span's style.
fn wrap_line(line: TextLine<'static>, width: usize) -> Vec<TextLine<'static>> {
    let total: usize = line.spans.iter().map(|s| text_width(&s.content)).sum();
    if total <= width {
        return vec![line];
    }
    let style = line.spans.last().map(|s| s.style).unwrap_or_default();
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    wrap_text(&text, width)
        .into_iter()
        .map(|l| TextLine::from(Span::styled(l, style)))
        .collect()
}

/// The details pane's lines, each wrapped to `width`.
fn detail_lines(app: &App, width: usize) -> Vec<TextLine<'static>> {
    detail_lines_raw(app, width)
        .into_iter()
        .flat_map(|l| wrap_line(l, width))
        .collect()
}

fn detail_lines_raw(app: &App, width: usize) -> Vec<TextLine<'static>> {
    let g = &app.glyphs;
    let dim = Style::default().fg(DIM);
    let Some(line) = app.docker.selected() else {
        return vec![TextLine::from(Span::styled("nothing selected", dim))];
    };
    let mut out: Vec<TextLine<'static>> = Vec::new();
    let plain = |t: String| TextLine::from(clean_text(&t, g));
    let warn = |t: String| {
        TextLine::from(Span::styled(
            clean_text(&t, g),
            Style::default().fg(Color::Yellow),
        ))
    };
    let now = now_secs();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    match line.kind {
        DKind::Section(_) => {
            out.push(TextLine::from(Span::styled(
                "space marks everything here that is safe to remove in bulk",
                dim,
            )));
            return out;
        }
        DKind::Layer(n) => {
            let Some((image, _)) = layer_ref(&line.key) else {
                return out;
            };
            let layer = app
                .docker
                .layers()
                .and_then(|l| l.get(image))
                .and_then(|l| l.get(n));
            let Some(l) = layer else {
                return out;
            };
            out.push(TextLine::from(Span::styled(
                format!("layer {} of {}", n + 1, image_name(app, image)),
                bold,
            )));
            out.push(plain(clean_text(&l.command, g)));
            out.push(TextLine::raw(""));
            out.push(plain(format!(
                "size {} {} created {}",
                human_size(l.size),
                g.dot(),
                crate::fmt::date(l.created)
            )));
            let with = layer_shared_with(app, image, &l.diff_id);
            if l.size == 0 {
                out.push(TextLine::from(Span::styled(
                    "an empty step: it adds no data",
                    dim,
                )));
            } else if with.is_empty() {
                out.push(plain(
                    "only in this image: removing the image frees it".to_string(),
                ));
            } else {
                out.push(warn(format!(
                    "also in: {} (stays while they stay)",
                    with.join(", ")
                )));
            }
            return out;
        }
        _ => {}
    }
    let Some(f) = app.docker.facts(&line.key) else {
        return vec![TextLine::from(Span::styled(fit(&line.key, width, g), dim))];
    };
    let safety = safety_of(f);
    out.push(TextLine::from(vec![
        Span::styled(
            format!("{} ", safety_dot(g, safety)),
            Style::default().fg(safety_color(safety)),
        ),
        Span::styled(fit(&f.name, width.saturating_sub(3), g), bold),
    ]));
    if f.kind != ItemKind::CacheRecord {
        out.push(TextLine::from(Span::styled(
            fit_start(&line.key, width, g),
            dim,
        )));
    }
    let data = app.docker.data();
    let holder_names = |f: &ItemFacts| -> Vec<String> {
        f.holders
            .iter()
            .filter_map(|id| {
                let c = data?
                    .containers
                    .iter()
                    .find(|c| format!("container:{}", c.id) == *id)?;
                Some(format!("{} ({})", c.name, state_word(c.state)))
            })
            .collect()
    };
    let last_used = |out: &mut Vec<TextLine<'static>>| {
        let when = ago(now, f.last_used);
        if !f.last_used_is_proxy {
            out.push(plain(format!("last used: {when}")));
        } else if f.holders.is_empty() {
            out.push(plain(format!(
                "last used: not recorded; never used by a container (created {})",
                ago(now, f.created)
            )));
        } else {
            out.push(plain(format!(
                "last used: {when} (worked out from its containers)"
            )));
        }
    };
    match f.kind {
        ItemKind::Image => {
            let tags = app
                .docker
                .data()
                .and_then(|d| {
                    d.images
                        .iter()
                        .find(|i| format!("image:{}", i.id) == line.key)
                })
                .map(|i| i.tags.clone())
                .unwrap_or_default();
            out.push(plain(if tags.is_empty() {
                "no tags (dangling)".to_string()
            } else {
                format!("tags: {}", tags.join(", "))
            }));
            out.push(plain(format!(
                "created {} ({})",
                crate::fmt::date(f.created),
                ago(now, f.created)
            )));
            last_used(&mut out);
            out.push(TextLine::from(Span::styled(
                "comes back: pull it again, or rebuild it",
                dim,
            )));
            out.push(TextLine::raw(""));
            out.push(TextLine::from(Span::styled(
                format!(
                    "frees now {} {} kept by other images {}",
                    human_size(f.frees_alone),
                    g.dot(),
                    human_size(f.kept_by_others)
                ),
                Style::default().fg(Color::Green),
            )));
            if f.approximate {
                out.push(warn(
                    "layer information is incomplete: shared figures are a guess".to_string(),
                ));
            }
            let image_id = line.key.strip_prefix("image:").unwrap_or(&line.key);
            if let Some(ls) = app.docker.layers().and_then(|l| l.get(image_id))
                && !ls.is_empty()
            {
                let big = ls.iter().max_by_key(|l| l.size);
                out.push(plain(format!(
                    "layers: {} {} biggest {}",
                    ls.len(),
                    g.dot(),
                    big.map_or_else(
                        || "-".to_string(),
                        |b| format!("{} ({})", human_size(b.size), tidy_command(&b.command))
                    )
                )));
            }
        }
        ItemKind::Container => {
            if let Some(c) = data.and_then(|d| {
                d.containers
                    .iter()
                    .find(|c| format!("container:{}", c.id) == line.key)
            }) {
                out.push(plain(format!(
                    "{} {} image {}",
                    state_word(c.state),
                    g.dot(),
                    image_name(app, &c.image_id)
                )));
                out.push(plain(format!(
                    "created {} {} started {} {} finished {}",
                    crate::fmt::date(c.created),
                    g.dot(),
                    crate::fmt::date(c.started),
                    g.dot(),
                    crate::fmt::date(c.finished)
                )));
                if !c.volumes.is_empty() {
                    out.push(plain(format!("volumes: {}", c.volumes.join(", "))));
                }
            }
        }
        ItemKind::Volume => {
            if let Some(v) = data.and_then(|d| {
                d.volumes
                    .iter()
                    .find(|v| format!("volume:{}", v.name) == line.key)
            }) {
                out.push(TextLine::from(Span::styled(
                    fit_start(&v.mountpoint, width, g),
                    dim,
                )));
                out.push(plain(format!(
                    "{} {} created {}",
                    human_size(v.size),
                    g.dot(),
                    crate::fmt::date(v.created)
                )));
            }
            last_used(&mut out);
            let users = holder_names(f);
            out.push(plain(if users.is_empty() {
                "used by no container".to_string()
            } else {
                format!("used by: {}", users.join(", "))
            }));
            out.push(warn(
                "data in a volume cannot be recovered once it is removed".to_string(),
            ));
        }
        ItemKind::CacheRecord => {
            out.push(plain(clean_text(&f.name, g)));
            if let Some(r) = data.and_then(|d| {
                d.cache
                    .iter()
                    .find(|r| format!("cache:{}", r.id) == line.key)
            }) {
                out.push(TextLine::raw(""));
                out.push(plain(format!(
                    "{} {} used {} times {} {}",
                    r.kind,
                    g.dot(),
                    r.usage_count,
                    g.dot(),
                    if r.shared { "shared" } else { "not shared" }
                )));
                out.push(plain(format!(
                    "created {} {} last used {}",
                    crate::fmt::date(f.created),
                    g.dot(),
                    ago(now, f.last_used)
                )));
                let children = data
                    .map(|d| {
                        d.cache
                            .iter()
                            .filter(|c| c.parents.first() == Some(&r.id))
                            .count()
                    })
                    .unwrap_or(0);
                if !r.parents.is_empty() || children > 0 {
                    out.push(plain(format!(
                        "built on {} earlier step(s); {} later step(s) build on it",
                        r.parents.len(),
                        children
                    )));
                }
            }
        }
        ItemKind::VmTrim => {}
    }
    if !matches!(f.kind, ItemKind::Container | ItemKind::CacheRecord) {
        let users = holder_names(f);
        if !users.is_empty() && f.kind == ItemKind::Image {
            out.push(TextLine::raw(""));
            out.push(TextLine::from(Span::styled("used by:", bold)));
            for u in users {
                out.push(plain(format!("  {u}")));
            }
        }
    }
    if !f.needs.is_empty() && f.removable.is_ok() {
        let names: Vec<String> = f
            .needs
            .iter()
            .map(|id| {
                app.docker
                    .facts(id)
                    .map_or_else(|| id.clone(), |c| c.name.clone())
            })
            .collect();
        out.push(warn(format!(
            "remove these containers first: {} (m marks them)",
            names.join(", ")
        )));
    }
    match &f.removable {
        Err(Refusal::Running(names)) => out.push(TextLine::from(Span::styled(
            format!(
                "cannot be removed: used by running container(s) {}",
                names.join(", ")
            ),
            Style::default().fg(Color::Red),
        ))),
        Err(Refusal::CacheInUse) => out.push(TextLine::from(Span::styled(
            "cannot be removed: a build is using it right now",
            Style::default().fg(Color::Red),
        ))),
        Ok(()) => {}
    }
    if !app.docker_marked().is_empty() {
        out.push(TextLine::raw(""));
        out.push(TextLine::from(Span::styled(
            format!(
                "with everything you marked: frees {} in Docker",
                app.docker_frees()
                    .map_or_else(|| g.ellipsis().to_string(), human_size)
            ),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
    }
    out
}

fn state_word(s: ContainerState) -> &'static str {
    match s {
        ContainerState::Running => "running",
        ContainerState::Stopped => "stopped",
        ContainerState::Other => "paused",
    }
}

fn image_name(app: &App, image_id: &str) -> String {
    app.docker
        .facts(&format!("image:{image_id}"))
        .map_or_else(|| image_id.to_string(), |f| f.name.clone())
}

/// Text from Docker made safe to draw (control characters, ASCII mode).
fn clean_text(text: &str, g: &Glyphs) -> String {
    super::clean(text, g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_command_drops_docker_noise() {
        assert_eq!(
            tidy_command("mount / from exec /bin/sh -c apt-get update"),
            "apt-get update"
        );
        assert_eq!(
            tidy_command("RUN /bin/sh -c pnpm   install # buildkit"),
            "RUN pnpm install"
        );
        assert_eq!(
            tidy_command("/bin/sh -c #(nop)  CMD [\"node\"]"),
            "CMD [\"node\"]"
        );
        assert_eq!(tidy_command("[4/8] WORKDIR /app"), "[4/8] WORKDIR /app");
        assert_eq!(tidy_command(""), "");
    }
}
