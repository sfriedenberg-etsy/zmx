//! VT escape sequence parser: a byte-level state machine modeled on the
//! classic vt500 parser (Ground / Escape / CSI / OSC / string states) with
//! inline UTF-8 decoding for printable text.

use super::{Color, MouseMode, Style, Terminal};
use super::{BLINK, BOLD, DIM, INVISIBLE, ITALIC, REVERSE, STRIKE, UNDERLINE};

const MAX_PARAMS: usize = 32;
const MAX_SUBPARAMS: usize = 8;
const MAX_OSC: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    OscEsc,
    /// DCS/SOS/PM/APC string: skipped until ST.
    Str,
    StrEsc,
}

pub(super) struct Parser {
    state: State,
    // UTF-8 decoding
    utf8_buf: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
    // ESC intermediates
    esc_intermediate: u8,
    // CSI accumulation
    params: Vec<Vec<u32>>,
    cur_param: Vec<u32>,
    cur_value: u32,
    cur_has_digits: bool,
    private_marker: u8,
    csi_intermediate: u8,
    csi_ignored: bool,
    // OSC accumulation (collected but unused beyond bounds-keeping; window
    // titles are tracked separately by the daemon's TitleTracker).
    osc_len: usize,
}

impl Parser {
    pub(super) fn new() -> Parser {
        Parser {
            state: State::Ground,
            utf8_buf: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
            esc_intermediate: 0,
            params: Vec::new(),
            cur_param: Vec::new(),
            cur_value: 0,
            cur_has_digits: false,
            private_marker: 0,
            csi_intermediate: 0,
            csi_ignored: false,
            osc_len: 0,
        }
    }

    pub(super) fn feed(&mut self, term: &mut Terminal, data: &[u8]) {
        for &byte in data {
            self.advance(term, byte);
        }
    }

    fn advance(&mut self, term: &mut Terminal, byte: u8) {
        match self.state {
            State::Ground => self.ground(term, byte),
            State::Escape => self.escape(term, byte),
            State::EscapeIntermediate => self.escape_intermediate(term, byte),
            State::Csi => self.csi(term, byte),
            State::Osc => self.osc(byte),
            State::OscEsc => self.osc_esc(term, byte),
            State::Str => self.string_skip(byte),
            State::StrEsc => self.string_esc(byte),
        }
    }

    // ---- Ground / UTF-8 ---------------------------------------------------

    fn ground(&mut self, term: &mut Terminal, byte: u8) {
        if self.utf8_need > 0 {
            if (0x80..0xC0).contains(&byte) {
                self.utf8_buf[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    let ch = std::str::from_utf8(&self.utf8_buf[..self.utf8_len])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or('\u{FFFD}');
                    self.utf8_need = 0;
                    self.utf8_len = 0;
                    term.print_char(ch);
                }
                return;
            }
            // Incomplete sequence: emit a replacement char, reprocess byte.
            self.utf8_need = 0;
            self.utf8_len = 0;
            term.print_char('\u{FFFD}');
        }

        match byte {
            0x00..=0x1F | 0x7F => self.execute(term, byte),
            0x20..=0x7E => term.print_char(byte as char),
            0xC2..=0xDF => self.utf8_start(byte, 2),
            0xE0..=0xEF => self.utf8_start(byte, 3),
            0xF0..=0xF4 => self.utf8_start(byte, 4),
            _ => term.print_char('\u{FFFD}'),
        }
    }

    fn utf8_start(&mut self, byte: u8, need: usize) {
        self.utf8_buf[0] = byte;
        self.utf8_len = 1;
        self.utf8_need = need;
    }

    /// C0 control execution (valid in Ground and mid-sequence).
    fn execute(&mut self, term: &mut Terminal, byte: u8) {
        match byte {
            0x08 => term.backspace(),
            0x09 => term.horizontal_tab(),
            0x0A..=0x0C => term.linefeed(),
            0x0D => term.carriage_return(),
            0x1B => self.enter_escape(),
            // BEL, SO/SI (charset shifts), NUL, DEL and the rest: ignored.
            _ => {}
        }
    }

    fn enter_escape(&mut self) {
        self.state = State::Escape;
        self.esc_intermediate = 0;
    }

    // ---- Escape -------------------------------------------------------------

    fn escape(&mut self, term: &mut Terminal, byte: u8) {
        match byte {
            0x18 | 0x1A => self.state = State::Ground,
            0x1B => {}
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => self.execute(term, byte),
            b'[' => self.enter_csi(),
            b']' => self.enter_osc(),
            b'P' | b'X' | b'^' | b'_' => self.state = State::Str,
            0x20..=0x2F => {
                self.esc_intermediate = byte;
                self.state = State::EscapeIntermediate;
            }
            _ => {
                self.esc_dispatch(term, 0, byte);
                self.state = State::Ground;
            }
        }
    }

