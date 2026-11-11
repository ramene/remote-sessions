//! From cloud-sessions (same authors): the condensed event log of a session as clean, bounded text entries. All of it is untrusted.
use serde::Serialize;
use serde_json::Value;

pub const EVENT_TEXT: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    /// The event's own id (`uuid`, `id` or `event_id`), or a stable hash when it has none.
    pub id: String,
    /// The raw `type`: user, assistant, tool_use, tool_result, result, …
    pub kind: String,
    /// A message the user sent (as opposed to the remote run's own output).
    pub user: bool,
    /// Plain text, control and bidi characters removed, capped at 8 KB. Never executed.
    pub text: String,
    /// Milliseconds since the epoch, when the event carries a timestamp.
    pub at: Option<i64>,
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x100000001b3))
}
pub fn clean(text: &str, limit: usize) -> String {
    let text: String = text.chars().filter(|c| !(c.is_control() && *c != '\n' && *c != '\t') && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).collect();
    if text.len() > limit { text.chars().take(limit / 4).collect::<String>() + "\n…" } else { text }
}
pub fn one_line(text: &str, limit: usize) -> String {
    let line = clean(text, limit * 4).split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > limit { line.chars().take(limit).collect::<String>() + "…" } else { line }
}
/// The first non-empty line of a tool's output, capped, with how many more lines there were.
fn first_line(text: &str, limit: usize) -> String {
    let text = clean(text, limit * 40);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = one_line(lines.next().unwrap_or(""), limit);
    match lines.count() { 0 => first, more => format!("{first} (+{more} lines)") }
}
/// Plain text of a message `content`: a string, or blocks (text, tool_use, tool_result).
fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().map(|block| match block["type"].as_str().unwrap_or("text") {
            "tool_use" => format!("▸ {}: {}", block["name"].as_str().unwrap_or("tool"), one_line(&block["input"].to_string(), 160)),
            "tool_result" => format!("◂ {}: {}", if block["is_error"] == true { "error" } else { "result" }, first_line(&content_text(&block["content"]), 200)),
            "thinking" => String::new(),
            _ => block["text"].as_str().unwrap_or("").to_string(),
        }).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}
fn has_block(content: &Value, kind: &str) -> bool {
    content.as_array().is_some_and(|blocks| blocks.iter().any(|b| b["type"] == kind))
}
fn secs(ms: &Value) -> Option<String> { ms.as_f64().map(|m| format!("{:.1}s", m / 1000.0)) }

/// The list inside an API response: a bare array, `{data: […]}` or `{events: […]}`. Cloud sessions return the
/// newest event first; when every event has a `sequence_num` the list is put back in order, oldest first.
pub fn event_list(body: &Value) -> Vec<Value> {
    let mut list: Vec<Value> = body.as_array().or_else(|| body["data"].as_array()).or_else(|| body["events"].as_array()).cloned().unwrap_or_default();
    let seq = |e: &Value| e["sequence_num"].as_u64().or_else(|| e["sequence_num"].as_str().and_then(|s| s.parse().ok()));
    if !list.is_empty() && list.iter().all(|e| seq(e).is_some()) { list.sort_by_key(|e| seq(e)); }
    list
}

/// What a non-conversation event says, in one short line: provisioning logs, system notices, results.
fn notice(kind: &str, e: &Value) -> Option<String> {
    Some(match kind {
        "env_manager_log" => format!("env[{}] {}", e["data"]["level"].as_str().unwrap_or("info"), e["data"]["content"].as_str()?),
        "system" => {
            let sub = e["subtype"].as_str().unwrap_or("notice");
            match sub {
                "init" => format!("system init: {} · {} · claude {}", e["model"].as_str().unwrap_or("?"), e["cwd"].as_str().unwrap_or("?"), e["claude_code_version"].as_str().unwrap_or("?")),
                _ => format!("system {sub}{}", e["status"].as_str().map(|s| format!(": {s}")).unwrap_or_default()),
            }
        }
        "rate_limit_event" => format!("rate_limit: {} ({})", e["rate_limit_info"]["status"].as_str().unwrap_or("?"), e["rate_limit_info"]["rateLimitType"].as_str().unwrap_or("?")),
        "autocompact_state" => format!("autocompact: {}, threshold {}", if e["value"]["enabled"] == true { "on" } else { "off" }, e["value"]["threshold"]),
        "control_request" => format!("control_request: {} {}", e["request"]["subtype"].as_str().unwrap_or("?"), e["request"]["mode"].as_str().unwrap_or("")).trim_end().to_string(),
        "control_response" => format!("control_response: {}", e["response"]["subtype"].as_str().unwrap_or("?")),
        _ => return None,
    })
}

