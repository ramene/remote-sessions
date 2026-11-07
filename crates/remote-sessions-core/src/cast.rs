//! A session's events as an asciicast v2 recording (https://docs.asciinema.org/manual/asciicast/v2/), so a cloud
//! or Remote Control session can be replayed with `asciinema play` like a terminal session.
//!
//! The times are the events' own timestamps; a gap longer than `idle_max` seconds is shortened to `idle_max`.
//! Event text is already stripped of control characters by [`crate::events`], so it can only carry the colors added here.
use crate::events::Event;
use serde_json::json;

pub const DEFAULT_IDLE_MAX: f64 = 2.0;
const RESET: &str = "\x1b[0m";

/// The color of one line of an event, by what the line says.
fn style(kind: &str, line: &str) -> &'static str {
    if line.starts_with("· result:") { return if line.contains("error") { "\x1b[1;31m" } else { "\x1b[1;32m" }; }
    if line.starts_with('▸') { return "\x1b[33m"; }
    if line.starts_with("◂ error") { return "\x1b[31m"; }
    if line.starts_with('◂') || line.starts_with('·') { return "\x1b[90m"; }
    match kind { "user" => "\x1b[1;36m", "result" => "\x1b[32m", _ => "" }
}

/// The terminal text of one event: colored lines ending in CRLF, a blank line after a prompt or a result.
fn frame_text(event: &Event) -> String {
    let mut out = String::new();
    for (i, line) in event.text.lines().enumerate() {
        let prefix = if event.kind == "user" && i == 0 { "\x1b[1;36m❯ \x1b[0m" } else { "" };
        let color = style(&event.kind, line);
        out.push_str(&format!("{prefix}{color}{line}{}\r\n", if color.is_empty() { "" } else { RESET }));
    }
    if matches!(event.kind.as_str(), "user" | "result") { out.push_str("\r\n"); }
    out
}

/// The cast as text: one header line, then one `[seconds, "o", text]` line per event. Times never decrease.
pub fn render(events: &[Event], title: &str, idle_max: f64) -> String {
    let idle_max = if idle_max.is_finite() && idle_max >= 0.0 { idle_max } else { DEFAULT_IDLE_MAX };
    let start = events.iter().find_map(|e| e.at).map_or(0, |ms| ms.div_euclid(1000));
    let mut lines = vec![json!({"version": 2, "width": 120, "height": 40, "timestamp": start, "title": title, "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"}}).to_string()];
    let (mut clock, mut last): (f64, Option<i64>) = (0.0, None);
    for event in events {
        if let Some(at) = event.at {
            if let Some(prev) = last { clock += (((at - prev) as f64) / 1000.0).clamp(0.0, idle_max); }
            last = Some(at);
        }
        let text = frame_text(event);
        if text.is_empty() { continue; }
        lines.push(json!([(clock * 1e6).round() / 1e6, "o", text]).to_string());
    }
    lines.join("\n") + "\n"
}


