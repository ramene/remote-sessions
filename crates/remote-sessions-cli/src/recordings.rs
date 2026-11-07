//! `remote-sessions recordings serve`: a small HTTP API over the fleet's asciicast recordings.
//!
//! * `POST   /recordings {kind: pane|vm|cloud|rc, host, session}` start (pane, vm) or make (cloud, rc) a recording -> `{id}`
//! * `DELETE /recordings/<id>` stop a pane or vm recording
//! * `GET    /recordings` list the casts under `~/.local/share/tmux-logs/<Y>/<M>/<D>/cast/` on each host
//! * `GET    /recordings/<id>.cast`, `.mp4`, `.gif` the file (mp4 and gif are rendered by cast2mp4 and cached; 501 if agg/ffmpeg are missing)
//! * `GET    /recordings/<id>/live` server-sent events: the lines of the cast, then new ones as the file grows
//! * `GET    /play/<id>` a page with the vendored asciinema-player (Apache-2.0, see `vendor/asciinema-player/LICENSE`)
//!
//! Every route except the player's own static files needs `Authorization: Bearer <token>` (or `?token=` for the browser page). The token
//! lives in a 0600 file, is generated when missing and is never printed. Cast ids are `<host>.<YYYYMMDD>.<file name>`; every part is
//! validated, so an id can never name a path outside the cast folders. [`handle`] is pure (a [`Backend`] does the ssh), so the router
//! is tested without a network.
use crate::Result;
use remote_sessions_core::cast::Mp4Error;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const PLAYER_JS: &[u8] = include_bytes!("../vendor/asciinema-player/asciinema-player.min.js");
const PLAYER_CSS: &[u8] = include_bytes!("../vendor/asciinema-player/asciinema-player.css");
const PLAYER_LICENSE: &[u8] = include_bytes!("../vendor/asciinema-player/LICENSE");
pub const DEFAULT_BIND: &str = "127.0.0.1:3790";
pub const DEFAULT_HOSTS: [&str; 1] = ["localhost"];
const CAST_ROOT: &str = "$HOME/.local/share/tmux-logs";

#[derive(Clone, Debug, PartialEq)]
pub struct CastInfo { pub id: String, pub host: String, pub name: String, pub date: String, pub size: u64, pub frames: u64, pub duration: f64 }
#[derive(Default)]
pub struct Listing { pub casts: Vec<CastInfo>, pub errors: Vec<String> }

/// What the API needs from the machines. [`SshBackend`] is the real one; tests use a fake.
pub trait Backend: Send + Sync {
    fn list(&self) -> Listing;
    /// The cast's bytes from `from` on.
    fn read(&self, id: &str, from: u64) -> Result<Vec<u8>>;
    /// Start (pane, vm) or make (cloud, rc) a recording; returns its id.
    fn start(&self, kind: &str, host: &str, session: &str) -> Result<String>;
    fn stop(&self, id: &str) -> Result<()>;
    /// The cast rendered as `mp4` or `gif`.
    fn media(&self, id: &str, ext: &str) -> std::result::Result<Vec<u8>, Mp4Error>;
}

