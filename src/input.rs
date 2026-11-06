use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::app::{Action, App, Dialog, Screen};

/// Translate a key press into an action, depending on what is on screen.
pub fn map_key(key: KeyEvent, app: &App) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    if app.too_small {
        // The user cannot see the screen, so a stray key must not act on it.
        let quit = key.code == KeyCode::Char('q')
            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'));
        return quit.then_some(Action::Quit);
    }
    if app.filter.editing && app.dialog.is_none() {
        return filter_key(key);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            // Page keys: Ctrl-F / Ctrl-B as in vim, plus Ctrl-D / Ctrl-U.
            KeyCode::Char('f' | 'd') if pages(app) => Some(Action::PageDown),
            KeyCode::Char('b' | 'u') if pages(app) => Some(Action::PageUp),
            _ => None,
        };
    }
    if let Some(dialog) = &app.dialog {
        return dialog_key(key.code, dialog);
    }
    match key.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Esc => Some(if app.filter.active() {
            Action::FilterClear
        } else if app.screen == Screen::Explorer && app.stack.len() > 1 {
            Action::Out
        } else {
            Action::Quit
        }),
        KeyCode::Char('/')
            if matches!(
                app.screen,
                Screen::Explorer | Screen::Targets | Screen::Docker
            ) =>
        {
            Some(Action::FilterOpen)
        }
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
        KeyCode::Char('h') | KeyCode::Left => Some(Action::Out),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => Some(Action::Into),
        KeyCode::Home => Some(Action::Top),
        KeyCode::End | KeyCode::Char('G') => Some(Action::Bottom),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Char(' ') => Some(Action::ToggleMark),
        KeyCode::Char('d') => Some(Action::OpenReview),
        KeyCode::Char('J') if app.screen == Screen::Docker => Some(Action::DetailScroll(1)),
        KeyCode::Char('K') if app.screen == Screen::Docker => Some(Action::DetailScroll(-1)),
        KeyCode::Char('m') if matches!(app.screen, Screen::Targets | Screen::Docker) => {
            Some(Action::MarkRelated)
        }
        KeyCode::Char('g') => Some(Action::Top),
        KeyCode::Char('t') => Some(Action::CycleGraph),
        KeyCode::Char('s') => Some(Action::CycleSort),
        KeyCode::Char('[') => Some(Action::GrowGraph),
        KeyCode::Char(']') => Some(Action::ShrinkGraph),
        KeyCode::Tab => Some(Action::FocusToggle),
        // A digit is the page's position in the menu, always.
        KeyCode::Char(c @ '1'..='9') => Some(Action::Menu(usize::from(c as u8 - b'0'))),
        KeyCode::Char('r') if app.screen == Screen::Holding => Some(Action::RestoreSelected),
        KeyCode::Char('D') if app.screen == Screen::Holding => Some(Action::DeleteSelectedForever),
        KeyCode::Char('E') if app.screen == Screen::Holding => Some(Action::EmptyAll),
        KeyCode::Char('r') => Some(Action::Rescan),
        KeyCode::Char('R') if app.screen != Screen::Holding => Some(Action::RescanAll),
        _ => None,
    }
}

/// Keys while the filter box has the keyboard: everything printable is text.
fn filter_key(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('c') if ctrl => Some(Action::Quit),
        KeyCode::Char('u') if ctrl => Some(Action::FilterClearLine),
        KeyCode::Char('w') if ctrl => Some(Action::FilterClearWord),
        KeyCode::Char(_) if ctrl || alt => None,
        KeyCode::Char(c) => Some(Action::FilterChar(c)),
        KeyCode::Enter => Some(Action::FilterAccept),
        KeyCode::Esc => Some(Action::FilterClear),
        KeyCode::Backspace => Some(Action::FilterBackspace),
        KeyCode::Delete => Some(Action::FilterDelete),
        KeyCode::Left => Some(Action::FilterLeft),
        KeyCode::Right => Some(Action::FilterRight),
        KeyCode::Home => Some(Action::FilterHome),
        KeyCode::End => Some(Action::FilterEnd),
        KeyCode::Up => Some(Action::Up),
        KeyCode::Down => Some(Action::Down),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        _ => None,
    }
}

