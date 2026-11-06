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
use crate::usage::Usage;

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
        Some(Dialog::Review(r)) => review(frame, area, g, r, scale),
        Some(Dialog::Irreversible(r)) => irreversible(frame, area, g, r, scale),
        Some(Dialog::ConfirmForever(t)) => confirm_forever(frame, area, g, t, scale),
        Some(Dialog::Deleting(p)) => deleting(frame, area, app, p, scale),
        Some(Dialog::Finish(f)) => finish(frame, area, app, f, scale),
        Some(Dialog::Notice(msg)) => notice(frame, area, g, msg, scale),
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

fn review(frame: &mut Frame, area: Rect, g: &Glyphs, r: &Review, scale: f64) {
    let width = area.width.saturating_sub(4).min(96);
    let wanted = r.items.len() as u16 + 11;
    let height = wanted.min(area.height.saturating_sub(2)).max(11);
    let Some(inner) = popup(
        frame,
        area,
        g,
        "Review before moving to ToBeDeleted",
        ACCENT,
        width,
        height,
        scale,
    ) else {
        return;
    };
    let room = (inner.height as usize).saturating_sub(8);
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
    let shown = r.items.len().min(room.max(1));
    for item in r.items.iter().take(shown) {
        lines.push(Line::from(vec![
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
        ]));
    }
    if r.items.len() > shown {
        lines.push(Line::from(Span::styled(
            format!("{} and {} more", g.ellipsis(), r.items.len() - shown),
            Style::default().fg(DIM),
        )));
    }
    lines.push(Line::raw(""));
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
    lines.push(Line::from(Span::styled(
        if r.shared == Usage::default() {
            "Deleting for good frees all of it.".to_string()
        } else {
            format!(
                "Deleting for good frees about {}; {} is shared with other places and stays.",
                both_usage(r.unique),
                both_usage(r.shared)
            )
        },
        Style::default().fg(if r.shared == Usage::default() {
            SAFE
        } else {
            CAUTION
        }),
    )));
    lines.push(key_line(
        g,
        &[("Enter", "Move to ToBeDeleted"), ("Esc", "Cancel")],
    ));
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
    let Some(inner) = popup(
        frame,
        area,
        g,
        "Working",
        ACCENT,
        area.width.saturating_sub(4).min(76),
        9,
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
    };
    let bar_w = (inner.width as usize).saturating_sub(7);
    let lines = vec![
        Line::from(vec![
            Span::styled(g.spinner()[idx].to_string(), Style::default().fg(ACCENT)),
            Span::raw(format!(" {what}, please wait")),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled(eighths_bar(frac, bar_w, g), Style::default().fg(SAFE)),
            Span::raw(format!(" {:>3}%", (frac * 100.0).floor() as u32)),
        ]),
        Line::from(format!("{} / {} items", p.done, p.total)),
        Line::from(format!(
            "{} of {}",
            human_size(p.usage.size),
            both_usage(p.of)
        )),
        Line::from(Span::styled(
            fit_start(&p.current.to_string_lossy(), inner.width as usize, g),
            Style::default().fg(DIM),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
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
    lines.push(key_line(g, &[("Enter", "Done")]));
    let height = lines.len() as u16 + 2;
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
