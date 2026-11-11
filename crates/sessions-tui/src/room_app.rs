//! The control room's event loop: threads and one `mpsc` channel, the main thread owns the state and the terminal.
//!
//! Threads: an input reader, a Remote Control poller (every 3 s) and a cloud poller (every 10 s). Each poll is one
//! call of the backend (a process listing and one request per served session, or one `cloud-sessions list`); a
//! kick (the `r` key, a finished action) wakes a poller early. One short worker thread per action. Reading a cloud
//! session's newest event is one call and never more than one is in flight (see `RoomState::want_activity`).
//! Dropping in leaves the alternate screen, runs the session's own client in the same terminal, then comes back.
use crate::room::*;
use crate::term::{resume, suspend, TermGuard};
use crate::logo::Logo;
use crate::theme::Theme;
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::execute;
use crossterm::style::Print;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REMOTE_EVERY: Duration = Duration::from_secs(3);
pub const CLOUD_EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct RoomOptions {
    pub host: String,
    pub no_color: bool,
    pub ascii: bool,
    pub demo: bool,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn spawn_input(tx: Sender<RoomMsg>, stop: Arc<AtomicBool>, paused: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if paused.load(Ordering::Relaxed) || !event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if paused.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
            let msg = match event::read() {
                Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => RoomMsg::Key(k),
                Ok(Event::Paste(t)) => RoomMsg::Paste(t),
                Ok(Event::Resize(w, h)) => RoomMsg::Resize(w, h),
                Ok(Event::Mouse(m)) => match m.kind {
                    MouseEventKind::ScrollUp => RoomMsg::Wheel(m.column, m.row, 1),
                    MouseEventKind::ScrollDown => RoomMsg::Wheel(m.column, m.row, -1),
                    MouseEventKind::Down(MouseButton::Left) => RoomMsg::Click(m.column, m.row),
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

/// A poller: read now, then wait for the interval or a kick, forever (until the receiver is gone).
fn spawn_poller(every: Duration, kick: Receiver<()>, tx: Sender<RoomMsg>, read: impl Fn() -> RoomMsg + Send + 'static) {
    std::thread::spawn(move || loop {
        if tx.send(read()).is_err() {
            return;
        }
        match kick.recv_timeout(every) {
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            _ => {}
        }
    });
}

/// Run the control room until the user leaves. It never stops, answers or sends anything on its own.
pub fn run_room(backend: Arc<dyn RoomBackend>, opts: RoomOptions) -> Result<(), String> {
    if !io::stdout().is_terminal() {
        return Err("the control room needs a terminal (use --once for one frame)".into());
    }
    let th = Theme::detect(opts.no_color, opts.ascii);
    let sig = Arc::new(AtomicBool::new(false));
    for s in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        signal_hook::flag::register(s, sig.clone()).map_err(|e| e.to_string())?;
    }
    let guard = TermGuard::enter().map_err(|e| e.to_string())?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel::<RoomMsg>();
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    spawn_input(tx.clone(), stop.clone(), paused.clone());
    let (kick_remote, rx_remote) = mpsc::channel::<()>();
    let (kick_cloud, rx_cloud) = mpsc::channel::<()>();
    let specs = backend.panels();
    // One poller per extra panel, at the panel's own interval.
    let mut kick_panels: Vec<Sender<()>> = Vec::new();
    {
        let b = backend.clone();
        spawn_poller(REMOTE_EVERY, rx_remote, tx.clone(), move || RoomMsg::Remote(b.remote()));
        let b = backend.clone();
        spawn_poller(CLOUD_EVERY, rx_cloud, tx.clone(), move || RoomMsg::Cloud(b.cloud()));
        for (k, spec) in specs.iter().enumerate() {
            let (kick, rx_panel) = mpsc::channel::<()>();
            kick_panels.push(kick);
            let b = backend.clone();
            spawn_poller(Duration::from_millis(spec.every_ms.max(500)), rx_panel, tx.clone(), move || RoomMsg::Panel(k, b.panel(k)));
        }
    }
    let kick_all_panels = || {
        for k in &kick_panels {
            let _ = k.send(());
        }
    };
    let mut state = RoomState::new(&opts.host).with_panels(specs);
    state.demo = opts.demo;
    state.air = th.air();
    // The optional logo image (None: the one-row text header, untouched). Written after a frame, on the first one and after a
    // resize or a drop-in; ratatui rewrites only changed cells, so the cells under the image stay as they are between those.
    let logo = Logo::load(&|k: &str| std::env::var(k).ok(), crate::logo::terminal_cell_px());
    let band_for = |size: (u16, u16)| logo.is_some() && size.1 >= BAND_MIN_H && size.0 >= MIN_W;
    let mut logo_fresh = true;
    let mut last_size = (0u16, 0u16);
    state.routines_available = backend.routines_supported();
    state.routines = backend.routines_on();
    state.now_ms = now_ms();
    if let Ok(sz) = term.size() {
        state.size = (sz.width, sz.height);
    }
    let mut dirty = true;
    let mut last_sec = state.now_ms / 1000;
    let mut result: Result<(), String> = Ok(());
    'outer: loop {
        if sig.load(Ordering::Relaxed) {
            break;
        }
        let first = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let now = now_ms();
        let mut batch = vec![RoomMsg::Tick(now)];
        batch.extend(first);
        while batch.len() < 64 {
            match rx.try_recv() {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        if now / 1000 != last_sec {
            last_sec = now / 1000;
            dirty = true; // ages tick
        }
        for m in batch {
            if !matches!(m, RoomMsg::Tick(_)) {
                dirty = true;
            }
            let (next, effects) = reduce(state, m);
            state = next;
            state.band = band_for(state.size);
            for e in effects {
                match e {
                    RoomEffect::Quit => break 'outer,
                    RoomEffect::Refresh => {
                        let _ = kick_remote.send(());
                        let _ = kick_cloud.send(());
                        kick_all_panels();
                    }
                    RoomEffect::Routines(on) => {
                        backend.set_routines(on);
                        let _ = kick_cloud.send(());
                    }
                    RoomEffect::FetchActivity(id) => {
                        let (b, tx) = (backend.clone(), tx.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(RoomMsg::Activity(b.activity(&id)));
                        });
                    }
                    RoomEffect::Answer { session, request_id, allow } => {
                        let (b, tx, k) = (backend.clone(), tx.clone(), kick_remote.clone());
                        std::thread::spawn(move || {
                            let t = match b.answer(&session, &request_id, allow) {
                                Ok(text) => Toast { text, ok: true },
                                Err(text) => Toast { text, ok: false },
                            };
                            let _ = tx.send(RoomMsg::Done(t));
                            let _ = k.send(());
                        });
                    }
                    RoomEffect::CloudSend { id, text } => {
                        let (b, tx, k) = (backend.clone(), tx.clone(), kick_cloud.clone());
                        std::thread::spawn(move || {
                            let t = match b.cloud_send(&id, &text) {
                                Ok(text) => Toast { text, ok: true },
                                Err(text) => Toast { text, ok: false },
                            };
                            let _ = tx.send(RoomMsg::Done(t));
                            let _ = k.send(());
                        });
                    }
                    RoomEffect::DropIn(sel) => {
                        // Stop reading the keyboard, give the terminal away, run the client, take it back.
                        paused.store(true, Ordering::Relaxed);
                        std::thread::sleep(Duration::from_millis(250));
                        suspend();
                        let outcome = backend.drop_in(&sel);
                        resume();
                        let _ = term.clear();
                        logo_fresh = true;
                        paused.store(false, Ordering::Relaxed);
                        let t = match outcome {
                            Ok(text) => Toast { text, ok: true },
                            Err(text) => Toast { text, ok: false },
                        };
                        let _ = tx.send(RoomMsg::Done(t));
                        let _ = kick_remote.send(());
                        let _ = kick_cloud.send(());
                        kick_all_panels();
                        if let Ok(sz) = term.size() {
                            let _ = tx.send(RoomMsg::Resize(sz.width, sz.height));
                        }
                    }
                }
            }
        }
        if dirty {
            dirty = false;
            if let Ok(sz) = term.size() {
                if (sz.width, sz.height) != state.size {
                    state.size = (sz.width, sz.height);
                }
            }
            state.band = band_for(state.size);
            if state.size != last_size {
                last_size = state.size;
                logo_fresh = true;
            }
            if let Err(e) = term.draw(|f| {
                draw(f, &state, &th);
            }) {
                result = Err(e.to_string());
                break;
            }
            if let (true, true, Some(l)) = (state.band, logo_fresh, &logo) {
                let _ = execute!(io::stdout(), MoveTo(1, 0), Print(&l.sequence));
                logo_fresh = false;
                dirty = true; // one more frame puts the cursor back
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    drop(term);
    drop(guard);
    result
}
