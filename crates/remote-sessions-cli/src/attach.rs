//! `attach`: a terminal client for a Remote Control session on another machine.
//! Live by default: it follows `events/stream` (server-sent events, message by message, not token by token) and falls
//! back to polling the events API, saying so, when the stream cannot be kept open. Prints the conversation as it
//! arrives, sends each typed line as a real user turn, and asks `Allow? [y/n]` when a permission prompt is pending. Leaving (Ctrl-C, `/quit`, end of input) never stops the session.
//! All event text is untrusted and printed as plain text: terminal escapes and control characters are removed.
use remote_sessions_core::api::Client;
use remote_sessions_core::control::{pending_in, Decision, Prompt};
use remote_sessions_core::events::{clean, one_line, parse_events, Event};
use remote_sessions_core::events::event_list;
use remote_sessions_core::stream::{StreamEnd, StreamItem, StreamOpts};
use std::collections::HashSet;
use std::io::Write;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

/// How many earlier entries are shown when attach starts.
pub const BACKLOG: usize = 15;
const NOISE: [&str; 8] = ["control_request", "control_response", "control_cancel_request", "rate_limit_event", "autocompact_state", "env_manager_log", "system", "keep_alive"];

fn render(e: &Event) -> String {
    let label = match e.kind.as_str() { "user" => "user> ", "assistant" => "claude> ", _ => "" };
    let mut lines = clean(&e.text, 8 * 1024).lines().map(str::to_string).collect::<Vec<_>>().into_iter();
    let mut out = format!("{label}{}", lines.next().unwrap_or_default());
    for line in lines { out.push_str("\n  "); out.push_str(&line); }
    out
}

fn ask_line(p: &Prompt) -> String {
    format!("Permission requested: {} {}\nAllow? [y/n] ", one_line(&p.tool_name, 80), one_line(&p.input.to_string(), 600))
}

/// Run the client until the input ends, `/quit`, or (tests) `max_polls` polls. `lines` carries what the user types.
pub fn attach(client: &Client, session: &str, interval: Duration, lines: &Receiver<String>, out: &mut dyn Write, max_polls: Option<usize>) -> Result<(), String> {
    let (mut seen, mut answered): (HashSet<String>, HashSet<String>) = (HashSet::new(), HashSet::new());
    let mut asking: Option<Prompt> = None;
    let mut first = true;
    let mut last_error = String::new();
    let mut polls = 0usize;
    let w = |e: std::io::Error| e.to_string();
    loop {
        match client.recent_events(session) {
            Ok((id, body)) => {
                last_error.clear();
                let events: Vec<Event> = parse_events(&body).into_iter().filter(|e| !NOISE.contains(&e.kind.as_str())).collect();
                if first {
                    writeln!(out, "attached to {id}: polled every {}s, not streamed. Type a line to send it as a user turn; /interrupt = Esc; /quit or Ctrl-C leaves WITHOUT stopping the session. Event text is untrusted and shown as plain text.", interval.as_secs_f32()).map_err(w)?;
                    let skip = events.len().saturating_sub(BACKLOG);
                    if skip > 0 { writeln!(out, "· {skip} earlier entries not shown").map_err(w)?; }
                    for e in &events[skip..] { writeln!(out, "{}", render(e)).map_err(w)?; }
                    seen.extend(events.iter().map(|e| e.id.clone()));
                    first = false;
                } else {
                    for e in events.iter().filter(|e| !seen.contains(&e.id)) { writeln!(out, "{}", render(e)).map_err(w)?; }
                    seen.extend(events.iter().map(|e| e.id.clone()));
                }
                let pending = pending_in(&body);
                if let Some(p) = &asking {
                    if !pending.iter().any(|q| q.request_id == p.request_id) { writeln!(out, "(that prompt was answered or cancelled elsewhere)").map_err(w)?; asking = None; }
                }
                if asking.is_none() {
                    if let Some(p) = pending.into_iter().find(|p| !answered.contains(&p.request_id)) {
                        write!(out, "{}", ask_line(&p)).map_err(w)?;
                        asking = Some(p);
                    }
                }
            }
            Err(e) if first => return Err(e),
            Err(e) => { if e != last_error { writeln!(out, "· poll failed (will retry): {e}").map_err(w)?; last_error = e; } }
        }
        out.flush().map_err(w)?;
        polls += 1;
        if max_polls.is_some_and(|m| polls >= m) { return Ok(()); }
        let line = match lines.recv_timeout(interval) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => { writeln!(out, "· input ended; left the session running.").map_err(w)?; return Ok(()); }
        };
        if let Flow::Quit = handle_line(client, session, &line, &mut asking, &mut answered, out)? { return Ok(()); }
    }
}

