//! `attach --tui`: the [`sessions_tui::Backend`] for a Remote Control session, over the core `Client`.
use remote_sessions_core::api::Client;
use remote_sessions_core::control::Decision as CoreDecision;
use remote_sessions_core::events::{blocks, event_list, parse_event, Block};
use remote_sessions_core::stream::{StreamEnd as CoreEnd, StreamItem, StreamOpts};
use serde_json::Value;
use sessions_tui::{Backend, Blk, Decision, Ev, InitInfo, Item, Msg, ResultInfo, Snapshot, StreamEnd};
use std::sync::mpsc::Sender;

pub struct RcBackend {
    pub client: Client,
    pub session: String,
    pub host: String,
}

fn blk(b: Block) -> Blk {
    match b {
        Block::Text(t) => Blk::Text(t),
        Block::ToolUse { id, name, input } => Blk::ToolUse { id, name, input },
        Block::ToolResult { tool_use_id, text, is_error } => Blk::ToolResult { tool_use_id, text, is_error },
        Block::Thinking => Blk::Thinking,
        Block::ControlRequest { request_id, subtype, tool_name, input, mode } => Blk::Request { request_id, subtype, tool_name, input, mode },
        Block::ControlResponse { request_id } => Blk::Response { request_id },
        Block::ControlCancel { request_id } => Blk::Cancel { request_id },
    }
}

/// One payload (the event itself, without the envelope) as a TUI event.
pub fn ev_from_payload(seq: Option<u64>, id: &str, payload: &Value, note: Option<String>) -> Ev {
    let kind = payload["type"].as_str().unwrap_or("event").to_string();
    let mut ev = Ev::new(seq, id, &kind, blocks(payload).into_iter().map(blk).collect());
    match kind.as_str() {
        "result" => {
            ev.result = Some(ResultInfo { subtype: payload["subtype"].as_str().unwrap_or("").to_string(), turns: payload["num_turns"].as_u64(), duration_ms: payload["duration_ms"].as_f64().map(|m| m as u64) });
        }
        "system" if payload["subtype"] == "init" => {
            ev.init = Some(InitInfo { model: payload["model"].as_str().map(str::to_string), mode: payload["permissionMode"].as_str().or(payload["permission_mode"].as_str()).map(str::to_string) });
        }
        _ => {}
    }
    if ev.blocks.is_empty() {
        ev.note = note;
    }
    ev
}

/// An events-API entry (envelope or bare event) as a TUI event.
pub fn ev_from_envelope(v: &Value) -> Ev {
    let payload = if v["payload"].is_object() { &v["payload"] } else { v };
    let seq = v["sequence_num"].as_u64().or_else(|| v["sequence_num"].as_str().and_then(|s| s.parse().ok()));
    let id = parse_event(v).map(|e| e.id).or_else(|| ["uuid", "id"].iter().find_map(|k| payload[*k].as_str()).or(v["event_id"].as_str()).map(str::to_string)).unwrap_or_default();
    let note = parse_event(v).map(|e| e.text);
    let mut ev = ev_from_payload(seq, &id, payload, note);
    if ev.kind == "event" {
        if let Some(t) = v["event_type"].as_str() {
            ev.kind = t.to_string();
        }
    }
    ev
}

/// A stream item as a TUI item.
pub fn item_from_stream(item: StreamItem) -> Item {
    match item {
        StreamItem::Event { seq, id, event, payload } => Item::Event(ev_from_payload(seq, &id, &payload, event.map(|e| e.text))),
        StreamItem::Receipt { event_id, status, .. } => Item::Receipt { event_id, status: status.trim_start_matches("DELIVERY_STATUS_").to_string() },
    }
}

fn short_name(session: &str) -> String {
    let id = session.trim();
    let tail = id.strip_prefix("session_").or_else(|| id.strip_prefix("cse_")).unwrap_or(id);
    let prefix = if id.starts_with("cse_") { "cse_" } else { "session_" };
    if tail.chars().count() > 8 { format!("{prefix}{}…", tail.chars().take(4).collect::<String>()) } else { id.to_string() }
}

