//! The logo band of the `attach --tui` screen: present only when asked for, never changing the screen without it.
mod common;
use common::*;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use sessions_tui::model::*;
use sessions_tui::theme::Theme;
use sessions_tui::view::{self, BAND_MIN_H, BAND_ROWS, BAND_TEXT_COL};

fn draw(s: &State, th: &Theme, w: u16, h: u16, band: bool) -> (Buffer, view::DrawInfo) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut info = None;
    term.draw(|f| info = Some(view::draw_in(f, s, th, band))).unwrap();
    (term.backend().buffer().clone(), info.unwrap())
}
fn row(b: &Buffer, y: u16) -> String {
    (0..b.area.width).map(|x| b[(x, y)].symbol().to_string()).collect()
}
fn session() -> State {
    live(vec![user(1, "u1", "fix the flaky parser test"), assistant(2, "a1", "Reading the failing test first.")])
}

#[test]
fn without_the_band_the_screen_is_exactly_what_draw_gives() {
    let th = plain();
    for (w, h) in [(80u16, 24u16), (100, 30), (40, 12), (24, 8)] {
        let (a, ai) = draw(&session(), &th, w, h, false);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut bi = None;
        term.draw(|f| bi = Some(view::draw(f, &session(), &th))).unwrap();
        assert_eq!(&a, term.backend().buffer(), "{w}x{h}: draw_in(false) is draw");
        let bi = bi.unwrap();
        assert_eq!((ai.total, ai.height, ai.rows), (bi.total, bi.height, bi.rows));
    }
}

#[test]
fn the_band_is_three_rows_with_blank_cells_for_the_image_and_the_text_to_its_right() {
    let th = plain();
    let mut s = session();
    s.conn = Conn::Live;
    let (b, info) = draw(&s, &th, 100, 30, true);
    for y in 0..BAND_ROWS {
        let line = row(&b, y);
        assert!(line.chars().take(BAND_TEXT_COL as usize).all(|c| c == ' '), "row {y}: the image's cells are blank: {line:?}");
    }
    let (l0, l1, l2) = (row(&b, 0), row(&b, 1), row(&b, 2));
    assert!(l0.trim_start().starts_with("remote-sessions v") && l0.contains("inside a Remote Control session"), "{l0:?}");
    assert_eq!(l0.find("remote-sessions"), Some(BAND_TEXT_COL as usize));
    assert!(l1.contains("session_01Ex") && l1.contains("mac-a"), "{l1:?}");
    assert!(l2.contains("two-way") && l2.contains("live"), "{l2:?}");
    // the rest is the same screen, moved down: the footer is still the last row, the transcript is three rows shorter
    let (plain_b, plain_info) = draw(&s, &th, 100, 30, false);
    assert_eq!(row(&b, 29), row(&plain_b, 29), "the footer stays on the last row");
    assert_eq!(info.height + BAND_ROWS as usize, plain_info.height);
    // click rows keep screen-row indexing: the band's rows belong to no block
    assert_eq!(info.rows.len(), BAND_ROWS as usize + info.height);
    assert!(info.rows[..BAND_ROWS as usize].iter().all(|r| r.is_none()));
    assert!(info.rows.iter().any(|r| r.is_some()), "the transcript is clickable below the band");
}

#[test]
fn the_state_shows_in_the_band_and_a_small_terminal_drops_it() {
    let th = plain();
    let mut s = session();
    s.conn = Conn::Reconnecting(2);
    assert!(row(&draw(&s, &th, 100, 30, true).0, 2).contains("reconnecting"));
    s.conn = Conn::Offline;
    assert!(row(&draw(&s, &th, 100, 30, true).0, 2).contains("offline"));
    s.ended = true;
    assert!(row(&draw(&s, &th, 100, 30, true).0, 2).contains("ended"));
    // too short for a band and a usable transcript: the screen is the plain one
    let h = BAND_MIN_H - 1;
    let (small, _) = draw(&session(), &th, 80, h, true);
    let (base, _) = draw(&session(), &th, 80, h, false);
    assert_eq!(small, base);
    // hostile session names are cleaned before they are drawn
    let evil = State::new("sess\u{1b}[31mion\u{7}\nname", "h\u{1b}[2Jost");
    let l1 = row(&draw(&evil, &th, 100, 30, true).0, 1);
    assert!(!l1.contains('\u{1b}') && !l1.contains('\u{7}'), "{l1:?}");
}
