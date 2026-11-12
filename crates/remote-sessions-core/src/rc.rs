//! Remote Control: eligibility, worker discovery and stop.
//!
//! The worker of a Remote Control session is a local process started by the `claude remote-control` server:
//! `claude --print --sdk-url https://api.anthropic.com/v1/code/sessions/cse_<id> ...`. It is found from a
//! `ps -axo pid=,command=` listing. `stop` sends ONE SIGINT: that ENDS the session and the server archives it.
use crate::ids::{cse_id, parse_session_id, session_url};
use crate::Result;
use serde::Serialize;

const SDK_URL: &str = "--sdk-url https://api.anthropic.com/v1/code/sessions/cse_";

/// A running Remote Control worker.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Worker {
    pub pid: u32,
    /// Canonical `session_<x>` (the id on claude.ai).
    pub session_id: String,
    /// The same session as the API names it, `cse_<x>`.
    pub cse_id: String,
    pub url: String,
}

fn is_claude(first: &str) -> bool {
    first.ends_with("/claude") || first.contains("/claude/versions/") || first == "claude"
}

/// Every Remote Control worker in `ps -axo pid=,command=` text. Only a `claude` process counts: a shell that merely
/// mentions the URL does not.
pub fn discover(ps_output: &str) -> Vec<Worker> {
    ps_output.lines().filter_map(|line| {
        let line = line.trim_start();
        let (pid, cmd) = line.split_once(char::is_whitespace)?;
        let pid: u32 = pid.parse().ok()?;
        let cmd = cmd.trim_start();
        if !is_claude(cmd.split_whitespace().next().unwrap_or("")) { return None; }
        let flat = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
        let after = flat.split_once(SDK_URL)?.1;
        let id: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
        let rest = &after[id.len()..];
        if id.len() < 8 || !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('?') || rest.starts_with('/')) { return None; }
        Some(Worker { pid, session_id: format!("session_{id}"), cse_id: format!("cse_{id}"), url: session_url(&format!("session_{id}")) })
    }).collect()
}

/// The workers of exactly one session (`session_x`, `cse_x`, a bare id or a claude.ai link).
pub fn workers_for(ps_output: &str, session: &str) -> Result<Vec<Worker>> {
    let want = cse_id(session)?;
    Ok(discover(ps_output).into_iter().filter(|w| w.cse_id == want).collect())
}

/// An ssh host: an ssh config name or a host name (letters, digits, `.`, `_`, `-`), optionally `user@` before it.
/// Never a leading `-` (so never an ssh option such as `-oProxyCommand=...`), no spaces, no other characters.
pub fn host_ok(host: &str) -> bool {
    let part = |s: &str| !s.is_empty() && !s.starts_with('-') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let (user, name) = match host.split_once('@') { Some((u, n)) => (Some(u), n), None => (None, host) };
    host.len() <= 253 && part(name) && user.map_or(true, part)
}
/// `ssh` arguments that run `remote_cmd` on `host` without reading stdin (`-n`), never asking for a password
/// (`BatchMode`), giving up on a host that does not answer (`ConnectTimeout`); `--` keeps the host from being an option.
pub fn ssh_args(host: &str, remote_cmd: &str) -> Result<Vec<String>> {
    if !host_ok(host) { return Err(format!("Not a valid ssh host name: {}. Use an ssh config name or a host name (letters, digits, '.', '_', '-'; user@ allowed).", host.chars().take(40).collect::<String>())); }
    Ok(vec!["-n".into(), "-o".into(), "BatchMode=yes".into(), "-o".into(), "ConnectTimeout=5".into(), "--".into(), host.into(), remote_cmd.into()])
}

/// One host name, checked (see [`host_ok`]).
pub fn checked_host(host: &str) -> Result<String> {
    ssh_args(host, "")?;
    Ok(host.to_string())
}
/// Host names from a comma (or space) separated list: `--host a,b` or `REMOTE_SESSIONS_HOSTS`. Any bad name is an error.
pub fn hosts_from_list(text: &str) -> Result<Vec<String>> {
    text.split(|c: char| c == ',' || c.is_whitespace()).filter(|h| !h.is_empty()).map(checked_host).collect()
}
/// Host names from the hosts file: one per line, `#` starts a comment, blank lines are skipped. Any bad name is an error.
pub fn hosts_from_file(text: &str) -> Result<Vec<String>> {
    text.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty()).map(checked_host).collect()
}
/// The lists in order, each name once (first place wins).
pub fn merge_hosts(lists: Vec<Vec<String>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in lists.into_iter().flatten() {
        if !out.contains(&h) { out.push(h); }
    }
    out
}

