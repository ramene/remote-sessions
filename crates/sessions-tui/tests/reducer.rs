//! The reducer: pure state transitions, fixtures in the shapes the core emits (fake ids only).
mod common;
use common::*;
use crossterm::event::KeyCode;
use serde_json::json;
use sessions_tui::model::*;

fn users(s: &State) -> Vec<(String, Mark)> {
    s.blocks.iter().filter_map(|b| if let BlockKind::User { text, mark } = &b.kind { Some((text.clone(), *mark)) } else { None }).collect()
}
fn notices(s: &State) -> Vec<String> {
    s.blocks.iter().filter_map(|b| if let BlockKind::Notice { text, .. } = &b.kind { Some(text.clone()) } else { None }).collect()
}

#[test]
fn snapshot_loads_blocks_then_starts_the_stream_after_the_last_seq() {
    let s = state();
    let (s, fx) = step(s, Msg::Snapshot(Ok(Snapshot { name: "n".into(), host: "mac-a".into(), events: vec![user(5, "u1", "hi"), assistant(9, "a1", "yo"), assistant(7, "a0", "earlier")], truncated: true })));
    assert_eq!(fx, vec![Effect::StartStream { from: Some(9) }], "resume from max seq, whatever the order");
    assert_eq!(s.blocks.len(), 4, "3 events + the truncation notice");
    assert_eq!(s.conn, Conn::Connecting, "live only once the stream opens");
    let (s, _) = step(s, Msg::Open);
    assert_eq!(s.conn, Conn::Live);
    assert_eq!(s.turn, Turn::Idle, "replayed history never makes the session look busy");
}

#[test]
fn dedup_across_stream_and_poll() {
    let s = live(vec![user(1, "u1", "hi")]);
    let s = feed(s, vec![assistant(2, "a1", "one"), assistant(2, "a1", "one"), user(1, "u1", "hi")]);
    assert_eq!(s.blocks.len(), 2);
    let s = step(s, Msg::Item(Item::Event(assistant(2, "a1", "one")))).0;
    assert_eq!(s.blocks.len(), 2);
    assert_eq!(s.last_seq, Some(2));
}

#[test]
fn optimistic_echo_is_replaced_by_uuid_and_by_text() {
    let (s, fx) = submit(live(vec![]), "run the tests");
    let (uuid, text) = said(&fx);
    let uuid = &uuid;
    assert_eq!(text, "run the tests");
    assert_eq!(users(&s), vec![("run the tests".into(), Mark::Local)]);
    let (s, _) = step(s, Msg::Net(Net::Say { uuid: uuid.clone(), res: Ok(()) }));
    assert_eq!(users(&s)[0].1, Mark::Sent);
    assert!(matches!(s.turn, Turn::Waiting { .. }));
    // the server echo carries our uuid: replaced in place
    let s2 = feed(s.clone(), vec![user(3, uuid, "run the tests")]);
    assert_eq!(s2.blocks.len(), 1);
    // an echo with a different uuid but the same text still reconciles
    let s3 = feed(s, vec![user(3, "server-made-uuid", "run the tests")]);
    assert_eq!(s3.blocks.len(), 1);
    assert_eq!(s3.blocks[0].key, "server-made-uuid");
    // receipts move the marker
    let s4 = step(s2, Msg::Item(Item::Receipt { event_id: uuid.clone(), status: "PROCESSED".into() })).0;
    assert_eq!(users(&s4)[0].1, Mark::Processed);
}

#[test]
fn failed_send_marks_the_block_and_says_so() {
    let (s, fx) = submit(live(vec![]), "hello");
    let (uuid, _) = said(&fx);
    let uuid = &uuid;
    let (s, _) = step(s, Msg::Net(Net::Say { uuid: uuid.clone(), res: Err("no route".into()) }));
    assert_eq!(users(&s)[0].1, Mark::Failed);
    assert!(notices(&s).iter().any(|n| n.contains("send failed: no route")));
    assert_eq!(s.turn, Turn::Idle);
}

