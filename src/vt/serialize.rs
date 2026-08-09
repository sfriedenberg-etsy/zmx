//! Terminal content serialization: plain text, VT escape-sequence replay
//! (with full state restoration for re-attach), and HTML.

use super::{Color, MouseMode, Row, Style, Terminal};
use super::{BLINK, BOLD, DIM, INVISIBLE, ITALIC, REVERSE, STRIKE, UNDERLINE};

fn row_text(row: &Row) -> String {
    let mut line = String::new();
    for cell in &row.cells {
        if cell.width == 0 {
            continue; // spacer behind a wide char
        }
        line.push(cell.ch);
        for &c in &cell.combining {
            line.push(c);
        }
    }
    let trimmed = line.trim_end_matches(' ');
    trimmed.to_string()
}

/// Plain text: scrollback followed by the visible screen, one line per row
/// with trailing spaces trimmed.
pub(super) fn serialize_plain(term: &Terminal) -> Vec<u8> {
    let mut out = String::new();
    for row in term.scrollback_rows() {
        out.push_str(&row_text(row));
        out.push('\n');
    }
    for row in term.visible_rows() {
        out.push_str(&row_text(row));
        out.push('\n');
    }
    out.into_bytes()
}

/// SGR sequence that switches from a reset pen to `style`. Empty when the
/// style is all-default.
fn style_sgr(style: &Style) -> String {
    if *style == Style::default() {
        return String::new();
    }
    let mut codes: Vec<String> = Vec::new();
    if style.flags & BOLD != 0 {
        codes.push("1".into());
    }
    if style.flags & DIM != 0 {
        codes.push("2".into());
    }
    if style.flags & ITALIC != 0 {
        codes.push("3".into());
    }
    if style.flags & UNDERLINE != 0 {
        codes.push("4".into());
    }
    if style.flags & BLINK != 0 {
        codes.push("5".into());
    }
    if style.flags & REVERSE != 0 {
        codes.push("7".into());
    }
    if style.flags & INVISIBLE != 0 {
        codes.push("8".into());
    }
    if style.flags & STRIKE != 0 {
        codes.push("9".into());
    }
    match style.fg {
        Color::Default => {}
        Color::Idx(n) if n < 8 => codes.push(format!("{}", 30 + n)),
        Color::Idx(n) if n < 16 => codes.push(format!("{}", 90 + n - 8)),
        Color::Idx(n) => codes.push(format!("38;5;{n}")),
        Color::Rgb(r, g, b) => codes.push(format!("38;2;{r};{g};{b}")),
    }
    match style.bg {
        Color::Default => {}
        Color::Idx(n) if n < 8 => codes.push(format!("{}", 40 + n)),
        Color::Idx(n) if n < 16 => codes.push(format!("{}", 100 + n - 8)),
        Color::Idx(n) => codes.push(format!("48;5;{n}")),
        Color::Rgb(r, g, b) => codes.push(format!("48;2;{r};{g};{b}")),
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// Draw one row of cells at 1-based screen line `line_no`, emitting SGR
/// transitions per run. Skips fully-blank rows. Always leaves the stream
/// with a reset pen.
fn draw_row(out: &mut String, row: &Row, line_no: usize) {
    // Find the last cell worth drawing (non-blank or styled).
    let last = row
        .cells
        .iter()
        .rposition(|c| !c.is_blank())
        .map(|i| i + 1)
        .unwrap_or(0);
    if last == 0 {
        return;
    }
    out.push_str(&format!("\x1b[{line_no};1H"));
    let mut cur_style = Style::default();
    for cell in &row.cells[..last] {
        if cell.width == 0 {
            continue;
        }
        if cell.style != cur_style {
            out.push_str("\x1b[0m");
            out.push_str(&style_sgr(&cell.style));
            cur_style = cell.style;
        }
        out.push(cell.ch);
        for &c in &cell.combining {
            out.push(c);
        }
    }
    if cur_style != Style::default() {
        out.push_str("\x1b[0m");
    }
}

fn draw_screen(out: &mut String, rows: &[Row]) {
    out.push_str("\x1b[2J\x1b[H");
    for (i, row) in rows.iter().enumerate() {
        draw_row(out, row, i + 1);
    }
}

fn cursor_position_seq(term: &Terminal) -> String {
    let cursor = term.cursor();
    let (top, _) = term.scroll_region();
    // With origin mode active, CUP is region-relative.
    let row = if term.modes.origin {
        cursor.y.saturating_sub(top) + 1
    } else {
        cursor.y + 1
    };
    format!("\x1b[{};{}H", row, cursor.x + 1)
}

/// VT format without mode restoration: clear, replay the visible screen with
/// attributes, position the cursor.
pub(super) fn serialize_vt(term: &Terminal) -> Vec<u8> {
    let mut out = String::new();
    draw_screen(&mut out, term.visible_rows());
    out.push_str(&format!(
        "\x1b[{};{}H",
        term.cursor().y + 1,
        term.cursor().x + 1
    ));
    out.into_bytes()
}

/// Full state restoration for re-attach: both screens, scrolling region,
/// cursor, pen, and modes. Returns None when there is nothing to restore.
pub(super) fn serialize_state(term: &Terminal) -> Option<Vec<u8>> {
    let mut out = String::new();

    // Primary screen content first (it stays behind the alt screen).
    draw_screen(&mut out, term.primary_rows());
    if term.alt_active() {
        out.push_str("\x1b[?1049h");
        draw_screen(&mut out, term.visible_rows());
    }

    // Scrolling region before cursor positioning (DECSTBM homes the cursor).
    let (top, bot) = term.scroll_region();
    if top != 0 || bot != term.rows() - 1 {
        out.push_str(&format!("\x1b[{};{}r", top + 1, bot + 1));
    }
    if term.modes.origin {
        out.push_str("\x1b[?6h");
    }
    out.push_str(&cursor_position_seq(term));
    out.push_str(&style_sgr(&term.pen()));

    // Modes.
    let m = &term.modes;
    if !m.cursor_visible {
        out.push_str("\x1b[?25l");
    }
    if !m.autowrap {
        out.push_str("\x1b[?7l");
    }
    if m.app_cursor_keys {
        out.push_str("\x1b[?1h");
    }
    if m.app_keypad {
        out.push_str("\x1b=");
    }
    if m.insert {
        out.push_str("\x1b[4h");
    }
    if m.bracketed_paste {
        out.push_str("\x1b[?2004h");
    }
    match m.mouse {
        MouseMode::None => {}
        MouseMode::Normal => out.push_str("\x1b[?1000h"),
        MouseMode::ButtonEvent => out.push_str("\x1b[?1002h"),
        MouseMode::AnyEvent => out.push_str("\x1b[?1003h"),
    }
    if m.mouse_sgr {
        out.push_str("\x1b[?1006h");
    }
    if m.focus_events {
        out.push_str("\x1b[?1004h");
    }

    Some(out.into_bytes())
}

// ---- HTML -------------------------------------------------------------------

/// xterm 256-color palette entry as (r, g, b).
fn palette_rgb(n: u8) -> (u8, u8, u8) {
    const BASE: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    match n {
        0..=15 => BASE[n as usize],
        16..=231 => {
            let n = n - 16;
            let levels = [0, 95, 135, 175, 215, 255];
            (
                levels[(n / 36) as usize],
                levels[((n / 6) % 6) as usize],
                levels[(n % 6) as usize],
            )
        }
        232..=255 => {
            let v = 8 + (n - 232) * 10;
            (v, v, v)
        }
    }
}

fn css_color(color: Color) -> Option<String> {
    match color {
        Color::Default => None,
        Color::Idx(n) => {
            let (r, g, b) = palette_rgb(n);
            Some(format!("#{r:02x}{g:02x}{b:02x}"))
        }
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
    }
}

fn html_escape(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        _ => out.push(ch),
    }
}

fn style_css(style: &Style) -> String {
    let mut css = String::new();
    let (mut fg, mut bg) = (style.fg, style.bg);
    if style.flags & REVERSE != 0 {
        std::mem::swap(&mut fg, &mut bg);
    }
    if let Some(c) = css_color(fg) {
        css.push_str(&format!("color:{c};"));
    }
    if let Some(c) = css_color(bg) {
        css.push_str(&format!("background:{c};"));
    }
    if style.flags & BOLD != 0 {
        css.push_str("font-weight:bold;");
    }
    if style.flags & ITALIC != 0 {
        css.push_str("font-style:italic;");
    }
    let mut deco = Vec::new();
    if style.flags & UNDERLINE != 0 {
        deco.push("underline");
    }
    if style.flags & STRIKE != 0 {
        deco.push("line-through");
    }
    if !deco.is_empty() {
        css.push_str(&format!("text-decoration:{};", deco.join(" ")));
    }
    if style.flags & DIM != 0 {
        css.push_str("opacity:0.7;");
    }
    if style.flags & INVISIBLE != 0 {
        css.push_str("visibility:hidden;");
    }
    css
}

/// HTML: scrollback + visible screen wrapped in a <pre> with inline styles.
pub(super) fn serialize_html(term: &Terminal) -> Vec<u8> {
    let mut out = String::from(
        "<pre style=\"font-family:monospace;background:#1d1f21;color:#c5c8c6;padding:1em\">\n",
    );
    let rows = term
        .scrollback_rows()
        .iter()
        .chain(term.visible_rows().iter());
    for row in rows {
        let last = row
            .cells
            .iter()
            .rposition(|c| !c.is_blank())
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut cur_style: Option<Style> = None;
        for cell in &row.cells[..last] {
            if cell.width == 0 {
                continue;
            }
            if cur_style != Some(cell.style) {
                if cur_style.is_some_and(|s| s != Style::default()) {
                    out.push_str("</span>");
                }
                if cell.style != Style::default() {
                    out.push_str(&format!("<span style=\"{}\">", style_css(&cell.style)));
                }
                cur_style = Some(cell.style);
            }
            html_escape(&mut out, cell.ch);
            for &c in &cell.combining {
                html_escape(&mut out, c);
            }
        }
        if cur_style.is_some_and(|s| s != Style::default()) {
            out.push_str("</span>");
        }
        out.push('\n');
    }
    out.push_str("</pre>\n");
    out.into_bytes()
}