pub struct Request { pub method: String, pub path: String, pub query: String, pub authorization: Option<String>, pub body: Vec<u8> }
pub enum Body { Bytes(Vec<u8>), Live(String) }
pub struct Response { pub status: u16, pub content_type: &'static str, pub body: Body }

fn bytes(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response { Response { status, content_type, body: Body::Bytes(body.into()) } }
fn json_response(status: u16, value: Value) -> Response { bytes(status, "application/json", value.to_string()) }
fn error(status: u16, message: &str) -> Response { json_response(status, json!({"ok": false, "error": message})) }

/// `<host>.<YYYYMMDD>.<name>` -> its three parts, or None when any part could name something else.
pub fn parse_id(id: &str) -> Option<(&str, &str, &str)> {
    let mut parts = id.splitn(3, '.');
    let (host, date, name) = (parts.next()?, parts.next()?, parts.next()?);
    let word = |s: &str, max: usize| (1..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (word(host, 64) && date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) && word(name, 200)).then_some((host, date, name))
}
fn cast_path(date: &str, name: &str) -> String { format!("{CAST_ROOT}/{}/{}/{}/cast/{name}.cast", &date[0..4], &date[4..6], &date[6..8]) }

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| pair.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}
fn authorized(req: &Request, token: &str) -> bool {
    let given = req.authorization.as_deref().and_then(|h| h.strip_prefix("Bearer ")).or_else(|| query_param(&req.query, "token"));
    given.is_some_and(|g| constant_time_eq(g.trim().as_bytes(), token.as_bytes()))
}

fn page(id: &str) -> String {
    format!("<!doctype html><meta charset=utf-8><title>{id}</title><link rel=stylesheet href=\"/play/assets/asciinema-player.css\">\
<body style=\"margin:0;background:#121314\"><div id=p></div><script src=\"/play/assets/asciinema-player.min.js\"></script>\
<script>const t=new URLSearchParams(location.search).get('token')||'';\
AsciinemaPlayer.create('/recordings/{id}.cast?token='+encodeURIComponent(t),document.getElementById('p'),{{idleTimeLimit:1.5,fit:'width',autoPlay:true}});</script>")
}

/// The router: one request -> one response. Pure; the backend does the work.
pub fn handle(backend: &dyn Backend, token: &str, req: &Request) -> Response {
    if let Some(asset) = req.path.strip_prefix("/play/assets/") {
        if req.method != "GET" { return error(405, "GET only."); }
        return match asset {
            "asciinema-player.min.js" => bytes(200, "text/javascript", PLAYER_JS),
            "asciinema-player.css" => bytes(200, "text/css", PLAYER_CSS),
            "LICENSE" => bytes(200, "text/plain; charset=utf-8", PLAYER_LICENSE),
            _ => error(404, "No such asset."),
        };
    }
    if !authorized(req, token) { return error(401, "A valid Bearer token is required."); }
    let rest = req.path.strip_prefix("/recordings").map(|r| r.trim_start_matches('/'));
    match (req.method.as_str(), rest) {
        ("GET", Some("")) => {
            let listing = backend.list();
            let casts: Vec<Value> = listing.casts.iter().map(|c| json!({"id": c.id, "host": c.host, "name": c.name, "date": c.date, "size": c.size, "frames": c.frames, "duration": c.duration})).collect();
            json_response(200, json!({"ok": true, "recordings": casts, "errors": listing.errors}))
        }
        ("POST", Some("")) => {
            let Ok(body) = serde_json::from_slice::<Value>(&req.body) else { return error(400, "The body must be JSON: {kind, host, session}."); };
            let field = |k: &str| body[k].as_str().unwrap_or("");
            let (kind, host, session) = (field("kind"), field("host"), field("session"));
            if !["pane", "vm", "cloud", "rc"].contains(&kind) { return error(400, "kind must be pane, vm, cloud or rc."); }
            if host.is_empty() || session.is_empty() { return error(400, "host and session are required."); }
            match backend.start(kind, host, session) { Ok(id) => json_response(201, json!({"ok": true, "id": id})), Err(e) => error(502, &e) }
        }
        ("DELETE", Some(id)) if parse_id(id).is_some() => match backend.stop(id) { Ok(()) => json_response(200, json!({"ok": true, "id": id, "stopped": true})), Err(e) => error(502, &e) },
        ("GET", Some(r)) if r.ends_with("/live") && parse_id(r.trim_end_matches("/live")).is_some() => {
            Response { status: 200, content_type: "text/event-stream", body: Body::Live(r.trim_end_matches("/live").to_string()) }
        }
        ("GET", Some(r)) => {
            let Some((id, ext)) = r.rsplit_once('.').filter(|(id, ext)| parse_id(id).is_some() && ["cast", "mp4", "gif"].contains(ext)) else { return error(404, "Not found."); };
            match ext {
                "cast" => match backend.read(id, 0) { Ok(data) => bytes(200, "application/x-asciicast", data), Err(e) => error(404, &e) },
                _ => match backend.media(id, ext) {
                    Ok(data) => bytes(200, if ext == "mp4" { "video/mp4" } else { "image/gif" }, data),
                    Err(Mp4Error::Missing(m)) => error(501, &m),
                    Err(Mp4Error::Failed(m)) => error(502, &m),
                },
            }
        }
        (_, Some(_)) => error(404, "Not found."),
        _ => match (req.method.as_str(), req.path.strip_prefix("/play/")) {
            ("GET", Some(id)) if parse_id(id).is_some() => bytes(200, "text/html; charset=utf-8", page(id)),
            _ => error(404, "Not found."),
        },
    }
}

// ---- tokens ----

fn private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_err(|e| e.to_string())?.permissions().mode();
        if mode & 0o077 != 0 { return Err(format!("The token file {} must be private: run chmod 600 on it.", path.display())); }
    }
    Ok(())
}
/// The Bearer token: read from the file, or generated (32 random bytes from the system, hex) into a new 0600 file. Never printed.
pub fn load_or_create_token(path: &Path) -> Result<String> {
    if path.exists() {
        private(path)?;
        let token = std::fs::read_to_string(path).map_err(|e| format!("Could not read the token file: {e}"))?.trim().to_string();
        return if token.len() >= 32 && token.bytes().all(|b| b.is_ascii_graphic()) { Ok(token) } else { Err("The token file does not hold a token (32+ characters).".into()) };
    }
    let mut raw = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut raw)).map_err(|e| format!("No system randomness: {e}"))?;
    let token: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) { std::fs::create_dir_all(dir).map_err(|e| format!("Could not create the token folder: {e}"))?; }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path).map_err(|e| format!("Could not create the token file: {e}"))?;
        writeln!(file, "{token}").map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, format!("{token}\n")).map_err(|e| e.to_string())?;
    Ok(token)
}
pub fn default_token_file() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config/remote-sessions/recordings.token")
}

