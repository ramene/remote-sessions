//! The event loop: threads and one `mpsc` channel of [`Msg`]. The main thread owns the state and the terminal.
//! Threads: a stream reader, an input reader, one short worker per HTTP call, and a poller while the stream is down.
use crate::model::*;
use crate::reduce::reduce;
use crate::term::TermGuard;
use crate::logo::Logo;
use crate::theme::Theme;
use crate::view::{draw_in, DrawInfo, BAND_MIN_H};
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, MouseButton, MouseEventKind};
use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::style::Print;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Options {
    pub name: String,
    pub host: String,
    pub no_color: bool,
    pub ascii: bool,
    /// Where the prompt history lives; None = `~/.local/state/sessions-tui/history`.
    pub history_path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// A fatal error shown in the panel (the caller should exit with code 2).
    pub fatal: Option<String>,
}

pub const POLL_EVERY: Duration = Duration::from_secs(3);
pub const STREAM_RETRY_EVERY: Duration = Duration::from_secs(30);
const HISTORY_KEEP: usize = 1000;

pub fn default_history_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/sessions-tui/history"))
}

/// One history entry per line; `\` and newlines are escaped.
pub fn encode_history(entry: &str) -> String {
    entry.replace('\\', "\\\\").replace('\n', "\\n")
}

pub fn decode_history(line: &str) -> String {
    let mut out = String::new();
    let mut it = line.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn load_history(path: &Option<PathBuf>) -> Vec<String> {
    let Some(p) = path else { return Vec::new() };
    let text = std::fs::read_to_string(p).unwrap_or_default();
    let all: Vec<String> = text.lines().map(decode_history).collect();
    all[all.len().saturating_sub(HISTORY_KEEP)..].to_vec()
}

fn save_history(path: &Option<PathBuf>, entry: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    let Some(p) = path else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(p) {
        let _ = writeln!(f, "{}", encode_history(entry));
    }
}

fn os_seed() -> u64 {
    use std::io::Read;
    let mut b = [0u8; 8];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok() {
        return u64::from_le_bytes(b);
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) ^ ((std::process::id() as u64) << 32)
}

fn spawn_input(tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
                continue;
            }
            let msg = match event::read() {
                Ok(Event::Key(k)) => Msg::Key(k),
                Ok(Event::Paste(t)) => Msg::Paste(t),
                Ok(Event::Resize(w, h)) => Msg::Resize(w, h),
                Ok(Event::Mouse(m)) => match m.kind {
                    MouseEventKind::ScrollUp => Msg::Wheel(1),
                    MouseEventKind::ScrollDown => Msg::Wheel(-1),
                    MouseEventKind::Down(MouseButton::Left) => Msg::ClickAt(m.column, m.row),
                    _ => continue,
                },
                Ok(_) => continue,
                Err(_) => break,
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
}

fn spawn_poll(backend: Arc<dyn Backend>, tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut since_retry = Duration::ZERO;
        loop {
            let mut waited = Duration::ZERO;
            while waited < POLL_EVERY {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
                waited += Duration::from_millis(100);
            }
            since_retry += POLL_EVERY;
            let sent = match backend.poll() {
                Ok(events) => events.into_iter().all(|e| tx.send(Msg::Item(Item::Event(e))).is_ok()),
                Err(e) => tx.send(Msg::PollFailed(e)).is_ok(),
            };
            if !sent {
                return;
            }
            if since_retry >= STREAM_RETRY_EVERY {
                since_retry = Duration::ZERO;
                if tx.send(Msg::Retry).is_err() {
                    return;
                }
            }
        }
    });
}

struct Runner {
    backend: Arc<dyn Backend>,
    tx: Sender<Msg>,
    poll_stop: Option<Arc<AtomicBool>>,
    history: Option<PathBuf>,
}

impl Runner {
    /// Run one effect. Returns true when the loop must end.
    fn run(&mut self, e: Effect, redraw: &mut bool, mouse_now: &mut Option<bool>) -> bool {
        let (b, tx) = (self.backend.clone(), self.tx.clone());
        match e {
            Effect::StartStream { from } => {
                std::thread::spawn(move || b.stream(from, tx));
            }
            Effect::StartPoll => {
                let stop = Arc::new(AtomicBool::new(false));
                self.poll_stop = Some(stop.clone());
                spawn_poll(b, tx, stop);
            }
            Effect::StopPoll => {
                if let Some(s) = self.poll_stop.take() {
                    s.store(true, Ordering::Relaxed);
                }
            }
            Effect::Say { uuid, text } => {
                std::thread::spawn(move || {
                    let res = b.say(&uuid, &text);
                    let _ = tx.send(Msg::Net(Net::Say { uuid, res }));
                });
            }
            Effect::Interrupt => {
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Net(Net::Interrupt(b.interrupt())));
                });
            }
            Effect::SetMode(mode) => {
                std::thread::spawn(move || {
                    let res = b.set_mode(&mode);
                    let _ = tx.send(Msg::Net(Net::Mode { mode, res }));
                });
            }
            Effect::Answer { request_id, decision } => {
                std::thread::spawn(move || {
                    let allow = decision == Decision::Allow;
                    let res = b.answer(&request_id, decision);
                    let _ = tx.send(Msg::Net(Net::Answer { request_id, allow, res }));
                });
            }
            Effect::Mouse(on) => *mouse_now = Some(on),
            Effect::Redraw => *redraw = true,
            Effect::SaveHistory(t) => save_history(&self.history, &t),
            Effect::Quit => return true,
        }
        false
    }
}

