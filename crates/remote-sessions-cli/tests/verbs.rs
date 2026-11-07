//! The CLI verbs end to end through `run`, against a fake backend.
use remote_sessions_cli::{ctx_from_env, run};
use remote_sessions_core::fakehttp::Fake;
use serde_json::{json, Value};

const SUF: &str = "01VerbsTest123456";
fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

#[test]
fn pending_approve_deny_say_interrupt_verbs() {
    let log = json!({"data": [{"sequence_num": 2, "payload": {"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool", "tool_name": "Edit", "input": {"file": "a"}}}}]}).to_string();
    let fake = Fake::start(vec![
        ("GET", format!("/v1/code/sessions/session_{SUF}/events"), 200, log),
        ("POST", format!("/v1/code/sessions/session_{SUF}/events"), 200, r#"{"results":[{"event_id":"x","sequence_num":3}]}"#.into()),
    ]);
    let mut ctx = ctx_from_env();
    ctx.api = Some(fake.base.clone());
    ctx.token = Box::new(|| Ok("test-token".into()));
    let id = format!("session_{SUF}");
    let none = &mut std::io::empty();
    let p = run(&ctx, "pending", &[id.clone()], none).unwrap();
    assert_eq!((p["pending"][0]["request_id"].as_str(), p["pending"][0]["tool_name"].as_str()), (Some("r1"), Some("Edit")));
    assert!(p["untrusted"].as_str().unwrap().contains("untrusted"));
    let a = run(&ctx, "approve", &s(&[&id, "--request", "r1"]), none).unwrap();
    assert_eq!((a["ok"].as_bool(), a["decision"].as_str()), (Some(true), Some("allow")));
    let d = run(&ctx, "deny", &s(&[&id, "--message", "not now"]), none).unwrap();
    assert_eq!(d["decision"], "deny");
    run(&ctx, "say", &s(&[&id, "hello", "world"]), none).unwrap();
    run(&ctx, "say", &s(&[&id, "-"]), &mut "from stdin\n".as_bytes()).unwrap();
    run(&ctx, "interrupt", &[id.clone()], none).unwrap();
    assert!(run(&ctx, "say", &[id.clone()], none).is_err());
    assert!(run(&ctx, "attach", &[id.clone()], none).is_err());
    let posts: Vec<Value> = fake.requests().into_iter().filter(|r| r.method == "POST").map(|r| serde_json::from_str::<Value>(&r.body).unwrap()["events"][0]["payload"].clone()).collect();
    assert_eq!(posts.len(), 5);
    assert_eq!(posts[1]["response"]["response"]["message"], "not now");
    assert_eq!(posts[2]["message"]["content"], "hello world");
    assert_eq!(posts[3]["message"]["content"], "from stdin");
    assert_eq!(posts[4]["request"]["subtype"], "interrupt");
}
