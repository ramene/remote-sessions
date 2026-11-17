//! `claude-mod/remote-sessions/scripts/open-room` (`/remote-room`): which multiplexer it asks for the split. Stub `cmux`
//! and `tmux` binaries record their arguments; no real split is made. The tmux stub models a server: `list-clients`
//! prints the attached clients (none = no server, exit 1), `split-window` exits as told (s569: the tmux branch detects
//! the attached client, not $TMUX, because the Claude Code CLI stopped exporting $TMUX into plugin subshells).
use std::path::{Path, PathBuf};
use std::process::Command;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../claude-mod/remote-sessions/scripts/open-room")
}

/// A fresh folder with stub binaries (`ok` = exit 0, else exit 1) that append their argv to `<name>.log`.
fn stubs(tag: &str, bins: &[(&str, bool)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("open-room-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, ok) in bins {
        write_stub(&dir, name, &format!("exit {}", if *ok { 0 } else { 1 }));
    }
    dir
}

fn write_stub(dir: &Path, name: &str, tail: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}/{name}.log\"\n{tail}\n", dir.display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A tmux stub. `clients` = the attached clients as `activity session` lines (empty = no server); `split_ok` = split-window's exit.
fn tmux_stub(dir: &Path, clients: &[&str], split_ok: bool) {
    let mut tail = String::from("case \"$1\" in\n  list-clients)\n");
    if clients.is_empty() {
        tail.push_str("    exit 1 ;;\n");
    } else {
        for client in clients {
            tail.push_str(&format!("    echo '{client}'\n"));
        }
        tail.push_str("    exit 0 ;;\n");
    }
    tail.push_str(&format!("  split-window) exit {} ;;\n  *) exit 0 ;;\nesac", if split_ok { 0 } else { 1 }));
    write_stub(dir, "tmux", &tail);
}

fn run(dir: &Path, env: &[(&str, &str)], args: &[&str]) -> String {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg(script()).args(args).env_clear().env("PATH", format!("{}:/usr/bin:/bin", dir.display()));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

fn log(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(format!("{name}.log"))).ok()
}

#[test]
fn inside_cmux_it_splits_with_cmux_first_even_inside_tmux() {
    let dir = stubs("cmux", &[("cmux", true)]);
    tmux_stub(&dir, &["1700000001 remote-main"], true);
    let out = run(&dir, &[("CMUX_SURFACE_ID", "surface-1"), ("TMUX", "/sock,1,0")], &["--host", "mac-a"]);
    assert!(out.contains(r#""split":true"#) && out.contains(r#""mux":"cmux""#), "{out}");
    let cmux = log(&dir, "cmux").expect("cmux was asked");
    assert!(cmux.starts_with("new-split right --command "), "{cmux}");
    assert!(cmux.contains("/remote-sessions\" control --host mac-a"), "{cmux}");
    assert_eq!(log(&dir, "tmux"), None, "tmux is not asked when cmux answered");
    // the workspace id alone also counts as inside cmux
    let dir = stubs("cmux-ws", &[("cmux", true)]);
    assert!(run(&dir, &[("CMUX_WORKSPACE_ID", "ws-1")], &[]).contains(r#""mux":"cmux""#));
}

#[test]
fn inside_tmux_without_the_tmux_env_it_docks_beside_the_attached_client() {
    // the Claude Code CLI subshell: no $TMUX in the environment (env_clear), a tmux server with two attached clients
    let dir = stubs("tmux", &[("cmux", true)]);
    tmux_stub(&dir, &["1700000001 worker", "1700000099 remote-main"], true);
    let out = run(&dir, &[], &[]);
    assert!(out.contains(r#""split":true"#) && out.contains(r#""mux":"tmux""#), "{out}");
    let tmux = log(&dir, "tmux").expect("tmux was asked");
    assert!(tmux.contains("split-window -h -d -l 45% -t remote-main: ") && tmux.contains("/remote-sessions\" control"), "{tmux}");
    assert_eq!(log(&dir, "cmux"), None, "no cmux variable: cmux is not asked");
}

#[test]
fn the_most_recently_active_client_wins_over_a_worker_pane() {
    let dir = stubs("tmux-recent", &[]);
    tmux_stub(&dir, &["1700000500 deploy-pane", "1700000090 remote-main"], true);
    let out = run(&dir, &[], &[]);
    assert!(out.contains(r#""split":true"#), "{out}");
    let tmux = log(&dir, "tmux").expect("tmux was asked");
    assert!(tmux.contains("-t deploy-pane:"), "the newest activity is the target: {tmux}");
    assert!(!tmux.contains("-t remote-main:"), "{tmux}");
}

#[test]
fn inside_a_tmux_pane_the_split_goes_beside_that_pane_not_the_client() {
    // s576: $TMUX_PANE is the pane this process runs in (the Claude Code pane); a worker client attached elsewhere is not the target
    let dir = stubs("tmux-pane", &[("cmux", true)]);
    tmux_stub(&dir, &["1700000900 worker"], true);
    let out = run(&dir, &[("TMUX_PANE", "%7")], &[]);
    assert!(out.contains(r#""split":true"#) && out.contains(r#""mux":"tmux""#), "{out}");
    let tmux = log(&dir, "tmux").expect("tmux was asked");
    assert!(tmux.contains("split-window -h -d -l 45% -t %7 "), "{tmux}");
    assert!(!tmux.contains("worker:"), "{tmux}");
}

/// A tmux stub that also answers `list-panes` with the given `left top id` lines.
fn tmux_stub_with_panes(dir: &Path, panes: &[&str]) {
    let mut tail = String::from("case \"$1\" in\n  list-panes)\n");
    for pane in panes {
        tail.push_str(&format!("    echo '{pane}'\n"));
    }
    tail.push_str("    exit 0 ;;\n  split-window) exit 0 ;;\n  *) exit 0 ;;\nesac");
    write_stub(dir, "tmux", &tail);
}

#[test]
fn with_a_pane_already_on_the_right_the_room_stacks_under_it() {
    // s576: the deck (or an earlier room) holds the right column (left 44); the room splits its top pane downward
    let dir = stubs("tmux-stack", &[]);
    tmux_stub_with_panes(&dir, &["0 1 %1", "44 1 %2", "44 20 %3"]);
    let out = run(&dir, &[("TMUX_PANE", "%1")], &[]);
    assert!(out.contains(r#""split":true"#), "{out}");
    let tmux = log(&dir, "tmux").expect("tmux was asked");
    assert!(tmux.contains("split-window -v -d -t %2 "), "under the topmost right pane: {tmux}");
    assert!(!tmux.contains("split-window -h"), "no third column: {tmux}");
}

#[test]
fn with_one_pane_and_no_right_column_the_room_splits_beside_the_target() {
    let dir = stubs("tmux-beside", &[]);
    tmux_stub_with_panes(&dir, &["0 1 %1"]);
    let out = run(&dir, &[("TMUX_PANE", "%1")], &[]);
    assert!(out.contains(r#""split":true"#), "{out}");
    let tmux = log(&dir, "tmux").expect("tmux was asked");
    assert!(tmux.contains("split-window -h -d -l 45% -t %1 "), "{tmux}");
}

#[test]
fn cmux_variables_without_a_cmux_binary_use_tmux() {
    let dir = stubs("tmux-nocmux", &[]);
    tmux_stub(&dir, &["1700000050 remote-main"], true);
    assert!(run(&dir, &[("CMUX_SURFACE_ID", "s")], &[]).contains(r#""mux":"tmux""#));
}

#[test]
fn outside_both_it_only_prints_the_command() {
    // tmux is installed but no server runs: list-clients fails, so no split is attempted
    let dir = stubs("none", &[("cmux", true)]);
    tmux_stub(&dir, &[], true);
    let out = run(&dir, &[], &[]);
    assert!(out.contains(r#""split":false"#) && out.contains(r#""run":"remote-sessions control""#), "{out}");
    assert_eq!(log(&dir, "cmux"), None, "nothing is created");
    assert!(!log(&dir, "tmux").unwrap_or_default().contains("split-window"), "no split without a client");
}

#[test]
fn a_refused_split_is_reported_with_the_command_to_run() {
    let dir = stubs("refused", &[("cmux", false)]);
    let out = run(&dir, &[("CMUX_SURFACE_ID", "s")], &[]);
    assert!(out.contains(r#""split":false"#) && out.contains("cmux refused the split") && out.contains("remote-sessions control"), "{out}");
    tmux_stub(&dir, &["1700000001 remote-main"], false);
    let out = run(&dir, &[], &[]);
    assert!(out.contains("tmux refused the split"), "{out}");
}
