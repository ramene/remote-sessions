//! Session control against a fake loopback HTTP backend: pending detection, exact JSON bodies, id fallback, 403.
use remote_sessions_core::api::{Client, UNTRUSTED_DEVICE};
use remote_sessions_core::control::{pending_in, Decision};
use remote_sessions_core::fakehttp::Fake;
use serde_json::{json, Value};

const SUF: &str = "01TestSession1234";

fn ev(seq: u64, payload: Value) -> Value { json!({"event_id": format!("e{seq}"), "sequence_num": seq, "payload": payload}) }
fn req(id: &str, tool: &str, input: Value) -> Value { json!({"type": "control_request", "request_id": id, "request": {"subtype": "can_use_tool", "tool_name": tool, "input": input}}) }

/// Newest first, like sort_order=desc. r3 is pending; r1 was answered; r2 was cancelled; one request is not a tool prompt.
fn log() -> String {
    json!({"data": [
        ev(10, req("r3", "WebFetch", json!({"url": "https://example.com"}))),
        ev(9, json!({"type": "control_cancel_request", "request_id": "r2"})),
        ev(8, req("r2", "Bash", json!({"command": "ls"}))),
        ev(7, json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1", "response": {"behavior": "allow"}}})),
        ev(6, req("r1", "Bash", json!({"command": "pwd"}))),
        ev(5, json!({"type": "control_request", "request_id": "m1", "request": {"subtype": "set_permission_mode", "mode": "auto"}})),
    ]}).to_string()
}
fn client(fake: &Fake) -> Client { Client::new(Some(&fake.base), "test-token", None).unwrap() }
fn events_route(status: u16, body: &str) -> Vec<(&'static str, String, u16, String)> { vec![("GET", format!("/v1/code/sessions/session_{SUF}/events"), status, body.to_string())] }
/// The body with uuids checked for shape and replaced, so the rest can be compared exactly.
fn body_of(seen: &remote_sessions_core::fakehttp::Seen) -> Value {
    let mut v: Value = serde_json::from_str(&seen.body).unwrap();
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => for (k, x) in m.iter_mut() {
                if k == "uuid" || k == "request_id" && x.as_str().is_some_and(|s| s.len() == 36) {
                    let s = x.as_str().unwrap();
                    assert!(s.len() == 36 && s.as_bytes()[14] == b'4' && s.matches('-').count() == 4, "uuid4 shape: {s}");
                    *x = json!("<uuid>");
                } else { walk(x) }
            },
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

#[test]
fn pending_skips_answered_cancelled_and_non_tool_requests() {
    let p = pending_in(&serde_json::from_str(&log()).unwrap());
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].request_id.as_str(), p[0].tool_name.as_str()), ("r3", "WebFetch"));
    assert_eq!(p[0].input, json!({"url": "https://example.com"}));
    // Oldest-first lists and bare (payload-less) events work too, and the newest unanswered prompt comes first.
    let bare = json!([req("a", "Bash", json!({})), req("b", "Edit", json!({})), {"type": "control_response", "response": {"request_id": "a"}}]);
    assert!(pending_in(&json!([{"type": "control_response", "response": {"request_id": "a"}}])).is_empty());
    assert_eq!(pending_in(&bare).iter().map(|p| p.request_id.as_str()).collect::<Vec<_>>(), ["b"]);
    assert!(pending_in(&json!({"nope": 1})).is_empty());
}

#[test]
fn pending_prompts_reads_the_newest_100_events_descending() {
    let fake = Fake::start(events_route(200, &log()));
    let prompts = client(&fake).pending_prompts(&format!("session_{SUF}")).unwrap();
    assert_eq!(prompts.len(), 1);
    let seen = fake.requests();
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", format!("/v1/code/sessions/session_{SUF}/events?limit=100&sort_order=desc").as_str()));
    let h = |k: &str| seen[0].headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    assert_eq!(h("authorization").as_deref(), Some("Bearer test-token"));
    assert_eq!(h("anthropic-version").as_deref(), Some("2023-06-01"));
    assert_eq!(h("anthropic-client-platform").as_deref(), Some("cli"));
}

