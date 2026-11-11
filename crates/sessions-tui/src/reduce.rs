//! The reducer: `reduce(State, Msg) -> (State, Vec<Effect>)`. Pure; time comes in as `Msg::Tick`, randomness from
//! `State::seed`. Every effect (HTTP call, thread start) is returned for the app to run.
use crate::keys::{map_key, Action, Edit};
use crate::model::*;
use crate::text::{clean, fmt_dur, one_line};

const TEXT_CAP: usize = 8 * 1024;
/// Slash commands, for Tab completion.
pub const COMMANDS: [&str; 10] = ["/approve", "/cast", "/deny", "/help", "/interrupt", "/log", "/mode", "/mouse", "/quit", "/read"];
/// `system` subtypes that fire constantly; shown only after `/log`.
const NOISY_SYSTEM: [&str; 7] = ["background_tasks_changed", "task_summary", "thinking_tokens", "task_updated", "post_turn_summary", "task_started", "task_notification"];
const GONE: [&str; 3] = ["http 404", "http 410", "not found"];

pub fn reduce(mut s: State, msg: Msg) -> (State, Vec<Effect>) {
    let mut fx: Vec<Effect> = Vec::new();
    match msg {
        Msg::Tick(t) => s.now_ms = t,
        Msg::Resize(w, h) => s.size = (w, h),
        Msg::ClickAt(..) => {}
        Msg::Key(k) => {
            let a = map_key(&s, k);
            act(&mut s, a, &mut fx);
        }
        Msg::Paste(text) => {
            if matches!(s.modal, Modal::None) && !s.ended && s.fatal.is_none() {
                let t = clean(&text.replace("\r\n", "\n").replace('\r', "\n"), 64 * 1024);
                insert(&mut s.input, &t);
                s.complete_base = None;
            }
        }
        Msg::Wheel(n) => scroll_by(&mut s, n as i64 * 3),
        Msg::Click(id) => {
            if !s.expanded.remove(&id) {
                s.expanded.insert(id);
            }
        }
        Msg::Snapshot(Ok(snap)) => {
            if !snap.name.is_empty() {
                s.name = snap.name;
            }
            if !snap.host.is_empty() {
                s.host = snap.host;
            }
            if snap.truncated {
                notice(&mut s, "earlier entries are not shown (the newest were loaded)", Level::Dim);
            }
            for ev in snap.events {
                apply_ev(&mut s, ev, false);
            }
            s.scroll = 0;
            s.new_count = 0;
            fx.push(Effect::StartStream { from: s.last_seq });
            for (uuid, text) in std::mem::take(&mut s.queued) {
                fx.push(Effect::Say { uuid, text });
            }
        }
        Msg::Snapshot(Err(e)) => s.fatal = Some(clean(&e, 600)),
        Msg::Open => {
            s.conn = Conn::Live;
            if s.poll_active {
                s.poll_active = false;
                fx.push(Effect::StopPoll);
            }
        }
        Msg::Item(Item::Event(ev)) => apply_ev(&mut s, ev, true),
        Msg::Item(Item::Receipt { event_id, status }) => receipt(&mut s, &event_id, &status),
        Msg::StreamEnded(StreamEnd::Failed(why)) => {
            if s.conn != Conn::Polling {
                notice(&mut s, &format!("live stream unavailable ({}); polling", one_line(&why, 160)), Level::Dim);
            }
            s.conn = Conn::Polling;
            if !s.poll_active {
                s.poll_active = true;
                fx.push(Effect::StartPoll);
            }
        }
        Msg::StreamEnded(StreamEnd::Stopped) => {}
        Msg::Retry => {
            if s.conn == Conn::Polling {
                fx.push(Effect::StartStream { from: s.last_seq });
            }
        }
        Msg::PollFailed(e) => {
            let low = e.to_ascii_lowercase();
            if GONE.iter().any(|g| low.contains(g)) {
                s.ended = true;
                s.conn = Conn::Offline;
                s.turn = Turn::Idle;
                if s.poll_active {
                    s.poll_active = false;
                    fx.push(Effect::StopPoll);
                }
            } else {
                let text = format!("poll failed (will retry): {}", one_line(&e, 200));
                let same = matches!(s.blocks.last(), Some(Block { kind: BlockKind::Notice { text: t, .. }, .. }) if *t == text);
                if !same {
                    notice(&mut s, &text, Level::Warn);
                }
            }
        }
        Msg::Net(n) => net(&mut s, n),
    }
    clamp_scroll(&mut s);
    if s.quit {
        fx.push(Effect::Quit);
    }
    (s, fx)
}

