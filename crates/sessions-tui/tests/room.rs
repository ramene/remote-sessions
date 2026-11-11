//! The control room: layout and styling snapshots on ratatui's `TestBackend`, the click map, the keymap, the reducer.
mod common;
use common::{ctrl, key, snap};
use crossterm::event::KeyCode;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;
use serde_json::json;
use sessions_tui::room::*;
use sessions_tui::theme::Theme;

const NOW: u64 = 1_800_000_000_000;
const A: &str = "session_01ExampleAAAAAAAAAAAAAAA";
const B: &str = "session_01ExampleBBBBBBBBBBBBBBB";
const C: &str = "session_01ExampleCCCCCCCCCCCCCCC";
const D: &str = "session_01ExampleDDDDDDDDDDDDDDD";
const E: &str = "session_01ExampleEEEEEEEEEEEEEEE";
const F: &str = "session_01ExampleFFFFFFFFFFFFFFF";

fn waiting(id: &str, pid: u32, tool: &str, input: serde_json::Value) -> RemoteRow {
    RemoteRow { id: id.into(), pid: Some(pid), state: RState::Waiting, prompts: vec![PromptInfo { request_id: format!("req_{pid}"), tool: tool.into(), input }] }
}
fn cloud(id: &str, title: &str, repo: &str, last: &str, ago_ms: u64, finished: bool) -> CloudRow {
    CloudRow { id: id.into(), title: title.into(), repo: repo.into(), last: last.into(), last_at_ms: Some(NOW - ago_ms), finished, error: None, fetched_ms: Some(NOW), from_list: false, kind: RowKind::Session }
}
fn remote_rows() -> Vec<RemoteRow> {
    vec![
        waiting(A, 4242, "Edit", json!({"file_path": "src/parser.rs"})),
        RemoteRow { id: B.into(), pid: Some(4243), state: RState::Idle, prompts: vec![] },
        waiting(C, 4244, "Bash", json!({"command": "cargo test --workspace"})),
    ]
}
fn cloud_rows() -> Vec<CloudRow> {
    vec![
        cloud(D, "Fix the flaky parser test", "your-org/your-repo", "assistant: Reading the failing test", 20_000, false),
        cloud(E, "Add a docs page for the CLI", "your-org/docs", "result: Opened a pull request", 7_200_000, true),
        cloud(F, "Migrate config to the new schema", "your-org/your-repo", "tool: Edit config.rs", 840_000, false),
    ]
}

fn feed(s: RoomState, ms: Vec<RoomMsg>) -> (RoomState, Vec<RoomEffect>) {
    let mut s = s;
    let mut all = vec![];
    for m in ms {
        let (n, fx) = reduce(s, m);
        s = n;
        all.extend(fx);
    }
    (s, all)
}

fn room(w: u16, h: u16) -> RoomState {
    let mut s = RoomState::new("example-host");
    s.size = (w, h);
    feed(
        s,
        vec![
            RoomMsg::Tick(NOW),
            RoomMsg::Remote(RemoteSnapshot { rows: remote_rows(), status: RemoteStatus::Ready }),
            RoomMsg::Cloud(CloudSnapshot { rows: cloud_rows(), status: CloudStatus::Ready { tracked: 3 } }),
        ],
    )
    .0
}

fn press(s: RoomState, keys: Vec<crossterm::event::KeyEvent>) -> (RoomState, Vec<RoomEffect>) {
    feed(s, keys.into_iter().map(RoomMsg::Key).collect())
}
fn typed(s: RoomState, text: &str) -> RoomState {
    press(s, text.chars().map(|c| key(KeyCode::Char(c))).collect()).0
}

fn text(s: &RoomState, th: &Theme) -> String {
    render_text(s, th, s.size.0, s.size.1, false)
}
fn buffer(s: &RoomState, th: &Theme) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(s.size.0, s.size.1)).unwrap();
    term.draw(|f| {
        draw(f, s, th);
    })
    .unwrap();
    term.backend().buffer().clone()
}

// ---- snapshots ------------------------------------------------------------------------------------------------------

#[test]
fn two_panels_remote_selected_100x24() {
    snap("room_remote_selected_100x24", text(&room(100, 24), &Theme::color()).trim_end());
}

#[test]
fn two_panels_cloud_selected_100x24() {
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Down), key(KeyCode::Down), key(KeyCode::Down)]);
    assert_eq!(s.sel, Some(Sel::Cloud(D.into())));
    snap("room_cloud_selected_100x24", text(&s, &Theme::color()).trim_end());
}

#[test]
fn no_color_ascii_80x20() {
    snap("room_ascii_80x20", text(&room(80, 20), &Theme::plain()).trim_end());
}

#[test]
fn narrow_terminal_drops_columns_not_rows() {
    snap("room_narrow_44x16", text(&room(44, 16), &Theme::color()).trim_end());
}

#[test]
fn too_small_says_so() {
    let s = room(24, 6);
    assert_eq!(text(&s, &Theme::color()).lines().next().unwrap().trim(), "terminal too small");
}

#[test]
fn empty_and_absent_states() {
    let mut s = RoomState::new("");
    s.size = (80, 16);
    let s = feed(s, vec![RoomMsg::Tick(NOW), RoomMsg::Remote(RemoteSnapshot { rows: vec![], status: RemoteStatus::Ready }), RoomMsg::Cloud(CloudSnapshot { rows: vec![], status: CloudStatus::Absent })]).0;
    snap("room_empty_cloud_absent_80x16", text(&s, &Theme::color()).trim_end());
    let s = feed(s, vec![RoomMsg::Remote(RemoteSnapshot { rows: vec![], status: RemoteStatus::NoLogin }), RoomMsg::Cloud(CloudSnapshot { rows: vec![], status: CloudStatus::Failed("not signed in".into()) })]).0;
    let t = text(&s, &Theme::color());
    assert!(t.contains("no claude.ai login") && t.contains("could not be read: not signed in"), "{t}");
}

#[test]
fn confirm_and_send_modes() {
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Char('a'))]);
    snap("room_confirm_approve_100x24", text(&s, &Theme::color()).trim_end());
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Char('j')), key(KeyCode::Char('j')), key(KeyCode::Char('j')), key(KeyCode::Char('s'))]);
    let s = typed(s, "also run the lints");
    snap("room_send_100x24", text(&s, &Theme::color()).trim_end());
}

#[test]
fn hostile_session_text_is_drawn_as_plain_text() {
    let mut s = RoomState::new("");
    s.size = (100, 20);
    let evil = "\u{1b}[31mred\u{1b}[0m \u{202e}reversed\u{7} done";
    let s = feed(s, vec![RoomMsg::Tick(NOW), RoomMsg::Cloud(CloudSnapshot { rows: vec![cloud(D, evil, evil, evil, 1000, false)], status: CloudStatus::Ready { tracked: 1 } })]).0;
    let t = text(&s, &Theme::color());
    assert!(!t.contains('\u{1b}') && !t.contains('\u{202e}') && !t.contains('\u{7}'), "{t}");
    assert!(t.contains("[31mred[0m reversed done"), "{t}");
}

// ---- styling ----------------------------------------------------------------------------------------------------------

fn row_y(s: &RoomState, sel: &Sel) -> u16 {
    layout(Rect::new(0, 0, s.size.0, s.size.1), s).rows.iter().find(|(_, x)| x == sel).map(|(r, _)| r.y).unwrap()
}

#[test]
fn the_selected_row_has_a_background_and_the_waiting_badge_is_amber() {
    let s = room(100, 24);
    let b = buffer(&s, &Theme::color());
    let sel_y = row_y(&s, &Sel::Remote(A.into()));
    let other_y = row_y(&s, &Sel::Remote(B.into()));
    assert_eq!(b[(30, sel_y)].bg, Color::Rgb(0x26, 0x33, 0x40));
    assert_ne!(b[(30, other_y)].bg, Color::Rgb(0x26, 0x33, 0x40));
    // The pause badge (column 3) is amber, the border of the focused panel is teal, the close button is coral.
    assert_eq!(b[(3, sel_y)].symbol(), "\u{23f8}");
    assert_eq!(b[(3, sel_y)].fg, Color::Rgb(0xff, 0xb7, 0x03));
    assert_eq!(b[(0, 1)].fg, Color::Rgb(0x2e, 0xc4, 0xb6), "focused panel border");
    let cloud_top = layout(Rect::new(0, 0, 100, 24), &s).cloud.y;
    assert_ne!(b[(0, cloud_top)].fg, Color::Rgb(0x2e, 0xc4, 0xb6), "the other panel is dim");
    assert_eq!(b[(92, 0)].symbol(), "\u{2715}");
    assert_eq!(b[(92, 0)].fg, Color::Rgb(0xef, 0x47, 0x6f));
}

