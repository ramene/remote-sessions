//! Renderer snapshots on ratatui's `TestBackend`: one per block renderer, the footer at five widths, the modal, the screen.
mod common;
use common::*;
use serde_json::json;
use sessions_tui::model::*;
use sessions_tui::view;
use sessions_tui::theme::Theme;

fn long_command() -> String {
    (1..=20).map(|i| format!("echo step-{i}")).collect::<Vec<_>>().join(" && \\\n  ")
}

#[test]
fn user_turn_plain_wrapped_and_marked() {
    let th = color();
    snap("user_short", &block_snapshot(BlockKind::User { text: "run the whole suite again".into(), mark: Mark::Local }, 40, &th, false));
    snap("user_wrapped_processed", &block_snapshot(BlockKind::User { text: "please look at the failing test in the stream reader and tell me what is wrong with it\nsecond line".into(), mark: Mark::Processed }, 30, &th, false));
    snap("user_sent_mark", &block_snapshot(BlockKind::User { text: "hello".into(), mark: Mark::Sent }, 20, &th, false));
}

#[test]
fn assistant_text_wrapping_cjk_fence_and_lists() {
    let th = color();
    snap("assistant_wrapped", &block_snapshot(BlockKind::Assistant("I checked the three failing tests. Two are timing flakes in the stream reader; the third is a real off-by-one in the resume path.".into()), 40, &th, false));
    snap("assistant_cjk", &block_snapshot(BlockKind::Assistant("日本語のテキストは幅が二倍になります。折り返しが正しいか確認します。".into()), 20, &th, false));
    snap("assistant_markdown", &block_snapshot(BlockKind::Assistant("Plan:\n- run **all** tests\n- read `stream.rs` and fix the loop that never ends because the counter is wrong\n```rust\nlet n = 1;\nlet m = n + 1;\n```\ndone".into()), 40, &th, false));
}

#[test]
fn tool_call_truncation_and_results() {
    let th = color();
    let call = |input: serde_json::Value, result: Option<ToolOut>| BlockKind::Tool { tool_id: "toolu_ex1".into(), name: "Bash".into(), input, result };
    snap("tool_call_short", &block_snapshot(call(json!({"command": "cargo test"}), None), 60, &th, false));
    snap("tool_call_truncated", &block_snapshot(call(json!({"command": "cargo test -p remote-sessions-core stream -- --nocapture --test-threads=1 and more words here"}), None), 50, &th, false));
    snap("tool_call_narrow_hides_args", &block_snapshot(call(json!({"command": "cargo test"}), None), 30, &th, false));
    let lines = (1..=38).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let res = Some(ToolOut { text: lines, is_error: false });
    snap("tool_result_folded", &block_snapshot(call(json!({"command": "ls"}), res.clone()), 50, &th, false));
    let few = Some(ToolOut { text: "a\nb\nc".into(), is_error: false });
    snap("tool_result_expanded", &block_snapshot(call(json!({"command": "ls"}), few), 50, &th, true));
    let err = Some(ToolOut { text: "no such file\nsecond".into(), is_error: true });
    snap("tool_result_error", &block_snapshot(call(json!({"command": "cat x"}), err), 50, &th, false));
    snap("tool_result_empty", &block_snapshot(call(json!({"command": "true"}), Some(ToolOut { text: String::new(), is_error: false })), 50, &th, false));
    snap("tool_result_orphan", &block_snapshot(BlockKind::Result(ToolOut { text: "orphan output".into(), is_error: false }), 40, &th, false));
}

#[test]
fn divider_and_notices() {
    let th = color();
    snap("divider", &block_snapshot(BlockKind::Divider("turn done · 2m 1s · 6 turns".into()), 60, &th, false));
    snap("notice_dim", &block_snapshot(BlockKind::Notice { text: "live stream unavailable (closed); polling".into(), level: Level::Dim }, 30, &th, false));
    snap("notice_error", &block_snapshot(BlockKind::Notice { text: "send failed: no route".into(), level: Level::Error }, 40, &th, false));
}

#[test]
fn no_color_and_ascii_variants() {
    let th = plain();
    let blocks = vec![
        BlockKind::User { text: "hello".into(), mark: Mark::Processed },
        BlockKind::Assistant("answer with **bold**".into()),
        BlockKind::Tool { tool_id: "t".into(), name: "Bash".into(), input: json!({"command": "ls"}), result: Some(ToolOut { text: "a\nb\nc".into(), is_error: false }) },
        BlockKind::Divider("turn done · 3s".into()),
    ];
    let mut out = String::new();
    for k in blocks {
        out.push_str(&block_snapshot(k, 40, &th, false));
        out.push('\n');
    }
    snap("ascii_blocks", out.trim_end());
    assert!(out.is_ascii(), "the ascii set has no non-ascii glyphs:\n{out}");
}

