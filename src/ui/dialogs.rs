use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::confetti;
use super::{
    ACCENT, CAUTION, DANGER, DIM, SAFE, centered, fit, fit_start, pad, panel, right, safety_color,
    safety_dot, safety_word,
};
use crate::anim;
use crate::app::{App, DeleteProgress, Dialog, FinishInfo, ForeverTarget, QuitWarning, Review};
use crate::fmt::{both_usage, eighths_bar, human_size};
use crate::glyphs::Glyphs;
use crate::jobs::Phase;

/// Ticks a window takes to grow to full size (about a third of a second).
pub const GROW_TICKS: u64 = 7;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let g = &app.glyphs;
    let scale = if app.animations {
        anim::ease_out_cubic(app.tick.saturating_sub(app.dialog_since) as f64 / GROW_TICKS as f64)
    } else {
        1.0
    };
    match &app.dialog {
        None => {}
        Some(Dialog::QuitWarning(w)) => quit_warning(frame, area, g, w, scale),
        Some(Dialog::Review(r)) => review(
            frame,
            area,
            g,
            r,
            app.review_scroll,
            app.review_seen_end,
            scale,
        ),
        Some(Dialog::Irreversible(r)) => irreversible(frame, area, g, r, scale),
        Some(Dialog::ConfirmForever(t)) => confirm_forever(frame, area, g, t, scale),
        Some(Dialog::Deleting(p)) => deleting(frame, area, app, p, scale),
        Some(Dialog::Finish(f)) => finish(frame, area, app, f, scale),
        Some(Dialog::Notice(msg)) => notice(frame, area, g, msg, scale),
        Some(Dialog::Manual(m)) => manual(frame, area, g, m, scale),
    }
}

/// The window rectangle `scale` (0..=1) of the way through its grow-in
/// animation: a small frame in the middle that grows to the final size.
pub fn grown(area: Rect, w: u16, h: u16, scale: f64) -> Rect {
    let full = centered(area, w, h);
    let p = if scale.is_nan() {
        1.0
    } else {
        scale.clamp(0.0, 1.0)
    };
    if p >= 1.0 || full.width == 0 || full.height == 0 {
        return full;
    }
    let (min_w, min_h) = (3.min(full.width), 3.min(full.height));
    let width = (min_w as f64 + (full.width - min_w) as f64 * p).round() as u16;
    let height = (min_h as f64 + (full.height - min_h) as f64 * p).round() as u16;
    Rect::new(
        full.x + (full.width - width) / 2,
        full.y + (full.height - height) / 2,
        width,
        height,
    )
}

/// Clear the window area and draw its border. Returns the inner area once
/// the window is fully grown; while it is still growing only the empty
/// frame is drawn and `None` is returned, so no content pops in early.
#[allow(clippy::too_many_arguments)]
fn popup(
    frame: &mut Frame,
    area: Rect,
    g: &Glyphs,
    title: &str,
    color: Color,
    w: u16,
    h: u16,
    scale: f64,
) -> Option<Rect> {
    let growing = scale < 1.0;
    let rect = grown(area, w, h, scale);
    frame.render_widget(Clear, rect);
    let title = if growing {
        String::new()
    } else {
        format!(" {title} ")
    };
    let block = panel(g, title).border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    (!growing).then_some(inner)
}

/// What to tell the user when a command is stuck on (or failed on) a lock
/// held by another process. Empty for any other output.
fn lock_hint(text: &str) -> Option<&'static str> {
    crate::catalog::is_lock_wait(text).then_some(
        "Another uv process is using the cache (for example a `uv run` server). \
         uv waits up to 5 minutes for it, then gives up.",
    )
}

/// How many rows these lines take once wrapped to `width` columns.
fn wrapped_rows(lines: &[Line], width: usize) -> usize {
    let width = width.max(1);
    lines
        .iter()
        .map(|l| {
            let w: usize = l.spans.iter().map(|s| super::text_width(&s.content)).sum();
            w.div_ceil(width).max(1)
        })
        .sum()
}

fn key_line<'a>(g: &Glyphs, keys: &[(&str, &str)]) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(format!(" {} ", g.dot())));
        }
        spans.push(Span::styled(
            format!("[ {key} ]"),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {what}")));
    }
    Line::from(spans)
}