#[test]
fn without_color_the_selection_is_reverse_video_and_nothing_is_colored() {
    let s = room(80, 20);
    let th = Theme::plain();
    let b = buffer(&s, &th);
    let y = row_y(&s, &Sel::Remote(A.into()));
    assert!(b[(20, y)].modifier.contains(Modifier::REVERSED));
    assert!(!b[(20, row_y(&s, &Sel::Remote(B.into())))].modifier.contains(Modifier::REVERSED));
    assert!(b.content.iter().all(|c| c.fg == Color::Reset && c.bg == Color::Reset), "no colors with NO_COLOR");
    assert!(b.content.iter().all(|c| c.symbol().is_ascii()), "ascii glyphs only");
}

#[test]
fn ansi_dump_carries_the_colors_and_plain_dump_does_not() {
    let s = room(100, 24);
    let ansi = render_text(&s, &Theme::color(), 100, 24, true);
    assert!(ansi.contains("\x1b[") && ansi.contains("38;2;255;183;3"));
    assert!(!render_text(&s, &Theme::color(), 100, 24, false).contains('\x1b'));
}

// ---- the click map -----------------------------------------------------------------------------------------------------

#[test]
fn every_visible_row_maps_back_to_itself_and_chrome_maps_to_nothing() {
    let s = room(100, 24);
    let l = layout(Rect::new(0, 0, 100, 24), &s);
    assert_eq!(l.rows.len(), 6);
    for (r, sel) in &l.rows {
        for x in [r.x, r.x + r.width / 2, r.x + r.width - 1] {
            assert_eq!(l.hit(x, r.y), Some(sel));
        }
    }
    // borders, panel titles, the header, the detail panel, the status line and the footer are not rows
    for (x, y) in [(0, 2), (50, l.remote.y), (50, l.cloud.y), (50, 0), (10, l.detail.unwrap().y + 1), (10, l.status.y), (10, l.footer.y), (99, 3)] {
        assert_eq!(l.hit(x, y), None, "({x},{y})");
    }
    // the first remote row is on screen row 2 (header, then the panel's top border)
    assert_eq!(l.rows[0].0, Rect::new(1, 2, 98, 1));
}

#[test]
fn clicking_a_row_selects_it_and_clicking_it_again_drops_in() {
    let s = room(100, 24);
    let l = layout(Rect::new(0, 0, 100, 24), &s);
    let y_b = l.rows.iter().find(|(_, x)| *x == Sel::Remote(B.into())).unwrap().0.y;
    let (s, fx) = feed(s, vec![RoomMsg::Click(10, y_b)]);
    assert_eq!((s.sel.clone(), fx), (Some(Sel::Remote(B.into())), vec![]));
    let (s, fx) = feed(s, vec![RoomMsg::Click(10, y_b)]);
    assert_eq!(fx, vec![RoomEffect::DropIn(Sel::Remote(B.into()))]);
    assert_eq!(s.sel, Some(Sel::Remote(B.into())));
    // a click on a cloud row selects it and asks for its latest activity once it is stale
    let y_f = layout(Rect::new(0, 0, 100, 24), &s).rows.iter().find(|(_, x)| *x == Sel::Cloud(F.into())).unwrap().0.y;
    let (s, _) = feed(s, vec![RoomMsg::Tick(NOW + 60_000)]);
    let (s, fx) = feed(s, vec![RoomMsg::Click(40, y_f)]);
    assert_eq!((s.sel, fx), (Some(Sel::Cloud(F.into())), vec![RoomEffect::FetchActivity(F.into())]));
}

#[test]
fn the_close_button_is_clickable_in_every_mode() {
    let s = room(100, 24);
    let l = layout(Rect::new(0, 0, 100, 24), &s);
    assert_eq!(l.close, Rect::new(91, 0, 9, 1));
    for x in [91, 95, 99] {
        assert_eq!(feed(room(100, 24), vec![RoomMsg::Click(x, 0)]).1, vec![RoomEffect::Quit], "x={x}");
    }
    assert_eq!(feed(room(100, 24), vec![RoomMsg::Click(90, 0)]).1, vec![]);
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Char('a'))]);
    assert_eq!(feed(s, vec![RoomMsg::Click(95, 0)]).1, vec![RoomEffect::Quit]);
}

#[test]
fn the_wheel_scrolls_the_panel_under_the_pointer_and_the_map_follows() {
    let mut s = RoomState::new("");
    s.size = (80, 14);
    let many: Vec<RemoteRow> = (0..20).map(|i| RemoteRow { id: format!("session_01Example{i:02}XXXXXXXXXXXXXX"), pid: Some(100 + i), state: RState::Idle, prompts: vec![] }).collect();
    let (s, _) = feed(s, vec![RoomMsg::Tick(NOW), RoomMsg::Remote(RemoteSnapshot { rows: many, status: RemoteStatus::Ready })]);
    let l = layout(Rect::new(0, 0, 80, 14), &s);
    let cap = l.caps[0];
    assert!(cap >= 3 && cap < 20, "cap {cap}");
    assert_eq!(l.rows.len(), cap);
    let (s, _) = feed(s, vec![RoomMsg::Wheel(10, 3, -1), RoomMsg::Wheel(10, 3, -1)]);
    assert_eq!(s.top[0], 2);
    let l = layout(Rect::new(0, 0, 80, 14), &s);
    assert_eq!(l.hit(10, 2), Some(&Sel::Remote("session_01Example02XXXXXXXXXXXXXX".into())));
    let (s, _) = feed(s, (0..40).map(|_| RoomMsg::Wheel(10, 3, -1)).collect());
    assert_eq!(s.top[0], 20 - cap, "clamped at the end");
    let (s, _) = feed(s, (0..40).map(|_| RoomMsg::Wheel(10, 3, 1)).collect());
    assert_eq!(s.top[0], 0);
    // the keyboard keeps the selected row on screen
    let (s, _) = press(s, (0..cap + 2).map(|_| key(KeyCode::Down)).collect());
    assert_eq!(s.sel, Some(Sel::Remote(format!("session_01Example{:02}XXXXXXXXXXXXXX", cap + 2))));
    assert_eq!(s.top[0], 3);
    assert!(layout(Rect::new(0, 0, 80, 14), &s).rows.iter().any(|(_, x)| Some(x) == s.sel.as_ref()));
}

// ---- keys ---------------------------------------------------------------------------------------------------------------

#[test]
fn keymap_browse_confirm_and_send() {
    let b = Mode::Browse;
    let cases = [
        (key(KeyCode::Up), Act::Move(-1)),
        (key(KeyCode::Char('k')), Act::Move(-1)),
        (key(KeyCode::Down), Act::Move(1)),
        (key(KeyCode::Char('j')), Act::Move(1)),
        (key(KeyCode::Enter), Act::DropIn),
        (key(KeyCode::Char('a')), Act::Approve),
        (key(KeyCode::Char('d')), Act::Deny),
        (key(KeyCode::Char('s')), Act::Send),
        (key(KeyCode::Char('r')), Act::Refresh),
        (key(KeyCode::Tab), Act::OtherPanel),
        (key(KeyCode::Home), Act::First),
        (key(KeyCode::End), Act::Last),
        (key(KeyCode::Char('q')), Act::Quit),
        (key(KeyCode::Esc), Act::Quit),
        (ctrl('c'), Act::Quit),
        (key(KeyCode::Char('z')), Act::None),
    ];
    for (k, want) in cases {
        assert_eq!(map_key(&b, k), want, "{k:?}");
    }
    let c = Mode::Confirm { allow: true, session: A.into(), request_id: "r".into(), tool: "Edit".into(), args: String::new() };
    assert_eq!(map_key(&c, key(KeyCode::Char('y'))), Act::Confirm);
    assert_eq!(map_key(&c, key(KeyCode::Enter)), Act::None, "Enter never answers a prompt");
    assert_eq!(map_key(&c, key(KeyCode::Esc)), Act::Cancel);
    assert_eq!(map_key(&c, key(KeyCode::Char('n'))), Act::Cancel);
    assert_eq!(map_key(&c, ctrl('c')), Act::Cancel);
    let t = Mode::Send { id: D.into(), text: String::new() };
    assert_eq!(map_key(&t, key(KeyCode::Char('q'))), Act::Char('q'), "q types, it does not quit, while typing");
    assert_eq!(map_key(&t, key(KeyCode::Enter)), Act::Submit);
    assert_eq!(map_key(&t, key(KeyCode::Esc)), Act::Cancel);
    assert_eq!(map_key(&t, ctrl('u')), Act::ClearLine);
    assert_eq!(map_key(&t, key(KeyCode::Backspace)), Act::Backspace);
}

