//! `control`: the interactive control room over Remote Control sessions and cloud sessions. The screen itself is the
//! `sessions-tui` crate (`room`); this file is the data side and the actions.
//!
//! * Remote Control: `ps` for the served workers (one process), then each worker's pending prompts over the events API
//!   (requests, in parallel, at most [`MAX_REMOTE`]). The login is read once and kept, not looked up every poll.
//! * Cloud: one `cloud-sessions list`; a session's newest event is one `cloud-sessions events <id>` call, asked for by
//!   the screen for the selected row and never in a loop. When `cloud-sessions` is not installed the panel says so.
//! * Drop in: the session's own client runs in the same terminal (`remote-sessions attach <id> --tui`,
//!   `cloud-sessions watch <id>`), the room is back when it exits. A cloud message is passed on stdin, never on argv.
//! * Approve and deny reuse the core `Client::answer` (the same call as the `approve` / `deny` verbs).
//!
//! Event text and titles are untrusted data; they are cleaned by the screen before they are drawn.
use crate::{Ctx, Result};
use remote_sessions_core::api::Client;
use remote_sessions_core::control::Decision;
use remote_sessions_core::rc;
use serde_json::Value;
use sessions_tui::room::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_REMOTE: usize = 16;
pub const DEFAULT_CLOUD_MAX: usize = 50;
/// Runs shown under each routine. A fixed argv, never built from remote text.
const ROUTINE_RUNS: &str = "3";
pub const ABSENT: &str = "cloud-sessions is not installed";
pub const INSTALL_HINT: &str = "Install cloud-sessions to see cloud sessions here.";
const LOGIN_RETRY: Duration = Duration::from_secs(30);
type Runner = Box<dyn Fn(&[String], Option<&str>, Duration) -> Result<String> + Send + Sync>;
type Spawner = Box<dyn Fn(&str, &[String]) -> Result<String> + Send + Sync>;
pub struct LiveRoom {
    ctx: Ctx,
    host: Option<String>,
    /// The other machines listed beside this one (each over ssh -n); empty: this machine (or `host`) only.
    hosts: Vec<String>,
    /// This machine's label in the host column.
    local: String,
    login: Mutex<(Option<Client>, Option<Instant>)>,
    /// Runs `cloud-sessions <args>` with optional stdin and a time limit; `Err(ABSENT)` when it is not installed.
    cloud: Runner,
    /// Runs a program with the terminal handed over; returns once it exits.
    inherit: Spawner,
    cloud_max: usize,
    /// Read the account's sessions (`list --unofficial`), not only the ones tracked on this machine.
    cloud_all: bool,
    /// Routines shown (the `R` key, `--routines`, `REMOTE_SESSIONS_ROUTINES=1`). Off: no `routines` call at all.
    routines: AtomicBool,
    /// Panels beyond Remote Control and Cloud (none in the public build).
    extra: Vec<Box<dyn RoomPanel>>,
}

