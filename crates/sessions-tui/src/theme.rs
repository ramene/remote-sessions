//! Palette and glyph sets. Colors are ours (teal / amber / slate / coral); with NO_COLOR nothing is colored and
//! emphasis is bold / underline / reverse only. The glyph set switches to ASCII with NO_COLOR, `--ascii` or TERM=dumb.
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// The two palettes. `Classic` is the original teal / amber / slate set; `RenderSvg` is the palette of the renderSVG / archify
/// diagrams (midnight): near-black surfaces, neutral ink, one teal for what is local and live, one amber for what waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Classic,
    RenderSvg,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub color: bool,
    pub truecolor: bool,
    pub ascii: bool,
    pub look: Look,
}

// renderSVG tokens (midnight), as (r, g, b).
const BG2: (u8, u8, u8) = (0x11, 0x11, 0x11);
const NODE_LINE: (u8, u8, u8) = (0x33, 0x33, 0x33);
const INK: (u8, u8, u8) = (0xf5, 0xf5, 0xf5);
const INK2: (u8, u8, u8) = (0xa3, 0xa3, 0xa3);
const INK3: (u8, u8, u8) = (0x73, 0x73, 0x73);
const LOCAL: (u8, u8, u8) = (0x2d, 0xd4, 0xbf);
const LOCAL_SOFT: (u8, u8, u8) = (0x0b, 0x3d, 0x33);
const FRONTIER: (u8, u8, u8) = (0xfb, 0xbf, 0x24);
const ROSE: (u8, u8, u8) = (0xfb, 0x71, 0x85);

/// The xterm-256 palette entry nearest to a color (the 6x6x6 cube or the gray ramp).
pub fn nearest_256(c: (u8, u8, u8)) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let d = |a: (u8, u8, u8), b: (u8, u8, u8)| {
        let (dr, dg, db) = (a.0 as i32 - b.0 as i32, a.1 as i32 - b.1 as i32, a.2 as i32 - b.2 as i32);
        dr * dr + dg * dg + db * db
    };
    let nearest_level = |v: u8| (0..6).min_by_key(|&i| (LEVELS[i] as i32 - v as i32).abs()).unwrap_or(0);
    let (r, g, b) = (nearest_level(c.0), nearest_level(c.1), nearest_level(c.2));
    let cube = (LEVELS[r], LEVELS[g], LEVELS[b]);
    let cube_idx = 16 + 36 * r as u8 + 6 * g as u8 + b as u8;
    let avg = ((c.0 as u32 + c.1 as u32 + c.2 as u32) / 3) as i32;
    let gi = ((avg - 8 + 5) / 10).clamp(0, 23) as u8;
    let gv = 8 + 10 * gi;
    let gray = (gv, gv, gv);
    if d(c, gray) < d(c, cube) { 232 + gi } else { cube_idx }
}

