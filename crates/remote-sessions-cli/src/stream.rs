//! `stream <session> [--from <seq>] [--for <seconds>]`: one JSON line per event as it arrives, over `events/stream`
//! (server-sent events). History is replayed first, then every new event is pushed (message by message, not token by
//! token). If the stream cannot be kept open, a `notice` line says so and the paged events API is polled instead.
//! Event text is untrusted and every output says so.
use crate::UNTRUSTED;
use remote_sessions_core::api::Client;
use remote_sessions_core::events::Event;
use remote_sessions_core::stream::{StreamEnd, StreamItem, StreamOpts};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Write;
use std::time::{Duration, Instant};

fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }

fn event_line(seq: Option<u64>, e: &Event, via: &str) -> Value {
    json!({"item": "event", "via": via, "seq": seq, "id": e.id, "kind": e.kind, "user": e.user, "text": e.text, "at": e.at, "received_ms": now_ms()})
}

/// Run the stream until the time limit, a closed output, Ctrl-C (the process ends), or a failure that polling cannot fix.
pub fn stream_with(client: &Client, session: &str, from: Option<u64>, limit: Option<Duration>, opts: StreamOpts, poll: Duration, out: &mut dyn Write) -> Result<(), String> {
    let deadline = limit.map(|l| Instant::now() + l);
    let opts = StreamOpts { from, deadline, ..opts };
    let emit = |v: Value, out: &mut dyn Write| -> bool { writeln!(out, "{v}").and_then(|_| out.flush()).is_ok() };
    emit(json!({"ok": true, "stream": "start", "session": session, "from": from, "untrusted": UNTRUSTED}), out);
    let mut broken = false;
    let end = client.stream(session, &opts, &mut |_| {}, &mut |item| {
        let line = match item {
            StreamItem::Event { seq, event: Some(e), .. } => event_line(seq, &e, "stream"),
            StreamItem::Event { .. } => return true,
            StreamItem::Receipt { event_id, status, timestamp } => json!({"item": "receipt", "event_id": event_id, "status": status, "timestamp": timestamp, "received_ms": now_ms()}),
        };
        if !emit(line, out) { broken = true; return false; }
        true
    });
    match end {
        StreamEnd::Deadline | StreamEnd::Stopped => Ok(()),
        StreamEnd::Failed(why) => {
            if broken { return Ok(()); }
            emit(json!({"item": "notice", "text": format!("live stream unavailable ({why}); polling the events API instead"), "untrusted": UNTRUSTED}), out);
            poll_events(client, session, deadline, poll, out)
        }
    }
}

fn poll_events(client: &Client, session: &str, deadline: Option<Instant>, poll: Duration, out: &mut dyn Write) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut first = true;
    loop {
        match client.recent_events(session) {
            Ok((_, body)) => {
                let mut list = remote_sessions_core::events::event_list(&body);
                list.sort_by_key(|e| e["sequence_num"].as_u64());
                for env in &list {
                    if let Some(e) = remote_sessions_core::events::parse_event(env) {
                        if seen.insert(e.id.clone()) && writeln!(out, "{}", event_line(env["sequence_num"].as_u64(), &e, "poll")).and_then(|_| out.flush()).is_err() { return Ok(()); }
                    }
                }
                first = false;
            }
            Err(e) if first => return Err(e),
            Err(_) => {}
        }
        if deadline.is_some_and(|d| Instant::now() + poll >= d) { return Ok(()); }
        std::thread::sleep(poll);
    }
}