/// Run the TUI until the user leaves. Never stops the session. The terminal is restored on every exit path
/// (normal, error, panic, SIGINT/SIGTERM/SIGHUP).
pub fn run(backend: Arc<dyn Backend>, opts: Options) -> Result<Outcome, String> {
    if !io::stdout().is_terminal() {
        return Err("the TUI needs a terminal".into());
    }
    let th = Theme::detect(opts.no_color, opts.ascii);
    let sig = Arc::new(AtomicBool::new(false));
    for s in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        signal_hook::flag::register(s, sig.clone()).map_err(|e| e.to_string())?;
    }
    let history = opts.history_path.clone().or_else(default_history_path);
    let guard = TermGuard::enter().map_err(|e| e.to_string())?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<Msg>();
    let stop_input = Arc::new(AtomicBool::new(false));
    spawn_input(tx.clone(), stop_input.clone());
    {
        let (b, tx) = (backend.clone(), tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Snapshot(b.snapshot()));
        });
    }
    let mut state = State::new(&opts.name, &opts.host);
    state.seed = os_seed();
    state.history = load_history(&history);
    if let Ok(sz) = term.size() {
        state.size = (sz.width, sz.height);
    }
    let mut runner = Runner { backend, tx: tx.clone(), poll_stop: None, history };
    let start = Instant::now();
    let mut info = DrawInfo::default();
    let mut result: Result<(), String> = Ok(());
    let mut dirty = true;
    // The optional logo image (None: today's screen, untouched). Written after a frame, on the first one and after a clear or a
    // resize; ratatui redraws only changed cells, so the blank cells under it are not rewritten and the image stays.
    let logo = Logo::load(&|k: &str| std::env::var(k).ok(), crate::logo::terminal_cell_px());
    let mut logo_fresh = true;
    let mut last_size = (0u16, 0u16);
    'outer: loop {
        if sig.load(Ordering::Relaxed) {
            break;
        }
        let first = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut batch = vec![Msg::Tick(start.elapsed().as_millis() as u64)];
        let ticked_only = first.is_none();
        batch.extend(first);
        while batch.len() < 64 {
            match rx.try_recv() {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        if ticked_only && state.spinner_active() {
            dirty = true;
        }
        let (mut redraw, mut mouse_now) = (false, None);
        for m in batch {
            let m = match m {
                Msg::ClickAt(_, row) => match info.rows.get(row as usize).copied().flatten() {
                    Some(id) => Msg::Click(id),
                    None => continue,
                },
                other => {
                    if !matches!(other, Msg::Tick(_)) {
                        dirty = true;
                    }
                    other
                }
            };
            let (next, effects) = reduce(state, m);
            state = next;
            for e in effects {
                if runner.run(e, &mut redraw, &mut mouse_now) {
                    break 'outer;
                }
            }
        }
        if let Some(on) = mouse_now {
            let _ = if on { execute!(io::stdout(), EnableMouseCapture) } else { execute!(io::stdout(), DisableMouseCapture) };
        }
        if redraw {
            let _ = term.clear();
            dirty = true;
            logo_fresh = true;
        }
        if state.size != last_size {
            last_size = state.size;
            logo_fresh = true;
            dirty = true;
        }
        if dirty {
            dirty = false;
            let mut drawn = DrawInfo::default();
            let band = logo.is_some() && state.size.1 >= BAND_MIN_H && state.size.0 >= crate::view::MIN_W;
            if let Err(e) = term.draw(|f| drawn = draw_in(f, &state, &th, band)) {
                result = Err(e.to_string());
                break;
            }
            state.view_total = drawn.total;
            state.view_h = drawn.height;
            info = drawn;
            if let (true, true, Some(l)) = (band, logo_fresh, &logo) {
                let _ = execute!(io::stdout(), MoveTo(1, 0), Print(&l.sequence));
                logo_fresh = false;
                dirty = true; // one more frame puts the cursor back where the prompt wants it
            }
        }
    }
    stop_input.store(true, Ordering::Relaxed);
    if let Some(s) = runner.poll_stop.take() {
        s.store(true, Ordering::Relaxed);
    }
    drop(term);
    drop(guard);
    result?;
    if state.fatal.is_none() {
        println!("left the session running");
    }
    Ok(Outcome { fatal: state.fatal })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_lines_round_trip() {
        for e in ["plain", "two\nlines", "back\\slash", "\\n literal", ""] {
            let enc = encode_history(e);
            assert!(!enc.contains('\n'));
            assert_eq!(decode_history(&enc), e);
        }
    }
}
