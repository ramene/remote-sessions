//! Reading a session's events over HTTPS with a bearer token the caller supplies (this crate stores no secrets).
//! From cloud-sessions (same authors), reduced to `GET /v1/code/sessions/<id>/events`. Polled, not streamed.
use crate::events::{self, Event};
use crate::ids::parse_session_id;
use crate::Result;
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

pub const API: &str = "https://api.anthropic.com";
const LIMIT: u64 = 16 * 1024 * 1024;
const SERVICE: &str = "Claude sessions API";

/// New events since a cursor, plus where the cursor now is.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Page {
    pub events: Vec<Event>,
    /// Id of the newest event seen; pass it back as `after`.
    pub cursor: Option<String>,
    /// The log ends with a `result` event.
    pub finished: bool,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    token: String,
    beta: Option<String>,
}

/// The only address that gets the login token: `https://api.anthropic.com` (no other host, no port).
fn anthropic(base: &str) -> bool {
    base.strip_prefix(API).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn loopback(base: &str) -> bool {
    ["http://127.0.0.1", "http://localhost", "http://[::1]"].iter().any(|p| base.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with(':') || rest.starts_with('/')))
}

/// Said when the service refuses an untrusted device. Claude's own client re-enrolls the device by signing; this tool does not.
pub const UNTRUSTED_DEVICE: &str = "The Claude sessions API refused this machine as an untrusted device (HTTP 403). Claude Code's own client re-enrolls a device by signing a challenge; remote-sessions does not implement signing, so enroll this machine from Claude Code first, or act from a machine that is already trusted.";

/// Whether an error body is the untrusted-device refusal.
pub fn untrusted_device(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    ["untrusted", "trusted device", "trusted_device", "device_not_trusted", "not a trusted"].iter().any(|m| lower.contains(m))
}

/// The service's own message from an error body, else the status. Credentials never appear here.
pub fn error_message(status: u16, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let message = parsed.as_ref().and_then(|v| v["message"].as_str().or(v["error"]["message"].as_str()).or(v["error"].as_str())).map(str::to_string)
        .or_else(|| Some(body.trim().chars().take(300).collect::<String>()).filter(|s| !s.is_empty() && !s.starts_with('<')));
    match (status, message) {
        (403, _) if untrusted_device(body) => UNTRUSTED_DEVICE.to_string(),
        (401 | 403, _) => format!("{SERVICE} refused the credentials (HTTP {status}). A token from `claude setup-token` is inference-only and cannot read sessions; inside Claude Code use its RemoteTrigger get_run_log (/remote-read) instead."),
        (_, Some(message)) => format!("{SERVICE}: {message}"),
        _ => format!("{SERVICE} returned HTTP {status}."),
    }
}

impl Client {
    /// `base` is `https://api.anthropic.com` (the default); any other address is refused, except plain HTTP to a
    /// loopback test server. So the token goes to Anthropic's API and nowhere else.
    pub fn new(base: Option<&str>, token: &str, beta: Option<&str>) -> Result<Self> {
        let base = base.unwrap_or(API).trim_end_matches('/').to_string();
        if !(anthropic(&base) || loopback(&base)) { return Err("The API address must be https://api.anthropic.com (plain http only to a loopback test server).".into()); }
        let token = token.trim().to_string();
        if token.is_empty() || token.chars().any(char::is_control) { return Err("The token is empty or malformed.".into()); }
        let beta = beta.filter(|b| !b.is_empty()).map(str::to_string).or_else(|| token.starts_with("sk-ant-oat").then(|| "oauth-2025-04-20".to_string()));
        Ok(Self { base, token, beta })
    }

