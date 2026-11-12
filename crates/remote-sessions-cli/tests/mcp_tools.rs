//! Every MCP tool, by name: `tools/list` gives it a title and all four annotation hints as booleans
//! (MCP spec 2025-06-18, ToolAnnotations), and `tools/call` checks its arguments. No network, no ssh, no signal:
//! the context has no token, an empty process list and no recordings backend.
use remote_sessions_cli::{mcp, recordings::NoRecordings, Ctx};
use serde_json::{json, Value};
use std::sync::Arc;

const TOOLS: [&str; 12] = [
    "remote_list", "remote_send", "remote_events", "remote_pending", "remote_approve", "remote_deny",
    "remote_say", "remote_stop", "recording_start", "recording_stop", "recording_list", "remote_check",
];

fn ctx() -> Ctx {
    Ctx {
        claude: None,
        api: None,
        token: Box::new(|| Err("no token in tests".into())),
        ps: Box::new(|_| Ok(String::new())),
        signal: Box::new(|_, _| panic!("a test must not signal a worker")),
        env: Box::new(|_| None),
        recordings: Arc::new(NoRecordings),
        transcript: Box::new(|_, _| Err("no transcript in tests".into())),
    }
}

fn listed() -> Vec<Value> {
    let reply = mcp::handle(&ctx(), &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
    reply["result"]["tools"].as_array().unwrap().clone()
}

/// The tool's (readOnly, destructive, idempotent, openWorld) hints; each must be a boolean, and the title must be set twice the same.
fn hints(name: &str) -> (bool, bool, bool, bool) {
    let tool = listed().into_iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("{name} is not listed"));
    let a = &tool["annotations"];
    let title = tool["title"].as_str().unwrap_or_else(|| panic!("{name} has no title"));
    assert!(!title.is_empty());
    assert_eq!(a["title"], title, "{name}: annotations.title must equal title");
    let hint = |k: &str| a[k].as_bool().unwrap_or_else(|| panic!("{name}: {k} must be a boolean"));
    (hint("readOnlyHint"), hint("destructiveHint"), hint("idempotentHint"), hint("openWorldHint"))
}

/// Call a tool; returns (isError, text).
fn call(name: &str, arguments: Value) -> (bool, String) {
    let reply = mcp::handle(&ctx(), &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": arguments}})).unwrap();
    (reply["result"]["isError"].as_bool().unwrap(), reply["result"]["content"][0]["text"].as_str().unwrap().to_string())
}

fn refuses(name: &str, arguments: Value, needle: &str) {
    let (is_error, text) = call(name, arguments);
    assert!(is_error, "{name} must refuse: {text}");
    assert!(text.contains(needle), "{name}: expected {needle:?} in {text}");
}

#[test]
fn lists_exactly_the_twelve_tools() {
    let names: Vec<String> = listed().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, TOOLS);
}

#[test]
fn remote_list() {
    assert_eq!(hints("remote_list"), (true, false, true, true));
    let (is_error, text) = call("remote_list", json!({}));
    assert!(!is_error, "{text}");
}

#[test]
fn remote_send() {
    assert_eq!(hints("remote_send"), (false, false, false, true));
    refuses("remote_send", json!({"session": "session_01AbCdEfGh"}), "message is required");
}

#[test]
fn remote_events() {
    assert_eq!(hints("remote_events"), (true, false, true, true));
    refuses("remote_events", json!({}), "session is required");
}

#[test]
fn remote_pending() {
    assert_eq!(hints("remote_pending"), (true, false, true, true));
    refuses("remote_pending", json!({}), "session is required");
}

#[test]
fn remote_approve() {
    assert_eq!(hints("remote_approve"), (false, true, false, true));
    refuses("remote_approve", json!({}), "session is required");
}

#[test]
fn remote_deny() {
    assert_eq!(hints("remote_deny"), (false, true, false, true));
    refuses("remote_deny", json!({"message": "no"}), "session is required");
}

#[test]
fn remote_say() {
    assert_eq!(hints("remote_say"), (false, false, false, true));
    refuses("remote_say", json!({"session": "session_01AbCdEfGh"}), "text is required");
}