#[test]
fn sends_queue_while_connecting_and_flush_with_the_snapshot() {
    let (s, fx) = submit(state(), "early message");
    assert!(fx.iter().all(|e| matches!(e, Effect::SaveHistory(_))), "nothing is sent before the first snapshot");
    assert_eq!(s.queued.len(), 1);
    let (s, fx) = step(s, Msg::Snapshot(Ok(Snapshot::default())));
    assert!(matches!(&fx[..], [Effect::StartStream { from: None }, Effect::Say { text, .. }] if text == "early message"));
    let _ = said(&fx);
    assert!(s.queued.is_empty());
}

#[test]
fn turn_goes_waiting_working_idle() {
    let (s, fx) = submit(live(vec![]), "go");
    let (uuid, _) = said(&fx);
    let s = step(s, Msg::Net(Net::Say { uuid: uuid.clone(), res: Ok(()) })).0;
    let s = step(s, Msg::Tick(5_000)).0;
    assert!(matches!(s.turn, Turn::Waiting { .. }));
    let s = feed(s, vec![thinking(2, "t1")]);
    assert!(matches!(s.turn, Turn::Working { .. }), "a thinking-only event still counts as activity");
    assert_eq!(s.blocks.len(), 1, "and draws nothing");
    let s = feed(s, vec![tool_use(3, "a1", "toolu_ex1", "Bash", json!({"command": "ls"}))]);
    assert!(matches!(&s.turn, Turn::Working { tool: Some(t), .. } if t == "Bash"));
    let s = feed(s, vec![result(4, "r1", "success", 1, 61_000)]);
    assert_eq!(s.turn, Turn::Idle);
    assert!(matches!(&s.blocks.last().unwrap().kind, BlockKind::Divider(t) if t == "turn done · 1m 1s · 1 turn"));
}

#[test]
fn a_result_with_an_error_subtype_adds_a_warning() {
    let s = feed(live(vec![]), vec![result(1, "r1", "error_max_turns", 9, 5_000)]);
    assert!(notices(&s).iter().any(|n| n == "turn ended with error_max_turns"));
}

#[test]
fn tool_results_attach_to_their_call_or_stand_alone() {
    let s = feed(live(vec![]), vec![tool_use(1, "a1", "toolu_ex1", "Bash", json!({"command": "ls"})), tool_result(2, "u1", "toolu_ex1", "out", false), tool_result(3, "u2", "toolu_missing", "orphan", true)]);
    assert_eq!(s.blocks.len(), 2);
    assert!(matches!(&s.blocks[0].kind, BlockKind::Tool { result: Some(r), .. } if r.text == "out" && !r.is_error));
    assert!(matches!(&s.blocks[1].kind, BlockKind::Result(r) if r.is_error));
}

#[test]
fn system_init_sets_the_mode_and_unknown_types_get_a_notice() {
    let s = feed(live(vec![]), vec![init(1, "i1", "acceptEdits"), note(2, "x1", "brand_new_thing", "· brand_new_thing"), note(3, "x2", "autocompact_state", "· autocompact"), note(4, "x3", "keep_alive", "")]);
    assert_eq!(s.mode.as_deref(), Some("acceptEdits"));
    assert!(notices(&s).contains(&"brand_new_thing".to_string()));
    assert!(!notices(&s).iter().any(|n| n.contains("autocompact")), "noise stays hidden");
    let s = feed(s, vec![note(5, "l1", "env_manager_log", "· env[info] allocating")]);
    assert!(!notices(&s).iter().any(|n| n.contains("allocating")));
    let s = submit(s, "/log").0;
    let s = feed(s, vec![note(6, "l2", "env_manager_log", "· env[info] allocating")]);
    assert!(notices(&s).iter().any(|n| n.contains("allocating")), "/log makes it visible");
}