fn quit_warning(frame: &mut Frame, area: Rect, g: &Glyphs, w: &QuitWarning, scale: f64) {
    let Some(inner) = popup(
        frame,
        area,
        g,
        &format!("{} Before you quit", g.warn()),
        CAUTION,
        area.width.saturating_sub(4).min(76),
        9,
        scale,
    ) else {
        return;
    };
    let items = if w.count == 1 {
        "1 item".to_string()
    } else {
        format!("{} items", w.count)
    };
    let lines = vec![
        Line::from(Span::styled(
            format!("ToBeDeleted still holds {items}: {}.", both_usage(w.usage)),
            Style::default().fg(CAUTION).add_modifier(Modifier::BOLD),
        )),
        Line::from("Their space is not freed until you empty it."),
        Line::raw(""),
        key_line(g, &[("e", "Open ToBeDeleted"), ("q", "Quit anyway")]),
        key_line(g, &[("Esc", "Stay")]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Lines of the review that are neither folders nor commands.
const REVIEW_FIXED: usize = 12;

/// How the review fits a terminal: the folders and then the commands are one
/// list that scrolls, so nothing is ever cut; the lines around it stay put.
#[derive(Clone, Copy, Debug)]
pub struct ReviewLayout {
    pub width: u16,
    pub height: u16,
    /// How many lines of the list are visible at once.
    pub body_rows: usize,
}

pub fn review_layout(area: Rect, r: &Review) -> ReviewLayout {
    let width = area.width.saturating_sub(4).min(96);
    let max_h = area.height.saturating_sub(2) as usize;
    let height = (r.body_len() + REVIEW_FIXED + 2).min(max_h).max(13);
    let inner = height.saturating_sub(2);
    ReviewLayout {
        width,
        height: height as u16,
        body_rows: inner.saturating_sub(REVIEW_FIXED).max(1),
    }
}

fn review(
    frame: &mut Frame,
    area: Rect,
    g: &Glyphs,
    r: &Review,
    scroll: usize,
    seen_end: bool,
    scale: f64,
) {
    let lay = review_layout(area, r);
    let (width, height) = (lay.width, lay.height);
    let Some(inner) = popup(
        frame,
        area,
        g,
        if r.items.is_empty() {
            "Review before running cleanup commands"
        } else {
            "Review before moving to ToBeDeleted"
        },
        ACCENT,
        width,
        height,
        scale,
    ) else {
        return;
    };
    let name_w = (inner.width as usize).saturating_sub(34);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "  {} {} {}",
            pad("Item".to_string(), name_w),
            right("Size", 9),
            right("On disk", 9)
        ),
        Style::default().fg(DIM),
    )));
    // The list: every folder, then the commands. One list, so it scrolls as one.
    let mut body: Vec<Line> = Vec::new();
    for item in &r.items {
        body.push(Line::from(vec![
            Span::styled(
                format!("{} ", safety_dot(g, item.safety)),
                Style::default().fg(safety_color(item.safety)),
            ),
            Span::raw(format!(
                "{} ",
                pad(fit_start(&item.path.to_string_lossy(), name_w, g), name_w)
            )),
            Span::raw(right(&human_size(item.usage.size), 9)),
            Span::raw(format!(" {}", right(&human_size(item.usage.disk), 9))),
            Span::styled(
                format!(" {}", safety_word(item.safety)),
                Style::default().fg(safety_color(item.safety)),
            ),
            Span::styled(
                if item.needs_admin {
                    " may ask for your password"
                } else {
                    ""
                },
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }
    if !r.commands.is_empty() {
        body.push(Line::raw(""));
        body.push(Line::from(Span::styled(
            format!(
                "{} Tool cleanup commands (run after the moves, cannot be undone)",
                g.warn()
            ),
            Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
        )));
        for c in &r.commands {
            // The whole command: what it will do must be readable before it
            // is confirmed. Which tool, how much and where from go below.
            body.push(Line::from(vec![
                Span::styled(format!("  {} ", g.dot()), Style::default().fg(DANGER)),
                Span::styled(
                    fit(&c.line, (inner.width as usize).saturating_sub(4), g),
                    // A forced command is drawn in red: it skips a safety check.
                    if c.force {
                        Style::default().fg(DANGER).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().add_modifier(Modifier::BOLD)
                    },
                ),
            ]));
            body.push(Line::from(Span::styled(
                format!(
                    "    {}  up to {}  via {}",
                    c.tool,
                    human_size(c.up_to),
                    fit_start(
                        &c.spec.program,
                        (inner.width as usize).saturating_sub(40),
                        g
                    )
                ),
                Style::default().fg(DIM),
            )));
            if let Some(w) = &c.warn {
                body.push(Line::from(Span::styled(
                    fit(
                        &format!("    {} {w}", g.warn()),
                        (inner.width as usize).saturating_sub(2),
                        g,
                    ),
                    Style::default().fg(CAUTION),
                )));
            }
            if c.can_force {
                body.push(Line::from(if c.force {
                    Span::styled(
                        "    FORCED: skips the in-use check; a process using it now may be disturbed",
                        Style::default().fg(DANGER),
                    )
                } else {
                    Span::styled(
                        "    waits up to 5 minutes if another process uses it; f forces it",
                        Style::default().fg(DIM),
                    )
                }));
            }
        }
    }
    debug_assert_eq!(
        body.len(),
        r.body_len(),
        "Review::body_len and the lines agree"
    );
    // The list is a window onto the lines; it never starts past the last full page.
    let rows = lay.body_rows;
    let start = scroll.min(body.len().saturating_sub(rows));
    let scrollable = body.len() > rows;
    lines.extend(body.into_iter().skip(start).take(rows));
    if scrollable {
        let end = (start + rows).min(r.body_len());
        let hint = if r.commands.is_empty() {
            format!(
                "{}-{} of {}   j/k, arrows, PgUp/PgDn, Ctrl-F/B or the wheel scroll",
                start + 1,
                end,
                r.body_len()
            )
        } else if seen_end {
            format!(
                "lines {}-{} of {}   j/k, PgUp/PgDn, Home/End or the wheel scroll",
                start + 1,
                end,
                r.body_len()
            )
        } else {
            format!(
                "lines {}-{} of {}   the commands are at the end: scroll down (j, PgDn, End)",
                start + 1,
                end,
                r.body_len()
            )
        };
        lines.push(Line::from(Span::styled(
            fit(&hint, inner.width as usize, g),
            if r.commands.is_empty() || seen_end {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(CAUTION)
            },
        )));
    }
    lines.push(Line::raw(""));
    if r.items.is_empty() {
        lines.push(Line::from("Nothing is moved; only the commands above run."));
    } else {
        lines.push(Line::from(vec![
            Span::styled(
                format!("Total {}", both_usage(r.total)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" will be moved."),
        ]));
        lines.push(Line::from(
            "It is not freed until you empty ToBeDeleted.".to_string(),
        ));
    }
    let sp = r.space;
    if !r.space_ready {
        lines.push(Line::from(Span::styled(
            format!("Working out what this frees{}", g.ellipsis()),
            Style::default().fg(DIM),
        )));
    } else if sp.frees_now >= sp.footprint {
        lines.push(Line::from(Span::styled(
            "Deleting for good frees all of it right now.",
            Style::default().fg(SAFE),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                "Deleting for good frees about {} right now.",
                human_size(sp.frees_all())
            ),
            Style::default().fg(SAFE),
        )));
        if sp.snapshots > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "{} more is freed once Time Machine snapshots expire.",
                    human_size(sp.snapshots)
                ),
                Style::default().fg(CAUTION),
            )));
        }
        if sp.shared_inside > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "{} is shared only among these files and goes with them (unless a snapshot holds it).",
                    human_size(sp.shared_inside)
                ),
                Style::default().fg(CAUTION),
            )));
        }
        if sp.shared_outside > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "{} is shared with other places and stays.",
                    human_size(sp.shared_outside)
                ),
                Style::default().fg(CAUTION),
            )));
        }
    }
    if let Some(problem) = r.problems.first() {
        // Docker would refuse part of it: nothing can run until it is fixed.
        lines.push(Line::from(Span::styled(
            format!("{} {}", g.warn(), problem),
            Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
        )));
        lines.push(key_line(
            g,
            &[("Esc", "Cancel (mark them, then review again)")],
        ));
    } else if !r.needs_y() {
        lines.push(key_line(
            g,
            &[("Enter", "Move to ToBeDeleted"), ("Esc", "Cancel")],
        ));
    } else if r.commands.is_empty() {
        lines.push(key_line(
            g,
            &[
                ("y", "Move to ToBeDeleted (may ask for your password)"),
                ("Esc", "Cancel"),
            ],
        ));
    } else if scrollable && !seen_end {
        // y stays off until every command has been scrolled into view.
        lines.push(key_line(
            g,
            &[("End", "Scroll to the commands first"), ("Esc", "Cancel")],
        ));
    } else {
        // Commands cannot be undone, so Enter does nothing here.
        let run = if r.items.is_empty() {
            "Run the commands"
        } else {
            "Move, then run the commands"
        };
        if r.commands.iter().any(|c| c.can_force) {
            lines.push(key_line(
                g,
                &[
                    ("y", run),
                    ("f", "Force (skip the in-use check)"),
                    ("Esc", "Cancel"),
                ],
            ));
        } else {
            lines.push(key_line(g, &[("y", run), ("Esc", "Cancel")]));
        }
    }
    if !r.items.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                "[ x ]",
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" Move, then empty now  {} cannot be undone", g.warn()),
                Style::default().fg(DANGER),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn irreversible(frame: &mut Frame, area: Rect, g: &Glyphs, r: &Review, scale: f64) {
    let Some(inner) = popup(
        frame,
        area,
        g,
        &format!("{} Delete permanently", g.warn()),
        DANGER,
        area.width.saturating_sub(4).min(76),
        9,
        scale,
    ) else {
        return;
    };
    let lines = vec![
        Line::from(Span::styled(
            format!(
                "This permanently deletes {} item(s), {}.",
                r.items.len(),
                both_usage(r.total)
            ),
            Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
        )),
        Line::from("This cannot be undone. The files will not go to ToBeDeleted first."),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "[ y ]",
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Yes, delete permanently"),
        ]),
        key_line(g, &[("Esc", "Cancel")]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn confirm_forever(frame: &mut Frame, area: Rect, g: &Glyphs, t: &ForeverTarget, scale: f64) {
    let Some(inner) = popup(
        frame,
        area,
        g,
        &format!("{} Delete for good", g.warn()),
        DANGER,
        area.width.saturating_sub(4).min(76),
        9,
        scale,
    ) else {
        return;
    };
    let what = match t {
        ForeverTarget::Item { name, usage, .. } => {
            format!("{} ({})", fit(name, 40, g), both_usage(*usage))
        }
        ForeverTarget::All { count, usage } => {
            format!("all {count} items ({}) in ToBeDeleted", both_usage(*usage))
        }
    };
    let lines = vec![
        Line::from(Span::styled(
            format!("Permanently delete {what}?"),
            Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
        )),
        Line::from("This cannot be undone."),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                "[ y ]",
                Style::default().fg(DANGER).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Yes, delete permanently"),
        ]),
        key_line(g, &[("Esc", "Cancel")]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn deleting(frame: &mut Frame, area: Rect, app: &App, p: &DeleteProgress, scale: f64) {
    let g = &app.glyphs;
    let command = p.phase == Phase::Command;
    let Some(inner) = popup(
        frame,
        area,
        g,
        "Working",
        ACCENT,
        area.width.saturating_sub(4).min(86),
        if command { 15 } else { 9 },
        scale,
    ) else {
        return;
    };
    let idx = anim::spinner_frame(app.tick, g.spinner().len());
    // Fraction by bytes, so one huge folder still moves the bar; by item
    // count when no sizes are known.
    let frac = if p.of.size > 0 {
        (p.usage.size as f64 / p.of.size as f64).clamp(0.0, 1.0)
    } else if p.total > 0 {
        (p.done as f64 / p.total as f64).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let what = match p.phase {
        Phase::Moving => "Moving files to ToBeDeleted",
        Phase::Deleting => "Deleting for good",
        Phase::Command => "Running cleanup commands",
    };
    let bar_w = (inner.width as usize).saturating_sub(7);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(g.spinner()[idx].to_string(), Style::default().fg(ACCENT)),
            Span::raw(format!(" {what}, please wait")),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled(eighths_bar(frac, bar_w, g), Style::default().fg(SAFE)),
            Span::raw(format!(" {:>3}%", (frac * 100.0).floor() as u32)),
        ]),
    ];
    if command {
        let status = app.command_status();
        let elapsed = status
            .as_ref()
            .and_then(|s| s.elapsed)
            .map_or_else(|| "0s".to_string(), crate::fmt::duration);
        lines.push(Line::from(format!(
            "command {} of {}",
            (p.done + 1).min(p.total.max(1)),
            p.total.max(1)
        )));
        lines.push(Line::from(vec![
            Span::styled(
                fit(&p.current.to_string_lossy(), 40, g),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  running for {elapsed}")),
        ]));
        let last = status.as_ref().map(|s| s.last_line.as_str()).unwrap_or("");
        lines.push(Line::from(Span::styled(
            fit(
                &if last.is_empty() {
                    "(no output yet)".to_string()
                } else {
                    format!("{} {last}", g.dot())
                },
                inner.width as usize,
                g,
            ),
            Style::default().fg(DIM),
        )));
        lines.push(Line::raw(""));
        if let Some(hint) = lock_hint(last) {
            lines.push(Line::from(Span::styled(hint, Style::default().fg(CAUTION))));
            lines.push(Line::from(Span::styled(
                "Close it, or cancel here and retry with --force afterwards.",
                Style::default().fg(CAUTION),
            )));
        }
        if status.as_ref().is_some_and(|s| s.cancelling) {
            lines.push(Line::from(Span::styled(
                "Cancelling the command...",
                Style::default().fg(CAUTION).add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::raw(""));
            lines.push(key_line(
                g,
                &[("Esc", "Cancel this command (moves already made stay)")],
            ));
        }
    } else {
        lines.push(Line::from(format!("{} / {} items", p.done, p.total)));
        lines.push(Line::from(format!(
            "{} of {}",
            human_size(p.usage.size),
            both_usage(p.of)
        )));
        lines.push(Line::from(Span::styled(
            fit_start(&p.current.to_string_lossy(), inner.width as usize, g),
            Style::default().fg(DIM),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn finish(frame: &mut Frame, area: Rect, app: &App, f: &FinishInfo, scale: f64) {
    let g = &app.glyphs;
    let freed = app.accounting.freed(app.free_now);
    let shown = if app.animations {
        anim::count_up(0, freed, app.tick.saturating_sub(f.started_tick), 30)
    } else {
        freed
    };
    let events = app.accounting.events();
    let recent = events.len().saturating_sub(3);
    let mut lines: Vec<Line> = Vec::new();
    if freed > 0 {
        lines.push(Line::from(Span::styled(
            format!("{} Congratulations! {}", g.star(), g.star()),
            Style::default().fg(SAFE).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::raw("You have freed "),
            Span::styled(
                human_size(shown),
                Style::default().fg(SAFE).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" this session."),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "Nothing freed yet.",
            Style::default().fg(CAUTION).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(
            "macOS may release the space a moment later. Time Machine snapshots can also hold on to it.",
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(format!(
        "This cleanup: moved {}",
        both_usage(f.moved)
    )));
    lines.push(Line::from(format!(
        "              deleted for good {}",
        both_usage(f.emptied)
    )));
    if f.failures > 0 {
        lines.push(Line::from(Span::styled(
            match &f.first_failure {
                Some(reason) => format!(
                    "{} item(s) could not be processed ({})",
                    f.failures,
                    fit(reason, 50, g)
                ),
                None => format!("{} item(s) could not be processed", f.failures),
            },
            Style::default().fg(DANGER),
        )));
    }
    // A long list is summarised: every failure is shown (up to the cap),
    // then the first of the rest, and a line says how many are hidden.
    const SHOWN: usize = 8;
    let mut shown: Vec<usize> = f
        .commands
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.ok)
        .map(|(i, _)| i)
        .take(SHOWN)
        .collect();
    for i in 0..f.commands.len() {
        if shown.len() >= SHOWN {
            break;
        }
        if !shown.contains(&i) {
            shown.push(i);
        }
    }
    shown.sort_unstable();
    let hidden = f.commands.len() - shown.len();
    let hidden_failed = f
        .commands
        .iter()
        .enumerate()
        .filter(|(i, c)| !c.ok && !shown.contains(i))
        .count();
    for c in shown.iter().map(|&i| &f.commands[i]) {
        let (mark, color) = if c.ok {
            (g.check(), SAFE)
        } else if c.cancelled {
            (g.warn(), CAUTION)
        } else {
            (g.warn(), DANGER)
        };
        let note = if c.ok {
            String::new()
        } else if c.cancelled {
            " (cancelled)".to_string()
        } else if c.note.is_empty() {
            String::new()
        } else {
            format!(" ({})", fit(&c.note, 60, g))
        };
        lines.push(Line::from(Span::styled(
            format!("{mark} {}{note}", c.line),
            Style::default().fg(color),
        )));
        if c.lock_wait
            && let Some(hint) = lock_hint(&c.note)
        {
            lines.push(Line::from(Span::styled(
                format!("    {hint}"),
                Style::default().fg(CAUTION),
            )));
        }
    }
    if hidden > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} and {hidden} more ({})",
                g.ellipsis(),
                if hidden_failed == 0 {
                    "all ok".to_string()
                } else {
                    format!("{hidden_failed} failed")
                }
            ),
            Style::default().fg(DIM),
        )));
    }
    if let Some((before, after)) = f.vm_disk {
        let change = match after.cmp(&before) {
            std::cmp::Ordering::Less => format!("shrank by {}", human_size(before - after)),
            std::cmp::Ordering::Equal => "unchanged".to_string(),
            std::cmp::Ordering::Greater => format!("grew by {}", human_size(after - before)),
        };
        lines.push(Line::from(format!(
            "Docker VM disk file on this Mac: {} {} {} ({change})",
            human_size(before),
            g.arrow(),
            human_size(after)
        )));
    }
    lines.push(Line::from(format!(
        "Free space now {}   pending {}",
        human_size(app.free_now),
        both_usage(app.accounting.pending())
    )));
    for e in &events[recent..] {
        lines.push(Line::from(Span::styled(
            format!("{} {} {}", g.dot(), e.label, both_usage(e.usage)),
            Style::default().fg(DIM),
        )));
    }
    lines.push(Line::raw(""));
    if f.retry.is_empty() {
        lines.push(key_line(g, &[("Enter", "Done")]));
    } else {
        lines.push(key_line(
            g,
            &[
                ("f", "Retry with --force (skips uv's in-use check)"),
                ("Enter", "Done"),
            ],
        ));
    }
    // Long lines wrap inside the window, so count rows, not lines.
    let popup_w = area.width.saturating_sub(4).min(86);
    let height = wrapped_rows(&lines, popup_w.saturating_sub(2) as usize) as u16 + 2;
    let Some(inner) = popup(
        frame,
        area,
        g,
        "Done",
        SAFE,
        area.width.saturating_sub(4).min(86),
        height,
        scale,
    ) else {
        return;
    };
    // Confetti falls behind the text (text cells overwrite it).
    if freed > 0 && app.animations {
        let since = app
            .tick
            .saturating_sub(app.dialog_since)
            .saturating_sub(GROW_TICKS);
        let buf = frame.buffer_mut();
        for piece in confetti::pieces(
            since,
            inner.width,
            inner.height,
            g.mode == crate::glyphs::GlyphMode::Ascii,
        ) {
            let cell = &mut buf[(inner.x + piece.x, inner.y + piece.y)];
            cell.set_symbol(piece.glyph).set_fg(piece.color);
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Items that need administrator rights, after the password prompt did not
/// do it: the exact commands, to copy and run in a terminal.
fn manual(frame: &mut Frame, area: Rect, g: &Glyphs, m: &crate::app::Manual, scale: f64) {
    let height = (m.lines.len() as u16 + 9).min(area.height.saturating_sub(2));
    let Some(inner) = popup(
        frame,
        area,
        g,
        "Needs administrator rights",
        ACCENT,
        area.width.saturating_sub(4).min(110),
        height,
        scale,
    ) else {
        return;
    };
    let mut lines = vec![
        Line::from(super::clean(&m.reason, g)),
        Line::raw(""),
        Line::from("Run these in a terminal (nothing was moved yet):"),
    ];
    lines.extend(m.lines.iter().map(|l| {
        Line::from(Span::styled(
            super::clean(l, g),
            Style::default().fg(Color::Yellow),
        ))
    }));
    lines.push(Line::raw(""));
    lines.push(key_line(
        g,
        &[
            ("c", if m.copied { "Copied" } else { "Copy" }),
            ("r", "I ran the commands"),
            ("Esc", "Cancel"),
        ],
    ));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn notice(frame: &mut Frame, area: Rect, g: &Glyphs, msg: &str, scale: f64) {
    let Some(inner) = popup(
        frame,
        area,
        g,
        "Note",
        ACCENT,
        area.width.saturating_sub(4).min(76),
        8,
        scale,
    ) else {
        return;
    };
    let lines = vec![
        Line::from(super::clean(msg, g)),
        Line::raw(""),
        key_line(g, &[("Enter", "OK")]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 120, 30);

    #[test]
    fn full_scale_is_the_final_window() {
        assert_eq!(grown(AREA, 80, 12, 1.0), centered(AREA, 80, 12));
    }

    #[test]
    fn zero_scale_is_a_small_frame_in_the_middle() {
        let r = grown(AREA, 80, 12, 0.0);
        assert_eq!((r.width, r.height), (3, 3));
        let full = centered(AREA, 80, 12);
        let (cx, cy) = (full.x + full.width / 2, full.y + full.height / 2);
        assert!(r.x <= cx && cx <= r.x + r.width);
        assert!(r.y <= cy && cy <= r.y + r.height);
    }

    #[test]
    fn the_window_grows_monotonically_and_stays_inside_the_final_one() {
        let full = centered(AREA, 80, 12);
        let mut last = (0, 0);
        for i in 0..=20 {
            let r = grown(AREA, 80, 12, i as f64 / 20.0);
            assert!(r.width >= last.0 && r.height >= last.1, "step {i}");
            assert!(r.x >= full.x && r.y >= full.y);
            assert!(r.x + r.width <= full.x + full.width);
            assert!(r.y + r.height <= full.y + full.height);
            last = (r.width, r.height);
        }
    }

    #[test]
    fn small_windows_and_bad_scales_do_not_misbehave() {
        // Never smaller than the final window allows, even for tiny dialogs.
        let tiny = grown(AREA, 2, 2, 0.0);
        assert!(tiny.width <= 2 && tiny.height <= 2);
        for scale in [f64::NAN, -1.0, 7.0] {
            let r = grown(AREA, 80, 12, scale);
            assert!(r.width <= 80 && r.height <= 12 && r.width >= 3);
        }
        assert_eq!(grown(Rect::new(0, 0, 0, 0), 80, 12, 0.5).area(), 0);
    }
}

#[cfg(test)]
mod review_layout_tests {
    use super::*;
    use crate::app::{Review, ReviewCommand, ReviewItem};
    use crate::catalog::CommandSpec;
    use crate::safety::Safety;
    use crate::usage::Usage;

    fn review(items: usize, commands: usize) -> Review {
        Review {
            items: (0..items)
                .map(|i| ReviewItem {
                    path: format!("/x/{i}").into(),
                    usage: Usage::default(),
                    safety: Safety::Safe,
                    needs_admin: false,
                })
                .collect(),
            commands: (0..commands)
                .map(|i| ReviewCommand {
                    id: format!("c{i}"),
                    tool: "t".to_string(),
                    line: format!("tool{i} clean"),
                    spec: CommandSpec::new("tool", &["clean"]),
                    up_to: 0,
                    path: std::path::PathBuf::new(),
                    can_force: false,
                    force: false,
                    group: Vec::new(),
                    warn: None,
                })
                .collect(),
            total: Usage::default(),
            space: Default::default(),
            space_ready: true,
            problems: Vec::new(),
        }
    }

    #[test]
    fn plenty_of_room_shows_the_whole_list() {
        let r = review(3, 2);
        let l = review_layout(Rect::new(0, 0, 140, 50), &r);
        assert!(l.body_rows >= r.body_len(), "{l:?}");
        assert_eq!(r.body_len(), 3 + 2 + 2 * 2);
    }

    #[test]
    fn many_commands_on_a_short_terminal_scroll_instead_of_being_cut() {
        let r = review(2, 10);
        let l = review_layout(Rect::new(0, 0, 140, 20), &r);
        assert!(l.body_rows >= 1 && l.body_rows < r.body_len(), "{l:?}");
        assert!(usize::from(l.height) <= 18);
    }

    #[test]
    fn warnings_and_force_hints_add_lines_to_the_list() {
        let mut r = review(0, 2);
        let plain = r.body_len();
        r.commands[0].warn = Some("careful".to_string());
        r.commands[1].can_force = true;
        assert_eq!(r.body_len(), plain + 2);
    }

    #[test]
    fn folders_alone_scroll_too() {
        let l = review_layout(Rect::new(0, 0, 140, 16), &review(200, 0));
        assert!(l.body_rows >= 1);
    }
}