/// Page keys act on the list, or on the review's item list while it is open.
fn pages(app: &App) -> bool {
    matches!(app.dialog, None | Some(Dialog::Review(_)))
}

/// Keys that scroll a long review list.
fn review_scroll_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::Home => Some(Action::Top),
        KeyCode::End | KeyCode::Char('G') => Some(Action::Bottom),
        _ => None,
    }
}

fn dialog_key(code: KeyCode, dialog: &Dialog) -> Option<Action> {
    match dialog {
        Dialog::QuitWarning(_) => match code {
            KeyCode::Char('q') | KeyCode::Char('y') => Some(Action::Confirm),
            KeyCode::Char('e') => Some(Action::ReviewHeld),
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Enter => Some(Action::Cancel),
            _ => None,
        },
        // Tool commands cannot be undone: only an explicit `y` runs them.
        Dialog::Review(r) if !r.commands.is_empty() => match code {
            code if review_scroll_key(code).is_some() => review_scroll_key(code),
            KeyCode::Char('f') if r.commands.iter().any(|c| c.can_force) => {
                Some(Action::ToggleForce)
            }
            KeyCode::Char('y') => Some(Action::Confirm),
            KeyCode::Char('x') if !r.items.is_empty() => Some(Action::ConfirmIrreversible),
            KeyCode::Esc | KeyCode::Char('n') => Some(Action::Cancel),
            _ => None,
        },
        Dialog::Review(_) => match code {
            code if review_scroll_key(code).is_some() => review_scroll_key(code),
            KeyCode::Enter => Some(Action::Confirm),
            KeyCode::Char('x') => Some(Action::ConfirmIrreversible),
            KeyCode::Esc | KeyCode::Char('n') => Some(Action::Cancel),
            _ => None,
        },
        // Irreversible actions need an explicit `y`; Enter never confirms them.
        Dialog::Irreversible(_) | Dialog::ConfirmForever(_) => match code {
            KeyCode::Char('y') => Some(Action::Confirm),
            KeyCode::Esc | KeyCode::Char('n') => Some(Action::Cancel),
            _ => None,
        },
        // A running tool command can be stopped; a move or a delete cannot.
        Dialog::Deleting(p) if p.phase == crate::jobs::Phase::Command => match code {
            KeyCode::Esc => Some(Action::Cancel),
            KeyCode::Char('q') => Some(Action::Quit),
            _ => None,
        },
        Dialog::Deleting(_) => None,
        // After a lock failure, `f` offers the forced form (through the review).
        Dialog::Finish(f) if !f.retry.is_empty() && code == KeyCode::Char('f') => {
            Some(Action::RetryForced)
        }
        Dialog::Finish(_) | Dialog::Notice(_) => match code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => Some(Action::Cancel),
            _ => None,
        },
    }
}