#[test]
fn rate_limit_notices_only_when_the_status_changes() {
    let s = feed(live(vec![]), vec![note(1, "r1", "rate_limit_event", "· rate_limit: allowed (seven_day)"), note(2, "r2", "rate_limit_event", "· rate_limit: allowed (seven_day)"), note(3, "r3", "rate_limit_event", "· rate_limit: warning (seven_day)")]);
    assert_eq!(notices(&s).len(), 2);
}

#[test]
fn permission_prompt_opens_closes_and_says_when_answered_elsewhere() {
    let s = live(vec![]);
    let s = feed(s, vec![ask(1, "c1", "req_ex1", "WebFetch", fake_input())]);
    assert!(matches!(s.modal, Modal::Permission { choice: 0, .. }));
    assert_eq!(s.prompts.len(), 1);
    let s2 = feed(s.clone(), vec![response(2, "c2", "req_ex1")]);
    assert_eq!(s2.modal, Modal::None);
    assert!(notices(&s2).contains(&"answered elsewhere".to_string()));
    let s3 = feed(s.clone(), vec![cancel(2, "c3", "req_ex1")]);
    assert_eq!((s3.modal.clone(), s3.prompts.len()), (Modal::None, 0));
    assert!(notices(&s3).contains(&"answered elsewhere".to_string()));
    // a prompt already answered in the snapshot never opens
    let s4 = live(vec![ask(1, "c1", "req_ex1", "WebFetch", fake_input()), response(2, "c2", "req_ex1")]);
    assert_eq!((s4.modal, s4.prompts.len()), (Modal::None, 0));
    // pending in the snapshot opens the modal at attach
    assert!(matches!(live(vec![ask(1, "c1", "req_ex1", "Bash", json!({"command": "ls"}))]).modal, Modal::Permission { .. }));
}

#[test]
fn queued_prompts_show_in_order_and_the_modal_resets_for_the_next() {
    let s = live(vec![ask(1, "c1", "req_ex1", "Bash", json!({"command": "a"})), ask(2, "c2", "req_ex2", "Bash", json!({"command": "b"}))]);
    assert_eq!(s.prompts.len(), 2);
    let s = press(s, vec![key(KeyCode::Down)]).0;
    assert!(matches!(s.modal, Modal::Permission { choice: 1, .. }));
    let s = feed(s, vec![response(3, "c3", "req_ex1")]);
    assert_eq!(s.prompts[0].request_id, "req_ex2");
    assert!(matches!(s.modal, Modal::Permission { choice: 0, .. }), "fresh choice for the next prompt");
}

#[test]
fn allow_once_enter_answers_and_our_own_answer_is_not_elsewhere() {
    let s = live(vec![ask(1, "c1", "req_ex1", "WebFetch", fake_input())]);
    let (s, fx) = press(s, vec![key(KeyCode::Enter)]);
    assert_eq!(fx, vec![Effect::Answer { request_id: "req_ex1".into(), decision: Decision::Allow }]);
    assert!(matches!(s.modal, Modal::Permission { sending: true, .. }));
    // further keys are ignored while sending
    let (s, fx2) = press(s, vec![key(KeyCode::Esc), key(KeyCode::Char('2')), key(KeyCode::Enter)]);
    assert!(fx2.is_empty());
    // the server's control_response arrives before our POST returns
    let s = feed(s, vec![response(2, "c2", "req_ex1")]);
    assert!(!notices(&s).contains(&"answered elsewhere".to_string()));
    assert_eq!(s.modal, Modal::None);
    let s = step(s, Msg::Net(Net::Answer { request_id: "req_ex1".into(), allow: true, res: Ok(()) })).0;
    assert!(notices(&s).contains(&"allowed once".to_string()));
}

