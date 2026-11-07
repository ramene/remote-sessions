//! A Claude Code session's local transcript (`<claude dir>/projects/<project>/<session uuid>.jsonl`) as events, so it can be
//! recorded with [`crate::cast`]. The transcript is on the host that ran the session; `host` reads it over `ssh -n`.
//! Its text is untrusted.
use crate::events::{self, Event};
use crate::{rc, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// `8-4-4-4-12` hex digits: the name of a transcript file, and safe to put in a remote shell command.
pub fn uuid_ok(text: &str) -> bool {
    text.len() == 36 && text.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() })
}

/// Conversation lines of a transcript (user, assistant, tool results) as events; bookkeeping lines are skipped.
pub fn parse(text: &str) -> Vec<Event> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| matches!(v["type"].as_str(), Some("user" | "assistant")))
        .filter_map(|v| events::parse_event(&v))
        .collect()
}

/// The transcript file of one session under `home/.claude/projects/*/`.
pub fn local_path(home: &Path, uuid: &str) -> Result<PathBuf> {
    if !uuid_ok(uuid) { return Err("A transcript is named by the session's uuid (8-4-4-4-12 hex digits).".into()); }
    let projects = home.join(".claude").join("projects");
    let dirs = std::fs::read_dir(&projects).map_err(|_| "No Claude projects folder on this machine.".to_string())?;
    dirs.filter_map(|d| d.ok()).map(|d| d.path().join(format!("{uuid}.jsonl"))).find(|p| p.is_file())
        .ok_or_else(|| format!("No transcript for {uuid} on this machine. Run this on the host that ran the session, or add --host <ssh-host>."))
}

/// The transcript text, here or from `host` over ssh (never reads stdin; the uuid was validated, so the glob is the only shell syntax).
pub fn read(host: Option<&str>, uuid: &str) -> Result<String> {
    if !uuid_ok(uuid) { return Err("A transcript is named by the session's uuid (8-4-4-4-12 hex digits).".into()); }
    let bytes = match host {
        None => {
            let path = local_path(&std::env::var_os("HOME").map(PathBuf::from).ok_or("No HOME.")?, uuid)?;
            if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > MAX_BYTES { return Err("That transcript is over 64 MB.".into()); }
            std::fs::read(&path).map_err(|e| format!("Could not read the transcript: {e}"))?
        }
        Some(host) => {
            let out = std::process::Command::new("ssh").args(rc::ssh_args(host, &format!("cat ~/.claude/projects/*/{uuid}.jsonl"))?).output().map_err(|e| format!("Could not run ssh: {e}"))?;
            if !out.status.success() || out.stdout.is_empty() { return Err(format!("No transcript for {uuid} on {host} (is it reachable by ssh, and did that host run the session?).")); }
            if out.stdout.len() as u64 > MAX_BYTES { return Err("That transcript is over 64 MB.".into()); }
            out.stdout
        }
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    const UUID: &str = "00000000-0000-4000-8000-000000000002";
    const JSONL: &str = concat!(
        r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2024-01-01T23:20:25.338Z"}"#, "\n",
        r#"{"type":"user","timestamp":"2024-01-01T23:20:25.343Z","message":{"role":"user","content":"list the files"}}"#, "\n",
        r#"{"type":"assistant","timestamp":"2024-01-01T23:20:26.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}"#, "\n",
        r#"{"type":"assistant","timestamp":"2024-01-01T23:20:27.000Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#, "\n",
        r#"{"type":"user","timestamp":"2024-01-01T23:20:28.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a.txt\nb.txt"}]}}"#, "\n",
        r#"{"type":"assistant","timestamp":"2024-01-01T23:20:29.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Two files."}]}}"#, "\n",
        "not json at all\n",
        r#"{"type":"last-prompt","lastPrompt":"x"}"#, "\n");
    #[test]
    fn a_transcript_becomes_conversation_events_and_skips_bookkeeping() {
        let events = parse(JSONL);
        let shape: Vec<(&str, &str)> = events.iter().map(|e| (e.kind.as_str(), e.text.as_str())).collect();
        assert_eq!(shape, [("user", "list the files"), ("assistant", "▸ Bash: {\"command\":\"ls\"}"), ("tool_result", "◂ result: a.txt (+1 lines)"), ("assistant", "Two files.")]);
        assert!(events[0].user && !events[2].user);
        assert!(events.windows(2).all(|w| w[0].at <= w[1].at));
    }
    #[test]
    fn only_a_real_uuid_is_accepted_and_found_under_projects() {
        assert!(uuid_ok(UUID));
        for bad in ["", "../../etc/passwd", "00000000-0000-4000-8000-00000000000;", "00000000000040008000000000000002", "session_01AbCdEfGh"] { assert!(!uuid_ok(bad), "{bad}"); }
        let home = std::env::temp_dir().join(format!("rs-transcript-{}", std::process::id()));
        let project = home.join(".claude").join("projects").join("-x-y");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{UUID}.jsonl")), JSONL).unwrap();
        assert_eq!(local_path(&home, UUID).unwrap(), project.join(format!("{UUID}.jsonl")));
        assert!(local_path(&home, "00000000-0000-0000-0000-000000000000").unwrap_err().contains("--host"));
        assert!(local_path(&home, "nope").is_err());
        assert!(read(Some("-oProxyCommand=x"), UUID).is_err(), "a hostile host name never reaches ssh");
    }
}