// ---- the server ----

/// Only a loopback address may be bound: the token travels in clear over plain HTTP (reach it from elsewhere with an ssh tunnel).
pub fn check_bind(bind: &str) -> Result<SocketAddr> {
    let addr: SocketAddr = bind.parse().map_err(|_| "--bind must look like 127.0.0.1:3790.".to_string())?;
    if !addr.ip().is_loopback() { return Err("recordings serve binds loopback only (the token travels over plain HTTP); reach it from another machine with an ssh tunnel.".into()); }
    Ok(addr)
}

fn read_request(stream: &TcpStream) -> Result<Request> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?.take(80 * 1024));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    let (mut authorization, mut length) = (None, 0usize);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).map_err(|e| e.to_string())? == 0 || header.trim().is_empty() { break; }
        if let Some((name, value)) = header.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() { "authorization" => authorization = Some(value.trim().to_string()), "content-length" => length = value.trim().parse().unwrap_or(0), _ => {} }
        }
    }
    if length > 64 * 1024 { return Err("Request body too large.".into()); }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    Ok(Request { method, path: path.to_string(), query: query.to_string(), authorization, body })
}
const REASONS: [(u16, &str); 8] = [(200, "OK"), (201, "Created"), (400, "Bad Request"), (401, "Unauthorized"), (404, "Not Found"), (405, "Method Not Allowed"), (501, "Not Implemented"), (502, "Bad Gateway")];
fn write_head(out: &mut dyn Write, status: u16, content_type: &str, length: Option<usize>) -> std::io::Result<()> {
    let reason = REASONS.iter().find(|(s, _)| *s == status).map_or("OK", |(_, r)| r);
    write!(out, "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCache-Control: no-store\r\nConnection: close\r\n")?;
    if let Some(n) = length { write!(out, "Content-Length: {n}\r\n")?; }
    write!(out, "\r\n")
}
/// Follow a cast: send what is there, then new lines as the file grows, until the client leaves or an hour passes.
fn stream_live(backend: &dyn Backend, id: &str, out: &mut dyn Write) -> std::io::Result<()> {
    let (mut offset, mut pending, mut quiet) = (0u64, Vec::<u8>::new(), 0u32);
    for _ in 0..3600 {
        let chunk = backend.read(id, offset).unwrap_or_default();
        offset += chunk.len() as u64;
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]);
            if !text.trim().is_empty() { write!(out, "data: {text}\n\n")?; }
            quiet = 0;
        }
        quiet += 1;
        if quiet % 15 == 0 { write!(out, ": keepalive\n\n")?; }
        out.flush()?;
        std::thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}