#[test]
fn approve_posts_the_exact_allow_body() {
    let mut routes = events_route(200, &log());
    routes.push(("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[{"event_id":"x","sequence_num":11}]}"#.into()));
    let fake = Fake::start(routes);
    let done = client(&fake).answer(&format!("cse_{SUF}"), None, &Decision::Allow).unwrap();
    assert_eq!((done["request_id"].as_str(), done["decision"].as_str()), (Some("r3"), Some("allow")));
    assert_eq!(done["results"][0]["sequence_num"], 11);
    let post = fake.requests().into_iter().find(|s| s.method == "POST").unwrap();
    assert_eq!(post.path, format!("/v1/code/sessions/session_{SUF}/events"));
    assert_eq!(post.headers.iter().find(|(n, _)| n == "content-type").map(|(_, v)| v.as_str()), Some("application/json"));
    assert_eq!(body_of(&post), json!({"events": [{"payload": {"type": "control_response", "uuid": "<uuid>", "response": {"subtype": "success", "request_id": "r3", "response": {"behavior": "allow", "updatedInput": {"url": "https://example.com"}, "tool_name": "WebFetch"}}}}]}));
}

#[test]
fn deny_posts_the_exact_deny_body_with_default_or_given_message() {
    let mut routes = events_route(200, &log());
    routes.push(("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[]}"#.into()));
    let fake = Fake::start(routes);
    let c = client(&fake);
    c.answer(&format!("session_{SUF}"), Some("r3"), &Decision::Deny(Some("no thanks".into()))).unwrap();
    c.answer(&format!("session_{SUF}"), None, &Decision::Deny(None)).unwrap();
    let posts: Vec<_> = fake.requests().into_iter().filter(|s| s.method == "POST").collect();
    let want = |msg: &str| json!({"events": [{"payload": {"type": "control_response", "uuid": "<uuid>", "response": {"subtype": "success", "request_id": "r3", "response": {"behavior": "deny", "message": msg}}}}]});
    assert_eq!(body_of(&posts[0]), want("no thanks"));
    assert_eq!(body_of(&posts[1]), want("Denied from another machine (remote-sessions)."));
}

#[test]
fn answering_a_prompt_that_is_not_pending_posts_nothing() {
    let fake = Fake::start(events_route(200, &log()));
    let c = client(&fake);
    for rid in ["r1", "r2", "nope"] {
        assert!(c.answer(&format!("session_{SUF}"), Some(rid), &Decision::Allow).unwrap_err().contains("No pending permission prompt"), "{rid}");
    }
    fake.set("GET", &format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"data":[]}"#);
    assert!(c.answer(&format!("session_{SUF}"), None, &Decision::Allow).is_err());
    assert!(fake.requests().iter().all(|s| s.method == "GET"));
}

#[test]
fn say_posts_a_real_user_turn() {
    let fake = Fake::start(vec![("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[{"event_id":"u","sequence_num":3}]}"#.into())]);
    let c = client(&fake);
    let done = c.say(&format!("https://claude.ai/code/session_{SUF}"), "hello \"there\"\nline 2").unwrap();
    assert_eq!(done["results"][0]["sequence_num"], 3);
    let post = &fake.requests()[0];
    assert_eq!(body_of(post), json!({"events": [{"payload": {"uuid": "<uuid>", "session_id": format!("session_{SUF}"), "type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": "hello \"there\"\nline 2"}}}]}));
    assert!(c.say(&format!("session_{SUF}"), "  ").is_err());
}

#[test]
fn interrupt_posts_the_documented_control_request() {
    let fake = Fake::start(vec![("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[]}"#.into())]);
    client(&fake).interrupt(&format!("session_{SUF}")).unwrap();
    let post = &fake.requests()[0];
    assert_eq!(body_of(post), json!({"events": [{"payload": {"type": "control_request", "request_id": "<uuid>", "uuid": "<uuid>", "request": {"subtype": "interrupt"}}}]}));
    let raw: Value = serde_json::from_str(&post.body).unwrap();
    assert_ne!(raw["events"][0]["payload"]["request_id"], raw["events"][0]["payload"]["uuid"]);
}

#[test]
fn id_falls_back_from_session_to_cse_for_reads_and_writes() {
    let fake = Fake::start(vec![
        ("GET", format!("/v1/code/sessions/session_{SUF}/events"), 404, r#"{"error":{"message":"not found"}}"#.into()),
        ("GET", format!("/v1/code/sessions/cse_{SUF}/events"), 200, log()),
        ("POST", format!("/v1/code/sessions/session_{SUF}/events"), 404, "{}".into()),
        ("POST", format!("/v1/code/sessions/cse_{SUF}/events"), 200, r#"{"results":[]}"#.into()),
    ]);
    let c = client(&fake);
    c.answer(&format!("session_{SUF}"), None, &Decision::Allow).unwrap();
    c.say(&format!("session_{SUF}"), "hi").unwrap();
    let paths: Vec<String> = fake.requests().iter().map(|s| format!("{} {}", s.method, s.path.split('?').next().unwrap())).collect();
    assert_eq!(paths, [
        format!("GET /v1/code/sessions/session_{SUF}/events"), format!("GET /v1/code/sessions/cse_{SUF}/events"), format!("POST /v1/code/sessions/cse_{SUF}/events"),
        format!("POST /v1/code/sessions/session_{SUF}/events"), format!("POST /v1/code/sessions/cse_{SUF}/events"),
    ]);
    let last = fake.requests().pop().unwrap();
    assert_eq!(body_of(&last)["events"][0]["payload"]["session_id"], format!("cse_{SUF}"), "the user turn names the id form that worked");
}

#[test]
fn untrusted_device_403_prints_a_clear_sentence_and_no_secret() {
    for body in [r#"{"error":{"type":"permission_error","message":"Request from untrusted device"}}"#, r#"{"message":"This is not a trusted device"}"#] {
        let fake = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 403, body.into()), ("POST", "/v1/code/sessions/".into(), 403, body.into())]);
        let c = client(&fake);
        for err in [c.pending_prompts(&format!("session_{SUF}")).unwrap_err(), c.say(&format!("session_{SUF}"), "x").unwrap_err()] {
            assert_eq!(err, UNTRUSTED_DEVICE);
            assert!(err.contains("untrusted device") && err.contains("signing") && !err.contains("test-token"));
        }
    }
    let fake = Fake::start(vec![("GET", "/v1/code/sessions/".into(), 403, r#"{"error":"nope"}"#.into())]);
    let err = client(&fake).pending_prompts(&format!("session_{SUF}")).unwrap_err();
    assert!(err.contains("refused the credentials") && !err.contains("untrusted device"), "{err}");
}
