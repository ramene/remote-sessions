//! `remote-sessions control` end to end as a process: the one-frame render over placeholder sessions, and the refusals.
use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_remote-sessions"))
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("COLORTERM")
        .env("REMOTE_SESSIONS_CLOUD_BIN", "/nonexistent/cloud-sessions")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

#[test]
fn demo_once_renders_both_panels_with_placeholder_data_only() {
    let (code, out, _) = run(&["control", "--demo", "--once", "--size", "100x24"]);
    assert_eq!(code, 0, "{out}");
    for want in ["Remote Control", "Cloud", "session_01ExampleAAAAAAAAAAAAAAA", "waiting on Edit src/parser.rs", "your-org/your-repo", "Fix the flaky parser test", "close"] {
        assert!(out.contains(want), "missing {want:?} in\n{out}");
    }
    assert_eq!(out.lines().count(), 24);
    assert!(!out.contains('\x1b'), "NO_COLOR: no escape codes");
    assert!(out.chars().all(|c| c.is_ascii() || c == '\n'), "NO_COLOR: ascii glyphs only");
}

#[test]
fn a_bad_size_is_refused() {
    let (code, out, _) = run(&["control", "--demo", "--once", "--size", "10x3"]);
    assert_eq!(code, 1);
    assert!(out.contains("--size must look like"), "{out}");
}

#[test]
fn the_interactive_room_needs_a_terminal() {
    let (code, out, _) = run(&["control", "--demo"]);
    assert_eq!(code, 1);
    assert!(out.contains("needs a terminal"), "{out}");
}

#[test]
fn cloud_sessions_missing_is_one_line_not_a_failure() {
    // Not --demo: the cloud half runs for real, finds no cloud-sessions binary, and says so. The Remote Control half
    // reads this machine's process list only (one `ps`), with no login configured here.
    let out = Command::new(env!("CARGO_BIN_EXE_remote-sessions"))
        .args(["control", "--once", "--size", "90x20"])
        .env("NO_COLOR", "1")
        .env("REMOTE_SESSIONS_CLOUD_BIN", "/nonexistent/cloud-sessions")
        .env("PATH", "/usr/bin:/bin")
        .env("REMOTE_SESSIONS_TOKEN", "")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("Cloud sessions need cloud-sessions: brew install ramene/tap/cloud-sessions"), "{text}");
}
