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
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            KeyCode::Char('d') if app.dialog.is_none() => Some(Action::PageDown),
            KeyCode::Char('u') if app.dialog.is_none() => Some(Action::PageUp),
            _ => None,
        };
    }
    if let Some(dialog) = &app.dialog {
        return dialog_key(key.code, dialog);
    }
    match key.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Esc => Some(if app.stack.len() > 1 {
            Action::Out
        } else {
            Action::Quit
        }),
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
        KeyCode::Char('g') => Some(Action::Top),
        KeyCode::Char('t') => Some(Action::CycleGraph),
        KeyCode::Char('s') => Some(Action::CycleSort),
        KeyCode::Char('[') => Some(Action::GrowGraph),
        KeyCode::Char(']') => Some(Action::ShrinkGraph),
        KeyCode::Tab => Some(Action::FocusToggle),
        KeyCode::Char('1') => Some(Action::GoTo(Screen::Home)),
        KeyCode::Char('2') => Some(Action::GoTo(Screen::Explorer)),
        KeyCode::Char('3') => Some(Action::GoTo(Screen::Holding)),
        KeyCode::Char('r') if app.screen == Screen::Holding => Some(Action::RestoreSelected),
        KeyCode::Char('D') if app.screen == Screen::Holding => Some(Action::DeleteSelectedForever),
        KeyCode::Char('E') if app.screen == Screen::Holding => Some(Action::EmptyAll),
        _ => None,
    }
}

fn dialog_key(code: KeyCode, dialog: &Dialog) -> Option<Action> {
    match dialog {
        Dialog::Review(_) => match code {
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
        Dialog::Deleting(_) => None,
        Dialog::Finish(_) | Dialog::Notice(_) => match code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => Some(Action::Cancel),
            _ => None,
        },
    }
}

/// Translate a mouse event: left click selects, wheel scrolls.
pub fn map_mouse(ev: MouseEvent, app: &App) -> Option<Action> {
    if app.dialog.is_some() || app.too_small {
        return None;
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
            (ch('1'), Some(Action::GoTo(Screen::Home))),
            (ch('2'), Some(Action::GoTo(Screen::Explorer))),
            (ch('3'), Some(Action::GoTo(Screen::Holding))),
            (key(KeyCode::Esc), Some(Action::Quit)),
            // Dialog-only and holding-only keys do nothing here.
            (ch('x'), None),
            (ch('y'), None),
            (ch('r'), None),
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
        assert_eq!(map_key(ch('j'), app), None);
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
}