// ---- the reducer ----------------------------------------------------------------------------------------------------------

#[test]
fn arrows_walk_remote_then_cloud_and_stop_at_the_ends() {
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Up)]);
    assert_eq!(s.sel, Some(Sel::Remote(A.into())));
    let (s, _) = press(s, vec![key(KeyCode::Down), key(KeyCode::Down)]);
    assert_eq!(s.sel, Some(Sel::Remote(C.into())));
    let (s, fx) = press(s, vec![key(KeyCode::Down)]);
    assert_eq!(s.sel, Some(Sel::Cloud(D.into())));
    assert_eq!(fx, vec![], "its activity is fresh, nothing to read");
    let (s, _) = press(s, vec![key(KeyCode::End), key(KeyCode::Down)]);
    assert_eq!(s.sel, Some(Sel::Cloud(F.into())));
    let (s, _) = press(s, vec![key(KeyCode::Tab)]);
    assert_eq!(s.sel, Some(Sel::Remote(A.into())), "Tab jumps to the other panel");
    let (s, _) = press(s, vec![key(KeyCode::Tab)]);
    assert_eq!(s.sel, Some(Sel::Cloud(D.into())));
}

#[test]
fn enter_drops_in_and_q_esc_ctrl_c_close() {
    let (_, fx) = press(room(100, 24), vec![key(KeyCode::Enter)]);
    assert_eq!(fx, vec![RoomEffect::DropIn(Sel::Remote(A.into()))]);
    for k in [key(KeyCode::Char('q')), key(KeyCode::Esc), ctrl('c')] {
        assert_eq!(press(room(100, 24), vec![k]).1, vec![RoomEffect::Quit]);
    }
}

#[test]
fn approve_needs_a_y_and_cloud_has_no_approve() {
    let (s, fx) = press(room(100, 24), vec![key(KeyCode::Char('a'))]);
    assert!(matches!(&s.mode, Mode::Confirm { allow: true, session, request_id, tool, args } if session == A && request_id == "req_4242" && tool == "Edit" && args == "src/parser.rs"));
    assert_eq!(fx, vec![]);
    let (s, fx) = press(s, vec![key(KeyCode::Enter)]);
    assert_eq!((&s.mode, &fx), (&s.mode.clone(), &vec![]), "Enter does nothing in a confirmation");
    assert!(matches!(s.mode, Mode::Confirm { .. }));
    let (s, fx) = press(s, vec![key(KeyCode::Char('y'))]);
    assert_eq!(fx, vec![RoomEffect::Answer { session: A.into(), request_id: "req_4242".into(), allow: true }]);
    assert_eq!(s.mode, Mode::Browse);
    let (s, fx) = press(room(100, 24), vec![key(KeyCode::Char('d')), key(KeyCode::Char('y'))]);
    assert_eq!(fx, vec![RoomEffect::Answer { session: A.into(), request_id: "req_4242".into(), allow: false }]);
    assert_eq!(s.mode, Mode::Browse);
    let (s, fx) = press(room(100, 24), vec![key(KeyCode::Char('a')), key(KeyCode::Esc)]);
    assert_eq!((s.mode, fx), (Mode::Browse, vec![]));
    // an idle Remote Control session has nothing to approve
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Down), key(KeyCode::Char('a'))]);
    assert_eq!(s.mode, Mode::Browse);
    assert!(s.toast.as_ref().unwrap().text.contains("not waiting"));
    // a cloud session has no prompts at all
    let (s, fx) = press(room(100, 24), vec![key(KeyCode::End), key(KeyCode::Char('a'))]);
    assert_eq!((s.mode.clone(), fx), (Mode::Browse, vec![]));
    assert!(s.toast.unwrap().text.contains("raise no permission prompts"));
}

#[test]
fn a_prompt_that_disappears_withdraws_the_confirmation() {
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Char('a'))]);
    let mut rows = remote_rows();
    rows[0].state = RState::Idle;
    rows[0].prompts.clear();
    let (s, _) = feed(s, vec![RoomMsg::Remote(RemoteSnapshot { rows, status: RemoteStatus::Ready })]);
    assert_eq!(s.mode, Mode::Browse);
    assert!(s.toast.unwrap().text.contains("no longer pending"));
}

#[test]
fn send_is_for_cloud_sessions_and_types_into_a_line() {
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Char('s'))]);
    assert_eq!(s.mode, Mode::Browse);
    assert!(s.toast.as_ref().unwrap().text.contains("cloud sessions"));
    let (s, _) = press(room(100, 24), vec![key(KeyCode::End), key(KeyCode::Char('s'))]);
    assert!(matches!(&s.mode, Mode::Send { id, .. } if id == F));
    let s = typed(s, "run the lints, q");
    let (s, fx) = press(s, vec![key(KeyCode::Backspace), key(KeyCode::Backspace), key(KeyCode::Enter)]);
    assert_eq!(fx, vec![RoomEffect::CloudSend { id: F.into(), text: "run the lints,".into() }]);
    assert_eq!(s.mode, Mode::Browse);
    // an empty line is not sent, Esc leaves
    let (s, fx) = press(room(100, 24), vec![key(KeyCode::End), key(KeyCode::Char('s')), key(KeyCode::Enter)]);
    assert_eq!(fx, vec![]);
    assert!(matches!(s.mode, Mode::Send { .. }));
    let (s, _) = press(s, vec![key(KeyCode::Esc)]);
    assert_eq!(s.mode, Mode::Browse);
    let (s, _) = feed(s, vec![]);
    assert_eq!(s.sel, Some(Sel::Cloud(F.into())));
}

#[test]
fn refresh_keeps_the_selection_by_id_and_moves_off_a_vanished_row() {
    let (s, _) = press(room(100, 24), vec![key(KeyCode::Down), key(KeyCode::Down)]);
    assert_eq!(s.sel, Some(Sel::Remote(C.into())));
    let mut rows = remote_rows();
    rows.reverse();
    let (s, _) = feed(s, vec![RoomMsg::Remote(RemoteSnapshot { rows, status: RemoteStatus::Ready })]);
    assert_eq!(s.sel, Some(Sel::Remote(C.into())));
    let (s, _) = feed(s, vec![RoomMsg::Remote(RemoteSnapshot { rows: vec![], status: RemoteStatus::Ready })]);
    assert_eq!(s.sel, Some(Sel::Cloud(D.into())), "falls to the first row that is left");
}

