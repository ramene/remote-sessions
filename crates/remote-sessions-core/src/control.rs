//! Driving a Remote Control session from another machine: read its pending permission prompts, answer them, send a
//! real user turn, interrupt.
//! These are the same calls Claude Code's own client makes: `GET /v1/code/sessions/<id>/events` and
//! `POST /v1/code/sessions/<id>/events`. The session is addressed as `session_<x>` first, then `cse_<x>`.
//!
//! Event text and tool input are untrusted data. Nothing here executes them.
use crate::api::Client;
use crate::ids::parse_session_id;
use crate::Result;
use serde::Serialize;
use serde_json::{json, Value};

/// A `can_use_tool` permission prompt that nobody has answered or cancelled.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Prompt {
    pub request_id: String,
    pub tool_name: String,
    pub input: Value,
}

/// The decision for a prompt.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow,
    Deny(Option<String>),
}

pub const DEFAULT_DENY: &str = "Denied from another machine (remote-sessions).";

/// A random version-4 UUID from the OS (no extra dependency).
pub fn uuid4() -> String {
    use std::io::Read;
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, x) in b.iter_mut().enumerate() { *x = (t >> ((i % 16) * 8)) as u8 ^ (std::process::id() as u8).wrapping_mul(i as u8 + 1); }
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The two names the API may know a session by, in the order they are tried.
pub fn candidates(session: &str) -> Result<Vec<String>> {
    let id = parse_session_id(session)?;
    let suffix = id.strip_prefix("session_").unwrap_or(&id).to_string();
    Ok(vec![format!("session_{suffix}"), format!("cse_{suffix}")])
}

/// Pending permission prompts in an events response, newest first. A prompt is pending when it is a `can_use_tool`
/// control_request with no `control_response` for its request_id and no `control_cancel_request`.
/// The response may list events newest first (sort_order=desc) or in order; `sequence_num` decides when present.
pub fn pending_in(body: &Value) -> Vec<Prompt> {
    let mut items: Vec<&Value> = body.as_array().or_else(|| body["data"].as_array()).or_else(|| body["events"].as_array()).map(|l| l.iter().collect()).unwrap_or_default();
    let seq = |e: &Value| e["sequence_num"].as_u64().or_else(|| e["sequence_num"].as_str().and_then(|s| s.parse().ok()));
    let numbered = !items.is_empty() && items.iter().all(|e| seq(e).is_some());
    if numbered { items.sort_by_key(|e| std::cmp::Reverse(seq(e))); }
    let ps: Vec<&Value> = items.iter().map(|e| if e["payload"].is_object() { &e["payload"] } else { *e }).collect();
    let answered: Vec<&str> = ps.iter().filter(|p| p["type"] == "control_response").filter_map(|p| p["response"]["request_id"].as_str()).collect();
    let cancelled: Vec<&str> = ps.iter().filter(|p| p["type"] == "control_cancel_request").filter_map(|p| p["request_id"].as_str()).collect();
    ps.iter().filter(|p| p["type"] == "control_request" && p["request"]["subtype"] == "can_use_tool")
        .filter_map(|p| Some(Prompt { request_id: p["request_id"].as_str()?.to_string(), tool_name: p["request"]["tool_name"].as_str().unwrap_or("").to_string(), input: p["request"]["input"].clone() }))
        .filter(|p| !answered.contains(&p.request_id.as_str()) && !cancelled.contains(&p.request_id.as_str()))
        .collect()
}

/// The `control_response` payload for a prompt.
pub fn response_payload(prompt: &Prompt, decision: &Decision) -> Value {
    let inner = match decision {
        Decision::Allow => json!({"behavior": "allow", "updatedInput": prompt.input, "tool_name": prompt.tool_name}),
        Decision::Deny(message) => json!({"behavior": "deny", "message": message.as_deref().filter(|m| !m.trim().is_empty()).unwrap_or(DEFAULT_DENY)}),
    };
    json!({"type": "control_response", "uuid": uuid4(), "response": {"subtype": "success", "request_id": prompt.request_id, "response": inner}})
}

/// A real user turn (not a peer message).
pub fn user_payload(session_id: &str, text: &str) -> Value { user_payload_as(&uuid4(), session_id, text) }

/// Like [`user_payload`] with a uuid the caller chose, so a UI can match the server's echo to its own copy.
pub fn user_payload_as(uuid: &str, session_id: &str, text: &str) -> Value {
    json!({"uuid": uuid, "session_id": session_id, "type": "user", "parent_tool_use_id": null, "message": {"role": "user", "content": text}})
}

/// Esc, not kill: the control_request Claude's client sends.
/// The permission modes a session accepts (same list as `claude --permission-mode`).
pub const MODES: [&str; 6] = ["default", "acceptEdits", "auto", "plan", "dontAsk", "bypassPermissions"];
/// Change the session's permission mode: the control_request the Claude app sends (`set_permission_mode`).
pub fn mode_payload(mode: &str) -> Value {
    json!({"type": "control_request", "request_id": uuid4(), "uuid": uuid4(), "request": {"subtype": "set_permission_mode", "mode": mode}})
}

