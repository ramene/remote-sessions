//! Text helpers: sanitising untrusted text, display-width aware wrapping and truncation, light markdown.
use unicode_width::UnicodeWidthChar;

/// Display width of a char (0 for combining marks, 2 for wide).
pub fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

pub fn width(s: &str) -> usize {
    s.chars().map(cw).sum()
}

/// Remove control and bidi-override characters (keeps `\n`, turns `\t` into four spaces), and cap the length.
/// All text from a session goes through here before it is drawn.
pub fn clean(text: &str, limit: usize) -> String {
    let mut out = String::with_capacity(text.len().min(limit + 8));
    for c in text.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\r' => {}
            c if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}') => {}
            c => out.push(c),
        }
        if out.len() > limit {
            out.push_str("\n…");
            break;
        }
    }
    out
}

/// One line: whitespace collapsed, at most `max` display columns, `…` when cut.
pub fn one_line(text: &str, max: usize) -> String {
    let flat = clean(text, max * 8 + 64).split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flat, max)
}

/// Cut to `max` display columns, ending in `…` when something was cut.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let n = cw(c);
        if w + n + 1 > max {
            break;
        }
        out.push(c);
        w += n;
    }
    out.push('…');
    out
}

/// `12s`, `1m 4s`, `1h 02m`.
pub fn fmt_dur(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sty {
    Plain,
    Bold,
    Code,
    /// A line inside a fenced block.
    Fence,
}

pub type Run = (String, Sty);

/// Greedy wrap of styled cells: break at the last space when there is one, else inside a word. `first` and `rest` are
/// the usable widths of the first and later lines (at least 1).
fn wrap_cells(cells: &[(char, Sty)], first: usize, rest: usize) -> Vec<Vec<(char, Sty)>> {
    let mut lines: Vec<Vec<(char, Sty)>> = Vec::new();
    let (mut cur, mut curw, mut last_space): (Vec<(char, Sty)>, usize, Option<usize>) = (Vec::new(), 0, None);
    let mut avail = first.max(1);
    for &(c, s) in cells {
        let w = cw(c);
        if curw + w > avail && !cur.is_empty() {
            if c == ' ' {
                // The space itself is where the line ends: drop it.
                lines.push(std::mem::take(&mut cur));
                curw = 0;
                last_space = None;
                avail = rest.max(1);
                continue;
            }
            if let Some(i) = last_space {
                let tail = cur.split_off(i + 1);
                cur.pop();
                lines.push(std::mem::replace(&mut cur, tail));
                curw = cur.iter().map(|x| cw(x.0)).sum();
            } else {
                lines.push(std::mem::take(&mut cur));
                curw = 0;
            }
            last_space = None;
            avail = rest.max(1);
        }
        if c == ' ' {
            last_space = Some(cur.len());
        }
        cur.push((c, s));
        curw += w;
    }
    lines.push(cur);
    lines
}

fn group(cells: Vec<(char, Sty)>) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (c, s) in cells {
        match runs.last_mut() {
            Some((t, rs)) if *rs == s => t.push(c),
            _ => runs.push((c.to_string(), s)),
        }
    }
    runs
}

/// Plain text wrapped to `width` columns (newlines kept).
pub fn wrap_plain(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let cells: Vec<(char, Sty)> = line.chars().map(|c| (c, Sty::Plain)).collect();
        for l in wrap_cells(&cells, width, width) {
            out.push(l.into_iter().map(|x| x.0).collect());
        }
    }
    out
}

/// Inline markup of one line: `**bold**` and `` `code` `` (only when the closer exists).
fn inline(line: &str) -> Vec<(char, Sty)> {
    let chars: Vec<char> = line.chars().collect();
    let (mut out, mut i, mut bold, mut code) = (Vec::new(), 0usize, false, false);
    let has = |from: usize, pat: &[char]| chars[from..].windows(pat.len()).any(|w| w == pat);
    while i < chars.len() {
        if !code && chars[i] == '*' && chars.get(i + 1) == Some(&'*') && (bold || has(i + 2, &['*', '*'])) {
            bold = !bold;
            i += 2;
            continue;
        }
        if chars[i] == '`' && (code || has(i + 1, &['`'])) {
            code = !code;
            i += 1;
            continue;
        }
        out.push((chars[i], if code { Sty::Code } else if bold { Sty::Bold } else { Sty::Plain }));
        i += 1;
    }
    out
}