#[test]
fn remote_stop() {
    assert_eq!(hints("remote_stop"), (false, true, false, true));
    // Without confirm=true it refuses before any signal (the test context panics on a signal).
    refuses("remote_stop", json!({"session": "session_01AbCdEfGh"}), "confirm=true is required");
    refuses("remote_stop", json!({"session": "session_01AbCdEfGh", "confirm": "true"}), "confirm=true is required");
}

#[test]
fn recording_start() {
    assert_eq!(hints("recording_start"), (false, false, false, true));
    refuses("recording_start", json!({"kind": "pane", "host": "box"}), "session is required");
}

#[test]
fn recording_stop() {
    assert_eq!(hints("recording_stop"), (false, false, true, true));
    refuses("recording_stop", json!({}), "id is required");
}

#[test]
fn recording_list() {
    assert_eq!(hints("recording_list"), (true, false, true, true));
    let (is_error, text) = call("recording_list", json!({}));
    assert!(!is_error, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["recordings"], json!([]));
}

#[test]
fn remote_check() {
    assert_eq!(hints("remote_check"), (true, false, true, false));
    let (is_error, text) = call("remote_check", json!({}));
    assert!(!is_error, "{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["eligible"], true);
}

/// Call a tool with this value of REMOTE_SESSIONS_MCP_APPROVE; returns (isError, text).
fn call_with_approve_env(name: &str, arguments: Value, value: Option<&'static str>) -> (bool, String) {
    let mut c = ctx();
    c.env = Box::new(move |k| if k == mcp::MCP_APPROVE_ENV { value.map(str::to_string) } else { None });
    let reply = mcp::handle(&c, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name, "arguments": arguments}})).unwrap();
    (reply["result"]["isError"].as_bool().unwrap(), reply["result"]["content"][0]["text"].as_str().unwrap().to_string())
}

#[test]
fn approve_and_deny_through_mcp_do_nothing_by_default() {
    // Off (unset, or any value but 1): no token is read, no call is made, and the text gives the user's command.
    for value in [None, Some("0"), Some("true"), Some("yes"), Some("")] {
        for (tool, verb) in [("remote_approve", "approve"), ("remote_deny", "deny")] {
            let (is_error, text) = call_with_approve_env(tool, json!({"session": "session_01ExampleAAAAAAAAAAAAAAA", "request": "r1"}), value);
            assert!(is_error, "{tool} with {value:?} must not act: {text}");
            assert!(text.contains(&format!("remote-sessions {verb} session_01ExampleAAAAAAAAAAAAAAA --request r1")), "{text}");
            assert!(text.contains("REMOTE_SESSIONS_MCP_APPROVE=1") && text.contains("cannot answer a permission prompt by itself"), "{text}");
            assert!(!text.contains("no token in tests"), "{tool} must not reach the API: {text}");
        }
    }
    let (_, text) = call_with_approve_env("remote_approve", json!({"session": "session_01ExampleAAAAAAAAAAAAAAA"}), None);
    assert!(text.contains("/remote-approve session_01ExampleAAAAAAAAAAAAAAA") && !text.contains("--request"), "{text}");
    // A request id that is not a plain id is not echoed into the command.
    let (_, text) = call_with_approve_env("remote_deny", json!({"session": "session_01ExampleAAAAAAAAAAAAAAA", "request": "r1; rm -rf ~"}), None);
    assert!(!text.contains("rm -rf") && text.contains("remote-sessions deny session_01ExampleAAAAAAAAAAAAAAA [--message <why>]"), "{text}");
    let (is_error, text) = call_with_approve_env("remote_approve", json!({"session": "https://evil.example/code/session_01AbCdEf"}), None);
    assert!(is_error && !text.contains("evil.example/code"), "{text}");
}

#[test]
fn approve_and_deny_through_mcp_act_only_with_the_opt_in() {
    // With 1 the tool goes on to the API: here it stops at the missing token, which proves it passed the gate.
    for tool in ["remote_approve", "remote_deny"] {
        let (is_error, text) = call_with_approve_env(tool, json!({"session": "session_01ExampleAAAAAAAAAAAAAAA", "request": "r1"}), Some("1"));
        assert!(is_error && text.contains("no token in tests"), "{tool}: {text}");
    }
}
