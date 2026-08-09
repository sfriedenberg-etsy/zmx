//! Streaming parser that tracks the most recent window title set via the
//! terminal's OSC 0 (icon + window title) or OSC 2 (window title) sequences.
//!
//! zmx passes raw PTY output straight through to attached clients, so a title
//! emitted by the inner shell only reaches the real terminal while a client is
//! attached. On re-attach the daemon replays serialized screen state but never
//! the title, so the outer terminal keeps whatever the previous foreground
//! process last set (see issue #6). This tracker captures the latest title from
//! the PTY byte stream so the daemon can re-emit it on attach.
//!
//! The parser is intentionally minimal: it only recognizes `ESC ] 0 ; <text>`
//! and `ESC ] 2 ; <text>` terminated by BEL (0x07) or ST (`ESC \`). Every other
//! escape/OSC sequence is skipped. State persists across `feed` calls so titles
//! split across PTY reads are handled. Titles longer than `MAX_LEN` bytes are
//! dropped rather than truncated.

pub const MAX_LEN: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Outside any escape sequence.
    Ground,
    /// Saw ESC (0x1b).
    Esc,
    /// Inside `ESC ]`, reading the numeric OSC code up to ';'.
    OscCode,
    /// Inside an OSC 0/2 sequence, collecting title bytes into `pending`.
    Collect,
    /// Saw ESC while collecting; expecting '\' to complete an ST terminator.
    CollectEsc,
    /// Inside an OSC we don't capture; skipping until the terminator.
    Skip,
    /// Saw ESC while skipping; expecting '\' to complete an ST terminator.
    SkipEsc,
}

#[derive(Debug, Clone, Copy)]
pub struct Title<'a> {
    /// The OSC code the title was set with (0 or 2).
    pub code: u8,
    /// The title text (may be empty, meaning the title was cleared).
    pub text: &'a [u8],
}

pub struct TitleTracker {
    state: State,
    /// OSC code accumulated in the OscCode state.
    code_acc: u16,
    /// Title bytes for the in-progress OSC 0/2 sequence.
    pending: Vec<u8>,
    /// Set when the in-progress title overflowed; the sequence is dropped on
    /// completion rather than committing a truncated title.
    pending_overflow: bool,
    /// OSC code (0 or 2) of the in-progress collected title.
    pending_code: u8,
    /// Most recently committed title.
    committed: Vec<u8>,
    committed_code: u8,
    has_title: bool,
}

const BEL: u8 = 0x07;
const ESC: u8 = 0x1b;

impl Default for TitleTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl TitleTracker {
    pub fn new() -> TitleTracker {
        TitleTracker {
            state: State::Ground,
            code_acc: 0,
            pending: Vec::new(),
            pending_overflow: false,
            pending_code: 0,
            committed: Vec::new(),
            committed_code: 0,
            has_title: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.feed_byte(byte);
        }
    }

    fn feed_byte(&mut self, byte: u8) {
        match self.state {
            State::Ground => {
                if byte == ESC {
                    self.state = State::Esc;
                }
            }
            State::Esc => self.after_esc(byte),
            State::OscCode => match byte {
                b'0'..=b'9' => {
                    // Saturate rather than overflow; any absurd code just
                    // ends up in the skip path below once ';' arrives.
                    self.code_acc = self
                        .code_acc
                        .saturating_mul(10)
                        .saturating_add((byte - b'0') as u16);
                }
                b';' => {
                    if self.code_acc == 0 || self.code_acc == 2 {
                        self.state = State::Collect;
                        self.pending.clear();
                        self.pending_overflow = false;
                        self.pending_code = self.code_acc as u8;
                    } else {
                        self.state = State::Skip;
                    }
                }
                ESC => self.state = State::SkipEsc,
                BEL => self.state = State::Ground,
                // Unexpected byte in the code field (e.g. another OSC
                // parameter): treat the rest as an OSC we don't capture.
                _ => self.state = State::Skip,
            },
            State::Collect => match byte {
                BEL => self.commit_pending(),
                ESC => self.state = State::CollectEsc,
                _ => {
                    if self.pending.len() < MAX_LEN {
                        self.pending.push(byte);
                    } else {
                        self.pending_overflow = true;
                    }
                }
            },
            State::CollectEsc => match byte {
                b'\\' => self.commit_pending(),
                // Not an ST: the ESC cancelled the OSC and begins a new escape
                // sequence. Drop the partial title and reinterpret this byte.
                _ => self.after_esc(byte),
            },
            State::Skip => match byte {
                BEL => self.state = State::Ground,
                ESC => self.state = State::SkipEsc,
                _ => {}
            },
            State::SkipEsc => match byte {
                b'\\' => self.state = State::Ground,
                _ => self.after_esc(byte),
            },
        }
    }