/// Light markdown to styled lines of at most `width` columns: bold, inline code, bullets, fenced code. Lines inside a
/// fence are `Sty::Fence`; the view draws the dim box prefix.
pub fn md_lines(text: &str, width: usize) -> Vec<Vec<Run>> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            let cells: Vec<(char, Sty)> = line.chars().map(|c| (c, Sty::Fence)).collect();
            for l in wrap_cells(&cells, width.saturating_sub(2), width.saturating_sub(2)) {
                out.push(group(l));
            }
            continue;
        }
        let indent = line.len() - trimmed.len();
        let (bullet, body) = match trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            Some(rest) => (true, rest),
            None => (false, trimmed),
        };
        let mut cells: Vec<(char, Sty)> = Vec::new();
        cells.extend(std::iter::repeat((' ', Sty::Plain)).take(indent.min(width / 2)));
        let lead = cells.len();
        if bullet {
            cells.push(('•', Sty::Plain));
            cells.push((' ', Sty::Plain));
        }
        cells.extend(inline(body));
        let hang = lead + if bullet { 2 } else { 0 };
        let wrapped = wrap_cells(&cells, width, width.saturating_sub(hang));
        for (n, l) in wrapped.into_iter().enumerate() {
            let mut l = l;
            if n > 0 {
                let mut pad: Vec<(char, Sty)> = std::iter::repeat((' ', Sty::Plain)).take(hang).collect();
                pad.append(&mut l);
                l = pad;
            }
            out.push(group(l));
        }
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_drops_escapes_and_bidi_and_caps() {
        assert_eq!(clean("a\u{1b}[31mb\u{202e}c\td\r\ne", 100), "a[31mbc    d\ne");
        assert!(clean(&"x".repeat(100_000), 1000).len() < 1100);
    }
    #[test]
    fn truncate_counts_columns_not_bytes() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("日本語です", 5), "日本…");
        assert_eq!(truncate("ab", 4), "ab");
        assert_eq!(fmt_dur(61), "1m 1s");
        assert_eq!(fmt_dur(7260), "2h 01m");
    }
    #[test]
    fn wrapping_prefers_spaces_and_hard_breaks_long_words() {
        assert_eq!(wrap_plain("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap_plain("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_plain("a\n\nb", 5), ["a", "", "b"]);
    }
    #[test]
    fn wrapping_never_panics_for_any_width() {
        let samples = ["", "hello world", "日本語のテキストです。長い行", "**bold** and `code` and - bullet", &"word ".repeat(80), "```\ncode line that is long\n```"];
        for w in 1..300 {
            for s in samples {
                let _ = wrap_plain(s, w);
                let lines = md_lines(s, w);
                assert!(!lines.is_empty());
                for l in lines {
                    let used: usize = l.iter().map(|r| width(&r.0)).sum();
                    assert!(used <= w.max(2) + 2, "width {w}: {used} for {s:?}");
                }
            }
        }
    }
    #[test]
    fn markdown_bold_code_bullets_and_fences() {
        let l = md_lines("a **b** `c` d", 40);
        assert_eq!(l[0], vec![("a ".to_string(), Sty::Plain), ("b".to_string(), Sty::Bold), (" ".to_string(), Sty::Plain), ("c".to_string(), Sty::Code), (" d".to_string(), Sty::Plain)]);
        assert_eq!(md_lines("- item", 40)[0][0].0, "• item");
        let f = md_lines("x\n```rs\nlet a = 1;\n```\ny", 40);
        assert_eq!(f.len(), 3);
        assert_eq!(f[1], vec![("let a = 1;".to_string(), Sty::Fence)]);
        assert_eq!(md_lines("2 ** 3 and a ` tick", 40)[0].len(), 1, "unmatched markers stay literal");
    }
}

/// Standard-alphabet base64 with padding.
pub fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
