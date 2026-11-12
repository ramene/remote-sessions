//! s463 mock-up: draw the control room and the drop-in screen with the real code on ratatui's `TestBackend` and print
//! every cell (symbol, fg, bg, bold) as JSON, for SVG frames in the README. Placeholder data only.
//!   cargo run -p sessions-tui --example mock_frames > frames.json
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use serde_json::{json, Value};
use sessions_tui::model::*;
use sessions_tui::reduce::reduce as step;
use sessions_tui::room::*;
use sessions_tui::theme::Theme;
use sessions_tui::view;

const NOW: u64 = 1_800_000_000_000;
const A: &str = "session_01ExampleAAAAAAAAAAAAAAA";
const B: &str = "session_01ExampleBBBBBBBBBBBBBBB";
const C: &str = "session_01ExampleCCCCCCCCCCCCCCC";
const D: &str = "session_01ExampleDDDDDDDDDDDDDDD";
const E: &str = "session_01ExampleEEEEEEEEEEEEEEE";
const F: &str = "session_01ExampleFFFFFFFFFFFFFFF";

fn color(c: Color) -> Value {
    match c {
        Color::Reset => Value::Null,
        Color::Rgb(r, g, b) => json!(format!("#{r:02x}{g:02x}{b:02x}")),
        other => json!(format!("{other:?}")),
    }
}

fn cells(buf: &Buffer) -> Value {
    let mut rows = vec![];
    for y in 0..buf.area.height {
        let mut row = vec![];
        for x in 0..buf.area.width {
            let c = &buf[(x, y)];
            row.push(json!([c.symbol(), color(c.fg), color(c.bg), c.modifier.contains(Modifier::BOLD)]));
        }
        rows.push(Value::Array(row));
    }
    json!({"w": buf.area.width, "h": buf.area.height, "rows": rows})
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn waiting(id: &str, pid: u32, tool: &str, input: Value) -> RemoteRow {
    RemoteRow { host: String::new(), id: id.into(), pid: Some(pid), state: RState::Waiting, prompts: vec![PromptInfo { request_id: format!("req_{pid}"), tool: tool.into(), input }] }
}
fn cloud(id: &str, title: &str, repo: &str, last: &str, ago_ms: u64, finished: bool) -> CloudRow {
    CloudRow { id: id.into(), title: title.into(), repo: repo.into(), last: last.into(), last_at_ms: Some(NOW - ago_ms), finished, error: None, fetched_ms: Some(NOW), from_list: false, kind: RowKind::Session }
}

fn room(w: u16, h: u16) -> RoomState {
    let mut s = RoomState::new("mac-a");
    s.size = (w, h);
    s.air = true;
    let msgs = vec![
        RoomMsg::Tick(NOW),
        RoomMsg::Remote(RemoteSnapshot {
            hosts: vec![],
            rows: vec![
                waiting(A, 4242, "Bash", json!({"command": "npm test"})),
                RemoteRow { host: String::new(), id: B.into(), pid: Some(4243), state: RState::Idle, prompts: vec![] },
                waiting(C, 4244, "Edit", json!({"file_path": "src/auth.ts"})),
            ],
            status: RemoteStatus::Ready,
        }),
        RoomMsg::Cloud(CloudSnapshot {
            rows: vec![
                cloud(D, "Add rate limits to the public API", "acme/api", "assistant: Reading the router", 20_000, false),
                cloud(E, "Fix the dark-mode toggle", "acme/web", "result: Opened a pull request", 7_200_000, true),
                cloud(F, "Upgrade the build to Node 22", "acme/web", "tool: Edit package.json", 840_000, false),
            ],
            status: CloudStatus::Ready { tracked: 3 },
        }),
    ];
    for m in msgs {
        s = sessions_tui::room::reduce(s, m).0;
    }
    s
}

fn room_buf(s: &RoomState, th: &Theme) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(s.size.0, s.size.1)).unwrap();
    term.draw(|f| {
        draw(f, s, th);
    })
    .unwrap();
    term.backend().buffer().clone()
}

fn ev(seq: u64, id: &str, kind: &str, blocks: Vec<Blk>) -> Ev {
    Ev::new(Some(seq), id, kind, blocks)
}

fn drop_in(w: u16, h: u16) -> State {
    let name = A;
    let mut init = ev(1, "i1", "system", vec![]);
    init.init = Some(InitInfo { model: Some("model-x".into()), mode: Some("default".into()) });
    let mut done = ev(12, "r1", "result", vec![]);
    done.result = Some(ResultInfo { subtype: "success".into(), turns: Some(4), duration_ms: Some(96_000) });
    let events = vec![
        init,
        ev(2, "u1", "user", vec![Blk::Text("run the tests and fix what fails".into())]),
        ev(3, "a1", "assistant", vec![Blk::Text("I will run the test suite first.".into())]),
        ev(4, "a2", "assistant", vec![Blk::ToolUse { id: "toolu_ex1".into(), name: "Bash".into(), input: json!({"command": "npm test"}) }]),
        ev(5, "u2", "user", vec![Blk::ToolResult { tool_use_id: "toolu_ex1".into(), text: "> acme-api@1.4.0 test\n> vitest run\n\n src/routes.test.ts  12 passed\n src/auth.test.ts  1 failed\n Tests  1 failed | 30 passed (31)".into(), is_error: true }]),
        ev(6, "a3", "assistant", vec![Blk::Text("One test fails: the token expiry check in src/auth.ts compares seconds with milliseconds. I will fix the unit.".into())]),
        ev(7, "a4", "assistant", vec![Blk::ToolUse { id: "toolu_ex2".into(), name: "Edit".into(), input: json!({"file_path": "src/auth.ts", "old_string": "exp < now", "new_string": "exp * 1000 < now"}) }]),
        ev(8, "u3", "user", vec![Blk::ToolResult { tool_use_id: "toolu_ex2".into(), text: "The file src/auth.ts has been updated.".into(), is_error: false }]),
        ev(9, "a5", "assistant", vec![Blk::ToolUse { id: "toolu_ex3".into(), name: "Bash".into(), input: json!({"command": "npm test"}) }]),
        ev(10, "u4", "user", vec![Blk::ToolResult { tool_use_id: "toolu_ex3".into(), text: " Tests  31 passed (31)".into(), is_error: false }]),
        ev(11, "a6", "assistant", vec![Blk::Text("31 passed, 0 failed.".into())]),
        done,
    ];
    let mut s = State::new(name, "mac-b");
    s.size = (w, h);
    s = step(s, Msg::Snapshot(Ok(Snapshot { name: name.into(), host: "mac-b".into(), events, truncated: false }))).0;
    s = step(s, Msg::Open).0;
    let typed = "also add a test for the expiry edge case";
    s.input.text = typed.into();
    s.input.cursor = typed.len();
    s
}

fn main() {
    let th = Theme::rendersvg();
    let a = room(120, 36);
    let (b, _) = sessions_tui::room::reduce(a.clone(), RoomMsg::Key(key(KeyCode::Char('a'))));
    let c = drop_in(100, 30);
    let mut term = Terminal::new(TestBackend::new(c.size.0, c.size.1)).unwrap();
    term.draw(|f| {
        view::draw(f, &c, &th);
    })
    .unwrap();
    let out = json!({
        "a": cells(&room_buf(&a, &th)),
        "b": cells(&room_buf(&b, &th)),
        "c": cells(term.backend().buffer()),
    });
    println!("{out}");
}
