//! SEND: `claude -p "<message>" --cloud session_<id> --output-format json`.
//! The message arrives in the session as a PEER message: it can nudge a session but never approve a permission prompt.
//! Adapted from cloud-sessions (same authors).
use crate::events::one_line;
use crate::ids::{parse_session_id, session_url};
use crate::Result;
use serde_json::Value;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run a command with a deadline and a stdout cap; returns its stdout.
pub fn run_bounded(mut command: Command, timeout: Duration, limit: usize) -> Result<Vec<u8>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("Could not start the command: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("No output pipe.")?;
    let reader = std::thread::spawn(move || { let mut bytes = Vec::new(); let _ = (&mut stdout).take(limit as u64).read_to_end(&mut bytes); bytes });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => break,
            None if Instant::now() >= deadline => { let _ = child.kill(); let _ = child.wait(); return Err("Claude Code did not answer in time.".into()); }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    Ok(reader.join().unwrap_or_default())
}

/// The message is one argument (no shell), so nothing in it is interpreted. Returns the session URL.
pub fn send(bin: &Path, session_id: &str, message: &str, timeout: Duration) -> Result<String> {
    let id = parse_session_id(session_id)?;
    if message.trim().is_empty() { return Err("The message is empty.".into()); }
    let mut command = Command::new(bin);
    command.args(["-p", message, "--cloud", &id, "--output-format", "json"]);
    let stdout = run_bounded(command, timeout, 1024 * 1024)?;
    let text = String::from_utf8_lossy(&stdout).into_owned();
    let parsed = text.lines().rev().find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object)).or_else(|| serde_json::from_str::<Value>(text.trim()).ok());
    match parsed {
        Some(v) if v["ok"] == true => Ok(v["url"].as_str().map(str::to_string).unwrap_or_else(|| session_url(&id))),
        Some(v) => Err(one_line(v["error"].as_str().unwrap_or("Claude Code could not send the message."), 400)),
        None => Err(format!("Claude Code gave an unreadable answer: {}", one_line(&text, 200))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rs-core-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn fake_claude(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn send_runs_the_documented_command_with_a_fake_claude() {
        let dir = scratch("send");
        let log = dir.join("args.txt");
        let ok = fake_claude(&dir, &format!("printf '%s\\n' \"$@\" > {}\necho '{{\"ok\":true,\"url\":\"https://claude.ai/code/session_01Send1234\"}}'", log.display()));
        assert_eq!(send(&ok, "cse_01Send1234", "also add a test; $(rm -rf /)", Duration::from_secs(20)).unwrap(), "https://claude.ai/code/session_01Send1234");
        let args = std::fs::read_to_string(&log).unwrap();
        assert_eq!(args.lines().collect::<Vec<_>>(), ["-p", "also add a test; $(rm -rf /)", "--cloud", "session_01Send1234", "--output-format", "json"]);
        let refused = fake_claude(&dir, "echo '{\"ok\":false,\"error\":\"Session is archived\"}'; exit 1");
        assert_eq!(send(&refused, "session_01Send1234", "hi", Duration::from_secs(20)).unwrap_err(), "Session is archived");
        assert!(send(&ok, "bad id", "hi", Duration::from_secs(5)).is_err());
        assert!(send(&ok, "session_01Send1234", "  ", Duration::from_secs(5)).is_err());
    }
}