    fn escape_intermediate(&mut self, term: &mut Terminal, byte: u8) {
        match byte {
            0x18 | 0x1A => self.state = State::Ground,
            0x1B => self.enter_escape(),
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => self.execute(term, byte),
            0x20..=0x2F => {} // further intermediates: keep the first
            _ => {
                self.esc_dispatch(term, self.esc_intermediate, byte);
                self.state = State::Ground;
            }
        }
    }

    fn esc_dispatch(&mut self, term: &mut Terminal, intermediate: u8, final_byte: u8) {
        match intermediate {
            0 => match final_byte {
                b'7' => term.save_cursor(),
                b'8' => term.restore_cursor(),
                b'D' => term.linefeed(),
                b'E' => {
                    term.carriage_return();
                    term.linefeed();
                }
                b'H' => term.set_tabstop(),
                b'M' => term.reverse_index(),
                b'c' => term.full_reset(),
                b'=' => term.modes.app_keypad = true,
                b'>' => term.modes.app_keypad = false,
                _ => {}
            },
            b'#' => {
                if final_byte == b'8' {
                    term.screen_alignment_pattern();
                }
            }
            // Charset designation (ESC ( ) * + <final>): ignored.
            _ => {}
        }
    }

    // ---- CSI ----------------------------------------------------------------

    fn enter_csi(&mut self) {
        self.state = State::Csi;
        self.params.clear();
        self.cur_param.clear();
        self.cur_value = 0;
        self.cur_has_digits = false;
        self.private_marker = 0;
        self.csi_intermediate = 0;
        self.csi_ignored = false;
    }

    fn csi(&mut self, term: &mut Terminal, byte: u8) {
        match byte {
            0x18 | 0x1A => self.state = State::Ground,
            0x1B => self.enter_escape(),
            0x00..=0x17 | 0x19 | 0x1C..=0x1F => self.execute(term, byte),
            b'0'..=b'9' => {
                self.cur_value = self
                    .cur_value
                    .saturating_mul(10)
                    .saturating_add((byte - b'0') as u32);
                self.cur_has_digits = true;
            }
            b':' => {
                if self.cur_param.len() < MAX_SUBPARAMS {
                    self.cur_param.push(self.cur_value);
                }
                self.cur_value = 0;
                self.cur_has_digits = true; // ":" implies a parameter exists
            }
            b';' => self.finish_param(),
            b'<' | b'=' | b'>' | b'?' => {
                if self.params.is_empty() && !self.cur_has_digits {
                    self.private_marker = byte;
                } else {
                    self.csi_ignored = true;
                }
            }
            0x20..=0x2F => self.csi_intermediate = byte,
            0x40..=0x7E => {
                self.finish_param();
                if !self.csi_ignored {
                    self.csi_dispatch(term, byte);
                }
                self.state = State::Ground;
            }
            _ => {
                self.csi_ignored = true;
            }
        }
    }

    fn finish_param(&mut self) {
        if self.params.len() >= MAX_PARAMS {
            self.cur_param.clear();
            self.cur_value = 0;
            self.cur_has_digits = false;
            return;
        }
        let mut param = std::mem::take(&mut self.cur_param);
        if self.cur_has_digits || !param.is_empty() {
            param.push(self.cur_value);
        }
        self.params.push(param);
        self.cur_value = 0;
        self.cur_has_digits = false;
    }

    /// First value of param `i`, or `default` when absent/zero-length.
    fn param(&self, i: usize, default: u32) -> u32 {
        match self.params.get(i) {
            Some(p) if !p.is_empty() => p[0],
            _ => default,
        }
    }

    /// Like `param` but treats an explicit 0 as the default (most cursor
    /// movement params are 1-based with 0 meaning 1).
    fn param_nz(&self, i: usize, default: u32) -> u32 {
        let v = self.param(i, default);
        if v == 0 {
            default
        } else {
            v
        }
    }

