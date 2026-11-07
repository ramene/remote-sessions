//! `stream`: one JSON line per event with receipts, a resume from --from, and the polling fallback.
use remote_sessions_cli::stream::stream_with;
use remote_sessions_core::api::Client;
use remote_sessions_core::fakehttp::Fake;
use remote_sessions_core::stream::StreamOpts;
use serde_json::{json, Value};
use std::time::Duration;

fn sse(seq: u64, payload: Value) -> String { format!("data: {}\n\n", json!({"event_id": format!("e{seq}"), "sequence_num": seq, "payload": payload})) }
fn lines(out: Vec<u8>) -> Vec<Value> { String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect() }
fn opts() -> StreamOpts { StreamOpts { retry_delay: Duration::from_millis(10), max_failures: 2, max_connects: Some(1), ..Default::default() } }

#[test]
fn prints_one_json_line_per_event_and_receipt_with_from() {
    let body = sse(4, json!({"type": "user", "uuid": "u1", "message": {"content": "old"}})) + &sse(5, json!({"type": "user", "uuid": "u2", "message": {"content": "say hi"}}))
        + &format!("data: {}\n\n", json!({"event_id": "u2", "status": "DELIVERY_STATUS_RECEIVED", "timestamp": "2024-01-02T10:00:00Z"}))
        + &sse(6, json!({"type": "assistant", "uuid": "a1", "message": {"content": [{"type": "text", "text": "hi \u{1b}[31mthere"}]}}));
    let fake = Fake::start(vec![("GET", "/v1/code/sessions/session_ST1/events/stream".into(), 200, body)]);
    let client = Client::new(Some(&fake.base), "t", None).unwrap();
    let mut out = Vec::new();
    stream_with(&client, "session_ST1", Some(4), Some(Duration::from_secs(5)), opts(), Duration::from_millis(10), &mut out).unwrap();
    let l = lines(out);
    assert_eq!(l[0]["stream"], "start");
    assert!(l[0]["untrusted"].as_str().unwrap().contains("untrusted"));
    let kinds: Vec<(&str, Option<&str>)> = l[1..].iter().map(|v| (v["item"].as_str().unwrap(), v["status"].as_str().or(v["text"].as_str()))).collect();
    assert_eq!(kinds, [("event", Some("say hi")), ("receipt", Some("DELIVERY_STATUS_RECEIVED")), ("event", Some("hi [31mthere"))], "seq 4 is at or before --from and is skipped");
    assert!(!l.iter().any(|v| v.to_string().contains('\u{1b}')), "escapes are removed");
    assert_eq!((l[1]["seq"].as_u64(), l[1]["via"].as_str(), l[1]["user"].as_bool()), (Some(5), Some("stream"), Some(true)));
    assert!(l[1]["received_ms"].as_u64().unwrap() > 0);
    assert!(fake.requests()[0].path.ends_with("from_sequence_num=4"));
}

#[test]
fn falls_back_to_the_events_api_with_a_notice() {
    let log = json!({"data": [{"event_id": "e1", "sequence_num": 1, "payload": {"type": "user", "uuid": "u1", "message": {"content": "start"}}}]}).to_string();
    let fake = Fake::start(vec![
        ("GET", "/v1/code/sessions/session_ST2/events/stream".into(), 500, r#"{"message":"boom"}"#.into()),
        ("GET", "/v1/code/sessions/session_ST2/events".into(), 200, log),
    ]);
    let client = Client::new(Some(&fake.base), "t", None).unwrap();
    let mut out = Vec::new();
    stream_with(&client, "session_ST2", None, Some(Duration::from_millis(300)), StreamOpts { max_connects: None, ..opts() }, Duration::from_millis(20), &mut out).unwrap();
    let l = lines(out);
    assert!(l.iter().any(|v| v["item"] == "notice" && v["text"].as_str().unwrap().contains("polling")), "{l:?}");
    let events: Vec<&Value> = l.iter().filter(|v| v["item"] == "event").collect();
    assert_eq!(events.len(), 1, "polled events are not repeated");
    assert_eq!((events[0]["via"].as_str(), events[0]["text"].as_str()), (Some("poll"), Some("start")));
}