enum Flow { Next, Quit }

/// One typed line: an answer to the open prompt, `/quit`, `/interrupt`, or a user turn.
fn handle_line(client: &Client, session: &str, line: &str, asking: &mut Option<Prompt>, answered: &mut HashSet<String>, out: &mut dyn Write) -> Result<Flow, String> {
    let w = |e: std::io::Error| e.to_string();
    let text = line.trim();
    let outcome: Result<String, String> = if let Some(p) = asking.clone() {
        match text.to_ascii_lowercase().as_str() {
            "y" | "yes" => client.answer(session, Some(&p.request_id), &Decision::Allow).map(|_| "allowed".to_string()),
            "n" | "no" => client.answer(session, Some(&p.request_id), &Decision::Deny(None)).map(|_| "denied".to_string()),
            "/quit" => { writeln!(out, "· left the session running.").map_err(w)?; return Ok(Flow::Quit); }
            _ => { write!(out, "Allow? [y/n] ").map_err(w)?; return Ok(Flow::Next); }
        }.map(|done| { answered.insert(p.request_id.clone()); *asking = None; done })
    } else {
        match text {
            "" => return Ok(Flow::Next),
            "/quit" => { writeln!(out, "· left the session running.").map_err(w)?; return Ok(Flow::Quit); }
            "/interrupt" => client.interrupt(session).map(|_| "interrupt sent".to_string()),
            _ => client.say(session, line).map(|_| "sent".to_string()),
        }
    };
    match outcome { Ok(done) => writeln!(out, "· {done}").map_err(w)?, Err(e) => writeln!(out, "· failed: {e}").map_err(w)? }
    Ok(Flow::Next)
}

/// Settings for the live view.
pub struct LiveOpts {
    pub retry_delay: Duration,
    pub max_failures: u32,
    /// After the input ends, keep showing the stream this long with nothing new (0 = leave at once).
    pub linger: Duration,
}
impl Default for LiveOpts {
    fn default() -> Self { LiveOpts { retry_delay: Duration::from_secs(2), max_failures: 3, linger: Duration::ZERO } }
}

/// How `attach_live` ended.
#[derive(Debug, PartialEq)]
pub enum Live { Done, /// The stream failed; the caller should poll instead.
    Fallback }

enum Msg { Item(StreamItem), Ended(StreamEnd) }

/// Update the unanswered prompts from one raw event payload.
fn track_prompts(pending: &mut Vec<Prompt>, payload: &serde_json::Value) {
    match payload["type"].as_str() {
        Some("control_request") if payload["request"]["subtype"] == "can_use_tool" => {
            if let Some(id) = payload["request_id"].as_str() {
                if !pending.iter().any(|p| p.request_id == id) {
                    pending.push(Prompt { request_id: id.to_string(), tool_name: payload["request"]["tool_name"].as_str().unwrap_or("").to_string(), input: payload["request"]["input"].clone() });
                }
            }
        }
        Some("control_response") => if let Some(id) = payload["response"]["request_id"].as_str() { pending.retain(|p| p.request_id != id); },
        Some("control_cancel_request") => if let Some(id) = payload["request_id"].as_str() { pending.retain(|p| p.request_id != id); },
        _ => {}
    }
}