    fn csi_dispatch(&mut self, term: &mut Terminal, final_byte: u8) {
        if self.csi_intermediate != 0 {
            // CSI with intermediates (DECSCUSR " q", soft reset "! p", ...):
            // not modeled.
            return;
        }
        match final_byte {
            b'A' => term.cursor_up(self.param_nz(0, 1) as usize),
            b'B' => term.cursor_down(self.param_nz(0, 1) as usize),
            b'C' => term.cursor_forward(self.param_nz(0, 1) as usize),
            b'D' => term.cursor_back(self.param_nz(0, 1) as usize),
            b'E' => {
                term.cursor_down(self.param_nz(0, 1) as usize);
                term.carriage_return();
            }
            b'F' => {
                term.cursor_up(self.param_nz(0, 1) as usize);
                term.carriage_return();
            }
            b'G' | b'`' => term.set_column(self.param_nz(0, 1) as usize - 1),
            b'H' | b'f' => {
                let row = self.param_nz(0, 1) as usize - 1;
                let col = self.param_nz(1, 1) as usize - 1;
                term.set_position(row, col);
            }
            b'I' => term.forward_tabs(self.param_nz(0, 1) as usize),
            b'J' => term.erase_display(self.param(0, 0) as usize),
            b'K' => term.erase_line(self.param(0, 0) as usize),
            b'L' => term.insert_lines(self.param_nz(0, 1) as usize),
            b'M' => term.delete_lines(self.param_nz(0, 1) as usize),
            b'P' => term.delete_chars(self.param_nz(0, 1) as usize),
            b'S' => term.scroll_up(self.param_nz(0, 1) as usize),
            b'T' => {
                // CSI T with >1 params is mouse tracking config; ignore.
                if self.params.len() <= 1 {
                    term.scroll_down(self.param_nz(0, 1) as usize);
                }
            }
            b'X' => term.erase_chars(self.param_nz(0, 1) as usize),
            b'Z' => term.backward_tabs(self.param_nz(0, 1) as usize),
            b'@' => term.insert_chars(self.param_nz(0, 1) as usize),
            b'a' => term.cursor_forward(self.param_nz(0, 1) as usize),
            b'b' => term.repeat_last(self.param_nz(0, 1) as usize),
            b'd' => term.set_row(self.param_nz(0, 1) as usize - 1),
            b'e' => term.cursor_down(self.param_nz(0, 1) as usize),
            b'g' => term.clear_tabstop(self.param(0, 0) as usize),
            b'h' => self.set_modes(term, true),
            b'l' => self.set_modes(term, false),
            b'm' => {
                if self.private_marker == 0 {
                    self.sgr(term);
                }
            }
            b'r' => {
                if self.private_marker == 0 {
                    let top = self.param_nz(0, 1) as usize - 1;
                    let bot = self.param_nz(1, term.rows() as u32) as usize - 1;
                    term.set_scroll_region(top, bot);
                }
            }
            b's' => {
                if self.private_marker == 0 {
                    term.save_cursor();
                }
            }
            b'u' => {
                if self.private_marker == 0 {
                    term.restore_cursor();
                }
            }
            // DSR/DA/window ops and other queries: record-only emulator, no
            // response channel — the real terminal answers the application.
            _ => {}
        }
    }

    fn set_modes(&mut self, term: &mut Terminal, enable: bool) {
        for i in 0..self.params.len() {
            let mode = self.param(i, 0);
            if self.private_marker == b'?' {
                self.set_private_mode(term, mode, enable);
            } else if self.private_marker == 0 && mode == 4 {
                term.modes.insert = enable
            }
        }
    }

    fn set_private_mode(&mut self, term: &mut Terminal, mode: u32, enable: bool) {
        match mode {
            1 => term.modes.app_cursor_keys = enable,
            6 => {
                term.modes.origin = enable;
                term.set_position(0, 0);
            }
            7 => term.modes.autowrap = enable,
            25 => term.modes.cursor_visible = enable,
            47 => {
                if enable {
                    term.enter_alt_screen(false);
                } else {
                    term.exit_alt_screen();
                }
            }
            1000 => {
                term.modes.mouse = if enable {
                    MouseMode::Normal
                } else {
                    MouseMode::None
                }
            }
            1002 => {
                term.modes.mouse = if enable {
                    MouseMode::ButtonEvent
                } else {
                    MouseMode::None
                }
            }
            1003 => {
                term.modes.mouse = if enable {
                    MouseMode::AnyEvent
                } else {
                    MouseMode::None
                }
            }
            1004 => term.modes.focus_events = enable,
            1006 => term.modes.mouse_sgr = enable,
            1047 => {
                if enable {
                    term.enter_alt_screen(true);
                } else {
                    term.exit_alt_screen();
                }
            }
            1048 => {
                if enable {
                    term.save_cursor();
                } else {
                    term.restore_cursor();
                }
            }
            1049 => {
                if enable {
                    term.save_cursor();
                    term.enter_alt_screen(true);
                } else {
                    term.exit_alt_screen();
                    term.restore_cursor();
                }
            }
            2004 => term.modes.bracketed_paste = enable,
            _ => {}
        }
    }

    // ---- SGR ------------------------------------------------------------------