pub fn interrupt_payload() -> Value {
    json!({"type": "control_request", "request_id": uuid4(), "uuid": uuid4(), "request": {"subtype": "interrupt"}})
}

impl Client {
    /// The newest 100 events (newest first) and the id form that worked. Tries `session_<x>`, then `cse_<x>`.
    pub fn recent_events(&self, session: &str) -> Result<(String, Value)> {
        let mut last = String::new();
        for id in candidates(session)? {
            match self.request("GET", &format!("/v1/code/sessions/{id}/events?limit=100&sort_order=desc"), None) {
                Ok(body) => return Ok((id, body)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// POST one event payload to the session, built per id form (`build` gets the id being tried).
    fn post_built(&self, session: &str, build: &dyn Fn(&str) -> Value) -> Result<Value> {
        let mut last = String::new();
        for id in candidates(session)? {
            let body = json!({"events": [{"payload": build(&id)}]});
            match self.request("POST", &format!("/v1/code/sessions/{id}/events"), Some(&body)) {
                Ok(reply) => return Ok(json!({"session_id": id, "results": reply["results"]})),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// POST `{"events":[{"payload": payload}]}`. Returns the service's `results` and the id form used.
    pub fn post_event(&self, session: &str, payload: &Value) -> Result<Value> { self.post_built(session, &|_| payload.clone()) }

    /// Unanswered permission prompts, newest first.
    pub fn pending_prompts(&self, session: &str) -> Result<Vec<Prompt>> { Ok(pending_in(&self.recent_events(session)?.1)) }

    /// Answer a permission prompt: `request_id` None means the newest pending one. Refuses a prompt that is not pending.
    pub fn answer(&self, session: &str, request_id: Option<&str>, decision: &Decision) -> Result<Value> {
        let (id, body) = self.recent_events(session)?;
        let pending = pending_in(&body);
        let prompt = match request_id {
            Some(r) => pending.into_iter().find(|p| p.request_id == r).ok_or_else(|| format!("No pending permission prompt with request id {r} (already answered, cancelled, or older than the newest 100 events)."))?,
            None => pending.into_iter().next().ok_or("No pending permission prompt.")?,
        };
        let payload = response_payload(&prompt, decision);
        let reply = self.request("POST", &format!("/v1/code/sessions/{id}/events"), Some(&json!({"events": [{"payload": payload}]})))?;
        Ok(json!({"session_id": id, "request_id": prompt.request_id, "tool_name": prompt.tool_name, "decision": if *decision == Decision::Allow { "allow" } else { "deny" }, "results": reply["results"]}))
    }

    /// Send text as a real user turn.
    pub fn say(&self, session: &str, text: &str) -> Result<Value> {
        if text.trim().is_empty() { return Err("The message is empty.".into()); }
        self.post_built(session, &|id| user_payload(id, text))
    }

    /// Send text as a real user turn with a caller-chosen event uuid (same request as [`Client::say`]).
    pub fn say_as(&self, session: &str, uuid: &str, text: &str) -> Result<Value> {
        if text.trim().is_empty() { return Err("The message is empty.".into()); }
        self.post_built(session, &|id| user_payload_as(uuid, id, text))
    }

    /// Interrupt the running turn (Esc).
    pub fn interrupt(&self, session: &str) -> Result<Value> { self.post_built(session, &|_| interrupt_payload()) }
    /// Set the permission mode (default = Manual: tool calls ask).
    pub fn set_mode(&self, session: &str, mode: &str) -> Result<Value> { let m = mode.to_string(); self.post_built(session, &move |_| mode_payload(&m)) }
}


#[cfg(test)]
mod mode_tests {
    use super::*;
    #[test]
    fn user_payload_as_keeps_the_callers_uuid() {
        let p = user_payload_as("11111111-2222-4333-8444-555555555555", "session_01ExampleAAAAAAAAAAAAAAA", "hi");
        assert_eq!((p["uuid"].as_str(), p["type"].as_str(), p["message"]["content"].as_str()), (Some("11111111-2222-4333-8444-555555555555"), Some("user"), Some("hi")));
        assert_eq!(user_payload("session_01ExampleAAAAAAAAAAAAAAA", "hi")["message"], p["message"]);
    }
    #[test]
    fn mode_payload_is_the_set_permission_mode_request() {
        let p = mode_payload("default");
        assert_eq!(p["type"], "control_request");
        assert_eq!(p["request"]["subtype"], "set_permission_mode");
        assert_eq!(p["request"]["mode"], "default");
        assert!(p["request_id"].as_str().unwrap().len() >= 32);
        assert!(MODES.contains(&"default") && MODES.contains(&"auto"));
    }
}
