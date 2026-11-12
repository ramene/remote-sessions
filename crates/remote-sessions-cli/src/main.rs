//! `remote-sessions serve|list|send|events|stop|mcp`. JSON on stdout; failures `{"ok":false,"error":...}` with exit 1.
use remote_sessions_cli::{attach::{attach, attach_auto, LiveOpts}, ctx_from_env, help::{self, USAGE}, mcp, parse_args, recordings, run, serve_plan, which};
use serde_json::{json, Value};
use std::io::{stdin, stdout};

fn fail(message: &str) -> ! {
    println!("{}", json!({"ok": false, "error": message}));
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else { eprintln!("{USAGE}"); std::process::exit(2) };
    let rest = &args[1..];
    // Help first: `help [<command>]`, and `<command> --help` or `-h` for every command (never runs the command).
    match command {
        "-h" | "--help" => { println!("{USAGE}"); return; }
        "help" => {
            match rest.first().map(String::as_str).filter(|a| !matches!(*a, "-h" | "--help")) {
                None => println!("{USAGE}"),
                Some(name) => match help::command_help(name) { Some(text) => println!("{text}"), None => fail(&help::unknown_command(name)) },
            }
            return;
        }
        _ => if help::wants_help(rest) { if let Some(text) = help::command_help(command) { println!("{text}"); return; } },
    }
    let ctx = ctx_from_env();
    let outcome: Result<Value, String> = match command {
        "-V" | "--version" | "version" => { println!("remote-sessions {}", env!("CARGO_PKG_VERSION")); return; }
        "mcp" => {
            if let Err(error) = mcp::serve(&ctx, &mut stdin().lock(), &mut stdout().lock()) { fail(&error.to_string()); }
            return;
        }
        "recordings" if rest.first().map(String::as_str) == Some("serve") => {
            let parsed = match parse_args(&rest[1..]) { Ok(p) => p, Err(e) => fail(&e) };
            let bind = parsed.values.get("bind").map_or(recordings::DEFAULT_BIND, String::as_str);
            let token_file = parsed.values.get("token-file").map(std::path::PathBuf::from).unwrap_or_else(recordings::default_token_file);
            if let Err(e) = recordings::serve(ctx.recordings.clone(), bind, &token_file) { fail(&e); }
            return;
        }
        "control" => {
            let parsed = match parse_args(rest) { Ok(p) => p, Err(e) => fail(&e) };
            let flag = parsed.values.get("host").cloned();
            let demo = parsed.switch("demo");
            // This machine always; more machines from --host, REMOTE_SESSIONS_HOSTS and ~/.config/remote-sessions/hosts.
            let (hosts, local) = if demo { (vec![], String::new()) } else {
                match remote_sessions_cli::configured_hosts(&ctx, flag.as_deref()) { Ok(h) => (h, remote_sessions_cli::local_label()), Err(e) => fail(&e) }
            };
            // An extra panel (private build) reads one host: the --host, when exactly one was given.
            let host = flag.filter(|f| remote_sessions_core::rc::hosts_from_list(f).is_ok_and(|v| v.len() == 1));
            let label = if hosts.is_empty() { host.clone().unwrap_or_default() } else { format!("{local} +{}", hosts.len()) };
            let backend: std::sync::Arc<dyn sessions_tui::room::RoomBackend> = if demo { std::sync::Arc::new(remote_sessions_cli::control_room::DemoRoom) } else { std::sync::Arc::new(remote_sessions_cli::control_room::LiveRoom::new(ctx, host.clone()).with_hosts(hosts, local).with_all(parsed.switch("all")).with_tracked(parsed.switch("tracked")).with_routines(parsed.switch("routines")).with_no_routines(parsed.switch("no-routines"))) };
            if parsed.switch("once") {
                let size = match parsed.values.get("size") { Some(s) => s.split_once("x").and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?))).filter(|(w, h)| *w >= 30 && *h >= 9 && *w <= 400 && *h <= 200).unwrap_or_else(|| fail("--size must look like 100x28 (at least 30x9).")), None => (100, 28) };
                let th = sessions_tui::theme::Theme::detect(parsed.switch("no-color"), parsed.switch("ascii"));
                let ansi = th.color && (parsed.switch("ansi") || std::io::IsTerminal::is_terminal(&stdout()));
                let now = if demo { remote_sessions_cli::control_room::DEMO_NOW_MS } else { sessions_tui::room_app::now_ms() };
                print!("{}", remote_sessions_cli::control_room::render_once(&*backend, &label, demo, size, &th, ansi, now, None));
                return;
            }
            let opts = sessions_tui::RoomOptions { host: label, no_color: parsed.switch("no-color"), ascii: parsed.switch("ascii"), demo };
            if let Err(e) = sessions_tui::run_room(backend, opts) { fail(&e); }
            return;
        }
        "attach" => {
            let parsed = match parse_args(rest) { Ok(p) => p, Err(e) => fail(&e) };
            let session = parsed.positional.first().cloned().unwrap_or_else(|| fail("attach <session> [--interval 1]"));
            let seconds = parsed.values.get("interval").map(|v| v.parse::<f64>().ok().filter(|n| n.is_finite() && (0.2..=60.0).contains(n)).unwrap_or_else(|| fail("--interval must be 0.2-60 seconds."))).unwrap_or(1.0);
            let client = match ctx.client() { Ok(c) => c, Err(e) => fail(&e) };
            if parsed.switch("tui") && std::io::IsTerminal::is_terminal(&stdout()) {
                let backend = std::sync::Arc::new(remote_sessions_cli::tui::RcBackend { client, session: session.clone(), host: parsed.values.get("host").cloned().unwrap_or_else(|| "remote".into()) });
                let opts = sessions_tui::Options { no_color: parsed.switch("no-color"), ascii: parsed.switch("ascii"), ..Default::default() };
                match sessions_tui::run(backend, opts) {
                    Ok(out) => if let Some(why) = out.fatal { eprintln!("{why}"); std::process::exit(2); },
                    Err(e) => fail(&e),
                }
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || { use std::io::BufRead; for line in stdin().lock().lines() { match line { Ok(l) => if tx.send(l).is_err() { break; }, Err(_) => break } } });
            let interval = std::time::Duration::from_secs_f64(seconds);
            let result = if parsed.switch("poll") { attach(&client, &session, interval, &rx, &mut stdout().lock(), None) } else { attach_auto(&client, &session, interval, &rx, &mut stdout().lock(), &LiveOpts::default(), None) };
            if let Err(e) = result { fail(&e); }
            return;
        }
        "stream" => {
            let parsed = match parse_args(rest) { Ok(p) => p, Err(e) => fail(&e) };
            let session = parsed.positional.first().cloned().unwrap_or_else(|| fail("stream <session> [--from <seq>] [--for <seconds>]"));
            let from = parsed.values.get("from").map(|v| v.parse::<u64>().unwrap_or_else(|_| fail("--from must be a sequence number.")));
            let limit = parsed.values.get("for").map(|v| v.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).map(std::time::Duration::from_secs_f64).unwrap_or_else(|| fail("--for must be a number of seconds above 0.")));
            let client = match ctx.client() { Ok(c) => c, Err(e) => fail(&e) };
            if let Err(e) = remote_sessions_cli::stream::stream_with(&client, &session, from, limit, Default::default(), std::time::Duration::from_secs(2), &mut stdout().lock()) { fail(&e); }
            return;
        }
        "serve" => match parse_args(rest) {
            Ok(parsed) if !parsed.switch("check") => {
                let plan = match serve_plan(&ctx, &parsed) { Ok(p) => p, Err(e) => fail(&e) };
                let claude = ctx.claude.clone().or_else(|| which("claude")).unwrap_or_else(|| fail("Claude Code (`claude`) was not found on PATH."));
                use std::os::unix::process::CommandExt;
                let error = std::process::Command::new(claude).args(plan).exec();
                fail(&format!("Could not start claude: {error}"));
            }
            _ => run(&ctx, command, rest, &mut stdin().lock()),
        },
        other => run(&ctx, other, rest, &mut stdin().lock()),
    };
    match outcome {
        Ok(value) => { println!("{value}"); if value["ok"] == false { std::process::exit(1); } }
        Err(error) => fail(&error),
    }
}
