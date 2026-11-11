//! The optional logo in the drop-in header: an SVG file shown through the terminal's own inline-image support
//! (iTerm2's `OSC 1337 File=` or the kitty graphics protocol), also inside tmux (DCS passthrough).
//!
//! * The SVG is untrusted input: at most [`MAX_SVG_BYTES`], parsed by `usvg` with no text, no raster images and no system
//!   fonts (the crate features are off), and an image resolver that refuses every `href` (no file, no network).
//! * It is drawn into a transparent PNG exactly [`LOGO_COLS`] x [`LOGO_ROWS`] cells large and the PNG is cached per
//!   (path, mtime, size, pixel size), so a redraw never renders it again.
//! * No logo file, or a terminal without a supported protocol, means `None`: the caller draws the text mark as before.
use crate::text::b64_encode;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// An SVG larger than this is refused (1 MiB).
pub const MAX_SVG_BYTES: u64 = 1024 * 1024;
/// The image covers this many terminal cells, at the header's left.
pub const LOGO_COLS: u16 = 6;
pub const LOGO_ROWS: u16 = 3;
/// Assumed cell size in pixels when the terminal does not report one.
pub const DEFAULT_CELL_PX: (u32, u32) = (8, 16);
const KITTY_CHUNK: usize = 4096;
const ESC: char = '\x1b';

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Iterm2,
    Kitty,
}

/// Which inline-image protocol this terminal speaks, from its environment (None: no image is drawn).
/// iTerm2 is recognised by `TERM_PROGRAM=iTerm.app` or `LC_TERMINAL=iTerm2` (the latter survives tmux and ssh);
/// kitty by `TERM=xterm-kitty`, or by `KITTY_WINDOW_ID` (kitty sets it, and it survives tmux).
pub fn protocol_from_env(env: &dyn Fn(&str) -> Option<String>) -> Option<Protocol> {
    let is = |k: &str, v: &str| env(k).is_some_and(|x| x.trim() == v);
    if is("TERM_PROGRAM", "iTerm.app") || is("LC_TERMINAL", "iTerm2") {
        Some(Protocol::Iterm2)
    } else if is("TERM", "xterm-kitty") || env("KITTY_WINDOW_ID").is_some_and(|x| !x.trim().is_empty()) {
        Some(Protocol::Kitty)
    } else {
        None
    }
}

/// Inside tmux (`$TMUX` set and not empty).
pub fn in_tmux(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("TMUX").is_some_and(|v| !v.trim().is_empty())
}

/// Wrap a terminal sequence in tmux's DCS passthrough: `ESC P tmux ; <sequence with every ESC doubled> ESC \`.
/// tmux needs `set -g allow-passthrough on` to forward it.
pub fn tmux_wrap(seq: &str) -> String {
    let doubled: String = seq.chars().flat_map(|c| if c == ESC { vec![ESC, ESC] } else { vec![c] }).collect();
    format!("{ESC}Ptmux;{doubled}{ESC}\\")
}

/// Where the logo file is: `REMOTE_SESSIONS_LOGO_SVG`, else `~/.config/remote-sessions/logo.svg`.
pub fn logo_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(p) = env("REMOTE_SESSIONS_LOGO_SVG").map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    Some(PathBuf::from(env("HOME")?).join(".config/remote-sessions/logo.svg"))
}

/// The SVG as a PNG of exactly `px` pixels (transparent background, the drawing fitted and centred).
pub fn render_png(svg: &[u8], px: (u32, u32)) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};
    if svg.len() as u64 > MAX_SVG_BYTES {
        return Err("the logo SVG is larger than 1 MiB".into());
    }
    let mut opt = usvg::Options::default();
    opt.resources_dir = None;
    // No `href` is followed: not a file, not a URL, not even an embedded data URL.
    opt.image_href_resolver = usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) };
    let tree = usvg::Tree::from_data(svg, &opt).map_err(|e| format!("the logo is not a usable SVG: {e}"))?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err("the logo SVG has no size".into());
    }
    let (pw, ph) = (px.0.clamp(8, 1024), px.1.clamp(8, 1024));
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or("could not allocate the logo image")?;
    let scale = (pw as f32 / w).min(ph as f32 / h);
    let (tx, ty) = ((pw as f32 - w * scale) / 2.0, (ph as f32 - h * scale) / 2.0);
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| format!("could not encode the logo: {e}"))
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    mtime: Option<SystemTime>,
    len: u64,
    px: (u32, u32),
    png: Arc<Vec<u8>>,
}