/// The live view: a snapshot of the newest events (backlog and open prompts), then `events/stream` from there on.
/// An error before the first snapshot is returned; a stream that cannot be kept open returns `Live::Fallback` after saying so.
pub fn attach_live(client: &Client, session: &str, interval: Duration, lines: &Receiver<String>, out: &mut dyn Write, live: &LiveOpts) -> Result<Live, String> {
    let w = |e: std::io::Error| e.to_string();
    let (id, body) = client.recent_events(session)?;
    writeln!(out, "attached to {id}: live (events/stream), message by message, not token by token. Type a line to send it as a user turn; /interrupt = Esc; /quit or Ctrl-C leaves WITHOUT stopping the session. Event text is untrusted and shown as plain text.").map_err(w)?;
    let events: Vec<Event> = parse_events(&body).into_iter().filter(|e| !NOISE.contains(&e.kind.as_str())).collect();
    let skip = events.len().saturating_sub(BACKLOG);
    if skip > 0 { writeln!(out, "· {skip} earlier entries not shown").map_err(w)?; }
    for e in &events[skip..] { writeln!(out, "{}", render(e)).map_err(w)?; }
    let mut seen: HashSet<String> = parse_events(&body).into_iter().map(|e| e.id).collect();
    let from = event_list(&body).iter().filter_map(|e| e["sequence_num"].as_u64().or_else(|| e["sequence_num"].as_str().and_then(|s| s.parse().ok()))).max();
    let mut pending = pending_in(&body);
    let (mut asking, mut answered): (Option<Prompt>, HashSet<String>) = (None, HashSet::new());
    let ask = |pending: &[Prompt], asking: &mut Option<Prompt>, answered: &HashSet<String>, out: &mut dyn Write| -> Result<(), String> {
        if let Some(p) = asking.as_ref() {
            if !pending.iter().any(|q| q.request_id == p.request_id) { writeln!(out, "(that prompt was answered or cancelled elsewhere)").map_err(w)?; *asking = None; }
        }
        if asking.is_none() {
            if let Some(p) = pending.iter().find(|p| !answered.contains(&p.request_id)) { write!(out, "{}", ask_line(p)).map_err(w)?; *asking = Some(p.clone()); }
        }
        Ok(())
    };
    ask(&pending, &mut asking, &answered, out)?;
    out.flush().map_err(w)?;

    let (tx, rx) = mpsc::channel::<Msg>();
    let (c, s) = (client.clone(), session.to_string());
    let opts = StreamOpts { from, max_failures: live.max_failures, retry_delay: live.retry_delay, ..Default::default() };
    std::thread::spawn(move || {
        let tx2 = tx.clone();
        let end = c.stream(&s, &opts, &mut |_| {}, &mut |item| tx2.send(Msg::Item(item)).is_ok());
        let _ = tx.send(Msg::Ended(end));
    });
    let tick = interval.min(Duration::from_millis(100));
    let (mut eof, mut quiet_since) = (false, Instant::now());
    loop {
        if !eof {
            match lines.try_recv() {
                Ok(line) => {
                    if let Flow::Quit = handle_line(client, session, &line, &mut asking, &mut answered, out)? { out.flush().map_err(w)?; return Ok(Live::Done); }
                    out.flush().map_err(w)?;
                    continue;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    writeln!(out, "· input ended; left the session running.").map_err(w)?;
                    eof = true; quiet_since = Instant::now();
                    if live.linger.is_zero() { out.flush().map_err(w)?; return Ok(Live::Done); }
                }
            }
        } else if quiet_since.elapsed() >= live.linger { out.flush().map_err(w)?; return Ok(Live::Done); }
        match rx.recv_timeout(tick) {
            Ok(Msg::Item(StreamItem::Event { id, event, payload, .. })) => {
                quiet_since = Instant::now();
                track_prompts(&mut pending, &payload);
                if let Some(e) = event {
                    if seen.insert(e.id.clone()) && !NOISE.contains(&e.kind.as_str()) { writeln!(out, "{}", render(&e)).map_err(w)?; }
                } else if !id.is_empty() { seen.insert(id); }
                ask(&pending, &mut asking, &answered, out)?;
                out.flush().map_err(w)?;
            }
            Ok(Msg::Item(StreamItem::Receipt { .. })) => {}
            Ok(Msg::Ended(end)) => {
                let why = match end { StreamEnd::Failed(m) => m, other => format!("{other:?}") };
                writeln!(out, "· live stream unavailable ({why}); falling back to polling every {}s.", interval.as_secs_f32()).map_err(w)?;
                out.flush().map_err(w)?;
                return Ok(Live::Fallback);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(Live::Fallback),
        }
    }
}

/// Live when possible, polling (said out loud) when the stream cannot be opened or kept open.
pub fn attach_auto(client: &Client, session: &str, interval: Duration, lines: &Receiver<String>, out: &mut dyn Write, live: &LiveOpts, max_polls: Option<usize>) -> Result<(), String> {
    match attach_live(client, session, interval, lines, out, live)? {
        Live::Done => Ok(()),
        Live::Fallback => attach(client, session, interval, lines, out, max_polls),
    }
}