#[test]
fn cloud_activity_is_read_one_at_a_time_and_never_in_a_chain() {
    // a listing of three rows never read
    let mut s = RoomState::new("");
    s.size = (100, 24);
    let rows = vec![CloudRow::listed(D, "one", "your-org/a"), CloudRow::listed(E, "two", "your-org/b"), CloudRow::listed(F, "three", "your-org/c")];
    let (s, fx) = feed(s, vec![RoomMsg::Tick(NOW), RoomMsg::Cloud(CloudSnapshot { rows: rows.clone(), status: CloudStatus::Ready { tracked: 3 } })]);
    assert_eq!(fx, vec![RoomEffect::FetchActivity(D.into())], "one read, for the selected (first) row");
    assert_eq!(s.fetching.as_deref(), Some(D));
    // another snapshot while the read is out asks for nothing
    let (s, fx) = feed(s, vec![RoomMsg::Cloud(CloudSnapshot { rows, status: CloudStatus::Ready { tracked: 3 } })]);
    assert_eq!(fx, vec![]);
    // the answer arrives: the row is filled in, and no second read is chained
    let (s, fx) = feed(s, vec![RoomMsg::Tick(NOW + 1000), RoomMsg::Activity(CloudActivity { id: D.into(), finished: false, last: "assistant: hi".into(), last_at_ms: Some(NOW), error: None })]);
    assert_eq!(fx, vec![]);
    assert_eq!(s.fetching, None);
    let c = s.cloud_row(D).unwrap();
    assert_eq!((c.state(NOW + 1000), c.last.as_str()), (CState::Running, "assistant: hi"));
    // the next listing (the timer) reads one more row, the first never read
    let rows = vec![CloudRow::listed(D, "one", "your-org/a"), CloudRow::listed(E, "two", "your-org/b"), CloudRow::listed(F, "three", "your-org/c")];
    let (s, fx) = feed(s, vec![RoomMsg::Cloud(CloudSnapshot { rows, status: CloudStatus::Ready { tracked: 3 } })]);
    assert_eq!(s.cloud_row(D).unwrap().last, "assistant: hi", "what was read is kept across listings");
    assert_eq!(fx, vec![RoomEffect::FetchActivity(E.into())]);
}

#[test]
fn ages_and_states_come_from_the_newest_event() {
    assert_eq!((ago(NOW, NOW - 20_000), ago(NOW, NOW - 300_000), ago(NOW, NOW - 7_200_000), ago(NOW, NOW - 200_000_000)), ("20s".to_string(), "5m".to_string(), "2h".to_string(), "2d".to_string()));
    let mut c = cloud(D, "t", "r", "x", 20_000, false);
    assert_eq!(c.state(NOW), CState::Running);
    assert_eq!(c.state(NOW + 600_000), CState::Idle);
    c.finished = true;
    assert_eq!(c.state(NOW), CState::Finished);
    c.error = Some("boom".into());
    assert_eq!(c.state(NOW), CState::Unreadable);
    assert_eq!(CloudRow::listed(D, "t", "r").state(NOW), CState::Unknown);
}

#[test]
fn resize_keeps_the_selected_row_visible() {
    let mut s = RoomState::new("");
    s.size = (80, 30);
    let many: Vec<RemoteRow> = (0..12).map(|i| RemoteRow { id: format!("session_01Example{i:02}XXXXXXXXXXXXXX"), pid: Some(100 + i), state: RState::Idle, prompts: vec![] }).collect();
    let (s, _) = feed(s, vec![RoomMsg::Tick(NOW), RoomMsg::Remote(RemoteSnapshot { rows: many, status: RemoteStatus::Ready })]);
    let (s, _) = press(s, vec![key(KeyCode::End)]);
    let (s, _) = feed(s, vec![RoomMsg::Resize(80, 12)]);
    assert!(layout(Rect::new(0, 0, 80, 12), &s).rows.iter().any(|(_, x)| Some(x) == s.sel.as_ref()));
}

// ---- extra panels: the panel interface, with two placeholder panels ---------------------------------------------------------

fn builds_spec() -> PanelSpec {
    PanelSpec {
        title: "Builds".into(),
        noun: "builds".into(),
        tag: "placeholder source".into(),
        badge: Some((Tone::Ok, Glyph::Dot, "running".into())),
        empty: "no build is running".into(),
        failed: "the build source could not be read".into(),
        approve_hint: "A build has no prompts; Enter drops in to read it.".into(),
        send_hint: "Enter drops into this build".into(),
        name_fit: NameFit::Clamp(8, 18),
        every_ms: 5000,
    }
}
fn queue_spec() -> PanelSpec {
    PanelSpec { title: "Queue".into(), noun: "queue".into(), tag: "placeholder queue".into(), badge: Some((Tone::Warn, Glyph::Pause, "held".into())), empty: "the queue is empty".into(), failed: "the queue could not be read".into(), approve_hint: "Queue items are released at their source.".into(), send_hint: "Enter drops into this item".into(), name_fit: NameFit::Fixed(14), every_ms: 3000 }
}
fn prow(id: &str, name: &str, rank: u8, tone: Tone, state: &str, extra: &str, place: (&str, &str)) -> PanelRow {
    PanelRow {
        id: id.into(),
        name: name.into(),
        rank,
        glyph: match tone {
            Tone::Ok => Glyph::Dot,
            Tone::Warn => Glyph::Pause,
            Tone::Error => Glyph::Cross,
            Tone::Dim => Glyph::Ring,
        },
        tone,
        cols: vec![
            Col { text: Txt::new(state), ink: Ink::Tone, fit: Fit::Fixed(10), min_left: 30, skip_empty: false },
            Col { text: Txt::new(extra), ink: Ink::Accent2, fit: Fit::Share { div: 3, min: 6, max: 12 }, min_left: 24, skip_empty: true },
            Col { text: Txt::joined(vec![place.0.into(), place.1.into()]), ink: Ink::Dim, fit: Fit::Rest, min_left: 8, skip_empty: false },
        ],
        detail: vec![
            vec![Piece::new(name, Ink::Bold, Max::WidthLess(0))],
            vec![Piece::new("where ", Ink::Dim, Max::Any), Piece { text: Txt::joined(vec![place.0.into(), place.1.into()]), ink: Ink::Plain, max: Max::WidthLess(6) }, Piece::new(format!("  id {id}"), Ink::Dim, Max::Any)],
            vec![Piece::new("Enter ", Ink::Accent, Max::Any), Piece::new("drop in: read it, two-way", Ink::Dim, Max::Any)],
        ],
    }
}
fn build_rows() -> Vec<PanelRow> {
    vec![prow("b-1", "compile", 0, Tone::Ok, "running", "linux", ("ci", "main")), prow("b-2", "lint", 0, Tone::Dim, "idle", "", ("ci", ""))]
}
fn queue_rows() -> Vec<PanelRow> {
    vec![
        prow("q-1", "planner", 3, Tone::Dim, "idle", "kind-a", ("api", "main")),
        prow("q-2", "builder", 1, Tone::Ok, "working", "kind-b", ("api", "main")),
        prow("q-3", "reviewer", 0, Tone::Warn, "held", "kind-a", ("api", "")),
        prow("q-4", "docs", 2, Tone::Dim, "done", "kind-a", ("web", "t2")),
        prow("q-5", "tester", 1, Tone::Ok, "working", "kind-b", ("web", "t2")),
    ]
}
fn with_specs(s: RoomState) -> RoomState {
    let mut p = RoomState::new(&s.host).with_panels(vec![builds_spec(), queue_spec()]);
    p.size = s.size;
    p.air = s.air;
    p.band = s.band;
    feed(p, vec![RoomMsg::Tick(NOW), RoomMsg::Remote(RemoteSnapshot { rows: remote_rows(), status: RemoteStatus::Ready }), RoomMsg::Cloud(CloudSnapshot { rows: cloud_rows(), status: CloudStatus::Ready { tracked: 3 } })]).0
}
fn with_builds(s: RoomState) -> RoomState {
    feed(s, vec![RoomMsg::Panel(0, PanelSnapshot { rows: build_rows(), status: PanelStatus::Ready })]).0
}
fn with_queue(s: RoomState) -> RoomState {
    feed(s, vec![RoomMsg::Panel(1, PanelSnapshot { rows: queue_rows(), status: PanelStatus::Ready })]).0
}

#[test]
fn alloc3_gives_each_list_what_it_wants_then_shares_the_rest() {
    assert_eq!(alloc3(30, [4, 5, 6]), [4, 5, 6], "everything fits");
    assert_eq!(alloc3(9, [10, 10, 10]), [3, 3, 3], "at least 3 each");
    assert_eq!(alloc3(12, [10, 10, 10]), [4, 4, 4], "the spare rows go one at a time to the list furthest below what it wants");
    let h = alloc3(20, [3, 4, 30]);
    assert_eq!(h, [3, 3, 14]);
    assert!(h.iter().map(|&x| x as u32).sum::<u32>() <= 20);
}

