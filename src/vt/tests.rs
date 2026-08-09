use super::*;

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, 1_000_000)
}

fn plain(term: &Terminal) -> String {
    String::from_utf8(term.serialize(Format::Plain).unwrap()).unwrap()
}

fn screen_lines(term: &Terminal) -> Vec<String> {
    plain(term).lines().map(|s| s.to_string()).collect()
}

#[test]
fn prints_simple_text() {
    let mut t = term(20, 4);
    t.feed(b"hello");
    assert_eq!(screen_lines(&t), vec!["hello", "", "", ""]);
    assert_eq!(
        t.cursor(),
        Cursor {
            x: 5,
            y: 0,
            pending_wrap: false
        }
    );
}

#[test]
fn crlf_moves_to_next_line() {
    let mut t = term(20, 4);
    t.feed(b"one\r\ntwo");
    assert_eq!(screen_lines(&t), vec!["one", "two", "", ""]);
}

#[test]
fn wraps_at_last_column() {
    let mut t = term(5, 3);
    t.feed(b"abcdefg");
    assert_eq!(screen_lines(&t), vec!["abcde", "fg", ""]);
    assert_eq!(
        t.cursor(),
        Cursor {
            x: 2,
            y: 1,
            pending_wrap: false
        }
    );
}

#[test]
fn pending_wrap_holds_cursor_at_last_column() {
    let mut t = term(5, 3);
    t.feed(b"abcde");
    assert_eq!(
        t.cursor(),
        Cursor {
            x: 4,
            y: 0,
            pending_wrap: true
        }
    );
    // CR clears pending wrap.
    t.feed(b"\r");
    assert_eq!(
        t.cursor(),
        Cursor {
            x: 0,
            y: 0,
            pending_wrap: false
        }
    );
}

#[test]
fn scrolls_into_scrollback() {
    let mut t = term(10, 2);
    t.feed(b"one\r\ntwo\r\nthree");
    assert_eq!(plain(&t), "one\ntwo\nthree\n");
    assert_eq!(screen_lines(&t), vec!["one", "two", "three"]);
    // Visible screen is the last 2 rows.
    assert_eq!(t.cursor().y, 1);
}

#[test]
fn cursor_positioning() {
    let mut t = term(10, 5);
    t.feed(b"\x1b[3;4Hx");
    assert_eq!(screen_lines(&t)[2], "   x");
}

#[test]
fn erase_display_below() {
    let mut t = term(10, 3);
    t.feed(b"aaa\r\nbbb\r\nccc");
    t.feed(b"\x1b[2;2H\x1b[J");
    assert_eq!(screen_lines(&t), vec!["aaa", "b", ""]);
}

#[test]
fn erase_line_variants() {
    let mut t = term(10, 1);
    t.feed(b"abcdefghij");
    t.feed(b"\x1b[5G\x1b[1K"); // erase to start, cursor at col 5
    assert_eq!(screen_lines(&t), vec!["     fghij"]);
    t.feed(b"\x1b[8G\x1b[0K"); // erase to end from col 8
    assert_eq!(screen_lines(&t), vec!["     fg"]);
}

#[test]
fn scroll_region_confines_scrolling() {
    let mut t = term(10, 4);
    t.feed(b"top\r\nA\r\nB\r\nbot");
    // Region rows 2..3 (1-based), cursor to bottom of region, then LF scrolls
    // only inside the region.
    t.feed(b"\x1b[2;3r\x1b[3;1H\nNEW");
    assert_eq!(screen_lines(&t), vec!["top", "B", "NEW", "bot"]);
}

#[test]
fn alt_screen_round_trip() {
    let mut t = term(10, 3);
    t.feed(b"primary");
    t.feed(b"\x1b[?1049h");
    assert!(t.alt_active());
    // 1049 does not home the cursor; applications do that themselves.
    t.feed(b"\x1b[Halt!");
    assert_eq!(screen_lines(&t), vec!["alt!", "", ""]);
    t.feed(b"\x1b[?1049l");
    assert!(!t.alt_active());
    assert_eq!(screen_lines(&t), vec!["primary", "", ""]);
    // Cursor restored to where it was on the primary screen.
    assert_eq!(t.cursor().x, 7);
}