impl Theme {
    pub fn plain() -> Theme {
        Theme { color: false, truecolor: false, ascii: true, look: Look::Classic }
    }
    pub fn color() -> Theme {
        Theme { color: true, truecolor: true, ascii: false, look: Look::Classic }
    }
    /// The renderSVG look, full color (what `detect` picks on a truecolor terminal).
    pub fn rendersvg() -> Theme {
        Theme { color: true, truecolor: true, ascii: false, look: Look::RenderSvg }
    }
    /// From the environment: `NO_COLOR` (any value), `TERM=dumb`, `COLORTERM`. The flags force the plain sets.
    pub fn detect(no_color_flag: bool, ascii_flag: bool) -> Theme {
        let env_no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let dumb = std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false);
        let color = !(no_color_flag || env_no_color || dumb);
        let truecolor = std::env::var("COLORTERM").map(|c| c == "truecolor" || c == "24bit").unwrap_or(false);
        // REMOTE_SESSIONS_THEME=classic|rendersvg chooses; unset, renderSVG is the look wherever truecolor is available.
        let look = match std::env::var("REMOTE_SESSIONS_THEME").ok().as_deref().map(str::trim) {
            Some("classic") => Look::Classic,
            Some("rendersvg") => Look::RenderSvg,
            _ if color && truecolor => Look::RenderSvg,
            _ => Look::Classic,
        };
        Theme { color, truecolor, ascii: ascii_flag || !color, look }
    }

    /// renderSVG look in color.
    fn rs(&self) -> bool {
        self.color && self.look == Look::RenderSvg
    }
    /// A renderSVG token: true color, or its nearest xterm-256 entry.
    fn tok(&self, c: (u8, u8, u8)) -> Color {
        if self.truecolor { Color::Rgb(c.0, c.1, c.2) } else { Color::Indexed(nearest_256(c)) }
    }
    /// Panels are separated by a blank row (renderSVG spacing); Classic keeps them touching.
    pub fn air(&self) -> bool {
        self.rs()
    }
    /// Panel and section titles: ink, not bold, under renderSVG.
    pub fn title(&self) -> Style {
        if self.rs() { Style::default().fg(self.tok(INK)) } else { self.bold() }
    }
    /// A panel title while the panel has the selection.
    pub fn panel_title(&self, focused: bool) -> Style {
        match (focused, self.rs()) {
            (true, true) => Style::default().fg(self.tok(LOCAL)),
            (true, false) => self.accent().add_modifier(Modifier::BOLD),
            (false, _) => self.title(),
        }
    }
    /// The wordmark in the header.
    pub fn brand(&self) -> Style {
        if self.rs() { Style::default().fg(self.tok(LOCAL)) } else { self.accent().add_modifier(Modifier::BOLD) }
    }
    /// The close button.
    pub fn close_style(&self) -> Style {
        if self.rs() { Style::default().fg(self.tok(ROSE)) } else { self.error().add_modifier(Modifier::BOLD) }
    }
    /// Body text of a panel: ink-2 under renderSVG, the terminal's own color otherwise.
    pub fn body(&self) -> Style {
        if self.rs() { Style::default().fg(self.tok(INK2)) } else { Style::default() }
    }
    /// The header band's surface (bg-2 under renderSVG).
    pub fn band(&self) -> Style {
        if self.rs() { Style::default().bg(self.tok(BG2)) } else { Style::default() }
    }
    /// A panel's border: node-line, teal while it holds the selection (Classic: dim / accent, as before).
    pub fn panel_border(&self, focused: bool) -> Style {
        match (focused, self.rs()) {
            (true, true) => Style::default().fg(self.tok(LOCAL)),
            (false, true) => Style::default().fg(self.tok(NODE_LINE)),
            (true, false) => self.accent(),
            (false, false) => self.dim(),
        }
    }

    fn pick(&self, rgb: (u8, u8, u8), basic: Color) -> Color {
        if self.truecolor { Color::Rgb(rgb.0, rgb.1, rgb.2) } else { basic }
    }
    pub fn accent(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(LOCAL)); }
        if self.color { Style::default().fg(self.pick((0x2e, 0xc4, 0xb6), Color::Cyan)) } else { Style::default().add_modifier(Modifier::BOLD) }
    }
    pub fn accent2(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(FRONTIER)); }
        if self.color { Style::default().fg(self.pick((0xff, 0xb7, 0x03), Color::Yellow)) } else { Style::default() }
    }
    pub fn dim(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(INK3)); }
        if self.color { Style::default().fg(Color::DarkGray) } else { Style::default() }
    }
    pub fn error(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(ROSE)); }
        if self.color { Style::default().fg(self.pick((0xef, 0x47, 0x6f), Color::LightRed)) } else { Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED) }
    }
    pub fn warn(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(FRONTIER)); }
        if self.color { Style::default().fg(self.pick((0xff, 0xb7, 0x03), Color::Yellow)) } else { Style::default().add_modifier(Modifier::BOLD) }
    }
    pub fn ok(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(LOCAL)); }
        if self.color { Style::default().fg(self.pick((0x06, 0xd6, 0xa0), Color::Green)) } else { Style::default() }
    }
    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    pub fn code(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(FRONTIER)); }
        if self.color { Style::default().fg(self.pick((0xff, 0xb7, 0x03), Color::Yellow)) } else { Style::default().add_modifier(Modifier::UNDERLINED) }
    }
    /// The faint background of a user turn.
    pub fn user_bg(&self) -> Style {
        if self.color && self.truecolor { Style::default().bg(Color::Rgb(0x1f, 0x29, 0x33)) } else { Style::default() }
    }
    pub fn selected(&self) -> Style {
        if self.rs() { return Style::default().fg(self.tok(LOCAL)).bg(self.tok(LOCAL_SOFT)); }
        if self.color { self.accent().add_modifier(Modifier::BOLD) } else { Style::default().add_modifier(Modifier::REVERSED) }
    }

    pub fn user_glyph(&self) -> &'static str { ">" }
    pub fn assistant_glyph(&self) -> &'static str { if self.ascii { "*" } else { "●" } }
    pub fn tool_glyph(&self) -> &'static str { if self.ascii { "+" } else { "◆" } }
    pub fn hang_glyph(&self) -> &'static str { if self.ascii { "|" } else { "╰" } }
    pub fn prompt_glyph(&self) -> &'static str { if self.ascii { ">" } else { "›" } }
    pub fn notice_glyph(&self) -> &'static str { if self.ascii { "." } else { "·" } }
    pub fn rule_char(&self) -> &'static str { if self.ascii { "-" } else { "─" } }
    pub fn bar(&self) -> &'static str { if self.ascii { "|" } else { "│" } }
    pub fn ellipsis(&self) -> &'static str { "…" }
    pub fn sent_mark(&self) -> &'static str { if self.ascii { "." } else { "·" } }
    pub fn done_mark(&self) -> &'static str { if self.ascii { "v" } else { "✓" } }
    pub fn fail_mark(&self) -> &'static str { "!" }
    pub fn spinner(&self, now_ms: u64) -> &'static str {
        const BRAILLE: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        const BAR: [&str; 4] = ["|", "/", "-", "\\"];
        let n = (now_ms / 100) as usize;
        if self.ascii { BAR[n % 4] } else { BRAILLE[n % 8] }
    }
    pub fn down_arrow(&self) -> &'static str { if self.ascii { "v" } else { "↓" } }
    pub fn dot(&self) -> &'static str { if self.ascii { "*" } else { "●" } }
    pub fn half(&self) -> &'static str { if self.ascii { "~" } else { "◐" } }
    pub fn ring(&self) -> &'static str { if self.ascii { "o" } else { "○" } }
    pub fn cross(&self) -> &'static str { if self.ascii { "x" } else { "✕" } }
    /// The waiting badge.
    pub fn pause_glyph(&self) -> &'static str { if self.ascii { "||" } else { "\u{23f8}" } }
    /// The background of the selected row in a list: slate with color, reverse video without.
    pub fn row_bg(&self) -> Style {
        // renderSVG: the selected row is local-soft with local text.
        if self.rs() { return Style::default().fg(self.tok(LOCAL)).bg(self.tok(LOCAL_SOFT)); }
        if self.color && self.truecolor { Style::default().bg(Color::Rgb(0x26, 0x33, 0x40)) }
        else { Style::default().add_modifier(Modifier::REVERSED) }
    }
    pub fn sep(&self) -> &'static str { if self.ascii { " | " } else { " │ " } }

    /// One glyph of ours in its ASCII form (other characters are returned as they are).
    pub fn fold_char(c: char) -> char {
        match c {
            '·' => '.',
            '…' => '~',
            '↓' => 'v',
            '●' => '*',
            '◆' => '+',
            '╰' => '|',
            '›' => '>',
            '─' => '-',
            '│' => '|',
            '┌' | '┐' | '└' | '┘' => '+',
            '◐' => '~',
            '○' => 'o',
            '✕' => 'x',
            '✓' => 'v',
            '\u{23f8}' => '|',
            '▌' => '>',
            '▶' => '>',
            '▏' => '_',
            '•' => '-',
            c => c,
        }
    }

    /// In ASCII mode, replace our non-ASCII glyphs in these lines.
    pub fn fold_lines(&self, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        if !self.ascii {
            return lines;
        }
        lines.into_iter().map(|l| Line { style: l.style, alignment: l.alignment, spans: l.spans.into_iter().map(|s| Span::styled(s.content.chars().map(Self::fold_char).collect::<String>(), s.style)).collect() }).collect()
    }

    /// In ASCII mode, fold every cell of a drawn buffer.
    pub fn fold_buffer(&self, buf: &mut Buffer) {
        if !self.ascii {
            return;
        }
        for cell in buf.content.iter_mut() {
            let mut chars = cell.symbol().chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                let f = Self::fold_char(c);
                if f != c {
                    cell.set_char(f);
                }
            }
        }
    }
}