#[test]
fn an_extra_panel_is_drawn_only_after_its_source_answered() {
    let th = Theme::color();
    let plain = text(&room(100, 30), &th);
    let before = text(&with_specs(room(100, 30)), &th);
    assert_eq!(before, plain, "panels given but not answered: the two-panel room, byte for byte");
    let (absent, _) = feed(with_specs(room(100, 30)), vec![RoomMsg::Panel(0, PanelSnapshot { rows: vec![], status: PanelStatus::Absent })]);
    assert_eq!(text(&absent, &th), before, "an Absent answer changes nothing on screen");
    let with = text(&with_builds(with_specs(room(100, 30))), &th);
    assert!(with.contains(" Builds ") && with.contains("compile") && with.contains("lint") && with.contains("1 running") && with.contains("placeholder source"), "{with}");
    let (empty, _) = feed(with_specs(room(100, 30)), vec![RoomMsg::Panel(0, PanelSnapshot { rows: vec![], status: PanelStatus::Ready })]);
    assert!(text(&empty, &th).contains("no build is running"));
    let (failed, _) = feed(with_specs(room(100, 40)), vec![RoomMsg::Panel(1, PanelSnapshot { rows: vec![], status: PanelStatus::Failed("timed out".into()) })]);
    assert!(text(&failed, &th).contains("the queue could not be read: timed out"));
    let (loading, _) = feed(with_specs(room(100, 40)), vec![RoomMsg::Panel(1, PanelSnapshot { rows: vec![], status: PanelStatus::Loading })]);
    assert!(text(&loading, &th).contains(" Queue ") && text(&loading, &th).contains("reading ..."));
    // a read for a panel index that does not exist is ignored
    let (odd, _) = feed(with_specs(room(100, 30)), vec![RoomMsg::Panel(9, PanelSnapshot { rows: build_rows(), status: PanelStatus::Ready })]);
    assert_eq!(text(&odd, &th), before);
}

#[test]
fn rows_are_ordered_by_rank_and_keep_the_sources_order_within_a_rank() {
    let s = with_queue(with_specs(room(100, 40)));
    let order: Vec<&str> = s.panels[1].rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(order, vec!["q-3", "q-2", "q-5", "q-4", "q-1"]);
}

#[test]
fn tab_cycles_remote_cloud_then_each_extra_panel_and_skips_one_with_no_rows() {
    let mut s = with_queue(with_builds(with_specs(room(100, 44))));
    assert_eq!(s.sel, Some(Sel::Remote(A.into())));
    let mut seen = vec![];
    for _ in 0..4 {
        s = press(s, vec![key(KeyCode::Tab)]).0;
        seen.push(s.sel.clone().unwrap());
    }
    assert_eq!(seen, vec![Sel::Cloud(D.into()), Sel::Panel(0, "b-1".into()), Sel::Panel(1, "q-3".into()), Sel::Remote(A.into())]);
    let mut s = with_queue(with_specs(room(100, 40)));
    let seen: Vec<Sel> = (0..3)
        .map(|_| {
            s = press(s.clone(), vec![key(KeyCode::Tab)]).0;
            s.sel.clone().unwrap()
        })
        .collect();
    assert_eq!(seen, vec![Sel::Cloud(D.into()), Sel::Panel(1, "q-3".into()), Sel::Remote(A.into())], "the first panel has no rows: Cloud goes straight to the second");
}

#[test]
fn enter_drops_into_an_extra_row_and_approve_and_send_give_the_panels_hint() {
    let s = press(with_queue(with_specs(room(100, 40))), vec![key(KeyCode::Tab), key(KeyCode::Tab)]).0;
    assert_eq!(s.sel, Some(Sel::Panel(1, "q-3".into())));
    assert_eq!(press(s.clone(), vec![key(KeyCode::Enter)]).1, vec![RoomEffect::DropIn(Sel::Panel(1, "q-3".into()))]);
    for k in ['a', 'd'] {
        let (s3, fx) = press(s.clone(), vec![key(KeyCode::Char(k))]);
        assert!(fx.is_empty() && s3.mode == Mode::Browse, "{k} answers nothing");
        assert_eq!(s3.toast.as_ref().unwrap().text, "Queue items are released at their source.");
    }
    let (s3, fx) = press(s, vec![key(KeyCode::Char('s'))]);
    assert!(fx.is_empty() && s3.mode == Mode::Browse);
    assert_eq!(s3.toast.as_ref().unwrap().text, "Enter drops into this item");
    let detail = text(&s3, &Theme::detect(true, false));
    assert!(detail.contains("reviewer") && detail.contains("where api") && detail.contains("id q-3") && detail.contains("drop in: read it, two-way"), "{detail}");
    assert!(detail.contains("Enter/click drop in"), "the footer chip:\n{detail}");
    assert_eq!(drop_in_text(&Sel::Panel(1, "q-3".into())), "drop in: q-3");
}

#[test]
fn panel_line_columns_fit_the_width_and_hostile_text_is_cleaned() {
    let th = Theme::color();
    let flat = |l: ratatui::text::Line<'static>| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
    let wide = flat(panel_line(&queue_rows()[2], NameFit::Fixed(14), 100, false, &th));
    for want in ["reviewer", "held", "kind-a", "api"] {
        assert!(wide.contains(want), "{want} in {wide:?}");
    }
    let joined = flat(panel_line(&queue_rows()[0], NameFit::Fixed(14), 100, false, &th));
    assert!(joined.contains(&format!("api {} main", th.sep().trim())), "{joined:?}");
    let narrow = flat(panel_line(&queue_rows()[2], NameFit::Fixed(14), 30, false, &th));
    assert!(narrow.contains("reviewer") && !narrow.contains("kind-a"), "{narrow:?}");
    let no_extra = flat(panel_line(&build_rows()[1], NameFit::Clamp(8, 18), 100, false, &th));
    let idle_at = no_extra.find("idle").unwrap();
    assert_eq!(no_extra.find("ci"), Some(idle_at + 10 + 2), "an empty skip_empty column is left out, so the place follows the state: {no_extra:?}");
    let evil = prow("x\u{1b}[2Jy", "x\u{1b}[31mred\u{7}", 0, Tone::Ok, "work\ning", "c\u{1b}[2Jl", ("a\nb", "t\tt"));
    let (s, _) = feed(with_specs(room(100, 40)), vec![RoomMsg::Panel(1, PanelSnapshot { rows: vec![evil], status: PanelStatus::Ready })]);
    let r = &s.panels[1].rows[0];
    let mut all: Vec<String> = vec![r.id.clone(), r.name.clone()];
    all.extend(r.cols.iter().flat_map(|c| c.text.parts.clone()));
    all.extend(r.detail.iter().flatten().flat_map(|p| p.text.parts.clone()));
    for f in &all {
        assert!(!f.contains('\u{1b}') && !f.contains('\u{7}') && !f.contains('\n') && !f.contains('\t'), "{f:?}");
    }
    assert!(r.detail[2][0].text.parts[0] == "Enter ", "a label keeps its trailing space");
}

#[test]
fn four_panels_fit_or_the_extras_are_dropped_never_overflow() {
    let h = alloc_n(20, &[3, 4, 30, 3]);
    assert!(h.iter().all(|&x| x >= 3) && h.iter().map(|&x| x as u32).sum::<u32>() <= 20, "{h:?}");
    assert_eq!(alloc_n(40, &[4, 5, 6, 7]), vec![4, 5, 6, 7]);
    let all = with_queue(with_builds(with_specs(room(100, 44))));
    let l = layout(Rect::new(0, 0, 100, 44), &all);
    assert!(l.remote.height > 0 && l.cloud.height > 0 && l.panels[0].height > 0 && l.panels[1].height > 0);
    assert!(l.rows.iter().any(|(_, sel)| matches!(sel, Sel::Panel(1, _))));
    assert!(l.panels[1].y + l.panels[1].height <= 44, "no panel runs off the screen");
    for h in 12..=20u16 {
        let mut s = all.clone();
        s.size = (100, h);
        let l = layout(Rect::new(0, 0, 100, h), &s);
        let used = l.remote.height + l.cloud.height + l.panels.iter().map(|r| r.height).sum::<u16>();
        assert!(used + 3 <= h, "h={h} used={used}");
        let _ = text(&s, &Theme::color());
    }
    let mut s = all;
    s.size = (100, 11);
    let l = layout(Rect::new(0, 0, 100, 11), &s);
    assert_eq!((l.panels[1].height, l.panels[0].height), (0, 0), "11 rows: the two-panel room");
}

