//! Drawing. Block renderers are pure: `render_block(block, width, theme, expanded) -> Vec<Line>`. `draw` lays out the
//! whole screen (transcript, status strip, input box, footer) and the modals. Every string from the session is cleaned.
use crate::model::*;
use crate::text::{clean, fmt_dur, md_lines, one_line, truncate, width, wrap_plain, Sty};
use crate::theme::Theme;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as Frame_, Borders, Clear, Paragraph};
use ratatui::Frame;
use serde_json::Value;

/// Where the last draw put things (the app uses it for scrolling and clicks).
#[derive(Clone, Debug, Default)]
pub struct DrawInfo {
    pub total: usize,
    pub height: usize,
    /// Per transcript row (top to bottom): the block drawn there.
    pub rows: Vec<Option<usize>>,
}

pub const MIN_W: u16 = 24;
pub const MIN_H: u16 = 8;
pub const INPUT_MAX_ROWS: usize = 6;
const MODAL_BODY_CAP: usize = 12;

pub(crate) fn sp(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

fn pad_to(spans: &mut Vec<Span<'static>>, used: usize, w: usize, style: Style) {
    if used < w {
        spans.push(sp(" ".repeat(w - used), style));
    }
}

/// The one-line argument summary of a tool call.
pub fn tool_args(name: &str, input: &Value, max: usize) -> String {
    let get = |k: &str| input[k].as_str().map(str::to_string);
    let s = match name {
        "Bash" => get("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => get("file_path").or_else(|| get("path")).or_else(|| get("notebook_path")),
        "Glob" | "Grep" => get("pattern"),
        "WebFetch" => get("url"),
        "WebSearch" => get("query"),
        _ => None,
    };
    let s = s.unwrap_or_else(|| match input {
        Value::Object(m) => m.iter().filter_map(|(k, v)| match v {
            Value::String(x) => Some(format!("{k}={x}")),
            Value::Number(n) => Some(format!("{k}={n}")),
            Value::Bool(b) => Some(format!("{k}={b}")),
            _ => None,
        }).take(3).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    });
    one_line(&s, max)
}

fn result_lines(out: &ToolOut, w: usize, first_prefix: &str, rest_prefix: &str, th: &Theme, expanded: bool) -> Vec<Line<'static>> {
    let style = if out.is_error { th.error() } else { th.dim() };
    let text = clean(&out.text, 8 * 1024);
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let pw = width(first_prefix).max(width(rest_prefix));
    let inner = w.saturating_sub(pw).max(1);
    let lead = if out.is_error { "error: " } else { "" };
    let mut rows: Vec<Line<'static>> = Vec::new();
    if lines.is_empty() {
        rows.push(Line::from(vec![sp(first_prefix.to_string(), th.dim()), sp(format!("{lead}(no output)"), style)]));
        return rows;
    }
    if !expanded {
        let first = format!("{lead}{}", lines[0].trim_end());
        rows.push(Line::from(vec![sp(first_prefix.to_string(), th.dim()), sp(truncate(&first, inner), style)]));
        if lines.len() > 1 {
            rows.push(Line::from(vec![sp(rest_prefix.to_string(), th.dim()), sp(truncate(&format!("… +{} lines · ctrl-o to expand", lines.len() - 1), inner), th.dim())]));
        }
        return rows;
    }
    let mut first = true;
    for (i, l) in lines.iter().enumerate() {
        let src = if i == 0 { format!("{lead}{l}") } else { l.to_string() };
        for piece in wrap_plain(&src, inner) {
            let prefix = if first { first_prefix } else { rest_prefix };
            first = false;
            rows.push(Line::from(vec![sp(prefix.to_string(), th.dim()), sp(piece, style)]));
        }
    }
    rows
}

/// One block as lines of at most `width` columns.
pub fn render_block(b: &Block, width_: u16, th: &Theme, expanded: bool) -> Vec<Line<'static>> {
    th.fold_lines(render_block_raw(b, width_, th, expanded))
}