#[test]
fn deny_paths_digit_two_esc_and_a_typed_reason() {
    let base = live(vec![ask(1, "c1", "req_ex1", "WebFetch", fake_input())]);
    let (_, fx) = press(base.clone(), vec![key(KeyCode::Char('2')), key(KeyCode::Enter)]);
    assert_eq!(fx, vec![Effect::Answer { request_id: "req_ex1".into(), decision: Decision::Deny(None) }]);
    let (_, fx) = press(base.clone(), vec![key(KeyCode::Esc)]);
    assert_eq!(fx, vec![Effect::Answer { request_id: "req_ex1".into(), decision: Decision::Deny(None) }]);
    let mut keys = vec![key(KeyCode::Char('3'))];
    keys.extend("not now".chars().map(|c| key(KeyCode::Char(c))));
    keys.push(key(KeyCode::Enter));
    let (s, fx) = press(base.clone(), keys);
    assert_eq!(fx, vec![Effect::Answer { request_id: "req_ex1".into(), decision: Decision::Deny(Some("not now".into())) }]);
    assert!(s.input.text.is_empty(), "nothing leaked into the prompt");
    // an empty reason uses the default message (None)
    let (_, fx) = press(base.clone(), vec![key(KeyCode::Tab), key(KeyCode::Enter)]);
    assert_eq!(fx, vec![Effect::Answer { request_id: "req_ex1".into(), decision: Decision::Deny(None) }]);
    // Esc inside the field only closes the field
    let (s, fx) = press(base, vec![key(KeyCode::Tab), key(KeyCode::Char('x')), key(KeyCode::Esc)]);
    assert!(fx.is_empty() && matches!(s.modal, Modal::Permission { reason: None, .. }));
}

#[test]
fn an_answer_the_server_refuses_closes_the_modal_with_the_error() {
    let s = live(vec![ask(1, "c1", "req_ex1", "WebFetch", fake_input())]);
    let (s, _) = press(s, vec![key(KeyCode::Enter)]);
    let s = step(s, Msg::Net(Net::Answer { request_id: "req_ex1".into(), allow: true, res: Err("No pending permission prompt with request id req_ex1".into()) })).0;
    assert_eq!((s.modal.clone(), s.prompts.len()), (Modal::None, 0));
    assert!(notices(&s).iter().any(|n| n.starts_with("could not answer: No pending")));
}

#[test]
fn esc_interrupts_only_while_busy_and_double_esc_clears() {
    let idle = type_text(live(vec![]), "draft");
    let (s, fx) = press(idle, vec![key(KeyCode::Esc)]);
    assert!(fx.is_empty());
    assert_eq!(s.input.text, "draft", "one Esc on an idle prompt does nothing");
    let (s, _) = press(s, vec![key(KeyCode::Esc)]);
    assert_eq!(s.input.text, "", "a second Esc soon after clears the input");
    let mut busy = live(vec![]);
    busy.turn = Turn::Working { since: 0, tool: None };
    let (busy, fx) = press(busy, vec![key(KeyCode::Esc)]);
    assert_eq!(fx, vec![Effect::Interrupt]);
    let s = step(busy, Msg::Net(Net::Interrupt(Ok(())))).0;
    assert_eq!(s.turn, Turn::Idle);
    assert!(notices(&s).contains(&"interrupted by you".to_string()));
}

#[test]
fn ctrl_c_only_leaves() {
    for s in [live(vec![]), { let mut b = live(vec![]); b.turn = Turn::Waiting { since: 0 }; b }, live(vec![ask(1, "c1", "req_ex1", "Bash", json!({"command": "ls"}))])] {
        let (s, fx) = press(s, vec![ctrl('c')]);
        assert_eq!(fx, vec![Effect::Quit]);
        assert!(s.quit);
    }
}

#[test]
fn tab_cycles_manual_accept_edits_plan_and_never_reaches_bypass() {
    let mut mode = None;
    let mut seen = Vec::new();
    for _ in 0..6 {
        let mut s = live(vec![]);
        s.mode = mode.clone();
        let (_, fx) = press(s, vec![key(KeyCode::Tab)]);
        let Effect::SetMode(m) = &fx[0] else { panic!() };
        seen.push(m.clone());
        mode = Some(m.clone());
    }
    assert_eq!(seen, ["acceptEdits", "plan", "default", "acceptEdits", "plan", "default"]);
    for from in ["auto", "dontAsk", "bypassPermissions"] {
        assert_eq!(next_mode(Some(from)), "default");
    }
    assert!(!seen.iter().any(|m| m == "bypassPermissions" || m == "auto"));
    let s = step(live(vec![]), Msg::Net(Net::Mode { mode: "plan".into(), res: Ok(()) })).0;
    assert_eq!(s.mode.as_deref(), Some("plan"));
}

