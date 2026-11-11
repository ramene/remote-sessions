#![allow(dead_code)]
//! Shared test helpers: event builders (fake ids only), a `TestBackend` text dump, and golden-file snapshots.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use serde_json::{json, Value};
use sessions_tui::model::*;
use sessions_tui::reduce::reduce;
use sessions_tui::theme::Theme;
use sessions_tui::view;
use unicode_width::UnicodeWidthStr;

pub const SESSION: &str = "session_01ExampleAAAAAAAAAAAAAAA";

// ---- events ----------------------------------------------------------------------------------------------------

pub fn user(seq: u64, id: &str, text: &str) -> Ev {
    Ev::new(Some(seq), id, "user", vec![Blk::Text(text.into())])
}
pub fn assistant(seq: u64, id: &str, text: &str) -> Ev {
    Ev::new(Some(seq), id, "assistant", vec![Blk::Text(text.into())])
}
pub fn tool_use(seq: u64, id: &str, tool_id: &str, name: &str, input: Value) -> Ev {
    Ev::new(Some(seq), id, "assistant", vec![Blk::ToolUse { id: tool_id.into(), name: name.into(), input }])
}
pub fn tool_result(seq: u64, id: &str, tool_id: &str, text: &str, is_error: bool) -> Ev {
    Ev::new(Some(seq), id, "user", vec![Blk::ToolResult { tool_use_id: tool_id.into(), text: text.into(), is_error }])
}
pub fn thinking(seq: u64, id: &str) -> Ev {
    Ev::new(Some(seq), id, "assistant", vec![Blk::Thinking])
}
pub fn ask(seq: u64, id: &str, req: &str, tool: &str, input: Value) -> Ev {
    Ev::new(Some(seq), id, "control_request", vec![Blk::Request { request_id: req.into(), subtype: "can_use_tool".into(), tool_name: tool.into(), input, mode: None }])
}
pub fn mode_req(seq: u64, id: &str, mode: &str) -> Ev {
    Ev::new(Some(seq), id, "control_request", vec![Blk::Request { request_id: format!("req_{id}"), subtype: "set_permission_mode".into(), tool_name: String::new(), input: Value::Null, mode: Some(mode.into()) }])
}
pub fn response(seq: u64, id: &str, req: &str) -> Ev {
    Ev::new(Some(seq), id, "control_response", vec![Blk::Response { request_id: req.into() }])
}
pub fn cancel(seq: u64, id: &str, req: &str) -> Ev {
    Ev::new(Some(seq), id, "control_cancel_request", vec![Blk::Cancel { request_id: req.into() }])
}
pub fn result(seq: u64, id: &str, subtype: &str, turns: u64, ms: u64) -> Ev {
    let mut e = Ev::new(Some(seq), id, "result", vec![]);
    e.result = Some(ResultInfo { subtype: subtype.into(), turns: Some(turns), duration_ms: Some(ms) });
    e
}
pub fn init(seq: u64, id: &str, mode: &str) -> Ev {
    let mut e = Ev::new(Some(seq), id, "system", vec![]);
    e.init = Some(InitInfo { model: Some("model-x".into()), mode: Some(mode.into()) });
    e
}
pub fn note(seq: u64, id: &str, kind: &str, text: &str) -> Ev {
    let mut e = Ev::new(Some(seq), id, kind, vec![]);
    e.note = Some(text.into());
    e
}

// ---- state driving -----------------------------------------------------------------------------------------------

pub fn state() -> State {
    State::new("session_01Ex…", "mac-a")
}

/// A live, loaded session (the snapshot arrived and the stream opened).
pub fn live(events: Vec<Ev>) -> State {
    let mut s = state();
    s = step(s, Msg::Snapshot(Ok(Snapshot { name: "session_01Ex…".into(), host: "mac-a".into(), events, truncated: false }))).0;
    step(s, Msg::Open).0
}

pub fn step(s: State, m: Msg) -> (State, Vec<Effect>) {
    reduce(s, m)
}

pub fn feed(mut s: State, evs: Vec<Ev>) -> State {
    for e in evs {
        s = step(s, Msg::Item(Item::Event(e))).0;
    }
    s
}

pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}
pub fn ctrl(c: char) -> KeyEvent {
    KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

/// Press keys in order, collecting effects.
pub fn press(mut s: State, keys: Vec<KeyEvent>) -> (State, Vec<Effect>) {
    let mut all = Vec::new();
    for k in keys {
        let (n, fx) = reduce(s, Msg::Key(k));
        s = n;
        all.extend(fx);
    }
    (s, all)
}

pub fn type_text(s: State, text: &str) -> State {
    press(s, text.chars().map(|c| key(KeyCode::Char(c))).collect()).0
}

pub fn submit(s: State, text: &str) -> (State, Vec<Effect>) {
    let s = type_text(s, text);
    press(s, vec![key(KeyCode::Enter)])
}

// ---- rendering ---------------------------------------------------------------------------------------------------

/// The buffer as text: one row per line, trailing spaces trimmed, wide glyphs counted by their width.
pub fn buffer_text(buf: &Buffer) -> String {
    let mut rows = Vec::new();
    for y in 0..buf.area.height {
        let mut row = String::new();
        let mut x = 0;
        while x < buf.area.width {
            let sym = buf[(x, y)].symbol().to_string();
            x += UnicodeWidthStr::width(sym.as_str()).max(1) as u16;
            row.push_str(&sym);
        }
        rows.push(row.trim_end().to_string());
    }
    rows.join("\n")
}

/// Lines drawn through a `TestBackend`.
pub fn lines_snapshot(lines: Vec<Line<'static>>, w: u16) -> String {
    let h = lines.len().max(1) as u16;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| f.render_widget(Paragraph::new(lines), f.area())).unwrap();
    buffer_text(term.backend().buffer())
}

pub fn block_snapshot(kind: BlockKind, w: u16, th: &Theme, expanded: bool) -> String {
    let b = Block { id: 1, key: "k".into(), kind };
    lines_snapshot(view::render_block(&b, w, th, expanded), w)
}

pub fn screen(s: &State, th: &Theme, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        view::draw(f, s, th);
    })
    .unwrap();
    buffer_text(term.backend().buffer())
}

/// The buffer cells of a drawn screen (for color assertions).
pub fn screen_buffer(s: &State, th: &Theme, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        view::draw(f, s, th);
    })
    .unwrap();
    term.backend().buffer().clone()
}

/// Compare with `tests/snap/<name>.txt`; `UPDATE_SNAPSHOTS=1` rewrites the file.
pub fn snap(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snap").join(format!("{name}.txt"));
    let actual = format!("{actual}\n");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing snapshot {name}; run with UPDATE_SNAPSHOTS=1"));
    assert_eq!(actual, want, "snapshot {name} differs (UPDATE_SNAPSHOTS=1 regenerates)");
}

pub fn plain() -> Theme {
    Theme::plain()
}
pub fn color() -> Theme {
    Theme::color()
}
pub fn fake_input() -> Value {
    json!({"url": "https://example.com/"})
}

/// The first `Say` effect: (uuid, text).
pub fn said(fx: &[Effect]) -> (String, String) {
    fx.iter().find_map(|e| if let Effect::Say { uuid, text } = e { Some((uuid.clone(), text.clone())) } else { None }).unwrap_or_else(|| panic!("no Say in {fx:?}"))
}