/// Why an MP4 could not be made: a tool is missing on this machine (HTTP 501), or the render itself failed.
#[derive(Debug, PartialEq)]
pub enum Mp4Error { Missing(String), Failed(String) }
impl std::fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { match self { Self::Missing(m) | Self::Failed(m) => f.write_str(m) } }
}
fn find_tool(name: &str, fallbacks: &[&str], dirs: &[std::path::PathBuf]) -> Option<std::path::PathBuf> {
    dirs.iter().map(|d| d.join(name)).chain(fallbacks.iter().map(std::path::PathBuf::from)).find(|p| p.is_file())
}
/// The `cast2mp4` script (agg draws the frames, ffmpeg encodes) plus agg and ffmpeg, searched in `path` then the usual places.
pub fn mp4_tools(path: &[std::path::PathBuf], home: &std::path::Path) -> Result<std::path::PathBuf, Mp4Error> {
    let (bin, local) = (home.join(".bin"), home.join(".local/bin"));
    let script = find_tool("cast2mp4", &[bin.join("cast2mp4").to_str().unwrap_or("")], path);
    let agg = find_tool("agg", &["/opt/homebrew/bin/agg", "/usr/local/bin/agg"], path);
    let ffmpeg = find_tool("ffmpeg", &[local.join("ffmpeg").to_str().unwrap_or(""), bin.join("ffmpeg").to_str().unwrap_or("")], path);
    match (script, agg, ffmpeg) {
        (Some(script), Some(_), Some(_)) => Ok(script),
        (script, agg, ffmpeg) => Err(Mp4Error::Missing(format!("MP4 needs cast2mp4, agg and ffmpeg on this machine (missing: {}). Run it where they exist (mac-b), or play the .cast with asciinema.", [("cast2mp4", script.is_none()), ("agg", agg.is_none()), ("ffmpeg", ffmpeg.is_none())].iter().filter(|(_, m)| *m).map(|(n, _)| *n).collect::<Vec<_>>().join(", ")))),
    }
}
/// Render `cast` to `mp4` with cast2mp4 (agg --idle-time-limit 1.5 --last-frame-duration 3 --font-size 18 --theme monokai, then ffmpeg
/// libx264 yuv420p faststart). cast2mp4 names its output after the cast, next to it; it is then renamed to `mp4` when that differs.
pub fn render_mp4(cast: &std::path::Path, mp4: &std::path::Path) -> Result<(), Mp4Error> { render_mp4_opts(cast, mp4, true) }
/// [`render_mp4`], with the end card (see [`crate::endcard`]) appended when `card` is true. The .cast is never modified.
pub fn render_mp4_opts(cast: &std::path::Path, mp4: &std::path::Path, card: bool) -> Result<(), Mp4Error> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let path: Vec<std::path::PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let script = mp4_tools(&path, &home)?;
    let dir = cast.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    let out = std::process::Command::new(&script).env("ENDCARD_SKIP", "1").arg(cast).arg(dir).output().map_err(|e| Mp4Error::Failed(format!("Could not run cast2mp4: {e}")))?;
    if !out.status.success() { return Err(Mp4Error::Failed(format!("cast2mp4 failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("no message")))); }
    let made = dir.join(format!("{}.mp4", cast.file_stem().and_then(|s| s.to_str()).unwrap_or("cast")));
    if made != mp4 {
        if let Some(parent) = mp4.parent().filter(|d| !d.as_os_str().is_empty()) { std::fs::create_dir_all(parent).map_err(|e| Mp4Error::Failed(e.to_string()))?; }
        std::fs::rename(&made, mp4).or_else(|_| std::fs::copy(&made, mp4).map(|_| ())).map_err(|e| Mp4Error::Failed(format!("Could not place the mp4: {e}")))?;
    }
    if card { crate::endcard::apply(crate::endcard::TOOL, cast, mp4); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev(kind: &str, text: &str, at: Option<i64>) -> Event { Event { id: kind.into(), kind: kind.into(), user: kind == "user", text: text.into(), at } }

    #[test]
    fn header_is_asciicast_v2_and_times_are_monotonic_with_idle_gaps_capped() {
        let events = [ev("user", "hi", Some(1_000)), ev("assistant", "a", Some(1_500)), ev("tool_use", "▸ Bash: {}", Some(901_500)), ev("result", "· result: success", Some(901_000))];
        let cast = render(&events, "t", 2.0);
        let lines: Vec<serde_json::Value> = cast.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!((lines[0]["version"].as_i64(), lines[0]["width"].as_i64(), lines[0]["timestamp"].as_i64()), (Some(2), Some(120), Some(1)));
        let times: Vec<f64> = lines[1..].iter().map(|l| l[0].as_f64().unwrap()).collect();
        assert_eq!(times, [0.0, 0.5, 2.5, 2.5], "a long gap is capped and a backwards timestamp never rewinds");
        assert!(lines[1..].iter().all(|l| l[1] == "o"));
    }
    #[test]
    fn frames_are_colored_crlf_text_and_events_without_time_keep_the_clock() {
        let cast = render(&[ev("user", "do it", None), ev("assistant", "ok\nline2", None), ev("tool_result", "◂ error: boom", None), ev("result", "· result: success · 1 turns", None)], "t", 2.0);
        let frames: Vec<String> = cast.lines().skip(1).map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()[2].as_str().unwrap().to_string()).collect();
        assert_eq!(frames[0], "\x1b[1;36m❯ \x1b[0m\x1b[1;36mdo it\x1b[0m\r\n\r\n");
        assert_eq!(frames[1], "ok\r\nline2\r\n");
        assert!(frames[2].starts_with("\x1b[31m◂ error") && frames[3].starts_with("\x1b[1;32m· result"));
    }
    #[test]
    fn a_bad_idle_max_falls_back_to_the_default() {
        let events = [ev("user", "a", Some(0)), ev("user", "b", Some(60_000))];
        let t = |c: &str| serde_json::from_str::<serde_json::Value>(c.lines().nth(2).unwrap()).unwrap()[0].as_f64().unwrap();
        assert_eq!(t(&render(&events, "t", f64::NAN)), 2.0);
        assert_eq!(t(&render(&events, "t", 0.0)), 0.0);
    }
    #[test]
    fn mp4_without_the_tools_says_which_are_missing_and_is_a_501_case() {
        let nowhere = std::env::temp_dir().join("cast-no-such-home");
        let err = mp4_tools(&[], &nowhere).unwrap_err();
        let Mp4Error::Missing(message) = err else { panic!("expected Missing") };
        assert!(message.contains("cast2mp4") && message.contains("mac-b"), "{message}");
    }
}