    fn sgr(&mut self, term: &mut Terminal) {
        if self.params.is_empty() {
            term.set_pen(Style::default());
            return;
        }
        let mut pen = term.pen();
        let mut i = 0;
        while i < self.params.len() {
            let group = &self.params[i];
            let code = if group.is_empty() { 0 } else { group[0] };
            match code {
                0 => pen = Style::default(),
                1 => pen.flags |= BOLD,
                2 => pen.flags |= DIM,
                3 => pen.flags |= ITALIC,
                4 => {
                    // 4:0 turns underline off; 4 / 4:n turn it on.
                    if group.len() > 1 && group[1] == 0 {
                        pen.flags &= !UNDERLINE;
                    } else {
                        pen.flags |= UNDERLINE;
                    }
                }
                5 | 6 => pen.flags |= BLINK,
                7 => pen.flags |= REVERSE,
                8 => pen.flags |= INVISIBLE,
                9 => pen.flags |= STRIKE,
                21 => pen.flags |= UNDERLINE,
                22 => pen.flags &= !(BOLD | DIM),
                23 => pen.flags &= !ITALIC,
                24 => pen.flags &= !UNDERLINE,
                25 => pen.flags &= !BLINK,
                27 => pen.flags &= !REVERSE,
                28 => pen.flags &= !INVISIBLE,
                29 => pen.flags &= !STRIKE,
                30..=37 => pen.fg = Color::Idx((code - 30) as u8),
                38 => {
                    if let Some((color, consumed)) = self.extended_color(i) {
                        pen.fg = color;
                        i += consumed;
                    }
                }
                39 => pen.fg = Color::Default,
                40..=47 => pen.bg = Color::Idx((code - 40) as u8),
                48 => {
                    if let Some((color, consumed)) = self.extended_color(i) {
                        pen.bg = color;
                        i += consumed;
                    }
                }
                49 => pen.bg = Color::Default,
                90..=97 => pen.fg = Color::Idx((code - 90 + 8) as u8),
                100..=107 => pen.bg = Color::Idx((code - 100 + 8) as u8),
                _ => {}
            }
            i += 1;
        }
        term.set_pen(pen);
    }

    /// Parse an extended color (SGR 38/48) starting at param index `i`.
    /// Handles both the colon form (`38:5:n`, `38:2:r:g:b`, `38:2::r:g:b`)
    /// and the legacy semicolon form (`38;5;n`, `38;2;r;g;b`). Returns the
    /// color and how many *extra* params (semicolon groups) were consumed.
    fn extended_color(&self, i: usize) -> Option<(Color, usize)> {
        let group = &self.params[i];
        if group.len() > 1 {
            // Colon form: everything inside this group.
            match group[1] {
                5 if group.len() >= 3 => Some((Color::Idx(group[2].min(255) as u8), 0)),
                2 if group.len() >= 5 => {
                    // 38:2:r:g:b or 38:2:<colorspace>:r:g:b
                    let (r, g, b) = if group.len() >= 6 {
                        (group[3], group[4], group[5])
                    } else {
                        (group[2], group[3], group[4])
                    };
                    Some((
                        Color::Rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8),
                        0,
                    ))
                }
                _ => None,
            }
        } else {
            // Semicolon form: consume following params.
            match self.param(i + 1, 0) {
                5 => Some((Color::Idx(self.param(i + 2, 0).min(255) as u8), 2)),
                2 => Some((
                    Color::Rgb(
                        self.param(i + 2, 0).min(255) as u8,
                        self.param(i + 3, 0).min(255) as u8,
                        self.param(i + 4, 0).min(255) as u8,
                    ),
                    4,
                )),
                _ => None,
            }
        }
    }

    // ---- OSC / strings -----------------------------------------------------

    fn enter_osc(&mut self) {
        self.state = State::Osc;
        self.osc_len = 0;
    }

    fn osc(&mut self, byte: u8) {
        match byte {
            0x07 => self.state = State::Ground,
            0x1B => self.state = State::OscEsc,
            0x18 | 0x1A => self.state = State::Ground,
            _ => {
                if self.osc_len < MAX_OSC {
                    self.osc_len += 1;
                }
            }
        }
    }

    fn osc_esc(&mut self, term: &mut Terminal, byte: u8) {
        match byte {
            b'\\' => self.state = State::Ground,
            0x1B => {}
            _ => {
                // Not an ST: the ESC aborted the OSC and begins a new escape
                // sequence; reinterpret this byte in the Escape state.
                self.enter_escape();
                self.escape(term, byte);
            }
        }
    }

    fn string_skip(&mut self, byte: u8) {
        match byte {
            0x1B => self.state = State::StrEsc,
            0x18 | 0x1A => self.state = State::Ground,
            0x07 => self.state = State::Ground, // be lenient: BEL ends strings too
            _ => {}
        }
    }

    fn string_esc(&mut self, byte: u8) {
        match byte {
            b'\\' => self.state = State::Ground,
            0x1B => {}
            _ => self.state = State::Str,
        }
    }
}