impl LiveRoom {
    pub fn new(ctx: Ctx, host: Option<String>) -> LiveRoom {
        let cloud_max = (ctx.env)("REMOTE_SESSIONS_CLOUD_MAX").and_then(|v| v.parse().ok()).filter(|n| (1..=50).contains(n)).unwrap_or(DEFAULT_CLOUD_MAX);
        let cloud_all = (ctx.env)("REMOTE_SESSIONS_CLOUD_ALL").is_some_and(|v| v.trim() == "1");
        let routines = AtomicBool::new((ctx.env)("REMOTE_SESSIONS_ROUTINES").is_some_and(|v| v.trim() == "1"));
        LiveRoom { ctx, host, hosts: vec![], local: String::new(), login: Mutex::new((None, None)), cloud: Box::new(run_cloud), inherit: Box::new(run_inherit), cloud_max, cloud_all, routines, extra: linked_panels() }
    }
    /// List these machines too (over ssh -n), beside this one, which is labeled `local`.
    pub fn with_hosts(mut self, hosts: Vec<String>, local: String) -> LiveRoom {
        self.hosts = hosts;
        self.local = if local.is_empty() { "local".into() } else { local };
        self
    }
    /// Replace the extra panels (tests, and programs that embed the room).
    pub fn with_panels(mut self, panels: Vec<Box<dyn RoomPanel>>) -> LiveRoom {
        self.extra = panels;
        self
    }
    fn panel_cx(&self) -> PanelCx<'_> {
        PanelCx { env: &*self.ctx.env, host: self.host.as_deref() }
    }
    /// `--all`: read the account's sessions too (also `REMOTE_SESSIONS_CLOUD_ALL=1`).
    pub fn with_all(mut self, all: bool) -> LiveRoom {
        self.cloud_all |= all;
        self
    }
    /// `--routines`: start with Routines shown (also `REMOTE_SESSIONS_ROUTINES=1`).
    pub fn with_routines(self, on: bool) -> LiveRoom {
        if on {
            self.routines.store(true, Ordering::Relaxed);
        }
        self
    }
    pub fn with_runners(mut self, cloud: Runner, inherit: Spawner) -> LiveRoom {
        self.cloud = cloud;
        self.inherit = inherit;
        self
    }

    fn client(&self) -> Option<Client> {
        let mut g = self.login.lock().ok()?;
        if let Some(c) = &g.0 {
            return Some(c.clone());
        }
        if g.1.is_some_and(|t| t.elapsed() < LOGIN_RETRY) {
            return None;
        }
        g.1 = Some(Instant::now());
        let c = (self.ctx.token)().ok().and_then(|t| Client::new(self.ctx.api.as_deref(), &t, None).ok());
        g.0 = c.clone();
        c
    }
    fn forget_login(&self) {
        if let Ok(mut g) = self.login.lock() {
            g.0 = None;
        }
    }
    fn cloud_json(&self, args: &[&str]) -> Result<Value> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let text = (self.cloud)(&args, None, Duration::from_secs(10))?;
        let v: Value = serde_json::from_str(text.trim()).map_err(|_| "cloud-sessions gave no JSON".to_string())?;
        if v["ok"].as_bool() == Some(false) {
            return Err(v["error"].as_str().unwrap_or("cloud-sessions failed").chars().take(80).collect());
        }
        Ok(v)
    }

    /// Title and repo of a tracked cloud session (one `cloud-sessions list`), for the drop-in header. Untrusted text.
    fn cloud_meta(&self, id: &str) -> (String, String) {
        let Ok(v) = self.cloud_json(&["list"]) else { return (String::new(), String::new()) };
        let row = v["tracked"].as_array().and_then(|a| a.iter().find(|t| t["session_id"].as_str() == Some(id)).cloned()).unwrap_or(Value::Null);
        let clean = |s: &str| sessions_tui::text::clean(s, 200);
        (clean(row["title"].as_str().unwrap_or("")), clean(row["repo"].as_str().unwrap_or("")))
    }

    /// The real conversation of a session, oldest first: (kind, cleaned text) for user and assistant turns only.
    /// Read from /v1/code/sessions/<id>/events through the Client (the same source Remote Control `attach` reads),
    /// never from `cloud-sessions events`, which carries only placeholder "· event" metadata.
    fn cloud_turns(&self, id: &str) -> Result<Vec<(String, String)>> {
        let client = self.client().ok_or("no claude.ai login to read this session (sign in to Claude Code here, or set REMOTE_SESSIONS_TOKEN)")?;
        let body = client.events_raw(id)?;
        Ok(remote_sessions_core::events::parse_events(&body)
            .into_iter()
            .filter(|e| e.kind == "user" || e.kind == "assistant")
            .map(|e| (e.kind.clone(), remote_sessions_core::events::clean(&e.text, 8 * 1024).trim().to_string()))
            .filter(|(_, t)| !t.is_empty())
            .collect())
    }

    /// Drop into a cloud session: a full-screen, two-way mini terminal. The room has already given the terminal away
    /// (suspend), so this takes it in raw mode on the alternate screen, and hands it back on Esc or Ctrl-C only.
    /// Network calls run on background threads: it draws at once, refreshes every 3 s, keeps what you are typing, and sends a line as a real
    /// follow-up turn. Ctrl-C is a key here, not a signal, so it can never take the room or the pane down.
    fn cloud_read(&self, id: &str) -> Result<String> {
        if !valid_id(id) {
            return Err("not a valid session id".into());
        }
        let src = DropSource {
            label: "inside a cloud session".into(),
            info: format!("{id}  \u{00b7}  two-way  \u{00b7}  live, refreshed every 3 s"),
            name: "claude".into(),
            every: Duration::from_secs(3),
            meta: &|| self.cloud_meta(id),
            turns: &|| self.cloud_turns(id),
            send: &|msg| self.cloud_send(id, msg),
            echo: true,
        };
        self.drop_loop(&src)
    }

    /// The full-screen two-way view shared by cloud sessions and the extra panels. The room has already given the terminal
    /// away (suspend), so this takes it in raw mode on the alternate screen, and hands it back on Esc or Ctrl-C only.
    fn drop_loop(&self, src: &DropSource) -> Result<String> {
        use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers};
        use crossterm::{cursor, execute, terminal};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        const BACK: &str = "back in the control room";
        // Non-interactive (tests, pipes): read once and return, never block on the keyboard.
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            let _ = (src.meta)();
            let _ = (src.turns)();
            return Ok(BACK.into());
        }
        enum Upd {
            Meta(String, String),
            Turns(Result<Vec<(String, String)>>),
            Sent(Result<String>),
        }
        let mut out = std::io::stdout();
        terminal::enable_raw_mode().map_err(|e| e.to_string())?;
        let _ = execute!(out, terminal::EnterAlternateScreen, EnableBracketedPaste, cursor::Show);
        let stop = AtomicBool::new(false);
        let result: Result<String> = std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel::<Upd>();
            let (kick_tx, kick_rx) = mpsc::channel::<()>();
            // Every network call lives off the screen thread: the view draws at once and never waits on the network.
            {
                let (tx, stop) = (tx.clone(), &stop);
                scope.spawn(move || {
                    let (t, r) = (src.meta)();
                    let _ = tx.send(Upd::Meta(t, r));
                    while !stop.load(Ordering::Relaxed) {
                        if tx.send(Upd::Turns((src.turns)())).is_err() {
                            break;
                        }
                        // Next read after the refresh interval, or at once when a send asks for it; wake often to notice `stop`.
                        let mut waited = Duration::ZERO;
                        while waited < src.every && !stop.load(Ordering::Relaxed) {
                            if kick_rx.recv_timeout(Duration::from_millis(150)).is_ok() {
                                break;
                            }
                            waited += Duration::from_millis(150);
                        }
                    }
                });
            }
            let mut v = DropView { label: src.label.clone(), info: src.info.clone(), name: src.name.clone(), title: String::new(), repo: String::new(), turns: Vec::new(), input: String::new(), note: String::new(), loaded: false };
            let mut dirty = true;
            // The logo image (None: the text mark). Written on the first frame and after a resize, not on every frame.
            let mut logo_fresh = true;
            let logo = {
                crate::logo::Logo::load(&*self.ctx.env, crate::logo::terminal_cell_px())
            };
            let res: Result<String> = loop {
                while let Ok(u) = rx.try_recv() {
                    dirty = true;
                    match u {
                        Upd::Meta(t, r) => {
                            v.title = t;
                            v.repo = r;
                        }
                        Upd::Turns(Ok(t)) => {
                            // Keep an optimistic "you" line until the server echoes it back.
                            let pending = v.turns.len() > t.len() && v.turns.last().map(|x| x.0 == "user").unwrap_or(false);
                            if !pending || t.last().map(|x| x.0 == "user").unwrap_or(false) {
                                v.turns = t;
                            }
                            v.loaded = true;
                        }
                        Upd::Turns(Err(e)) => v.note = format!("could not read: {e}"),
                        Upd::Sent(Ok(_)) => v.note = "sent".into(),
                        Upd::Sent(Err(e)) => v.note = format!("send failed: {e}"),
                    }
                }
                if dirty {
                    draw_drop(&v, &mut out, logo.as_ref().map(|l| LogoDraw { logo: l, clear: logo_fresh, send: logo_fresh }));
                    logo_fresh = false;
                    dirty = false;
                }
                match event::poll(Duration::from_millis(60)) {
                    Ok(false) => {}
                    Err(_) => break Ok(BACK.into()),
                    Ok(true) => match event::read() {
                        Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => {
                            dirty = true;
                            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                            match k.code {
                                // Only Esc or Ctrl-C leave. Enter never exits the session.
                                KeyCode::Esc => break Ok(BACK.into()),
                                KeyCode::Char('c') if ctrl => break Ok(BACK.into()),
                                KeyCode::Char('u') if ctrl => v.input.clear(),
                                KeyCode::Enter => {
                                    let msg = v.input.trim().to_string();
                                    if msg.is_empty() {
                                        v.note = "type a message first \u{00b7} esc goes back to the room".into();
                                        continue;
                                    }
                                    if src.echo {
                                        v.turns.push(("user".into(), sessions_tui::text::clean(&msg, 8 * 1024)));
                                    }
                                    v.note = "sending\u{2026}".into();
                                    v.input.clear();
                                    let (tx, kick) = (tx.clone(), kick_tx.clone());
                                    scope.spawn(move || {
                                        let r = (src.send)(&msg);
                                        let _ = tx.send(Upd::Sent(r));
                                        let _ = kick.send(());
                                    });
                                }
                                KeyCode::Backspace => {
                                    v.input.pop();
                                }
                                KeyCode::Char(c) if !ctrl => {
                                    if v.input.chars().count() < 20_000 {
                                        v.input.push(c);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Ok(Event::Paste(s)) => {
                            v.input.push_str(&s.replace(['\r', '\n'], " "));
                            dirty = true;
                        }
                        Ok(Event::Resize(..)) => {
                            dirty = true;
                            logo_fresh = true;
                        }
                        _ => {}
                    },
                }
            };
            stop.store(true, Ordering::Relaxed);
            res
        });
        let _ = execute!(out, DisableBracketedPaste, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
        result
    }
}

/// State of the drop-in view.
struct DropView {
    /// For example "inside a cloud session".
    label: String,
    /// The dim third header line.
    info: String,
    /// Who the assistant turns are shown as.
    name: String,
    title: String,
    repo: String,
    turns: Vec<(String, String)>,
    input: String,
    note: String,
    loaded: bool,
}

/// Cut `s` to at most `n` display columns, with an ellipsis when cut.
fn fit(s: &str, n: usize) -> String {
    if sessions_tui::text::width(s) <= n {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if sessions_tui::text::width(&out) + 2 > n {
            break;
        }
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

/// Draw the drop-in screen. Our own look (not Claude Code's): a teal frame mark with two amber "eyes" (two-way), the
/// product line, the session, a full-width rule; the conversation, newest at the bottom; a full-width input box.
fn draw_drop(v: &DropView, out: &mut impl std::io::Write, logo: Option<LogoDraw>) {
    let size = crossterm::terminal::size().map(|(w, h)| (w as usize, h as usize)).unwrap_or((100, 30));
    draw_drop_at(v, out, size, logo);
}

/// The logo image for one drawing: `clear` wipes the screen first (the first frame, a resize); `send` writes the image
/// sequence (only then; between those moments the image stays where it is and is not sent again).
pub struct LogoDraw<'a> {
    pub logo: &'a crate::logo::Logo,
    pub clear: bool,
    pub send: bool,
}

/// [`draw_drop`] for a terminal of `size` columns and rows.
fn draw_drop_at(v: &DropView, out: &mut impl std::io::Write, size: (usize, usize), logo: Option<LogoDraw>) {
    use crossterm::{cursor, queue, style::Print, terminal};
    let (w, h) = size;
    let (w, h) = (w.max(40), h.max(12));
    const GOLD: &str = "\x1b[38;5;214m";
    const TEAL: &str = "\x1b[38;5;37m";
    const DIM: &str = "\x1b[2m";
    const BOLD: &str = "\x1b[1m";
    const RS: &str = "\x1b[0m";
    let rule = "\u{2500}".repeat(w);
    let info = w.saturating_sub(9);
    let place = [v.title.as_str(), v.repo.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("  \u{00b7}  ");
    // Each header line is a text mark (the fallback), two spaces, then its text. With a logo image the mark's cells belong to
    // the image and only the texts are drawn, from column 8.
    let marks = [
        format!("{TEAL} \u{2597}\u{2584}\u{2584}\u{2584}\u{2596}{RS}"),
        format!("{TEAL} \u{2590}{GOLD}\u{25c9} \u{25c9}{TEAL}\u{258c}{RS}"),
        format!("{TEAL} \u{259d}\u{2580}\u{2580}\u{2580}\u{2598}{RS}"),
    ];
    let texts = [
        format!("{BOLD}remote-sessions{RS} {DIM}v{}{RS}  {GOLD}\u{00b7} {}{RS}", env!("CARGO_PKG_VERSION"), v.label),
        fit(if place.is_empty() { "cloud session" } else { &place }, info),
        format!("{DIM}{}{RS}", fit(&v.info, info)),
    ];
    let mut top = vec![
        format!("{}  {}", marks[0], texts[0]),
        format!("{}  {}", marks[1], texts[1]),
        format!("{}  {}", marks[2], texts[2]),
        format!("{TEAL}{DIM}{rule}{RS}"),
    ];
    // Conversation, wrapped, newest at the bottom.
    let mut body: Vec<String> = Vec::new();
    for (kind, text) in &v.turns {
        if kind == "dim" {
            body.push(format!("   {DIM}{}{RS}", fit(&text.replace('\n', " "), w.saturating_sub(5).max(10))));
            continue;
        }
        body.push(String::new());
        if kind == "user" {
            body.push(format!(" {GOLD}\u{203a} you{RS}"));
        } else {
            body.push(format!(" {TEAL}\u{2726} {}{RS}", v.name));
        }
        for line in text.lines() {
            for part in sessions_tui::text::wrap_plain(line, w.saturating_sub(5).max(10)) {
                body.push(format!("   {part}"));
            }
        }
    }
    if !v.loaded {
        body.push(format!("{DIM}   reading the conversation\u{2026}{RS}"));
    } else if v.turns.is_empty() {
        body.push(format!("{DIM}   no messages yet \u{2014} type below to start{RS}"));
    } else if v.turns.last().map(|t| t.0 == "user").unwrap_or(false) {
        body.push(String::new());
        body.push(format!(" {TEAL}{DIM}\u{2726} {} is working\u{2026}{RS}", v.name));
    }
    let body_h = h.saturating_sub(top.len() + 4);
    let start = body.len().saturating_sub(body_h);
    let mut screen: Vec<String> = top.drain(..).collect();
    screen.extend(body[start..].iter().cloned());
    while screen.len() < h - 4 {
        screen.push(String::new());
    }
    // Input box: full-width rules above and below, hints under it.
    let room = w.saturating_sub(4);
    let shown: String = {
        let chars: Vec<char> = v.input.chars().collect();
        let mut s: String = chars.iter().collect();
        while sessions_tui::text::width(&s) > room {
            s.remove(0);
        }
        s
    };
    screen.push(format!("{DIM}{rule}{RS}"));
    screen.push(format!(" {GOLD}\u{203a}{RS} {shown}"));
    screen.push(format!("{DIM}{rule}{RS}"));
    let note = if v.note.is_empty() { String::new() } else { format!("   {GOLD}{}{RS}", fit(&v.note, w / 2)) };
    screen.push(format!(" {DIM}enter send  \u{00b7}  esc back to the control room  \u{00b7}  ctrl-u clear line{RS}{note}"));
    match logo {
        None => {
            let _ = queue!(out, terminal::Clear(terminal::ClearType::All));
            for (row, line) in screen.iter().take(h).enumerate() {
                let _ = queue!(out, cursor::MoveTo(0, row as u16), Print(line));
            }
        }
        Some(d) => {
            // Rewrite each row in place (no full clear), leaving the image's cells alone, so a redraw never wipes the image
            // and the image is sent only when asked.
            if d.clear {
                let _ = queue!(out, terminal::Clear(terminal::ClearType::All));
            }
            for (row, line) in screen.iter().take(h).enumerate() {
                if row < 3 {
                    let _ = queue!(out, cursor::MoveTo(8, row as u16), Print(&texts[row]));
                } else {
                    let _ = queue!(out, cursor::MoveTo(0, row as u16), Print(line));
                }
                let _ = queue!(out, terminal::Clear(terminal::ClearType::UntilNewLine));
            }
            if d.send {
                let _ = queue!(out, cursor::MoveTo(1, 0), Print(&d.logo.sequence));
            }
        }
    }
    let _ = queue!(out, cursor::MoveTo((3 + sessions_tui::text::width(&shown)) as u16, (h - 3) as u16));
    let _ = out.flush();
}

/// A session id is safe to put on a command line.
pub fn valid_id(id: &str) -> bool {
    (8..=100).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The program and arguments that drop into a session. `exe` is this binary.
pub fn drop_in_command(sel: &Sel, exe: &str) -> Result<(String, Vec<String>)> {
    if let Sel::Panel(..) = sel {
        return Err("the rows of an extra panel are opened in-process".into());
    }
    if !valid_id(sel.id()) {
        return Err("that is not a valid session id".into());
    }
    Ok(match sel {
        Sel::Remote(id) => (exe.to_string(), vec!["attach".into(), id.clone(), "--tui".into()]),
        Sel::Cloud(id) => ("cloud-sessions".to_string(), vec!["watch".into(), id.clone()]),
        Sel::Panel(..) => unreachable!("handled above"),
    })
}

fn cloud_bin() -> Option<PathBuf> {
    std::env::var_os("REMOTE_SESSIONS_CLOUD_BIN").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| crate::which("cloud-sessions"))
}

/// Run `cloud-sessions <args>`: stdout on success, [`ABSENT`] when it is not installed, an error when it runs longer
/// than `limit` (it is killed). `stdin`, when given, is written to it and closed.
pub fn run_cloud(args: &[String], stdin: Option<&str>, limit: Duration) -> Result<String> {
    let bin = cloud_bin().ok_or_else(|| ABSENT.to_string())?;
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args);
    run_limited(cmd, stdin, limit, "cloud-sessions")
}

/// Run `cmd` with optional stdin, capture stdout, kill it after `limit`. `name` words the errors.
fn run_limited(mut cmd: std::process::Command, stdin: Option<&str>, limit: Duration, name: &str) -> Result<String> {
    let mut child = cmd
        .stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{name} did not start: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes());
    }
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        s
    });
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} took too long"));
            }
        }
    }
    reader.join().map_err(|_| format!("{name} output unreadable"))
}