    fn failure(error: ureq::Error) -> String {
        match error {
            ureq::Error::Status(code, response) => error_message(code, &response.into_string().unwrap_or_default()),
            ureq::Error::Transport(t) if matches!(t.kind(), ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed) => format!("Could not reach the {SERVICE}. Check your connection."),
            ureq::Error::Transport(_) => format!("{SERVICE} request failed."),
        }
    }
    fn get_json(&self, path: &str) -> Result<Value> { self.request("GET", path, None) }
    /// One HTTPS call: no redirects, bounded body, the credential only in the Authorization header.
    pub(crate) fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut builder = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(60)).user_agent(concat!("remote-sessions-core/", env!("CARGO_PKG_VERSION")));
        if !loopback(&self.base) { builder = builder.try_proxy_from_env(true); }
        let mut request = builder.build().request(method, &format!("{}{path}", self.base)).set("Authorization", &format!("Bearer {}", self.token)).set("anthropic-version", "2023-06-01").set("anthropic-client-platform", "cli");
        if let Some(beta) = &self.beta { request = request.set("anthropic-beta", beta); }
        let response = match body {
            Some(json) => request.set("Content-Type", "application/json").send_string(&json.to_string()),
            None => request.call(),
        }.map_err(Self::failure)?;
        let mut text = String::new();
        response.into_reader().take(LIMIT + 1).read_to_string(&mut text).map_err(|_| format!("{SERVICE} response was cut off."))?;
        if text.len() as u64 > LIMIT { return Err(format!("{SERVICE} sent a response that is too large.")); }
        if text.trim().is_empty() { return Ok(Value::Null); }
        serde_json::from_str(&text).map_err(|_| format!("{SERVICE} returned an unreadable response."))
    }


    /// Open `GET /v1/code/sessions/<id>/events/stream` (server-sent events), the call Claude Code's own client makes.
    /// Tries `session_<x>` then `cse_<x>`. `from` is sent as `from_sequence_num` and `Last-Event-ID`. Returns the id form
    /// that worked and a reader over the body. The error flag is true when retrying cannot help (a 4xx other than 408/429).
    pub fn open_stream(&self, session: &str, from: Option<u64>, read_timeout: Duration) -> std::result::Result<(String, Box<dyn Read + Send>), (bool, String)> {
        let mut last = (false, String::new());
        for id in crate::control::candidates(session).map_err(|e| (true, e))? {
            let mut builder = ureq::AgentBuilder::new().redirects(0).timeout_connect(Duration::from_secs(10)).timeout_read(read_timeout).user_agent(concat!("remote-sessions-core/", env!("CARGO_PKG_VERSION")));
            if !loopback(&self.base) { builder = builder.try_proxy_from_env(true); }
            let query = from.map(|n| format!("?from_sequence_num={n}")).unwrap_or_default();
            let mut request = builder.build().get(&format!("{}/v1/code/sessions/{id}/events/stream{query}", self.base)).set("Authorization", &format!("Bearer {}", self.token)).set("anthropic-version", "2023-06-01").set("anthropic-client-platform", "cli").set("Accept", "text/event-stream");
            if let Some(beta) = &self.beta { request = request.set("anthropic-beta", beta); }
            if let Some(n) = from { request = request.set("Last-Event-ID", &n.to_string()); }
            match request.call() {
                Ok(response) => return Ok((id, response.into_reader())),
                Err(ureq::Error::Status(code, response)) => {
                    let fatal = (400..500).contains(&code) && code != 404 && code != 408 && code != 429;
                    last = (fatal, error_message(code, &response.into_string().unwrap_or_default()));
                    if fatal { return Err(last); }
                }
                Err(other) => last = (false, Self::failure(other)),
            }
        }
        Err(last)
    }

    /// The whole condensed event log, raw.
    pub fn events_raw(&self, session_id: &str) -> Result<Value> {
        self.get_json(&format!("/v1/code/sessions/{}/events", parse_session_id(session_id)?))
    }
    /// Events after `after` (an event id from an earlier page; unknown or absent -> the whole log).
    pub fn events(&self, session_id: &str, after: Option<&str>) -> Result<Page> {
        let body = self.events_raw(session_id)?;
        Ok(page_from(&body, after))
    }
}

/// A response body -> a page of entries after the cursor.
pub fn page_from(body: &Value, after: Option<&str>) -> Page {
    let mut entries = events::parse_events(body);
    if let Some(position) = after.and_then(|a| entries.iter().position(|e| e.id == a)) { entries.drain(..=position); }
    let cursor = entries.last().map(|e| e.id.clone()).or_else(|| after.map(str::to_string));
    Page { events: entries, cursor, finished: events::finished(body) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cursor_skips_seen_events() {
        let body = json!({"data": [{"uuid": "a", "type": "user", "text": "one"}, {"uuid": "b", "type": "assistant", "text": "two"}, {"uuid": "c", "type": "result", "result": "done"}]});
        let page = page_from(&body, Some("a"));
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.cursor.as_deref(), Some("c"));
        assert!(page.finished);
    }
    #[test]
    fn plain_http_is_refused_except_loopback() {
        assert!(Client::new(Some("http://example.com"), "t", None).is_err());
        assert!(Client::new(Some("http://127.0.0.1:9"), "t", None).is_ok());
        assert!(Client::new(None, "", None).is_err());
    }
    #[test]
    fn the_token_goes_only_to_api_anthropic_com() {
        for ok in [None, Some("https://api.anthropic.com"), Some("https://api.anthropic.com/"), Some("http://127.0.0.1:9"), Some("http://localhost:8080"), Some("http://[::1]:9")] {
            assert!(Client::new(ok, "t", None).is_ok(), "{ok:?}");
        }
        for bad in ["https://example.com", "https://api.anthropic.com.evil.example", "https://api.anthropic.com:8443", "https://api.anthropic.com@evil.example", "http://api.anthropic.com", "https://evil.example/https://api.anthropic.com", "http://127.0.0.1.evil.example"] {
            let e = Client::new(Some(bad), "t", None).err().unwrap_or_else(|| panic!("{bad} must be refused"));
            assert!(e.contains("must be https://api.anthropic.com"), "{e}");
        }
    }
}