fn clamp_scroll(s: &mut State) {
    let max = s.view_total.saturating_sub(s.view_h);
    if s.scroll > max && s.view_total > 0 {
        s.scroll = max;
    }
    if s.scroll == 0 {
        s.new_count = 0;
    }
}

fn scroll_by(s: &mut State, delta: i64) {
    let max = s.view_total.saturating_sub(s.view_h) as i64;
    s.scroll = (s.scroll as i64 + delta).clamp(0, max.max(0)) as usize;
    if s.scroll == 0 {
        s.new_count = 0;
    }
}

fn add_block(s: &mut State, key: &str, kind: BlockKind) {
    let id = s.next_id;
    s.next_id += 1;
    s.blocks.push(Block { id, key: key.to_string(), kind });
    if s.scroll > 0 {
        s.new_count += 1;
    }
}

pub fn notice(s: &mut State, text: &str, level: Level) {
    add_block(s, "", BlockKind::Notice { text: clean(text, TEXT_CAP), level });
}

fn busy_from(s: &State) -> u64 {
    match s.turn {
        Turn::Waiting { since } | Turn::Working { since, .. } => since,
        Turn::Idle => s.now_ms,
    }
}

fn working(s: &mut State, tool: Option<String>) {
    let since = busy_from(s);
    let tool = tool.or_else(|| if let Turn::Working { tool, .. } = &s.turn { tool.clone() } else { None });
    s.turn = Turn::Working { since, tool };
}

fn receipt(s: &mut State, event_id: &str, status: &str) {
    for b in s.blocks.iter_mut() {
        if b.key == event_id {
            if let BlockKind::User { mark, .. } = &mut b.kind {
                match status {
                    "PROCESSED" => *mark = Mark::Processed,
                    "RECEIVED" | "PROCESSING" if *mark != Mark::Processed => *mark = Mark::Sent,
                    _ => {}
                }
            }
        }
    }
    if status == "PROCESSING" && s.turn == Turn::Idle && s.blocks.iter().any(|b| b.key == event_id) {
        s.turn = Turn::Waiting { since: s.now_ms };
    }
}

fn close_prompt(s: &mut State, request_id: &str) {
    let Some(pos) = s.prompts.iter().position(|p| p.request_id == request_id) else { return };
    s.prompts.remove(pos);
    let ours = s.answering.as_deref() == Some(request_id);
    if ours {
        s.answering = None;
    } else {
        notice(s, "answered elsewhere", Level::Dim);
    }
    if s.prompts.is_empty() {
        if matches!(s.modal, Modal::Permission { .. }) {
            s.modal = Modal::None;
        }
    } else if pos == 0 && matches!(s.modal, Modal::Permission { .. }) {
        s.modal = Modal::Permission { choice: 0, reason: None, sending: false, full: false };
    }
}