/// Run a program with stdin, stdout and stderr inherited; the text is the line shown when the room comes back.
pub fn run_inherit(program: &str, args: &[String]) -> Result<String> {
    let program = if program == "cloud-sessions" { cloud_bin().ok_or_else(|| INSTALL_HINT.to_string())?.to_string_lossy().to_string() } else { program.to_string() };
    let status = std::process::Command::new(&program).args(args).status().map_err(|e| format!("could not start {}: {e}", args.first().map(String::as_str).unwrap_or("it")))?;
    Ok(if status.success() { "back in the control room".into() } else { format!("the session client exited with {status}") })
}

impl RoomBackend for LiveRoom {
    fn remote(&self) -> RemoteSnapshot {
        // This machine, then each other host (--host, REMOTE_SESSIONS_HOSTS, the hosts file), one thread per host.
        // A host that fails or times out becomes its own error row; it never hides the others.
        let several = !self.hosts.is_empty();
        let mut targets: Vec<Option<String>> = vec![if several { None } else { self.host.clone() }];
        targets.extend(self.hosts.iter().cloned().map(Some));
        let mut workers: Vec<(String, rc::Worker)> = Vec::new();
        let mut down: Vec<RemoteRow> = Vec::new();
        for (target, result) in targets.iter().zip(crate::list_hosts(&self.ctx, &targets)) {
            let label = if several { target.clone().unwrap_or_else(|| self.local.clone()) } else { String::new() };
            match result {
                Ok(mut ws) => {
                    ws.sort_by(|a, b| a.session_id.cmp(&b.session_id));
                    workers.extend(ws.into_iter().map(|w| (label.clone(), w)));
                }
                Err(e) if !several => return RemoteSnapshot { hosts: vec![], rows: vec![], status: RemoteStatus::Failed(e) },
                Err(e) => down.push(RemoteRow { host: label.clone(), id: format!("host:{label}"), pid: None, state: RState::HostDown(e), prompts: vec![] }),
            }
        }
        let mut seen = std::collections::HashSet::new();
        workers.retain(|(_, w)| seen.insert(w.session_id.clone()));
        workers.truncate(MAX_REMOTE);
        // Rows in host order (--host, REMOTE_SESSIONS_HOSTS, the hosts file), a failed host's row at its place.
        let order: Vec<String> = targets.iter().map(|t| t.clone().unwrap_or_else(|| self.local.clone())).collect();
        let by_host = |mut rows: Vec<RemoteRow>| {
            if several {
                rows.sort_by_key(|r| order.iter().position(|h| *h == r.host).unwrap_or(usize::MAX));
            }
            rows
        };
        let Some(client) = self.client() else {
            let mut rows: Vec<RemoteRow> = workers.iter().map(|(h, w)| RemoteRow { host: h.clone(), id: w.session_id.clone(), pid: Some(w.pid), state: RState::Unknown, prompts: vec![] }).collect();
            rows.extend(down);
            return RemoteSnapshot { hosts: if several { order.clone() } else { vec![] }, rows: by_host(rows), status: RemoteStatus::NoLogin };
        };
        // Prompts are read by claude.ai session id through the session API: the same for a session on any host.
        let mut rows: Vec<RemoteRow> = std::thread::scope(|scope| {
            let handles: Vec<_> = workers
                .iter()
                .map(|(h, w)| {
                    let client = &client;
                    scope.spawn(move || {
                        let (state, prompts) = match client.pending_prompts(&w.session_id) {
                            Ok(ps) if ps.is_empty() => (RState::Idle, vec![]),
                            Ok(ps) => (RState::Waiting, ps.into_iter().map(|p| PromptInfo { request_id: p.request_id, tool: p.tool_name, input: p.input }).collect()),
                            Err(e) => (RState::Unreadable(e), vec![]),
                        };
                        RemoteRow { host: h.clone(), id: w.session_id.clone(), pid: Some(w.pid), state, prompts }
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        // Every lookup failed: the login is probably stale. Read it afresh next time.
        if !rows.is_empty() && rows.iter().all(|r| matches!(r.state, RState::Unreadable(_))) {
            self.forget_login();
        }
        rows.extend(down);
        RemoteSnapshot { hosts: if several { order.clone() } else { vec![] }, rows: by_host(rows), status: RemoteStatus::Ready }
    }

    fn cloud(&self) -> CloudSnapshot {
        // One `list` call (or `list --unofficial`, falling back to `list`), then one `routines` call. The argv is fixed.
        let mut notes: Vec<CloudRow> = Vec::new();
        let account = if self.cloud_all {
            match self.cloud_json(&["list", "--unofficial"]) {
                Ok(v) if v["remote"]["data"].is_array() => Some(v),
                Err(e) if e == ABSENT => return CloudSnapshot { rows: vec![], status: CloudStatus::Absent },
                _ => {
                    notes.push(note_row("account", "account list unavailable: showing only sessions tracked on this machine"));
                    None
                }
            }
        } else {
            None
        };
        let account_ok = account.is_some();
        let listing = match account {
            Some(v) => v,
            None => match self.cloud_json(&["list"]) {
                Err(e) if e == ABSENT => return CloudSnapshot { rows: vec![], status: CloudStatus::Absent },
                Err(e) => return CloudSnapshot { rows: vec![], status: CloudStatus::Failed(e) },
                Ok(v) => v,
            },
        };
        let mut sessions = list_rows(&listing);
        let mut total = if account_ok { sessions.len() } else { listing["tracked"].as_array().map_or(0, Vec::len) };
        let mut rows = notes;
        // Off (the default): no `routines` call, no rows, no note. Costs nothing.
        if self.routines.load(Ordering::Relaxed) {
            match self.cloud_json(&["routines", "--runs", ROUTINE_RUNS]) {
                Ok(v) => rows.extend(routine_rows(&v)),
                Err(e) if e == ABSENT => {}
                Err(e) => rows.push(note_row("routines", &format!("routines unavailable: {e}"))),
            }
        }
        // A routine's run that is also tracked on this machine is shown once, under its routine (one id, one row).
        let before = sessions.len();
        sessions.retain(|r| !rows.iter().any(|x| x.kind == RowKind::Run && x.id == r.id));
        total = total.saturating_sub(before - sessions.len());
        sessions.truncate(self.cloud_max);
        rows.extend(sessions);
        CloudSnapshot { rows, status: CloudStatus::Ready { tracked: total } }
    }

    fn activity(&self, id: &str) -> CloudActivity {
        let fail = |e: String| CloudActivity { id: id.to_string(), finished: false, last: String::new(), last_at_ms: None, error: Some(e) };
        if !valid_id(id) {
            return fail("not a valid session id".into());
        }
        match self.cloud_json(&["events", id]) {
            Err(e) => fail(e),
            Ok(v) => {
                let newest = v["events"].as_array().and_then(|evs| evs.iter().max_by_key(|e| e["at"].as_u64().unwrap_or(0)));
                let (mut last, mut at) = (String::new(), None);
                if let Some(e) = newest {
                    at = e["at"].as_u64();
                    let kind = e["kind"].as_str().unwrap_or("");
                    let body = e["text"].as_str().unwrap_or("").trim();
                    last = if body.is_empty() || body == "\u{b7} event" { kind.to_string() } else { format!("{kind}: {body}") };
                }
                CloudActivity { id: id.to_string(), finished: v["finished"].as_bool().unwrap_or(false), last, last_at_ms: at, error: None }
            }
        }
    }

    fn routines_supported(&self) -> bool {
        true
    }
    fn routines_on(&self) -> bool {
        self.routines.load(Ordering::Relaxed)
    }
    fn set_routines(&self, on: bool) {
        self.routines.store(on, Ordering::Relaxed);
    }

    fn answer(&self, session: &str, request_id: &str, allow: bool) -> Result<String> {
        if !valid_id(session) {
            return Err("not a valid session id".into());
        }
        let client = self.client().ok_or("no claude.ai login to answer with")?;
        let decision = if allow { Decision::Allow } else { Decision::Deny(None) };
        client.answer(session, Some(request_id), &decision)?;
        Ok(format!("{} the prompt for {}", if allow { "approved" } else { "denied" }, session))
    }

    fn cloud_send(&self, id: &str, text: &str) -> Result<String> {
        if !valid_id(id) {
            return Err("not a valid session id".into());
        }
        if text.trim().is_empty() || text.chars().count() > 20_000 {
            return Err("the message must be 1-20000 characters".into());
        }
        let args: Vec<String> = vec!["send".into(), id.into(), "--message".into(), "-".into()];
        let out = (self.cloud)(&args, Some(text), Duration::from_secs(30)).map_err(|e| if e == ABSENT { INSTALL_HINT.to_string() } else { e })?;
        let v: Value = serde_json::from_str(out.trim()).map_err(|_| "cloud-sessions gave no JSON".to_string())?;
        if v["ok"].as_bool() == Some(false) {
            return Err(v["error"].as_str().unwrap_or("cloud-sessions failed").chars().take(80).collect());
        }
        Ok(format!("sent to {id}"))
    }

    fn drop_in(&self, target: &Sel) -> Result<String> {
        // Cloud: a readable conversation rendered in-process, with a clean return — never the raw `watch` JSON, and
        // never a shelled command whose Ctrl-C takes the room down with it.
        if let Sel::Cloud(id) = target {
            return self.cloud_read(id);
        }
        // An extra panel's row: the panel drops in through the same two-way view.
        if let Sel::Panel(k, id) = target {
            let panel = self.extra.get(*k).ok_or("that panel is gone")?;
            return panel.drop_in(id, &self.panel_cx(), self);
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?.to_string_lossy().to_string();
        let (program, args) = drop_in_command(target, &exe)?;
        (self.inherit)(&program, &args).map_err(|e| format!("{e}. Run it yourself: {}", drop_in_text(target)))
    }

    fn panels(&self) -> Vec<PanelSpec> {
        self.extra.iter().map(|p| p.spec()).collect()
    }

    fn panel(&self, k: usize) -> PanelSnapshot {
        match self.extra.get(k) {
            Some(p) => p.read(&self.panel_cx()),
            None => PanelSnapshot { rows: vec![], status: PanelStatus::Absent },
        }
    }
}

impl DropHost for LiveRoom {
    fn two_way(&self, src: &DropSource) -> Result<String> {
        self.drop_loop(src)
    }
}

/// The extra panels this build links in. The public build links none.
#[allow(unused_mut)]
pub fn linked_panels() -> Vec<Box<dyn RoomPanel>> {
    let mut panels: Vec<Box<dyn RoomPanel>> = Vec::new();
    panels
}

// ---- cloud rows from the list JSON (first-frame state), account sessions, routines -------------------------------------

/// `cse_<suffix>` and `session_<suffix>` name the same session; the room uses `session_<suffix>`.
pub fn session_form(id: &str) -> Option<String> {
    let suffix = id.strip_prefix("cse_").or_else(|| id.strip_prefix("session_"))?;
    let form = format!("session_{suffix}");
    valid_id(&form).then_some(form)
}

fn to_ms(n: u64) -> u64 {
    if n > 1_000_000_000_000 {
        n
    } else {
        n.saturating_mul(1000)
    }
}

/// RFC 3339 (`2026-11-07T03:05:00Z`, optional fraction, `Z` or `+hh:mm`) as epoch milliseconds.
pub fn iso_ms(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    if b.len() < 20 || !text.is_ascii() || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| text[a..z].parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut i = 19;
    let mut millis = 0i64;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &text[start..i];
        if frac.is_empty() {
            return None;
        }
        millis = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    }
    let offset_min = match &text[i..] {
        "Z" | "z" => 0,
        tz if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) && tz.as_bytes()[3] == b':' => {
            let v = tz[1..3].parse::<i64>().ok()? * 60 + tz[4..6].parse::<i64>().ok()?;
            if tz.starts_with('-') {
                -v
            } else {
                v
            }
        }
        _ => return None,
    };
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss - offset_min * 60;
    u64::try_from(secs * 1000 + millis).ok()
}

fn status_finished(status: &str) -> bool {
    matches!(status, "archived" | "completed" | "complete" | "success" | "succeeded" | "failed" | "error" | "cancelled" | "canceled")
}

fn remote_repo(item: &Value) -> String {
    item.pointer("/config/outcomes")
        .and_then(Value::as_array)
        .and_then(|o| o.iter().find_map(|x| x.pointer("/git_info/repo").and_then(Value::as_str).filter(|r| !r.is_empty())))
        .unwrap_or("")
        .to_string()
}

fn list_row(id: String, title: &str, repo: &str, at: Option<u64>, finished: bool, kind: RowKind) -> CloudRow {
    let mut r = CloudRow::listed(&id, title, repo);
    r.last_at_ms = at;
    r.finished = finished;
    r.from_list = true;
    r.kind = kind;
    r
}

/// The sessions of a `list` / `list --unofficial` reply as rows with their state from the list itself: tracked entries
/// by `last_seen` / `started_at`, account entries (`remote.data`) by `last_event_at` / `updated_at` / `created_at` and
/// `status`. Newest first. Ids that are not safe on a command line are dropped.
pub fn list_rows(listing: &Value) -> Vec<CloudRow> {
    let mut rows: Vec<CloudRow> = Vec::new();
    for t in listing["tracked"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let raw = t["session_id"].as_str().unwrap_or("");
        let Some(id) = session_form(raw).or_else(|| valid_id(raw).then(|| raw.to_string())) else { continue };
        let at = t["last_seen"].as_u64().or_else(|| t["started_at"].as_u64()).map(to_ms);
        rows.push(list_row(id, t["title"].as_str().unwrap_or(""), t["repo"].as_str().unwrap_or(""), at, false, RowKind::Session));
    }
    for item in listing["remote"]["data"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(id) = item["id"].as_str().and_then(session_form) else { continue };
        let created = item["created_at"].as_str().and_then(iso_ms);
        let at = item["last_event_at"].as_str().and_then(iso_ms).or_else(|| item["updated_at"].as_str().and_then(iso_ms)).or(created);
        let finished = status_finished(item["status"].as_str().unwrap_or(""));
        if let Some(r) = rows.iter_mut().find(|r| r.id == id) {
            if r.title.trim().is_empty() {
                r.title = item["title"].as_str().unwrap_or("").to_string();
            }
            if r.repo.is_empty() {
                r.repo = remote_repo(item);
            }
            r.last_at_ms = r.last_at_ms.max(at);
            r.finished = finished;
            continue;
        }
        rows.push(list_row(id, item["title"].as_str().unwrap_or(""), &remote_repo(item), at, finished, RowKind::Session));
    }
    rows.sort_by(|a, b| b.last_at_ms.cmp(&a.last_at_ms).then_with(|| a.id.cmp(&b.id)));
    rows
}

fn note_row(key: &str, text: &str) -> CloudRow {
    let mut r = list_row(format!("note~{key}"), text, "", None, false, RowKind::Note);
    r.title = text.to_string();
    r
}

/// The `routines` reply as group rows: a dim routine row (name, schedule, enabled) with its newest runs under it as
/// normal rows. A routine whose runs could not be read gets a note row; nothing here is an error.
pub fn routine_rows(reply: &Value) -> Vec<CloudRow> {
    let mut out = Vec::new();
    for r in reply["routines"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let rid = r["id"].as_str().unwrap_or("");
        if !valid_slug(rid) {
            continue;
        }
        let name = r["name"].as_str().unwrap_or("").to_string();
        let schedule = if r["schedule"]["kind"].as_str() == Some("cron") {
            format!("cron {}", r["schedule"]["cron"].as_str().unwrap_or("?"))
        } else {
            match r["schedule"]["run_once_at"].as_str() {
                Some(at) => format!("once {at}"),
                None => "once".to_string(),
            }
        };
        out.push(list_row(format!("routine~{rid}"), &name, "", None, false, RowKind::Routine { schedule, enabled: r["enabled"].as_bool().unwrap_or(false) }));
        if let Some(e) = r["runs_error"].as_str() {
            out.push(note_row(&format!("runs~{rid}"), &format!("  runs unavailable: {e}")));
        }
        for run in r["runs"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let Some(id) = run["id"].as_str().and_then(session_form) else { continue };
            let at = run["last_event_at"].as_str().and_then(iso_ms).or_else(|| run["created_at"].as_str().and_then(iso_ms));
            out.push(list_row(id, run["title"].as_str().unwrap_or(""), "", at, status_finished(run["status"].as_str().unwrap_or("")), RowKind::Run));
        }
    }
    out
}

/// A routine id: letters, digits, underscore, dash.
pub fn valid_slug(s: &str) -> bool {
    (1..=100).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ---- placeholder data (the showcase and the tests) ----------------------------------------------------------------

/// A fixed "now" for the placeholder room, so its frame is the same every time.
pub const DEMO_NOW_MS: u64 = 1_800_000_000_000;
pub const DEMO_A: &str = "session_01ExampleAAAAAAAAAAAAAAA";
pub const DEMO_B: &str = "session_01ExampleBBBBBBBBBBBBBBB";
pub const DEMO_C: &str = "session_01ExampleCCCCCCCCCCCCCCC";
pub const DEMO_D: &str = "session_01ExampleDDDDDDDDDDDDDDD";
pub const DEMO_E: &str = "session_01ExampleEEEEEEEEEEEEEEE";
pub const DEMO_F: &str = "session_01ExampleFFFFFFFFFFFFFFF";

/// Placeholder sessions only (fake ids, neutral repos). Nothing is read, sent or run.
pub struct DemoRoom;

fn demo_cloud(id: &str, title: &str, repo: &str, last: &str, ago_ms: u64, finished: bool) -> CloudRow {
    CloudRow { id: id.into(), title: title.into(), repo: repo.into(), last: last.into(), last_at_ms: Some(DEMO_NOW_MS - ago_ms), finished, error: None, fetched_ms: Some(DEMO_NOW_MS), from_list: false, kind: RowKind::Session }
}

impl RoomBackend for DemoRoom {
    fn remote(&self) -> RemoteSnapshot {
        let waiting = |id: &str, pid: u32, tool: &str, input: Value| RemoteRow { host: String::new(), id: id.into(), pid: Some(pid), state: RState::Waiting, prompts: vec![PromptInfo { request_id: format!("req_{pid}"), tool: tool.into(), input }] };
        RemoteSnapshot {
            hosts: vec![],
            rows: vec![
                waiting(DEMO_A, 4242, "Edit", serde_json::json!({"file_path": "src/parser.rs"})),
                RemoteRow { host: String::new(), id: DEMO_B.into(), pid: Some(4243), state: RState::Idle, prompts: vec![] },
                waiting(DEMO_C, 4244, "Bash", serde_json::json!({"command": "cargo test --workspace"})),
            ],
            status: RemoteStatus::Ready,
        }
    }
    fn cloud(&self) -> CloudSnapshot {
        CloudSnapshot {
            rows: vec![
                demo_cloud(DEMO_D, "Fix the flaky parser test", "your-org/your-repo", "assistant: Reading the failing test", 20_000, false),
                demo_cloud(DEMO_E, "Add a docs page for the CLI", "your-org/docs", "result: Opened a pull request", 7_200_000, true),
                demo_cloud(DEMO_F, "Migrate config to the new schema", "your-org/your-repo", "tool: Edit config.rs", 840_000, false),
            ],
            status: CloudStatus::Ready { tracked: 3 },
        }
    }
    fn activity(&self, id: &str) -> CloudActivity {
        let c = self.cloud().rows.into_iter().find(|c| c.id == id);
        match c {
            Some(c) => CloudActivity { id: c.id, finished: c.finished, last: c.last, last_at_ms: c.last_at_ms, error: None },
            None => CloudActivity { id: id.into(), finished: false, last: String::new(), last_at_ms: None, error: Some("unknown".into()) },
        }
    }
    fn answer(&self, _: &str, _: &str, allow: bool) -> Result<String> {
        Ok(format!("demo: nothing was {}", if allow { "approved" } else { "denied" }))
    }
    fn cloud_send(&self, _: &str, _: &str) -> Result<String> {
        Ok("demo: nothing was sent".into())
    }
    fn drop_in(&self, target: &Sel) -> Result<String> {
        Err(format!("demo: would run {}", drop_in_text(target)))
    }
}

// ---- one frame ----------------------------------------------------------------------------------------------------

/// Read each panel once (an extra panel is drawn only when its source answers) and draw one frame as text (`control --once`). `select` picks the highlighted row (the first
/// one when None). At most one cloud activity read happens, for the selected row, as in the interactive room.
pub fn render_once(backend: &dyn RoomBackend, host: &str, demo: bool, size: (u16, u16), th: &sessions_tui::theme::Theme, ansi: bool, now_ms: u64, select: Option<Sel>) -> String {
    let specs = backend.panels();
    let n = specs.len();
    let mut s = RoomState::new(host).with_panels(specs);
    s.demo = demo;
    s.air = th.air();
    s.routines_available = backend.routines_supported();
    s.routines = backend.routines_on();
    s.size = size;
    let mut pending: Vec<RoomMsg> = vec![RoomMsg::Tick(now_ms), RoomMsg::Remote(backend.remote()), RoomMsg::Cloud(backend.cloud())];
    pending.extend((0..n).map(|k| RoomMsg::Panel(k, backend.panel(k))));
    let mut fetched = false;
    while !pending.is_empty() {
        let m = pending.remove(0);
        let (next, effects) = reduce(s, m);
        s = next;
        for e in effects {
            if let RoomEffect::FetchActivity(id) = e {
                if !fetched {
                    fetched = true;
                    pending.push(RoomMsg::Activity(backend.activity(&id)));
                }
            }
        }
    }
    if let Some(sel) = select {
        s.sel = Some(sel);
    }
    render_text(&s, th, size.0, size.1, ansi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn drop_in_commands_are_exact_and_refuse_odd_ids() {
        let r = drop_in_command(&Sel::Remote(DEMO_A.into()), "/bin/remote-sessions").unwrap();
        assert_eq!(r, ("/bin/remote-sessions".to_string(), vec!["attach".to_string(), DEMO_A.to_string(), "--tui".to_string()]));
        let c = drop_in_command(&Sel::Cloud(DEMO_D.into()), "x").unwrap();
        assert_eq!(c, ("cloud-sessions".to_string(), vec!["watch".to_string(), DEMO_D.to_string()]));
        assert!(drop_in_command(&Sel::Cloud("a b; rm -rf".into()), "x").is_err());
        assert!(drop_in_command(&Sel::Remote("short".into()), "x").is_err());
    }

    fn live(cloud: Runner) -> (LiveRoom, Arc<Mutex<Vec<(String, Vec<String>)>>>) {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let r2 = ran.clone();
        let mut ctx = crate::ctx_from_env();
        ctx.env = Box::new(|_| None);
        ctx.token = Box::new(|| Err("no token in tests".into()));
        ctx.ps = Box::new(|_| Ok("  65579 /opt/claude/versions/2.1.318 --print --sdk-url https://api.anthropic.com/v1/code/sessions/cse_01ExampleCCCCCCCCCCCCCCC\n".into()));
        let inherit: Spawner = Box::new(move |p, a| {
            r2.lock().unwrap().push((p.to_string(), a.to_vec()));
            Ok("back in the control room".into())
        });
        (LiveRoom::new(ctx, None).with_runners(cloud, inherit), ran)
    }

    fn canned(table: Vec<(&'static str, String)>) -> Runner {
        Box::new(move |args, _, _| table.iter().find(|(k, _)| *k == args.join(" ")).map(|(_, v)| v.clone()).ok_or_else(|| ABSENT.to_string()))
    }

    #[test]
    fn remote_without_a_login_lists_the_workers_as_unknown() {
        let (room, _) = live(canned(vec![]));
        let snap = room.remote();
        assert_eq!(snap.status, RemoteStatus::NoLogin);
        assert_eq!((snap.rows.len(), snap.rows[0].id.as_str(), snap.rows[0].pid, &snap.rows[0].state), (1, DEMO_C, Some(65579), &RState::Unknown));
    }

    #[test]
    fn cloud_absent_is_a_status_not_an_error() {
        let (room, _) = live(canned(vec![]));
        assert_eq!(room.cloud(), CloudSnapshot { rows: vec![], status: CloudStatus::Absent });
    }

    #[test]
    fn cloud_list_is_one_call_newest_first_and_capped() {
        let listing = serde_json::json!({"ok": true, "tracked": [
            {"session_id": DEMO_D, "title": "Older", "repo": "your-org/a", "started_at": 100, "last_seen": null},
            {"session_id": DEMO_E, "title": "Newer", "repo": "your-org/b", "started_at": 200, "last_seen": null},
            {"session_id": "bad id!", "title": "Odd", "repo": "x", "started_at": 300, "last_seen": null},
        ]})
        .to_string();
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let c2 = calls.clone();
        let runner: Runner = Box::new(move |args, _, _| {
            c2.lock().unwrap().push(args.join(" "));
            Ok(listing.clone())
        });
        let (room, _) = live(runner);
        let snap = room.cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list".to_string()], "routines are off by default: one listing, no routines call, no per-session reads");
        assert_eq!(snap.status, CloudStatus::Ready { tracked: 3 });
        assert_eq!(snap.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![DEMO_E, DEMO_D], "newest first; the unsafe id is dropped");
    }

    #[test]
    fn activity_reads_the_newest_event_and_reports_failures() {
        let ev = serde_json::json!({"ok": true, "finished": false, "events": [
            {"at": 1000, "kind": "tool", "text": "older"}, {"at": 9000, "kind": "assistant", "text": "Reading the failing test"}]})
        .to_string();
        let key: &'static str = Box::leak(format!("events {DEMO_D}").into_boxed_str());
        let (room, _) = live(canned(vec![(key, ev)]));
        let a = room.activity(DEMO_D);
        assert_eq!((a.last.as_str(), a.last_at_ms, a.finished, a.error.is_none()), ("assistant: Reading the failing test", Some(9000), false, true));
        assert!(room.activity(DEMO_E).error.is_some());
        assert!(room.activity("bad id").error.is_some());
    }

    #[test]
    fn cloud_send_passes_the_message_on_stdin_not_argv() {
        let seen = Arc::new(Mutex::new(Vec::<(Vec<String>, Option<String>)>::new()));
        let s2 = seen.clone();
        let runner: Runner = Box::new(move |args, stdin, _| {
            s2.lock().unwrap().push((args.to_vec(), stdin.map(str::to_string)));
            Ok(r#"{"ok":true}"#.into())
        });
        let (room, _) = live(runner);
        assert_eq!(room.cloud_send(DEMO_D, "please also run the lints").unwrap(), format!("sent to {DEMO_D}"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0, vec!["send".to_string(), DEMO_D.to_string(), "--message".to_string(), "-".to_string()]);
        assert_eq!(seen[0].1.as_deref(), Some("please also run the lints"));
        assert!(!seen[0].0.iter().any(|a| a.contains("lints")));
        drop(seen);
        assert!(room.cloud_send("bad id", "x").is_err());
        assert!(room.cloud_send(DEMO_D, "   ").is_err());
    }

    #[test]
    fn cloud_drop_in_reads_the_conversation_not_raw_watch() {
        let ev = serde_json::json!({"ok": true, "finished": false, "events": [
            {"at": 1000, "kind": "user", "text": "fix the parser"},
            {"at": 2000, "kind": "assistant", "text": "Reading the failing test"}]})
        .to_string();
        let key: &'static str = Box::leak(format!("events {DEMO_D}").into_boxed_str());
        let (room, ran) = live(canned(vec![(key, ev)]));
        // Tests run with stdin not a terminal: the reader renders once and returns. No shell-out happens.
        assert_eq!(room.drop_in(&Sel::Cloud(DEMO_D.into())).unwrap(), "back in the control room");
        assert!(ran.lock().unwrap().is_empty(), "cloud drop-in must NOT shell out to `cloud-sessions watch`");
    }

    #[test]
    fn remote_drop_in_runs_attach_with_the_terminal() {
        let (room, ran) = live(canned(vec![]));
        assert_eq!(room.drop_in(&Sel::Remote(DEMO_A.into())).unwrap(), "back in the control room");
        let ran = ran.lock().unwrap();
        assert_eq!(ran[0].1, vec!["attach".to_string(), DEMO_A.to_string(), "--tui".to_string()], "remote still attaches with the terminal");
    }

    fn calls_runner(reply: impl Fn(&str) -> Result<String> + Send + Sync + 'static) -> (Runner, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let c2 = calls.clone();
        let r: Runner = Box::new(move |args, _, _| {
            let key = args.join(" ");
            c2.lock().unwrap().push(key.clone());
            reply(&key)
        });
        (r, calls)
    }

    const ROUTINE_ID_1: &str = "session_01ExampleRunAAAAAAAAAAAA";
    const ROUTINE_ID_2: &str = "cse_01ExampleRunBBBBBBBBBBBBBB";
    fn routines_json() -> String {
        serde_json::json!({"ok": true, "unofficial": "x", "routines": [
            {"id": "trig_1", "name": "Nightly", "enabled": true, "schedule": {"kind": "cron", "cron": "0 3 * * *", "run_once_at": null},
             "runs": [
                {"id": ROUTINE_ID_1, "title": "Nightly run", "status": "success", "created_at": "2026-11-07T03:00:00Z", "last_event_at": "2026-11-07T03:05:00Z"},
                {"id": ROUTINE_ID_2, "title": "Older run", "status": "running", "created_at": "2026-11-06T03:00:00Z", "last_event_at": null},
                {"id": "weird", "title": "unsafe id", "status": "running"}]},
            {"id": "trig_2", "name": "Broken", "enabled": false, "schedule": {"kind": "once", "cron": null, "run_once_at": "2026-12-01T00:00:00Z"}, "runs": [], "runs_error": "HTTP 500"}
        ]})
        .to_string()
    }
    fn tracked_json(n: usize) -> String {
        let tracked: Vec<Value> = (0..n)
            .map(|i| serde_json::json!({"session_id": format!("session_01Example{i:03}AAAAAAAAAAAA"), "title": format!("Session {i}"), "repo": "your-org/repo", "started_at": 1_793_865_913u64 + i as u64 * 1000, "last_seen": null}))
            .collect();
        serde_json::json!({"ok": true, "tracked": tracked}).to_string()
    }

    #[test]
    fn iso_ms_and_session_form() {
        assert_eq!(iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_ms("2026-11-07T03:05:00Z"), Some(1_794_020_700_000));
        assert_eq!(iso_ms("2026-11-07T03:05:00.250Z"), Some(1_794_020_700_250));
        assert_eq!(iso_ms("2026-11-07T04:05:00+01:00"), Some(1_794_020_700_000));
        assert_eq!(iso_ms("2026-11-07T02:05:00-01:00"), Some(1_794_020_700_000));
        for bad in ["", "not a date", "2026-13-07T03:05:00Z", "2026-11-07T25:05:00Z", "2026-11-07T03:05:00", "2026-11-07T03:05:00+0100"] {
            assert_eq!(iso_ms(bad), None, "{bad:?}");
        }
        assert_eq!(session_form("cse_01Example1234"), Some("session_01Example1234".to_string()));
        assert_eq!(session_form("session_01Example1234"), Some("session_01Example1234".to_string()));
        assert_eq!(session_form("weird"), None);
        assert_eq!(session_form("cse_a b"), None);
    }

    #[test]
    fn the_account_list_merges_with_the_tracked_one_and_every_row_has_its_state() {
        let listing = serde_json::json!({"ok": true,
            "tracked": [
                {"session_id": DEMO_D, "title": "Tracked title", "repo": "", "started_at": 1_793_865_913u64, "last_seen": null},
                {"session_id": DEMO_E, "title": "Old", "repo": "your-org/old", "started_at": 1000, "last_seen": 2000},
                {"session_id": "bad id!", "title": "Odd", "repo": "x", "started_at": 3000, "last_seen": null}],
            "remote": {"data": [
                {"id": "cse_01ExampleDDDDDDDDDDDDDDD", "title": "Remote title", "status": "active", "last_event_at": "2026-11-07T07:20:00Z", "config": {"outcomes": [{"git_info": {"repo": "your-org/your-repo"}}]}},
                {"id": "cse_01ExampleOTHERMAC", "title": "Other Mac", "status": "archived", "last_event_at": null, "updated_at": "2026-11-06T02:00:00Z"},
                {"id": "weird", "title": "x"}], "next_cursor": "abc"}});
        let rows = list_rows(&listing);
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![DEMO_D, "session_01ExampleOTHERMAC", DEMO_E], "newest first; unusable ids dropped");
        let d = &rows[0];
        assert_eq!((d.title.as_str(), d.repo.as_str(), d.last_at_ms, d.finished), ("Tracked title", "your-org/your-repo", Some(1_794_036_000_000), false), "the tracked title stays; the account list adds the repo and the activity");
        let other = &rows[1];
        assert_eq!((other.title.as_str(), other.finished, other.last_at_ms), ("Other Mac", true, Some(1_793_930_400_000)));
        assert!(rows.iter().all(|r| r.from_list && r.kind == RowKind::Session));
        assert_eq!(rows[2].last_at_ms, Some(2_000_000), "last_seen seconds become milliseconds");
    }

    #[test]
    fn all_asks_for_the_account_list_and_falls_back_to_tracked_with_a_note() {
        let (runner, calls) = calls_runner(|k| match k {
            "list --unofficial" => Err("HTTP 500".into()),
            "list" => Ok(tracked_json(2)),
            _ => Err("Unknown command routines".into()),
        });
        let (room, _) = live(runner);
        let snap = room.with_all(true).with_routines(true).cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list --unofficial", "list", "routines --runs 3"]);
        assert_eq!(snap.status, CloudStatus::Ready { tracked: 2 });
        let notes: Vec<&str> = snap.rows.iter().filter(|r| r.kind == RowKind::Note).map(|r| r.title.as_str()).collect();
        assert_eq!(notes, vec!["account list unavailable: showing only sessions tracked on this machine", "routines unavailable: Unknown command routines"], "a note each, never an error panel");
        assert_eq!(snap.rows.iter().filter(|r| r.kind == RowKind::Session).count(), 2);
        // the account list answering is one call, no fallback
        let (runner, calls) = calls_runner(|k| match k {
            "list --unofficial" => Ok(serde_json::json!({"ok": true, "tracked": [], "remote": {"data": [{"id": "cse_01ExampleDDDDDDDDDDDDDDD", "title": "T", "status": "active", "last_event_at": "2026-11-07T07:20:00Z"}]}}).to_string()),
            _ => Err(ABSENT.to_string()),
        });
        let (room, _) = live(runner);
        let snap = room.with_all(true).with_routines(true).cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list --unofficial", "routines --runs 3"]);
        assert_eq!((snap.rows.len(), &snap.status), (1, &CloudStatus::Ready { tracked: 1 }), "routines absent: no rows, no note");
    }

    #[test]
    fn routines_become_dim_group_rows_with_their_newest_runs_under_them() {
        let (runner, calls) = calls_runner(|k| match k {
            "list" => Ok(tracked_json(1)),
            "routines --runs 3" => Ok(routines_json()),
            _ => Err(ABSENT.to_string()),
        });
        let (room, _) = live(runner);
        let snap = room.with_routines(true).cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list", "routines --runs 3"], "one list and one routines call per poll");
        let shape: Vec<(String, String)> = snap
            .rows
            .iter()
            .map(|r| (match &r.kind { RowKind::Routine { .. } => "routine", RowKind::Run => "run", RowKind::Note => "note", RowKind::Session => "session" }.to_string(), r.id.clone()))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("routine".to_string(), "routine~trig_1".to_string()),
                ("run".to_string(), ROUTINE_ID_1.to_string()),
                ("run".to_string(), "session_01ExampleRunBBBBBBBBBBBBBB".to_string()),
                ("routine".to_string(), "routine~trig_2".to_string()),
                ("note".to_string(), "note~runs~trig_2".to_string()),
                ("session".to_string(), "session_01Example000AAAAAAAAAAAA".to_string()),
            ],
            "the run with an unsafe id is dropped"
        );
        assert_eq!(snap.rows[0].kind, RowKind::Routine { schedule: "cron 0 3 * * *".into(), enabled: true });
        assert_eq!(snap.rows[3].kind, RowKind::Routine { schedule: "once 2026-12-01T00:00:00Z".into(), enabled: false });
        let (run1, run2) = (&snap.rows[1], &snap.rows[2]);
        assert_eq!((run1.repo.as_str(), run1.finished, run1.last_at_ms), ("", true, Some(1_794_020_700_000)));
        assert_eq!((run2.finished, run2.last_at_ms), (false, Some(1_793_934_000_000)), "no last event: the creation time");
        assert_eq!(snap.rows[4].title, "  runs unavailable: HTTP 500");
    }

    #[test]
    fn a_routine_run_that_is_also_tracked_is_listed_once_under_its_routine() {
        let (runner, _) = calls_runner(|k| match k {
            "list" => Ok(serde_json::json!({"ok": true, "tracked": [
                {"session_id": ROUTINE_ID_1, "title": "Nightly run", "repo": "", "started_at": 1_793_865_913u64, "last_seen": null},
                {"session_id": DEMO_D, "title": "Plain", "repo": "", "started_at": 1_793_865_913u64, "last_seen": null}]})
            .to_string()),
            "routines --runs 3" => Ok(routines_json()),
            _ => Err(ABSENT.to_string()),
        });
        let (room, _) = live(runner);
        let snap = room.with_routines(true).cloud();
        let ids: Vec<&str> = snap.rows.iter().filter(|r| !r.is_group()).map(|r| r.id.as_str()).collect();
        assert_eq!(ids.iter().filter(|i| **i == ROUTINE_ID_1).count(), 1, "{ids:?}");
        assert_eq!(ids.len(), 3, "two runs plus the one plain session: {ids:?}");
        assert_eq!(snap.status, CloudStatus::Ready { tracked: 1 }, "the deduplicated run is not counted as an extra tracked session");
        let mut st = RoomState::new("");
        st.size = (100, 30);
        let (st, _) = reduce(st, RoomMsg::Cloud(snap));
        let order: Vec<String> = st.order().iter().map(|s| s.id().to_string()).collect();
        let unique: std::collections::BTreeSet<&String> = order.iter().collect();
        assert_eq!(order.len(), unique.len(), "no id is listed twice, so a selection lights exactly one row: {order:?}");
    }

    #[test]
    fn the_argv_is_fixed_and_never_carries_remote_text() {
        let hostile = "$(touch x); `id`; \n --flag";
        let listing = serde_json::json!({"ok": true,
            "tracked": [{"session_id": DEMO_D, "title": hostile, "repo": hostile, "started_at": 1, "last_seen": null}],
            "remote": {"data": [{"id": "cse_01ExampleOTHERMAC", "title": hostile, "status": hostile}]}})
        .to_string();
        let routines = serde_json::json!({"ok": true, "routines": [{"id": "trig_1", "name": hostile, "enabled": true, "schedule": {"kind": "cron", "cron": hostile},
            "runs": [{"id": DEMO_F, "title": hostile, "status": hostile, "created_at": hostile}]}]})
        .to_string();
        for all in [false, true] {
            let (l, r) = (listing.clone(), routines.clone());
            let (runner, calls) = calls_runner(move |k| if k.starts_with("list") { Ok(l.clone()) } else { Ok(r.clone()) });
            let (room, _) = live(runner);
            let snap = room.with_all(all).with_routines(true).cloud();
            assert!(snap.rows.len() >= 3, "{:?}", snap.rows);
            for c in calls.lock().unwrap().iter() {
                assert!(["list", "list --unofficial", "routines --runs 3"].contains(&c.as_str()), "unexpected argv {c:?}");
            }
        }
    }

    #[test]
    fn a_41_row_list_renders_no_pending_rows_and_the_default_cap_is_50() {
        assert_eq!(DEFAULT_CLOUD_MAX, 50);
        let (runner, _) = calls_runner(|k| if k == "list" { Ok(tracked_json(41)) } else { Err(ABSENT.to_string()) });
        let (room, _) = live(runner);
        let th = sessions_tui::theme::Theme::detect(true, false);
        let frame = render_once(&room, "h", false, (110, 30), &th, false, 1_794_100_000_000, None);
        assert!(!frame.contains("..."), "no row waits for a read:\n{frame}");
        assert!(frame.contains("idle"), "{frame}");
        // 60 tracked entries: the default cap shows 50 rows and says how many more there are
        let (runner, _) = calls_runner(|k| if k == "list" { Ok(tracked_json(60)) } else { Err(ABSENT.to_string()) });
        let (room, _) = live(runner);
        let snap = room.cloud();
        assert_eq!((snap.rows.len(), snap.status), (50, CloudStatus::Ready { tracked: 60 }));
    }

    fn routines_world() -> (LiveRoom, Arc<Mutex<Vec<String>>>) {
        let (runner, calls) = calls_runner(|k| match k {
            "list" => Ok(tracked_json(2)),
            "routines --runs 3" => Ok(routines_json()),
            _ => Err(ABSENT.to_string()),
        });
        let (room, _) = live(runner);
        (room, calls)
    }

    #[test]
    fn routines_are_off_by_default_no_call_no_rows_and_no_note() {
        let (room, calls) = routines_world();
        assert!(room.routines_supported() && !room.routines_on());
        let snap = room.cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list"], "zero routines calls while off");
        assert!(snap.rows.iter().all(|r| r.kind == RowKind::Session), "no routine rows and no note line: {:?}", snap.rows.iter().map(|r| &r.id).collect::<Vec<_>>());
        // a runner that fails routines would show a note only when routines are on: off, it is never asked
        let (runner, calls) = calls_runner(|k| if k == "list" { Ok(tracked_json(1)) } else { Err("HTTP 500".into()) });
        let (room, _) = live(runner);
        let snap = room.with_all(true).cloud();
        assert!(snap.rows.iter().all(|r| r.kind != RowKind::Routine { schedule: String::new(), enabled: false } && r.kind != RowKind::Run), "{:?}", snap.rows);
        assert!(!calls.lock().unwrap().iter().any(|c| c.starts_with("routines")));
    }

    #[test]
    fn turning_routines_on_makes_exactly_one_routines_call_per_poll_and_off_makes_none() {
        let (room, calls) = routines_world();
        room.set_routines(true);
        assert!(room.routines_on());
        let snap = room.cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list", "routines --runs 3"], "one routines call");
        assert!(snap.rows.iter().any(|r| matches!(r.kind, RowKind::Routine { .. })));
        room.set_routines(false);
        calls.lock().unwrap().clear();
        let snap = room.cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list"]);
        assert!(snap.rows.iter().all(|r| r.kind == RowKind::Session));
        // starting on: the builder and the environment variable
        let (room, calls) = routines_world();
        let _ = room.with_routines(true).cloud();
        assert_eq!(calls.lock().unwrap().len(), 2);
        let (runner, calls) = calls_runner(|k| if k == "list" { Ok(tracked_json(1)) } else { Ok(routines_json()) });
        let mut ctx = crate::ctx_from_env();
        ctx.env = Box::new(|k| (k == "REMOTE_SESSIONS_ROUTINES").then(|| "1".to_string()));
        ctx.ps = Box::new(|_| Ok(String::new()));
        let room = LiveRoom::new(ctx, None).with_runners(runner, Box::new(|_, _| Ok(String::new())));
        assert!(room.routines_on());
        let _ = room.cloud();
        assert_eq!(*calls.lock().unwrap(), vec!["list", "routines --runs 3"]);
    }

    #[test]
    fn routines_read_as_one_plain_header_line_with_plain_session_rows_under_it() {
        let (room, _) = routines_world();
        let snap = room.with_routines(true).cloud();
        let mut st = RoomState::new("");
        st.size = (110, 30);
        st.routines_available = true;
        st.routines = true;
        let (st, _) = reduce(st, RoomMsg::Tick(1_794_100_000_000));
        let (st, _) = reduce(st, RoomMsg::Cloud(snap));
        let th = sessions_tui::theme::Theme::color(); // the unicode glyph set, so a stray glyph would show
        let frame = render_text(&st, &th, 110, 30, false);
        let line = frame.lines().find(|l| l.contains("routine \u{b7} Nightly")).unwrap_or_else(|| panic!("no routine line in\n{frame}"));
        assert!(line.contains("routine \u{b7} Nightly \u{b7} cron 0 3 * * * \u{b7} on"), "{line}");
        assert!(frame.contains("routine \u{b7} Broken \u{b7} once 2026-12-01T00:00:00Z \u{b7} off"), "{frame}");
        for glyph in ['\u{23f8}', '\u{26a1}'] {
            assert!(!frame.contains(glyph), "no {glyph:?} glyph:\n{frame}");
        }
        assert_eq!(frame.matches("routine \u{b7} Nightly").count(), 1, "the routine header appears once:\n{frame}");
        // a run is a normal row: the state glyph and word like any session, and no repo column carrying the routine name
        let run = frame.lines().find(|l| l.contains("Nightly run")).unwrap_or_else(|| panic!("no run row in\n{frame}"));
        assert!(run.contains("finished") && run.contains('\u{2713}'), "{run}");
        assert_eq!(run.matches("Nightly").count(), 1, "the run shows its own title only, not the routine name again: {run}");
        assert!(frame.contains("R routines"), "the footer offers the toggle:\n{frame}");
    }

    const GOLDEN_LEN: usize = 1740;
    const GOLDEN_HASH: u64 = 0x7dd120741b0f7679;

    fn golden_drop_view() -> DropView {
        DropView {
            label: "inside a cloud session".into(),
            info: "session_01ExampleDDDDDDDDDDDDDDD  \u{00b7}  two-way  \u{00b7}  live, refreshed every 3 s".into(),
            name: "claude".into(),
            title: "Fix the flaky parser test".into(),
            repo: "your-org/your-repo".into(),
            turns: vec![("user".into(), "fix the parser".into()), ("assistant".into(), "Reading the failing test\nsecond line".into()), ("dim".into(), "tool: Edit src/parser.rs".into())],
            input: "typing".into(),
            note: "sent".into(),
            loaded: true,
        }
    }
    /// The pinned bytes were recorded at version 0.2.2; the header carries the crate version, so it is put back first.
    fn pinned_version(out: Vec<u8>) -> Vec<u8> {
        String::from_utf8(out).unwrap().replace(&format!("v{}", env!("CARGO_PKG_VERSION")), "v0.2.2").into_bytes()
    }
    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))
    }

    #[test]
    fn the_text_header_drop_in_bytes_are_pinned() {
        let mut out: Vec<u8> = Vec::new();
        draw_drop_at(&golden_drop_view(), &mut out, (100, 30), None);
        // Recorded from the drawing code as it was before any logo code existed (S391 f4f8c10b).
        let out = pinned_version(out);
        assert_eq!((out.len(), fnv1a(&out)), (GOLDEN_LEN, GOLDEN_HASH), "len {} hash {:#x}", out.len(), fnv1a(&out));
    }

    fn fake_logo() -> crate::logo::Logo {
        crate::logo::Logo { protocol: crate::logo::Protocol::Iterm2, cols: 6, rows: 3, sequence: "\x1b]1337;File=inline=1;width=6;height=3;preserveAspectRatio=1:AAAA\x07".to_string() }
    }
    fn draw_with_logo(logo: &crate::logo::Logo, clear: bool, send: bool) -> String {
        let mut out: Vec<u8> = Vec::new();
        draw_drop_at(&golden_drop_view(), &mut out, (100, 30), Some(LogoDraw { logo, clear, send }));
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn the_logo_image_is_sent_once_at_the_headers_left_and_a_redraw_neither_resends_nor_clears() {
        let logo = fake_logo();
        let first = draw_with_logo(&logo, true, true);
        assert_eq!(first.matches(&logo.sequence).count(), 1, "sent once");
        assert!(first.contains(&format!("\x1b[1;2H{}", logo.sequence)), "placed at row 1, column 2 (the mark's place)");
        assert!(first.contains("\x1b[2J"), "the first frame clears the screen");
        let again = draw_with_logo(&logo, false, false);
        assert_eq!(again.matches(&logo.sequence).count(), 0, "a redraw does not send it again");
        assert!(!again.contains("\x1b[2J"), "and does not clear the screen, so the image stays");
        assert!(again.len() < first.len());
        // after a resize it is sent again, once
        let resized = draw_with_logo(&logo, true, true);
        assert_eq!((resized.matches(&logo.sequence).count(), resized.contains("\x1b[2J")), (1, true));
        // the text mark is not drawn, and the header text starts to its right (row 1, column 9); the rest of the screen is intact
        for f in [&first, &again] {
            assert!(!f.contains('\u{2597}') && !f.contains('\u{2596}') && !f.contains('\u{2590}'), "no text mark under the image");
            assert!(f.contains("\x1b[1;9H") && f.contains("remote-sessions") && f.contains("Fix the flaky parser test"));
            assert!(f.contains("Reading the failing test") && f.contains("enter send"));
            assert!(f.contains("\x1b[K"), "each row is cleared to its end instead of the whole screen");
        }
    }

    #[test]
    fn without_a_logo_the_text_mark_is_drawn_exactly_as_before() {
        let mut out: Vec<u8> = Vec::new();
        draw_drop_at(&golden_drop_view(), &mut out, (100, 30), None);
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains('\u{2597}') && text.contains('\u{25c9}') && text.contains("\x1b[2J"));
        let out = pinned_version(out);
        assert_eq!((out.len(), fnv1a(&out)), (GOLDEN_LEN, GOLDEN_HASH), "byte-identical to the drawing before the logo existed");
        assert!(!text.contains("1337") && !text.contains("\x1b_G"), "no image sequence of any kind");
    }

    // ---- the panel interface: a placeholder extra panel through the live room --------------------------------------------

    struct SamplePanel {
        drops: Arc<Mutex<Vec<String>>>,
    }

    fn sample_row(id: &str, name: &str, tone: Tone) -> PanelRow {
        PanelRow {
            id: id.into(),
            name: name.into(),
            rank: if tone == Tone::Warn { 0 } else { 1 },
            glyph: if tone == Tone::Warn { Glyph::Pause } else { Glyph::Dot },
            tone,
            cols: vec![Col { text: Txt::new("example/state"), ink: Ink::Accent2, fit: Fit::Share { div: 3, min: 6, max: 16 }, min_left: 24, skip_empty: true }],
            detail: vec![vec![Piece::new(name, Ink::Bold, Max::WidthLess(0))], vec![Piece::new("Enter ", Ink::Accent, Max::Any), Piece::new("drop in", Ink::Dim, Max::Any)]],
        }
    }

    impl RoomPanel for SamplePanel {
        fn spec(&self) -> PanelSpec {
            PanelSpec {
                title: "Sample".into(),
                noun: "sample".into(),
                tag: "placeholder".into(),
                badge: Some((Tone::Warn, Glyph::Pause, "held".into())),
                empty: "no sample rows".into(),
                failed: "the sample source could not be read".into(),
                approve_hint: "sample rows have nothing to approve".into(),
                send_hint: "Enter drops in".into(),
                name_fit: NameFit::Fixed(12),
                every_ms: 3000,
            }
        }
        fn read(&self, cx: &PanelCx) -> PanelSnapshot {
            assert_eq!(cx.host, None);
            PanelSnapshot { rows: vec![sample_row("row-b", "second", Tone::Ok), sample_row("row-a", "first", Tone::Warn)], status: PanelStatus::Ready }
        }
        fn drop_in(&self, id: &str, _cx: &PanelCx, view: &dyn DropHost) -> Result<String> {
            self.drops.lock().unwrap().push(id.to_string());
            let src = DropSource { label: "inside a sample row".into(), info: id.into(), name: "sample".into(), every: Duration::from_secs(2), meta: &|| (String::new(), String::new()), turns: &|| Ok(vec![]), send: &|_| Ok(String::new()), echo: true };
            view.two_way(&src)
        }
    }

    #[test]
    fn the_public_build_links_no_extra_panel() {
        #[allow(unreachable_code)]
        {
            assert!(linked_panels().is_empty());
            let (room, _) = live(canned(vec![]));
            assert!(room.panels().is_empty());
        }
    }

    #[test]
    fn an_extra_panel_is_read_drawn_and_dropped_into_through_the_live_room() {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (room, ran) = live(canned(vec![]));
        let room = room.with_panels(vec![Box::new(SamplePanel { drops: drops.clone() })]);
        assert_eq!(room.panels().len(), 1);
        let th = sessions_tui::theme::Theme::detect(true, false);
        let frame = render_once(&room, "h", false, (110, 40), &th, false, 1_794_100_000_000, None);
        assert!(frame.contains(" Sample ") && frame.contains("1 held") && frame.contains("placeholder"), "{frame}");
        assert!(frame.find("first").unwrap() < frame.find("second").unwrap(), "the lower rank is first:\n{frame}");
        assert!(frame.contains("sample 0s ago"), "the status line names the panel:\n{frame}");
        assert_eq!(room.drop_in(&Sel::Panel(0, "row-a".into())).unwrap(), "back in the control room");
        assert_eq!(*drops.lock().unwrap(), vec!["row-a".to_string()]);
        assert!(ran.lock().unwrap().is_empty(), "an extra panel's drop-in starts no other client");
        assert!(room.drop_in(&Sel::Panel(7, "row-a".into())).is_err(), "a panel that is not there");
        assert!(drop_in_command(&Sel::Panel(0, "row-a".into()), "x").unwrap_err().contains("in-process"));
    }

    #[test]
    fn the_demo_room_is_placeholder_only() {
        let d = DemoRoom;
        let all = format!("{:?}{:?}", d.remote(), d.cloud());
        assert!(all.contains("your-org/your-repo") && all.contains("session_01Example"));
        assert!(d.answer("x", "y", true).unwrap().starts_with("demo:"));
        assert!(d.drop_in(&Sel::Remote(DEMO_A.into())).unwrap_err().starts_with("demo: would run remote-sessions attach"));
    }
}
