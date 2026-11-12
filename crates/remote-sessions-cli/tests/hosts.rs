//! Several machines, end to end as a process: `list` and `control --once` with a stub `ssh` on PATH that answers for
//! two hosts and times out for a third; the three host sources and their order; host names that are ssh options are
//! refused before ssh runs. No network: the API address is a closed loopback port.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const STUB: &str = r#"#!/bin/sh
# A stand-in for ssh: logs its arguments, then answers like `ps -axo pid=,command=` on the named host.
printf '%s\n' "$*" >> "$(dirname "$0")/ssh.log"
for a in "$@"; do h=$p; p=$a; done
case "$h" in
  mac-a) echo "  101 /opt/claude/versions/2.1.318 --print --sdk-url https://api.anthropic.com/v1/code/sessions/cse_01ExampleHOSTAAAAAAAAAA" ;;
  mac-b) echo "  202 /opt/claude/versions/2.1.318 --print --sdk-url https://api.anthropic.com/v1/code/sessions/cse_01ExampleHOSTBBBBBBBBBB" ;;
  *) echo "ssh: connect to host $h port 22: Operation timed out" >&2; exit 255 ;;
esac
"#;

/// A fresh folder with the stub ssh and an empty home.
fn world(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("hosts-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::create_dir_all(dir.join("home/.config/remote-sessions")).unwrap();
    let stub = dir.join("bin/ssh");
    std::fs::write(&stub, STUB).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _ = std::fs::write(dir.join("bin/ssh.log"), "");
    dir
}

fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_remote-sessions"));
    cmd.args(args)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.join("bin").display()))
        .env("HOME", dir.join("home"))
        .env("NO_COLOR", "1")
        .env("REMOTE_SESSIONS_TOKEN", "test-token")
        .env("REMOTE_SESSIONS_API", "http://127.0.0.1:9")
        .env("REMOTE_SESSIONS_CLOUD_BIN", "/nonexistent/cloud-sessions")
        .stdin(std::process::Stdio::null());
    for (k, v) in env { cmd.env(k, v); }
    let out = cmd.output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
}

fn ssh_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("bin/ssh.log")).unwrap_or_default()
}

fn workers_of(v: &Value, host: &str) -> Vec<String> {
    v["workers"].as_array().unwrap().iter().filter(|w| w["host"] == host).map(|w| w["session_id"].as_str().unwrap().to_string()).collect()
}