fn render_block_raw(b: &Block, width_: u16, th: &Theme, expanded: bool) -> Vec<Line<'static>> {
    let w = width_ as usize;
    let inner = w.saturating_sub(2).max(1);
    match &b.kind {
        BlockKind::User { text, mark } => {
            let bg = th.user_bg();
            let mut out = Vec::new();
            let wrapped = wrap_plain(&clean(text, 8 * 1024), inner);
            let n = wrapped.len();
            for (i, l) in wrapped.into_iter().enumerate() {
                let mut spans = vec![if i == 0 { sp(th.user_glyph(), th.accent().patch(bg)) } else { sp(" ", bg) }, sp(" ", bg), sp(l.clone(), bg)];
                let mut used = 2 + crate::text::width(&l);
                if i + 1 == n {
                    let m = match mark {
                        Mark::Local => "",
                        Mark::Sent => th.sent_mark(),
                        Mark::Processed => th.done_mark(),
                        Mark::Failed => th.fail_mark(),
                    };
                    if !m.is_empty() && used + 2 <= w {
                        spans.push(sp(format!(" {m}"), if *mark == Mark::Failed { th.error().patch(bg) } else { th.dim().patch(bg) }));
                        used += 2;
                    }
                }
                pad_to(&mut spans, used, w, bg);
                out.push(Line::from(spans));
            }
            out
        }
        BlockKind::Assistant(text) => {
            let mut out = Vec::new();
            for (i, runs) in md_lines(&clean(text, 8 * 1024), inner).into_iter().enumerate() {
                let mut spans = vec![if i == 0 { sp(th.assistant_glyph(), th.accent2()) } else { sp(" ", Style::default()) }, sp(" ", Style::default())];
                let fenced = runs.first().is_some_and(|r| r.1 == Sty::Fence);
                if fenced {
                    spans.push(sp(format!("{} ", th.bar()), th.dim()));
                }
                for (t, st) in runs {
                    let style = match st {
                        Sty::Plain => Style::default(),
                        Sty::Bold => th.bold(),
                        Sty::Code => th.code(),
                        Sty::Fence => th.dim(),
                    };
                    spans.push(sp(t, style));
                }
                out.push(Line::from(spans));
            }
            out
        }
        BlockKind::Tool { name, input, result, .. } => {
            let name = clean(name, 60);
            let mut head = vec![sp(th.tool_glyph(), th.accent()), sp(" ", Style::default())];
            let room = w.saturating_sub(2 + crate::text::width(&name) + 2);
            let args = if w < 40 || room < 4 { String::new() } else { tool_args(&name, input, room) };
            if args.is_empty() {
                head.push(sp(truncate(&name, inner), th.bold()));
            } else {
                head.push(sp(name, th.bold()));
                head.push(sp(format!("({args})"), th.dim()));
            }
            let mut out = vec![Line::from(head)];
            if let Some(r) = result {
                out.extend(result_lines(r, w, &format!("  {} ", th.hang_glyph()), "    ", th, expanded));
            }
            out
        }
        BlockKind::Result(r) => result_lines(r, w, &format!("{} ", th.hang_glyph()), "  ", th, expanded),
        BlockKind::Notice { text, level } => {
            let style = match level {
                Level::Dim => th.dim(),
                Level::Warn => th.warn(),
                Level::Error => th.error(),
            };
            let mut out = Vec::new();
            for (i, l) in wrap_plain(&clean(text, 8 * 1024), inner).into_iter().enumerate() {
                out.push(Line::from(vec![sp(if i == 0 { format!("{} ", th.notice_glyph()) } else { "  ".to_string() }, style), sp(l, style)]));
            }
            out
        }
        BlockKind::Divider(text) => {
            let rule = th.rule_char();
            let label = truncate(&clean(text, 200), w.saturating_sub(6).max(1));
            let used = 3 + crate::text::width(&label);
            vec![Line::from(vec![sp(format!("{rule}{rule} {label} {}", rule.repeat(w.saturating_sub(used + 1))), th.dim())])]
        }
    }
}

/// All blocks as lines, and which block owns each line.
pub fn transcript(s: &State, w: u16, th: &Theme) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
    let (mut lines, mut owners) = (Vec::new(), Vec::new());
    for (i, b) in s.blocks.iter().enumerate() {
        let attach = matches!(b.kind, BlockKind::Notice { .. } | BlockKind::Divider(_));
        if i > 0 && !attach {
            lines.push(Line::default());
            owners.push(None);
        }
        let expanded = s.expand_all || s.expanded.contains(&b.id);
        for l in render_block(b, w, th, expanded) {
            lines.push(l);
            owners.push(Some(b.id));
        }
    }
    (lines, owners)
}

// ---- status strip and footer ---------------------------------------------------------------------------------

