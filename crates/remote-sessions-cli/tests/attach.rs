//! `attach` against a fake loopback backend: prints the conversation, answers prompts, sends turns, strips escapes.
use remote_sessions_cli::attach::attach;
use remote_sessions_core::api::Client;
use remote_sessions_core::fakehttp::Fake;
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

const SUF: &str = "01AttachTest12345";
fn ev(seq: u64, payload: Value) -> Value { json!({"event_id": format!("e{seq}"), "sequence_num": seq, "payload": payload}) }

fn log() -> String {
    json!({"data": [
        ev(5, json!({"type": "control_request", "request_id": "r9", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "echo \u{1b}]0;pwned\u{7}done"}}})),
        ev(4, json!({"type": "assistant", "uuid": "a2", "message": {"content": [{"type": "text", "text": "\u{1b}[31mred text\u{1b}[0m"}, {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}}]}})),
        ev(3, json!({"type": "user", "uuid": "u2", "message": {"role": "user", "content": [{"type": "tool_result", "content": "file1\nfile2\nfile3"}]}})),
        ev(2, json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}})),
        ev(1, json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": "start"}})),
    ]}).to_string()
}
fn run(lines: &[&str], routes_log: String) -> (String, Vec<Value>, Vec<String>) {
    let fake = Fake::start(vec![
        ("GET", format!("/v1/code/sessions/session_{SUF}/events"), 200, routes_log),
        ("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[]}"#.into()),
    ]);
    let client = Client::new(Some(&fake.base), "test-token", None).unwrap();
    let (tx, rx) = mpsc::channel();
    for l in lines { tx.send(l.to_string()).unwrap(); }
    drop(tx);
    let mut out = Vec::new();
    attach(&client, &format!("session_{SUF}"), Duration::from_millis(5), &rx, &mut out, Some(50)).unwrap();
    let posts = fake.requests().into_iter().filter(|s| s.method == "POST").map(|s| serde_json::from_str::<Value>(&s.body).unwrap()["events"][0]["payload"].clone()).collect();
    let paths = fake.requests().into_iter().map(|s| s.path).collect();
    (String::from_utf8(out).unwrap(), posts, paths)
}

#[test]
fn prints_conversation_asks_allow_and_sends_the_answer_and_a_user_turn() {
    let (out, posts, paths) = run(&["y", "hello there", "/interrupt"], log());
    assert!(out.contains("polled every") && out.contains("not streamed") && out.contains("WITHOUT stopping"), "{out}");
    assert!(out.contains("user> start") && out.contains("red text") && out.contains("▸ Bash: {\"command\":\"ls\"}") && out.contains("◂ result: file1 (+2 lines)"), "{out}");
    assert!(!out.contains("rate_limit"), "noise is hidden");
    assert!(out.contains("Permission requested: Bash") && out.contains("Allow? [y/n] "), "{out}");
    assert_eq!(out.matches("Allow? [y/n]").count(), 1, "an answered prompt is not asked again");
    assert!(!out.contains('\u{1b}') && !out.contains('\u{7}'), "terminal escapes are stripped");
    assert_eq!(posts.len(), 3);
    assert_eq!(posts[0]["response"]["response"]["behavior"], "allow");
    assert_eq!(posts[0]["response"]["request_id"], "r9");
    assert_eq!((posts[1]["type"].as_str(), posts[1]["message"]["content"].as_str()), (Some("user"), Some("hello there")));
    assert_eq!(posts[2]["request"]["subtype"], "interrupt");
    assert!(paths.iter().all(|p| p.starts_with(&format!("/v1/code/sessions/session_{SUF}/events"))));
}

#[test]
fn n_denies_and_other_input_re_asks_without_sending() {
    let (out, posts, _) = run(&["maybe", "n"], log());
    assert_eq!(out.matches("Allow? [y/n]").count(), 2);
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0]["response"]["response"]["behavior"], "deny");
}

#[test]
fn quit_and_end_of_input_leave_without_posting_anything() {
    let quiet = json!({"data": [ev(1, json!({"type": "user", "uuid": "u1", "message": {"content": "hi"}}))]}).to_string();
    let (out, posts, _) = run(&["", "/quit", "ignored"], quiet.clone());
    assert!(out.contains("left the session running") && posts.is_empty());
    let (out, posts, _) = run(&[], quiet);
    assert!(out.contains("input ended") && posts.is_empty());
}

#[test]
fn first_poll_failure_is_an_error() {
    let fake = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 403, r#"{"message":"untrusted device"}"#.into())]);
    let client = Client::new(Some(&fake.base), "test-token", None).unwrap();
    let (_tx, rx) = mpsc::channel();
    let err = attach(&client, &format!("session_{SUF}"), Duration::from_millis(5), &rx, &mut Vec::new(), Some(1)).unwrap_err();
    assert_eq!(err, remote_sessions_core::api::UNTRUSTED_DEVICE);
}

