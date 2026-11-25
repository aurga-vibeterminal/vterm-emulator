use std::fmt::Write as _;

use crate::{Cell, CellAttributes, Color, MouseEncoding, MouseMode};

use super::terminal::Terminal;

impl Terminal {
    /// Serialize the current visible screen into an ANSI byte sequence that
    /// reproduces this terminal's visible state — grid contents with colors
    /// and attributes, OSC default-color overrides, alt-buffer mode, scroll
    /// region, origin/insert/wrap modes, interactive modes (mouse tracking,
    /// bracketed paste, application cursor/keypad) and the cursor — when
    /// replayed into a fresh emulator of the same size and stream encoding.
    ///
    /// A client attaching to a session whose retained raw history no longer
    /// covers a full repaint cannot reconstruct the screen by replaying the
    /// trimmed byte window (full-screen TUI apps would come up partially
    /// blank). The server renders this snapshot from its authoritative grid
    /// instead; replaying it is deterministic regardless of any earlier
    /// bytes, so older history chunks may still be prepended for scrollback.
    pub fn render_snapshot(&self) -> Vec<u8> {
        let mut out = String::new();
        // Deterministic starting point for painting: default rendition,
        // absolute addressing, autowrap on, replace mode, full scroll
        // region, numeric keypad, all interactive modes off.
        out.push_str("\x1b[0m\x1b[?6l\x1b[?7h\x1b[4l\x1b[r\x1b>");
        out.push_str("\x1b[?1l\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        out.push_str("\x1b[?1005l\x1b[?1006l\x1b[?2004l");
        out.push_str("\x1b]110\x1b\\\x1b]111\x1b\\");
        for (code, color) in [(10, self.osc_default_fg), (11, self.osc_default_bg)] {
            if let Some(Color::Rgb(r, g, b)) = color {
                let _ = write!(out, "\x1b]{code};rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\");
            }
        }
        if self.using_alt_buffer {
            // Entering the alt buffer also clears it in this emulator.
            out.push_str("\x1b[?1049h");
        }
        out.push_str("\x1b[2J\x1b[H");

        let mut hyperlink_id = 0u32;
        for (row_index, row) in self.grid().iter().enumerate() {
            let Some(last) = row.iter().rposition(|cell| !is_default_blank(cell)) else {
                continue;
            };
            let _ = write!(out, "\x1b[{};1H", row_index + 1);
            let mut run_style: Option<(Color, Color, CellAttributes)> = None;
            for cell in row.iter().take(last + 1) {
                // OSC 8 hyperlinks ride along as id transitions; the active
                // link state survives the per-row cursor addressing, so only
                // changes are emitted and any open link is closed below.
                if cell.hyperlink_id != hyperlink_id {
                    hyperlink_id = cell.hyperlink_id;
                    push_hyperlink(
                        &mut out,
                        self.hyperlink_uris.get(&hyperlink_id).map(|u| u.as_ref()),
                    );
                }
                let style = (cell.fg_color, cell.bg_color, cell.attributes);
                if run_style != Some(style) {
                    push_sgr(&mut out, cell);
                    run_style = Some(style);
                }
                if !cell.is_wide_continuation() {
                    out.push(cell.character);
                }
            }
        }
        if hyperlink_id != 0 {
            push_hyperlink(&mut out, None);
        }

        // Restore the modes and cursor the live byte stream expects.
        out.push_str("\x1b[0m");
        let rows = self.size.rows as usize;
        if self.top_margin != 0 || self.bottom_margin != rows - 1 {
            let _ = write!(
                out,
                "\x1b[{};{}r",
                self.top_margin + 1,
                self.bottom_margin + 1
            );
        }
        if self.origin_mode {
            out.push_str("\x1b[?6h");
        }
        if self.insert_mode {
            out.push_str("\x1b[4h");
        }
        if !self.auto_wrap {
            out.push_str("\x1b[?7l");
        }
        // Interactive modes the application enabled and will not re-send.
        match self.mouse_mode {
            MouseMode::None => {}
            MouseMode::X10 => out.push_str("\x1b[?9h"),
            MouseMode::Normal => out.push_str("\x1b[?1000h"),
            MouseMode::ButtonEvent => out.push_str("\x1b[?1002h"),
            MouseMode::AnyEvent => out.push_str("\x1b[?1003h"),
        }
        match self.mouse_encoding {
            MouseEncoding::Default => {}
            MouseEncoding::Utf8 => out.push_str("\x1b[?1005h"),
            MouseEncoding::Sgr => out.push_str("\x1b[?1006h"),
        }
        if self.bracketed_paste {
            out.push_str("\x1b[?2004h");
        }
        if self.app_cursor {
            out.push_str("\x1b[?1h");
        }
        if self.app_keypad {
            out.push_str("\x1b=");
        }
        let (row, col) = self.cursor_pos();
        let row = if self.origin_mode {
            row.saturating_sub(self.top_margin)
        } else {
            row
        };
        let _ = write!(out, "\x1b[{};{}H", row + 1, col + 1);
        out.push_str(if self.cursor_visible {
            "\x1b[?25h"
        } else {
            "\x1b[?25l"
        });

        // The snapshot is built from decoded characters (UTF-8), but
        // clients replay it through `process`, which decodes input with the
        // session's encoding (GBK, Big5, ...). Re-encode the payload into
        // that encoding so the replay decodes back to the same characters
        // instead of mojibake.
        super::terminal::encode_stream_bytes(self.encoding.as_deref(), &out)
    }
}

fn is_default_blank(cell: &Cell) -> bool {
    cell.character == ' '
        && cell.fg_color == Color::Default
        && cell.bg_color == Color::Default
        && (cell.attributes - CellAttributes::WIDE_CONTINUATION).is_empty()
}

/// Emit an OSC 8 transition: open the hyperlink, or close it when `uri`
/// is `None`.
fn push_hyperlink(out: &mut String, uri: Option<&str>) {
    match uri {
        Some(uri) => {
            let _ = write!(out, "\x1b]8;;{uri}\x1b\\");
        }
        None => out.push_str("\x1b]8;;\x1b\\"),
    }
}

fn push_sgr(out: &mut String, cell: &Cell) {
    out.push_str("\x1b[0");
    let attributes = cell.attributes;
    if attributes.contains(CellAttributes::BOLD) {
        out.push_str(";1");
    }
    if attributes.contains(CellAttributes::DIM) {
        out.push_str(";2");
    }
    if attributes.contains(CellAttributes::ITALIC) {
        out.push_str(";3");
    }
    if attributes.contains(CellAttributes::UNDERLINE) {
        out.push_str(";4");
    }
    if attributes.contains(CellAttributes::BLINK) {
        out.push_str(";5");
    }
    if attributes.contains(CellAttributes::INVERSE) {
        out.push_str(";7");
    }
    if attributes.contains(CellAttributes::HIDDEN) {
        out.push_str(";8");
    }
    if attributes.contains(CellAttributes::STRIKETHROUGH) {
        out.push_str(";9");
    }
    push_color(out, false, cell.fg_color);
    push_color(out, true, cell.bg_color);
    out.push('m');
}

fn push_color(out: &mut String, background: bool, color: Color) {
    let named = match color {
        Color::Black => Some(0),
        Color::Red => Some(1),
        Color::Green => Some(2),
        Color::Yellow => Some(3),
        Color::Blue => Some(4),
        Color::Magenta => Some(5),
        Color::Cyan => Some(6),
        Color::White => Some(7),
        Color::BrightBlack => Some(8),
        Color::BrightRed => Some(9),
        Color::BrightGreen => Some(10),
        Color::BrightYellow => Some(11),
        Color::BrightBlue => Some(12),
        Color::BrightMagenta => Some(13),
        Color::BrightCyan => Some(14),
        Color::BrightWhite => Some(15),
        _ => None,
    };
    match (background, color) {
        (false, Color::Default) => out.push_str(";39"),
        (true, Color::Default) => out.push_str(";49"),
        (_, Color::Indexed(index)) => {
            let _ = write!(out, ";{};5;{}", if background { 48 } else { 38 }, index);
        }
        (_, Color::Rgb(r, g, b)) => {
            let _ = write!(
                out,
                ";{};2;{};{};{}",
                if background { 48 } else { 38 },
                r,
                g,
                b
            );
        }
        _ => {
            let index = named.expect("named color covered by match");
            let base = if background { 40 } else { 30 };
            if index < 8 {
                let _ = write!(out, ";{}", base + index);
            } else {
                let _ = write!(out, ";{}", base + 60 + index - 8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalSize;

    fn gbk_terminal(rows: u16, cols: u16) -> Terminal {
        Terminal::new_with_type_and_encoding(TerminalSize::new(rows, cols), None, Some("gbk"))
    }

    /// A snapshot rendered for an encoded session must replay through that
    /// same encoding without mojibake — the tab-switch re-attach scenario.
    #[test]
    fn gbk_snapshot_replays_without_mojibake() {
        let mut term = gbk_terminal(3, 20);
        // GBK bytes for "中a": 中 = D6 D0.
        term.process(&[0xD6, 0xD0, 0x61]);

        let snapshot = term.render_snapshot();
        // The payload must be GBK-encoded, not the UTF-8 of the decoded
        // grid (E4 B8 AD), which a GBK client mis-decodes into mojibake.
        assert!(
            snapshot.windows(2).any(|w| w == [0xD6, 0xD0]),
            "snapshot is not GBK-encoded: {snapshot:?}"
        );

        let mut replayed = gbk_terminal(3, 20);
        replayed.process(&snapshot);
        assert_eq!(replayed.cell(0, 0).unwrap().character, '中');
        assert_eq!(replayed.cell(0, 1).unwrap().character, ' '); // wide continuation
        assert_eq!(replayed.cell(0, 2).unwrap().character, 'a');
    }

    /// A snapshot for an unencoded (UTF-8) session stays UTF-8.
    #[test]
    fn utf8_snapshot_stays_utf8() {
        let mut term = Terminal::new(TerminalSize::new(3, 20));
        term.process("中".as_bytes());
        let snapshot = term.render_snapshot();
        assert!(snapshot.windows(3).any(|w| w == "中".as_bytes()));
    }

    /// Interactive modes (mouse tracking, bracketed paste, application
    /// cursor/keypad) survive a snapshot replay — the tab re-attach scenario
    /// where the original mode-setting sequences have scrolled out of the
    /// retained raw-history window.
    #[test]
    fn snapshot_restores_interactive_modes() {
        let mut term = Terminal::new(TerminalSize::new(3, 20));
        term.process(b"\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b=");

        let snapshot = term.render_snapshot();
        let mut replayed = Terminal::new(TerminalSize::new(3, 20));
        replayed.process(&snapshot);

        assert_eq!(replayed.mouse_mode(), MouseMode::ButtonEvent);
        assert_eq!(replayed.mouse_encoding(), MouseEncoding::Sgr);
        assert!(replayed.bracketed_paste_mode());
        assert!(replayed.app_cursor_mode());
        assert!(replayed.app_keypad_mode());
    }

    /// The snapshot also resets modes deterministically: replaying a
    /// snapshot taken while tracking is off must clear modes set by earlier
    /// bytes, not leave them behind.
    #[test]
    fn snapshot_clears_interactive_modes_when_off() {
        let term = Terminal::new(TerminalSize::new(3, 20));
        let snapshot = term.render_snapshot();

        let mut replayed = Terminal::new(TerminalSize::new(3, 20));
        replayed.process(b"\x1b[?1003h\x1b[?1005h\x1b[?2004h\x1b[?1h\x1b=");
        replayed.process(&snapshot);

        assert_eq!(replayed.mouse_mode(), MouseMode::None);
        assert_eq!(replayed.mouse_encoding(), MouseEncoding::Default);
        assert!(!replayed.bracketed_paste_mode());
        assert!(!replayed.app_cursor_mode());
        assert!(!replayed.app_keypad_mode());
    }

    #[test]
    fn snapshot_restores_osc_default_colors_without_promoting_theme_colors() {
        let mut term = Terminal::new(TerminalSize::new(3, 20));
        term.set_theme_default_colors(
            Some(Color::Rgb(0x11, 0x12, 0x13)),
            Some(Color::Rgb(0x21, 0x22, 0x23)),
        );
        term.process(b"\x1b]10;rgb:31/32/33\x1b\\\x1b]11;rgb:41/42/43\x1b\\");

        let snapshot = term.render_snapshot();
        let mut replayed = Terminal::new(TerminalSize::new(3, 20));
        replayed.process(&snapshot);

        assert_eq!(
            replayed.osc_default_colors(),
            (
                Some(Color::Rgb(0x31, 0x32, 0x33)),
                Some(Color::Rgb(0x41, 0x42, 0x43)),
            )
        );
        assert_eq!(replayed.theme_default_foreground(), None);
        assert_eq!(replayed.theme_default_background(), None);

        let reset = Terminal::new(TerminalSize::new(3, 20)).render_snapshot();
        replayed.process(&reset);
        assert_eq!(replayed.osc_default_colors(), (None, None));
    }

    /// Table-backed encodings (cp437 etc.) re-encode through the reverse
    /// of their decode table.
    #[test]
    fn table_encoding_snapshot_roundtrip() {
        let mut term =
            Terminal::new_with_type_and_encoding(TerminalSize::new(3, 20), None, Some("cp437"));
        // 0x80 decodes to U+00C7 (Ç) under cp437.
        term.process(&[0x80, 0x61]);

        let snapshot = term.render_snapshot();
        assert!(snapshot.windows(2).any(|w| w == [0x80, 0x61]));

        let mut replayed =
            Terminal::new_with_type_and_encoding(TerminalSize::new(3, 20), None, Some("cp437"));
        replayed.process(&snapshot);
        assert_eq!(replayed.cell(0, 0).unwrap().character, '\u{00C7}');
        assert_eq!(replayed.cell(0, 1).unwrap().character, 'a');
    }

    /// UTF-16 sessions re-encode everything (escape sequences included)
    /// into UTF-16 and still replay correctly through the UTF-16 decoder.
    #[test]
    fn utf16le_snapshot_roundtrip() {
        let mut term =
            Terminal::new_with_type_and_encoding(TerminalSize::new(3, 20), None, Some("utf16le"));
        term.process(&[0x6F, 0x00, 0x6B, 0x00]); // "ok"

        let snapshot = term.render_snapshot();
        let mut replayed =
            Terminal::new_with_type_and_encoding(TerminalSize::new(3, 20), None, Some("utf16le"));
        replayed.process(&snapshot);
        assert_eq!(replayed.cell(0, 0).unwrap().character, 'o');
        assert_eq!(replayed.cell(0, 1).unwrap().character, 'k');
    }
}
