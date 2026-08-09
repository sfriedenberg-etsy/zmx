//! Built-in terminal emulator: a pure-Rust replacement for the ghostty-vt /
//! libvterm backends. The daemon feeds raw PTY output through `Terminal` to
//! maintain a model of the session screen (grid + scrollback + modes), which
//! is serialized on re-attach (`serialize_state`) and for the `history`
//! command (`serialize`).
//!
//! The emulator is record-only: sequences that would normally require writing
//! a response back to the application (DSR, DA, ...) are ignored, because zmx
//! passes PTY bytes straight through to the attached client and the real
//! terminal answers those queries itself.

mod parser;
mod serialize;

use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;

/// Output format for terminal serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Format {
    Plain = 0,
    Vt = 1,
    Html = 2,
}

impl Format {
    pub fn from_u8(v: u8) -> Format {
        match v {
            1 => Format::Vt,
            2 => Format::Html,
            _ => Format::Plain,
        }
    }
}

/// Cursor position and state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    pub pending_wrap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

pub const BOLD: u16 = 1 << 0;
pub const DIM: u16 = 1 << 1;
pub const ITALIC: u16 = 1 << 2;
pub const UNDERLINE: u16 = 1 << 3;
pub const BLINK: u16 = 1 << 4;
pub const REVERSE: u16 = 1 << 5;
pub const INVISIBLE: u16 = 1 << 6;
pub const STRIKE: u16 = 1 << 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// 1 for normal cells, 2 for a wide-character head, 0 for the spacer
    /// cell following a wide character.
    pub width: u8,
    pub style: Style,
    /// Zero-width combining characters attached to this cell.
    pub combining: Vec<char>,
}

impl Cell {
    fn blank(style: Style) -> Cell {
        Cell {
            ch: ' ',
            width: 1,
            style,
            combining: Vec::new(),
        }
    }