/// One cached rendering and how many times the rasterizer ran.
pub struct Cache {
    entry: Mutex<Option<Entry>>,
    pub renders: AtomicUsize,
}

impl Cache {
    pub const fn new() -> Cache {
        Cache { entry: Mutex::new(None), renders: AtomicUsize::new(0) }
    }
    /// The PNG for this file at this pixel size. Rendered once per (path, mtime, size, pixel size); a changed file or size renders again.
    pub fn png(&self, path: &Path, px: (u32, u32)) -> Result<Arc<Vec<u8>>, String> {
        let meta = std::fs::metadata(path).map_err(|_| "no logo file".to_string())?;
        if !meta.is_file() {
            return Err("the logo is not a regular file".into());
        }
        if meta.len() > MAX_SVG_BYTES {
            return Err("the logo SVG is larger than 1 MiB".into());
        }
        let (mtime, len) = (meta.modified().ok(), meta.len());
        if let Some(e) = self.entry.lock().ok().and_then(|g| g.clone()) {
            if e.path == path && e.mtime == mtime && e.len == len && e.px == px {
                return Ok(e.png);
            }
        }
        let mut data = Vec::new();
        std::fs::File::open(path).and_then(|f| f.take(MAX_SVG_BYTES + 1).read_to_end(&mut data)).map_err(|e| format!("could not read the logo: {e}"))?;
        let png = Arc::new(render_png(&data, px)?);
        self.renders.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.entry.lock() {
            *g = Some(Entry { path: path.to_path_buf(), mtime, len, px, png: png.clone() });
        }
        Ok(png)
    }
}

static CACHE: Cache = Cache::new();

/// The terminal's cell size in pixels (the pixel fields of TIOCGWINSZ), when it reports one.
pub fn terminal_cell_px() -> Option<(u32, u32)> {
    let ws = crossterm::terminal::window_size().ok()?;
    (ws.columns > 0 && ws.rows > 0 && ws.width > 0 && ws.height > 0).then(|| (ws.width as u32 / ws.columns as u32, ws.height as u32 / ws.rows as u32))
}

/// iTerm2: `ESC ] 1337 ; File=inline=1;width=<cols>;height=<rows>;preserveAspectRatio=1 : <base64 PNG> BEL`.
pub fn iterm2_sequence(png: &[u8], cols: u16, rows: u16) -> String {
    format!("{ESC}]1337;File=inline=1;width={cols};height={rows};preserveAspectRatio=1:{}\x07", b64_encode(png))
}

/// kitty graphics protocol, PNG payload (`f=100`) transmitted and displayed at the cursor (`a=T`) over `cols` x `rows`
/// cells without moving the cursor (`C=1`), in chunks of at most 4096 base64 characters. Returns each APC separately.
pub fn kitty_sequences(png: &[u8], cols: u16, rows: u16) -> Vec<String> {
    let b64 = b64_encode(png);
    let chunks: Vec<&str> = b64.as_bytes().chunks(KITTY_CHUNK).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    let last = chunks.len().saturating_sub(1);
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let more = if i < last { 1 } else { 0 };
            if i == 0 {
                format!("{ESC}_Ga=T,f=100,c={cols},r={rows},C=1,q=2,m={more};{c}{ESC}\\")
            } else {
                format!("{ESC}_Gm={more};{c}{ESC}\\")
            }
        })
        .collect()
}

/// kitty: delete every image placed on the screen (before a re-send after a clear or a resize).
pub fn kitty_delete_all() -> String {
    format!("{ESC}_Ga=d,d=A,q=2{ESC}\\")
}

/// A logo ready to draw: the exact bytes to write to the terminal to show it.
#[derive(Clone, Debug)]
pub struct Logo {
    pub protocol: Protocol,
    pub cols: u16,
    pub rows: u16,
    /// What to write at the header's left (already tmux-wrapped inside tmux). kitty images are preceded by a delete.
    pub sequence: String,
}

impl Logo {
    /// The logo for this environment, or None (no file, no supported terminal, unusable SVG): the text mark is drawn.
    /// `cell_px` is the terminal's cell size in pixels, when it reported one.
    pub fn load(env: &dyn Fn(&str) -> Option<String>, cell_px: Option<(u32, u32)>) -> Option<Logo> {
        Logo::load_with(&CACHE, env, cell_px)
    }

