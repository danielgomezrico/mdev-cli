//! Own key vocabulary, mapped once from the terminal library's events.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Space,
    SelectAll,
    SelectNone,
    Filter,
    Enter,
    Esc,
    Quit,
    Backspace,
    Char(char),
    Other,
}

/// Map a key press. `filter_mode` makes plain letters text input instead of
/// shortcuts.
pub fn from_event(ev: &KeyEvent, filter_mode: bool) -> Key {
    if ev.modifiers.contains(KeyModifiers::CONTROL) && ev.code == KeyCode::Char('c') {
        return Key::Quit;
    }
    match ev.code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(c) if filter_mode => Key::Char(c),
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char('a') => Key::SelectAll,
        KeyCode::Char('n') => Key::SelectNone,
        KeyCode::Char('/') => Key::Filter,
        KeyCode::Char('q') => Key::Quit,
        KeyCode::Char('k') => Key::Up,
        KeyCode::Char('j') => Key::Down,
        KeyCode::Char('h') => Key::Left,
        KeyCode::Char('l') => Key::Right,
        KeyCode::Char(c) => Key::Char(c),
        _ => Key::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn shortcuts_outside_filter_mode() {
        assert_eq!(from_event(&press(KeyCode::Char(' ')), false), Key::Space);
        assert_eq!(
            from_event(&press(KeyCode::Char('a')), false),
            Key::SelectAll
        );
        assert_eq!(
            from_event(&press(KeyCode::Char('n')), false),
            Key::SelectNone
        );
        assert_eq!(from_event(&press(KeyCode::Char('/')), false), Key::Filter);
        assert_eq!(from_event(&press(KeyCode::Char('q')), false), Key::Quit);
        assert_eq!(from_event(&press(KeyCode::Char('j')), false), Key::Down);
        assert_eq!(from_event(&press(KeyCode::Enter), false), Key::Enter);
    }

    #[test]
    fn letters_are_text_in_filter_mode_and_ctrl_c_quits() {
        assert_eq!(from_event(&press(KeyCode::Char('a')), true), Key::Char('a'));
        assert_eq!(from_event(&press(KeyCode::Char('/')), true), Key::Char('/'));
        assert_eq!(from_event(&press(KeyCode::Esc), true), Key::Esc);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(from_event(&ctrl_c, true), Key::Quit);
    }
}