/// How long one listing over ssh may take; then ssh is stopped and the host reports "ssh timed out".
pub const SSH_LIMIT: std::time::Duration = std::time::Duration::from_secs(8);

/// Run `cmd` (stdin closed) and collect its output; stop it at `limit`. `Err("timed out")` when it was stopped.
pub fn output_within(mut cmd: std::process::Command, limit: std::time::Duration) -> Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let (mut out, mut err) = (child.stdout.take().ok_or("no stdout")?, child.stderr.take().ok_or("no stderr")?);
    // Read both pipes while waiting, so a long listing never fills a pipe and stalls.
    let out = std::thread::spawn(move || { let mut v = Vec::new(); let _ = out.read_to_end(&mut v); v });
    let err = std::thread::spawn(move || { let mut v = Vec::new(); let _ = err.read_to_end(&mut v); v });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(std::time::Duration::from_millis(20)),
            Ok(None) => { let _ = child.kill(); let _ = child.wait(); return Err("timed out".into()); }
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(std::process::Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

/// A short reason for a failed ssh listing, from ssh's own message: "ssh timed out", "unknown host", ...
pub fn ssh_reason(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let low = line.to_ascii_lowercase();
    if low.contains("timed out") { "ssh timed out".into() }
    else if low.contains("could not resolve") || low.contains("name or service not known") || low.contains("nodename nor servname") { "ssh: unknown host".into() }
    else if low.contains("permission denied") { "ssh: login refused (BatchMode: keys only)".into() }
    else if low.contains("connection refused") { "ssh: connection refused".into() }
    else if line.is_empty() { "ssh failed".into() }
    else { format!("ssh failed: {}", line.chars().filter(|c| !c.is_control()).take(80).collect::<String>()) }
}
pub const PS_COMMAND: &str = "ps -axo pid=,command=";

/// Run `ps` here (`host` None) or over ssh.
pub fn ps_listing(host: Option<&str>) -> Result<String> {
    let output = match host {
        None => std::process::Command::new("ps").args(["-axo", "pid=,command="]).output(),
        Some(host) => {
            let mut cmd = std::process::Command::new("ssh");
            cmd.args(ssh_args(host, PS_COMMAND)?);
            let output = output_within(cmd, SSH_LIMIT).map_err(|e| if e == "timed out" { "ssh timed out".to_string() } else { format!("Could not run ssh: {e}") })?;
            if !output.status.success() && output.stdout.is_empty() { return Err(ssh_reason(&output.stderr)); }
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }.map_err(|e| format!("Could not list processes: {e}"))?;
    if !output.status.success() && output.stdout.is_empty() { return Err("The process listing failed.".into()); }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
/// Send one SIGINT to `pid`, here or over ssh.
pub fn signal_int(host: Option<&str>, pid: u32) -> Result<()> {
    let status = match host {
        None => std::process::Command::new("kill").args(["-INT", &pid.to_string()]).status(),
        Some(host) => std::process::Command::new("ssh").args(ssh_args(host, &format!("kill -INT {pid}"))?).status(),
    }.map_err(|e| format!("Could not signal {pid}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("Could not signal {pid}.")) }
}

pub const STOP_WARNING: &str = "Stopping ENDS the Remote Control session: one interrupt stops the turn, the worker exits and the server archives the session (unarchive it in the app to continue, or start a new one).";

/// The result of a stop.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stopped {
    pub session_id: String,
    pub interrupted_pids: Vec<u32>,
}

/// Stop a session: refuses without `yes`; finds the worker in `ps_output`; calls `signal` once per worker.
/// Pure over its inputs: tests pass a recording closure, so no process is ever signalled by a test.
pub fn stop(ps_output: &str, session: &str, yes: bool, signal: &mut dyn FnMut(u32) -> Result<()>) -> Result<Stopped> {
    let session_id = parse_session_id(session)?;
    if !yes { return Err(format!("Refusing to stop without --yes. {STOP_WARNING}")); }
    let workers = workers_for(ps_output, &session_id)?;
    if workers.is_empty() { return Err("No Remote Control worker for this session is running there (use --host for another machine).".into()); }
    let mut sent = Vec::new();
    for w in &workers { signal(w.pid)?; sent.push(w.pid); }
    Ok(Stopped { session_id, interrupted_pids: sent })
}

/// What stops `claude remote-control` from working, as `(variable, why)`. `get` reads the environment.
/// A claude.ai subscription login cannot be seen from the environment; `claude remote-control` reports it itself.
pub fn eligibility(get: &dyn Fn(&str) -> Option<String>) -> Vec<(&'static str, String)> {
    let mut problems = Vec::new();
    if let Some(url) = get("ANTHROPIC_BASE_URL").filter(|v| !v.trim().is_empty()) {
        let host = url.trim().trim_start_matches("https://").trim_start_matches("http://").split(['/', ':']).next().unwrap_or("").to_string();
        if !(url.trim().starts_with("https://") && host == "api.anthropic.com") {
            problems.push(("ANTHROPIC_BASE_URL", "must be unset or point to api.anthropic.com (a proxy URL blocks Remote Control)".to_string()));
        }
    }
    for name in ["DISABLE_GROWTHBOOK", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"] {
        if get(name).is_some_and(|v| !v.is_empty()) { problems.push((name, "must be unset".to_string())); }
    }
    problems
}
/// `Ok` when eligible, else an error naming the first failing variable.
pub fn check_eligibility(get: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    match eligibility(get).first() {
        None => Ok(()),
        Some((name, why)) => Err(format!("Remote Control is blocked by {name}: it {why}.")),
    }
}

/// A server name: letters, digits, space, `.`, `_`, `-`; never a leading dash.
pub fn name_ok(name: &str) -> bool {
    (1..=64).contains(&name.len()) && !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "  65579 /Users/x/.local/share/claude/versions/2.1.318 --print --sdk-url https://api.anthropic.com/v1/code/sessions/cse_01ExampleCCCCCCCCCCCCCCC --replay-user-messages\n  70001 /bin/bash -c echo --sdk-url https://api.anthropic.com/v1/code/sessions/cse_01ExampleCCCCCCCCCCCCCCC\n  70002 /Users/x/.local/bin/claude --print --sdk-url https://api.anthropic.com/v1/code/sessions/cse_01ExampleCCCCCCCCCCCCCCCX\n  70003 /Users/x/.local/bin/claude remote-control --name box\n  70004 /usr/bin/vim notes.txt\n";
    const ID: &str = "session_01ExampleCCCCCCCCCCCCCCC";

    #[test]
    fn discovery_reads_only_claude_workers_from_a_fake_ps() {
        let found = discover(PS);
        assert_eq!(found.iter().map(|w| w.pid).collect::<Vec<_>>(), vec![65579, 70002]);
        assert_eq!(found[0].session_id, ID);
        assert_eq!(found[0].cse_id, "cse_01ExampleCCCCCCCCCCCCCCC");
        assert_eq!(found[0].url, format!("https://claude.ai/code/{ID}"));
        assert!(discover("garbage\n\n 12\n").is_empty());
    }
    #[test]
    fn one_session_means_that_exact_session() {
        for form in [ID, "cse_01ExampleCCCCCCCCCCCCCCC", "https://claude.ai/code/session_01ExampleCCCCCCCCCCCCCCC"] {
            assert_eq!(workers_for(PS, form).unwrap().iter().map(|w| w.pid).collect::<Vec<_>>(), vec![65579], "{form}");
        }
        assert!(workers_for(PS, "session_01NOPEXXXX").unwrap().is_empty());
    }
    #[test]
    fn stop_refuses_without_yes_and_signals_nothing() {
        let mut signalled = Vec::new();
        let err = stop(PS, ID, false, &mut |pid| { signalled.push(pid); Ok(()) }).unwrap_err();
        assert!(err.contains("--yes") && err.contains("ENDS"));
        assert!(signalled.is_empty());
    }
    #[test]
    fn stop_with_yes_signals_exactly_the_worker_once() {
        let mut signalled = Vec::new();
        let done = stop(PS, "cse_01ExampleCCCCCCCCCCCCCCC", true, &mut |pid| { signalled.push(pid); Ok(()) }).unwrap();
        assert_eq!(signalled, vec![65579]);
        assert_eq!(done.interrupted_pids, vec![65579]);
        assert!(stop(PS, "session_01NOPEXXXX", true, &mut |_| panic!("must not signal")).unwrap_err().contains("No Remote Control worker"));
    }
    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }
    #[test]
    fn eligibility_names_the_failing_variable() {
        assert!(check_eligibility(&env(&[])).is_ok());
        assert!(check_eligibility(&env(&[("ANTHROPIC_BASE_URL", "https://api.anthropic.com")])).is_ok());
        assert!(check_eligibility(&env(&[("ANTHROPIC_BASE_URL", "")])).is_ok());
        assert!(check_eligibility(&env(&[("ANTHROPIC_BASE_URL", "http://localhost:8080")])).unwrap_err().contains("ANTHROPIC_BASE_URL"));
        assert!(check_eligibility(&env(&[("ANTHROPIC_BASE_URL", "https://api.anthropic.com.evil.example")])).unwrap_err().contains("ANTHROPIC_BASE_URL"));
        assert!(check_eligibility(&env(&[("DISABLE_GROWTHBOOK", "1")])).unwrap_err().contains("DISABLE_GROWTHBOOK"));
        assert!(check_eligibility(&env(&[("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")])).unwrap_err().contains("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"));
    }
    #[test]
    fn ssh_hosts_and_names_are_validated() {
        assert!(ssh_args("-oProxyCommand=x", "ps").is_err());
        assert!(ssh_args("host; rm", "ps").is_err());
        let args = ssh_args("user@box.local", PS_COMMAND).unwrap();
        assert_eq!(args[0], "-n");
        assert_eq!(args[args.len() - 2], "user@box.local");
        assert!(name_ok("my box") && !name_ok("--evil") && !name_ok(""));
        assert!(args.contains(&"BatchMode=yes".to_string()) && args.contains(&"ConnectTimeout=5".to_string()));
        assert_eq!(args[args.len() - 3], "--");
    }
    #[test]
    fn host_names_refuse_options_and_odd_characters() {
        for ok in ["mac-a", "mac-b.local", "vm_1", "user@mac-a", "10.0.0.7"] { assert!(host_ok(ok), "{ok}"); }
        for bad in ["-oProxyCommand=touch x", "-oProxyCommand=x", "mac-a -oProxyCommand=x", "mac a", "a;b", "a|b", "$(id)", "`id`", "a/b", "@mac", "user@", "user@-x", "-", "", "[::1]", "a:22", "a\nb"] {
            assert!(!host_ok(bad), "{bad:?}");
            assert!(ssh_args(bad, "ps").is_err(), "{bad:?}");
        }
        assert!(hosts_from_list("mac-a,-oProxyCommand=x").unwrap_err().contains("Not a valid ssh host name"));
        assert!(hosts_from_file("mac-a\n-oProxyCommand=x\n").is_err());
    }
    #[test]
    fn host_lists_and_files_parse_and_merge_in_order() {
        assert_eq!(hosts_from_list("mac-a, mac-b,,vm-1 ").unwrap(), ["mac-a", "mac-b", "vm-1"]);
        assert_eq!(hosts_from_list("").unwrap(), Vec::<String>::new());
        assert_eq!(hosts_from_file("# my machines\nmac-b\n\n  vm-1   # the VM\nmac-a\n").unwrap(), ["mac-b", "vm-1", "mac-a"]);
        let merged = merge_hosts(vec![vec!["mac-a".into()], vec!["mac-b".into(), "mac-a".into()], vec!["vm-1".into(), "mac-b".into()]]);
        assert_eq!(merged, ["mac-a", "mac-b", "vm-1"]);
    }
    #[test]
    fn a_slow_command_is_stopped_at_the_limit() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 5"]);
        let t = std::time::Instant::now();
        assert_eq!(output_within(cmd, std::time::Duration::from_millis(300)).unwrap_err(), "timed out");
        assert!(t.elapsed() < std::time::Duration::from_secs(3));
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "echo out; echo err >&2"]);
        let o = output_within(cmd, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!((o.stdout.as_slice(), o.stderr.as_slice()), (&b"out\n"[..], &b"err\n"[..]));
    }
    #[test]
    fn ssh_failures_become_short_reasons() {
        assert_eq!(ssh_reason(b"ssh: connect to host vm-1 port 22: Operation timed out\n"), "ssh timed out");
        assert_eq!(ssh_reason(b"ssh: Could not resolve hostname nope: nodename nor servname provided\n"), "ssh: unknown host");
        assert_eq!(ssh_reason(b"admin@vm-1: Permission denied (publickey).\n"), "ssh: login refused (BatchMode: keys only)");
        assert_eq!(ssh_reason(b""), "ssh failed");
    }
}