/// One event object → an entry (None when it has no text). Two shapes are understood: the cloud-sessions envelope
/// `{event_id, event_type, created_at, sequence_num, payload: {…}}` and the bare event (`type`, `message`, …).
pub fn parse_event(envelope: &Value) -> Option<Event> {
    let event = if envelope["payload"].is_object() { &envelope["payload"] } else { envelope };
    let mut kind = envelope["event_type"].as_str().or(event["type"].as_str()).unwrap_or("event");
    let raw = ["uuid", "id", "event_id"].iter().find_map(|key| event[*key].as_str().or(envelope[*key].as_str())).map(str::to_string).unwrap_or_else(|| format!("h{:016x}", fnv(&envelope.to_string())));
    let id: String = raw.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '_' | '-' | ':' | '.')).take(100).collect();
    let id = if id.is_empty() { format!("h{:016x}", fnv(&envelope.to_string())) } else { id };
    let body_text = [&event["text"], &event["message"]["content"], &event["content"], &event["result"]].into_iter().map(content_text).find(|t| !t.trim().is_empty()).unwrap_or_default();
    // A "user" event made only of tool output is the tool's answer, not something a person said.
    if kind == "user" && has_block(&event["message"]["content"], "tool_result") { kind = "tool_result"; }
    let text = match kind {
        "user" | "assistant" => body_text,
        "tool_use" => format!("▸ {}: {}", event["name"].as_str().or(event["tool_name"].as_str()).unwrap_or("tool"), one_line(&event["input"].to_string(), 160)),
        "tool_result" if event["message"]["content"].is_array() => body_text,
        "tool_result" => format!("◂ result: {}", first_line(&body_text, 200)),
        "result" => {
            let head: Vec<String> = [event["subtype"].as_str().map(str::to_string), event["num_turns"].as_u64().map(|n| format!("{n} turns")), secs(&event["duration_ms"]), event["total_cost_usd"].as_f64().map(|c| format!("${c:.3}"))].into_iter().flatten().collect();
            let answer = event["result"].as_str().unwrap_or("").trim();
            match (head.is_empty(), answer.is_empty()) {
                (true, _) => format!("· result{}", if body_text.is_empty() { String::new() } else { format!(": {}", one_line(&body_text, 300)) }),
                (false, true) => format!("· result: {}", head.join(" · ")),
                (false, false) => format!("· result: {}\n{}", head.join(" · "), answer),
            }
        }
        other => match notice(other, event) {
            Some(line) => format!("· {line}"),
            None => format!("· {other}{}", if body_text.is_empty() { String::new() } else { format!(": {}", one_line(&body_text, 300)) }),
        },
    };
    let text = clean(text.trim(), EVENT_TEXT);
    if text.is_empty() { return None; }
    let at = [&envelope["created_at"], &event["timestamp"], &event["created_at"], &event["ts"]].into_iter().find_map(|t| t.as_str()).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|d| d.timestamp_millis());
    Some(Event { id, kind: clean(kind, 40), user: kind == "user", text, at })
}

/// A response body → entries, oldest first.
pub fn parse_events(body: &Value) -> Vec<Event> {
    event_list(body).iter().filter_map(parse_event).collect()
}

/// Whether the log's last event is a `result` (the run is over until someone sends again).
pub fn finished(body: &Value) -> bool {
    event_list(body).last().is_some_and(|e| e["event_type"] == "result" || e["type"] == "result")
}

/// One structured item of an event payload: what a UI needs where `parse_event` flattens to text.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// Plain text of a user or assistant message (or a text content block).
    Text(String),
    /// The model asks to run a tool.
    ToolUse { id: String, name: String, input: Value },
    /// A tool's answer. `text` is the plain text of its content.
    ToolResult { tool_use_id: String, text: String, is_error: bool },
    /// Reasoning blocks carry no readable content; they only show activity.
    Thinking,
    /// `control_request`: `subtype` is `can_use_tool`, `set_permission_mode`, `interrupt`, ...
    ControlRequest { request_id: String, subtype: String, tool_name: String, input: Value, mode: Option<String> },
    /// `control_response`: closes the request with this id.
    ControlResponse { request_id: String },
    /// `control_cancel_request`: withdraws the request with this id.
    ControlCancel { request_id: String },
}