/// Translate a mouse event: left click selects, wheel scrolls.
pub fn map_mouse(ev: MouseEvent, app: &App) -> Option<Action> {
    if app.too_small {
        return None;
    }
    if app.dialog.is_some() {
        // Only a review has something to scroll; clicks never reach the list behind it.
        if !matches!(app.dialog, Some(Dialog::Review(_))) {
            return None;
        }
        return match ev.kind {
            MouseEventKind::ScrollDown => Some(Action::Scroll(3)),
            MouseEventKind::ScrollUp => Some(Action::Scroll(-3)),
            _ => None,
        };
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => Some(Action::Click {
            x: ev.column,
            y: ev.row,
        }),
        MouseEventKind::ScrollDown => Some(Action::Scroll(3)),
        MouseEventKind::ScrollUp => Some(Action::Scroll(-3)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Screen};
    use crate::testutil::{env, select, settle};
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    const FILES: &[(&str, usize)] = &[("a/f", 3000), ("b.bin", 2000)];

    #[test]
    fn input_key_mapping_table() {
        let e = env(FILES);
        let app = &e.app;
        let table: Vec<(KeyEvent, Option<Action>)> = vec![
            (ch('q'), Some(Action::Quit)),
            (ch('j'), Some(Action::Down)),
            (key(KeyCode::Down), Some(Action::Down)),
            (ch('k'), Some(Action::Up)),
            (key(KeyCode::Up), Some(Action::Up)),
            (ch('h'), Some(Action::Out)),
            (key(KeyCode::Left), Some(Action::Out)),
            (ch('l'), Some(Action::Into)),
            (key(KeyCode::Right), Some(Action::Into)),
            (key(KeyCode::Enter), Some(Action::Into)),
            (key(KeyCode::Home), Some(Action::Top)),
            (key(KeyCode::End), Some(Action::Bottom)),
            (ch('G'), Some(Action::Bottom)),
            (ch('g'), Some(Action::Top)),
            (key(KeyCode::PageUp), Some(Action::PageUp)),
            (key(KeyCode::PageDown), Some(Action::PageDown)),
            (ch(' '), Some(Action::ToggleMark)),
            (ch('d'), Some(Action::OpenReview)),
            (ch('t'), Some(Action::CycleGraph)),
            (ch('s'), Some(Action::CycleSort)),
            (ch('['), Some(Action::GrowGraph)),
            (ch(']'), Some(Action::ShrinkGraph)),
            (key(KeyCode::Tab), Some(Action::FocusToggle)),
            (ch('1'), Some(Action::Menu(1))),
            (ch('2'), Some(Action::Menu(2))),
            (ch('3'), Some(Action::Menu(3))),
            (ch('4'), Some(Action::Menu(4))),
            (ch('5'), Some(Action::Menu(5))),
            (ch('6'), Some(Action::Menu(6))),
            (ch('7'), Some(Action::Menu(7))),
            (ch('0'), None),
            (key(KeyCode::Esc), Some(Action::Quit)),
            // Dialog-only and holding-only keys do nothing here.
            (ch('x'), None),
            (ch('m'), None), // related-folders marking is Caches & Projects only
            (ch('y'), None),
            (ch('r'), Some(Action::Rescan)),
            (ch('R'), Some(Action::RescanAll)),
            (ch('D'), None),
            (ch('E'), None),
            (ch('z'), None),
        ];
        for (k, expected) in table {
            assert_eq!(map_key(k, app), expected, "{k:?}");
        }
    }

    #[test]
    fn ctrl_c_always_quits() {
        let e = env(FILES);
        let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(k, &e.app), Some(Action::Quit));
    }

    #[test]
    fn release_events_are_ignored() {
        let e = env(FILES);
        let k = KeyEvent {
            code: KeyCode::Char('q'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert_eq!(map_key(k, &e.app), None);
    }

    #[test]
    fn esc_goes_out_when_deeper() {
        let mut e = env(FILES);
        e.app.update(Action::Into);
        assert_eq!(map_key(key(KeyCode::Esc), &e.app), Some(Action::Out));
    }

    #[test]
    fn review_dialog_keys() {
        let mut e = env(FILES);
        select(&mut e.app, "b.bin");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        let app = &e.app;
        assert_eq!(map_key(key(KeyCode::Enter), app), Some(Action::Confirm));
        assert_eq!(map_key(ch('x'), app), Some(Action::ConfirmIrreversible));
        assert_eq!(map_key(key(KeyCode::Esc), app), Some(Action::Cancel));
        assert_eq!(map_key(ch('y'), app), None);
        // The item list scrolls; the rest of the screen stays out of reach.
        assert_eq!(map_key(ch('j'), app), Some(Action::Down));
        for blocked in [' ', 's', 'h', 'l', 'd', 't', '/', 'r', '5'] {
            assert_eq!(map_key(ch(blocked), app), None, "{blocked:?}");
        }
    }

    #[test]
    fn irreversible_dialog_needs_y_not_enter() {
        let mut e = env(FILES);
        select(&mut e.app, "b.bin");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        e.app.update(Action::ConfirmIrreversible);
        let app = &e.app;
        assert_eq!(map_key(ch('y'), app), Some(Action::Confirm));
        assert_eq!(map_key(key(KeyCode::Enter), app), None);
        assert_eq!(map_key(ch('n'), app), Some(Action::Cancel));
        assert_eq!(map_key(key(KeyCode::Esc), app), Some(Action::Cancel));
    }

    #[test]
    fn holding_screen_keys() {
        let mut e = env(FILES);
        e.app.update(Action::GoTo(Screen::Holding));
        let app = &e.app;
        assert_eq!(map_key(ch('r'), app), Some(Action::RestoreSelected));
        assert_eq!(map_key(ch('D'), app), Some(Action::DeleteSelectedForever));
        assert_eq!(map_key(ch('E'), app), Some(Action::EmptyAll));
    }

    #[test]
    fn notice_dialog_dismisses_with_enter_or_esc() {
        let mut e = env(FILES);
        select(&mut e.app, "b.bin");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        settle(&mut e.app);
        let app = &e.app;
        assert_eq!(map_key(key(KeyCode::Enter), app), Some(Action::Cancel));
        assert_eq!(map_key(key(KeyCode::Esc), app), Some(Action::Cancel));
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_mapping() {
        let e = env(FILES);
        let app = &e.app;
        assert_eq!(
            map_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 7), app),
            Some(Action::Click { x: 5, y: 7 })
        );
        assert_eq!(
            map_mouse(mouse(MouseEventKind::ScrollDown, 5, 7), app),
            Some(Action::Scroll(3))
        );
        assert_eq!(
            map_mouse(mouse(MouseEventKind::ScrollUp, 5, 7), app),
            Some(Action::Scroll(-3))
        );
        assert_eq!(
            map_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 5, 7), app),
            None
        );
        assert_eq!(map_mouse(mouse(MouseEventKind::Moved, 5, 7), app), None);
    }

    #[test]
    fn mouse_ignored_while_dialog_open() {
        let mut e = env(FILES);
        select(&mut e.app, "b.bin");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        assert_eq!(
            map_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 7), &e.app),
            None
        );
    }

    #[test]
    fn too_small_terminal_only_accepts_quit() {
        let mut e = env(FILES);
        select(&mut e.app, "b.bin");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        e.app.too_small = true;
        let app = &e.app;
        assert_eq!(map_key(key(KeyCode::Enter), app), None);
        assert_eq!(map_key(ch('x'), app), None);
        assert_eq!(map_key(ch('y'), app), None);
        assert_eq!(map_key(ch('j'), app), None);
        assert_eq!(map_key(ch('q'), app), Some(Action::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_c, app), Some(Action::Quit));
        let click = mouse(MouseEventKind::Down(MouseButton::Left), 5, 7);
        assert_eq!(map_mouse(click, app), None);
        let wheel = mouse(MouseEventKind::ScrollDown, 5, 7);
        assert_eq!(map_mouse(wheel, app), None);
    }

    #[test]
    fn rescan_keys_on_every_screen() {
        let mut e = env(FILES);
        assert_eq!(map_key(ch('r'), &e.app), Some(Action::Rescan));
        assert_eq!(map_key(ch('R'), &e.app), Some(Action::RescanAll));
        e.app.update(Action::GoTo(Screen::Home));
        assert_eq!(map_key(ch('r'), &e.app), Some(Action::Rescan));
        assert_eq!(map_key(ch('R'), &e.app), Some(Action::RescanAll));
        // On the ToBeDeleted screen `r` restores, and R does nothing.
        e.app.update(Action::GoTo(Screen::Holding));
        assert_eq!(map_key(ch('r'), &e.app), Some(Action::RestoreSelected));
        assert_eq!(map_key(ch('R'), &e.app), None);
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.update(Action::FilterChar(c));
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn slash_opens_the_filter_only_in_the_explorer() {
        let mut e = env(FILES);
        assert_eq!(map_key(ch('/'), &e.app), Some(Action::FilterOpen));
        e.app.update(Action::GoTo(Screen::Home));
        assert_eq!(map_key(ch('/'), &e.app), None);
        e.app.update(Action::GoTo(Screen::Holding));
        assert_eq!(map_key(ch('/'), &e.app), None);
    }

    #[test]
    fn while_editing_every_character_is_text() {
        let mut e = env(FILES);
        e.app.update(Action::FilterOpen);
        for c in [
            'q', 'j', 'k', '/', ' ', 'd', 'G', '1', 'r', 'R', 'x', 'y', 'n', 'é', '日',
        ] {
            assert_eq!(map_key(ch(c), &e.app), Some(Action::FilterChar(c)), "{c:?}");
        }
    }

    #[test]
    fn editing_control_keys() {
        let mut e = env(FILES);
        e.app.update(Action::FilterOpen);
        let app = &e.app;
        let table: Vec<(KeyEvent, Option<Action>)> = vec![
            (key(KeyCode::Enter), Some(Action::FilterAccept)),
            (key(KeyCode::Esc), Some(Action::FilterClear)),
            (key(KeyCode::Backspace), Some(Action::FilterBackspace)),
            (key(KeyCode::Delete), Some(Action::FilterDelete)),
            (key(KeyCode::Left), Some(Action::FilterLeft)),
            (key(KeyCode::Right), Some(Action::FilterRight)),
            (key(KeyCode::Home), Some(Action::FilterHome)),
            (key(KeyCode::End), Some(Action::FilterEnd)),
            (key(KeyCode::Up), Some(Action::Up)),
            (key(KeyCode::Down), Some(Action::Down)),
            (key(KeyCode::PageUp), Some(Action::PageUp)),
            (key(KeyCode::PageDown), Some(Action::PageDown)),
            (key(KeyCode::Tab), None),
            (ctrl('u'), Some(Action::FilterClearLine)),
            (ctrl('w'), Some(Action::FilterClearWord)),
            (ctrl('c'), Some(Action::Quit)),
            (ctrl('x'), None),
            (KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT), None),
        ];
        for (k, expected) in table {
            assert_eq!(map_key(k, app), expected, "{k:?}");
        }
    }

    #[test]
    fn esc_clears_an_active_filter_before_going_out_or_quitting() {
        let mut e = env(FILES);
        e.app.update(Action::FilterOpen);
        typed(&mut e.app, "a");
        e.app.update(Action::FilterAccept);
        assert_eq!(
            map_key(key(KeyCode::Esc), &e.app),
            Some(Action::FilterClear)
        );
        assert_eq!(
            map_key(ch('q'), &e.app),
            Some(Action::Quit),
            "q still quits"
        );
        assert_eq!(
            map_key(ch('/'), &e.app),
            Some(Action::FilterOpen),
            "/ edits it"
        );
        e.app.update(Action::FilterClear);
        assert_eq!(map_key(key(KeyCode::Esc), &e.app), Some(Action::Quit));

        // Deeper in the tree: first Esc clears the filter, the next goes out.
        e.app.update(Action::Into);
        e.app.update(Action::FilterOpen);
        typed(&mut e.app, "f");
        e.app.update(Action::FilterAccept);
        assert_eq!(
            map_key(key(KeyCode::Esc), &e.app),
            Some(Action::FilterClear)
        );
        e.app.update(Action::FilterClear);
        assert_eq!(map_key(key(KeyCode::Esc), &e.app), Some(Action::Out));
    }

    #[test]
    fn the_filter_keys_are_blocked_on_a_too_small_terminal() {
        let mut e = env(FILES);
        e.app.too_small = true;
        assert_eq!(map_key(ch('/'), &e.app), None);
    }

    #[test]
    fn quit_warning_keys() {
        let mut e = env(FILES);
        select(&mut e.app, "b.bin");
        e.app.update(Action::ToggleMark);
        e.app.update(Action::OpenReview);
        e.app.update(Action::Confirm);
        crate::testutil::settle(&mut e.app);
        e.app.update(Action::Cancel);
        e.app.update(Action::Quit);
        assert!(matches!(
            e.app.dialog,
            Some(crate::app::Dialog::QuitWarning(_))
        ));
        let app = &e.app;
        assert_eq!(map_key(ch('q'), app), Some(Action::Confirm));
        assert_eq!(map_key(ch('y'), app), Some(Action::Confirm));
        assert_eq!(map_key(ch('e'), app), Some(Action::ReviewHeld));
        assert_eq!(map_key(key(KeyCode::Esc), app), Some(Action::Cancel));
        assert_eq!(map_key(ch('n'), app), Some(Action::Cancel));
        assert_eq!(
            map_key(key(KeyCode::Enter), app),
            Some(Action::Cancel),
            "Enter stays"
        );
        assert_eq!(map_key(ch('x'), app), None);
        assert_eq!(map_key(ch('j'), app), None);
        assert_eq!(
            map_key(ctrl('c'), app),
            Some(Action::Quit),
            "Ctrl-C twice quits"
        );
    }

    fn review_with(items: usize, commands: usize) -> Dialog {
        use crate::app::{Review, ReviewCommand, ReviewItem};
        use crate::catalog::CommandSpec;
        use crate::safety::Safety;
        use crate::usage::Usage;
        Dialog::Review(Review {
            items: (0..items)
                .map(|i| ReviewItem {
                    path: format!("/x/{i}").into(),
                    usage: Usage::default(),
                    safety: Safety::Safe,
                })
                .collect(),
            commands: (0..commands)
                .map(|i| ReviewCommand {
                    id: format!("c{i}"),
                    tool: "t".to_string(),
                    line: "tool clean".to_string(),
                    spec: CommandSpec::new("tool", &["clean"]),
                    up_to: 0,
                    path: std::path::PathBuf::new(),
                    can_force: false,
                    force: false,
                })
                .collect(),
            total: Usage::default(),
            space_ready: true,
            space: Default::default(),
            problems: Vec::new(),
        })
    }

    #[test]
    fn a_review_with_commands_is_confirmed_only_by_y() {
        let d = review_with(1, 1);
        assert_eq!(
            dialog_key(KeyCode::Enter, &d),
            None,
            "Enter never runs a command"
        );
        assert_eq!(dialog_key(KeyCode::Char('y'), &d), Some(Action::Confirm));
        assert_eq!(
            dialog_key(KeyCode::Char('x'), &d),
            Some(Action::ConfirmIrreversible)
        );
        assert_eq!(dialog_key(KeyCode::Esc, &d), Some(Action::Cancel));
    }

    #[test]
    fn a_review_of_only_commands_has_no_move_then_empty() {
        let d = review_with(0, 2);
        assert_eq!(dialog_key(KeyCode::Char('x'), &d), None);
        assert_eq!(dialog_key(KeyCode::Char('y'), &d), Some(Action::Confirm));
    }

    #[test]
    fn a_review_of_folders_only_still_confirms_with_enter() {
        let d = review_with(2, 0);
        assert_eq!(dialog_key(KeyCode::Enter, &d), Some(Action::Confirm));
    }

    #[test]
    fn a_digit_key_is_the_menu_position_of_a_page() {
        let e = env(FILES);
        for (i, entry) in crate::app::SIDEBAR.iter().enumerate() {
            let digit = char::from_digit(i as u32 + 1, 10).unwrap();
            assert_eq!(
                map_key(ch(digit), &e.app),
                Some(Action::Menu(i + 1)),
                "{} is entry {}",
                entry.label,
                i + 1
            );
        }
    }

    #[test]
    fn digits_are_text_while_typing_a_filter() {
        let mut e = env(FILES);
        e.app.update(Action::FilterOpen);
        assert_eq!(map_key(ch('5'), &e.app), Some(Action::FilterChar('5')));
    }

    #[test]
    fn ctrl_f_and_ctrl_b_page_like_vim_and_ctrl_d_and_u_still_work() {
        let e = env(FILES);
        let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(map_key(k('f'), &e.app), Some(Action::PageDown));
        assert_eq!(map_key(k('b'), &e.app), Some(Action::PageUp));
        assert_eq!(map_key(k('d'), &e.app), Some(Action::PageDown));
        assert_eq!(map_key(k('u'), &e.app), Some(Action::PageUp));
        // Ctrl-N is not a page key.
        assert_eq!(map_key(k('n'), &e.app), None);
    }

    #[test]
    fn page_keys_do_nothing_while_a_dialog_is_open_and_are_editing_keys_in_the_filter() {
        let mut e = env(FILES);
        let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        e.app.update(Action::FilterOpen);
        assert_eq!(
            map_key(k('f'), &e.app),
            None,
            "Ctrl-F is not text and not a page key while typing"
        );
        e.app.update(Action::FilterAccept);
        e.app.dialog = Some(Dialog::Notice("x".into()));
        assert_eq!(map_key(k('f'), &e.app), None);
        assert_eq!(map_key(k('b'), &e.app), None);
    }

    fn review_dialog(items: usize, commands: usize) -> Dialog {
        review_with(items, commands)
    }

    #[test]
    fn the_review_list_scrolls_with_every_navigation_key() {
        for commands in [0, 1] {
            let d = review_dialog(60, commands);
            for (code, want) in [
                (KeyCode::Char('j'), Action::Down),
                (KeyCode::Down, Action::Down),
                (KeyCode::Char('k'), Action::Up),
                (KeyCode::Up, Action::Up),
                (KeyCode::PageDown, Action::PageDown),
                (KeyCode::PageUp, Action::PageUp),
                (KeyCode::Home, Action::Top),
                (KeyCode::End, Action::Bottom),
                (KeyCode::Char('G'), Action::Bottom),
            ] {
                assert_eq!(
                    dialog_key(code, &d),
                    Some(want),
                    "{code:?} with {commands} commands"
                );
            }
        }
    }

    #[test]
    fn shift_j_and_k_scroll_the_docker_details_only_on_that_page() {
        let mut e = env(FILES);
        let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT);
        e.app.update(Action::GoTo(Screen::Docker));
        assert_eq!(map_key(k('J'), &e.app), Some(Action::DetailScroll(1)));
        assert_eq!(map_key(k('K'), &e.app), Some(Action::DetailScroll(-1)));
        e.app.update(Action::GoTo(Screen::Explorer));
        assert_eq!(map_key(k('J'), &e.app), None);
    }

    #[test]
    fn ctrl_f_and_ctrl_b_page_the_review_list_too() {
        let mut e = env(FILES);
        let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        e.app.dialog = Some(review_dialog(60, 0));
        assert_eq!(map_key(k('f'), &e.app), Some(Action::PageDown));
        assert_eq!(map_key(k('b'), &e.app), Some(Action::PageUp));
        assert_eq!(map_key(k('d'), &e.app), Some(Action::PageDown));
        assert_eq!(map_key(k('u'), &e.app), Some(Action::PageUp));
        // Other dialogs have nothing to scroll.
        e.app.dialog = Some(Dialog::Notice("x".into()));
        assert_eq!(map_key(k('f'), &e.app), None);
    }

    #[test]
    fn scrolling_the_review_never_confirms_or_cancels_it() {
        let d = review_dialog(60, 0);
        assert_eq!(dialog_key(KeyCode::Enter, &d), Some(Action::Confirm));
        assert_eq!(
            dialog_key(KeyCode::Char('x'), &d),
            Some(Action::ConfirmIrreversible)
        );
        assert_eq!(dialog_key(KeyCode::Esc, &d), Some(Action::Cancel));
        let d = review_dialog(60, 1);
        assert_eq!(
            dialog_key(KeyCode::Enter, &d),
            None,
            "Enter still never runs a command"
        );
    }

    #[test]
    fn the_mouse_wheel_scrolls_the_review_but_clicks_do_not_reach_the_list_behind_it() {
        let mut e = env(FILES);
        e.app.dialog = Some(review_dialog(60, 0));
        assert_eq!(
            map_mouse(mouse(MouseEventKind::ScrollDown, 5, 7), &e.app),
            Some(Action::Scroll(3))
        );
        assert_eq!(
            map_mouse(mouse(MouseEventKind::ScrollUp, 5, 7), &e.app),
            Some(Action::Scroll(-3))
        );
        assert_eq!(
            map_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 7), &e.app),
            None
        );
        // Other dialogs ignore the wheel as before.
        e.app.dialog = Some(Dialog::Notice("x".into()));
        assert_eq!(
            map_mouse(mouse(MouseEventKind::ScrollDown, 5, 7), &e.app),
            None
        );
    }

    fn progress(phase: crate::jobs::Phase) -> crate::app::DeleteProgress {
        crate::app::DeleteProgress {
            done: 0,
            total: 1,
            usage: crate::usage::Usage::default(),
            of: crate::usage::Usage::default(),
            phase,
            current: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn a_running_command_can_be_cancelled_but_a_move_cannot() {
        use crate::jobs::Phase;
        let running = Dialog::Deleting(progress(Phase::Command));
        assert_eq!(dialog_key(KeyCode::Esc, &running), Some(Action::Cancel));
        assert_eq!(dialog_key(KeyCode::Char('q'), &running), Some(Action::Quit));
        let moving = Dialog::Deleting(progress(Phase::Moving));
        assert_eq!(dialog_key(KeyCode::Esc, &moving), None);
        assert_eq!(dialog_key(KeyCode::Char('q'), &moving), None);
        // Ctrl-C always quits (which cancels a running command).
        let e = env(FILES);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_c, &e.app), Some(Action::Quit));
    }

    #[test]
    fn f_toggles_force_in_a_review_only_when_a_command_can_be_forced() {
        use crate::app::Dialog;
        let Dialog::Review(mut r) = review_with(1, 1) else {
            panic!()
        };
        assert_eq!(
            dialog_key(KeyCode::Char('f'), &Dialog::Review(r.clone())),
            None
        );
        r.commands[0].can_force = true;
        assert_eq!(
            dialog_key(KeyCode::Char('f'), &Dialog::Review(r.clone())),
            Some(Action::ToggleForce)
        );
        r.commands.clear();
        assert_eq!(dialog_key(KeyCode::Char('f'), &Dialog::Review(r)), None);
    }

    #[test]
    fn f_on_the_result_screen_retries_with_force_only_when_offered() {
        use crate::app::{Dialog, FinishInfo};
        let mut f = FinishInfo {
            moved: Default::default(),
            emptied: Default::default(),
            free_before: 0,
            free_after: 0,
            failures: 0,
            first_failure: None,
            commands: Vec::new(),
            retry: Vec::new(),
            started_tick: 0,
            vm_disk: None,
        };
        assert_eq!(
            dialog_key(KeyCode::Char('f'), &Dialog::Finish(f.clone())),
            None
        );
        let Dialog::Review(r) = review_with(0, 1) else {
            panic!()
        };
        f.retry = r.commands;
        assert_eq!(
            dialog_key(KeyCode::Char('f'), &Dialog::Finish(f.clone())),
            Some(Action::RetryForced)
        );
        // The ordinary ways out still work.
        assert_eq!(
            dialog_key(KeyCode::Enter, &Dialog::Finish(f)),
            Some(Action::Cancel)
        );
    }
}
