//! Terminal setup and restore. `TermGuard` restores on drop; the panic hook restores first and then lets the panic
//! message print to stderr. `kill -9` cannot be handled: run `reset` if a terminal is left raw.
use crossterm::cursor::Show;
use crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, stdout};
use std::sync::Once;

pub fn restore() {
    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, Show);
    let _ = disable_raw_mode();
}

/// Install (once) a panic hook that restores the terminal before the previous hook prints the panic.
pub fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            prev(info);
        }));
    });
}

pub struct TermGuard;

impl TermGuard {
    pub fn enter() -> io::Result<TermGuard> {
        install_panic_hook();
        enable_raw_mode()?;
        let guard = TermGuard;
        execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture)?;
        Ok(guard)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore();
    }
}

/// Give the terminal back to the shell for a while (a child process owns it), without dropping the guard.
pub fn suspend() {
    restore();
}

/// Take the terminal again after [`suspend`].
pub fn resume() {
    let _ = enable_raw_mode();
    let _ = execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture);
}