pub fn status_line(s: &State, w: u16, th: &Theme) -> Line<'static> {
    let w = w as usize;
    let (left, style): (String, Style) = if s.ended {
        ("session ended · read-only".into(), th.warn())
    } else if matches!(s.conn, Conn::Connecting) {
        (format!("{} connecting to {}…", th.spinner(s.now_ms), one_line(&s.name, 40)), th.dim())
    } else if !s.prompts.is_empty() {
        ("waiting for your decision".into(), th.warn())
    } else {
        match &s.turn {
            Turn::Idle => (String::new(), th.dim()),
            Turn::Waiting { since } | Turn::Working { since, .. } => {
                let secs = s.now_ms.saturating_sub(*since) / 1000;
                let mut t = match &s.turn {
                    Turn::Waiting { .. } => format!("{} waiting for the session · {}", th.spinner(s.now_ms), fmt_dur(secs)),
                    Turn::Working { tool, .. } => format!("{} session is working · {}{}", th.spinner(s.now_ms), fmt_dur(secs), tool.as_ref().map(|t| format!(" · {t}")).unwrap_or_default()),
                    Turn::Idle => String::new(),
                };
                let gap = s.now_ms.saturating_sub(s.last_event_ms.max(*since)) / 1000;
                if gap >= 20 {
                    t.push_str(&format!(" · no events for {gap}s"));
                }
                if gap > 120 {
                    t.push_str(" · esc to interrupt");
                }
                (t, th.dim())
            }
        }
    };
    let right = if s.scroll > 0 {
        if s.new_count > 0 { format!("{} {} new · End to jump", th.down_arrow(), s.new_count) } else { format!("{} End to jump", th.down_arrow()) }
    } else {
        String::new()
    };
    let left = truncate(&left, w.saturating_sub(width(&right) + 1));
    let gap = w.saturating_sub(width(&left) + width(&right));
    Line::from(vec![sp(left, style), sp(" ".repeat(gap), Style::default()), sp(right, th.dim())])
}

fn link_text(s: &State, th: &Theme) -> (String, Style) {
    if s.ended {
        return (format!("{} ended", th.ring()), th.warn());
    }
    match s.conn {
        Conn::Connecting => (format!("{} connecting", th.ring()), th.dim()),
        Conn::Live => (format!("{} live", th.dot()), th.ok()),
        Conn::Polling => (format!("{} polling 3s", th.half()), th.warn()),
        Conn::Reconnecting(_) => (format!("{} reconnecting", th.ring()), th.warn()),
        Conn::Offline => (format!("{} offline", th.cross()), th.error()),
    }
}

fn hints(s: &State) -> &'static str {
    match &s.modal {
        Modal::Permission { .. } => "waiting for you",
        Modal::Help | Modal::ModePicker { .. } => "esc close",
        Modal::None if s.ended => "ctrl-c leave",
        Modal::None if s.busy() => "esc stop · tab mode · ctrl-o expand",
        Modal::None => "tab mode · ctrl-o expand · ? help",
    }
}

/// The one-row footer. Segments drop by priority when the row is narrow: hints first, then rec, then host, then the name is cut.
pub fn footer_line(s: &State, w: u16, th: &Theme) -> Line<'static> {
    let w = w as usize;
    let sep = th.sep();
    let mode = match s.mode.as_deref() {
        Some(m) => (mode_label(m).to_string(), if m == "bypassPermissions" { th.warn() } else { Style::default() }),
        None => ("mode ?".to_string(), th.dim()),
    };
    let (link, link_style) = link_text(s, th);
    let rec = s.rec_since.map(|t| {
        let secs = s.now_ms.saturating_sub(t) / 1000;
        format!("{} rec {:02}:{:02}", th.dot(), secs / 60, secs % 60)
    });
    let mut name = one_line(&s.name, 40);
    let mut host = Some(one_line(&s.host, 24)).filter(|h| !h.is_empty());
    let mut rec = rec;
    let fixed = width(&mode.0) + width(&link) + 2 * width(sep) + 1;
    let total = |name: &str, host: &Option<String>, rec: &Option<String>| {
        fixed + width(name) + host.as_ref().map_or(0, |h| width(h) + width(sep)) + rec.as_ref().map_or(0, |r| width(r) + width(sep))
    };
    if total(&name, &host, &rec) > w {
        rec = None;
    }
    if total(&name, &host, &rec) > w {
        host = None;
    }
    if total(&name, &host, &rec) > w {
        let room = w.saturating_sub(fixed).max(1);
        name = truncate(&name, room);
    }
    let mut spans: Vec<Span<'static>> = vec![sp(" ", Style::default()), sp(name, th.bold())];
    if let Some(h) = &host {
        spans.push(sp(sep, th.dim()));
        spans.push(sp(h.clone(), Style::default()));
    }
    spans.push(sp(sep, th.dim()));
    spans.push(sp(mode.0, mode.1));
    spans.push(sp(sep, th.dim()));
    spans.push(sp(link, link_style));
    if let Some(r) = &rec {
        spans.push(sp(sep, th.dim()));
        spans.push(sp(r.clone(), th.error()));
    }
    let used: usize = spans.iter().map(|x| width(&x.content)).sum();
    let h = hints(s);
    if used + 2 + width(h) < w {
        spans.push(sp(" ".repeat(w - used - width(h) - 1), Style::default()));
        spans.push(sp(h, th.dim()));
    }
    Line::from(spans)
}