/// Plain text of a tool_result `content` (a string or text blocks).
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// The structured blocks of an event (an envelope `{payload: {...}}` or a bare payload), in order. Text is NOT
/// cleaned here; callers clean what they draw. Event kinds without blocks (result, system, ...) give an empty list.
pub fn blocks(envelope: &Value) -> Vec<Block> {
    let p = if envelope["payload"].is_object() { &envelope["payload"] } else { envelope };
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    match p["type"].as_str().unwrap_or("") {
        "user" | "assistant" => match &p["message"]["content"] {
            Value::String(t) => vec![Block::Text(t.clone())],
            Value::Array(items) => items.iter().filter_map(|b| match b["type"].as_str().unwrap_or("text") {
                "text" => b["text"].as_str().map(|t| Block::Text(t.to_string())),
                "tool_use" => Some(Block::ToolUse { id: s(&b["id"]), name: s(&b["name"]), input: b["input"].clone() }),
                "tool_result" => Some(Block::ToolResult { tool_use_id: s(&b["tool_use_id"]), text: result_text(&b["content"]), is_error: b["is_error"] == true }),
                "thinking" | "redacted_thinking" => Some(Block::Thinking),
                _ => None,
            }).collect(),
            _ => Vec::new(),
        },
        "control_request" => vec![Block::ControlRequest { request_id: s(&p["request_id"]), subtype: s(&p["request"]["subtype"]), tool_name: s(&p["request"]["tool_name"]), input: p["request"]["input"].clone(), mode: p["request"]["mode"].as_str().map(str::to_string) }],
        "control_response" => vec![Block::ControlResponse { request_id: s(&p["response"]["request_id"]) }],
        "control_cancel_request" => vec![Block::ControlCancel { request_id: s(&p["request_id"]) }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod blocks_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assistant_text_and_tool_use_are_structured() {
        let b = blocks(&json!({"payload": {"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "On it."}, {"type": "tool_use", "id": "toolu_ex1", "name": "Bash", "input": {"command": "ls"}}]}}}));
        assert_eq!(b, vec![Block::Thinking, Block::Text("On it.".into()), Block::ToolUse { id: "toolu_ex1".into(), name: "Bash".into(), input: json!({"command": "ls"}) }]);
    }
    #[test]
    fn tool_results_keep_their_link_error_flag_and_text() {
        let b = blocks(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_ex1", "is_error": true, "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}]}}));
        assert_eq!(b, vec![Block::ToolResult { tool_use_id: "toolu_ex1".into(), text: "a\nb".into(), is_error: true }]);
        let plain = blocks(&json!({"type": "user", "message": {"content": "hi"}}));
        assert_eq!(plain, vec![Block::Text("hi".into())]);
    }
    #[test]
    fn control_events_are_structured() {
        let ask = json!({"type": "control_request", "request_id": "req_ex1", "request": {"subtype": "can_use_tool", "tool_name": "WebFetch", "input": {"url": "https://example.com/"}}});
        assert_eq!(blocks(&ask), vec![Block::ControlRequest { request_id: "req_ex1".into(), subtype: "can_use_tool".into(), tool_name: "WebFetch".into(), input: json!({"url": "https://example.com/"}), mode: None }]);
        let mode = blocks(&json!({"type": "control_request", "request_id": "r2", "request": {"subtype": "set_permission_mode", "mode": "plan"}}));
        assert!(matches!(&mode[0], Block::ControlRequest { mode: Some(m), .. } if m == "plan"));
        assert_eq!(blocks(&json!({"type": "control_response", "response": {"request_id": "req_ex1"}})), vec![Block::ControlResponse { request_id: "req_ex1".into() }]);
        assert_eq!(blocks(&json!({"type": "control_cancel_request", "request_id": "req_ex1"})), vec![Block::ControlCancel { request_id: "req_ex1".into() }]);
        assert!(blocks(&json!({"type": "result"})).is_empty() && blocks(&json!({"nope": 1})).is_empty());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn sample() -> Value {
        json!({"data": [
            {"uuid": "e1", "type": "user", "message": {"role": "user", "content": "Fix the flaky test"}},
            {"uuid": "e2", "type": "assistant", "message": {"content": [{"type": "text", "text": "On it."}, {"type": "tool_use", "name": "Bash", "input": {"command": "npm test"}}]}},
            {"uuid": "e3", "type": "tool_result", "content": "1 failing\u{202e}"},
            {"uuid": "e4", "type": "result", "result": "Opened a pull request"},
        ]})
    }
    #[test]
    fn events_become_clean_one_line_entries() {
        let parsed = parse_events(&sample());
        assert_eq!(parsed.len(), 4);
        assert!(parsed[0].user && parsed[0].text == "Fix the flaky test");
        assert_eq!(parsed[1].text, "On it.\n▸ Bash: {\"command\":\"npm test\"}");
        assert_eq!(parsed[2].text, "◂ result: 1 failing");
        assert!(parsed[3].text.starts_with("· result: Opened"));
        assert!(finished(&sample()));
        assert!(parse_events(&json!({"nope": 1})).is_empty());
    }
    const CLOUD: &str = include_str!("../tests/fixtures/cloud-events-session.json");
    fn cloud() -> Value { serde_json::from_str(CLOUD).unwrap() }

    #[test]
    fn cloud_envelopes_decode_to_real_lines_in_order_and_never_to_a_bare_event_label() {
        let parsed = parse_events(&cloud());
        assert_eq!(parsed.len(), 13);
        assert!(parsed.iter().all(|e| e.text != "· event" && e.kind != "event"));
        let kinds: Vec<&str> = parsed.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds[..3], ["control_request", "user", "env_manager_log"], "sorted oldest first by sequence_num");
        assert_eq!(*kinds.last().unwrap(), "result");
        assert!(finished(&cloud()));
        let by = |k: &str| parsed.iter().find(|e| e.kind == k).unwrap();
        assert!(by("user").user && by("user").text.starts_with("Example task prompt."));
        assert_eq!(by("env_manager_log").text, "· env[info] Allocating sandbox");
        assert_eq!(by("control_request").text, "· control_request: set_permission_mode auto");
        assert_eq!(by("rate_limit_event").text, "· rate_limit: allowed (seven_day)");
        assert!(by("tool_result").text.starts_with("◂ result: example tool output") && !by("tool_result").user);
        assert!(parsed.iter().any(|e| e.text.starts_with("▸ Bash: {\"command\":\"echo")));
        assert!(parsed.iter().any(|e| e.kind == "assistant" && e.text.starts_with("tmux is present")));
        assert!(parsed.iter().any(|e| e.text.starts_with("· system init: claude-sonnet-5-5 · /home/user/example-project · claude 2.1.319")));
        assert!(parsed.iter().any(|e| e.text == "· system permission_check_status: checking"));
        let result = by("result");
        assert!(result.text.starts_with("· result: success · 3 turns · 27.3s · $0.155\nExample result text."), "{}", result.text);
        assert_eq!(result.at, Some(1704226999652), "the envelope's created_at is the time");
    }
    #[test]
    fn unknown_types_keep_their_own_label_and_tool_errors_say_so() {
        let odd = parse_event(&json!({"event_type": "brand_new_thing", "created_at": "2024-01-02T20:22:48Z", "payload": {"type": "brand_new_thing", "x": 1}})).unwrap();
        assert_eq!(odd.text, "· brand_new_thing");
        let err = parse_event(&json!({"type": "user", "message": {"content": [{"type": "tool_result", "is_error": true, "content": "nope\nsecond"}]}})).unwrap();
        assert_eq!((err.kind.as_str(), err.text.as_str()), ("tool_result", "◂ error: nope (+1 lines)"));
    }
    #[test]
    fn ids_are_stable_without_an_id_and_hostile_text_is_capped() {
        let event = json!({"type": "assistant", "text": "x".repeat(100_000)});
        let (a, b) = (parse_event(&event).unwrap(), parse_event(&event).unwrap());
        assert_eq!(a.id, b.id);
        assert!(a.text.len() < 4000);
    }
}