    fn is_blank(&self) -> bool {
        self.ch == ' ' && self.combining.is_empty() && self.style == Style::default()
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// True when the line soft-wrapped into the next row.
    pub wrapped: bool,
}

impl Row {
    fn blank(cols: usize, style: Style) -> Row {
        Row {
            cells: vec![Cell::blank(style); cols],
            wrapped: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SavedCursor {
    x: usize,
    y: usize,
    style: Style,
    origin: bool,
    pending_wrap: bool,
}

struct Screen {
    lines: Vec<Row>,
    scrollback: VecDeque<Row>,
    scrollback_cells: usize,
    saved: Option<SavedCursor>,
    /// Scrolling region, inclusive row indices.
    scroll_top: usize,
    scroll_bot: usize,
    tabstops: Vec<bool>,
}

impl Screen {
    fn new(cols: usize, rows: usize) -> Screen {
        Screen {
            lines: (0..rows)
                .map(|_| Row::blank(cols, Style::default()))
                .collect(),
            scrollback: VecDeque::new(),
            scrollback_cells: 0,
            saved: None,
            scroll_top: 0,
            scroll_bot: rows.saturating_sub(1),
            tabstops: default_tabstops(cols),
        }
    }
}

fn default_tabstops(cols: usize) -> Vec<bool> {
    (0..cols).map(|i| i > 0 && i % 8 == 0).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    None,
    Normal,
    ButtonEvent,
    AnyEvent,
}

#[derive(Debug, Clone, Copy)]
pub struct Modes {
    pub cursor_visible: bool,
    pub autowrap: bool,
    pub origin: bool,
    pub insert: bool,
    pub app_cursor_keys: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    pub mouse_sgr: bool,
    pub focus_events: bool,
}

impl Default for Modes {
    fn default() -> Modes {
        Modes {
            cursor_visible: true,
            autowrap: true,
            origin: false,
            insert: false,
            app_cursor_keys: false,
            app_keypad: false,
            bracketed_paste: false,
            mouse: MouseMode::None,
            mouse_sgr: false,
            focus_events: false,
        }
    }
}

pub struct Terminal {
    cols: usize,
    rows: usize,
    primary: Screen,
    alt: Screen,
    alt_active: bool,
    cursor: Cursor,
    pen: Style,
    pub modes: Modes,
    last_printed: Option<char>,
    /// Scrollback budget expressed in cells (approximating the configured
    /// byte budget at ~16 bytes per cell).
    max_scrollback_cells: usize,
    parser: parser::Parser,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Terminal {
        let cols = (cols as usize).max(1);
        let rows = (rows as usize).max(1);
        Terminal {
            cols,
            rows,
            primary: Screen::new(cols, rows),
            alt: Screen::new(cols, rows),
            alt_active: false,
            cursor: Cursor {
                x: 0,
                y: 0,
                pending_wrap: false,
            },
            pen: Style::default(),
            modes: Modes::default(),
            last_printed: None,
            max_scrollback_cells: max_scrollback / 16,
            parser: parser::Parser::new(),
        }
    }

    #[allow(dead_code)]
    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn alt_active(&self) -> bool {
        self.alt_active
    }

    /// Process the next slice of PTY output through the emulator.
    pub fn feed(&mut self, data: &[u8]) {
        let mut parser = std::mem::replace(&mut self.parser, parser::Parser::new());
        parser.feed(self, data);
        self.parser = parser;
    }

    fn screen(&self) -> &Screen {
        if self.alt_active {
            &self.alt
        } else {
            &self.primary
        }
    }

    fn screen_mut(&mut self) -> &mut Screen {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.primary
        }
    }

    fn blank_cell(&self) -> Cell {
        Cell::blank(Style {
            fg: Color::Default,
            bg: self.pen.bg,
            flags: 0,
        })
    }

    fn blank_row(&self) -> Row {
        Row {
            cells: vec![self.blank_cell(); self.cols],
            wrapped: false,
        }
    }

    // ---- printing -------------------------------------------------------

    pub(crate) fn print_char(&mut self, ch: char) {
        let width = UnicodeWidthChar::width(ch).unwrap_or(if (ch as u32) < 0x20 { 0 } else { 1 });

        if width == 0 {
            // Combining character: attach to the most recently printed cell.
            let tx = if self.cursor.pending_wrap {
                self.cursor.x
            } else if self.cursor.x > 0 {
                self.cursor.x - 1
            } else {
                return;
            };
            let y = self.cursor.y;
            let screen = self.screen_mut();
            if let Some(cell) = screen.lines[y].cells.get_mut(tx) {
                cell.combining.push(ch);
            }
            return;
        }

        if self.cursor.pending_wrap && self.modes.autowrap {
            let y = self.cursor.y;
            self.screen_mut().lines[y].wrapped = true;
            self.cursor.x = 0;
            self.cursor.pending_wrap = false;
            self.linefeed();
        }

        // A wide character that doesn't fit in the remaining columns wraps
        // (or clamps when autowrap is off).
        if width == 2 && self.cursor.x + 2 > self.cols {
            if self.modes.autowrap {
                let y = self.cursor.y;
                self.screen_mut().lines[y].wrapped = true;
                self.cursor.x = 0;
                self.linefeed();
            } else {
                self.cursor.x = self.cols.saturating_sub(2);
            }
        }

        let width = width.min(self.cols); // degenerate 1-column terminals
        let (x, y) = (self.cursor.x, self.cursor.y);

        if self.modes.insert {
            let blank = self.blank_cell();
            let cols = self.cols;
            let row = &mut self.screen_mut().lines[y];
            for _ in 0..width {
                row.cells.insert(x, blank.clone());
            }
            row.cells.truncate(cols);
        }

        let cell = Cell {
            ch,
            width: width as u8,
            style: self.pen,
            combining: Vec::new(),
        };
        self.set_cell(x, y, cell);
        if width == 2 {
            self.set_cell(
                x + 1,
                y,
                Cell {
                    ch: ' ',
                    width: 0,
                    style: self.pen,
                    combining: Vec::new(),
                },
            );
        }

        let new_x = x + width;
        if new_x >= self.cols {
            self.cursor.x = self.cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.x = new_x;
            self.cursor.pending_wrap = false;
        }
        self.last_printed = Some(ch);
    }

    /// Write a cell, splitting any wide character it partially overwrites.
    fn set_cell(&mut self, x: usize, y: usize, cell: Cell) {
        let blank = self.blank_cell();
        let cols = self.cols;
        let row = &mut self.screen_mut().lines[y];
        if x >= cols {
            return;
        }
        // Overwriting a spacer: clear the wide head before it.
        if row.cells[x].width == 0 && x > 0 && row.cells[x - 1].width == 2 {
            row.cells[x - 1] = blank.clone();
        }
        // Overwriting a wide head: clear its spacer.
        if row.cells[x].width == 2 && x + 1 < cols && row.cells[x + 1].width == 0 {
            row.cells[x + 1] = blank;
        }
        row.cells[x] = cell;
    }

    pub(crate) fn repeat_last(&mut self, n: usize) {
        if let Some(ch) = self.last_printed {
            for _ in 0..n {
                self.print_char(ch);
            }
        }
    }

    // ---- cursor movement -------------------------------------------------

    pub(crate) fn linefeed(&mut self) {
        self.cursor.pending_wrap = false;
        let bot = self.screen().scroll_bot;
        if self.cursor.y == bot {
            self.scroll_up(1);
        } else if self.cursor.y + 1 < self.rows {
            self.cursor.y += 1;
        }
    }

    pub(crate) fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        let top = self.screen().scroll_top;
        if self.cursor.y == top {
            self.scroll_down(1);
        } else if self.cursor.y > 0 {
            self.cursor.y -= 1;
        }
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
        }
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn horizontal_tab(&mut self) {
        let screen = self.screen();
        let mut x = self.cursor.x;
        while x + 1 < self.cols {
            x += 1;
            if screen.tabstops.get(x).copied().unwrap_or(false) {
                break;
            }
        }
        self.cursor.x = x;
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn forward_tabs(&mut self, n: usize) {
        for _ in 0..n {
            self.horizontal_tab();
        }
    }

    pub(crate) fn backward_tabs(&mut self, n: usize) {
        for _ in 0..n {
            let screen = self.screen();
            let mut x = self.cursor.x;
            while x > 0 {
                x -= 1;
                if screen.tabstops.get(x).copied().unwrap_or(false) {
                    break;
                }
            }
            self.cursor.x = x;
        }
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn set_tabstop(&mut self) {
        let x = self.cursor.x;
        if let Some(slot) = self.screen_mut().tabstops.get_mut(x) {
            *slot = true;
        }
    }

    pub(crate) fn clear_tabstop(&mut self, mode: usize) {
        match mode {
            0 => {
                let x = self.cursor.x;
                if let Some(slot) = self.screen_mut().tabstops.get_mut(x) {
                    *slot = false;
                }
            }
            3 => {
                for slot in self.screen_mut().tabstops.iter_mut() {
                    *slot = false;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn cursor_up(&mut self, n: usize) {
        let top = self.screen().scroll_top;
        let limit = if self.cursor.y >= top { top } else { 0 };
        self.cursor.y = self.cursor.y.saturating_sub(n).max(limit);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn cursor_down(&mut self, n: usize) {
        let bot = self.screen().scroll_bot;
        let limit = if self.cursor.y <= bot {
            bot
        } else {
            self.rows - 1
        };
        self.cursor.y = (self.cursor.y + n).min(limit);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn cursor_forward(&mut self, n: usize) {
        self.cursor.x = (self.cursor.x + n).min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn cursor_back(&mut self, n: usize) {
        self.cursor.x = self.cursor.x.saturating_sub(n);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn set_column(&mut self, col: usize) {
        self.cursor.x = col.min(self.cols - 1);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn set_row(&mut self, row: usize) {
        let y = if self.modes.origin {
            let screen = self.screen();
            (screen.scroll_top + row).min(screen.scroll_bot)
        } else {
            row.min(self.rows - 1)
        };
        self.cursor.y = y;
        self.cursor.pending_wrap = false;
    }

    /// CUP/HVP with 0-based coordinates (already origin-mode aware).
    pub(crate) fn set_position(&mut self, row: usize, col: usize) {
        self.set_row(row);
        self.set_column(col);
    }

    // ---- scrolling -------------------------------------------------------

    fn push_scrollback(&mut self, row: Row) {
        if self.alt_active {
            return; // alt screen has no scrollback
        }
        let max_cells = self.max_scrollback_cells;
        let screen = &mut self.primary;
        screen.scrollback_cells += row.cells.len();
        screen.scrollback.push_back(row);
        while screen.scrollback_cells > max_cells {
            match screen.scrollback.pop_front() {
                Some(old) => screen.scrollback_cells -= old.cells.len(),
                None => break,
            }
        }
    }

    pub(crate) fn scroll_up(&mut self, n: usize) {
        let (top, bot) = {
            let screen = self.screen();
            (screen.scroll_top, screen.scroll_bot)
        };
        let n = n.min(bot - top + 1);
        let to_scrollback = !self.alt_active && top == 0 && bot == self.rows - 1;
        for _ in 0..n {
            let blank = self.blank_row();
            let removed = self.screen_mut().lines.remove(top);
            if to_scrollback {
                self.push_scrollback(removed);
            }
            self.screen_mut().lines.insert(bot, blank);
        }
    }

    pub(crate) fn scroll_down(&mut self, n: usize) {
        let (top, bot) = {
            let screen = self.screen();
            (screen.scroll_top, screen.scroll_bot)
        };
        let n = n.min(bot - top + 1);
        for _ in 0..n {
            let blank = self.blank_row();
            self.screen_mut().lines.remove(bot);
            self.screen_mut().lines.insert(top, blank);
        }
    }

    pub(crate) fn set_scroll_region(&mut self, top: usize, bot: usize) {
        let bot = bot.min(self.rows - 1);
        if top >= bot {
            return;
        }
        {
            let screen = self.screen_mut();
            screen.scroll_top = top;
            screen.scroll_bot = bot;
        }
        // DECSTBM homes the cursor (origin-mode aware).
        self.set_position(0, 0);
    }

    // ---- erasing / editing ------------------------------------------------

    pub(crate) fn erase_display(&mut self, mode: usize) {
        self.cursor.pending_wrap = false;
        let blank = self.blank_cell();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let rows = self.rows;
        match mode {
            0 => {
                let screen = self.screen_mut();
                for cell in screen.lines[y].cells[x..].iter_mut() {
                    *cell = blank.clone();
                }
                for row in screen.lines[y + 1..rows.max(y + 1)].iter_mut() {
                    for cell in row.cells.iter_mut() {
                        *cell = blank.clone();
                    }
                    row.wrapped = false;
                }
            }
            1 => {
                let cols = self.cols;
                let screen = self.screen_mut();
                for row in screen.lines[..y].iter_mut() {
                    for cell in row.cells.iter_mut() {
                        *cell = blank.clone();
                    }
                    row.wrapped = false;
                }
                for cell in screen.lines[y].cells[..=x.min(cols - 1)].iter_mut() {
                    *cell = blank.clone();
                }
            }
            2 => {
                let screen = self.screen_mut();
                for row in screen.lines.iter_mut() {
                    for cell in row.cells.iter_mut() {
                        *cell = blank.clone();
                    }
                    row.wrapped = false;
                }
            }
            3 => {
                let screen = self.screen_mut();
                screen.scrollback.clear();
                screen.scrollback_cells = 0;
            }
            _ => {}
        }
    }

    pub(crate) fn erase_line(&mut self, mode: usize) {
        self.cursor.pending_wrap = false;
        let blank = self.blank_cell();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cols = self.cols;
        let row = &mut self.screen_mut().lines[y];
        match mode {
            0 => {
                for cell in row.cells[x..].iter_mut() {
                    *cell = blank.clone();
                }
                row.wrapped = false;
            }
            1 => {
                for cell in row.cells[..=x.min(cols - 1)].iter_mut() {
                    *cell = blank.clone();
                }
            }
            2 => {
                for cell in row.cells.iter_mut() {
                    *cell = blank.clone();
                }
                row.wrapped = false;
            }
            _ => {}
        }
    }

    pub(crate) fn insert_chars(&mut self, n: usize) {
        self.cursor.pending_wrap = false;
        let blank = self.blank_cell();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cols = self.cols;
        let row = &mut self.screen_mut().lines[y];
        for _ in 0..n.min(cols - x) {
            row.cells.insert(x, blank.clone());
        }
        row.cells.truncate(cols);
    }

    pub(crate) fn delete_chars(&mut self, n: usize) {
        self.cursor.pending_wrap = false;
        let blank = self.blank_cell();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cols = self.cols;
        let row = &mut self.screen_mut().lines[y];
        for _ in 0..n.min(cols - x) {
            row.cells.remove(x);
            row.cells.push(blank.clone());
        }
    }

    pub(crate) fn erase_chars(&mut self, n: usize) {
        self.cursor.pending_wrap = false;
        let blank = self.blank_cell();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let end = (x + n.max(1)).min(self.cols);
        let row = &mut self.screen_mut().lines[y];
        for cell in row.cells[x..end].iter_mut() {
            *cell = blank.clone();
        }
    }

    pub(crate) fn insert_lines(&mut self, n: usize) {
        let (top, bot) = {
            let screen = self.screen();
            (screen.scroll_top, screen.scroll_bot)
        };
        let y = self.cursor.y;
        if y < top || y > bot {
            return;
        }
        let n = n.min(bot - y + 1);
        for _ in 0..n {
            let blank = self.blank_row();
            self.screen_mut().lines.remove(bot);
            self.screen_mut().lines.insert(y, blank);
        }
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn delete_lines(&mut self, n: usize) {
        let (top, bot) = {
            let screen = self.screen();
            (screen.scroll_top, screen.scroll_bot)
        };
        let y = self.cursor.y;
        if y < top || y > bot {
            return;
        }
        let n = n.min(bot - y + 1);
        for _ in 0..n {
            let blank = self.blank_row();
            self.screen_mut().lines.remove(y);
            self.screen_mut().lines.insert(bot, blank);
        }
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
    }

    // ---- save/restore + screens -------------------------------------------

    pub(crate) fn save_cursor(&mut self) {
        let saved = SavedCursor {
            x: self.cursor.x,
            y: self.cursor.y,
            style: self.pen,
            origin: self.modes.origin,
            pending_wrap: self.cursor.pending_wrap,
        };
        self.screen_mut().saved = Some(saved);
    }

    pub(crate) fn restore_cursor(&mut self) {
        if let Some(saved) = self.screen().saved {
            self.cursor.x = saved.x.min(self.cols - 1);
            self.cursor.y = saved.y.min(self.rows - 1);
            self.cursor.pending_wrap = saved.pending_wrap;
            self.pen = saved.style;
            self.modes.origin = saved.origin;
        } else {
            self.cursor = Cursor {
                x: 0,
                y: 0,
                pending_wrap: false,
            };
            self.pen = Style::default();
        }
    }

    pub(crate) fn enter_alt_screen(&mut self, clear: bool) {
        if !self.alt_active {
            self.alt_active = true;
        }
        if clear {
            self.alt = Screen::new(self.cols, self.rows);
        }
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn exit_alt_screen(&mut self) {
        if self.alt_active {
            self.alt_active = false;
        }
        self.cursor.x = self.cursor.x.min(self.cols - 1);
        self.cursor.y = self.cursor.y.min(self.rows - 1);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn set_pen(&mut self, pen: Style) {
        self.pen = pen;
    }

    pub(crate) fn pen(&self) -> Style {
        self.pen
    }

    pub(crate) fn full_reset(&mut self) {
        let scrollback = std::mem::take(&mut self.primary.scrollback);
        let scrollback_cells = self.primary.scrollback_cells;
        self.primary = Screen::new(self.cols, self.rows);
        self.primary.scrollback = scrollback;
        self.primary.scrollback_cells = scrollback_cells;
        self.alt = Screen::new(self.cols, self.rows);
        self.alt_active = false;
        self.cursor = Cursor {
            x: 0,
            y: 0,
            pending_wrap: false,
        };
        self.pen = Style::default();
        self.modes = Modes::default();
        self.last_printed = None;
    }

    pub(crate) fn screen_alignment_pattern(&mut self) {
        // DECALN: fill the screen with E, reset margins, home the cursor.
        let cell = Cell {
            ch: 'E',
            width: 1,
            style: Style::default(),
            combining: Vec::new(),
        };
        let rows = self.rows;
        {
            let screen = self.screen_mut();
            screen.scroll_top = 0;
            screen.scroll_bot = rows - 1;
            for row in screen.lines.iter_mut() {
                for c in row.cells.iter_mut() {
                    *c = cell.clone();
                }
            }
        }
        self.cursor = Cursor {
            x: 0,
            y: 0,
            pending_wrap: false,
        };
    }

    // ---- resize ------------------------------------------------------------

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let new_cols = (cols as usize).max(1);
        let new_rows = (rows as usize).max(1);
        if new_cols == self.cols && new_rows == self.rows {
            return;
        }

        let alt_active = self.alt_active;
        let max_cells = self.max_scrollback_cells;
        for (is_active, is_primary, screen) in [
            (!alt_active, true, &mut self.primary),
            (alt_active, false, &mut self.alt),
        ] {
            resize_screen(
                screen,
                new_cols,
                new_rows,
                is_primary,
                max_cells,
                if is_active {
                    Some(&mut self.cursor)
                } else {
                    None
                },
            );
        }

        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor.x = self.cursor.x.min(new_cols - 1);
        self.cursor.y = self.cursor.y.min(new_rows - 1);
        self.cursor.pending_wrap = false;
    }

    // ---- serialization (see serialize.rs) -----------------------------------

    /// Serialize terminal state for session restoration (VT format with
    /// modes/screen/cursor). Returns None if there is no content.
    pub fn serialize_state(&self) -> Option<Vec<u8>> {
        serialize::serialize_state(self)
    }

    /// Serialize terminal content in the specified format.
    pub fn serialize(&self, format: Format) -> Option<Vec<u8>> {
        match format {
            Format::Plain => Some(serialize::serialize_plain(self)),
            Format::Vt => Some(serialize::serialize_vt(self)),
            Format::Html => Some(serialize::serialize_html(self)),
        }
    }

    // Accessors for the serializer.
    fn visible_rows(&self) -> &[Row] {
        &self.screen().lines
    }

    fn primary_rows(&self) -> &[Row] {
        &self.primary.lines
    }

    fn scrollback_rows(&self) -> &VecDeque<Row> {
        &self.primary.scrollback
    }

    fn scroll_region(&self) -> (usize, usize) {
        let screen = self.screen();
        (screen.scroll_top, screen.scroll_bot)
    }
}

fn resize_screen(
    screen: &mut Screen,
    new_cols: usize,
    new_rows: usize,
    is_primary: bool,
    max_cells: usize,
    mut cursor: Option<&mut Cursor>,
) {
    // Column adjustment: pad or truncate every visible row.
    for row in screen.lines.iter_mut() {
        if row.cells.len() < new_cols {
            let style = Style::default();
            row.cells.resize(new_cols, Cell::blank(style));
        } else {
            row.cells.truncate(new_cols);
            // Don't leave a dangling wide-char head at the new edge.
            if let Some(last) = row.cells.last_mut() {
                if last.width == 2 {
                    *last = Cell::blank(Style::default());
                }
            }
        }
    }

    // Row shrink: prefer pushing top rows into scrollback (primary screen)
    // so content above the cursor is preserved; drop bottom rows once the
    // cursor would otherwise be pushed off-screen.
    while screen.lines.len() > new_rows {
        let cursor_y = cursor.as_ref().map(|c| c.y).unwrap_or(0);
        if cursor_y > 0 || cursor.is_none() {
            let removed = screen.lines.remove(0);
            if is_primary {
                screen.scrollback_cells += removed.cells.len();
                screen.scrollback.push_back(removed);
                while screen.scrollback_cells > max_cells {
                    match screen.scrollback.pop_front() {
                        Some(old) => screen.scrollback_cells -= old.cells.len(),
                        None => break,
                    }
                }
            }
            if let Some(c) = cursor.as_deref_mut() {
                if c.y > 0 {
                    c.y -= 1;
                }
            }
        } else {
            screen.lines.pop();
        }
    }

    // Row grow: pull rows back out of the scrollback first, then pad with
    // blanks at the bottom.
    while screen.lines.len() < new_rows {
        if is_primary {
            if let Some(mut row) = screen.scrollback.pop_back() {
                screen.scrollback_cells = screen.scrollback_cells.saturating_sub(row.cells.len());
                if row.cells.len() < new_cols {
                    row.cells.resize(new_cols, Cell::blank(Style::default()));
                } else {
                    row.cells.truncate(new_cols);
                }
                screen.lines.insert(0, row);
                if let Some(c) = cursor.as_deref_mut() {
                    c.y = (c.y + 1).min(new_rows - 1);
                }
                continue;
            }
        }
        screen.lines.push(Row::blank(new_cols, Style::default()));
    }

    screen.scroll_top = 0;
    screen.scroll_bot = new_rows - 1;
    screen.tabstops = default_tabstops(new_cols);
    if let Some(saved) = screen.saved.as_mut() {
        saved.x = saved.x.min(new_cols - 1);
        saved.y = saved.y.min(new_rows - 1);
    }
}

#[cfg(test)]
mod tests;
