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

/// Where the process listing comes from: this machine, or a host over `ssh -n`.
pub fn host_ok(host: &str) -> bool {
    (1..=253).contains(&host.len()) && !host.starts_with('-') && host.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-@:[]".contains(&b))
}
/// `ssh` arguments that run `remote_cmd` on `host` without reading stdin (`-n`); `--` keeps the host from being an option.
pub fn ssh_args(host: &str, remote_cmd: &str) -> Result<Vec<String>> {
    if !host_ok(host) { return Err("That is not a valid ssh host name.".into()); }
    Ok(vec!["-n".into(), "-o".into(), "BatchMode=yes".into(), "--".into(), host.into(), remote_cmd.into()])
}
pub const PS_COMMAND: &str = "ps -axo pid=,command=";

/// Run `ps` here (`host` None) or over ssh.
pub fn ps_listing(host: Option<&str>) -> Result<String> {
    let output = match host {
        None => std::process::Command::new("ps").args(["-axo", "pid=,command="]).output(),
        Some(host) => std::process::Command::new("ssh").args(ssh_args(host, PS_COMMAND)?).output(),
    }.map_err(|e| format!("Could not list processes: {e}"))?;
    if !output.status.success() && output.stdout.is_empty() { return Err("The process listing failed (is the host reachable by ssh?).".into()); }
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
    }
}