#[test]
fn slash_commands() {
    let mut s = live(vec![ask(1, "c1", "req_ex1", "Bash", json!({"command": "ls"}))]);
    s.modal = Modal::None;
    let (_, fx) = submit(s.clone(), "/approve");
    assert_eq!(fx.iter().filter(|e| matches!(e, Effect::Answer { decision: Decision::Allow, .. })).count(), 1);
    let (_, fx) = submit(s.clone(), "/deny too risky");
    assert!(fx.contains(&Effect::Answer { request_id: "req_ex1".into(), decision: Decision::Deny(Some("too risky".into())) }));
    let (s2, fx) = submit(live(vec![]), "/approve");
    assert!(!fx.iter().any(|e| matches!(e, Effect::Answer { .. })));
    assert!(notices(&s2).contains(&"no pending permission prompt".to_string()));
    let (_, fx) = submit(live(vec![]), "/mode accept-edits");
    assert!(fx.contains(&Effect::SetMode("acceptEdits".into())));
    let (_, fx) = submit(live(vec![]), "/mode bypass");
    assert!(fx.contains(&Effect::SetMode("bypassPermissions".into())), "reachable by /mode only");
    let (p, _) = submit(live(vec![]), "/mode");
    assert!(matches!(p.modal, Modal::ModePicker { .. }));
    let (_, fx) = press(p, vec![key(KeyCode::Char('4')), key(KeyCode::Enter)]);
    assert_eq!(fx, vec![Effect::SetMode("plan".into())]);
    let (_, fx) = submit(live(vec![]), "/interrupt");
    assert!(fx.contains(&Effect::Interrupt));
    let (q, fx) = submit(live(vec![]), "/quit");
    assert!(q.quit && fx.contains(&Effect::Quit));
    let (u, fx) = submit(live(vec![]), "/nope");
    assert!(fx.iter().all(|e| !matches!(e, Effect::Say { .. })));
    assert!(notices(&u).iter().any(|n| n.starts_with("unknown command /nope")));
    let (_, fx) = submit(live(vec![]), "//literal slash");
    assert_eq!(said(&fx).1, "/literal slash");
    let (r, _) = submit(live(vec![assistant(1, "a1", "one"), assistant(2, "a2", "two")]), "/read 5");
    assert!(notices(&r).last().unwrap().contains("assistant> two"));
    let (m, fx) = submit(live(vec![]), "/mouse off");
    assert!(!m.mouse && fx.contains(&Effect::Mouse(false)));
}

#[test]
fn tab_completes_slash_commands() {
    let s = type_text(live(vec![]), "/ap");
    let s = press(s, vec![key(KeyCode::Tab)]).0;
    assert_eq!(s.input.text, "/approve ");
    let s = type_text(live(vec![]), "/m");
    let s = press(s, vec![key(KeyCode::Tab)]).0;
    assert_eq!(s.input.text, "/mode");
    let s = press(s, vec![key(KeyCode::Tab)]).0;
    assert_eq!(s.input.text, "/mouse");
}

