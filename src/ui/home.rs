use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{ACCENT, CAUTION, DANGER, DIM, SAFE, fit_pad, panel, right};
use crate::anim;
use crate::app::App;
use crate::fmt::{eighths_bar, human_size};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let g = &app.glyphs;
    let block = panel(g, format!(" {} Home ", g.nav(0)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 10 || inner.height < 6 {
        return;
    }

    let banner = app.fda_missing || !app.scanner.denied().is_empty();
    let banner_h = if banner { 4.min(inner.height / 2) } else { 0 };
    let [content, banner_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(banner_h)]).areas(inner);

    let total = app.total_space.max(1);
    let free = app.free_shown.value().max(0.0) as u64;
    let frac = (free as f64 / total as f64).clamp(0.0, 1.0);
    let bar_color = if frac > 0.2 {
        SAFE
    } else if frac > 0.1 {
        CAUTION
    } else {
        DANGER
    };
    let bar_width = (content.width as usize).saturating_sub(2);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "Disk",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("Free "),
            Span::styled(
                human_size(free),
                Style::default().fg(bar_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" / {}", human_size(app.total_space)),
                Style::default().fg(DIM),
            ),
        ]),
        Line::from(Span::styled(
            eighths_bar(frac, bar_width, g),
            Style::default().fg(bar_color),
        )),
        Line::from(format!(
            "Used {}   {} pending {}",
            human_size(app.total_space.saturating_sub(free)),
            g.pending(),
            human_size(app.accounting.pending())
        )),
        Line::raw(""),
    ];

    let (files, bytes) = app.scanner.stats();
    let activity = if app.scanner.is_idle() {
        Line::from(vec![
            Span::styled(g.check().to_string(), Style::default().fg(SAFE)),
            Span::raw(format!(
                " Scan complete {} {files} files {} {}",
                g.dot(),
                g.dot(),
                human_size(bytes)
            )),
        ])
    } else {
        let frame_idx = anim::spinner_frame(app.tick, g.spinner().len());
        Line::from(vec![
            Span::styled(
                g.spinner()[frame_idx].to_string(),
                Style::default().fg(ACCENT),
            ),
            Span::raw(format!(
                " Scanning {} {files} files {} {}",
                g.dot(),
                g.dot(),
                human_size(bytes)
            )),
        ])
    };
    lines.push(activity);
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Biggest folders",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));

    let mut top = app.stack[0].children();
    top.sort_by_key(|n| std::cmp::Reverse(n.size()));
    if top.is_empty() {
        lines.push(Line::from(Span::styled(
            "(nothing scanned yet)",
            Style::default().fg(DIM),
        )));
    }
    let biggest = top.first().map_or(1, |n| n.size()).max(1);
    let room = (content.height as usize).saturating_sub(lines.len());
    for node in top.iter().take(5.min(room)) {
        let name_w = 22.min((content.width as usize).saturating_sub(30)).max(6);
        let name = node.name.to_string_lossy();
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", g.folder()), Style::default().fg(ACCENT)),
            Span::raw(fit_pad(&name, name_w, g)),
            Span::styled(
                format!(
                    " {} ",
                    eighths_bar(node.size() as f64 / biggest as f64, 10, g)
                ),
                Style::default().fg(CAUTION),
            ),
            Span::raw(right(&human_size(node.size()), 9)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), content);

    if banner && banner_h > 0 {
        let denied = app.scanner.denied().len();
        let detail = if denied > 0 {
            format!("{denied} locations could not be read. ")
        } else {
            String::new()
        };
        let text = format!(
            "{} Full Disk Access is off. {detail}Grant it to your terminal app in System Settings > Privacy & Security > Full Disk Access, then restart MacCleanup.",
            g.warn()
        );
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(CAUTION)),
            banner_area,
        );
    }
}