fn serve_connection(backend: &dyn Backend, token: &str, mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let response = match read_request(&stream) { Ok(req) => handle(backend, token, &req), Err(e) => error(400, &e) };
    let _ = match response.body {
        Body::Bytes(data) => write_head(&mut stream, response.status, response.content_type, Some(data.len())).and_then(|_| stream.write_all(&data)),
        Body::Live(id) => write_head(&mut stream, response.status, response.content_type, None).and_then(|_| stream_live(backend, &id, &mut stream)),
    };
}
/// Bind and serve until killed. Prints one line (address and token file, never the token).
pub fn serve(backend: Arc<dyn Backend>, bind: &str, token_file: &Path) -> Result<()> {
    let addr = check_bind(bind)?;
    let token = Arc::new(load_or_create_token(token_file)?);
    let listener = TcpListener::bind(addr).map_err(|e| format!("Could not bind {addr}: {e}"))?;
    println!("{}", json!({"ok": true, "listening": format!("http://{addr}"), "token_file": token_file, "routes": ["GET|POST /recordings", "DELETE /recordings/<id>", "GET /recordings/<id>.cast|.mp4|.gif", "GET /recordings/<id>/live", "GET /play/<id>"]}));
    for stream in listener.incoming().flatten() {
        let (backend, token) = (backend.clone(), token.clone());
        std::thread::spawn(move || serve_connection(&*backend, &token, stream));
    }
    Ok(())
}

// ---- the real backend: ssh to each host ----

fn ssh_sh(host: &str, script: &str) -> Result<Vec<u8>> {
    if !remote_sessions_core::rc::host_ok(host) { return Err("That is not a valid ssh host name.".into()); }
    let mut child = std::process::Command::new("ssh").args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "--", host, "sh", "-s"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().map_err(|e| format!("Could not run ssh: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(out.stdout) } else { Err(format!("{host}: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("failed"))) }
}
const LIST_SCRIPT: &str = r#"for f in "$HOME"/.local/share/tmux-logs/[0-9][0-9][0-9][0-9]/[0-9][0-9]/[0-9][0-9]/cast/*.cast; do
  [ -f "$f" ] || continue
  printf '%s\t%s\t' "$f" "$(wc -c < "$f" | tr -d ' ')"
  awk 'NR>1{n++;l=$0} END{sub(/^\[/,"",l);split(l,a,",");printf "%d\t%s\n",n,a[1]}' "$f"