#[test]
fn the_header_band_and_status_line_count_each_shown_panel() {
    let mut s = with_queue(with_builds(with_specs(room(120, 44))));
    s.band = true;
    let frame = text(&s, &Theme::color());
    assert!(frame.contains("remote 3") && frame.contains("cloud 3") && frame.contains("builds 2") && frame.contains("queue 5"), "{frame}");
    let s = with_builds(with_specs(room(120, 44)));
    let frame = text(&s, &Theme::color());
    let status = frame.lines().rev().nth(1).unwrap();
    assert!(status.contains("builds 0s ago") && !status.contains("queue"), "{status:?}");
}

// ---- rows know their state from the list; routine group rows (s387) ------------------------------------------------------

fn from_list(id: &str, title: &str, repo: &str, ago_ms: u64, finished: bool) -> CloudRow {
    let mut c = CloudRow::listed(id, title, repo);
    c.last_at_ms = Some(NOW - ago_ms);
    c.finished = finished;
    c.from_list = true;
    c
}
fn routine_head(id: &str, name: &str, schedule: &str, enabled: bool) -> CloudRow {
    let mut c = CloudRow::listed(id, name, "");
    c.from_list = true;
    c.kind = RowKind::Routine { schedule: schedule.into(), enabled };
    c
}
fn note_row(text: &str) -> CloudRow {
    let mut c = CloudRow::listed("note~x", text, "");
    c.from_list = true;
    c.kind = RowKind::Note;
    c
}
fn cloud_only(rows: Vec<CloudRow>) -> RoomState {
    let mut s = RoomState::new("");
    s.size = (100, 30);
    let n = rows.iter().filter(|r| !r.is_group()).count();
    feed(s, vec![RoomMsg::Tick(NOW), RoomMsg::Remote(RemoteSnapshot { rows: vec![], status: RemoteStatus::Ready }), RoomMsg::Cloud(CloudSnapshot { rows, status: CloudStatus::Ready { tracked: n } })]).0
}

#[test]
fn a_listing_of_41_rows_has_a_state_on_the_first_frame_and_reads_only_the_selected_row() {
    let rows: Vec<CloudRow> = (0..41u64)
        .map(|i| from_list(&format!("session_01Example{i:02}AAAAAAAAAAAAAA"), &format!("Session number {i}"), "your-org/repo", 60_000 + i * 3_600_000, i % 7 == 6))
        .collect();
    let mut s0 = RoomState::new("");
    s0.size = (110, 30);
    let (s, fx) = feed(s0, vec![RoomMsg::Tick(NOW), RoomMsg::Remote(RemoteSnapshot { rows: vec![], status: RemoteStatus::Ready }), RoomMsg::Cloud(CloudSnapshot { rows, status: CloudStatus::Ready { tracked: 41 } })]);
    assert!(s.cloud.iter().all(|c| c.state(NOW) != CState::Unknown), "no row waits for an activity read");
    assert_eq!(fx.iter().filter(|e| matches!(e, RoomEffect::FetchActivity(_))).count(), 1, "one read, for the selected row only");
    let frame = text(&s, &Theme::detect(true, false));
    assert!(!frame.contains("..."), "no pending state anywhere on the first frame:\n{frame}");
    assert!(frame.contains("running") && frame.contains("idle"), "{frame}");
    // the answer arrives; a later timer tick asks for no further row (the old row-by-row crawl is gone)
    let id = s.fetching.clone().unwrap();
    let (s, _) = feed(s, vec![RoomMsg::Activity(CloudActivity { id, finished: false, last: "assistant: hi".into(), last_at_ms: Some(NOW), error: None })]);
    let (_, fx) = feed(s, vec![RoomMsg::Tick(NOW + 10_000)]);
    assert_eq!(fx, vec![]);
    // a row with no list information keeps the old behaviour: unknown until it is read
    assert_eq!(CloudRow::listed(D, "x", "y").state(NOW), CState::Unknown);
}

#[test]
fn a_new_listing_keeps_the_read_detail_and_takes_the_lists_state() {
    let first = vec![from_list(D, "one", "your-org/a", 600_000, false)];
    let s = cloud_only(first);
    let (s, _) = feed(s, vec![RoomMsg::Activity(CloudActivity { id: D.into(), finished: false, last: "assistant: hi".into(), last_at_ms: Some(NOW - 500_000), error: None })]);
    let (s, _) = feed(s, vec![RoomMsg::Cloud(CloudSnapshot { rows: vec![from_list(D, "one", "your-org/a", 1_000, true)], status: CloudStatus::Ready { tracked: 1 } })]);
    let c = s.cloud_row(D).unwrap();
    assert_eq!(c.last, "assistant: hi", "the detail line from the read is kept");
    assert_eq!((c.last_at_ms, c.finished), (Some(NOW - 1_000), true), "the newer age and the archive state come from the list");
    assert_eq!(c.state(NOW), CState::Finished);
}

#[test]
fn routine_group_rows_are_drawn_dim_and_are_never_selected_or_clickable() {
    let rows = vec![
        note_row("routines unavailable: test note"),
        routine_head("routine~trig_1", "Nightly build", "cron 0 3 * * *", true),
        { let mut r = from_list(D, "Nightly run one", "Nightly build", 60_000, false); r.kind = RowKind::Run; r },
        { let mut r = from_list(E, "Nightly run two", "Nightly build", 90_000_000, true); r.kind = RowKind::Run; r },
        routine_head("routine~trig_2", "One-off", "once 2026-12-01T00:00:00Z", false),
        from_list(F, "A plain session", "your-org/a", 5_000, false),
    ];
    let s = cloud_only(rows);
    let th = Theme::detect(true, false);
    let frame = text(&s, &th);
    for want in ["Nightly build", "cron 0 3 * * *", "One-off", "once 2026-12-01", "routines unavailable: test note", "Nightly run one"] {
        assert!(frame.contains(want), "{want}:\n{frame}");
    }
    assert!(frame.contains(" on") && frame.contains(" off"), "enabled state shown:\n{frame}");
    // the arrows walk the three real rows only
    let mut seen = vec![];
    let mut cur = s.clone();
    for _ in 0..6 {
        if let Some(Sel::Cloud(id)) = &cur.sel {
            if !seen.contains(id) {
                seen.push(id.clone());
            }
        }
        cur = press(cur, vec![key(KeyCode::Down)]).0;
    }
    assert_eq!(seen, vec![D.to_string(), E.to_string(), F.to_string()]);
    assert_eq!(s.order().len(), 3);
    // no clickable rectangle for a group row: the map holds exactly the three real rows
    let l = layout(Rect::new(0, 0, s.size.0, s.size.1), &s);
    assert_eq!(l.rows.len(), 3);
    assert!(l.rows.iter().all(|(_, sel)| matches!(sel, Sel::Cloud(id) if id == D || id == E || id == F)));
    // the running count does not count a routine header
    assert_eq!(s.running(), 2);
    // Enter on the first real row drops into the run (a normal cloud drop-in)
    assert_eq!(press(s, vec![key(KeyCode::Enter)]).1, vec![RoomEffect::DropIn(Sel::Cloud(D.into()))]);
}

// ---- routines are an optional view: the R key (s391) -------------------------------------------------------------------------

