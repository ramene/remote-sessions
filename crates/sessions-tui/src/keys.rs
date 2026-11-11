//! The key map: `(KeyEvent, &State) -> Action`. Pure and table-testable. The reducer decides what an action does.
use crate::model::{Modal, State};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edit {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    DelToStart,
    DelToEnd,
    DelWord,
    WordLeft,
    WordRight,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    None,
    Send,
    Newline,
    /// Esc while the session is working.
    Interrupt,
    /// Esc while idle (a second one soon after clears the input).
    EscIdle,
    CycleMode,
    ToggleExpand,
    Leave,
    PageUp,
    PageDown,
    HalfUp,
    HalfDown,
    Top,
    Bottom,
    Redraw,
    Help,
    HistorySearch,
    Up,
    Down,
    Complete,
    CompleteBack,
    Edit(Edit),
    // Modal keys. They never reach the input.
    ModalDigit(u8),
    ModalUp,
    ModalDown,
    ModalConfirm,
    /// Esc in the permission modal: deny with the default message.
    ModalDeny,
    ModalReason,
    ModalChar(char),
    ModalBackspace,
    ModalCancelField,
    ModalFull,
    CloseModal,
}

/// The action for a key press in this state. Key releases and repeats of modifiers give `None`.
pub fn map_key(s: &State, k: KeyEvent) -> Action {
    use Action::*;
    if k.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    // Ctrl-C always leaves, whatever owns the keyboard. It never stops the session.
    if ctrl && k.code == KeyCode::Char('c') {
        return Leave;
    }
    if s.fatal.is_some() {
        return Leave;
    }
    match &s.modal {
        Modal::Permission { reason, sending, .. } => {
            if *sending {
                return None;
            }
            if reason.is_some() {
                return match k.code {
                    KeyCode::Enter => ModalConfirm,
                    KeyCode::Esc => ModalCancelField,
                    KeyCode::Backspace => ModalBackspace,
                    KeyCode::Char(c) if !ctrl && !alt => ModalChar(c),
                    _ => None,
                };
            }
            return match k.code {
                KeyCode::Char(c @ '1'..='3') if !ctrl => ModalDigit(c as u8 - b'0'),
                KeyCode::Up => ModalUp,
                KeyCode::Down => ModalDown,
                KeyCode::Enter => ModalConfirm,
                KeyCode::Esc => ModalDeny,
                KeyCode::Tab => ModalReason,
                KeyCode::Char('o') if ctrl => ModalFull,
                _ => None,
            };
        }
        Modal::ModePicker { .. } => {
            return match k.code {
                KeyCode::Char(c @ '1'..='6') if !ctrl => ModalDigit(c as u8 - b'0'),
                KeyCode::Up => ModalUp,
                KeyCode::Down => ModalDown,
                KeyCode::Enter => ModalConfirm,
                KeyCode::Esc => CloseModal,
                _ => None,
            };
        }
        Modal::Help => {
            return match k.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?') => CloseModal,
                _ => None,
            };
        }
        Modal::None => {}
    }
    let empty = s.input.text.is_empty();
    match k.code {
        KeyCode::Enter if shift || alt => Newline,
        KeyCode::Enter => Send,
        KeyCode::Char('j') if ctrl => Newline,
        KeyCode::Esc => if s.busy() { Interrupt } else { EscIdle },
        KeyCode::Tab => if s.input.text.starts_with('/') && !s.input.text.contains(char::is_whitespace) { Complete } else { CycleMode },
        KeyCode::BackTab => CompleteBack,
        KeyCode::Char('o') if ctrl => ToggleExpand,
        KeyCode::Char('d') if ctrl && alt => HalfDown,
        KeyCode::Char('u') if ctrl && alt => HalfUp,
        KeyCode::Char('d') if ctrl => if empty { Leave } else { Edit(self::Edit::Delete) },
        KeyCode::Char('l') if ctrl => Redraw,
        KeyCode::Char('r') if ctrl => HistorySearch,
        KeyCode::Char('a') if ctrl => Edit(self::Edit::Home),
        KeyCode::Char('e') if ctrl => Edit(self::Edit::End),
        KeyCode::Char('u') if ctrl => Edit(self::Edit::DelToStart),
        KeyCode::Char('k') if ctrl => Edit(self::Edit::DelToEnd),
        KeyCode::Char('w') if ctrl => Edit(self::Edit::DelWord),
        KeyCode::Char('b') if alt => Edit(self::Edit::WordLeft),
        KeyCode::Char('f') if alt => Edit(self::Edit::WordRight),
        KeyCode::PageUp => PageUp,
        KeyCode::PageDown => PageDown,
        KeyCode::Home => if empty { Top } else { Edit(self::Edit::Home) },
        KeyCode::End => if empty { Bottom } else { Edit(self::Edit::End) },
        KeyCode::Up => Up,
        KeyCode::Down => Down,
        KeyCode::Left => if alt || ctrl { Edit(self::Edit::WordLeft) } else { Edit(self::Edit::Left) },
        KeyCode::Right => if alt || ctrl { Edit(self::Edit::WordRight) } else { Edit(self::Edit::Right) },
        KeyCode::Backspace => Edit(self::Edit::Backspace),
        KeyCode::Delete => Edit(self::Edit::Delete),
        KeyCode::Char('?') if empty && !ctrl && !alt => Help,
        KeyCode::Char(c) if !ctrl && !alt => Edit(self::Edit::Char(c)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Modal, Turn};
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }
    fn none() -> KeyModifiers { KeyModifiers::NONE }
    fn st() -> State { State::new("s", "mac-a") }
    fn busy() -> State { let mut s = st(); s.turn = Turn::Working { since: 0, tool: None }; s }
    fn perm() -> State { let mut s = st(); s.modal = Modal::Permission { choice: 0, reason: None, sending: false, full: false }; s }
    fn field() -> State { let mut s = st(); s.modal = Modal::Permission { choice: 1, reason: Some(String::new()), sending: false, full: false }; s }
    fn typed(t: &str) -> State { let mut s = st(); s.input.text = t.into(); s.input.cursor = t.chars().count(); s }

    #[test]
    fn global_keys_table() {
        use Action::*;
        let c = KeyModifiers::CONTROL;
        let rows: Vec<(KeyEvent, State, Action)> = vec![
            (key(KeyCode::Enter, none()), st(), Send),
            (key(KeyCode::Enter, KeyModifiers::SHIFT), st(), Newline),
            (key(KeyCode::Enter, KeyModifiers::ALT), st(), Newline),
            (key(KeyCode::Char('j'), c), st(), Newline),
            (key(KeyCode::Esc, none()), busy(), Interrupt),
            (key(KeyCode::Esc, none()), st(), EscIdle),
            (key(KeyCode::Tab, none()), st(), CycleMode),
            (key(KeyCode::Tab, none()), typed("/mo"), Complete),
            (key(KeyCode::Tab, none()), typed("/mode plan"), CycleMode),
            (key(KeyCode::BackTab, KeyModifiers::SHIFT), st(), CompleteBack),
            (key(KeyCode::Char('o'), c), st(), ToggleExpand),
            (key(KeyCode::Char('c'), c), st(), Leave),
            (key(KeyCode::Char('c'), c), busy(), Leave),
            (key(KeyCode::Char('c'), c), perm(), Leave),
            (key(KeyCode::Char('c'), c), field(), Leave),
            (key(KeyCode::Char('d'), c), st(), Leave),
            (key(KeyCode::Char('d'), c), typed("ab"), Edit(self::Edit::Delete)),
            (key(KeyCode::PageUp, none()), st(), PageUp),
            (key(KeyCode::PageDown, none()), st(), PageDown),
            (key(KeyCode::Home, none()), st(), Top),
            (key(KeyCode::End, none()), st(), Bottom),
            (key(KeyCode::Home, none()), typed("a"), Edit(self::Edit::Home)),
            (key(KeyCode::End, none()), typed("a"), Edit(self::Edit::End)),
            (key(KeyCode::Char('d'), c | KeyModifiers::ALT), st(), HalfDown),
            (key(KeyCode::Char('u'), c | KeyModifiers::ALT), st(), HalfUp),
            (key(KeyCode::Char('l'), c), st(), Redraw),
            (key(KeyCode::Char('r'), c), st(), HistorySearch),
            (key(KeyCode::Char('?'), none()), st(), Help),
            (key(KeyCode::Char('?'), none()), typed("a"), Edit(self::Edit::Char('?'))),
            (key(KeyCode::Up, none()), st(), Up),
            (key(KeyCode::Down, none()), st(), Down),
            (key(KeyCode::Char('u'), c), st(), Edit(self::Edit::DelToStart)),
            (key(KeyCode::Char('k'), c), st(), Edit(self::Edit::DelToEnd)),
            (key(KeyCode::Char('w'), c), st(), Edit(self::Edit::DelWord)),
            (key(KeyCode::Char('a'), c), st(), Edit(self::Edit::Home)),
            (key(KeyCode::Char('e'), c), st(), Edit(self::Edit::End)),
            (key(KeyCode::Char('b'), KeyModifiers::ALT), st(), Edit(self::Edit::WordLeft)),
            (key(KeyCode::Char('f'), KeyModifiers::ALT), st(), Edit(self::Edit::WordRight)),
            (key(KeyCode::Char('x'), none()), st(), Edit(self::Edit::Char('x'))),
            (key(KeyCode::Char('X'), KeyModifiers::SHIFT), st(), Edit(self::Edit::Char('X'))),
        ];
        for (k, s, want) in rows {
            assert_eq!(map_key(&s, k), want, "{k:?}");
        }
    }

    #[test]
    fn permission_modal_captures_every_key() {
        use Action::*;
        let rows = vec![
            (key(KeyCode::Char('1'), none()), ModalDigit(1)),
            (key(KeyCode::Char('2'), none()), ModalDigit(2)),
            (key(KeyCode::Char('3'), none()), ModalDigit(3)),
            (key(KeyCode::Up, none()), ModalUp),
            (key(KeyCode::Down, none()), ModalDown),
            (key(KeyCode::Enter, none()), ModalConfirm),
            (key(KeyCode::Esc, none()), ModalDeny),
            (key(KeyCode::Tab, none()), ModalReason),
            (key(KeyCode::Char('o'), KeyModifiers::CONTROL), ModalFull),
            (key(KeyCode::Char('x'), none()), None),
            (key(KeyCode::Char('4'), none()), None),
            (key(KeyCode::PageUp, none()), None),
        ];
        for (k, want) in rows {
            assert_eq!(map_key(&perm(), k), want, "{k:?}");
        }
        // Typing into the reason field never leaks to the prompt, Esc closes just the field.
        assert_eq!(map_key(&field(), key(KeyCode::Char('1'), none())), ModalChar('1'));
        assert_eq!(map_key(&field(), key(KeyCode::Backspace, none())), ModalBackspace);
        assert_eq!(map_key(&field(), key(KeyCode::Enter, none())), ModalConfirm);
        assert_eq!(map_key(&field(), key(KeyCode::Esc, none())), ModalCancelField);
        let mut sending = perm();
        sending.modal = Modal::Permission { choice: 0, reason: Option::None, sending: true, full: false };
        assert_eq!(map_key(&sending, key(KeyCode::Enter, none())), None);
    }

    #[test]
    fn other_modals_and_fatal() {
        let mut s = st();
        s.modal = Modal::Help;
        assert_eq!(map_key(&s, key(KeyCode::Esc, none())), Action::CloseModal);
        assert_eq!(map_key(&s, key(KeyCode::Char('x'), none())), Action::None);
        s.modal = Modal::ModePicker { choice: 0 };
        assert_eq!(map_key(&s, key(KeyCode::Char('6'), none())), Action::ModalDigit(6));
        assert_eq!(map_key(&s, key(KeyCode::Esc, none())), Action::CloseModal);
        let mut f = st();
        f.fatal = Some("x".into());
        assert_eq!(map_key(&f, key(KeyCode::Char('q'), none())), Action::Leave);
    }

    #[test]
    fn releases_are_ignored() {
        let mut k = key(KeyCode::Enter, none());
        k.kind = KeyEventKind::Release;
        assert_eq!(map_key(&st(), k), Action::None);
    }
}