    pub fn load_with(cache: &Cache, env: &dyn Fn(&str) -> Option<String>, cell_px: Option<(u32, u32)>) -> Option<Logo> {
        let protocol = protocol_from_env(env)?;
        let path = logo_path(env)?;
        let (cw, ch) = cell_px.filter(|(w, h)| *w > 0 && *h > 0).unwrap_or(DEFAULT_CELL_PX);
        let px = (LOGO_COLS as u32 * cw, LOGO_ROWS as u32 * ch);
        let png = cache.png(&path, px).ok()?;
        let tmux = in_tmux(env);
        let wrap = |s: String| if tmux { tmux_wrap(&s) } else { s };
        let sequence = match protocol {
            Protocol::Iterm2 => wrap(iterm2_sequence(&png, LOGO_COLS, LOGO_ROWS)),
            Protocol::Kitty => std::iter::once(kitty_delete_all()).chain(kitty_sequences(&png, LOGO_COLS, LOGO_ROWS)).map(wrap).collect::<String>(),
        };
        Some(Logo { protocol, cols: LOGO_COLS, rows: LOGO_ROWS, sequence })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" rx="12" fill="#25a7a0"/><circle cx="22" cy="30" r="7" fill="#f5a623"/><circle cx="42" cy="30" r="7" fill="#f5a623"/></svg>"##;

    fn env_of(pairs: Vec<(&'static str, &'static str)>) -> impl Fn(&str) -> Option<String> {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }
    /// A scratch directory next to the test binary (never /tmp).
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::current_exe().unwrap().parent().unwrap().join(format!("logo-{}-{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }
    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
    }
    fn png_size(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()))
    }

    #[test]
    fn the_protocol_comes_from_the_environment() {
        assert_eq!(protocol_from_env(&env_of(vec![("TERM_PROGRAM", "iTerm.app")])), Some(Protocol::Iterm2));
        assert_eq!(protocol_from_env(&env_of(vec![("LC_TERMINAL", "iTerm2")])), Some(Protocol::Iterm2));
        assert_eq!(protocol_from_env(&env_of(vec![("TERM", "xterm-kitty")])), Some(Protocol::Kitty));
        assert_eq!(protocol_from_env(&env_of(vec![("KITTY_WINDOW_ID", "3")])), Some(Protocol::Kitty));
        assert_eq!(protocol_from_env(&env_of(vec![("TERM_PROGRAM", "tmux"), ("LC_TERMINAL", "iTerm2"), ("TERM", "screen-256color")])), Some(Protocol::Iterm2), "tmux overwrites TERM_PROGRAM; LC_TERMINAL still says iTerm2");
        assert_eq!(protocol_from_env(&env_of(vec![("TERM_PROGRAM", "iTerm.app"), ("TERM", "xterm-kitty")])), Some(Protocol::Iterm2), "iTerm2 first");
        assert_eq!(protocol_from_env(&env_of(vec![("TERM", "xterm-256color"), ("TERM_PROGRAM", "Apple_Terminal")])), None);
        assert_eq!(protocol_from_env(&env_of(vec![("KITTY_WINDOW_ID", "")])), None);
        assert_eq!(protocol_from_env(&env_of(vec![])), None);
        assert!(in_tmux(&env_of(vec![("TMUX", "/private/tmp/tmux-501/default,123,0")])) && !in_tmux(&env_of(vec![("TMUX", "")])) && !in_tmux(&env_of(vec![])));
    }

    #[test]
    fn tmux_wrapping_doubles_every_escape_and_frames_the_sequence() {
        let seq = "\x1b]1337;File=inline=1:AAAA\x07";
        let w = tmux_wrap(seq);
        assert_eq!(w, "\x1bPtmux;\x1b\x1b]1337;File=inline=1:AAAA\x07\x1b\\");
        assert_eq!(w.matches('\x1b').count(), 2 * seq.matches('\x1b').count() + 2);
        let kitty = format!("{}{}", kitty_delete_all(), kitty_sequences(b"x", 6, 3)[0]);
        let wk = tmux_wrap(&kitty);
        assert_eq!(wk.matches('\x1b').count(), 2 * kitty.matches('\x1b').count() + 2);
        // undoing the framing and the doubling gives the original back
        let inner = wk.strip_prefix("\x1bPtmux;").unwrap().strip_suffix("\x1b\\").unwrap().replace("\x1b\x1b", "\x1b");
        assert_eq!(inner, kitty);
    }