fn routines_room() -> RoomState {
    let mut s = cloud_only(vec![
        routine_head("routine~trig_1", "Nightly build", "cron 0 3 * * *", true),
        { let mut r = from_list(D, "Run one", "", 60_000, false); r.kind = RowKind::Run; r },
        { let mut r = from_list(E, "Run two", "", 90_000, true); r.kind = RowKind::Run; r },
        { let mut n = note_row("routines unavailable: x"); n.id = "note~routines".into(); n },
        from_list(F, "A plain session", "your-org/a", 5_000, false),
    ]);
    s.routines_available = true;
    s.routines = true;
    s
}

#[test]
fn r_toggles_routines_with_a_toast_and_one_effect_each_way() {
    let s = routines_room();
    let (s, fx) = press(s, vec![key(KeyCode::Char('R'))]);
    assert_eq!((s.routines, fx), (false, vec![RoomEffect::Routines(false)]));
    assert_eq!(s.toast.as_ref().unwrap().text, "routines hidden");
    assert!(s.cloud.iter().all(|c| c.kind == RowKind::Session), "the routine rows, runs and note are gone at once");
    assert_eq!(s.cloud.len(), 1);
    let (s, fx) = press(s, vec![key(KeyCode::Char('R'))]);
    assert_eq!((s.routines, fx), (true, vec![RoomEffect::Routines(true)]));
    assert_eq!(s.toast.as_ref().unwrap().text, "routines shown");
    // a lowercase r is still refresh
    assert_eq!(press(s, vec![key(KeyCode::Char('r'))]).1, vec![RoomEffect::Refresh]);
}

#[test]
fn without_a_routines_backend_r_says_so_and_the_footer_has_no_hint() {
    let th = Theme::detect(true, false);
    let mut s = cloud_only(vec![from_list(D, "x", "y", 1000, false)]);
    assert!(!s.routines_available);
    let before = text(&s, &th);
    assert!(!before.contains("R routines"), "the footer of every old frame is unchanged:\n{before}");
    let (s2, fx) = press(s.clone(), vec![key(KeyCode::Char('R'))]);
    assert!(fx.is_empty() && s2.toast.as_ref().unwrap().text.contains("not available"));
    s.routines_available = true;
    assert!(text(&s, &th).contains("R routines"));
}

#[test]
fn the_routine_header_is_one_plain_line_of_text() {
    let th = Theme::color();
    let flat = |l: ratatui::text::Line<'static>| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
    let on = flat(cloud_line(&routine_head("routine~t", "Nightly build", "cron 0 3 * * *", true), NOW, 100, false, &th));
    assert_eq!(on.trim_end(), "  routine \u{b7} Nightly build \u{b7} cron 0 3 * * * \u{b7} on");
    let off = flat(cloud_line(&routine_head("routine~t", "One-off", "once 2026-12-01T00:00:00Z", false), NOW, 100, false, &th));
    assert_eq!(off.trim_end(), "  routine \u{b7} One-off \u{b7} once 2026-12-01T00:00:00Z \u{b7} off");
    for glyph in ['\u{23f8}', '\u{26a1}', '\u{25cb}', '\u{25cf}'] {
        assert!(!on.contains(glyph) && !off.contains(glyph), "no {glyph:?}");
    }
}

#[test]
fn an_empty_activity_read_keeps_the_age_and_state_the_list_gave() {
    let s = cloud_only(vec![from_list(D, "one", "your-org/a", 120_000, true), from_list(E, "two", "your-org/b", 90_000, false)]);
    let (s, _) = feed(s, vec![RoomMsg::Activity(CloudActivity { id: D.into(), finished: false, last: String::new(), last_at_ms: None, error: None })]);
    let c = s.cloud_row(D).unwrap();
    assert_eq!((c.last_at_ms, c.finished, c.state(NOW)), (Some(NOW - 120_000), true, CState::Finished), "the read found no event; the list's age and archive state stay");
    let (s, _) = feed(s, vec![RoomMsg::Activity(CloudActivity { id: E.into(), finished: false, last: "assistant: hi".into(), last_at_ms: Some(NOW - 1_000), error: None })]);
    assert_eq!(s.cloud_row(E).unwrap().last_at_ms, Some(NOW - 1_000), "a newer event wins");
    let (s, _) = feed(s, vec![RoomMsg::Activity(CloudActivity { id: E.into(), finished: false, last: String::new(), last_at_ms: None, error: Some("boom".into()) })]);
    assert_eq!(s.cloud_row(E).unwrap().state(NOW), CState::Unreadable, "an error is still shown as one");
}

// ---- the renderSVG look, the logo band, the air between panels (s398) --------------------------------------------------------

use sessions_tui::theme::{nearest_256, Look};

fn look_room(w: u16, h: u16, air: bool, band: bool) -> RoomState {
    let mut s = RoomState::new("example-host");
    s.size = (w, h);
    s.air = air;
    s.band = band;
    feed(
        s,
        vec![
            RoomMsg::Tick(NOW),
            RoomMsg::Remote(RemoteSnapshot { rows: remote_rows(), status: RemoteStatus::Ready }),
            RoomMsg::Cloud(CloudSnapshot { rows: cloud_rows(), status: CloudStatus::Ready { tracked: 3 } }),
        ],
    )
    .0
}
fn rs_text(w: u16, h: u16) -> String {
    text(&look_room(w, h, true, false), &Theme::rendersvg())
}

#[test]
fn frames_in_both_themes_at_two_sizes() {
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        snap(&format!("room_classic_{w}x{h}"), text(&look_room(w, h, false, false), &Theme::color()).trim_end());
        snap(&format!("room_rendersvg_{w}x{h}"), text(&look_room(w, h, true, false), &Theme::rendersvg()).trim_end());
    }
}

#[test]
fn the_classic_look_draws_exactly_what_it_drew() {
    // Classic never has air or a band, whatever the state flags would say elsewhere: the existing 100x24 snapshots pass unchanged,
    // and the same state in Classic with air off has panels touching.
    let s = look_room(120, 40, false, false);
    let l = layout(Rect::new(0, 0, 120, 40), &s);
    assert_eq!(l.cloud.y, l.remote.y + l.remote.height, "no blank row between panels");
    assert!(!Theme::color().air() && !Theme::plain().air());
}

#[test]
fn rendersvg_puts_one_blank_row_between_panels_where_the_height_allows() {
    let s = look_room(120, 40, true, false);
    let l = layout(Rect::new(0, 0, 120, 40), &s);
    assert_eq!(l.cloud.y, l.remote.y + l.remote.height + 1, "one blank row after the Remote Control panel");
    assert_eq!(l.detail.unwrap().y, l.cloud.y + l.cloud.height + 1, "and after the Cloud panel");
    assert_eq!(l.footer.y, 39);
    // the click map follows: every row still maps to a rectangle inside its panel
    assert!(l.rows.iter().all(|(r, _)| r.y > l.remote.y && r.y < l.detail.unwrap().y));
    // a short terminal gets no air (the lists come first)
    let short = look_room(80, 14, true, false);
    let l = layout(Rect::new(0, 0, 80, 14), &short);
    assert_eq!(l.cloud.y, l.remote.y + l.remote.height);
    // with two extra panels too
    let mut s = with_queue(with_builds(with_specs(look_room(120, 56, true, false))));
    s.air = true;
    let l = layout(Rect::new(0, 0, 120, 56), &s);
    for (a, b) in [(l.remote, l.cloud), (l.cloud, l.panels[0]), (l.panels[0], l.panels[1])] {
        assert_eq!(b.y, a.y + a.height + 1, "{a:?} then {b:?}");
    }
}

#[test]
fn rendersvg_colors_are_the_midnight_tokens() {
    let th = Theme::rendersvg();
    let s = feed(look_room(120, 40, true, false), vec![RoomMsg::Tick(NOW)]).0;
    let ansi = render_text(&s, &th, 120, 40, true);
    for (what, code) in [
        ("panel border (node-line #333)", "38;2;51;51;51"),
        ("selected row text (local #2dd4bf)", "38;2;45;212;191"),
        ("selected row background (local-soft #0b3d33)", "48;2;11;61;51"),
        ("secondary text (ink-3 #737373)", "38;2;115;115;115"),
        ("header band (bg-2 #111)", "48;2;17;17;17"),
        ("close button (rose #fb7185)", "38;2;251;113;133"),
        ("waiting (frontier #fbbf24)", "38;2;251;191;36"),
        ("body text (ink-2 #a3a3a3)", "38;2;163;163;163"),
    ] {
        assert!(ansi.contains(code), "{what}: {code} missing");
    }
    // Classic keeps its own set: none of the renderSVG surfaces
    let classic = render_text(&look_room(120, 40, false, false), &Theme::color(), 120, 40, true);
    for code in ["48;2;11;61;51", "48;2;17;17;17", "38;2;51;51;51"] {
        assert!(!classic.contains(code), "classic must not use {code}");
    }
}

