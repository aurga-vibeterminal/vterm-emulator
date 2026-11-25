//! vterm-emulator — Standalone terminal emulator (ANSI/VT100/xterm).
//!
//! Parses raw PTY byte streams into a cell grid with scrollback, SGR
//! attributes, hyperlinks (OSC 8), mouse modes, alternate screen and charset
//! switching. Designed to be embedded in desktop, server and mobile
//! (iOS/Android) applications.
//!
//! Deliberately lightweight: no async runtime, no PTY, no networking. Feed
//! bytes to [`Terminal::process`] and read the grid back out.
//!
//! The grid model types ([`Cell`], [`CellAttributes`], [`Color`],
//! [`TerminalSize`]) live here so the crate is self-contained; `vterm-common`
//! re-exports them for backwards compatibility.

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthChar;

pub mod encoding_tables;

mod eol_mark;
mod helpers;
mod history;
mod io;
mod link;
mod perform;
mod resize;
mod screen;
mod selection;
mod snapshot;
mod terminal;
mod text;
mod types;

#[cfg(test)]
mod tests;

pub use bitflags;
pub use link::{find_url_at, TerminalLink, UrlMatch};
pub use terminal::Terminal;
pub use types::{InteractiveModes, MouseEncoding, MouseMode, TerminalControlSequence, TerminalSequenceCallback, TerminalType};

use eol_mark::EolMarkFilter;
use helpers::*;
use types::{ActiveCharset, ActiveDcs, ApcParser, Charset, SavedCursorState};

// ---------------------------------------------------------------------------
// Grid model types (moved from vterm-common so this crate is self-contained)
// ---------------------------------------------------------------------------

/// Terminal dimensions in rows and columns
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl TerminalSize {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }

    /// Default terminal size (24 rows x 80 columns)
    pub fn default_size() -> Self {
        Self::new(24, 80)
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self::default_size()
    }
}

/// A single cell in the terminal grid
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub character: char,
    pub fg_color: Color,
    pub bg_color: Color,
    pub attributes: CellAttributes,
    /// OSC 8 hyperlink id. Zero means the cell is not part of a hyperlink;
    /// non-zero ids resolve to a URI through the emulator's hyperlink
    /// registry. Stored on the cell so links move with it through scrolls,
    /// erases and reflows without extra bookkeeping.
    #[serde(default)]
    pub hyperlink_id: u32,
}

impl Cell {
    pub fn new(character: char) -> Self {
        Self {
            character,
            fg_color: Color::Default,
            bg_color: Color::Default,
            attributes: CellAttributes::empty(),
            hyperlink_id: 0,
        }
    }

    pub fn empty() -> Self {
        Self::new(' ')
    }

    pub fn display_width_for_char(character: char) -> usize {
        UnicodeWidthChar::width(character).unwrap_or(1).max(1)
    }

    pub fn is_wide_continuation(&self) -> bool {
        self.attributes.contains(CellAttributes::WIDE_CONTINUATION)
    }

    pub fn display_width(&self) -> usize {
        if self.is_wide_continuation() {
            0
        } else {
            Self::display_width_for_char(self.character)
        }
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::empty()
    }
}

bitflags::bitflags! {
    /// Cell attributes (bold, italic, underline, etc.)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CellAttributes: u16 {
        const BOLD = 0x01;
        const ITALIC = 0x02;
        const UNDERLINE = 0x04;
        const STRIKETHROUGH = 0x08;
        const INVERSE = 0x10;
        const BLINK = 0x20;
        const HIDDEN = 0x40;
        const WIDE_CONTINUATION = 0x80;
        /// SGR 2 — dim/faint: rendered by blending the foreground toward the
        /// background (half intensity). Used by PSReadLine prediction text and
        /// other tools to distinguish generated/suggestion text from input.
        const DIM = 0x100;
    }
}

/// ANSI color support
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    Default,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Indexed(u8),     // 256 color palette (0-255)
    Rgb(u8, u8, u8), // True color
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}
