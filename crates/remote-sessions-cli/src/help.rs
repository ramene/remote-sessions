//! Help text: the top-level usage, and the help of one command (`<command> --help`, `<command> -h`,
//! `help <command>`): its usage lines from [`USAGE`], its options, and, for the full-screen commands, its keys.
use super::{SWITCH_FLAGS, VALUE_FLAGS};

pub const USAGE: &str = "usage: remote-sessions <command>
  serve  [--name <n>] [--check]                  check eligibility, then run `claude remote-control --name <n>` (--check: only check)
  list   [--host <h>[,<h>...]]                  Remote Control workers (found by --sdk-url .../sessions/cse_<id>): this machine plus REMOTE_SESSIONS_HOSTS and ~/.config/remote-sessions/hosts; --host: only those machines (ssh -n)
  send   <session> --message <text|->            send a message (arrives as a PEER message; it cannot approve permissions)
  events <session> [--after <event id>]          read events (paged; needs a token, or use /remote-read in Claude Code)
  read   <session> [--after <cursor>]          the conversation as text lines, every page (paged)
  cast   <session uuid> [--host <h>] [--out <file>] [--idle-max <s>] [--mp4 <file>]   record a session's local transcript as an asciicast v2 file
  recordings list | start --kind <pane|vm|cloud|rc> --host <h> --session <s> | stop <id>   the fleet's cast recordings (ssh)
  recordings serve [--bind 127.0.0.1:3790] [--token-file <path>]   HTTP API + web player over the recordings (Bearer token, 0600 file)
  pending   <session>                            unanswered permission prompts (tool + input); needs a claude.ai login token
  approve   <session> [--request <id>]           allow the newest pending prompt (or the named one)
  deny      <session> [--request <id>] [--message <m>]   deny it
  say       <session> <text|->                   send a real user turn (not a peer message)
  interrupt <session>                            Esc, not kill
  mode   <session> <default|acceptEdits|auto|plan|dontAsk|bypassPermissions>   set the permission mode (default = Manual)
  stream    <session> [--from <seq>] [--for <s>]  live events, one JSON line each as they arrive (events/stream: message by message, not token by token); falls back to polling
  attach    <session> [--interval 1] [--poll] [--tui [--host <label>] [--no-color] [--ascii]]    terminal client (--tui: full-screen UI): live view (events/stream; polls if it cannot, and says so), sends typed lines, asks Allow? [y/n]; Ctrl-C leaves without stopping
  stop   <session> --yes [--host <ssh-host>]     one SIGINT to the worker: ENDS the session, the server archives it
  control [--host <h>[,<h>...]] [--tracked] [--all] [--no-routines] [--routines] [--no-color] [--ascii] [--once [--size WxH] [--ansi]] [--demo]   the control room: Remote Control sessions of this machine and of each --host, REMOTE_SESSIONS_HOSTS and ~/.config/remote-sessions/hosts host (ssh -n), and cloud sessions, in two panels; arrows or click to select, Enter or a second click to drop in, a/d approve or deny, s send to a cloud session, q/Esc/click x close. --once prints one frame (--ansi: with colors even when piped). By default it shows every session of your claude.ai account (names, repo, state; the account's live Remote Control sessions on other machines as account rows) and the Routines, when cloud-sessions can read the account (cloud-sessions auth use-claude-login); else the sessions tracked here, with one line that says why. --tracked (or REMOTE_SESSIONS_CLOUD_ALL=0): only the sessions tracked on this machine; --all: accepted, the default; --no-routines (or REMOTE_SESSIONS_ROUTINES=0, or R in the room) hides the Routines; --routines: accepted, the default; --demo uses placeholder sessions
  mcp                                            run as an MCP server over stdio
  help [<command>]                               this text, or the help of one command (also <command> --help or -h)";

/// Every command that has a line in [`USAGE`], in that order.
pub const COMMANDS: [&str; 19] = [
    "serve", "list", "send", "events", "read", "cast", "recordings", "pending", "approve", "deny", "say", "interrupt",
    "mode", "stream", "attach", "stop", "control", "mcp", "help",
];

/// True when the arguments after the command ask for help: a `--help` or `-h` that is not the value of a value option
/// (in `send <s> --message --help` the text is "--help").
pub fn wants_help(args: &[String]) -> bool {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--help" || arg == "-h" { return true; }
        if arg.strip_prefix("--").is_some_and(|name| VALUE_FLAGS.contains(&name)) { iter.next(); }
    }
    false
}