#[test]
fn without_truecolor_the_tokens_fall_back_to_the_nearest_256_color_entry() {
    let th = Theme { color: true, truecolor: false, ascii: false, look: Look::RenderSvg };
    let ansi = render_text(&look_room(120, 40, true, false), &th, 120, 40, true);
    assert!(!ansi.contains("38;2;"), "no truecolor sequences without truecolor");
    for token in [(0x2du8, 0xd4u8, 0xbfu8), (0x0b, 0x3d, 0x33), (0x33, 0x33, 0x33), (0x73, 0x73, 0x73), (0xfb, 0xbf, 0x24), (0xfb, 0x71, 0x85), (0x11, 0x11, 0x11), (0xa3, 0xa3, 0xa3)] {
        // brute force over the xterm-256 cube and gray ramp
        let levels = [0u8, 95, 135, 175, 215, 255];
        let mut table: Vec<(u8, (u8, u8, u8))> = Vec::new();
        for i in 0..216u16 {
            table.push((16 + i as u8, (levels[(i / 36) as usize], levels[((i / 6) % 6) as usize], levels[(i % 6) as usize])));
        }
        for g in 0..24u8 {
            table.push((232 + g, (8 + 10 * g, 8 + 10 * g, 8 + 10 * g)));
        }
        let dist = |c: (u8, u8, u8)| {
            let (dr, dg, db) = (c.0 as i32 - token.0 as i32, c.1 as i32 - token.1 as i32, c.2 as i32 - token.2 as i32);
            dr * dr + dg * dg + db * db
        };
        let best = table.iter().map(|(_, c)| dist(*c)).min().unwrap();
        let got = table.iter().find(|(i, _)| *i == nearest_256(token)).unwrap().1;
        assert_eq!(dist(got), best, "{token:?} -> {}", nearest_256(token));
    }
    assert!(ansi.contains(&format!("38;5;{}", nearest_256((0x2d, 0xd4, 0xbf)))), "the local teal as an indexed color");
    assert!(ansi.contains(&format!("48;5;{}", nearest_256((0x0b, 0x3d, 0x33)))), "the selection background as an indexed color");
}

#[test]
fn no_color_and_ascii_are_untouched_by_the_look() {
    // a theme that asks for renderSVG but has no color (NO_COLOR, TERM=dumb) styles every role exactly like the plain theme
    let nc = Theme { color: false, truecolor: false, ascii: true, look: Look::RenderSvg };
    let p = Theme::plain();
    assert_eq!((nc.accent(), nc.accent2(), nc.dim(), nc.error(), nc.warn(), nc.ok(), nc.code(), nc.selected(), nc.row_bg()), (p.accent(), p.accent2(), p.dim(), p.error(), p.warn(), p.ok(), p.code(), p.selected(), p.row_bg()));
    assert_eq!((nc.title(), nc.panel_title(true), nc.panel_title(false), nc.brand(), nc.close_style(), nc.body(), nc.band(), nc.panel_border(true), nc.panel_border(false)), (p.title(), p.panel_title(true), p.panel_title(false), p.brand(), p.close_style(), p.body(), p.band(), p.panel_border(true), p.panel_border(false)));
    assert!(!nc.air());
    let a = render_text(&look_room(100, 24, false, false), &nc, 100, 24, true);
    let b = render_text(&look_room(100, 24, false, false), &p, 100, 24, true);
    assert_eq!(a, b, "a renderSVG request without color draws the plain frame");
    assert!(!a.contains("38;") && !a.contains("48;"), "and no color is set (bold / reverse only)");
}

#[test]
fn the_logo_band_is_three_rows_with_blank_image_cells_and_the_header_to_their_right() {
    let th = Theme::rendersvg();
    let s = look_room(110, 34, true, true);
    let l = layout(Rect::new(0, 0, 110, 34), &s);
    assert_eq!((l.header.y, l.header.height), (0, 3));
    assert_eq!((l.close.y, l.close.x + l.close.width), (0, 110), "the close button stays at the right end of the first row");
    assert!(l.remote.y >= 3, "the panels start under the band: {:?}", l.remote);
    assert_eq!(l.footer.y, 33);
    let frame = text(&s, &th);
    let rows: Vec<&str> = frame.lines().collect();
    for (y, line) in rows.iter().take(3).enumerate() {
        assert!(line.chars().take(8).all(|c| c == ' '), "row {y}: the image's cells are blank: {line:?}");
    }
    assert!(rows[0].contains("remote-sessions") && rows[0].contains("control room") && rows[0].contains("close"), "{:?}", rows[0]);
    assert_eq!(rows[0].find("remote-sessions"), Some(9), "the header text starts right of the image cells");
    assert!(rows[1].contains("remote 3") && rows[1].contains("cloud 3"), "{:?}", rows[1]);
    // the one-row header it replaces is not drawn twice
    assert_eq!(frame.matches("control room").count(), 1);
    // a click on the close button and on a row still work through the layout
    assert!(l.rows.iter().all(|(r, _)| r.y >= 3));
    // too short for a band and the lists: the plain header
    let short = look_room(80, 12, true, true);
    assert_eq!(layout(Rect::new(0, 0, 80, 12), &short).header.height, 1);
    // no band flag: today's one-row header, byte for byte the same as before
    let plain_hdr = text(&look_room(110, 34, true, false), &th);
    assert!(plain_hdr.lines().next().unwrap().trim_start().starts_with("remote-sessions"), "{:?}", plain_hdr.lines().next());
}

#[test]
fn no_new_glyphs_anywhere_in_the_screens_sources() {
    // The set of non-ASCII characters in sessions-tui/src/*.rs at the base of this work (S396, 2a7dbf9a). The look adds none.
    let base: Vec<char> = "\u{b7}\u{2022}\u{2026}\u{203a}\u{2191}\u{2193}\u{2500}\u{2502}\u{250c}\u{2510}\u{2514}\u{2518}\u{2570}\u{258c}\u{258f}\u{25b6}\u{25c6}\u{25cb}\u{25cf}\u{25d0}\u{2713}\u{2715}\u{280b}\u{2819}\u{2826}\u{2827}\u{2834}\u{2838}\u{2839}\u{283c}\u{3002}\u{3044}\u{3059}\u{3067}\u{306e}\u{30ad}\u{30b9}\u{30c6}\u{30c8}\u{65e5}\u{672c}\u{884c}\u{8a9e}\u{9577}".chars().collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut seen = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            for c in std::fs::read_to_string(&path).unwrap().chars().filter(|c| !c.is_ascii()) {
                seen.insert((c, path.file_name().unwrap().to_string_lossy().to_string()));
            }
        }
    }
    for (c, file) in &seen {
        assert!(base.contains(c), "new glyph {c:?} (U+{:04X}) in {file}", *c as u32);
    }
    // and a rendered renderSVG frame uses only glyphs the Classic frame already uses
    let classic: std::collections::BTreeSet<char> = text(&look_room(120, 40, false, false), &Theme::color()).chars().filter(|c| !c.is_ascii()).collect();
    let rs: std::collections::BTreeSet<char> = rs_text(120, 40).chars().filter(|c| !c.is_ascii()).collect();
    assert!(rs.is_subset(&classic), "renderSVG frame glyphs not in the classic frame: {:?}", rs.difference(&classic).collect::<Vec<_>>());
    let band: std::collections::BTreeSet<char> = text(&look_room(120, 40, true, true), &Theme::rendersvg()).chars().filter(|c| !c.is_ascii()).collect();
    assert!(band.is_subset(&classic), "band frame glyphs not in the classic frame: {:?}", band.difference(&classic).collect::<Vec<_>>());
}