impl Backend for RcBackend {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let (_, body) = self.client.recent_events(&self.session)?;
        let list = event_list(&body);
        Ok(Snapshot { name: short_name(&self.session), host: self.host.clone(), truncated: list.len() >= 100, events: list.iter().map(ev_from_envelope).collect() })
    }

    fn stream(&self, from: Option<u64>, tx: Sender<Msg>) {
        let opts = StreamOpts { from, ..Default::default() };
        let end = self.client.stream(&self.session, &opts, &mut |_| { let _ = tx.send(Msg::Open); }, &mut |item| tx.send(Msg::Item(item_from_stream(item))).is_ok());
        let _ = tx.send(Msg::StreamEnded(match end { CoreEnd::Failed(m) => StreamEnd::Failed(m), _ => StreamEnd::Stopped }));
    }

    fn say(&self, uuid: &str, text: &str) -> Result<(), String> {
        self.client.say_as(&self.session, uuid, text).map(|_| ())
    }
    fn interrupt(&self) -> Result<(), String> {
        self.client.interrupt(&self.session).map(|_| ())
    }
    fn set_mode(&self, mode: &str) -> Result<(), String> {
        self.client.set_mode(&self.session, mode).map(|_| ())
    }
    fn answer(&self, request_id: &str, d: Decision) -> Result<(), String> {
        let d = match d { Decision::Allow => CoreDecision::Allow, Decision::Deny(m) => CoreDecision::Deny(m) };
        self.client.answer(&self.session, Some(request_id), &d).map(|_| ())
    }
    fn poll(&self) -> Result<Vec<Ev>, String> {
        let (_, body) = self.client.recent_events(&self.session)?;
        Ok(event_list(&body).iter().map(ev_from_envelope).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remote_sessions_core::fakehttp::Fake;
    use serde_json::json;

    const S: &str = "session_01ExampleAAAAAAAAAAAAAAA";

    fn env(seq: u64, payload: Value) -> Value {
        json!({"event_id": format!("e{seq}"), "sequence_num": seq, "payload": payload})
    }

    #[test]
    fn payloads_become_structured_events() {
        let a = ev_from_envelope(&env(3, json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "hi"}, {"type": "tool_use", "id": "toolu_ex1", "name": "Bash", "input": {"command": "ls"}}]}})));
        assert_eq!((a.seq, a.id.as_str(), a.kind.as_str()), (Some(3), "a1", "assistant"));
        assert!(matches!(&a.blocks[1], Blk::ToolUse { name, .. } if name == "Bash"));
        let r = ev_from_envelope(&env(4, json!({"type": "result", "uuid": "r1", "subtype": "success", "num_turns": 3, "duration_ms": 121000.0, "result": "done"})));
        assert_eq!(r.result, Some(ResultInfo { subtype: "success".into(), turns: Some(3), duration_ms: Some(121000) }));
        let init = ev_from_envelope(&env(1, json!({"type": "system", "subtype": "init", "uuid": "s1", "model": "model-x", "cwd": "/home/user/project", "permissionMode": "default"})));
        assert_eq!(init.init, Some(InitInfo { model: Some("model-x".into()), mode: Some("default".into()) }));
        let ask = ev_from_envelope(&env(5, json!({"type": "control_request", "request_id": "req_ex1", "uuid": "c1", "request": {"subtype": "can_use_tool", "tool_name": "WebFetch", "input": {"url": "https://example.com/"}}})));
        assert!(matches!(&ask.blocks[0], Blk::Request { request_id, tool_name, .. } if request_id == "req_ex1" && tool_name == "WebFetch"));
        let odd = ev_from_envelope(&env(6, json!({"type": "brand_new_thing", "uuid": "o1"})));
        assert_eq!(odd.note.as_deref(), Some("· brand_new_thing"));
        let thinking = ev_from_envelope(&env(7, json!({"type": "assistant", "uuid": "t1", "message": {"content": [{"type": "thinking", "thinking": ""}]}})));
        assert_eq!(thinking.blocks, vec![Blk::Thinking]);
    }

    #[test]
    fn stream_items_map_and_receipts_lose_their_prefix() {
        let item = remote_sessions_core::stream::parse_data(&env(9, json!({"type": "user", "uuid": "u1", "message": {"content": "hello"}})).to_string()).unwrap();
        let Item::Event(e) = item_from_stream(item) else { panic!() };
        assert_eq!((e.id.as_str(), e.blocks.clone()), ("u1", vec![Blk::Text("hello".into())]));
        let rc = remote_sessions_core::stream::parse_data(r#"{"event_id":"u1","status":"DELIVERY_STATUS_PROCESSED","timestamp":"t"}"#).unwrap();
        assert_eq!(item_from_stream(rc), Item::Receipt { event_id: "u1".into(), status: "PROCESSED".into() });
    }

    #[test]
    fn names_are_short() {
        assert_eq!(short_name(S), "session_01Ex…");
        assert_eq!(short_name("session_abc"), "session_abc");
    }

    fn backend(fake: &Fake) -> RcBackend {
        RcBackend { client: Client::new(Some(&fake.base), "test-token", None).unwrap(), session: S.into(), host: "mac-a".into() }
    }

    #[test]
    fn calls_post_the_same_payloads_as_the_verbs() {
        let events = json!({"data": [env(2, json!({"type": "control_request", "request_id": "req_ex1", "uuid": "c1", "request": {"subtype": "can_use_tool", "tool_name": "WebFetch", "input": {"url": "https://example.com/"}}})), env(1, json!({"type": "user", "uuid": "u1", "message": {"content": "go"}}))]});
        let fake = Fake::start(vec![("GET", format!("/v1/code/sessions/{S}/events"), 200, events.to_string()), ("POST", format!("/v1/code/sessions/{S}/events"), 200, r#"{"results":[]}"#.into())]);
        let b = backend(&fake);
        let snap = b.snapshot().unwrap();
        assert_eq!((snap.host.as_str(), snap.events.len(), snap.events[0].seq, snap.truncated), ("mac-a", 2, Some(1), false), "oldest first");
        b.say("11111111-2222-4333-8444-555555555555", "run it").unwrap();
        b.interrupt().unwrap();
        b.set_mode("plan").unwrap();
        b.answer("req_ex1", Decision::Allow).unwrap();
        b.answer("req_ex1", Decision::Deny(Some("not now".into()))).unwrap();
        let posts: Vec<Value> = fake.requests().into_iter().filter(|r| r.method == "POST").map(|r| serde_json::from_str::<Value>(&r.body).unwrap()["events"][0]["payload"].clone()).collect();
        assert_eq!(posts.len(), 5);
        assert_eq!((posts[0]["type"].as_str(), posts[0]["uuid"].as_str(), posts[0]["message"]["content"].as_str(), posts[0]["session_id"].as_str()), (Some("user"), Some("11111111-2222-4333-8444-555555555555"), Some("run it"), Some(S)));
        assert_eq!(posts[1]["request"]["subtype"], "interrupt");
        assert_eq!((posts[2]["request"]["subtype"].as_str(), posts[2]["request"]["mode"].as_str()), (Some("set_permission_mode"), Some("plan")));
        assert_eq!((posts[3]["type"].as_str(), posts[3]["response"]["response"]["behavior"].as_str(), posts[3]["response"]["request_id"].as_str()), (Some("control_response"), Some("allow"), Some("req_ex1")));
        assert_eq!(posts[3]["response"]["response"]["updatedInput"]["url"], "https://example.com/");
        assert_eq!((posts[4]["response"]["response"]["behavior"].as_str(), posts[4]["response"]["response"]["message"].as_str()), (Some("deny"), Some("not now")));
        assert!(b.answer("req_missing", Decision::Allow).is_err(), "a prompt that is not pending is refused");
    }

    #[test]
    fn stream_sends_open_then_items_and_stops_when_the_receiver_goes() {
        let body = format!("data: {}\n\ndata: {}\n\n", env(1, json!({"type": "user", "uuid": "u1", "message": {"content": "one"}})), env(2, json!({"type": "assistant", "uuid": "a1", "message": {"content": "two"}})));
        let fake = Fake::start(vec![("GET", format!("/v1/code/sessions/{S}/events/stream"), 200, body)]);
        let b = backend(&fake);
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || b.stream(None, tx));
        assert!(matches!(rx.recv().unwrap(), Msg::Open));
        let mut seqs = Vec::new();
        while seqs.len() < 2 {
            if let Msg::Item(Item::Event(e)) = rx.recv().unwrap() {
                seqs.push(e.seq);
            }
        }
        assert_eq!(seqs, [Some(1), Some(2)]);
        drop(rx);
        handle.join().unwrap();
    }

    #[test]
    fn a_stream_that_cannot_open_reports_failed() {
        let fake = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 404, r#"{"message":"nope"}"#.into())]);
        let b = backend(&fake);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || b.stream(None, tx));
        let last = rx.iter().last().unwrap();
        assert!(matches!(last, Msg::StreamEnded(StreamEnd::Failed(m)) if m.contains("nope")));
    }
}
