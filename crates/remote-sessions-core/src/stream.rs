//! Live events: `GET /v1/code/sessions/<id>/events/stream` (server-sent events), the call Claude Code's own session
//! client makes. The stream first replays the history, then pushes every event as it is produced (message by message,
//! not token by token). Lines are `data: {json}`; an event has `payload` like the paged events API, and a delivery
//! receipt is `{"event_id","status":"DELIVERY_STATUS_...","timestamp"}` without a payload.
//!
//! The reader reconnects when the connection drops, asking to resume after the last sequence number it saw
//! (`from_sequence_num` and `Last-Event-ID`) and skipping anything it has already delivered. The paged
//! [`crate::api::Client::events`] API stays the fallback when the stream cannot be opened.
//!
//! Event text is untrusted. Nothing here executes it.
use crate::api::Client;
use crate::events::{parse_event, Event};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufReader, Read};
use std::time::{Duration, Instant};

const MAX_LINE: u64 = 4 * 1024 * 1024;

/// One thing the stream delivered.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "item", rename_all = "snake_case")]
pub enum StreamItem {
    /// A session event. `event` is None when it carries no text (it still advances the sequence).
    Event { seq: Option<u64>, id: String, event: Option<Event>, #[serde(skip)] payload: Value },
    /// A delivery receipt for an event you sent (`DELIVERY_STATUS_RECEIVED|PROCESSING|PROCESSED`).
    Receipt { event_id: String, status: String, timestamp: Option<String> },
}

/// One `data:` line -> an item. None for anything that is not a JSON object.
pub fn parse_data(data: &str) -> Option<StreamItem> {
    let v: Value = serde_json::from_str(data.trim()).ok()?;
    if !v.is_object() { return None; }
    let status = v["status"].as_str().filter(|s| s.starts_with("DELIVERY_STATUS_"));
    if let (Some(status), false) = (status, v["payload"].is_object()) {
        return Some(StreamItem::Receipt { event_id: v["event_id"].as_str().unwrap_or("").to_string(), status: status.to_string(), timestamp: v["timestamp"].as_str().map(str::to_string) });
    }
    if !v["payload"].is_object() && v["type"].is_null() && v["event_type"].is_null() { return None; }
    let seq = v["sequence_num"].as_u64().or_else(|| v["sequence_num"].as_str().and_then(|s| s.parse().ok()));
    let payload = if v["payload"].is_object() { v["payload"].clone() } else { v.clone() };
    let event = parse_event(&v);
    let id = event.as_ref().map(|e| e.id.clone()).or_else(|| v["event_id"].as_str().or(payload["uuid"].as_str()).map(str::to_string)).unwrap_or_default();
    Some(StreamItem::Event { seq, id, event, payload })
}

/// How the stream ended.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEnd {
    /// The caller's callback returned false.
    Stopped,
    /// The time limit passed.
    Deadline,
    /// It could not be opened or kept open; the message says why. Fall back to the paged events API.
    Failed(String),
}

pub struct StreamOpts {
    /// Resume after this sequence number (the first connect asks for it too).
    pub from: Option<u64>,
    /// Stop after this long.
    pub deadline: Option<Instant>,
    /// Consecutive connections that delivered nothing before giving up.
    pub max_failures: u32,
    /// Pause before a reconnect.
    pub retry_delay: Duration,
    /// Stop after this many connections (tests).
    pub max_connects: Option<u32>,
    /// A quiet connection is re-opened after this long.
    pub read_timeout: Duration,
}
impl Default for StreamOpts {
    fn default() -> Self { StreamOpts { from: None, deadline: None, max_failures: 3, retry_delay: Duration::from_secs(2), max_connects: None, read_timeout: Duration::from_secs(120) } }
}

/// The next line (without its newline), or None at the end of the body. A line over 4 MB is an error.
fn read_line(r: &mut BufReader<Box<dyn Read + Send>>) -> std::io::Result<Option<String>> {
    use std::io::BufRead;
    let mut buf = Vec::new();
    let n = (&mut *r).take(MAX_LINE + 1).read_until(b'\n', &mut buf)?;
    if n == 0 { return Ok(None); }
    if buf.len() as u64 > MAX_LINE { return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "stream line too long")); }
    while matches!(buf.last(), Some(b'\n' | b'\r')) { buf.pop(); }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn timed_out(e: &std::io::Error) -> bool { matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) }

