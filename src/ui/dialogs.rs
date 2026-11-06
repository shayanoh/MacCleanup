use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::{
    ACCENT, CAUTION, DANGER, DIM, SAFE, centered, fit, fit_start, pad, panel, right, safety_color,
    safety_dot, safety_word,
};
use crate::anim;
use crate::app::{App, DeleteProgress, Dialog, FinishInfo, ForeverTarget, Review};
use crate::fmt::{both_usage, eighths_bar, human_size};
use crate::glyphs::Glyphs;
use crate::usage::Usage;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let g = &app.glyphs;
    match &app.dialog {
        None => {}
        Some(Dialog::Review(r)) => review(frame, area, g, r),
        Some(Dialog::Irreversible(r)) => irreversible(frame, area, g, r),
        Some(Dialog::ConfirmForever(t)) => confirm_forever(frame, area, g, t),
        Some(Dialog::Deleting(p)) => deleting(frame, area, app, p),
        Some(Dialog::Finish(f)) => finish(frame, area, app, f),
        Some(Dialog::Notice(msg)) => notice(frame, area, g, msg),
    }
}

/// Clear the popup area and draw a titled border; returns the inner area.
fn popup(
    frame: &mut Frame,
    area: Rect,
    g: &Glyphs,
    title: &str,
    color: Color,
    w: u16,
    h: u16,
) -> Rect {
    let rect = centered(area, w, h);
    frame.render_widget(Clear, rect);
    let block = panel(g, format!(" {title} ")).border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
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

fn review(frame: &mut Frame, area: Rect, g: &Glyphs, r: &Review) {
    let width = area.width.saturating_sub(4).min(96);
    let wanted = r.items.len() as u16 + 11;
    let height = wanted.min(area.height.saturating_sub(2)).max(11);
    let inner = popup(
        frame,
        area,
        g,
        "Review before moving to ToBeDeleted",
        ACCENT,
        width,
        height,
    );
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

fn irreversible(frame: &mut Frame, area: Rect, g: &Glyphs, r: &Review) {
    let inner = popup(
        frame,
        area,
        g,
        &format!("{} Delete permanently", g.warn()),
        DANGER,
        area.width.saturating_sub(4).min(76),
        9,
    );
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

fn confirm_forever(frame: &mut Frame, area: Rect, g: &Glyphs, t: &ForeverTarget) {
    let inner = popup(
        frame,
        area,
        g,
        &format!("{} Delete for good", g.warn()),
        DANGER,
        area.width.saturating_sub(4).min(76),
        9,
    );
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

fn deleting(frame: &mut Frame, area: Rect, app: &App, p: &DeleteProgress) {
    let g = &app.glyphs;
    let inner = popup(
        frame,
        area,
        g,
        "Working",
        ACCENT,
        area.width.saturating_sub(4).min(76),
        8,
    );
    let idx = anim::spinner_frame(app.tick, g.spinner().len());
    let frac = if p.total == 0 {
        1.0
    } else {
        p.done as f64 / p.total as f64
    };
    let bar_w = (inner.width as usize).saturating_sub(2);
    let lines = vec![
        Line::from(vec![
            Span::styled(g.spinner()[idx].to_string(), Style::default().fg(ACCENT)),
            Span::raw(" Moving files, please wait"),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            eighths_bar(frac, bar_w, g),
            Style::default().fg(SAFE),
        )),
        Line::from(format!(
            "{} / {}   {}",
            p.done,
            p.total,
            both_usage(p.usage)
        )),
        Line::from(Span::styled(
            fit_start(&p.current.to_string_lossy(), inner.width as usize, g),
            Style::default().fg(DIM),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn finish(frame: &mut Frame, area: Rect, app: &App, f: &FinishInfo) {
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
    let inner = popup(
        frame,
        area,
        g,
        "Done",
        SAFE,
        area.width.saturating_sub(4).min(86),
        height,
    );
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn notice(frame: &mut Frame, area: Rect, g: &Glyphs, msg: &str) {
    let inner = popup(
        frame,
        area,
        g,
        "Note",
        ACCENT,
        area.width.saturating_sub(4).min(76),
        8,
    );
    let lines = vec![
        Line::from(super::clean(msg, g)),
        Line::raw(""),
        key_line(g, &[("Enter", "OK")]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}