#[test]
fn input_editing_history_and_multiline() {
    let mut s = live(vec![]);
    for t in ["first", "second"] {
        s = submit(s, t).0;
    }
    assert_eq!(s.history, ["first", "second"]);
    let s = press(s, vec![key(KeyCode::Up)]).0;
    assert_eq!(s.input.text, "second");
    let s = press(s, vec![key(KeyCode::Up)]).0;
    assert_eq!(s.input.text, "first");
    let s = press(s, vec![key(KeyCode::Down), key(KeyCode::Down)]).0;
    assert_eq!(s.input.text, "", "back to the draft");
    // a leading space is never kept in history
    let (s, fx) = submit(live(vec![]), " secret");
    assert!(s.history.is_empty() && !fx.iter().any(|e| matches!(e, Effect::SaveHistory(_))));
    // backslash then Enter makes a newline; Up inside multi-line input moves the cursor first
    let s = type_text(live(vec![]), "a\\");
    let s = press(s, vec![key(KeyCode::Enter)]).0;
    assert_eq!(s.input.text, "a\n");
    let s = type_text(s, "bc");
    let s = press(s, vec![key(KeyCode::Up)]).0;
    assert_eq!(s.input.text, "a\nbc");
    assert_eq!(s.input.cursor, 1, "moved up a line, same column clamped");
    // ctrl-u / ctrl-k / ctrl-w / alt-b
    let s = type_text(live(vec![]), "one two three");
    let s = press(s, vec![ctrl('w')]).0;
    assert_eq!(s.input.text, "one two ");
    let s = press(s, vec![ctrl('u')]).0;
    assert_eq!(s.input.text, "");
    // paste is verbatim, never sent
    let (s, fx) = step(live(vec![]), Msg::Paste("line1\r\nline2\u{1b}[31m".into()));
    assert!(fx.is_empty());
    assert_eq!(s.input.text, "line1\nline2[31m");
}

#[test]
fn conn_goes_polling_on_failure_and_recovers_without_duplicates() {
    let s = live(vec![user(1, "u1", "hi"), assistant(4, "a1", "yo")]);
    let (s, fx) = step(s, Msg::StreamEnded(StreamEnd::Failed("closed".into())));
    assert_eq!((s.conn, fx), (Conn::Polling, vec![Effect::StartPoll]));
    assert!(notices(&s).iter().any(|n| n == "live stream unavailable (closed); polling"));
    // polling brings an overlapping window: only the new event shows
    let s = feed(s, vec![user(1, "u1", "hi"), assistant(4, "a1", "yo"), assistant(5, "a2", "new")]);
    assert_eq!(s.blocks.iter().filter(|b| matches!(b.kind, BlockKind::Assistant(_))).count(), 2);
    // the retry asks for the stream from last_seq
    let (s, fx) = step(s, Msg::Retry);
    assert_eq!(fx, vec![Effect::StartStream { from: Some(5) }]);
    // it fails again: still polling, no second notice, no second poller
    let before = notices(&s).len();
    let (s, fx) = step(s, Msg::StreamEnded(StreamEnd::Failed("closed".into())));
    assert!(fx.is_empty());
    assert_eq!(notices(&s).len(), before);
    // it opens: live again and the poller stops
    let (s, fx) = step(s, Msg::Open);
    assert_eq!((s.conn, fx), (Conn::Live, vec![Effect::StopPoll]));
    // Retry while live does nothing
    assert!(step(s, Msg::Retry).1.is_empty());
}

#[test]
fn a_gone_session_ends_read_only() {
    let s = live(vec![]);
    let (s, _) = step(s, Msg::PollFailed("The sessions API: Not Found (HTTP 404)".into()));
    assert!(s.ended);
    let (s, fx) = submit(s, "anything");
    assert!(fx.is_empty() && s.input.text.is_empty() && users(&s).is_empty());
    let (_, fx) = press(s, vec![ctrl('c')]);
    assert_eq!(fx, vec![Effect::Quit]);
    // an ordinary poll error is only a notice, once
    let s = live(vec![]);
    let s = step(s, Msg::PollFailed("timeout".into())).0;
    let s = step(s, Msg::PollFailed("timeout".into())).0;
    assert_eq!(notices(&s).len(), 1);
    assert!(!s.ended);
}

#[test]
fn snapshot_failure_is_fatal_and_any_key_leaves() {
    let s = step(state(), Msg::Snapshot(Err("untrusted device".into()))).0;
    assert!(s.fatal.is_some());
    let (_, fx) = press(s, vec![key(KeyCode::Char('x'))]);
    assert_eq!(fx, vec![Effect::Quit]);
}