done"#;
/// Lines of `<path> TAB <size> TAB <frames> TAB <duration>` -> casts (files whose names are not plain words are skipped).
pub fn parse_listing(host: &str, text: &str) -> Vec<CastInfo> {
    let mut casts: Vec<CastInfo> = text.lines().filter_map(|line| {
        let mut f = line.split('\t');
        let (path, size, frames, duration) = (f.next()?, f.next()?, f.next()?, f.next()?);
        let tail: Vec<&str> = path.rsplitn(6, '/').collect(); // name.cast, cast, DD, MM, YYYY, rest
        let (file, d, m, y) = (tail.first()?, tail.get(2)?, tail.get(3)?, tail.get(4)?);
        let name = file.strip_suffix(".cast")?;
        let date = format!("{y}{m}{d}");
        let id = format!("{host}.{date}.{name}");
        parse_id(&id)?;
        Some(CastInfo { id, host: host.into(), name: name.into(), date, size: size.parse().ok()?, frames: frames.parse().unwrap_or(0), duration: duration.parse().unwrap_or(0.0) })
    }).collect();
    casts.sort_by(|a, b| b.date.cmp(&a.date).then(b.name.cmp(&a.name)));
    casts.truncate(500);
    casts
}
fn word_ok(s: &str, min: usize, max: usize) -> bool { (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }

pub struct SshBackend { pub hosts: Vec<String>, pub cache: PathBuf }
impl SshBackend {
    pub fn from_env() -> Self {
        let hosts = std::env::var("REMOTE_SESSIONS_RECORDING_HOSTS").map(|v| v.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect()).unwrap_or_else(|_| DEFAULT_HOSTS.iter().map(|h| h.to_string()).collect());
        SshBackend { hosts, cache: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".cache/remote-sessions/recordings") }
    }
    fn host_of<'a>(&self, id: &'a str) -> Result<(&'a str, &'a str, &'a str)> {
        let parts = parse_id(id).ok_or("Not a recording id.")?;
        if !self.hosts.iter().any(|h| h == parts.0) { return Err("That host is not one of the recording hosts.".into()); }
        Ok(parts)
    }
    fn id_from_path(host: &str, path: &str) -> Result<String> {
        parse_listing(host, &format!("{}\t0\t0\t0", path.trim())).into_iter().next().map(|c| c.id).ok_or_else(|| "The recorder did not report a cast file.".into())
    }
}
impl Backend for SshBackend {
    fn list(&self) -> Listing {
        let mut out = Listing::default();
        for host in &self.hosts {
            match ssh_sh(host, LIST_SCRIPT) { Ok(text) => out.casts.extend(parse_listing(host, &String::from_utf8_lossy(&text))), Err(e) => out.errors.push(e) }
        }
        out.casts.sort_by(|a, b| b.date.cmp(&a.date).then(b.name.cmp(&a.name)));
        out
    }
    fn read(&self, id: &str, from: u64) -> Result<Vec<u8>> {
        let (host, date, name) = self.host_of(id)?;
        ssh_sh(host, &format!("tail -c +{} \"{}\"", from + 1, cast_path(date, name)))
    }
    fn start(&self, kind: &str, host: &str, session: &str) -> Result<String> {
        if !self.hosts.iter().any(|h| h == host) || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') { return Err("host must be one of the recording hosts.".into()); }
        let day = "$(TZ=America/Chicago date +%Y/%m/%d)";
        let script = match kind {
            "pane" | "vm" => {
                if !(1..=64).contains(&session.len()) || !session.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:%@=-".contains(&b)) { return Err("session must be a tmux target such as vm-1 or %5.".into()); }
                format!("PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; export PATH\nP=$(tmux display-message -p -t '{session}' '#{{pane_id}}') || {{ echo 'no such tmux pane' >&2; exit 3; }}\nTMUX_PANE=$P \"$HOME/.bin/tmux-record-cast.sh\" start >/dev/null 2>&1\nS=$(tmux display-message -p -t \"$P\" '#{{session_name}}')\ncat \"/tmp/tmux-cast-$S.file\"\n")
            }
            "cloud" | "rc" => {
                let (tool, prefix) = if kind == "cloud" { ("cloud-sessions", "cloud") } else { ("remote-sessions", "rc") };
                let ok = if kind == "cloud" { word_ok(session, 8, 100) } else { remote_sessions_core::transcript::uuid_ok(session) };
                if !ok { return Err(if kind == "cloud" { "session must be a session_... id." } else { "session must be the session's uuid." }.into()); }
                format!("PATH=$HOME/.cargo/bin:$HOME/.local/bin:/opt/homebrew/bin:$PATH; export PATH\nF=\"{CAST_ROOT}/{day}/cast/{prefix}_{session}.cast\"\n{tool} cast {session} --out \"$F\" >/dev/null || exit 4\necho \"$F\"\n")
            }
            _ => return Err("kind must be pane, vm, cloud or rc.".into()),
        };
        Self::id_from_path(host, &String::from_utf8_lossy(&ssh_sh(host, &script)?))
    }
    fn stop(&self, id: &str) -> Result<()> {
        let (host, date, name) = self.host_of(id)?;
        let script = format!("PATH=/opt/homebrew/bin:/usr/local/bin:$PATH; export PATH\nF=\"{}\"\nfor R in /tmp/tmux-cast-*.file; do\n  [ -f \"$R\" ] || continue\n  [ \"$(cat \"$R\")\" = \"$F\" ] || continue\n  S=${{R#/tmp/tmux-cast-}}; S=${{S%.file}}\n  P=$(tmux display-message -p -t \"=$S:\" '#{{pane_id}}') || exit 3\n  TMUX_PANE=$P \"$HOME/.bin/tmux-record-cast.sh\" stop >/dev/null 2>&1\n  exit 0\ndone\necho 'no active recording for that cast' >&2; exit 5\n", cast_path(date, name));
        ssh_sh(host, &script).map(|_| ())
    }
    fn media(&self, id: &str, ext: &str) -> std::result::Result<Vec<u8>, Mp4Error> {
        let (cast, target) = (self.cache.join(format!("{id}.cast")), self.cache.join(format!("{id}.{ext}")));
        let data = self.read(id, 0).map_err(Mp4Error::Failed)?;
        let fresh = std::fs::metadata(&cast).map(|m| m.len() == data.len() as u64).unwrap_or(false) && target.is_file();
        if !fresh {
            std::fs::create_dir_all(&self.cache).and_then(|_| std::fs::write(&cast, &data)).map_err(|e| Mp4Error::Failed(format!("Could not write the cache: {e}")))?;
            remote_sessions_core::cast::render_mp4(&cast, &self.cache.join(format!("{id}.mp4")))?;
        }
        std::fs::read(&target).map_err(|e| Mp4Error::Failed(format!("The renderer made no {ext}: {e}")))
    }
}