impl Client {
    /// Follow a session live. `on` gets every item once, in order (history first); return false from it to stop.
    /// `on_open` is told which id form (`session_<x>` / `cse_<x>`) connected, each time it connects.
    pub fn stream(&self, session: &str, opts: &StreamOpts, on_open: &mut dyn FnMut(&str), on: &mut dyn FnMut(StreamItem) -> bool) -> StreamEnd {
        let mut last_seq = opts.from;
        let mut seen: HashSet<String> = HashSet::new();
        let (mut failures, mut connects) = (0u32, 0u32);
        loop {
            if opts.deadline.is_some_and(|d| Instant::now() >= d) { return StreamEnd::Deadline; }
            if opts.max_connects.is_some_and(|m| connects >= m) { return StreamEnd::Stopped; }
            if connects > 0 { std::thread::sleep(opts.retry_delay); }
            connects += 1;
            let remaining = opts.deadline.map(|d| d.saturating_duration_since(Instant::now()));
            let timeout = remaining.map_or(opts.read_timeout, |r| r.min(opts.read_timeout)).max(Duration::from_millis(50));
            let (id, body) = match self.open_stream(session, last_seq, timeout) {
                Ok(ok) => ok,
                Err((fatal, message)) => {
                    failures += 1;
                    if fatal || failures >= opts.max_failures { return StreamEnd::Failed(message); }
                    continue;
                }
            };
            on_open(&id);
            let mut reader = BufReader::new(body);
            let mut delivered = false;
            loop {
                let line = match read_line(&mut reader) {
                    Ok(Some(l)) => l,
                    Ok(None) => break,
                    Err(e) if timed_out(&e) => { delivered = true; break; }
                    Err(_) => break,
                };
                let Some(data) = line.strip_prefix("data:") else {
                    if opts.deadline.is_some_and(|d| Instant::now() >= d) { return StreamEnd::Deadline; }
                    continue;
                };
                let Some(item) = parse_data(data) else { continue };
                if let StreamItem::Event { seq, id, .. } = &item {
                    if let (Some(s), Some(l)) = (seq, last_seq) { if *s <= l { continue; } }
                    if !id.is_empty() && !seen.insert(id.clone()) { continue; }
                    if let Some(s) = seq { last_seq = Some(last_seq.map_or(*s, |l| l.max(*s))); }
                }
                if let StreamItem::Receipt { event_id, status, .. } = &item {
                    if !seen.insert(format!("receipt:{event_id}:{status}")) { continue; }
                }
                delivered = true;
                if !on(item) { return StreamEnd::Stopped; }
                if opts.deadline.is_some_and(|d| Instant::now() >= d) { return StreamEnd::Deadline; }
            }
            if delivered { failures = 0; } else {
                failures += 1;
                if failures >= opts.max_failures { return StreamEnd::Failed("the stream closed without delivering anything".into()); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakehttp::Fake;
    use serde_json::json;

    fn ev(seq: u64, payload: Value) -> String { format!("data: {}\n\n", json!({"event_id": format!("e{seq}"), "sequence_num": seq, "payload": payload})) }
    fn fast() -> StreamOpts { StreamOpts { retry_delay: Duration::from_millis(5), read_timeout: Duration::from_secs(5), ..Default::default() } }

    #[test]
    fn data_lines_become_events_and_receipts() {
        let StreamItem::Event { seq, event, payload, .. } = parse_data(&json!({"event_id": "e1", "sequence_num": 7, "payload": {"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "hi"}]}}}).to_string()).unwrap() else { panic!() };
        assert_eq!(seq, Some(7));
        let event = event.unwrap();
        assert_eq!((event.kind.as_str(), event.text.as_str()), ("assistant", "hi"));
        assert_eq!(payload["type"], "assistant");
        assert_eq!(parse_data(r#"{"event_id":"u9","status":"DELIVERY_STATUS_PROCESSED","timestamp":"2024-01-02T10:00:00Z"}"#), Some(StreamItem::Receipt { event_id: "u9".into(), status: "DELIVERY_STATUS_PROCESSED".into(), timestamp: Some("2024-01-02T10:00:00Z".into()) }));
        assert_eq!(parse_data("not json"), None);
        assert_eq!(parse_data("[1]"), None);
        assert_eq!(parse_data(r#"{"hello":"world"}"#), None, "a frame that is neither an event nor a receipt is ignored");
    }

    #[test]
    fn streams_in_order_sends_sse_headers_and_resumes_without_duplicates() {
        let body = format!(": keep-alive\n\nid: 1\n{}event: x\n{}{}", ev(1, json!({"type": "user", "uuid": "u1", "message": {"content": "one"}})), ev(2, json!({"type": "assistant", "uuid": "a1", "message": {"content": "two"}})),
            format!("data: {}\n\n", json!({"event_id": "u1", "status": "DELIVERY_STATUS_PROCESSED", "timestamp": "t"})));
        let fake = Fake::start(vec![("GET", "/v1/code/sessions/session_S1/events/stream".into(), 200, body)]);
        let client = Client::new(Some(&fake.base), "test-token", None).unwrap();
        let mut got = Vec::new();
        let end = client.stream("session_S1", &StreamOpts { max_connects: Some(2), ..fast() }, &mut |_| {}, &mut |i| { got.push(i); true });
        assert_eq!(end, StreamEnd::Stopped);
        let kinds: Vec<String> = got.iter().map(|i| match i { StreamItem::Event { seq, .. } => format!("e{}", seq.unwrap()), StreamItem::Receipt { status, .. } => status.clone() }).collect();
        assert_eq!(kinds, ["e1", "e2", "DELIVERY_STATUS_PROCESSED"], "the replayed events and receipt are skipped");
        let seen = fake.requests();
        assert_eq!(seen.len(), 2);
        let h = |r: &crate::fakehttp::Seen, k: &str| r.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(h(&seen[0], "accept").as_deref(), Some("text/event-stream"));
        assert_eq!(h(&seen[0], "authorization").as_deref(), Some("Bearer test-token"));
        assert!(seen[0].path.ends_with("/events/stream") && h(&seen[0], "last-event-id").is_none());
        assert!(seen[1].path.ends_with("/events/stream?from_sequence_num=2"), "{}", seen[1].path);
        assert_eq!(h(&seen[1], "last-event-id").as_deref(), Some("2"));
    }

    #[test]
    fn from_is_sent_on_the_first_connect_and_stop_ends_the_stream() {
        let fake = Fake::start(vec![("GET", "/v1/code/sessions/session_S2/events/stream".into(), 200, ev(5, json!({"type": "user", "uuid": "u", "message": {"content": "x"}})) + &ev(6, json!({"type": "user", "uuid": "v", "message": {"content": "y"}})))]);
        let client = Client::new(Some(&fake.base), "t", None).unwrap();
        let mut n = 0;
        let end = client.stream("session_S2", &StreamOpts { from: Some(5), ..fast() }, &mut |_| {}, &mut |_| { n += 1; false });
        assert_eq!((end, n), (StreamEnd::Stopped, 1), "seq 5 is skipped (already seen), seq 6 delivered then stop");
        assert!(fake.requests()[0].path.ends_with("?from_sequence_num=5"));
    }

    #[test]
    fn a_stream_that_cannot_open_fails_so_the_caller_can_fall_back() {
        let fake = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 404, r#"{"message":"nope"}"#.into())]);
        let client = Client::new(Some(&fake.base), "t", None).unwrap();
        let end = client.stream("session_S3", &StreamOpts { max_failures: 2, ..fast() }, &mut |_| {}, &mut |_| true);
        assert!(matches!(&end, StreamEnd::Failed(m) if m.contains("nope")), "{end:?}");
        let tried: Vec<String> = fake.requests().into_iter().map(|r| r.path).collect();
        assert!(tried.iter().any(|p| p.contains("session_S3")) && tried.iter().any(|p| p.contains("cse_S3")), "both id forms are tried: {tried:?}");
        let forbidden = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 403, r#"{"message":"untrusted device"}"#.into())]);
        let c2 = Client::new(Some(&forbidden.base), "t", None).unwrap();
        assert_eq!(c2.stream("session_S3", &fast(), &mut |_| {}, &mut |_| true), StreamEnd::Failed(crate::api::UNTRUSTED_DEVICE.into()));
        assert_eq!(forbidden.requests().len(), 1, "a 403 is not retried");
    }
}