#[test]
fn footer_at_five_widths() {
    let mut s = live(vec![init(1, "i1", "default")]);
    s.rec_since = Some(0);
    s.now_ms = 42_000;
    let th = color();
    let mut out = String::new();
    for w in [120u16, 80, 60, 40, 24] {
        let line = view::footer_line(&s, w, &th);
        out.push_str(&format!("{w:>3}|{}\n", lines_snapshot(vec![line], w)));
    }
    snap("footer_widths", out.trim_end());
    for w in [120u16, 80, 60, 40, 24] {
        let text = lines_snapshot(vec![view::footer_line(&s, w, &th)], w);
        assert!(unicode_width::UnicodeWidthStr::width(text.as_str()) <= w as usize);
        assert!(text.contains("manual") && text.contains("live"), "mode and link survive at {w}: {text}");
    }
}

fn two_prompt_state(tool: &str, input: serde_json::Value) -> State {
    live(vec![ask(1, "c1", "req_ex1", tool, input), ask(2, "c2", "req_ex2", "WebFetch", fake_input())])
}

#[test]
fn permission_modal_screens() {
    let th = color();
    snap("modal_bash_long", &screen(&two_prompt_state("Bash", json!({"command": long_command()})), &th, 80, 30));
    snap("modal_bash_full", &screen(&press(two_prompt_state("Bash", json!({"command": long_command()})), vec![ctrl('o')]).0, &th, 80, 30));
    snap("modal_edit_file", &screen(&live(vec![ask(1, "c1", "req_ex1", "Edit", json!({"file_path": "/home/user/project/src/lib.rs", "old_string": "a\nb", "new_string": "c"}))]), &th, 80, 20));
    snap("modal_webfetch_json", &screen(&live(vec![ask(1, "c1", "req_ex1", "WebFetch", json!({"url": "https://example.com/", "prompt": "summarise"}))]), &th, 80, 20));
    let (s, _) = press(live(vec![ask(1, "c1", "req_ex1", "WebFetch", fake_input())]), vec![key(crossterm::event::KeyCode::Tab)]);
    let s = type_text(s, "too risky");
    snap("modal_reason_field", &screen(&s, &th, 80, 20));
    snap("modal_hostile_text", &screen(&live(vec![ask(1, "c1", "req_ex1", "Bash", json!({"command": "echo \u{1b}[31mred\u{202e}rtl\u{7}"}))]), &th, 60, 18));
}

#[test]
fn main_screen_and_states() {
    let th = color();
    let mut s = live(vec![
        init(1, "i1", "default"),
        user(2, "u1", "fix the flaky test"),
        assistant(3, "a1", "I checked the three failing tests. Two are timing flakes in the stream reader."),
        tool_use(4, "a2", "toolu_ex1", "Bash", json!({"command": "cargo test -p core stream -- --nocapture"})),
        tool_result(5, "u2", "toolu_ex1", &(1..=12).map(|i| format!("running {i}")).collect::<Vec<_>>().join("\n"), false),
        assistant(6, "a3", "214 passed, 0 failed."),
        result(7, "r1", "success", 2, 112_000),
    ]);
    snap("screen_idle_80x24", &screen(&s, &th, 80, 24));
    s = press(s, vec![ctrl('o')]).0;
    snap("screen_expanded_80x24", &screen(&s, &th, 80, 24));
    let s2 = type_text(live(vec![]), "hello\nworld");
    snap("screen_empty_placeholder_80x10", &screen(&live(vec![]), &th, 80, 10));
    snap("screen_typing_40x12", &screen(&s2, &th, 40, 12));
    // working state with spinner
    let mut w = live(vec![user(1, "u1", "go")]);
    w = step(w, Msg::Tick(1_000)).0;
    w = feed(w, vec![tool_use(2, "a1", "toolu_ex2", "Read", json!({"file_path": "/a/b.rs"}))]);
    w = step(w, Msg::Tick(15_300)).0;
    snap("screen_working_60x12", &screen(&w, &th, 60, 12));
    // connecting
    snap("screen_connecting_60x10", &screen(&state(), &th, 60, 10));
    // polling fallback
    let p = step(live(vec![]), Msg::StreamEnded(StreamEnd::Failed("closed".into()))).0;
    snap("screen_polling_70x10", &screen(&p, &th, 70, 10));
    // ended
    let e = step(live(vec![assistant(1, "a1", "bye")]), Msg::PollFailed("The sessions API: Not Found (HTTP 404)".into())).0;
    snap("screen_ended_60x10", &screen(&e, &th, 60, 10));
    // fatal
    let f = step(state(), Msg::Snapshot(Err("The Claude sessions API refused this machine as an untrusted device (HTTP 403).".into()))).0;
    snap("screen_fatal_70x12", &screen(&f, &th, 70, 12));
    // help and picker
    snap("screen_help_70x16", &screen(&press(live(vec![]), vec![key(crossterm::event::KeyCode::Char('?'))]).0, &th, 70, 16));
    snap("screen_mode_picker_50x14", &screen(&submit(live(vec![]), "/mode").0, &th, 50, 14));
    // scrolled up with new blocks
    let mut sc = live((0..40).map(|i| assistant(i + 1, &format!("a{i}"), &format!("message number {i}"))).collect());
    sc.view_total = 79;
    sc.view_h = 10;
    sc = press(sc, vec![key(crossterm::event::KeyCode::PageUp)]).0;
    sc = feed(sc, vec![assistant(100, "a100", "new one")]);
    snap("screen_scrolled_60x14", &screen(&sc, &th, 60, 14));
}