// ---- input box -----------------------------------------------------------------------------------------------

/// The input text hard-wrapped to `wrap_w` columns, and the cursor's (row, col) within those rows.
pub fn input_layout(i: &Input, wrap_w: usize) -> (Vec<String>, (usize, usize)) {
    let wrap_w = wrap_w.max(1);
    let (mut rows, mut cur_row, mut col) = (vec![String::new()], 0usize, 0usize);
    let mut cursor = (0usize, 0usize);
    let chars: Vec<char> = i.text.chars().collect();
    for idx in 0..=chars.len() {
        if idx == i.cursor.min(chars.len()) {
            let wc = chars.get(idx).map_or(1, |c| crate::text::cw(*c).max(1));
            if col + wc > wrap_w && idx < chars.len() && chars[idx] != '\n' {
                cur_row += 1;
                rows.push(String::new());
                col = 0;
            }
            cursor = (cur_row, col);
        }
        let Some(&c) = chars.get(idx) else { break };
        if c == '\n' {
            cur_row += 1;
            rows.push(String::new());
            col = 0;
            continue;
        }
        let wc = crate::text::cw(c);
        if col + wc > wrap_w {
            cur_row += 1;
            rows.push(String::new());
            col = 0;
        }
        rows[cur_row].push(if c == '\t' { ' ' } else { c });
        col += wc;
    }
    (rows, cursor)
}

fn input_rows(s: &State, w: u16) -> usize {
    input_layout(&s.input, (w as usize).saturating_sub(2)).0.len().clamp(1, INPUT_MAX_ROWS)
}

// ---- modals --------------------------------------------------------------------------------------------------

pub(crate) fn border_set(th: &Theme) -> border::Set {
    if th.ascii {
        border::Set { top_left: "+", top_right: "+", bottom_left: "+", bottom_right: "+", vertical_left: "|", vertical_right: "|", horizontal_top: "-", horizontal_bottom: "-" }
    } else {
        border::PLAIN
    }
}

fn code_lines(text: &str, w: usize, cap: usize, th: &Theme, fold_hint: &str) -> Vec<Line<'static>> {
    let all: Vec<String> = wrap_plain(&clean(text, 8 * 1024), w.max(1));
    let mut out: Vec<Line<'static>> = all.iter().take(cap).map(|l| Line::from(vec![sp("  ", Style::default()), sp(l.clone(), th.code())])).collect();
    if all.len() > cap {
        out.push(Line::from(sp(format!("  … +{} lines · {fold_hint}", all.len() - cap), th.dim())));
    }
    out
}

fn prompt_body(p: &Prompt, w: usize, cap: usize, th: &Theme) -> Vec<Line<'static>> {
    let hint = "ctrl-o show full input";
    let text = |k: &str| p.input[k].as_str().map(str::to_string);
    match p.tool_name.as_str() {
        "Bash" if text("command").is_some() => code_lines(&text("command").unwrap_or_default(), w, cap, th, hint),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let path = text("file_path").or_else(|| text("path")).or_else(|| text("notebook_path")).unwrap_or_default();
            let mut out = vec![Line::from(vec![sp("  path: ", th.dim()), sp(truncate(&one_line(&path, 400), w.saturating_sub(8).max(1)), th.code())])];
            let lines = |k: &str| text(k).map(|t| t.lines().count());
            let summary = match p.tool_name.as_str() {
                "Write" => lines("content").map(|n| format!("write {n} line{}", if n == 1 { "" } else { "s" })),
                "Edit" => match (lines("old_string"), lines("new_string")) {
                    (Some(a), Some(b)) => Some(format!("replace {a} line{} with {b}", if a == 1 { "" } else { "s" })),
                    _ => None,
                },
                _ => None,
            };
            if let Some(sm) = summary {
                out.push(Line::from(sp(format!("  {sm}"), th.dim())));
            }
            out
        }
        _ => {
            let pretty = serde_json::to_string_pretty(&p.input).unwrap_or_default();
            code_lines(&pretty, w, cap, th, hint)
        }
    }
}