fn apply_ev(s: &mut State, ev: Ev, live: bool) {
    if !ev.id.is_empty() && s.seen.contains(&ev.id) {
        return;
    }
    if let Some(q) = ev.seq {
        s.last_seq = Some(s.last_seq.map_or(q, |l| l.max(q)));
    }
    if !ev.id.is_empty() {
        s.seen.insert(ev.id.clone());
    }
    s.last_event_ms = s.now_ms;
    let has_result = ev.blocks.iter().any(|b| matches!(b, Blk::ToolResult { .. }));
    match ev.kind.as_str() {
        "user" => {
            let texts: Vec<&str> = ev.blocks.iter().filter_map(|b| if let Blk::Text(t) = b { Some(t.as_str()) } else { None }).collect();
            if !texts.is_empty() && !has_result {
                let text = clean(&texts.join("\n"), TEXT_CAP);
                user_turn(s, &ev.id, text);
                if live && s.turn == Turn::Idle {
                    s.turn = Turn::Waiting { since: s.now_ms };
                }
            }
            for b in &ev.blocks {
                if let Blk::ToolResult { tool_use_id, text, is_error } = b {
                    let out = ToolOut { text: clean(text, TEXT_CAP), is_error: *is_error };
                    let slot = s.blocks.iter_mut().find_map(|bl| match &mut bl.kind {
                        BlockKind::Tool { tool_id, result, .. } if tool_id == tool_use_id && result.is_none() => Some(result),
                        _ => None,
                    });
                    match slot {
                        Some(r) => *r = Some(out),
                        None => add_block(s, &ev.id, BlockKind::Result(out)),
                    }
                    if live {
                        working(s, None);
                    }
                }
            }
        }
        "assistant" => {
            let mut tool = None;
            for b in &ev.blocks {
                match b {
                    Blk::Text(t) => {
                        let t = clean(t, TEXT_CAP);
                        if !t.trim().is_empty() {
                            add_block(s, &ev.id, BlockKind::Assistant(t));
                        }
                    }
                    Blk::ToolUse { id, name, input } => {
                        tool = Some(clean(name, 60));
                        add_block(s, &ev.id, BlockKind::Tool { tool_id: id.clone(), name: clean(name, 60), input: input.clone(), result: None });
                    }
                    _ => {}
                }
            }
            if live {
                working(s, tool);
            }
        }
        "result" => {
            let info = ev.result.clone().unwrap_or_default();
            let mut parts = vec!["turn done".to_string()];
            if let Some(ms) = info.duration_ms {
                parts.push(fmt_dur(ms / 1000));
            }
            if let Some(n) = info.turns {
                parts.push(format!("{n} turn{}", if n == 1 { "" } else { "s" }));
            }
            if info.subtype.starts_with("error") {
                notice(s, &format!("turn ended with {}", clean(&info.subtype, 60)), Level::Warn);
            }
            add_block(s, &ev.id, BlockKind::Divider(parts.join(" · ")));
            s.turn = Turn::Idle;
        }
        "system" => {
            if let Some(init) = &ev.init {
                if let Some(m) = &init.mode {
                    s.mode = Some(m.clone());
                }
                let model = init.model.as_deref().map(|m| clean(m, 80)).unwrap_or_default();
                let text = format!("session started{}", if model.is_empty() { String::new() } else { format!(" · {model}") });
                // Every turn announces itself again; say it once per distinct text.
                if s.last_init.as_deref() != Some(text.as_str()) {
                    notice(s, &text, Level::Dim);
                    s.last_init = Some(text);
                }
            } else if let Some(n) = &ev.note {
                let n = n.trim_start_matches("· ");
                let sub = n.trim_start_matches("system ").split(|c: char| c == ':' || c == ' ').next().unwrap_or("");
                let noisy = NOISY_SYSTEM.contains(&sub);
                let same = matches!(s.blocks.last(), Some(Block { kind: BlockKind::Notice { text, .. }, .. }) if text == n);
                if (!noisy || s.show_log) && !same {
                    notice(s, n, Level::Dim);
                }
            }
        }
        "control_request" => {
            for b in &ev.blocks {
                if let Blk::Request { request_id, subtype, tool_name, input, mode } = b {
                    match subtype.as_str() {
                        "can_use_tool" => {
                            if !request_id.is_empty() && !s.prompts.iter().any(|p| &p.request_id == request_id) {
                                s.prompts.push(Prompt { request_id: request_id.clone(), tool_name: clean(tool_name, 80), input: input.clone() });
                                if !matches!(s.modal, Modal::Permission { .. }) {
                                    s.modal = Modal::Permission { choice: 0, reason: None, sending: false, full: false };
                                }
                            }
                        }
                        "set_permission_mode" => {
                            if let Some(m) = mode {
                                s.mode = Some(m.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        "control_response" | "control_cancel_request" => {
            for b in &ev.blocks {
                if let Blk::Response { request_id } | Blk::Cancel { request_id } = b {
                    close_prompt(s, request_id);
                }
            }
        }
        "rate_limit_event" => {
            let n = ev.note.clone().unwrap_or_else(|| "rate_limit".into());
            if s.last_rate.as_deref() != Some(n.as_str()) {
                notice(s, n.trim_start_matches("· "), Level::Dim);
                s.last_rate = Some(n);
            }
        }
        "autocompact_state" | "keep_alive" => {}
        "env_manager_log" => {
            if s.show_log {
                if let Some(n) = &ev.note {
                    notice(s, n.trim_start_matches("· "), Level::Dim);
                }
            }
        }
        other => {
            let t = ev.note.clone().map(|n| n.trim_start_matches("· ").to_string()).unwrap_or_else(|| other.to_string());
            notice(s, &t, Level::Dim);
        }
    }
}

/// A user turn from the server: replaces our own optimistic copy (by uuid, else by text) or becomes a new block.
fn user_turn(s: &mut State, id: &str, text: String) {
    let by_key = s.blocks.iter().position(|b| !id.is_empty() && b.key == id && matches!(b.kind, BlockKind::User { .. }));
    let by_text = || {
        s.blocks.iter().position(|b| match &b.kind {
            BlockKind::User { text: t, mark: Mark::Local | Mark::Sent } => t.trim() == text.trim() && !s.seen.contains(&b.key) && !b.key.is_empty(),
            _ => false,
        })
    };
    if let Some(i) = by_key.or_else(by_text) {
        let b = &mut s.blocks[i];
        b.key = id.to_string();
        if let BlockKind::User { text: t, mark } = &mut b.kind {
            *t = text;
            if *mark == Mark::Local {
                *mark = Mark::Sent;
            }
        }
    } else {
        add_block(s, id, BlockKind::User { text, mark: Mark::Local });
    }
}

fn net(s: &mut State, n: Net) {
    match n {
        Net::Say { uuid, res } => {
            let b = s.blocks.iter_mut().find(|b| b.key == uuid);
            match res {
                Ok(()) => {
                    if let Some(Block { kind: BlockKind::User { mark, .. }, .. }) = b {
                        if *mark == Mark::Local {
                            *mark = Mark::Sent;
                        }
                    }
                    if s.turn == Turn::Idle {
                        s.turn = Turn::Waiting { since: s.now_ms };
                    }
                }
                Err(e) => {
                    if let Some(Block { kind: BlockKind::User { mark, .. }, .. }) = b {
                        *mark = Mark::Failed;
                    }
                    notice(s, &format!("send failed: {}", one_line(&e, 300)), Level::Error);
                }
            }
        }
        Net::Interrupt(Ok(())) => {
            notice(s, "interrupted by you", Level::Warn);
            s.turn = Turn::Idle;
        }
        Net::Interrupt(Err(e)) => notice(s, &format!("interrupt failed: {}", one_line(&e, 300)), Level::Error),
        Net::Mode { mode, res: Ok(()) } => {
            notice(s, &format!("mode: {}", mode_label(&mode)), Level::Dim);
            s.mode = Some(mode);
        }
        Net::Mode { res: Err(e), .. } => notice(s, &format!("mode change failed: {}", one_line(&e, 300)), Level::Error),
        Net::Answer { request_id, allow, res } => {
            match res {
                Ok(()) => {
                    notice(s, if allow { "allowed once" } else { "denied" }, Level::Dim);
                }
                Err(e) => notice(s, &format!("could not answer: {}", one_line(&e, 300)), Level::Error),
            }
            // Either way the prompt is done with: answered, or no longer pending.
            if let Some(pos) = s.prompts.iter().position(|p| p.request_id == request_id) {
                s.prompts.remove(pos);
            }
            if s.answering.as_deref() == Some(request_id.as_str()) {
                s.answering = None;
            }
            if s.prompts.is_empty() {
                if matches!(s.modal, Modal::Permission { .. }) {
                    s.modal = Modal::None;
                }
            } else if matches!(s.modal, Modal::Permission { .. }) {
                s.modal = Modal::Permission { choice: 0, reason: None, sending: false, full: false };
            }
        }
    }
}

// ---- input editing -------------------------------------------------------------------------------------------

fn chars_of(i: &Input) -> Vec<char> {
    i.text.chars().collect()
}

fn set_chars(i: &mut Input, cs: Vec<char>, cursor: usize) {
    i.text = cs.into_iter().collect();
    i.cursor = cursor;
}

pub fn insert(i: &mut Input, t: &str) {
    let mut cs = chars_of(i);
    let at = i.cursor.min(cs.len());
    let add: Vec<char> = t.chars().collect();
    let n = add.len();
    cs.splice(at..at, add);
    set_chars(i, cs, at + n);
}

fn line_start(cs: &[char], cur: usize) -> usize {
    cs[..cur].iter().rposition(|c| *c == '\n').map_or(0, |p| p + 1)
}

fn line_end(cs: &[char], cur: usize) -> usize {
    cs[cur..].iter().position(|c| *c == '\n').map_or(cs.len(), |p| cur + p)
}

fn word_left(cs: &[char], cur: usize) -> usize {
    let mut p = cur;
    while p > 0 && cs[p - 1].is_whitespace() {
        p -= 1;
    }
    while p > 0 && !cs[p - 1].is_whitespace() {
        p -= 1;
    }
    p
}

fn word_right(cs: &[char], cur: usize) -> usize {
    let mut p = cur;
    while p < cs.len() && cs[p].is_whitespace() {
        p += 1;
    }
    while p < cs.len() && !cs[p].is_whitespace() {
        p += 1;
    }
    p
}

pub fn edit(i: &mut Input, e: Edit) {
    let mut cs = chars_of(i);
    let cur = i.cursor.min(cs.len());
    match e {
        Edit::Char(c) => {
            cs.insert(cur, c);
            set_chars(i, cs, cur + 1);
        }
        Edit::Backspace => {
            if cur > 0 {
                cs.remove(cur - 1);
                set_chars(i, cs, cur - 1);
            }
        }
        Edit::Delete => {
            if cur < cs.len() {
                cs.remove(cur);
                set_chars(i, cs, cur);
            }
        }
        Edit::Left => i.cursor = cur.saturating_sub(1),
        Edit::Right => i.cursor = (cur + 1).min(cs.len()),
        Edit::Home => i.cursor = line_start(&cs, cur),
        Edit::End => i.cursor = line_end(&cs, cur),
        Edit::DelToStart => {
            let st = line_start(&cs, cur);
            cs.drain(st..cur);
            set_chars(i, cs, st);
        }
        Edit::DelToEnd => {
            let en = line_end(&cs, cur);
            let en = if en == cur && cur < cs.len() { cur + 1 } else { en };
            cs.drain(cur..en);
            set_chars(i, cs, cur);
        }
        Edit::DelWord => {
            let st = word_left(&cs, cur);
            cs.drain(st..cur);
            set_chars(i, cs, st);
        }
        Edit::WordLeft => i.cursor = word_left(&cs, cur),
        Edit::WordRight => i.cursor = word_right(&cs, cur),
    }
}

/// (line index, column) of the cursor, counting `\n` lines.
pub fn cursor_line_col(i: &Input) -> (usize, usize) {
    let cs = chars_of(i);
    let cur = i.cursor.min(cs.len());
    let line = cs[..cur].iter().filter(|c| **c == '\n').count();
    (line, cur - line_start(&cs, cur))
}

fn move_line(i: &mut Input, down: bool) -> bool {
    let cs = chars_of(i);
    let cur = i.cursor.min(cs.len());
    let (line, col) = cursor_line_col(i);
    let total = cs.iter().filter(|c| **c == '\n').count();
    if (!down && line == 0) || (down && line >= total) {
        return false;
    }
    let target = if down { line + 1 } else { line - 1 };
    let mut start = 0;
    for _ in 0..target {
        start = line_end(&cs, start) + 1;
    }
    let end = line_end(&cs, start);
    let _ = cur;
    i.cursor = start + col.min(end - start);
    true
}

// ---- actions -------------------------------------------------------------------------------------------------

fn history_prev(s: &mut State) {
    if s.history.is_empty() {
        return;
    }
    let pos = match s.hist_pos {
        None => {
            s.hist_query = Some(s.input.text.clone());
            s.history.len() - 1
        }
        Some(p) => p.saturating_sub(1),
    };
    s.hist_pos = Some(pos);
    let t = s.history[pos].clone();
    s.input = Input { cursor: t.chars().count(), text: t };
}

fn history_next(s: &mut State) {
    let Some(p) = s.hist_pos else { return };
    if p + 1 < s.history.len() {
        s.hist_pos = Some(p + 1);
        let t = s.history[p + 1].clone();
        s.input = Input { cursor: t.chars().count(), text: t };
    } else {
        s.hist_pos = None;
        let t = s.hist_query.take().unwrap_or_default();
        s.input = Input { cursor: t.chars().count(), text: t };
    }
}

fn history_search(s: &mut State) {
    if s.history.is_empty() {
        return;
    }
    if s.hist_pos.is_none() {
        s.hist_query = Some(s.input.text.clone());
    }
    let q = s.hist_query.clone().unwrap_or_default();
    let from = s.hist_pos.unwrap_or(s.history.len());
    if let Some(p) = (0..from).rev().find(|p| s.history[*p].contains(&q)) {
        s.hist_pos = Some(p);
        let t = s.history[p].clone();
        s.input = Input { cursor: t.chars().count(), text: t };
    }
}

fn complete(s: &mut State, back: bool) {
    let base = s.complete_base.clone().unwrap_or_else(|| s.input.text.clone());
    if !base.starts_with('/') || base.contains(char::is_whitespace) {
        return;
    }
    let cands: Vec<&str> = COMMANDS.iter().copied().filter(|c| c.starts_with(&base)).collect();
    if cands.is_empty() {
        return;
    }
    if s.complete_base.is_none() {
        s.complete_base = Some(base);
        s.complete_idx = if back { cands.len() - 1 } else { 0 };
    } else if back {
        s.complete_idx = (s.complete_idx + cands.len() - 1) % cands.len();
    } else {
        s.complete_idx = (s.complete_idx + 1) % cands.len();
    }
    let pick = cands[s.complete_idx % cands.len()];
    let t = if cands.len() == 1 { format!("{pick} ") } else { pick.to_string() };
    s.input = Input { cursor: t.chars().count(), text: t };
}

fn act(s: &mut State, a: Action, fx: &mut Vec<Effect>) {
    if !matches!(a, Action::Complete | Action::CompleteBack | Action::None) {
        s.complete_base = None;
    }
    match a {
        Action::None => {}
        Action::Leave => s.quit = true,
        Action::Redraw => fx.push(Effect::Redraw),
        Action::Send => send(s, fx),
        Action::Newline => {
            if !s.ended {
                insert(&mut s.input, "\n");
            }
        }
        Action::Interrupt => fx.push(Effect::Interrupt),
        Action::EscIdle => {
            let double = s.last_esc_ms.is_some_and(|t| s.now_ms.saturating_sub(t) < 800);
            if double && !s.input.text.is_empty() {
                s.input = Input::default();
                s.last_esc_ms = None;
            } else {
                s.last_esc_ms = Some(s.now_ms);
            }
        }
        Action::CycleMode => {
            if !s.ended {
                fx.push(Effect::SetMode(next_mode(s.mode.as_deref()).to_string()));
            }
        }
        Action::ToggleExpand => s.expand_all = !s.expand_all,
        Action::PageUp => scroll_by(s, s.view_h.saturating_sub(1).max(1) as i64),
        Action::PageDown => scroll_by(s, -(s.view_h.saturating_sub(1).max(1) as i64)),
        Action::HalfUp => scroll_by(s, (s.view_h / 2).max(1) as i64),
        Action::HalfDown => scroll_by(s, -((s.view_h / 2).max(1) as i64)),
        Action::Top => scroll_by(s, i64::MAX / 4),
        Action::Bottom => {
            s.scroll = 0;
            s.new_count = 0;
        }
        Action::Help => s.modal = Modal::Help,
        Action::HistorySearch => history_search(s),
        Action::Up => {
            if !move_line(&mut s.input, false) {
                history_prev(s);
            }
        }
        Action::Down => {
            if !move_line(&mut s.input, true) {
                history_next(s);
            }
        }
        Action::Complete => complete(s, false),
        Action::CompleteBack => complete(s, true),
        Action::Edit(e) => {
            if !s.ended {
                edit(&mut s.input, e);
                s.hist_pos = None;
            }
        }
        Action::CloseModal => s.modal = Modal::None,
        Action::ModalDigit(n) => match &mut s.modal {
            Modal::Permission { choice, reason, .. } => {
                *choice = (n as usize - 1).min(1);
                if n == 3 {
                    *reason = Some(String::new());
                }
            }
            Modal::ModePicker { choice } => *choice = (n as usize - 1).min(MODES.len() - 1),
            _ => {}
        },
        Action::ModalUp => match &mut s.modal {
            Modal::Permission { choice, .. } | Modal::ModePicker { choice } => *choice = choice.saturating_sub(1),
            _ => {}
        },
        Action::ModalDown => {
            let max = if matches!(s.modal, Modal::ModePicker { .. }) { MODES.len() - 1 } else { 1 };
            if let Modal::Permission { choice, .. } | Modal::ModePicker { choice } = &mut s.modal {
                *choice = (*choice + 1).min(max);
            }
        }
        Action::ModalReason => {
            if let Modal::Permission { choice, reason, .. } = &mut s.modal {
                *choice = 2;
                *reason = Some(String::new());
            }
        }
        Action::ModalCancelField => {
            if let Modal::Permission { reason, .. } = &mut s.modal {
                *reason = None;
            }
        }
        Action::ModalChar(c) => {
            if let Modal::Permission { reason: Some(r), .. } = &mut s.modal {
                if !c.is_control() && r.chars().count() < 300 {
                    r.push(c);
                }
            }
        }
        Action::ModalBackspace => {
            if let Modal::Permission { reason: Some(r), .. } = &mut s.modal {
                r.pop();
            }
        }
        Action::ModalFull => {
            if let Modal::Permission { full, .. } = &mut s.modal {
                *full = !*full;
            }
        }
        Action::ModalDeny => answer_current(s, Decision::Deny(None), fx),
        Action::ModalConfirm => match s.modal.clone() {
            Modal::Permission { reason: Some(r), .. } => {
                let r = r.trim().to_string();
                answer_current(s, Decision::Deny(if r.is_empty() { None } else { Some(r) }), fx)
            }
            Modal::Permission { choice: 0, .. } => answer_current(s, Decision::Allow, fx),
            Modal::Permission { choice: 1, .. } => answer_current(s, Decision::Deny(None), fx),
            Modal::Permission { .. } => {
                if let Modal::Permission { reason, .. } = &mut s.modal {
                    *reason = Some(String::new());
                }
            }
            Modal::ModePicker { choice } => {
                s.modal = Modal::None;
                fx.push(Effect::SetMode(MODES[choice.min(MODES.len() - 1)].to_string()));
            }
            _ => {}
        },
    }
}

fn answer_current(s: &mut State, decision: Decision, fx: &mut Vec<Effect>) {
    let Some(p) = s.prompts.first() else { return };
    let id = p.request_id.clone();
    if let Modal::Permission { sending, reason, .. } = &mut s.modal {
        *sending = true;
        *reason = None;
    }
    s.answering = Some(id.clone());
    fx.push(Effect::Answer { request_id: id, decision });
}

fn send(s: &mut State, fx: &mut Vec<Effect>) {
    if s.ended {
        return;
    }
    if s.input.text.ends_with('\\') && !s.input.text.ends_with("\\\\") {
        let mut cs = chars_of(&s.input);
        cs.pop();
        cs.push('\n');
        let n = cs.len();
        set_chars(&mut s.input, cs, n);
        return;
    }
    let raw = std::mem::take(&mut s.input);
    s.hist_pos = None;
    s.hist_query = None;
    let text = raw.text.trim_end().to_string();
    if text.trim().is_empty() {
        return;
    }
    if !text.starts_with(' ') {
        s.history.push(text.clone());
        fx.push(Effect::SaveHistory(text.clone()));
    }
    let text = text.trim_start().to_string();
    if let Some(literal) = text.strip_prefix("//") {
        say(s, format!("/{literal}"), fx);
    } else if text.starts_with('/') {
        command(s, &text, fx);
    } else {
        say(s, text, fx);
    }
}

fn say(s: &mut State, text: String, fx: &mut Vec<Effect>) {
    let uuid = s.new_uuid();
    add_block(s, &uuid, BlockKind::User { text: clean(&text, TEXT_CAP), mark: Mark::Local });
    s.scroll = 0;
    s.new_count = 0;
    if s.conn == Conn::Connecting {
        s.queued.push((uuid, text));
    } else {
        fx.push(Effect::Say { uuid, text });
    }
}

fn command(s: &mut State, line: &str, fx: &mut Vec<Effect>) {
    let (cmd, arg) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    match cmd {
        "/quit" => s.quit = true,
        "/interrupt" => fx.push(Effect::Interrupt),
        "/help" => s.modal = Modal::Help,
        "/log" => {
            s.show_log = !s.show_log;
            let t = if s.show_log { "provisioning log: on" } else { "provisioning log: off" };
            notice(s, t, Level::Dim);
        }
        "/mouse" => {
            let on = !matches!(arg, "off" | "0");
            s.mouse = on;
            fx.push(Effect::Mouse(on));
            notice(s, if on { "mouse capture on" } else { "mouse capture off (the terminal can select text)" }, Level::Dim);
        }
        "/cast" => notice(s, "in-app recording is not part of this build; record the pane from outside", Level::Dim),
        "/mode" => {
            if arg.is_empty() {
                let cur = s.mode.as_deref().and_then(|m| MODES.iter().position(|x| *x == m)).unwrap_or(0);
                s.modal = Modal::ModePicker { choice: cur };
            } else if let Some(m) = parse_mode(arg) {
                fx.push(Effect::SetMode(m.to_string()));
            } else {
                notice(s, &format!("unknown mode {}: use one of {}", one_line(arg, 40), MODES.join(", ")), Level::Error);
            }
        }
        "/approve" | "/deny" => {
            let approve = cmd == "/approve";
            let target = if approve && !arg.is_empty() { s.prompts.iter().find(|p| p.request_id == arg) } else { s.prompts.last() };
            match target.map(|p| p.request_id.clone()) {
                None => notice(s, "no pending permission prompt", Level::Error),
                Some(id) => {
                    let decision = if approve { Decision::Allow } else { Decision::Deny(if arg.is_empty() { None } else { Some(arg.to_string()) }) };
                    s.answering = Some(id.clone());
                    fx.push(Effect::Answer { request_id: id, decision });
                }
            }
        }
        "/read" => {
            let n = arg.parse::<usize>().unwrap_or(10).clamp(1, 200);
            let from = s.blocks.len().saturating_sub(n);
            let text = s.blocks[from..].iter().map(block_plain).collect::<Vec<_>>().join("\n");
            notice(s, &format!("last {} entries\n{text}", s.blocks.len() - from), Level::Dim);
        }
        other => notice(s, &format!("unknown command {} (try /help)", one_line(other, 40)), Level::Error),
    }
}

/// One block as plain text (for `/read`).
pub fn block_plain(b: &Block) -> String {
    match &b.kind {
        BlockKind::User { text, .. } => format!("user> {text}"),
        BlockKind::Assistant(t) => format!("assistant> {t}"),
        BlockKind::Tool { name, input, result, .. } => {
            let r = result.as_ref().map(|r| format!("\n  -> {}", one_line(&r.text, 160))).unwrap_or_default();
            format!("tool {name} {}{r}", crate::view::tool_args(name, input, 160))
        }
        BlockKind::Result(r) => format!("result: {}", one_line(&r.text, 160)),
        BlockKind::Notice { text, .. } => format!("- {text}"),
        BlockKind::Divider(t) => format!("-- {t}"),
    }
}