    /// Handle the byte following an ESC. ESC begins an escape sequence; we only
    /// care about the OSC introducer `]`. Any other byte (including ESC, which
    /// just restarts the wait) means this is a sequence we don't capture.
    fn after_esc(&mut self, byte: u8) {
        match byte {
            b']' => {
                self.state = State::OscCode;
                self.code_acc = 0;
            }
            ESC => self.state = State::Esc,
            _ => self.state = State::Ground,
        }
    }

    fn commit_pending(&mut self) {
        // Drop overflowed titles instead of committing a truncated one.
        if !self.pending_overflow {
            self.committed.clear();
            self.committed.extend_from_slice(&self.pending);
            self.committed_code = self.pending_code;
            self.has_title = true;
        }
        self.state = State::Ground;
    }

    /// Returns the most recently captured title, or None if none seen yet.
    pub fn current(&self) -> Option<Title<'_>> {
        if !self.has_title {
            return None;
        }
        Some(Title {
            code: self.committed_code,
            text: &self.committed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expect_title(tracker: &TitleTracker, code: u8, text: &str) {
        let t = tracker.current().expect("no title");
        assert_eq!(t.code, code);
        assert_eq!(t.text, text.as_bytes());
    }

    #[test]
    fn captures_osc2_window_title_terminated_by_bel() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;hello world\x07");
        expect_title(&tracker, 2, "hello world");
    }

    #[test]
    fn captures_osc0_title_terminated_by_st() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]0;myterm\x1b\\");
        expect_title(&tracker, 0, "myterm");
    }

    #[test]
    fn most_recent_title_wins() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;first\x07");
        tracker.feed(b"\x1b]2;second\x07");
        expect_title(&tracker, 2, "second");
    }

    #[test]
    fn title_split_across_feeds() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;ti");
        tracker.feed(b"tle\x07");
        expect_title(&tracker, 2, "title");
    }

    #[test]
    fn st_terminator_split_across_feeds() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;x\x1b");
        tracker.feed(b"\\");
        expect_title(&tracker, 2, "x");
    }

    #[test]
    fn ignores_non_title_osc_sequences() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]7;file:///tmp\x07");
        assert!(tracker.current().is_none());
    }

    #[test]
    fn ignores_osc1_icon_name() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]1;iconname\x07");
        assert!(tracker.current().is_none());
    }

    #[test]
    fn title_embedded_in_surrounding_output() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"some text\x1b]2;T\x07more text");
        expect_title(&tracker, 2, "T");
    }

    #[test]
    fn empty_title_clears_the_title() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;set\x07");
        tracker.feed(b"\x1b]2;\x07");
        expect_title(&tracker, 2, "");
    }

    #[test]
    fn non_title_osc_after_a_title_does_not_clobber_it() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;keep\x07");
        tracker.feed(b"\x1b]7;file:///tmp\x07");
        expect_title(&tracker, 2, "keep");
    }

    #[test]
    fn overlong_title_is_dropped_rather_than_truncated() {
        let mut tracker = TitleTracker::new();
        tracker.feed(b"\x1b]2;short\x07");
        tracker.feed(b"\x1b]2;");
        for _ in 0..MAX_LEN + 100 {
            tracker.feed(b"x");
        }
        tracker.feed(b"\x07");
        // The overlong title was discarded; the prior title is still current.
        expect_title(&tracker, 2, "short");
    }

    #[test]
    fn osc2_is_not_confused_by_a_preceding_csi_sequence() {
        let mut tracker = TitleTracker::new();
        // SGR reset (CSI 0 m) then a title; the digits/semicolon in the CSI
        // must not be mistaken for an OSC code.
        tracker.feed(b"\x1b[0m\x1b]2;ok\x07");
        expect_title(&tracker, 2, "ok");
    }

    #[test]
    fn title_after_a_skipped_osc_whose_esc_introduces_the_new_sequence() {
        let mut tracker = TitleTracker::new();
        // An ignored OSC followed immediately by ESC ] (no BEL/ST between them):
        // the ESC must be recognized as starting a fresh OSC, not swallowed.
        tracker.feed(b"\x1b]1;icon\x1b]2;real\x07");
        expect_title(&tracker, 2, "real");
    }
}