pub struct NoRecordings;
impl Backend for NoRecordings {
    fn list(&self) -> Listing { Listing::default() }
    fn read(&self, _: &str, _: u64) -> Result<Vec<u8>> { Err("Recordings are not available here.".into()) }
    fn start(&self, _: &str, _: &str, _: &str) -> Result<String> { Err("Recordings are not available here.".into()) }
    fn stop(&self, _: &str) -> Result<()> { Err("Recordings are not available here.".into()) }
    fn media(&self, _: &str, _: &str) -> std::result::Result<Vec<u8>, Mp4Error> { Err(Mp4Error::Missing("Recordings are not available here.".into())) }
}

/// The CLI/MCP verbs: `list`, `start --kind --host --session`, `stop <id>`.
pub fn command(backend: &dyn Backend, action: &str, positional: &[String], values: &HashMap<String, String>) -> Result<Value> {
    match action {
        "list" => { let l = backend.list(); Ok(json!({"ok": true, "recordings": l.casts.iter().map(|c| json!({"id": c.id, "host": c.host, "name": c.name, "date": c.date, "size": c.size, "frames": c.frames, "duration": c.duration})).collect::<Vec<_>>(), "errors": l.errors})) }
        "start" => {
            let get = |k: &str| values.get(k).map(String::as_str).ok_or(format!("--{k} is required."));
            Ok(json!({"ok": true, "id": backend.start(get("kind")?, get("host")?, get("session")?)?}))
        }
        "stop" => { let id = positional.first().ok_or("Give the recording id.")?; if parse_id(id).is_none() { return Err("Not a recording id.".into()); } backend.stop(id)?; Ok(json!({"ok": true, "id": id, "stopped": true})) }
        _ => Err("Use recordings list, start --kind <pane|vm|cloud|rc> --host <h> --session <s>, stop <id> or serve.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const ID: &str = "mac-b.20240102.vm-1_rec-vm_151805";

    #[derive(Default)]
    struct Fake { calls: Mutex<Vec<String>>, mp4: Option<Mp4Error> }
    impl Backend for Fake {
        fn list(&self) -> Listing { Listing { casts: vec![CastInfo { id: ID.into(), host: "mac-b".into(), name: "vm-1_rec-vm_151805".into(), date: "20240102".into(), size: 10, frames: 3, duration: 1.5 }], errors: vec!["mac-c: unreachable".into()] } }
        fn read(&self, id: &str, from: u64) -> Result<Vec<u8>> { if id == ID { Ok(b"{\"version\":2}\n[0.1,\"o\",\"x\"]\n"[from as usize..].to_vec()) } else { Err("no such cast".into()) } }
        fn start(&self, kind: &str, host: &str, session: &str) -> Result<String> { self.calls.lock().unwrap().push(format!("start {kind} {host} {session}")); Ok(ID.into()) }
        fn stop(&self, id: &str) -> Result<()> { self.calls.lock().unwrap().push(format!("stop {id}")); Ok(()) }
        fn media(&self, _: &str, ext: &str) -> std::result::Result<Vec<u8>, Mp4Error> { match &self.mp4 { Some(Mp4Error::Missing(m)) => Err(Mp4Error::Missing(m.clone())), Some(Mp4Error::Failed(m)) => Err(Mp4Error::Failed(m.clone())), None => Ok(format!("fake-{ext}").into_bytes()) } }
    }
    fn req(method: &str, path: &str, auth: bool, body: &str) -> Request {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        Request { method: method.into(), path: path.into(), query: query.into(), authorization: auth.then(|| format!("Bearer {TOKEN}")), body: body.as_bytes().to_vec() }
    }
    fn body(r: &Response) -> String { match &r.body { Body::Bytes(b) => String::from_utf8_lossy(b).into_owned(), Body::Live(id) => format!("live:{id}") } }

    #[test]
    fn every_route_but_the_player_files_needs_the_token() {
        let f = Fake::default();
        for (m, p) in [("GET", "/recordings"), ("POST", "/recordings"), ("GET", "/recordings/x.cast"), ("DELETE", &format!("/recordings/{ID}")), ("GET", &format!("/play/{ID}")), ("GET", &format!("/recordings/{ID}/live"))] {
            assert_eq!(handle(&f, TOKEN, &req(m, p, false, "")).status, 401, "{m} {p}");
        }
        let mut wrong = req("GET", "/recordings", true, "");
        wrong.authorization = Some("Bearer nope".into());
        assert_eq!(handle(&f, TOKEN, &wrong).status, 401);
        assert_eq!(handle(&f, TOKEN, &req("GET", &format!("/recordings?token={TOKEN}"), false, "")).status, 200, "the browser page passes the token in the query");
        let asset = handle(&f, TOKEN, &req("GET", "/play/assets/asciinema-player.min.js", false, ""));
        assert_eq!((asset.status, asset.content_type), (200, "text/javascript"));
        assert!(body(&handle(&f, TOKEN, &req("GET", "/play/assets/LICENSE", false, ""))).contains("Apache License"), "the player's license is served with it");
        assert_eq!(handle(&f, TOKEN, &req("GET", "/play/assets/../../etc/passwd", false, "")).status, 404);
    }
    #[test]
    fn list_start_stop_and_fetch() {
        let f = Fake::default();
        let list = serde_json::from_str::<Value>(&body(&handle(&f, TOKEN, &req("GET", "/recordings", true, "")))).unwrap();
        assert_eq!((list["recordings"][0]["id"].as_str(), list["recordings"][0]["frames"].as_u64(), list["errors"][0].as_str()), (Some(ID), Some(3), Some("mac-c: unreachable")));
        let made = handle(&f, TOKEN, &req("POST", "/recordings", true, r#"{"kind":"vm","host":"mac-b","session":"vm-1"}"#));
        assert_eq!((made.status, serde_json::from_str::<Value>(&body(&made)).unwrap()["id"].as_str().map(str::to_string)), (201, Some(ID.to_string())));
        assert_eq!(handle(&f, TOKEN, &req("POST", "/recordings", true, r#"{"kind":"disk","host":"h","session":"s"}"#)).status, 400);
        assert_eq!(handle(&f, TOKEN, &req("POST", "/recordings", true, "not json")).status, 400);
        assert_eq!(handle(&f, TOKEN, &req("POST", "/recordings", true, r#"{"kind":"pane","host":"","session":"s"}"#)).status, 400);
        assert_eq!(handle(&f, TOKEN, &req("DELETE", &format!("/recordings/{ID}"), true, "")).status, 200);
        assert_eq!(*f.calls.lock().unwrap(), [format!("start vm mac-b vm-1"), format!("stop {ID}")]);
        let cast = handle(&f, TOKEN, &req("GET", &format!("/recordings/{ID}.cast"), true, ""));
        assert_eq!((cast.status, cast.content_type), (200, "application/x-asciicast"));
        assert!(body(&cast).starts_with("{\"version\":2}"));
        assert_eq!(body(&handle(&f, TOKEN, &req("GET", &format!("/recordings/{ID}/live"), true, ""))), format!("live:{ID}"));
        assert_eq!(handle(&f, TOKEN, &req("GET", "/recordings/mac-b.20240102.nope.cast", true, "")).status, 404);
    }
    #[test]
    fn ids_cannot_name_a_path_outside_the_cast_folders() {
        let f = Fake::default();
        for bad in ["../../etc/passwd", "mac-b.20240102.../../x", "mac-b.2024010.name", "mac-b.20240102.a/b", "a b.20240102.n", "mac-b.20240102.", ".20240102.n"] {
            assert!(parse_id(bad).is_none(), "{bad}");
            assert_eq!(handle(&f, TOKEN, &req("GET", &format!("/recordings/{bad}.cast"), true, "")).status, 404, "{bad}");
            assert_eq!(handle(&f, TOKEN, &req("DELETE", &format!("/recordings/{bad}"), true, "")).status, 404, "{bad}");
        }
        assert_eq!(parse_id(ID), Some(("mac-b", "20240102", "vm-1_rec-vm_151805")));
        assert_eq!(cast_path("20240102", "n"), "$HOME/.local/share/tmux-logs/2024/01/02/cast/n.cast");
    }
    #[test]
    fn mp4_and_gif_are_501_when_the_renderer_is_missing_and_otherwise_served() {
        let missing = Fake { mp4: Some(Mp4Error::Missing("MP4 needs cast2mp4, agg and ffmpeg on this machine (missing: agg).".into())), ..Default::default() };
        let r = handle(&missing, TOKEN, &req("GET", &format!("/recordings/{ID}.mp4"), true, ""));
        assert_eq!(r.status, 501);
        assert!(body(&r).contains("agg"));
        assert_eq!(handle(&missing, TOKEN, &req("GET", &format!("/recordings/{ID}.gif"), true, "")).status, 501);
        let failed = Fake { mp4: Some(Mp4Error::Failed("cast2mp4 failed".into())), ..Default::default() };
        assert_eq!(handle(&failed, TOKEN, &req("GET", &format!("/recordings/{ID}.mp4"), true, "")).status, 502);
        let ok = handle(&Fake::default(), TOKEN, &req("GET", &format!("/recordings/{ID}.mp4"), true, ""));
        assert_eq!((ok.status, ok.content_type, body(&ok).as_str()), (200, "video/mp4", "fake-mp4"));
    }
    #[test]
    fn the_player_page_names_the_cast_and_never_echoes_the_token() {
        let f = Fake::default();
        let r = handle(&f, TOKEN, &req("GET", &format!("/play/{ID}?token={TOKEN}"), false, ""));
        let html = body(&r);
        assert_eq!((r.status, r.content_type), (200, "text/html; charset=utf-8"));
        assert!(html.contains(&format!("/recordings/{ID}.cast?token=")) && html.contains("asciinema-player.min.js"));
        assert!(!html.contains(TOKEN), "the page reads the token from its own address");
        assert_eq!(handle(&f, TOKEN, &req("GET", "/play/<script>", true, "")).status, 404);
    }
    #[test]
    fn the_token_file_is_generated_private_reused_and_never_loosened() {
        let dir = std::env::temp_dir().join(format!("rs-token-{}", std::process::id()));
        let path = dir.join("sub/recordings.token");
        let first = load_or_create_token(&path).unwrap();
        assert!(first.len() >= 32 && first.bytes().all(|b| b.is_ascii_hexdigit()));
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(load_or_create_token(&path).unwrap(), first, "an existing token is reused");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load_or_create_token(&path).unwrap_err().contains("chmod 600"));
        }
    }
    #[test]
    fn only_loopback_may_be_bound() {
        assert!(check_bind(DEFAULT_BIND).is_ok() && check_bind("[::1]:3790").is_ok());
        assert!(check_bind("0.0.0.0:3790").unwrap_err().contains("loopback"));
        assert!(check_bind("10.1.2.3:80").is_err() && check_bind("nonsense").is_err());
    }
    #[test]
    fn a_listing_becomes_ids_and_odd_file_names_are_skipped() {
        let text = "/Users/r/.local/share/tmux-logs/2024/01/02/cast/a_b-c.cast\t120\t5\t3.5\n/Users/r/.local/share/tmux-logs/2024/01/01/cast/older.cast\t9\t0\t\n/Users/r/.local/share/tmux-logs/2024/01/02/cast/has space.cast\t1\t1\t1\n/Users/r/.local/share/tmux-logs/2024/01/02/cast/dotted.name.cast\t1\t1\t1\ngarbage\n";
        let casts = parse_listing("mac-a", text);
        assert_eq!(casts.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["mac-a.20240102.a_b-c", "mac-a.20240101.older"]);
        assert_eq!((casts[0].size, casts[0].frames, casts[0].duration), (120, 5, 3.5));
    }
    #[test]
    fn the_real_backend_refuses_hosts_and_sessions_it_was_not_given() {
        let b = SshBackend { hosts: vec!["mac-b".into()], cache: std::env::temp_dir().join("rs-cache-none") };
        assert!(b.start("pane", "evil", "s").unwrap_err().contains("recording hosts"));
        assert!(b.start("pane", "mac-b", "x'; rm -rf ~; '").unwrap_err().contains("tmux target"));
        assert!(b.start("cloud", "mac-b", "bad id").is_err() && b.start("rc", "mac-b", "not-a-uuid").is_err() && b.start("tape", "mac-b", "s").is_err());
        assert!(b.stop("evil.20240102.n").unwrap_err().contains("recording hosts"));
        assert!(b.read("../x", 0).is_err());
    }
    #[test]
    fn commands_validate_before_calling_the_backend() {
        let f = Fake::default();
        let v: HashMap<String, String> = [("kind", "pane"), ("host", "mac-b"), ("session", "vm-1")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(command(&f, "start", &[], &v).unwrap()["id"], ID);
        assert!(command(&f, "start", &[], &HashMap::new()).is_err() && command(&f, "stop", &["../x".into()], &v).is_err() && command(&f, "stop", &[], &v).is_err());
        assert_eq!(command(&f, "stop", &[ID.into()], &v).unwrap()["stopped"], true);
        assert_eq!(command(&f, "list", &[], &v).unwrap()["recordings"][0]["id"], ID);
        assert!(command(&f, "serve", &[], &v).is_err());
    }
}
