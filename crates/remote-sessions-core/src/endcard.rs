//! The END CARD appended to an exported MP4 (not to the .cast: the recording is never modified).
//!
//! A 5 s card says which tool recorded it, shows the sha256 of the .cast (so the recording stays verifiable),
//! and carries an optional sponsor box and a call to action. The look comes from a JSON spec file
//! (`~/.config/endcard/endcard.json`, override: env `ENDCARD_CONFIG`), so restyling is edit, `endcard preview`, look: no rebuild.
//! See ENDCARD.md for the spec. Styling never fails an export: bad JSON is a warning plus the defaults, and a missing
//! ffmpeg or drawtext filter leaves the MP4 without a card (one warning).
//!
//! Safety: every line is written to a text file and read by drawtext (`textfile=`, `expansion=none`), so no text is ever
//! escaped into the filter string. The only recording-derived text on the card is the cast file name and its sha256.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The tool name shown by `{tool}`.
pub const TOOL: &str = "remote-sessions";
/// Advance of one monospace character as a fraction of the font size (Menlo is 0.602).
const CHAR_W: f64 = 0.62;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Background {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// A PNG (or any image ffmpeg reads), scaled and padded to the recording size. Wins over `color` when the file exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}
impl Default for Background {
    fn default() -> Self { Self { color: Some("#0b0f17".into()), image: None } }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Line {
    pub text: String,
    /// Font size as a fraction of the video height.
    pub size: f64,
    pub color: String,
    /// Vertical position of the top of the line, as a fraction of the video height.
    pub y: f64,
}
impl Default for Line {
    fn default() -> Self { Self { text: String::new(), size: 1.0 / 30.0, color: "#ffffff".into(), y: 0.5 } }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Sponsor {
    pub enabled: bool,
    pub text: String,
    pub color: String,
    pub box_color: String,
    /// Vertical CENTER of the box, as a fraction of the video height. The box is sized to the text, with padding.
    pub y: f64,
    pub size: f64,
}
impl Default for Sponsor {
    fn default() -> Self {
        Self { enabled: true, text: "Sponsored by example.com  -  one round-trip on the rail".into(), color: "#f2c94c".into(), box_color: "#f2c94c".into(), y: 0.66, size: 1.0 / 30.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Spec {
    pub enabled: bool,
    pub duration_s: f64,
    pub fade_s: f64,
    pub background: Background,
    /// A font file; None picks Menlo on macOS, else a DejaVu/Liberation mono, else ffmpeg's own choice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    pub lines: Vec<Line>,
    pub sponsor: Sponsor,
    pub cta: String,
}
impl Default for Spec {
    fn default() -> Self {
        let line = |text: &str, size: f64, color: &str, y: f64| Line { text: text.into(), size, color: color.into(), y };
        Self {
            enabled: true,
            duration_s: 5.0,
            fade_s: 0.5,
            background: Background::default(),
            font: None,
            lines: vec![
                line("Recorded with {tool}", 1.0 / 18.0, "#ffffff", 0.18),
                line("PROOF  sha256 of {cast_name}", 1.0 / 30.0, "#9fb3c8", 0.36),
                line("{sha256}", 1.0 / 30.0, "#7ee787", 0.44),
                line("{cta}", 1.0 / 30.0, "#ffffff", 0.84),
            ],
            sponsor: Sponsor::default(),
            cta: "Record yours: remote-sessions, free and MIT".into(),
        }
    }
}

/// Parse a spec. Bad JSON gives the defaults and a warning; unknown fields are ignored.
pub fn parse_spec(json: &str) -> (Spec, Option<String>) {
    match serde_json::from_str::<Spec>(json) {
        Ok(spec) => (spec, None),
        Err(e) => (Spec::default(), Some(format!("endcard: the spec is not valid ({e}); using the built-in card."))),
    }
}

pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ENDCARD_CONFIG").filter(|p| !p.is_empty()) { return Some(PathBuf::from(p)); }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/endcard/endcard.json"))
}

/// The spec from the config file; no file means the defaults, silently.
pub fn load() -> (Spec, Option<String>) {
    let Some(path) = config_path() else { return (Spec::default(), None) };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_spec(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Spec::default(), None),
        Err(e) => (Spec::default(), Some(format!("endcard: could not read {} ({e}); using the built-in card.", path.display()))),
    }
}

/// Write the default spec to the config path if it is absent. Never overwrites. Returns the path and whether it was written.
pub fn init() -> Result<(PathBuf, bool), String> {
    let path = config_path().ok_or("No HOME to put the endcard spec in; set ENDCARD_CONFIG.")?;
    if path.exists() { return Ok((path, false)); }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) { std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?; }
    let text = serde_json::to_string_pretty(&Spec::default()).map_err(|e| e.to_string())?;
    std::fs::write(&path, text + "\n").map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok((path, true))
}

/// Fill `{name}` placeholders in one pass (a value is never itself re-expanded). Unknown placeholders stay as written.
pub fn fill(text: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        match tail.find('}').and_then(|close| vars.iter().find(|(k, _)| tail[1..close] == **k).map(|(_, v)| (close, *v))) {
            Some((close, value)) => { out.push_str(value); rest = &tail[close + 1..]; }
            None => { out.push('{'); rest = &tail[1..]; }
        }
    }
    out.push_str(rest);
    out
}