/// The lines of [`USAGE`] for one command (more than one for `recordings`).
pub fn usage_lines(command: &str) -> Vec<&'static str> {
    USAGE.lines().skip(1).map(str::trim_start).filter(|l| l.split_whitespace().next() == Some(command)).collect()
}

/// The option names (without `--`) in a command's usage lines, first use first, each once. Only real options count
/// (the `--sdk-url` in the text of `list` is not one).
pub fn options_of(command: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for line in usage_lines(command) {
        for word in line.split(|c: char| c.is_whitespace() || c == '[' || c == ']' || c == ',' || c == ';' || c == '(' || c == ')' || c == ':') {
            let Some(name) = word.strip_prefix("--") else { continue };
            let known = VALUE_FLAGS.iter().chain(SWITCH_FLAGS.iter()).find(|f| **f == name);
            if let Some(f) = known { if !out.contains(f) { out.push(f); } }
        }
    }
    out
}

/// One option of one command: (left column, text). None for an option this table does not know.
pub fn option_text(command: &str, name: &str) -> Option<(&'static str, &'static str)> {
    Some(match (command, name) {
        ("serve", "name") => ("--name <n>", "the session name: 1-64 letters, digits, spaces, '.', '_' or '-' (default remote-session)"),
        ("serve", "check") => ("--check", "only check eligibility; do not start claude"),
        ("stop", "host") => ("--host <ssh-host>", "run on that machine over ssh -n (an ssh config name)"),
        ("list", "host") => ("--host <h>[,<h>...]", "only these machines, over ssh -n (ssh config names; comma list, or --host again)"),
        ("cast", "host") => ("--host <h>", "read the transcript on that machine over ssh"),
        ("recordings", "host") => ("--host <h>", "the machine to record on (start)"),
        ("attach", "host") => ("--host <label>", "with --tui: the host name to show in the footer"),
        ("control", "host") => ("--host <h>[,<h>...]", "more machines beside this one, over ssh -n (also REMOTE_SESSIONS_HOSTS and ~/.config/remote-sessions/hosts)"),
        ("send", "message") => ("--message <text|->", "the text to send; - reads it from stdin"),
        ("deny", "message") => ("--message <m>", "the reason to give with the deny"),
        ("events", "after") => ("--after <event id>", "start after this event"),
        ("read", "after") => ("--after <cursor>", "start after this cursor"),
        (_, "out") => ("--out <file>", "the .cast file to write"),
        (_, "idle-max") => ("--idle-max <s>", "cap each idle gap in the recording at this many seconds"),
        (_, "mp4") => ("--mp4 <file>", "also render an MP4 to this file"),
        (_, "kind") => ("--kind <pane|vm|cloud|rc>", "what to record (start)"),
        (_, "session") => ("--session <s>", "the session to record (start)"),
        (_, "bind") => ("--bind <addr:port>", "the address to listen on (serve; default 127.0.0.1:3790)"),
        (_, "token-file") => ("--token-file <path>", "the Bearer token file, mode 0600 (serve)"),
        (_, "request") => ("--request <id>", "the prompt to answer (default: the newest pending one)"),
        (_, "from") => ("--from <seq>", "start at this sequence number"),
        (_, "for") => ("--for <s>", "stop after this many seconds"),
        (_, "interval") => ("--interval <s>", "the poll interval, 0.2-60 seconds (default 1)"),
        (_, "poll") => ("--poll", "poll the events API; do not use events/stream"),
        (_, "tui") => ("--tui", "the full-screen terminal UI (without a terminal: plain attach)"),
        (_, "no-color") => ("--no-color", "no colors (also NO_COLOR)"),
        (_, "ascii") => ("--ascii", "ASCII glyphs only, no colors"),
        (_, "yes") => ("--yes", "required: stop ENDS the session"),
        (_, "tracked") => ("--tracked", "only the cloud sessions tracked on this machine (also REMOTE_SESSIONS_CLOUD_ALL=0); default: every session of the account"),
        (_, "all") => ("--all", "every session of the account: the default (kept so old commands work)"),
        (_, "no-routines") => ("--no-routines", "hide the Routines (also R in the room, or REMOTE_SESSIONS_ROUTINES=0)"),
        (_, "routines") => ("--routines", "show the Routines and their newest runs: the default (kept so old commands work)"),
        (_, "once") => ("--once", "print one frame and exit"),
        (_, "size") => ("--size <WxH>", "with --once: the frame size, at least 30x9 (default 100x28)"),
        (_, "ansi") => ("--ansi", "with --once: colors even when stdout is not a terminal"),
        (_, "demo") => ("--demo", "placeholder sessions; no network, no ssh"),
        _ => return None,
    })
}