#[test]
fn list_several_hosts_reports_each_one_and_a_timeout_does_not_hide_the_others() {
    let dir = world("list");
    let (code, out) = run(&dir, &["list", "--host", "mac-a,mac-b", "--host", "vm-1"], &[]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(workers_of(&v, "mac-a"), ["session_01ExampleHOSTAAAAAAAAAA"]);
    assert_eq!(workers_of(&v, "mac-b"), ["session_01ExampleHOSTBBBBBBBBBB"]);
    let hosts = v["hosts"].as_array().unwrap();
    assert_eq!(hosts.iter().map(|h| h["host"].as_str().unwrap()).collect::<Vec<_>>(), ["mac-a", "mac-b", "vm-1"]);
    assert_eq!((hosts[2]["ok"].as_bool(), hosts[2]["error"].as_str()), (Some(false), Some("ssh timed out")));
    // Each host over ssh -n, BatchMode, a connect timeout, and `--` before the host.
    let log = ssh_log(&dir);
    for h in ["mac-a", "mac-b", "vm-1"] {
        assert!(log.lines().any(|l| l == format!("-n -o BatchMode=yes -o ConnectTimeout=5 -- {h} ps -axo pid=,command=")), "{log}");
    }
}

#[test]
fn one_host_keeps_the_old_output() {
    let dir = world("one");
    let (code, out) = run(&dir, &["list", "--host", "mac-a"], &[]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["host"], "mac-a");
    assert!(v.get("hosts").is_none());
    assert_eq!(workers_of(&v, "mac-a"), ["session_01ExampleHOSTAAAAAAAAAA"]);
    let (code, out) = run(&dir, &["list", "--host", "vm-1"], &[]);
    assert_eq!(code, 1, "one host that fails is an error, as before: {out}");
    assert!(out.contains("ssh timed out"), "{out}");
}

#[test]
fn the_three_sources_merge_in_order_and_this_machine_is_first() {
    let dir = world("sources");
    std::fs::write(dir.join("home/.config/remote-sessions/hosts"), "# my machines\nmac-b\n\nvm-1   # the VM\nmac-a\n").unwrap();
    // No --host: this machine, then REMOTE_SESSIONS_HOSTS, then the hosts file; each host once.
    let (code, out) = run(&dir, &["list"], &[("REMOTE_SESSIONS_HOSTS", "mac-a")]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    let names: Vec<&str> = v["hosts"].as_array().unwrap().iter().map(|h| h["host"].as_str().unwrap()).collect();
    assert_eq!(names[1..], ["mac-a", "mac-b", "vm-1"], "{out}");
    assert!(!names[0].is_empty() && !["mac-a", "mac-b", "vm-1"].contains(&names[0]), "this machine first: {out}");
    assert_eq!(v["hosts"][0]["ok"], true, "this machine is listed directly, not over ssh: {out}");
    assert!(!ssh_log(&dir).contains(&format!("-- {} ", names[0])));
    // The room takes the same sources, after its own --host.
    let (code, out) = run(&dir, &["control", "--once", "--size", "160x24", "--no-color", "--ascii", "--host", "vm-1"], &[("REMOTE_SESSIONS_HOSTS", "mac-b,mac-a")]);
    assert_eq!(code, 0, "{out}");
    let title = out.lines().find(|l| l.contains("Remote Control")).unwrap();
    let (a, b, c) = (title.find("vm-1 x").unwrap(), title.find("mac-b 1").unwrap(), title.find("mac-a 1").unwrap());
    assert!(a < b && b < c, "order --host, env, file: {title}");
}

#[test]
fn the_room_lists_every_host_with_a_host_column_and_an_error_row_for_the_one_that_failed() {
    let dir = world("room");
    let (code, out) = run(&dir, &["control", "--once", "--size", "160x24", "--no-color", "--ascii", "--host", "mac-a,mac-b,vm-1"], &[]);
    assert_eq!(code, 0, "{out}");
    let row = |needle: &str| out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no {needle}:\n{out}")).to_string();
    assert!(row("session_01ExampleHOSTAAAAAAAAAA").contains("mac-a"), "{out}");
    assert!(row("session_01ExampleHOSTBBBBBBBBBB").contains("mac-b"), "{out}");
    assert!(row("ssh timed out").contains("vm-1"), "{out}");
    assert!(row("Remote Control").contains("mac-a 1 | mac-b 1 | vm-1 x"), "{out}");
}

#[test]
fn host_names_that_are_ssh_options_are_refused_before_ssh_runs() {
    let dir = world("inject");
    let evil = "-oProxyCommand=touch /nonexistent/pwned";
    for (args, env) in [
        (vec!["list", "--host", evil], vec![]),
        (vec!["list", "--host", "mac-a,-oProxyCommand=sh"], vec![]),
        (vec!["list"], vec![("REMOTE_SESSIONS_HOSTS", "mac-a,-oProxyCommand=sh")]),
        (vec!["control", "--once", "--host", evil], vec![]),
        (vec!["control", "--once"], vec![("REMOTE_SESSIONS_HOSTS", "mac;a")]),
        (vec!["stop", "session_01ExampleAAAAAAAAAAAAAAA", "--yes", "--host", evil], vec![]),
    ] {
        let (code, out) = run(&dir, &args, &env);
        assert_eq!(code, 1, "{args:?} {env:?}: {out}");
        assert!(out.contains("Not a valid ssh host name"), "{args:?}: {out}");
    }
    std::fs::write(dir.join("home/.config/remote-sessions/hosts"), "mac-a\n-oProxyCommand=sh\n").unwrap();
    let (code, out) = run(&dir, &["list"], &[]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains(".config/remote-sessions/hosts") && out.contains("Not a valid ssh host name"), "{out}");
    assert_eq!(ssh_log(&dir), "", "ssh must not run for a refused name");
}

#[test]
fn mcp_remote_list_takes_several_hosts() {
    let dir = world("mcp");
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"remote_list","arguments":{"host":"mac-a,vm-1"}}}"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_remote-sessions"))
        .arg("mcp")
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", dir.join("bin").display()))
        .env("HOME", dir.join("home"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    writeln!(child.stdin.take().unwrap(), "{req}").unwrap();
    let out = child.wait_with_output().unwrap();
    let reply: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let v: Value = serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(workers_of(&v, "mac-a"), ["session_01ExampleHOSTAAAAAAAAAA"]);
    assert_eq!(v["hosts"][1]["error"], "ssh timed out");
}
