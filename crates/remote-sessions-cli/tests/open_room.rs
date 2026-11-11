//! `claude-mod/remote-sessions/scripts/open-room` (`/remote-room`): which multiplexer it asks for the split. Stub `cmux`
//! and `tmux` binaries record their arguments; no real split is made.
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
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}/{name}.log\"\nexit {}\n", dir.display(), if *ok { 0 } else { 1 })).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
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
    let dir = stubs("cmux", &[("cmux", true), ("tmux", true)]);
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
fn inside_tmux_it_splits_with_tmux() {
    let dir = stubs("tmux", &[("tmux", true), ("cmux", true)]);
    let out = run(&dir, &[("TMUX", "/sock,1,0")], &[]);
    assert!(out.contains(r#""split":true"#) && out.contains(r#""mux":"tmux""#), "{out}");
    let tmux = log(&dir, "tmux").expect("tmux was asked");
    assert!(tmux.starts_with("split-window -h -d -l 50% ") && tmux.contains("/remote-sessions\" control"), "{tmux}");
    assert_eq!(log(&dir, "cmux"), None, "no cmux variable: cmux is not asked");
    // cmux variables but no cmux binary: tmux
    let dir = stubs("tmux-nocmux", &[("tmux", true)]);
    assert!(run(&dir, &[("CMUX_SURFACE_ID", "s"), ("TMUX", "/sock,1,0")], &[]).contains(r#""mux":"tmux""#));
}

#[test]
fn outside_both_it_only_prints_the_command() {
    let dir = stubs("none", &[("tmux", true), ("cmux", true)]);
    let out = run(&dir, &[], &[]);
    assert!(out.contains(r#""split":false"#) && out.contains(r#""run":"remote-sessions control""#), "{out}");
    assert_eq!((log(&dir, "tmux"), log(&dir, "cmux")), (None, None), "nothing is created");
}

#[test]
fn a_refused_split_is_reported_with_the_command_to_run() {
    let dir = stubs("refused", &[("cmux", false), ("tmux", false)]);
    let out = run(&dir, &[("CMUX_SURFACE_ID", "s")], &[]);
    assert!(out.contains(r#""split":false"#) && out.contains("cmux refused the split") && out.contains("remote-sessions control"), "{out}");
    let out = run(&dir, &[("TMUX", "/sock,1,0")], &[]);
    assert!(out.contains("tmux refused the split"), "{out}");
}