/// The keys of the full-screen commands.
fn keys_of(command: &str) -> &'static [(&'static str, &'static str)] {
    match command {
        "control" => &[
            ("Up/Down, k/j, click", "select a row"),
            ("PgUp/PgDn, Home/End, g/G", "page; first and last row"),
            ("Tab", "the next panel"),
            ("Enter, a second click", "open chat: the live two-way chat with the session (Remote Control: attach <id> --tui; cloud: in the room); Esc goes back"),
            ("a / d", "approve or deny the waiting prompt of the selected Remote Control session; y confirms, n or Esc cancels"),
            ("s", "send a follow-up to the selected cloud session"),
            ("r", "refresh"),
            ("R", "show or hide routines"),
            ("wheel", "scroll the panel under the pointer"),
            ("q, Esc, Ctrl-C, click x", "close the room"),
        ],
        "attach" => &[
            ("Enter", "send (plain attach: one typed line is one message; y or n answers Allow? [y/n])"),
            ("Shift-Enter, Alt-Enter", "new line (--tui)"),
            ("Esc", "interrupt the running turn (--tui)"),
            ("Tab", "cycle the permission mode (--tui)"),
            ("Ctrl-O", "fold and unfold tool results (--tui)"),
            ("PgUp/PgDn", "scroll (--tui)"),
            ("Ctrl-C", "leave; the session does not stop"),
        ],
        _ => &[],
    }
}

fn table(out: &mut String, title: &str, rows: &[(&str, &str)]) {
    let width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    out.push_str(&format!("\n{title}:\n"));
    for (left, text) in rows { out.push_str(&format!("  {left:width$}  {text}\n")); }
}

/// The help of one command, or None for a name that is not a command. `help` gives [`USAGE`].
pub fn command_help(command: &str) -> Option<String> {
    if command == "help" { return Some(USAGE.to_string()); }
    let lines = usage_lines(command);
    if lines.is_empty() { return None; }
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(if i == 0 { "usage: remote-sessions " } else { "       remote-sessions " });
        out.push_str(line);
        out.push('\n');
    }
    let mut rows: Vec<(&str, &str)> = options_of(command).into_iter().filter_map(|name| option_text(command, name)).collect();
    rows.push(("-h, --help", "this help"));
    table(&mut out, "options", &rows);
    let keys = keys_of(command);
    if !keys.is_empty() { table(&mut out, "keys", keys); }
    Some(out.trim_end().to_string())
}

/// The error for `help <name>` when the name is not a command.
pub fn unknown_command(name: &str) -> String {
    format!("Unknown command {name}. Use {}.", COMMANDS.join(", "))
}