pub(crate) fn modal_rect(area: Rect, content_rows: usize, max_w: u16) -> Rect {
    let h = (content_rows as u16 + 2).min(area.height);
    let w = area.width.min(max_w);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + area.height - h, width: w, height: h }
}

fn draw_permission(f: &mut Frame, area: Rect, s: &State, th: &Theme) {
    let Modal::Permission { choice, reason, sending, full } = &s.modal else { return };
    let Some(p) = s.prompts.first() else { return };
    let iw = (area.width as usize).saturating_sub(2).max(1);
    let fixed = 8usize;
    let avail = (area.height as usize).saturating_sub(2 + fixed).max(1);
    let cap = if *full { avail } else { MODAL_BODY_CAP.min(avail) };
    let verb = if p.tool_name == "Bash" { "wants to run:" } else { "wants to use:" };
    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![sp(format!(" {}", clean(&p.tool_name, 60)), th.bold()), sp(format!(" {verb}"), Style::default())]), Line::default()];
    lines.extend(prompt_body(p, iw.saturating_sub(2), cap, th));
    lines.push(Line::default());
    let opt = |n: usize, label: &str, extra: &str| {
        let on = *choice == n - 1;
        let mark = if on { th.prompt_glyph() } else { " " };
        let mut v = vec![sp(format!(" {mark} {n}  {label}"), if on { th.selected() } else { Style::default() })];
        if !extra.is_empty() {
            v.push(sp(format!("   {extra}"), th.dim()));
        }
        Line::from(v)
    };
    lines.push(opt(1, "Allow once", ""));
    lines.push(opt(2, "Deny", "(3: deny with a reason)"));
    let hint;
    if let Some(r) = reason {
        lines.push(Line::from(vec![sp("   reason (optional): ", th.dim()), sp(clean(r, 300), Style::default()), sp(if th.ascii { "_" } else { "▏" }, th.accent())]));
        hint = "Enter send · Esc back";
    } else if *sending {
        lines.push(Line::from(sp("   sending…", th.dim())));
        hint = "";
    } else {
        lines.push(Line::default());
        hint = "1-2 choose · Enter confirm · Esc deny · ctrl-o show full input";
    }
    lines.push(Line::from(sp(format!(" {}", truncate(hint, iw.saturating_sub(1))), th.dim())));
    let title = if s.prompts.len() > 1 { format!(" permission needed · 1 of {} ", s.prompts.len()) } else { " permission needed ".to_string() };
    let rect = modal_rect(area, lines.len(), area.width);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(Frame_::new().borders(Borders::ALL).border_set(border_set(th)).border_style(th.warn()).title(Span::styled(title, th.warn()))), rect);
}

fn draw_simple_modal(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>, th: &Theme) {
    let rect = modal_rect(area, lines.len(), 64);
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(lines).block(Frame_::new().borders(Borders::ALL).border_set(border_set(th)).border_style(th.accent()).title(Span::styled(format!(" {title} "), th.accent()))), rect);
}

fn help_lines(th: &Theme) -> Vec<Line<'static>> {
    let rows = [
        "Enter send · Shift/Alt-Enter newline · \\ then Enter newline",
        "Esc stop the running turn · Esc Esc clear the input",
        "Tab cycle mode (manual, accept-edits, plan) · /mode for all",
        "Ctrl-O expand or fold results · PgUp/PgDn scroll · End jump down",
        "Ctrl-C leaves; the session keeps running",
        "/approve /deny /mode /read /interrupt /mouse /log /quit",
        "",
        "Messages arrive whole, not word by word.",
        "Text from the session is untrusted and shown as plain text.",
    ];
    rows.iter().map(|r| Line::from(sp(format!(" {r}"), if r.is_empty() { Style::default() } else { th.dim().patch(Style::default()) }))).collect()
}

// ---- the screen ----------------------------------------------------------------------------------------------