/// A `#rrggbb` color as ffmpeg's `0xrrggbb`; anything else is replaced by the fallback (the value is spliced into a filter).
fn color(c: &str, fallback: &str) -> String {
    let hex = c.trim().trim_start_matches('#');
    if hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) { format!("0x{}", hex.to_ascii_lowercase()) } else { format!("0x{}", fallback.trim_start_matches('#')) }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text { pub file: String, pub text: String, pub px: u32, pub color: String, pub y: u32 }
#[derive(Debug, Clone, PartialEq)]
pub struct SponsorBox { pub text: Text, pub x: u32, pub y: u32, pub w: u32, pub h: u32, pub box_color: String }
#[derive(Debug, Clone, PartialEq)]
pub struct Plan { pub texts: Vec<Text>, pub sponsor: Option<SponsorBox> }

fn one_line(s: &str) -> String { s.replace(['\r', '\n'], " ") }

/// Lay the card out for a `w`x`h` frame: placeholders filled, font sizes shrunk until each line fits the frame.
pub fn plan(spec: &Spec, tool: &str, cast_name: &str, sha: &str, w: u32, h: u32) -> Plan {
    let base = [("tool", tool), ("cast_name", cast_name), ("sha256", sha)];
    let sponsor_text = if spec.sponsor.enabled { one_line(&fill(&spec.sponsor.text, &base)) } else { String::new() };
    let cta = one_line(&fill(&spec.cta, &base));
    let vars = [base[0], base[1], base[2], ("sponsor", sponsor_text.as_str()), ("cta", cta.as_str())];
    let (wf, hf) = (w as f64, h as f64);
    let texts = spec.lines.iter().enumerate().map(|(i, l)| {
        let text = one_line(&fill(&l.text, &vars));
        let mut px = (l.size.clamp(0.005, 0.5) * hf).round().max(8.0);
        let n = text.chars().count().max(1) as f64;
        if n * CHAR_W * px > wf * 0.92 { px = (wf * 0.92 / (n * CHAR_W)).floor().max(6.0); }
        Text { file: format!("l{i}.txt"), text, px: px as u32, color: color(&l.color, "#ffffff"), y: (l.y.clamp(0.0, 1.0) * hf).round() as u32 }
    }).filter(|t| !t.text.trim().is_empty()).collect();
    let sponsor = (spec.sponsor.enabled && !sponsor_text.trim().is_empty()).then(|| {
        let n = sponsor_text.chars().count().max(1) as f64;
        let mut px = (spec.sponsor.size.clamp(0.005, 0.5) * hf).round().max(8.0);
        // padding is 0.9 * the font size each side, so the text never touches the box edge
        if px * (n * CHAR_W + 1.8) > wf * 0.96 { px = (wf * 0.96 / (n * CHAR_W + 1.8)).floor().max(6.0); }
        let bw = (px * (n * CHAR_W + 1.8)).ceil().min(wf);
        let bh = (px * 2.6).ceil();
        let cy = spec.sponsor.y.clamp(0.0, 1.0) * hf;
        SponsorBox {
            text: Text { file: "sponsor.txt".into(), text: sponsor_text.clone(), px: px as u32, color: color(&spec.sponsor.color, "#f2c94c"), y: 0 },
            x: ((wf - bw) / 2.0).floor() as u32, y: (cy - bh / 2.0).max(0.0).round() as u32, w: bw as u32, h: bh as u32,
            box_color: color(&spec.sponsor.box_color, "#f2c94c"),
        }
    });
    Plan { texts, sponsor }
}