// ---- live view (events/stream) ----
use remote_sessions_cli::attach::{attach_auto, LiveOpts};

fn sse(seq: u64, payload: Value) -> String { format!("data: {}\n\n", json!({"event_id": format!("e{seq}"), "sequence_num": seq, "payload": payload})) }
fn quiet_log() -> String { json!({"data": [ev(1, json!({"type": "user", "uuid": "u1", "message": {"role": "user", "content": "start"}}))]}).to_string() }

fn live_run(snapshot: String, stream: (u16, String), typed: Vec<(u64, &'static str)>, max_polls: usize, max_failures: u32) -> (String, Vec<Value>, Vec<remote_sessions_core::fakehttp::Seen>) {
    let fake = Fake::start(vec![
        ("GET", format!("/v1/code/sessions/session_{SUF}/events/stream"), stream.0, stream.1),
        ("GET", format!("/v1/code/sessions/session_{SUF}/events"), 200, snapshot),
        ("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[]}"#.into()),
    ]);
    let client = Client::new(Some(&fake.base), "test-token", None).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || { for (after_ms, line) in typed { std::thread::sleep(Duration::from_millis(after_ms)); let _ = tx.send(line.to_string()); } });
    let mut out = Vec::new();
    let live = LiveOpts { retry_delay: Duration::from_millis(20), max_failures, linger: Duration::from_millis(300) };
    attach_auto(&client, &format!("session_{SUF}"), Duration::from_millis(5), &rx, &mut out, &live, Some(max_polls)).unwrap();
    let reqs = fake.requests();
    let posts = reqs.iter().filter(|s| s.method == "POST").map(|s| serde_json::from_str::<Value>(&s.body).unwrap()["events"][0]["payload"].clone()).collect();
    (String::from_utf8(out).unwrap(), posts, reqs)
}

#[test]
fn live_prints_pushed_events_and_resumes_from_the_snapshot_sequence() {
    let body = sse(1, json!({"type": "user", "uuid": "u1", "message": {"content": "start"}})) + &sse(2, json!({"type": "assistant", "uuid": "a9", "message": {"content": [{"type": "text", "text": "\u{1b}[31mpushed hello\u{1b}[0m"}]}}))
        + &format!("data: {}\n\n", json!({"event_id": "x", "status": "DELIVERY_STATUS_PROCESSED", "timestamp": "t"}));
    let (out, posts, reqs) = live_run(quiet_log(), (200, body), vec![(300, "hello there")], 3, 1000);
    assert!(out.contains("live (events/stream)") && out.contains("not token by token") && !out.contains("polled every"), "{out}");
    assert_eq!(out.matches("user> start").count(), 1, "history is not printed twice: {out}");
    assert!(out.contains("claude> ") && out.contains("pushed hello") && !out.contains('\u{1b}'), "{out}");
    assert!(!out.contains("DELIVERY_STATUS"), "receipts are not noise on the screen");
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0]["message"]["content"], "hello there");
    let stream = reqs.iter().find(|r| r.path.contains("/events/stream")).unwrap();
    assert!(stream.path.ends_with("from_sequence_num=1"), "{}", stream.path);
}

#[test]
fn live_asks_for_a_prompt_that_arrives_on_the_stream_and_sends_the_answer() {
    let body = sse(2, json!({"type": "control_request", "request_id": "r7", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}}}));
    let (out, posts, _) = live_run(quiet_log(), (200, body), vec![(600, "y")], 3, 1000);
    assert!(out.contains("Permission requested: Bash") && out.matches("Allow? [y/n]").count() == 1, "{out}");
    // the fake's events route still shows only the quiet log, so answering cannot find the prompt: that is reported, not posted
    assert!(posts.is_empty() && out.contains("failed:"), "{out}");
}

#[test]
fn live_falls_back_to_polling_and_says_so() {
    let (out, _, _) = live_run(log(), (404, r#"{"message":"no stream"}"#.into()), vec![], 3, 2);
    assert!(out.contains("live stream unavailable") && out.contains("falling back to polling"), "{out}");
    assert!(out.contains("polled every") && out.contains("user> start") && out.contains("Permission requested: Bash"), "{out}");
}

#[test]
fn live_first_snapshot_failure_is_an_error() {
    let fake = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 403, r#"{"message":"untrusted device"}"#.into())]);
    let client = Client::new(Some(&fake.base), "test-token", None).unwrap();
    let (_tx, rx) = mpsc::channel();
    let err = attach_auto(&client, &format!("session_{SUF}"), Duration::from_millis(5), &rx, &mut Vec::new(), &LiveOpts::default(), Some(1)).unwrap_err();
    assert_eq!(err, remote_sessions_core::api::UNTRUSTED_DEVICE);
}