#[test]
fn hostile_text_is_cleaned_at_ingestion() {
    let s = feed(live(vec![]), vec![assistant(1, "a1", "x\u{1b}[31m\u{202e}y\u{7}z"), assistant(2, "a2", &"q".repeat(100_000))]);
    let BlockKind::Assistant(t) = &s.blocks[0].kind else { panic!() };
    assert_eq!(t, "x[31myz");
    let BlockKind::Assistant(big) = &s.blocks[1].kind else { panic!() };
    assert!(big.len() < 9_000, "capped at the event text limit");
}

#[test]
fn scrolling_pins_unpins_and_counts_new_blocks() {
    let mut s = live((0..30).map(|i| assistant(i + 1, &format!("a{i}"), "x")).collect());
    s.view_total = 100;
    s.view_h = 20;
    let s = press(s, vec![key(KeyCode::PageUp)]).0;
    assert_eq!(s.scroll, 19);
    let s = feed(s, vec![assistant(99, "n1", "new"), assistant(100, "n2", "new")]);
    assert_eq!((s.scroll, s.new_count), (19, 2), "the view does not move");
    let s = step(s, Msg::Wheel(1)).0;
    assert_eq!(s.scroll, 22);
    let s = press(s, vec![key(KeyCode::End)]).0;
    assert_eq!((s.scroll, s.new_count), (0, 0));
    let s = press(s, vec![key(KeyCode::Home)]).0;
    assert_eq!(s.scroll, 80, "clamped to total - height");
    let s = step(s, Msg::Wheel(-100)).0;
    assert_eq!(s.scroll, 0);
}

#[test]
fn ctrl_o_toggles_all_and_a_click_toggles_one() {
    let s = live(vec![tool_use(1, "a1", "toolu_ex1", "Bash", json!({"command": "ls"}))]);
    let id = s.blocks[0].id;
    let s = press(s, vec![ctrl('o')]).0;
    assert!(s.expand_all);
    let s = press(s, vec![ctrl('o')]).0;
    assert!(!s.expand_all);
    let s = step(s, Msg::Click(id)).0;
    assert!(s.expanded.contains(&id));
    assert!(!step(s, Msg::Click(id)).0.expanded.contains(&id));
}

#[test]
fn uuids_are_versioned_distinct_and_seeded() {
    let mut a = state();
    let (u1, u2) = (a.new_uuid(), a.new_uuid());
    assert_ne!(u1, u2);
    assert_eq!(u1.len(), 36);
    assert_eq!(&u1[14..15], "4");
    assert!(matches!(&u1[19..20], "8" | "9" | "a" | "b"));
    let mut b = state();
    assert_eq!(b.new_uuid(), u1, "deterministic for a seed");
    b.seed = 7;
    assert_ne!(b.new_uuid(), u2);
}

#[test]
fn modes_parse_and_label() {
    assert_eq!(parse_mode("accept-edits"), Some("acceptEdits"));
    assert_eq!(parse_mode("PLAN"), Some("plan"));
    assert_eq!(parse_mode("manual"), Some("default"));
    assert_eq!(parse_mode("nope"), None);
    for m in MODES {
        assert_ne!(mode_label(m), "?");
    }
}

#[test]
fn chatty_system_events_are_hidden_and_init_is_said_once() {
    let s = feed(live(vec![]), vec![
        init(1, "i1", "default"), init(2, "i2", "default"),
        note(3, "n1", "system", "· system background_tasks_changed"), note(4, "n2", "system", "· system task_summary"),
        note(5, "n3", "system", "· system permission_check_status: checking"), note(6, "n4", "system", "· system permission_check_status: checking"),
    ]);
    assert_eq!(notices(&s), ["session started · model-x", "system permission_check_status: checking"]);
    let s = submit(s, "/log").0;
    let s = feed(s, vec![note(7, "n5", "system", "· system task_summary")]);
    assert!(notices(&s).iter().any(|n| n == "system task_summary"));
}