    #[test]
    fn the_svg_becomes_a_png_of_exactly_the_requested_size_and_hostile_references_draw_nothing() {
        let png = render_png(SVG.as_bytes(), (48, 48)).unwrap();
        assert_eq!(png_size(&png), (48, 48));
        let wide = render_png(SVG.as_bytes(), (96, 48)).unwrap();
        assert_eq!(png_size(&wide), (96, 48), "fitted and centred on the requested canvas");
        // An <image> that points at a file or a URL is never followed: the canvas stays fully transparent.
        let evil = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image href="file:///etc/hosts" width="8" height="8"/><image xlink:href="http://127.0.0.1:9/x.png" width="8" height="8"/><image href="data:image/png;base64,iVBORw0KGgo=" width="8" height="8"/></svg>"#;
        let png = render_png(evil.as_bytes(), (16, 16)).unwrap();
        let pm = resvg::tiny_skia::Pixmap::decode_png(&png).unwrap();
        assert!(pm.data().chunks(4).all(|p| p[3] == 0), "nothing was drawn from an external reference");
        assert!(render_png(b"not an svg", (16, 16)).is_err());
        assert!(render_png(&vec![b' '; MAX_SVG_BYTES as usize + 1], (16, 16)).is_err());
    }

    #[test]
    fn the_png_is_cached_per_file_version_and_size() {
        let dir = scratch("cache");
        let f = dir.join("logo.svg");
        std::fs::write(&f, SVG).unwrap();
        let cache = Cache::new();
        let a = cache.png(&f, (48, 48)).unwrap();
        let b = cache.png(&f, (48, 48)).unwrap();
        assert_eq!(cache.renders.load(Ordering::Relaxed), 1, "the second call is a cache hit");
        assert!(Arc::ptr_eq(&a, &b));
        let _ = cache.png(&f, (96, 48)).unwrap();
        assert_eq!(cache.renders.load(Ordering::Relaxed), 2, "another pixel size renders again");
        std::fs::write(&f, SVG.replace("#25a7a0", "#112233")).unwrap();
        let _ = cache.png(&f, (96, 48)).unwrap();
        assert_eq!(cache.renders.load(Ordering::Relaxed), 3, "a changed file renders again");
        // the size cap is checked before the file is read
        let big = dir.join("big.svg");
        std::fs::write(&big, vec![b' '; MAX_SVG_BYTES as usize + 1]).unwrap();
        assert!(cache.png(&big, (48, 48)).unwrap_err().contains("1 MiB"));
        assert!(cache.png(&dir.join("missing.svg"), (48, 48)).is_err());
        assert!(cache.png(&dir, (48, 48)).is_err(), "a directory is not a logo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_iterm2_sequence_has_the_exact_framing() {
        let png = render_png(SVG.as_bytes(), (48, 48)).unwrap();
        let s = iterm2_sequence(&png, 6, 3);
        assert!(s.starts_with("\x1b]1337;File=inline=1;width=6;height=3;preserveAspectRatio=1:iVBORw0KGgo"), "{}", &s[..80]);
        assert!(s.ends_with('\x07') && s.matches('\x1b').count() == 1 && s.matches('\x07').count() == 1);
    }

    #[test]
    fn the_kitty_sequence_is_chunked_and_carries_the_whole_png() {
        let big = vec![7u8; 10_000]; // 13336 base64 characters: four chunks
        let parts = kitty_sequences(&big, 6, 3);
        assert_eq!(parts.len(), 4);
        assert!(parts[0].starts_with("\x1b_Ga=T,f=100,c=6,r=3,C=1,q=2,m=1;"), "{}", &parts[0][..40]);
        assert!(parts[1].starts_with("\x1b_Gm=1;") && parts[2].starts_with("\x1b_Gm=1;") && parts[3].starts_with("\x1b_Gm=0;"));
        assert!(parts.iter().all(|p| p.ends_with("\x1b\\")));
        let payload: String = parts.iter().map(|p| p.split_once(';').unwrap().1.trim_end_matches("\x1b\\")).collect();
        assert_eq!(payload, b64_encode(&big));
        assert!(parts.iter().all(|p| p.split_once(';').unwrap().1.trim_end_matches("\x1b\\").len() <= 4096));
        assert_eq!(kitty_sequences(b"x", 6, 3).len(), 1);
        assert!(kitty_sequences(b"x", 6, 3)[0].contains(",m=0;"));
    }

    #[test]
    fn load_picks_the_protocol_wraps_in_tmux_and_falls_back_to_none() {
        let dir = scratch("load");
        let f = dir.join("logo.svg");
        std::fs::write(&f, SVG).unwrap();
        let path: &'static str = Box::leak(f.to_string_lossy().to_string().into_boxed_str());
        let cache = Cache::new();
        let iterm = Logo::load_with(&cache, &env_of(vec![("TERM_PROGRAM", "iTerm.app"), ("REMOTE_SESSIONS_LOGO_SVG", path)]), Some((10, 20))).unwrap();
        assert_eq!((iterm.protocol, iterm.cols, iterm.rows), (Protocol::Iterm2, 6, 3));
        assert!(iterm.sequence.starts_with("\x1b]1337;File=inline=1;width=6;height=3;preserveAspectRatio=1:"));
        let png = cache.png(&f, (60, 60)).unwrap();
        assert_eq!(png_size(&png), (60, 60), "6 x 3 cells of 10 x 20 px");
        // inside tmux the same sequence is wrapped
        let in_tmux = Logo::load_with(&cache, &env_of(vec![("LC_TERMINAL", "iTerm2"), ("TMUX", "/tmp/x,1,0"), ("REMOTE_SESSIONS_LOGO_SVG", path)]), Some((10, 20))).unwrap();
        assert!(in_tmux.sequence.starts_with("\x1bPtmux;\x1b\x1b]1337;File=inline=1;") && in_tmux.sequence.ends_with("\x07\x1b\\"));
        assert_eq!(cache.renders.load(Ordering::Relaxed), 1, "same file and pixel size: rendered once for both");
        // kitty: a delete first, then the chunks
        let kitty = Logo::load_with(&cache, &env_of(vec![("TERM", "xterm-kitty"), ("REMOTE_SESSIONS_LOGO_SVG", path)]), None).unwrap();
        assert!(kitty.sequence.starts_with("\x1b_Ga=d,d=A,q=2\x1b\\\x1b_Ga=T,f=100,c=6,r=3,C=1,q=2,m="));
        // no supported terminal, no file, no HOME: None, and nothing is rendered for them
        let before = cache.renders.load(Ordering::Relaxed);
        assert!(Logo::load_with(&cache, &env_of(vec![("TERM", "xterm-256color"), ("REMOTE_SESSIONS_LOGO_SVG", path)]), None).is_none());
        assert!(Logo::load_with(&cache, &env_of(vec![("TERM_PROGRAM", "iTerm.app"), ("HOME", "/nonexistent-home")]), None).is_none());
        assert!(Logo::load_with(&cache, &env_of(vec![("TERM_PROGRAM", "iTerm.app")]), None).is_none());
        std::fs::write(&f, "not an svg").unwrap();
        assert!(Logo::load_with(&cache, &env_of(vec![("TERM_PROGRAM", "iTerm.app"), ("REMOTE_SESSIONS_LOGO_SVG", path)]), None).is_none(), "an unusable SVG falls back");
        assert_eq!(cache.renders.load(Ordering::Relaxed), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_64_bytes_for_one_canned_svg() {
        let dir = scratch("hex");
        let f = dir.join("logo.svg");
        std::fs::write(&f, SVG).unwrap();
        let path: &'static str = Box::leak(f.to_string_lossy().to_string().into_boxed_str());
        let cache = Cache::new();
        for (name, env) in [
            ("iterm2", vec![("TERM_PROGRAM", "iTerm.app"), ("REMOTE_SESSIONS_LOGO_SVG", path)]),
            ("iterm2-in-tmux", vec![("LC_TERMINAL", "iTerm2"), ("TMUX", "/x,1,0"), ("REMOTE_SESSIONS_LOGO_SVG", path)]),
            ("kitty", vec![("TERM", "xterm-kitty"), ("REMOTE_SESSIONS_LOGO_SVG", path)]),
        ] {
            let l = Logo::load_with(&cache, &env_of(env), Some((8, 16))).unwrap();
            let head = &l.sequence.as_bytes()[..64];
            println!("HEX64 {name}: {}", hex(head));
            println!("TXT64 {name}: {:?}", String::from_utf8_lossy(head));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