/// Rows of the header band that carries the logo image (the same three rows as the cloud drop-in's header).
pub const BAND_ROWS: u16 = 3;
/// The text of the band starts at this column; columns 0..BAND_TEXT_COL stay blank for the image (column 1, 6 cells wide).
pub const BAND_TEXT_COL: u16 = 8;
/// A terminal needs at least this many rows for the band (the screen's own minimum plus the band plus a usable transcript).
pub const BAND_MIN_H: u16 = MIN_H + BAND_ROWS + 4;

pub fn draw(f: &mut Frame, s: &State, th: &Theme) -> DrawInfo {
    draw_in(f, s, th, false)
}

/// [`draw`], with a three-row header band on top when `band` is set (and the terminal is tall enough): blank cells at the left
/// for the logo image the app writes after the frame, and `remote-sessions vX · inside a Remote Control session`, the session
/// and its connection to the right. Everything else moves down by the band; the click rows keep their screen-row indexing.
pub fn draw_in(f: &mut Frame, s: &State, th: &Theme, band: bool) -> DrawInfo {
    let area = f.area();
    if area.width < MIN_W || area.height < MIN_H {
        f.render_widget(Paragraph::new(Line::from(sp(truncate("terminal too small", area.width as usize), th.warn()))), area);
        return DrawInfo::default();
    }
    let (w, h) = (area.width, area.height);
    let top: usize = if band && h >= BAND_MIN_H { BAND_ROWS as usize } else { 0 };
    let avail = h as usize - top;
    let rows_in = input_rows(s, w).min(avail.saturating_sub(6).max(1));
    let tr_h = avail - (rows_in + 4);
    let rect = |y: usize, hh: usize| Rect { x: 0, y: (y + top) as u16, width: w, height: hh as u16 };
    if top > 0 {
        draw_band(f, s, th, w);
    }

    // transcript, bottom anchored
    let (lines, owners) = transcript(s, w, th);
    let total = lines.len();
    let scroll = s.scroll.min(total.saturating_sub(tr_h));
    let end = total - scroll;
    let start = end.saturating_sub(tr_h);
    let shown = &lines[start..end];
    let mut view: Vec<Line<'static>> = vec![Line::default(); tr_h - shown.len()];
    let mut rows: Vec<Option<usize>> = vec![None; tr_h - shown.len()];
    view.extend(shown.iter().cloned());
    rows.extend(owners[start..end].iter().copied());
    f.render_widget(Paragraph::new(view), rect(0, tr_h));

    // status strip
    f.render_widget(Paragraph::new(status_line(s, w, th)), rect(tr_h, 1));

    // input box
    let modal_open = !matches!(s.modal, Modal::None);
    let focused = !modal_open && !s.ended;
    let rule_style = if focused { th.accent() } else { th.dim() };
    let rule = Line::from(sp(th.rule_char().repeat(w as usize), rule_style));
    f.render_widget(Paragraph::new(rule.clone()), rect(tr_h + 1, 1));
    let (irows, (cr, cc)) = input_layout(&s.input, (w as usize).saturating_sub(2));
    let first = if irows.len() <= rows_in { 0 } else { cr.saturating_sub(rows_in - 1).min(irows.len() - rows_in) };
    let mut ilines: Vec<Line<'static>> = Vec::new();
    for (n, r) in irows.iter().skip(first).take(rows_in).enumerate() {
        let lead = if n == 0 && first == 0 { sp(format!("{} ", th.prompt_glyph()), rule_style) } else { sp("  ", Style::default()) };
        let body = if s.input.text.is_empty() && n == 0 {
            if s.ended { sp("(session ended)", th.dim()) } else if modal_open { sp("(input paused)", th.dim()) } else if !s.busy() { sp(truncate("message the session, or / for commands", (w as usize).saturating_sub(2)), th.dim()) } else { sp("", Style::default()) }
        } else {
            sp(r.clone(), if modal_open || s.ended { th.dim() } else { Style::default() })
        };
        ilines.push(Line::from(vec![lead, body]));
    }
    f.render_widget(Paragraph::new(ilines), rect(tr_h + 2, rows_in));
    f.render_widget(Paragraph::new(rule), rect(tr_h + 2 + rows_in, 1));
    f.render_widget(Paragraph::new(footer_line(s, w, th)), rect(tr_h + 3 + rows_in, 1));

    // overlays
    let over = rect(0, tr_h);
    if let Some(msg) = &s.fatal {
        let mut lines = vec![Line::default()];
        for l in wrap_plain(msg, (w as usize).saturating_sub(4)) {
            lines.push(Line::from(sp(format!(" {l}"), th.error())));
        }
        lines.push(Line::default());
        lines.push(Line::from(sp(" press any key to leave (exit code 2)", th.dim())));
        let r = modal_rect(over, lines.len(), w);
        f.render_widget(Clear, r);
        f.render_widget(Paragraph::new(lines).block(Frame_::new().borders(Borders::ALL).border_set(border_set(th)).border_style(th.error()).title(Span::styled(" error ", th.error()))), r);
    } else {
        match &s.modal {
            Modal::Permission { .. } => draw_permission(f, over, s, th),
            Modal::Help => draw_simple_modal(f, over, "help", help_lines(th), th),
            Modal::ModePicker { choice } => {
                let lines: Vec<Line<'static>> = MODES.iter().enumerate().map(|(i, m)| {
                    let on = i == *choice;
                    Line::from(sp(format!(" {} {}  {}", if on { th.prompt_glyph() } else { " " }, i + 1, mode_label(m)), if on { th.selected() } else { Style::default() }))
                }).collect();
                draw_simple_modal(f, over, "permission mode", lines, th);
            }
            Modal::None => {
                if focused && irows.len() > 0 {
                    let col = if s.input.text.is_empty() { 2 } else { 2 + cc };
                    let row = if s.input.text.is_empty() { 0 } else { cr.saturating_sub(first) };
                    f.set_cursor_position(Position { x: (col as u16).min(w - 1), y: (top + tr_h + 2 + row.min(rows_in - 1)) as u16 });
                }
            }
        }
    }
    th.fold_buffer(f.buffer_mut());
    // Click rows are indexed by screen row: the band's rows belong to no block.
    let mut all_rows: Vec<Option<usize>> = vec![None; top];
    all_rows.extend(rows);
    DrawInfo { total, height: tr_h, rows: all_rows }
}

