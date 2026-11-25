//! Filter for zsh's PROMPT_SP partial-line marker sequence.
//!
//! Before every prompt zsh emits (see `preprompt()` in zsh's `Src/utils.c`):
//!
//! ```text
//! ESC[1m ESC[7m "%" ESC[27m ESC[1m ESC[0m  <fill spaces>  CR  SP  CR
//! ```
//!
//! (`#` instead of `%` for root, from the `%#` prompt escape.)
//!
//! The marker is designed to erase itself: the fill spaces reach the last
//! column (arming the deferred wrap at most), the carriage returns then let
//! the following prompt redraw overwrite everything. That only works while
//! the terminal width matches the width zsh used for the fill. vterm
//! replays raw session history (when a new viewer attaches, or when the
//! client rebuilds its emulator), and a marker emitted at a wider width
//! wraps during replay, leaving a stray inverse-video `%` line permanently
//! burned into the grid.
//!
//! This filter recognizes the sequence in the byte stream and drops it only
//! when it would wrap (cursor at column 0 and the fill overflows the
//! current width) — precisely the case that produces the artifact. When the
//! sequence fits the current width it is passed through verbatim, so the
//! genuine partial-line preservation (a `%` marker after output that did
//! not end with a newline) keeps working.
//!
//! The filter emits surviving bytes to the parser immediately, so the width
//! decision at the end of a candidate sequence observes the exact terminal
//! state produced by everything before it.

use vte::Parser;

use super::Terminal;

/// `ESC[1m ESC[7m "%" ESC[27m ESC[1m ESC[0m` — the default PROMPT_EOL_MARK
/// expansion. Index 8 is the mark character (`%` for users, `#` for root).
const MARK_PREFIX: &[u8; 22] = b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m";
const MARK_CHAR_INDEX: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilterState {
    Normal,
    /// `held` currently holds a strict prefix of `MARK_PREFIX`.
    Prefix,
    /// Full prefix matched; consuming fill spaces.
    Fill,
    /// Fill spaces ended with CR.
    FillSawCr,
    /// Fill ended with CR SP; one more CR completes the sequence.
    FillSawCrSp,
}

#[derive(Default)]
pub(super) struct EolMarkFilter {
    state: FilterState,
    /// Bytes consumed by a candidate sequence but not yet emitted.
    held: Vec<u8>,
    fill_spaces: usize,
}

impl Default for FilterState {
    fn default() -> Self {
        FilterState::Normal
    }
}

impl EolMarkFilter {
    /// Take any held-but-undecided bytes. Used on resize: the width the
    /// decision depends on is about to change, so the candidate is released
    /// verbatim under the old width.
    pub fn take_held(&mut self) -> Vec<u8> {
        self.state = FilterState::Normal;
        std::mem::take(&mut self.held)
    }

    /// Feed one input chunk through the filter, advancing `parser` with the
    /// surviving bytes.
    pub fn feed(&mut self, term: &mut Terminal, parser: &mut Parser, data: &[u8]) {
        // Fast path: nothing held, no escape byte — the sequence always
        // starts with ESC, so the chunk cannot contain it.
        if self.state == FilterState::Normal && self.held.is_empty() && !data.contains(&0x1B) {
            parser.advance(term, data);
            return;
        }

        // Start of the current pass-through segment in `data`, if any.
        // Bytes are only appended to a segment while in Normal state;
        // candidate bytes go to `held` instead.
        let mut seg_start: Option<usize> = None;
        let mut i = 0;
        while i < data.len() {
            let byte = data[i];
            let mut consume = true;
            match self.state {
                FilterState::Normal => {
                    if byte == 0x1B {
                        if let Some(start) = seg_start.take() {
                            if start < i {
                                parser.advance(term, &data[start..i]);
                            }
                        }
                        self.held.push(byte);
                        self.state = FilterState::Prefix;
                    } else if seg_start.is_none() {
                        seg_start = Some(i);
                    }
                }
                FilterState::Prefix => {
                    let expected = MARK_PREFIX[self.held.len()];
                    let matches = if self.held.len() == MARK_CHAR_INDEX {
                        byte == b'%' || byte == b'#'
                    } else {
                        byte == expected
                    };
                    if matches {
                        self.held.push(byte);
                        if self.held.len() == MARK_PREFIX.len() {
                            self.state = FilterState::Fill;
                            self.fill_spaces = 0;
                        }
                    } else {
                        self.flush(term, parser);
                        consume = false;
                    }
                }
                FilterState::Fill => match byte {
                    b' ' => {
                        self.held.push(byte);
                        self.fill_spaces += 1;
                    }
                    b'\r' => {
                        self.held.push(byte);
                        self.state = FilterState::FillSawCr;
                    }
                    _ => {
                        self.flush(term, parser);
                        consume = false;
                    }
                },
                FilterState::FillSawCr => {
                    if byte == b' ' {
                        self.held.push(byte);
                        self.state = FilterState::FillSawCrSp;
                    } else {
                        self.flush(term, parser);
                        consume = false;
                    }
                }
                FilterState::FillSawCrSp => {
                    if byte == b'\r' {
                        self.held.push(byte);
                        // The sequence wraps (and leaves a stray inverse `%`
                        // line) iff the fill overflows the current width
                        // from column 0. Writing exactly up to the last
                        // column only arms the deferred wrap, which the
                        // trailing CR cancels — that is the self-erasing
                        // case, so `>=` is the exact boundary.
                        let wraps = term.auto_wrap
                            && term.cursor_pos.1 == 0
                            && self.fill_spaces >= term.size.cols as usize;
                        if !wraps {
                            parser.advance(term, &self.held);
                        }
                        self.held.clear();
                        self.state = FilterState::Normal;
                    } else {
                        self.flush(term, parser);
                        consume = false;
                    }
                }
            }
            if consume {
                i += 1;
            }
        }
        if let Some(start) = seg_start {
            if start < data.len() {
                parser.advance(term, &data[start..]);
            }
        }
    }

    fn flush(&mut self, term: &mut Terminal, parser: &mut Parser) {
        if !self.held.is_empty() {
            parser.advance(term, &self.held);
            self.held.clear();
        }
        self.state = FilterState::Normal;
    }
}