#[test]
fn sgr_colors_tracked_and_serialized() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[31;1mrd\x1b[0m ok");
    let vt = String::from_utf8(t.serialize(Format::Vt).unwrap()).unwrap();
    assert!(vt.contains("\x1b[1;31m"), "vt output: {vt:?}");
    assert!(vt.contains("rd"));
}

#[test]
fn sgr_256_and_truecolor() {
    let mut t = term(20, 2);
    t.feed(b"\x1b[38;5;196ma\x1b[48;2;1;2;3mb\x1b[mc");
    let vt = String::from_utf8(t.serialize(Format::Vt).unwrap()).unwrap();
    assert!(vt.contains("\x1b[38;5;196m"), "vt output: {vt:?}");
    assert!(vt.contains("48;2;1;2;3"), "vt output: {vt:?}");
}

#[test]
fn sgr_colon_subparam_truecolor() {
    let mut t = term(20, 2);
    t.feed(b"\x1b[38:2:10:20:30mz");
    let vt = String::from_utf8(t.serialize(Format::Vt).unwrap()).unwrap();
    assert!(vt.contains("38;2;10;20;30"), "vt output: {vt:?}");
}

#[test]
fn wide_chars_occupy_two_cells() {
    let mut t = term(10, 2);
    t.feed("漢字".as_bytes());
    assert_eq!(t.cursor().x, 4);
    assert_eq!(screen_lines(&t)[0], "漢字");
}

#[test]
fn utf8_split_across_feeds() {
    let mut t = term(10, 2);
    let bytes = "é".as_bytes();
    t.feed(&bytes[..1]);
    t.feed(&bytes[1..]);
    assert_eq!(screen_lines(&t)[0], "é");
}

#[test]
fn osc_sequences_are_skipped() {
    let mut t = term(20, 2);
    t.feed(b"\x1b]2;window title\x07visible");
    t.feed(b"\x1b]7;file:///tmp\x1b\\!");
    assert_eq!(screen_lines(&t)[0], "visible!");
}

#[test]
fn insert_and_delete_chars() {
    let mut t = term(10, 1);
    t.feed(b"abcdef\x1b[3G\x1b[2@XY");
    assert_eq!(screen_lines(&t), vec!["abXYcdef"]);
    t.feed(b"\x1b[3G\x1b[2P");
    assert_eq!(screen_lines(&t), vec!["abcdef"]);
}

#[test]
fn insert_and_delete_lines() {
    let mut t = term(10, 4);
    t.feed(b"a\r\nb\r\nc\r\nd");
    t.feed(b"\x1b[2;1H\x1b[L");
    assert_eq!(screen_lines(&t), vec!["a", "", "b", "c"]);
    t.feed(b"\x1b[2;1H\x1b[M");
    assert_eq!(screen_lines(&t), vec!["a", "b", "c", ""]);
}

#[test]
fn resize_preserves_content_and_cursor() {
    let mut t = term(10, 4);
    t.feed(b"one\r\ntwo\r\n> ");
    t.resize(10, 2);
    // Top rows pushed to scrollback; cursor stays on the prompt line. The
    // prompt's trailing space is trimmed by plain serialization.
    assert_eq!(screen_lines(&t), vec!["one", "two", ">", ""]);
    assert_eq!(t.cursor().y, 0);
    t.resize(10, 4);
    assert_eq!(t.cursor().y, 2);
    assert_eq!(screen_lines(&t)[2], ">");
}

#[test]
fn resize_narrower_truncates() {
    let mut t = term(10, 2);
    t.feed(b"0123456789");
    t.resize(5, 2);
    assert_eq!(screen_lines(&t)[0], "01234");
}

#[test]
fn modes_are_tracked() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[?25l\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[?1h\x1b[4h");
    assert!(!t.modes.cursor_visible);
    assert!(t.modes.bracketed_paste);
    assert_eq!(t.modes.mouse, MouseMode::ButtonEvent);
    assert!(t.modes.mouse_sgr);
    assert!(t.modes.app_cursor_keys);
    assert!(t.modes.insert);
    let state = String::from_utf8(t.serialize_state().unwrap()).unwrap();
    for seq in [
        "\x1b[?25l",
        "\x1b[?2004h",
        "\x1b[?1002h",
        "\x1b[?1006h",
        "\x1b[?1h",
        "\x1b[4h",
    ] {
        assert!(state.contains(seq), "missing {seq:?} in {state:?}");
    }
}

#[test]
fn serialize_state_restores_cursor_position() {
    let mut t = term(20, 5);
    t.feed(b"$ ls\r\nfile\r\n$ ");
    let state = String::from_utf8(t.serialize_state().unwrap()).unwrap();
    assert!(state.contains("\x1b[3;3H"), "state: {state:?}");
    assert!(state.contains("$ ls"));
}

#[test]
fn serialize_state_replays_alt_screen() {
    let mut t = term(10, 3);
    t.feed(b"shell$\x1b[?1049h\x1b[Hfullscreen");
    let state = String::from_utf8(t.serialize_state().unwrap()).unwrap();
    let pos1049 = state.find("\x1b[?1049h").expect("alt screen switch");
    let pos_shell = state.find("shell$").expect("primary content");
    let pos_alt = state.find("fullscreen").expect("alt content");
    assert!(pos_shell < pos1049 && pos1049 < pos_alt);
}

#[test]
fn save_restore_cursor_with_decsc() {
    let mut t = term(10, 3);
    t.feed(b"ab\x1b7\r\nxy\x1b8Z");
    assert_eq!(screen_lines(&t)[0], "abZ");
}

#[test]
fn rep_repeats_last_char() {
    let mut t = term(10, 1);
    t.feed(b"x\x1b[3b");
    assert_eq!(screen_lines(&t), vec!["xxxx"]);
}

#[test]
fn tabs_hit_default_stops() {
    let mut t = term(20, 1);
    t.feed(b"\ta");
    assert_eq!(t.cursor().x, 9);
    let line = &screen_lines(&t)[0];
    assert_eq!(line.trim_start(), "a");
}

#[test]
fn reverse_index_scrolls_down_at_top() {
    let mut t = term(10, 3);
    t.feed(b"a\r\nb\r\nc\x1b[H\x1bM");
    assert_eq!(screen_lines(&t), vec!["", "a", "b"]);
}

#[test]
fn ed3_clears_scrollback() {
    let mut t = term(10, 2);
    t.feed(b"1\r\n2\r\n3\r\n4");
    assert!(plain(&t).lines().count() > 2);
    t.feed(b"\x1b[3J");
    assert_eq!(plain(&t).lines().count(), 2);
}

#[test]
fn scrollback_respects_budget() {
    // Budget of 16*100 cells = 100 ten-col rows -> 10 rows of scrollback.
    let mut t = Terminal::new(10, 2, 16 * 100);
    for i in 0..200 {
        t.feed(format!("line{i}\r\n").as_bytes());
    }
    assert!(t.scrollback_rows().len() <= 10);
}

#[test]
fn html_output_escapes_and_styles() {
    let mut t = term(20, 2);
    t.feed(b"<b>\x1b[31mred");
    let html = String::from_utf8(t.serialize(Format::Html).unwrap()).unwrap();
    assert!(html.contains("&lt;b&gt;"));
    assert!(html.contains("color:#cd0000"));
    assert!(html.starts_with("<pre"));
}

#[test]
fn full_reset_clears_screen_and_modes() {
    let mut t = term(10, 2);
    t.feed(b"\x1b[?25lhello\x1bc");
    assert!(t.modes.cursor_visible);
    assert_eq!(
        t.cursor(),
        Cursor {
            x: 0,
            y: 0,
            pending_wrap: false
        }
    );
    assert_eq!(screen_lines(&t).last().map(|s| s.as_str()), Some(""));
}

#[test]
fn combining_chars_attach_to_previous_cell() {
    let mut t = term(10, 1);
    t.feed("e\u{0301}x".as_bytes());
    assert_eq!(screen_lines(&t)[0], "e\u{0301}x");
    assert_eq!(t.cursor().x, 2);
}

#[test]
fn control_chars_inside_csi_are_executed() {
    // A CR arriving mid-CSI still takes effect (vt500 parser behavior).
    let mut t = term(10, 2);
    t.feed(b"abc\x1b[\r2Cx");
    assert_eq!(t.cursor().y, 0);
    // CR executed mid-sequence reset the column; CSI 2 C then moved to col 2
    // where the final 'x' overwrote the 'c'.
    assert_eq!(screen_lines(&t)[0], "abx");
}