#[test]
fn narrow_and_too_small_terminals() {
    let th = color();
    let s = live(vec![user(1, "u1", "hi there"), assistant(2, "a1", "hello, how can I help you today with this"), tool_use(3, "a2", "toolu_ex1", "Bash", json!({"command": "ls -la /some/long/path"}))]);
    snap("screen_narrow_30x10", &screen(&s, &th, 30, 10));
    snap("screen_min_24x8", &screen(&s, &th, 24, 8));
    snap("screen_too_small", &screen(&s, &th, 20, 6));
}

#[test]
fn no_color_draws_without_any_color() {
    let th = Theme::detect(true, false);
    assert!(!th.color && th.ascii);
    let s = two_prompt_state("Bash", json!({"command": "ls"}));
    let buf = screen_buffer(&s, &th, 60, 20);
    for cell in buf.content() {
        assert_eq!(cell.fg, ratatui::style::Color::Reset, "fg in {:?}", cell.symbol());
        assert_eq!(cell.bg, ratatui::style::Color::Reset);
        assert!(cell.symbol().is_ascii(), "non-ascii glyph {:?}", cell.symbol());
    }
    snap("screen_no_color_modal_60x20", &buffer_text(&buf));
}

#[test]
fn rendering_never_panics_for_any_width() {
    let th = color();
    let kinds = vec![
        BlockKind::User { text: "日本語 text with a very long unbroken-word-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nnext".into(), mark: Mark::Sent },
        BlockKind::Assistant("**bold** `code` - bullet\n```\nfence line that is long\n```\n".repeat(3)),
        BlockKind::Tool { tool_id: "t".into(), name: "Bash".into(), input: json!({"command": "x".repeat(300)}), result: Some(ToolOut { text: "l\n".repeat(40), is_error: true }) },
        BlockKind::Result(ToolOut { text: "r".repeat(500), is_error: false }),
        BlockKind::Notice { text: "n".repeat(200), level: Level::Warn },
        BlockKind::Divider("d".repeat(300)),
    ];
    for w in 1..300u16 {
        for k in &kinds {
            let b = Block { id: 1, key: "k".into(), kind: k.clone() };
            for exp in [false, true] {
                let lines = view::render_block(&b, w, &th, exp);
                assert!(!lines.is_empty());
            }
        }
    }
    let busy = two_prompt_state("Bash", json!({"command": long_command()}));
    for (w, h) in (1..90u16).flat_map(|w| [(w, 1u16), (w, 7), (w, 9), (w, 24)]) {
        let _ = screen(&busy, &th, w, h);
        let _ = screen(&type_text(live(vec![assistant(1, "a", "x")]), &"y".repeat(500)), &th, w, h);
    }
}

#[test]
fn hostile_text_never_reaches_the_screen() {
    let th = color();
    let s = live(vec![assistant(1, "a1", "before \u{1b}]0;title\u{7} \u{1b}[2Jafter"), assistant(2, "a2", &"z".repeat(100_000))]);
    let text = screen(&s, &th, 80, 24);
    assert!(!text.contains('\u{1b}') && !text.contains('\u{7}'));
    let small = screen(&live(vec![assistant(1, "a1", "before \u{1b}]0;title\u{7} \u{1b}[2Jafter")]), &th, 80, 12);
    assert!(small.contains("before ]0;title [2Jafter"), "{small}");
}
