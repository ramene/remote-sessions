//! `<command> --help`, `<command> -h` and `help <command>` as a process, one test per command: rc 0, the output names
//! the command, and the command does not run. Unknown options still fail.
use remote_sessions_cli::help::{self, COMMANDS};
use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_remote-sessions"))
        .args(args)
        .env("NO_COLOR", "1")
        .env("REMOTE_SESSIONS_CLOUD_BIN", "/nonexistent/cloud-sessions")
        .env("REMOTE_SESSIONS_CLAUDE", "/nonexistent/claude")
        .env("REMOTE_SESSIONS_API", "http://127.0.0.1:9")
        .env("REMOTE_SESSIONS_TOKEN", "")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
}

/// `<command> --help`, `<command> -h` and `help <command>`: rc 0, the same text, it names the command, no JSON error.
fn check(command: &str) -> String {
    let (code, out, err) = run(&[command, "--help"]);
    assert_eq!(code, 0, "{command} --help: rc {code}\n{out}{err}");
    assert!(out.contains(&format!("remote-sessions {command}")), "{command} --help does not name the command:\n{out}");
    assert!(out.contains("-h, --help"), "{command} --help:\n{out}");
    assert!(!out.contains("\"error\""), "{command} --help:\n{out}");
    assert!(err.is_empty(), "{command} --help wrote to stderr: {err}");
    let (code, short, _) = run(&[command, "-h"]);
    assert_eq!((code, &short), (0, &out), "{command} -h");
    let (code, named, _) = run(&["help", command]);
    assert_eq!((code, &named), (0, &out), "help {command}");
    out
}

macro_rules! help_test {
    ($($name:ident => $command:literal),* $(,)?) => {
        $(#[test] fn $name() { check($command); })*
        #[test]
        fn every_command_has_a_test() {
            let tested = [$($command),*];
            for c in COMMANDS.iter().filter(|c| **c != "help") { assert!(tested.contains(c), "no help test for {c}"); }
        }
    };
}

help_test! {
    serve_help => "serve",
    list_help => "list",
    send_help => "send",
    events_help => "events",
    read_help => "read",
    cast_help => "cast",
    recordings_help => "recordings",
    pending_help => "pending",
    approve_help => "approve",
    deny_help => "deny",
    say_help => "say",
    interrupt_help => "interrupt",
    mode_help => "mode",
    stream_help => "stream",
    attach_help => "attach",
    stop_help => "stop",
    control_help => "control",
    mcp_help => "mcp",
}

#[test]
fn control_help_names_its_options_and_keys() {
    let out = check("control");
    for want in ["--host <ssh-host>", "--all", "--routines", "--no-color", "--ascii", "--once", "--size <WxH>", "--ansi", "--demo", "keys:"] {
        assert!(out.contains(want), "control --help: missing {want:?}\n{out}");
    }
    for key in ["Up/Down", "Tab", "Enter", "a / d", "approve", "deny", "s ", "r ", "R ", "refresh", "routines", "q, Esc, Ctrl-C", "close"] {
        assert!(out.contains(key), "control --help: missing key {key:?}\n{out}");
    }
}

#[test]
fn help_after_other_options_and_in_subcommands() {
    let (code, out, _) = run(&["control", "--demo", "--once", "--help"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.starts_with("usage: remote-sessions control"), "{out}");
    let (code, out, _) = run(&["recordings", "serve", "--help"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("remote-sessions recordings serve") && out.contains("--token-file <path>"), "{out}");
    let (code, out, _) = run(&["stop", "session_01ExampleAAAAAAAAAAAAAAA", "-h"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("--yes"), "{out}");
}

#[test]
fn top_level_help_is_unchanged_in_form() {
    for args in [&["--help"][..], &["-h"], &["help"], &["help", "--help"], &["help", "help"]] {
        let (code, out, _) = run(args);
        assert_eq!(code, 0, "{args:?}");
        assert_eq!(out.trim_end(), help::USAGE, "{args:?}");
    }
    let (code, _, err) = run(&[]);
    assert_eq!((code, err.trim_end()), (2, help::USAGE));
}

#[test]
fn help_for_an_unknown_command_is_an_error() {
    let (code, out, _) = run(&["help", "nosuch"]);
    assert_eq!(code, 1);
    assert!(out.contains("Unknown command nosuch.") && out.contains("control"), "{out}");
}

#[test]
fn unknown_options_still_fail() {
    for args in [&["control", "--bogus"][..], &["control", "--demo", "--once", "--nohelp"], &["list", "--nope"], &["attach", "s", "--x"]] {
        let (code, out, _) = run(args);
        assert_eq!(code, 1, "{args:?}: {out}");
        assert!(out.contains("Unknown option --"), "{args:?}: {out}");
    }
}

#[test]
fn a_help_word_as_an_option_value_is_not_a_help_request() {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(!help::wants_help(&s(&["session_01ExampleAAAAAAAAAAAAAAA", "--message", "--help"])));
    assert!(!help::wants_help(&s(&["--request", "-h"])));
    assert!(help::wants_help(&s(&["--message", "hi", "--help"])));
    assert!(help::wants_help(&s(&["-h"])));
    assert!(!help::wants_help(&s(&["--demo", "--once"])));
}

#[test]
fn every_option_in_the_usage_has_a_text() {
    for c in COMMANDS.iter().filter(|c| **c != "help") {
        assert!(!help::usage_lines(c).is_empty(), "{c} has no usage line");
        for name in help::options_of(c) {
            assert!(help::option_text(c, name).is_some(), "{c}: no text for --{name}");
        }
    }
    assert_eq!(help::options_of("list"), vec!["host"], "--sdk-url in the text of list is not an option");
}
