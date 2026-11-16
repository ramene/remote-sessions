//! s554: what a scanner does in a CLEAN environment. The server is started with an empty HOME and no inherited
//! environment (no claude.ai login, no token, no PATH beyond the system's), it gets `initialize` and `tools/list`,
//! and every tool must be listed with the four hints as booleans. No login and no network are needed for that: auth is
//! checked only when a tool is called.
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

const TOOLS: [&str; 12] = [
    "remote_list", "remote_send", "remote_events", "remote_pending", "remote_approve", "remote_deny",
    "remote_say", "remote_stop", "recording_start", "recording_stop", "recording_list", "remote_check",
];

#[test]
fn a_clean_start_lists_every_tool_with_all_four_hints() {
    let home = std::env::temp_dir().join(format!("remote-sessions-clean-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_remote-sessions"))
        .arg("mcp")
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "scan", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ];
    {
        let mut stdin = child.stdin.take().unwrap();
        for message in input {
            writeln!(stdin, "{message}").unwrap();
        }
    } // stdin closed: the server ends after its last reply
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&home);
    let replies: Vec<Value> = String::from_utf8(out.stdout).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(replies.len(), 2, "initialize and tools/list are answered; the notification is not");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "remote-sessions");
    let tools = replies[1]["result"]["tools"].as_array().expect("tools/list gives the tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOLS);
    for tool in tools {
        let a = &tool["annotations"];
        for hint in ["readOnlyHint", "destructiveHint", "idempotentHint", "openWorldHint"] {
            assert!(a[hint].is_boolean(), "{}: {hint} must be an explicit boolean", tool["name"]);
        }
    }
}