/// A font path that is safe to splice into a filter (no filter metacharacters) and exists.
fn safe_font(path: &str) -> Option<String> {
    (!path.chars().any(|c| ":'\\,;[]\n\r\"".contains(c)) && Path::new(path).is_file()).then(|| path.to_string())
}
pub fn default_font(spec: &Spec) -> Option<String> {
    if let Some(f) = spec.font.as_deref().and_then(safe_font) { return Some(f); }
    ["/System/Library/Fonts/Menlo.ttc", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono.ttf", "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf", "/Library/Fonts/Courier New.ttf"]
        .iter().find_map(|p| safe_font(p))
}

/// The card's filter chain (no labels): optional image scaling, the text lines, the sponsor box and text, the fade-in.
/// The text files are read from the working directory ffmpeg runs in.
pub fn chain(spec: &Spec, plan: &Plan, w: u32, h: u32, font: Option<&str>, image: bool) -> String {
    let font_arg = font.map(|f| format!("fontfile={f}:")).unwrap_or_default();
    let dt = |t: &Text, y: &str| format!("drawtext={font_arg}textfile={}:expansion=none:fontsize={}:fontcolor={}:x=(w-tw)/2:y={y}", t.file, t.px, t.color);
    let mut parts: Vec<String> = Vec::new();
    if image { parts.push(format!("scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:{},setsar=1", color(spec.background.color.as_deref().unwrap_or("#0b0f17"), "#0b0f17"))); }
    for t in &plan.texts { parts.push(dt(t, &t.y.to_string())); }
    if let Some(s) = &plan.sponsor {
        parts.push(format!("drawbox=x={}:y={}:w={}:h={}:color={}@0.9:t=4", s.x, s.y, s.w, s.h, s.box_color));
        parts.push(dt(&s.text, &format!("{}+({}-th)/2", s.y, s.h)));
    }
    if spec.fade_s > 0.0 { parts.push(format!("fade=t=in:st=0:d={}", spec.fade_s.min(spec.duration_s))); }
    parts.push("format=yuv420p,setsar=1".into());
    parts.join(",")
}

/// ffmpeg input arguments for the card background (a lavfi color source, or a looped image).
fn background_args(spec: &Spec, w: u32, h: u32, fps: u32) -> (Vec<String>, bool) {
    let d = spec.duration_s;
    if let Some(img) = spec.background.image.as_deref().filter(|p| Path::new(p).is_file()) {
        return (vec!["-loop".into(), "1".into(), "-framerate".into(), fps.to_string(), "-t".into(), d.to_string(), "-i".into(), img.into()], true);
    }
    let c = color(spec.background.color.as_deref().unwrap_or("#0b0f17"), "#0b0f17");
    (vec!["-f".into(), "lavfi".into(), "-i".into(), format!("color=c={c}:s={w}x{h}:r={fps}:d={d}")], false)
}

/// `Video: ... 1280x720 ..., 30 fps` from `ffmpeg -i` stderr.
pub fn parse_probe(text: &str) -> Option<(u32, u32, u32)> {
    let line = text.lines().find(|l| l.contains("Video:"))?;
    let (mut size, mut fps) = (None, None);
    for tok in line.split(", ") {
        let t = tok.trim();
        if let Some(n) = t.strip_suffix(" fps") { fps = n.trim().parse::<f64>().ok().map(|f| f.round() as u32); }
        let head = t.split([' ', '[']).next().unwrap_or("");
        if let Some((a, b)) = head.split_once('x') { if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) { if a > 0 && b > 0 { size = Some((a, b)); } } }
    }
    let (w, h) = size?;
    Some((w, h, fps.filter(|f| *f > 0).unwrap_or(30)))
}
/// `Duration: 00:00:07.00` from `ffmpeg -i` stderr, in seconds.
pub fn parse_duration(text: &str) -> Option<f64> {
    let rest = text.split("Duration: ").nth(1)?;
    let mut p = rest.split(',').next()?.trim().split(':');
    let (h, m, s) = (p.next()?.parse::<f64>().ok()?, p.next()?.parse::<f64>().ok()?, p.next()?.parse::<f64>().ok()?);
    Some(h * 3600.0 + m * 60.0 + s)
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join("ffmpeg")).collect::<Vec<_>>()).unwrap_or_default();
    on_path.into_iter().chain([home.join(".local/bin/ffmpeg"), home.join(".bin/ffmpeg"), "/opt/homebrew/bin/ffmpeg".into(), "/usr/local/bin/ffmpeg".into()]).find(|p| p.is_file())
}
fn has_drawtext(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg).args(["-hide_banner", "-filters"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains(" drawtext ")).unwrap_or(false)
}
fn probe(ffmpeg: &Path, file: &Path) -> String {
    Command::new(ffmpeg).args(["-hide_banner", "-i"]).arg(file).output().map(|o| String::from_utf8_lossy(&o.stderr).into_owned()).unwrap_or_default()
}
/// The sha256 of a file as 64 hex characters, via the system's shasum / sha256sum / openssl.
pub fn sha256_file(path: &Path) -> Option<String> {
    let tries: [(&str, &[&str]); 3] = [("shasum", &["-a", "256"]), ("sha256sum", &[]), ("openssl", &["dgst", "-sha256", "-r"])];
    tries.iter().find_map(|(cmd, args)| {
        let out = Command::new(cmd).args(*args).arg(path).output().ok().filter(|o| o.status.success())?;
        let hex: String = String::from_utf8_lossy(&out.stdout).trim().chars().take(64).collect();
        (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
    })
}

struct Work(PathBuf);
impl Work {
    fn new(plan: &Plan) -> Result<Work, String> {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("endcard-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make a work folder: {e}"))?;
        let w = Work(dir);
        for t in plan.texts.iter().chain(plan.sponsor.iter().map(|s| &s.text)) { std::fs::write(w.0.join(&t.file), &t.text).map_err(|e| format!("could not write a text file: {e}"))?; }
        Ok(w)
    }
}
impl Drop for Work { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

fn run_ffmpeg(ffmpeg: &Path, work: &Work, args: &[String]) -> Result<(), String> {
    let out = Command::new(ffmpeg).current_dir(&work.0).args(args).output().map_err(|e| format!("could not run ffmpeg: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("no message"))) }
}

#[derive(Debug, PartialEq)]
pub enum Outcome { Applied, Disabled, Skipped(String) }

/// Append the card to `mp4` (rewritten in place). The `cast` is only read, for its name and sha256. Never errors: a problem is `Skipped`
/// and the MP4 is left exactly as it was.
pub fn apply_with(spec: &Spec, tool: &str, cast: &Path, mp4: &Path) -> Outcome {
    if !spec.enabled || spec.duration_s <= 0.0 { return Outcome::Disabled; }
    let Some(ffmpeg) = find_ffmpeg() else { return Outcome::Skipped("ffmpeg was not found".into()) };
    if !has_drawtext(&ffmpeg) { return Outcome::Skipped("this ffmpeg has no drawtext filter".into()); }
    let Some((w, h, fps)) = parse_probe(&probe(&ffmpeg, mp4)) else { return Outcome::Skipped("could not read the MP4's size".into()) };
    let Some(sha) = sha256_file(cast) else { return Outcome::Skipped("could not compute the cast's sha256".into()) };
    let name = cast.file_name().and_then(|n| n.to_str()).unwrap_or("recording.cast");
    let p = plan(spec, tool, name, &sha, w, h);
    let work = match Work::new(&p) { Ok(w) => w, Err(e) => return Outcome::Skipped(e) };
    let font = default_font(spec);
    let (bg, image) = background_args(spec, w, h, fps);
    let tmp = mp4.with_extension("endcard-tmp.mp4");
    let abs = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string();
    let graph = format!("[1:v]{}[card];[0:v]format=yuv420p,setsar=1,fps={fps}[rec];[rec][card]concat=n=2:v=1:a=0[v]", chain(spec, &p, w, h, font.as_deref(), image));
    let mut args: Vec<String> = ["-v", "error", "-y", "-i"].map(String::from).to_vec();
    args.push(abs(mp4));
    args.extend(bg);
    args.extend(["-filter_complex".to_string(), graph]);
    args.extend(["-map", "[v]", "-c:v", "libx264", "-crf", "20", "-preset", "medium", "-pix_fmt", "yuv420p", "-movflags", "+faststart"].map(String::from));
    let tmp_abs = abs(tmp.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))) + "/" + &tmp.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    args.push(tmp_abs.clone());
    if let Err(e) = run_ffmpeg(&ffmpeg, &work, &args) { let _ = std::fs::remove_file(&tmp_abs); return Outcome::Skipped(e); }
    match std::fs::rename(&tmp_abs, mp4) {
        Ok(()) => Outcome::Applied,
        Err(e) => { let _ = std::fs::remove_file(&tmp_abs); Outcome::Skipped(format!("could not replace the MP4: {e}")) }
    }
}

/// Load the spec, apply the card, print one warning on a problem. `tool` is the name `{tool}` expands to.
pub fn apply(tool: &str, cast: &Path, mp4: &Path) -> Outcome {
    let (spec, warning) = load();
    if let Some(w) = warning { eprintln!("{w}"); }
    let outcome = apply_with(&spec, tool, cast, mp4);
    if let Outcome::Skipped(why) = &outcome { eprintln!("endcard: the MP4 was written without the end card ({why})."); }
    outcome
}

/// Render the card alone to a PNG (the frame after the fade) so a spec can be restyled by eye. With no cast, sample text is used.
pub fn preview(spec: &Spec, tool: &str, cast: Option<&Path>, size: (u32, u32), out: &Path) -> Result<(), String> {
    let ffmpeg = find_ffmpeg().ok_or("ffmpeg was not found.")?;
    if !has_drawtext(&ffmpeg) { return Err("this ffmpeg has no drawtext filter.".into()); }
    let (name, sha) = match cast {
        Some(c) => (c.file_name().and_then(|n| n.to_str()).unwrap_or("recording.cast").to_string(), sha256_file(c).ok_or("could not compute the cast's sha256.")?),
        None => ("example.cast".to_string(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string()),
    };
    let (w, h) = (size.0 & !1, size.1 & !1);
    let p = plan(spec, tool, &name, &sha, w, h);
    let work = Work::new(&p)?;
    let (bg, image) = background_args(spec, w, h, 30);
    let at = (spec.fade_s + 0.05).min((spec.duration_s - 0.1).max(0.0));
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    let out_abs = std::fs::canonicalize(out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))).map_err(|e| e.to_string())?.join(out.file_name().ok_or("--out needs a file name.")?);
    let mut args: Vec<String> = ["-v", "error", "-y"].map(String::from).to_vec();
    args.extend(bg);
    args.extend(["-filter_complex".to_string(), format!("[0:v]{}[c]", chain(spec, &p, w, h, default_font(spec).as_deref(), image))]);
    args.extend(["-map".to_string(), "[c]".to_string(), "-ss".to_string(), format!("{at}"), "-frames:v".to_string(), "1".to_string(), out_abs.display().to_string()]);
    run_ffmpeg(&ffmpeg, &work, &args)
}

pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('x')?;
    let (a, b) = (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?);
    (a >= 64 && b >= 64 && a <= 8192 && b <= 8192).then_some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_reproduce_the_poc_card() {
        let s = Spec::default();
        assert!(s.enabled && s.sponsor.enabled);
        assert_eq!((s.duration_s, s.fade_s, s.background.color.as_deref()), (5.0, 0.5, Some("#0b0f17")));
        assert_eq!(s.lines.len(), 4);
        assert_eq!(s.lines[0].text, "Recorded with {tool}");
        assert!(s.sponsor.text.starts_with("Sponsored by example.com"));
        assert_eq!(parse_spec("{}").0, s, "an empty object is the defaults");
    }
    #[test]
    fn placeholders_fill_in_one_pass_and_unknowns_stay() {
        let v = [("tool", "t"), ("cast_name", "a.cast"), ("sha256", "abc")];
        assert_eq!(fill("{tool}/{cast_name} {sha256} {nope} {", &v), "t/a.cast abc {nope} {");
        assert_eq!(fill("{cast_name}", &[("cast_name", "{sha256}"), ("sha256", "x")]), "{sha256}", "a value is not expanded again");
        let p = plan(&Spec::default(), "remote-sessions", "r.cast", "f".repeat(64).as_str(), 1280, 720);
        assert_eq!(p.texts[0].text, "Recorded with remote-sessions");
        assert_eq!(p.texts[1].text, "PROOF  sha256 of r.cast");
        assert_eq!(p.texts[3].text, "Record yours: remote-sessions, free and MIT");
        assert!(p.sponsor.is_some());
    }
    #[test]
    fn a_sponsor_placeholder_in_a_line_expands_to_the_sponsor_text() {
        let mut s = Spec::default();
        s.lines = vec![Line { text: "x {sponsor}".into(), ..Line::default() }];
        s.sponsor.text = "S for {tool}".into();
        assert_eq!(plan(&s, "tt", "c", "h", 800, 600).texts[0].text, "x S for tt");
        s.sponsor.enabled = false;
        assert!(plan(&s, "tt", "c", "h", 800, 600).sponsor.is_none());
    }
    #[test]
    fn bad_json_falls_back_to_defaults_with_a_warning_and_unknown_fields_are_ignored() {
        let (spec, warn) = parse_spec("{not json");
        assert_eq!(spec, Spec::default());
        assert!(warn.unwrap().contains("not valid"));
        let (spec, warn) = parse_spec(r##"{"duration_s": 3, "future_field": [1], "background": {"color": "#112233"}}"##);
        assert!(warn.is_none());
        assert_eq!((spec.duration_s, spec.background.color.as_deref()), (3.0, Some("#112233")));
        assert_eq!(parse_spec(r#"{"duration_s": "long"}"#).0, Spec::default(), "a wrong type is a bad spec, not a failed export");
    }
    #[test]
    fn disabled_spec_leaves_the_mp4_alone() {
        let (spec, _) = parse_spec(r#"{"enabled": false}"#);
        assert_eq!(apply_with(&spec, "t", Path::new("/no/such.cast"), Path::new("/no/such.mp4")), Outcome::Disabled);
    }
    #[test]
    fn a_missing_mp4_is_a_skip_not_a_failure() {
        let o = apply_with(&Spec::default(), "t", Path::new("/no/such.cast"), Path::new("/no/such.mp4"));
        assert!(matches!(o, Outcome::Skipped(_) | Outcome::Disabled), "{o:?}");
    }
    #[test]
    fn the_filter_uses_textfiles_and_pads_the_sponsor_box() {
        let spec = Spec::default();
        let p = plan(&spec, "remote-sessions", "r.cast", &"a".repeat(64), 1280, 720);
        let f = chain(&spec, &p, 1280, 720, Some("/System/Library/Fonts/Menlo.ttc"), false);
        assert!(f.contains("drawtext=fontfile=/System/Library/Fonts/Menlo.ttc:textfile=l0.txt:expansion=none:fontsize=40:fontcolor=0xffffff:x=(w-tw)/2:y=130"), "{f}");
        assert!(f.contains("drawbox=") && f.contains("textfile=sponsor.txt") && f.contains("fade=t=in:st=0:d=0.5"), "{f}");
        assert!(f.ends_with("format=yuv420p,setsar=1"));
        assert!(!f.contains("Recorded") && !f.contains("aaaa"), "text must only travel in text files");
        let s = p.sponsor.unwrap();
        let text_w = (s.text.text.chars().count() as f64 * CHAR_W * s.text.px as f64) as u32;
        assert!(s.w >= text_w + 2 * s.text.px / 2 && s.w <= 1280, "box {} must out-size the text {text_w} by padding", s.w);
        assert!(s.h > s.text.px * 2);
    }
    #[test]
    fn long_text_shrinks_to_fit_a_small_frame() {
        let p = plan(&Spec::default(), "t", "c.cast", &"a".repeat(64), 320, 240);
        let sha = &p.texts[2];
        assert!(sha.px as f64 * CHAR_W * 64.0 <= 320.0 * 0.92 + 1.0, "{}", sha.px);
        let sp = p.sponsor.unwrap();
        assert!(sp.w <= 320 && sp.x + sp.w <= 320);
    }
    #[test]
    fn unsafe_colors_and_fonts_are_replaced() {
        assert_eq!(color("#AbCdEf", "#000000"), "0xabcdef");
        assert_eq!(color("red:x=1", "#7ee787"), "0x7ee787");
        assert_eq!(safe_font("/x/a:b.ttf"), None);
        let (s, _) = parse_spec(r##"{"lines":[{"text":"hi","color":"1;drawtext=bad"}]}"##);
        assert!(!chain(&s, &plan(&s, "t", "c", "h", 640, 360), 640, 360, None, false).contains("bad"));
    }
    #[test]
    fn image_background_adds_scale_and_pad() {
        let spec = Spec::default();
        let f = chain(&spec, &plan(&spec, "t", "c", "h", 640, 360), 640, 360, None, true);
        assert!(f.starts_with("scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2:0x0b0f17"), "{f}");
    }
    #[test]
    fn probe_and_duration_parsers() {
        let t = "  Duration: 00:01:02.50, start: 0.0\n  Stream #0:0: Video: h264 (High), yuv420p(progressive), 1280x720 [SAR 1:1 DAR 16:9], 30 fps, 30 tbr";
        assert_eq!(parse_probe(t), Some((1280, 720, 30)));
        assert_eq!(parse_duration(t), Some(62.5));
        assert_eq!(parse_probe("nothing"), None);
        assert_eq!(parse_size("640x360"), Some((640, 360)));
        assert_eq!(parse_size("10x10"), None);
    }
}