/// The band's text, to the right of the logo's cells: the product line, the session, and how it is connected.
fn draw_band(f: &mut Frame, s: &State, th: &Theme, w: u16) {
    let room = w.saturating_sub(BAND_TEXT_COL) as usize;
    let mut at = |n: u16, line: Line<'static>| f.render_widget(Paragraph::new(line), Rect { x: BAND_TEXT_COL, y: n, width: w.saturating_sub(BAND_TEXT_COL), height: 1 });
    let dot = th.notice_glyph();
    at(0, Line::from(vec![
        sp("remote-sessions", th.bold()),
        sp(format!(" v{}  ", env!("CARGO_PKG_VERSION")), th.dim()),
        sp(truncate(&format!("{dot} inside a Remote Control session"), room.saturating_sub(24)), th.accent()),
    ]));
    let place = if s.host.is_empty() { clean(&s.name, 120) } else { format!("{}  {dot}  {}", clean(&s.name, 120), clean(&s.host, 60)) };
    at(1, Line::from(sp(truncate(&place, room), Style::default())));
    let conn = if s.ended {
        "ended"
    } else {
        match s.conn {
            Conn::Live => "live",
            Conn::Polling => "polling",
            Conn::Connecting => "connecting",
            Conn::Reconnecting(_) => "reconnecting",
            Conn::Offline => "offline",
        }
    };
    at(2, Line::from(sp(truncate(&format!("two-way  {dot}  {conn}"), room), th.dim())));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn input_layout_wraps_and_tracks_the_cursor() {
        let i = Input { text: "abcdef\ngh".into(), cursor: 8 };
        let (rows, cur) = input_layout(&i, 4);
        assert_eq!(rows, ["abcd", "ef", "gh"]);
        assert_eq!(cur, (2, 1));
        let (rows, cur) = input_layout(&Input::default(), 10);
        assert_eq!((rows, cur), (vec![String::new()], (0, 0)));
    }
    #[test]
    fn tool_args_per_tool() {
        assert_eq!(tool_args("Bash", &serde_json::json!({"command": "ls   -la"}), 40), "ls -la");
        assert_eq!(tool_args("Read", &serde_json::json!({"file_path": "/a/b.rs"}), 40), "/a/b.rs");
        assert_eq!(tool_args("Thing", &serde_json::json!({"a": "x", "n": 3, "o": {}}), 40), "a=x, n=3");
    }
}
