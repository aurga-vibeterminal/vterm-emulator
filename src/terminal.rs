use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::{Cell, CellAttributes, Color, TerminalSize};
use tracing::debug;
use vte::Parser;

use super::*;

static NEXT_TERMINAL_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

pub(super) enum TerminalDecoder {
    Utf8,
    EncodingRs(encoding_rs::Decoder),
    Table(&'static [Option<char>; 128]),
}

impl TerminalDecoder {
    fn from_label(label: Option<&str>) -> Self {
        let Some(label) = label else {
            return Self::Utf8;
        };
        let normalized = label.trim().to_ascii_lowercase();
        let encoding = match normalized.as_str() {
            "utf16le" => Some(encoding_rs::UTF_16LE),
            "utf16be" => Some(encoding_rs::UTF_16BE),
            "gbk" => Some(encoding_rs::GBK),
            "gb18030" | "gb2312" => Some(encoding_rs::GB18030),
            "cp950" | "big5hkscs" => Some(encoding_rs::BIG5),
            "shiftjis" => Some(encoding_rs::SHIFT_JIS),
            "eucjp" => Some(encoding_rs::EUC_JP),
            "euckr" => Some(encoding_rs::EUC_KR),
            "windows1252" | "iso88591" => Some(encoding_rs::WINDOWS_1252),
            "iso885915" => Some(encoding_rs::ISO_8859_15),
            "macroman" => Some(encoding_rs::MACINTOSH),
            "windows1250" => Some(encoding_rs::WINDOWS_1250),
            "iso88592" => Some(encoding_rs::ISO_8859_2),
            "windows1257" => Some(encoding_rs::WINDOWS_1257),
            "iso88594" => Some(encoding_rs::ISO_8859_4),
            "iso885914" => Some(encoding_rs::ISO_8859_14),
            "iso885910" => Some(encoding_rs::ISO_8859_10),
            "iso885913" => Some(encoding_rs::ISO_8859_13),
            "windows1251" => Some(encoding_rs::WINDOWS_1251),
            "koi8r" => Some(encoding_rs::KOI8_R),
            "koi8u" => Some(encoding_rs::KOI8_U),
            "iso88595" => Some(encoding_rs::ISO_8859_5),
            "cp866" => Some(encoding_rs::IBM866),
            "windows1253" => Some(encoding_rs::WINDOWS_1253),
            "iso88597" => Some(encoding_rs::ISO_8859_7),
            "windows1254" | "iso88599" => Some(encoding_rs::WINDOWS_1254),
            "windows1255" => Some(encoding_rs::WINDOWS_1255),
            "iso88598" => Some(encoding_rs::ISO_8859_8),
            "windows1256" => Some(encoding_rs::WINDOWS_1256),
            "iso88596" => Some(encoding_rs::ISO_8859_6),
            "windows874" | "iso885911" => Some(encoding_rs::WINDOWS_874),
            "iso885916" => Some(encoding_rs::ISO_8859_16),
            "windows1258" => Some(encoding_rs::WINDOWS_1258),
            _ => None,
        };
        if let Some(encoding) = encoding {
            return Self::EncodingRs(encoding.new_decoder_with_bom_removal());
        }
        match normalized.as_str() {
            "iso88593" => Self::Table(&crate::encoding_tables::ISO_8859_3),
            "cp437" => Self::Table(&crate::encoding_tables::CP437),
            "cp850" => Self::Table(&crate::encoding_tables::CP850),
            "cp852" => Self::Table(&crate::encoding_tables::CP852),
            "cp857" => Self::Table(&crate::encoding_tables::CP857),
            "cp865" => Self::Table(&crate::encoding_tables::CP865),
            "cp1125" => Self::Table(&crate::encoding_tables::CP1125),
            "koi8t" => Self::Table(&crate::encoding_tables::KOI8_T),
            "koi8ru" => Self::Table(&crate::encoding_tables::KOI8_RU),
            _ => Self::Utf8,
        }
    }

    pub(super) fn decode(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        match self {
            Self::Utf8 => None,
            Self::EncodingRs(decoder) => {
                let capacity = decoder
                    .max_utf8_buffer_length(data.len())
                    .unwrap_or_else(|| data.len().saturating_mul(3).saturating_add(4));
                let mut output = String::with_capacity(capacity);
                let (_, read, _) = decoder.decode_to_string(data, &mut output, false);
                debug_assert_eq!(read, data.len());
                Some(output.into_bytes())
            }
            Self::Table(table) => {
                let mut output = String::with_capacity(data.len());
                for &byte in data {
                    if byte < 0x80 {
                        output.push(byte as char);
                    } else {
                        output.push(table[(byte - 0x80) as usize].unwrap_or('\u{FFFD}'));
                    }
                }
                Some(output.into_bytes())
            }
        }
    }
}

fn fresh_terminal_instance_id() -> u64 {
    // One relaxed atomic operation per Terminal construction keeps line
    // revisions from different replacement instances from aliasing in UI
    // caches. Zero is reserved as an invalid/sentinel instance ID.
    NEXT_TERMINAL_INSTANCE_ID
        .fetch_add(1, Ordering::Relaxed)
        .max(1)
}

/// The encoding_rs encoding backing a normalized stream label, mirroring
/// [`TerminalDecoder::from_label`]'s encoding_rs subset for the encode
/// direction.
fn stream_encoding(label: &str) -> Option<&'static encoding_rs::Encoding> {
    Some(match label {
        "utf16le" => encoding_rs::UTF_16LE,
        "utf16be" => encoding_rs::UTF_16BE,
        "gbk" => encoding_rs::GBK,
        "gb18030" | "gb2312" => encoding_rs::GB18030,
        "cp950" | "big5hkscs" => encoding_rs::BIG5,
        "shiftjis" => encoding_rs::SHIFT_JIS,
        "eucjp" => encoding_rs::EUC_JP,
        "euckr" => encoding_rs::EUC_KR,
        "windows1252" | "iso88591" => encoding_rs::WINDOWS_1252,
        "iso885915" => encoding_rs::ISO_8859_15,
        "macroman" => encoding_rs::MACINTOSH,
        "windows1250" => encoding_rs::WINDOWS_1250,
        "iso88592" => encoding_rs::ISO_8859_2,
        "windows1257" => encoding_rs::WINDOWS_1257,
        "iso88594" => encoding_rs::ISO_8859_4,
        "iso885914" => encoding_rs::ISO_8859_14,
        "iso885910" => encoding_rs::ISO_8859_10,
        "iso885913" => encoding_rs::ISO_8859_13,
        "windows1251" => encoding_rs::WINDOWS_1251,
        "koi8r" => encoding_rs::KOI8_R,
        "koi8u" => encoding_rs::KOI8_U,
        "iso88595" => encoding_rs::ISO_8859_5,
        "cp866" => encoding_rs::IBM866,
        "windows1253" => encoding_rs::WINDOWS_1253,
        "windows1254" | "iso88599" => encoding_rs::WINDOWS_1254,
        "windows1255" => encoding_rs::WINDOWS_1255,
        "windows1256" => encoding_rs::WINDOWS_1256,
        "iso88598" => encoding_rs::ISO_8859_8,
        "windows1258" => encoding_rs::WINDOWS_1258,
        _ => return None,
    })
}

/// The static decode table backing a normalized stream label, mirroring
/// [`TerminalDecoder::from_label`]'s table subset.
fn table_encoding(label: &str) -> Option<&'static [Option<char>; 128]> {
    Some(match label {
        "iso88593" => &crate::encoding_tables::ISO_8859_3,
        "cp437" => &crate::encoding_tables::CP437,
        "cp850" => &crate::encoding_tables::CP850,
        "cp852" => &crate::encoding_tables::CP852,
        "cp857" => &crate::encoding_tables::CP857,
        "cp865" => &crate::encoding_tables::CP865,
        "cp1125" => &crate::encoding_tables::CP1125,
        "koi8t" => &crate::encoding_tables::KOI8_T,
        "koi8ru" => &crate::encoding_tables::KOI8_RU,
        _ => return None,
    })
}

/// Encode internal UTF-8 text back into the terminal's configured stream
/// encoding, so artifacts re-fed through `process` (e.g. a rendered
/// snapshot replayed on a client that decodes input with the same
/// encoding label) decode back to the same characters instead of
/// mojibake. Unmappable characters degrade to `?`. UTF-8 or unset labels
/// pass through unchanged.
pub(super) fn encode_stream_bytes(encoding: Option<&str>, text: &str) -> Vec<u8> {
    let Some(label) = encoding.map(str::trim).filter(|label| !label.is_empty()) else {
        return text.as_bytes().to_vec();
    };
    let normalized = label.to_ascii_lowercase();
    if normalized == "utf8" || normalized == "utf-8" {
        return text.as_bytes().to_vec();
    }

    // encoding_rs provides no encoder for UTF-16 (per WHATWG its output
    // encoding is UTF-8), so encode it by hand.
    match normalized.as_str() {
        "utf16le" | "utf16be" => {
            let big_endian = normalized == "utf16be";
            let mut out = Vec::with_capacity(text.len() * 2);
            for unit in text.encode_utf16() {
                let bytes = if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                };
                out.extend_from_slice(&bytes);
            }
            return out;
        }
        _ => {}
    }

    if let Some(encoding) = stream_encoding(&normalized) {
        let mut encoder = encoding.new_encoder();
        let mut out = Vec::with_capacity(text.len());
        let mut src_buf = [0u8; 4];
        let mut dst_buf = [0u8; 16];
        for c in text.chars() {
            let src = c.encode_utf8(&mut src_buf);
            let (result, _, written) =
                encoder.encode_from_utf8_without_replacement(src, &mut dst_buf, true);
            out.extend_from_slice(&dst_buf[..written]);
            // An unmappable char (impossible for text that was decoded
            // from this same encoding, but defensive) or a too-small
            // buffer (16 bytes always fit one char in these encodings)
            // degrades to `?` instead of corrupting the stream.
            if !matches!(result, encoding_rs::EncoderResult::InputEmpty) {
                out.push(b'?');
            }
        }
        return out;
    }

    if let Some(table) = table_encoding(&normalized) {
        let mut out = Vec::with_capacity(text.len());
        for c in text.chars() {
            if c.is_ascii() {
                out.push(c as u8);
            } else if let Some(index) = table.iter().position(|&mapped| mapped == Some(c)) {
                out.push(0x80 + index as u8);
            } else {
                out.push(b'?');
            }
        }
        return out;
    }

    text.as_bytes().to_vec()
}

/// Default character cell metrics reported in XTWINOPS pixel-size replies
/// until the front-end supplies real font metrics (a common monospace cell
/// at default font sizes).
pub const DEFAULT_CELL_WIDTH_PX: u16 = 10;
pub const DEFAULT_CELL_HEIGHT_PX: u16 = 20;

pub struct Terminal {
    /// Distinguishes wholesale Terminal replacements that may reuse local
    /// line-revision numbers.
    pub(super) instance_id: u64,
    pub(super) terminal_type: TerminalType,
    pub(super) encoding: Option<String>,
    pub(super) input_decoder: TerminalDecoder,
    /// Current terminal size
    pub(super) size: TerminalSize,
    /// Character cell width in pixels, used to answer XTWINOPS pixel queries
    /// (`CSI 14/15/16 t`). The embedding application may override the default
    /// with real font metrics via `set_cell_pixel_size`.
    pub(super) cell_width_px: u16,
    /// Character cell height in pixels (see `cell_width_px`).
    pub(super) cell_height_px: u16,
    /// Primary screen buffer
    pub(super) grid: Vec<Vec<Cell>>,
    /// Alternate screen buffer (for full-screen apps like vim)
    pub(super) alt_grid: Vec<Vec<Cell>>,
    /// Content revision for each primary-grid row. Revisions move with rows
    /// during scrolling so renderers can detect changes without hashing cells.
    pub(super) grid_revisions: Vec<u64>,
    /// Content revision for each alternate-grid row.
    pub(super) alt_grid_revisions: Vec<u64>,
    /// Per-row soft-wrap flags for the primary grid
    pub(super) row_wrapped: Vec<bool>,
    /// Per-row soft-wrap flags for the alternate grid
    pub(super) alt_row_wrapped: Vec<bool>,
    /// Using alternate screen buffer
    pub(super) using_alt_buffer: bool,
    /// Scrollback buffer
    pub(super) scrollback: VecDeque<Vec<Cell>>,
    /// Content revision for each scrollback row.
    pub(super) scrollback_revisions: VecDeque<u64>,
    /// Per-row soft-wrap flags for the scrollback buffer
    pub(super) scrollback_wrapped: VecDeque<bool>,
    /// Running count of non-wrapped scrollback rows, used to compute the
    /// logical line count in O(1) instead of rescanning the flags.
    pub(super) scrollback_logical_lines: usize,
    /// Maximum scrollback lines
    pub(super) max_scrollback: usize,
    /// Next non-zero content revision allocated by this terminal.
    pub(super) next_line_revision: u64,
    /// Cursor position (row, col)
    pub(super) cursor_pos: (usize, usize),
    /// Last printable cell filled the row and the next printable character should wrap first.
    pub(super) pending_wrap: bool,
    /// Saved cursor position
    pub(super) saved_cursor_pos: (usize, usize),
    /// Saved cursor state
    pub(super) saved_cursor_state: SavedCursorState,
    /// Saved cursor state for the alternate screen
    pub(super) alt_saved_cursor_state: SavedCursorState,
    /// Cursor visibility
    pub(super) cursor_visible: bool,
    /// Current foreground color
    pub(super) fg_color: Color,
    /// Current background color
    pub(super) bg_color: Color,
    /// Current cell attributes
    pub(super) attributes: CellAttributes,
    /// Last printable graphic character, used by REP (CSI Ps b).
    pub(super) last_printed_char: char,
    /// VTE parser for escape sequences
    pub(super) parser: Option<Parser>,
    /// zsh PROMPT_SP end-of-line marker filter (see eol_mark.rs)
    pub(super) eol_mark_filter: EolMarkFilter,
    /// Auto-wrap mode
    pub(super) auto_wrap: bool,
    /// Insert mode
    pub(super) insert_mode: bool,
    /// Origin mode (relative to scroll region)
    pub(super) origin_mode: bool,
    /// Top margin for scrolling region
    pub(super) top_margin: usize,
    /// Bottom margin for scrolling region
    pub(super) bottom_margin: usize,
    /// Tab stops
    pub(super) tab_stops: Vec<bool>,
    /// Application keypad mode
    pub(super) app_keypad: bool,
    /// Application cursor keys mode
    pub(super) app_cursor: bool,
    /// Mouse protocol mode
    pub(super) mouse_mode: MouseMode,
    /// Mouse report encoding extension
    pub(super) mouse_encoding: MouseEncoding,
    /// Bracketed paste mode
    pub(super) bracketed_paste: bool,
    /// Kitty keyboard protocol progressive-enhancement flags of the active
    /// screen (bitmask: 1 disambiguate, 2 event types, 4 alternate keys,
    /// 8 all keys as escape codes, 16 associated text).
    pub(super) kitty_keyboard_flags: u16,
    /// Kitty keyboard flag stack of the active screen (`CSI > flags u` /
    /// `CSI < u`). Main and alternate screens keep independent stacks.
    pub(super) kitty_keyboard_stack: Vec<u16>,
    /// Kitty keyboard state (flags, stack) of the inactive screen, swapped in
    /// on alt-buffer transitions.
    pub(super) inactive_kitty_keyboard: (u16, Vec<u16>),
    /// Emulator-originated bytes that must be written back to the PTY input
    /// (e.g. the kitty keyboard flags query reply `CSI ? flags u`). Drained
    /// by the embedding application after `process`.
    pub(super) pending_query_responses: Vec<u8>,
    /// Host UI's default foreground/background colors (`None` = unknown).
    /// Reported by the embedding application through
    /// [`Terminal::set_theme_default_colors`] so the emulator can answer
    /// OSC 10/11 color queries on the terminal's behalf.
    pub(super) theme_default_fg: Option<Color>,
    pub(super) theme_default_bg: Option<Color>,
    /// Default foreground/background colors set by the application via
    /// OSC 10/11. Take precedence over the host's colors until OSC 110/111
    /// resets them.
    pub(super) osc_default_fg: Option<Color>,
    pub(super) osc_default_bg: Option<Color>,
    /// Whether query replies should be buffered in `pending_query_responses`
    /// for the embedding application to write back to the PTY. Only the
    /// PTY-owning side should respond; viewer mirrors of a server-side
    /// stream disable this so replies are not generated twice.
    pub(super) query_responder: bool,
    /// G0 character set
    pub(super) g0_charset: Charset,
    /// G1 character set
    pub(super) g1_charset: Charset,
    /// Active character set
    pub(super) active_charset: ActiveCharset,
    /// Title stack
    pub(super) title_stack: Vec<String>,
    /// Current title
    pub(super) title: String,
    /// Current working directory reported via OSC 7.
    pub(super) cwd: Option<String>,
    /// Whether the working directory changed since the last poll.
    pub(super) cwd_dirty: bool,
    /// Dirty flag for redraw optimization
    pub(super) dirty: bool,
    /// Selection (start_row, start_col, end_row, end_col)
    pub(super) selection: Option<(usize, usize, usize, usize)>,
    /// Optional observer for parsed control sequences.
    pub(super) sequence_callback: Option<TerminalSequenceCallback>,
    /// Which sequence types the callback is interested in ("osc", "apc", "esc").
    pub(super) sequence_callback_types: HashSet<String>,
    /// Active DCS state accumulated from vte hook/put/unhook callbacks.
    pub(super) active_dcs: Option<ActiveDcs>,
    /// Lightweight APC recognizer. vte consumes APC without surfacing it through Perform.
    pub(super) apc_parser: ApcParser,
    /// OSC 8 hyperlink id currently applied to printed cells. Zero means no
    /// active hyperlink.
    pub(super) active_hyperlink_id: u32,
    /// Next OSC 8 hyperlink id to allocate.
    pub(super) next_hyperlink_id: u32,
    /// OSC 8 hyperlink id → URI registry. Cells carry the id (see
    /// `Cell::hyperlink_id`); this map resolves it. Equal URIs share one id.
    pub(super) hyperlink_uris: HashMap<u32, Arc<str>>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("instance_id", &self.instance_id)
            .field("terminal_type", &self.terminal_type)
            .field("size", &self.size)
            .field("grid", &self.grid)
            .field("alt_grid", &self.alt_grid)
            .field("grid_revisions", &self.grid_revisions)
            .field("alt_grid_revisions", &self.alt_grid_revisions)
            .field("using_alt_buffer", &self.using_alt_buffer)
            .field("scrollback", &self.scrollback)
            .field("scrollback_revisions", &self.scrollback_revisions)
            .field("scrollback_wrapped", &self.scrollback_wrapped)
            .field("max_scrollback", &self.max_scrollback)
            .field("cursor_pos", &self.cursor_pos)
            .field("pending_wrap", &self.pending_wrap)
            .field("saved_cursor_pos", &self.saved_cursor_pos)
            .field("saved_cursor_state", &self.saved_cursor_state)
            .field("alt_saved_cursor_state", &self.alt_saved_cursor_state)
            .field("cursor_visible", &self.cursor_visible)
            .field("fg_color", &self.fg_color)
            .field("bg_color", &self.bg_color)
            .field("theme_default_fg", &self.theme_default_fg)
            .field("theme_default_bg", &self.theme_default_bg)
            .field("osc_default_fg", &self.osc_default_fg)
            .field("osc_default_bg", &self.osc_default_bg)
            .field("query_responder", &self.query_responder)
            .field("attributes", &self.attributes)
            .field("last_printed_char", &self.last_printed_char)
            .field("parser", &"<Parser>")
            .field("auto_wrap", &self.auto_wrap)
            .field("insert_mode", &self.insert_mode)
            .field("origin_mode", &self.origin_mode)
            .field("top_margin", &self.top_margin)
            .field("bottom_margin", &self.bottom_margin)
            .field("tab_stops", &self.tab_stops)
            .field("app_keypad", &self.app_keypad)
            .field("app_cursor", &self.app_cursor)
            .field("mouse_mode", &self.mouse_mode)
            .field("bracketed_paste", &self.bracketed_paste)
            .field("kitty_keyboard_flags", &self.kitty_keyboard_flags)
            .field("g0_charset", &self.g0_charset)
            .field("g1_charset", &self.g1_charset)
            .field("active_charset", &self.active_charset)
            .field("title_stack", &self.title_stack)
            .field("title", &self.title)
            .field("cwd", &self.cwd)
            .field("cwd_dirty", &self.cwd_dirty)
            .field("dirty", &self.dirty)
            .field("selection", &self.selection)
            .field("sequence_callback", &self.sequence_callback.is_some())
            .field("active_dcs", &self.active_dcs)
            .field("apc_parser", &self.apc_parser)
            .field("active_hyperlink_id", &self.active_hyperlink_id)
            .field("hyperlink_uris", &self.hyperlink_uris.len())
            .finish()
    }
}

impl Clone for Terminal {
    fn clone(&self) -> Self {
        Self {
            instance_id: fresh_terminal_instance_id(),
            terminal_type: self.terminal_type,
            encoding: self.encoding.clone(),
            input_decoder: TerminalDecoder::from_label(self.encoding.as_deref()),
            size: self.size,
            cell_width_px: self.cell_width_px,
            cell_height_px: self.cell_height_px,
            grid: self.grid.clone(),
            alt_grid: self.alt_grid.clone(),
            grid_revisions: self.grid_revisions.clone(),
            alt_grid_revisions: self.alt_grid_revisions.clone(),
            row_wrapped: self.row_wrapped.clone(),
            alt_row_wrapped: self.alt_row_wrapped.clone(),
            using_alt_buffer: self.using_alt_buffer,
            scrollback: self.scrollback.clone(),
            scrollback_revisions: self.scrollback_revisions.clone(),
            scrollback_wrapped: self.scrollback_wrapped.clone(),
            scrollback_logical_lines: self.scrollback_logical_lines,
            max_scrollback: self.max_scrollback,
            next_line_revision: self.next_line_revision,
            cursor_pos: self.cursor_pos,
            pending_wrap: self.pending_wrap,
            saved_cursor_pos: self.saved_cursor_pos,
            saved_cursor_state: self.saved_cursor_state,
            alt_saved_cursor_state: self.alt_saved_cursor_state,
            cursor_visible: self.cursor_visible,
            fg_color: self.fg_color,
            bg_color: self.bg_color,
            theme_default_fg: self.theme_default_fg,
            theme_default_bg: self.theme_default_bg,
            osc_default_fg: self.osc_default_fg,
            osc_default_bg: self.osc_default_bg,
            query_responder: self.query_responder,
            attributes: self.attributes,
            last_printed_char: self.last_printed_char,
            parser: Some(Parser::new()), // Create new parser on clone
            eol_mark_filter: EolMarkFilter::default(),
            auto_wrap: self.auto_wrap,
            insert_mode: self.insert_mode,
            origin_mode: self.origin_mode,
            top_margin: self.top_margin,
            bottom_margin: self.bottom_margin,
            tab_stops: self.tab_stops.clone(),
            app_keypad: self.app_keypad,
            app_cursor: self.app_cursor,
            mouse_mode: self.mouse_mode,
            mouse_encoding: self.mouse_encoding,
            bracketed_paste: self.bracketed_paste,
            kitty_keyboard_flags: self.kitty_keyboard_flags,
            kitty_keyboard_stack: self.kitty_keyboard_stack.clone(),
            inactive_kitty_keyboard: self.inactive_kitty_keyboard.clone(),
            pending_query_responses: Vec::new(),
            g0_charset: self.g0_charset,
            g1_charset: self.g1_charset,
            active_charset: self.active_charset,
            title_stack: self.title_stack.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            cwd_dirty: self.cwd_dirty,
            dirty: self.dirty,
            selection: self.selection,
            sequence_callback: None,
            sequence_callback_types: HashSet::new(),
            active_dcs: None,
            apc_parser: ApcParser::default(),
            active_hyperlink_id: self.active_hyperlink_id,
            next_hyperlink_id: self.next_hyperlink_id,
            hyperlink_uris: self.hyperlink_uris.clone(),
        }
    }
}

impl Terminal {
    pub fn new(size: TerminalSize) -> Self {
        Self::new_with_type(size, None)
    }

    pub fn new_with_type(size: TerminalSize, terminal_type: Option<&str>) -> Self {
        Self::new_with_type_and_encoding(size, terminal_type, None)
    }

    pub fn new_with_type_and_encoding(
        size: TerminalSize,
        terminal_type: Option<&str>,
        encoding: Option<&str>,
    ) -> Self {
        //         debug!(
        //             "[Emulator] new_with_type: {}x{}, type={:?}",
        //             size.rows, size.cols, terminal_type
        //         );
        let rows = size.rows as usize;
        let cols = size.cols as usize;
        let terminal_type = TerminalType::from_name(terminal_type);
        let encoding = encoding.map(str::to_owned);

        let mut tab_stops = vec![false; cols];
        for i in (0..cols).step_by(8) {
            tab_stops[i] = true;
        }

        // Revisions are unique per row, including across the primary and
        // alternate buffers. Zero is reserved for a missing line.
        let grid_revisions = (1..=rows as u64).collect();
        let alt_revision_start = rows as u64 + 1;
        let alt_grid_revisions = (alt_revision_start..alt_revision_start + rows as u64).collect();
        let next_line_revision = alt_revision_start + rows as u64;

        Self {
            instance_id: fresh_terminal_instance_id(),
            terminal_type,
            input_decoder: TerminalDecoder::from_label(encoding.as_deref()),
            encoding,
            size,
            cell_width_px: DEFAULT_CELL_WIDTH_PX,
            cell_height_px: DEFAULT_CELL_HEIGHT_PX,
            grid: create_grid(rows, cols),
            alt_grid: create_grid(rows, cols),
            grid_revisions,
            alt_grid_revisions,
            row_wrapped: vec![false; rows],
            alt_row_wrapped: vec![false; rows],
            using_alt_buffer: false,
            scrollback: VecDeque::with_capacity(10000),
            scrollback_revisions: VecDeque::with_capacity(10000),
            scrollback_wrapped: VecDeque::with_capacity(10000),
            scrollback_logical_lines: 0,
            max_scrollback: 10000,
            next_line_revision,
            cursor_pos: (0, 0),
            pending_wrap: false,
            saved_cursor_pos: (0, 0),
            saved_cursor_state: SavedCursorState {
                cursor_pos: (0, 0),
                pending_wrap: false,
                fg_color: Color::Default,
                bg_color: Color::Default,
                attributes: CellAttributes::empty(),
                g0_charset: Charset::Ascii,
                g1_charset: Charset::Ascii,
                active_charset: ActiveCharset::G0,
            },
            alt_saved_cursor_state: SavedCursorState {
                cursor_pos: (0, 0),
                pending_wrap: false,
                fg_color: Color::Default,
                bg_color: Color::Default,
                attributes: CellAttributes::empty(),
                g0_charset: Charset::Ascii,
                g1_charset: Charset::Ascii,
                active_charset: ActiveCharset::G0,
            },
            cursor_visible: true,
            fg_color: Color::Default,
            bg_color: Color::Default,
            theme_default_fg: None,
            theme_default_bg: None,
            osc_default_fg: None,
            osc_default_bg: None,
            query_responder: true,
            attributes: CellAttributes::empty(),
            last_printed_char: ' ',
            parser: Some(Parser::new()),
            eol_mark_filter: EolMarkFilter::default(),
            auto_wrap: true,
            insert_mode: false,
            origin_mode: false,
            top_margin: 0,
            bottom_margin: rows - 1,
            tab_stops,
            app_keypad: false,
            app_cursor: false,
            mouse_mode: MouseMode::None,
            mouse_encoding: MouseEncoding::Default,
            bracketed_paste: false,
            kitty_keyboard_flags: 0,
            kitty_keyboard_stack: Vec::new(),
            inactive_kitty_keyboard: (0, Vec::new()),
            pending_query_responses: Vec::new(),
            g0_charset: Charset::Ascii,
            g1_charset: Charset::Ascii,
            active_charset: ActiveCharset::G0,
            title_stack: Vec::new(),
            title: String::new(),
            cwd: None,
            cwd_dirty: false,
            dirty: true,
            selection: None,
            sequence_callback: None,
            sequence_callback_types: HashSet::new(),
            active_dcs: None,
            apc_parser: ApcParser::default(),
            active_hyperlink_id: 0,
            next_hyperlink_id: 1,
            hyperlink_uris: HashMap::new(),
        }
    }

    pub fn set_max_scrollback_logical_lines(&mut self, max_lines: usize) {
        self.max_scrollback = max_lines.max(1);
        self.truncate_scrollback_to_logical_limit();
    }

    #[cfg(test)]
    pub(super) fn push_scrollback_row(&mut self, row: Vec<Cell>, wrapped: bool) {
        let revision = self.fresh_line_revision();
        self.push_scrollback_row_with_revision(row, wrapped, revision);
    }

    pub(super) fn push_scrollback_row_with_revision(
        &mut self,
        row: Vec<Cell>,
        wrapped: bool,
        revision: u64,
    ) {
        if !wrapped {
            self.scrollback_logical_lines += 1;
        }
        self.scrollback.push_back(row);
        self.scrollback_revisions.push_back(revision);
        self.scrollback_wrapped.push_back(wrapped);
        self.truncate_scrollback_to_logical_limit();
    }

    pub(super) fn truncate_scrollback_to_logical_limit(&mut self) {
        while self.scrollback_logical_line_count() > self.max_scrollback {
            self.pop_front_scrollback_logical_line();
        }

        // Hard physical-row cap — a stream without newlines forms one logical
        // line and bypasses the logical trim above (see
        // MAX_SCROLLBACK_PHYSICAL_ROWS_FACTOR).
        let physical_cap = self
            .max_scrollback
            .saturating_mul(super::resize::MAX_SCROLLBACK_PHYSICAL_ROWS_FACTOR);
        if self.scrollback.len() > physical_cap {
            let overflow = self.scrollback.len() - physical_cap;
            for _ in 0..overflow {
                self.scrollback.pop_front();
                self.scrollback_revisions.pop_front();
                if self.scrollback_wrapped.pop_front() == Some(false) {
                    self.scrollback_logical_lines = self.scrollback_logical_lines.saturating_sub(1);
                }
            }
            // The truncated head may begin mid-logical-line; promote it to a
            // logical start so the counter stays consistent.
            if self.scrollback_wrapped.front() == Some(&true) {
                if let Some(front) = self.scrollback_wrapped.front_mut() {
                    *front = false;
                }
                self.scrollback_logical_lines += 1;
            }
        }
    }

    /// Recompute the logical line counter from scratch (O(N)). Only needed
    /// after wholesale scrollback replacement (resize, history restore).
    pub(super) fn recount_scrollback_logical_lines(&mut self) {
        self.scrollback_logical_lines = self
            .scrollback_wrapped
            .iter()
            .filter(|wrapped| !**wrapped)
            .count();
    }

    /// O(1) logical line count: every non-wrapped row terminates one logical
    /// line; a trailing wrapped row means the last logical line is still open.
    fn scrollback_logical_line_count(&self) -> usize {
        let trailing_open = matches!(self.scrollback_wrapped.back(), Some(true)) as usize;
        self.scrollback_logical_lines + trailing_open
    }

    fn pop_front_scrollback_logical_line(&mut self) {
        while self.scrollback.pop_front().is_some() {
            self.scrollback_revisions.pop_front();
            let wrapped = self.scrollback_wrapped.pop_front().unwrap_or(false);
            if !wrapped {
                self.scrollback_logical_lines = self.scrollback_logical_lines.saturating_sub(1);
                break;
            }
        }
    }

    pub(super) fn fresh_line_revision(&mut self) -> u64 {
        let revision = self.next_line_revision.max(1);
        self.next_line_revision = revision.wrapping_add(1).max(1);
        revision
    }

    pub(super) fn fresh_line_revisions(&mut self, count: usize) -> Vec<u64> {
        (0..count).map(|_| self.fresh_line_revision()).collect()
    }

    pub(super) fn mark_active_row_changed(&mut self, row: usize) {
        let revision = self.fresh_line_revision();
        let revisions = if self.using_alt_buffer {
            &mut self.alt_grid_revisions
        } else {
            &mut self.grid_revisions
        };
        if let Some(slot) = revisions.get_mut(row) {
            *slot = revision;
        }
    }

    pub(super) fn mark_active_rows_changed(&mut self, start: usize, end: usize) {
        let len = if self.using_alt_buffer {
            self.alt_grid_revisions.len()
        } else {
            self.grid_revisions.len()
        };
        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            return;
        }
        for row in start..end {
            self.mark_active_row_changed(row);
        }
    }

    /// Refresh revisions after a wholesale content rewrite such as reflow or
    /// history restoration. This is intentionally O(lines), but those paths
    /// are rare and already rebuild every row.
    pub(super) fn refresh_all_line_revisions(&mut self) {
        self.grid_revisions = self.fresh_line_revisions(self.grid.len());
        self.alt_grid_revisions = self.fresh_line_revisions(self.alt_grid.len());
        self.scrollback_revisions =
            VecDeque::from(self.fresh_line_revisions(self.scrollback.len()));
    }

    /// Get the current terminal size
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Identity of this emulator instance. Changes when the application swaps
    /// in a newly constructed or cloned Terminal.
    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    /// Revision counter for detecting any terminal content change. Unlike
    /// per-line revisions, this also changes for misses cached outside a line.
    pub fn content_revision(&self) -> u64 {
        self.next_line_revision
    }

    /// Get the cursor position
    pub fn cursor_pos(&self) -> (usize, usize) {
        self.cursor_pos
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    pub fn app_cursor_mode(&self) -> bool {
        self.app_cursor
    }

    pub fn app_keypad_mode(&self) -> bool {
        self.app_keypad
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.bracketed_paste
    }

    /// Current mouse tracking mode (DECSET 9/1000/1002/1003).
    pub fn mouse_mode(&self) -> MouseMode {
        self.mouse_mode
    }

    /// Current mouse report encoding extension (DECSET 1005/1006).
    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.mouse_encoding
    }

    /// Application-controlled interactive modes currently in effect.
    pub fn interactive_modes(&self) -> InteractiveModes {
        InteractiveModes {
            mouse_mode: self.mouse_mode,
            mouse_encoding: self.mouse_encoding,
            bracketed_paste: self.bracketed_paste,
            app_cursor: self.app_cursor,
            app_keypad: self.app_keypad,
        }
    }

    /// Restore interactive modes captured from another emulator. Used when
    /// this emulator was rebuilt by replaying a trimmed history window that
    /// no longer contains the original mode-setting sequences — the
    /// application still believes the modes are on and will not re-send
    /// them. Apply before replaying a stream whose own mode sequences
    /// (e.g. a server snapshot) should take precedence.
    pub fn apply_interactive_modes(&mut self, modes: InteractiveModes) {
        self.mouse_mode = modes.mouse_mode;
        self.mouse_encoding = modes.mouse_encoding;
        self.bracketed_paste = modes.bracketed_paste;
        self.app_cursor = modes.app_cursor;
        self.app_keypad = modes.app_keypad;
    }

    /// Kitty keyboard protocol progressive-enhancement flags currently in
    /// effect on the active screen. Zero means legacy key reporting.
    pub fn kitty_keyboard_flags(&self) -> u16 {
        self.kitty_keyboard_flags
    }

    pub(super) fn queue_query_response(&mut self, response: &[u8]) {
        if self.query_responder {
            self.pending_query_responses.extend_from_slice(response);
        }
    }

    /// Take emulator-originated bytes that must be written back to the PTY
    /// input (device query replies such as the kitty keyboard flags report).
    pub fn take_pending_query_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_query_responses)
    }

    /// Report the host UI's actual default foreground/background colors so
    /// the emulator can answer OSC 10/11 color queries (and report defaults
    /// to apps) on the terminal's behalf. `None` leaves a color unknown:
    /// queries about it stay unanswered, exactly like a terminal that does
    /// not implement OSC 10/11 at all, so probing apps fall back to their
    /// own dark default instead of timing out on a wrong answer.
    pub fn set_theme_default_colors(
        &mut self,
        foreground: Option<Color>,
        background: Option<Color>,
    ) {
        let changed = self.theme_default_fg != foreground || self.theme_default_bg != background;
        self.theme_default_fg = foreground;
        self.theme_default_bg = background;
        if changed {
            self.dirty = true;
        }
    }

    pub fn theme_default_foreground(&self) -> Option<Color> {
        self.theme_default_fg
    }

    pub fn theme_default_background(&self) -> Option<Color> {
        self.theme_default_bg
    }

    pub fn osc_default_colors(&self) -> (Option<Color>, Option<Color>) {
        (self.osc_default_fg, self.osc_default_bg)
    }

    pub fn set_osc_default_colors(&mut self, foreground: Option<Color>, background: Option<Color>) {
        let changed = self.osc_default_fg != foreground || self.osc_default_bg != background;
        self.osc_default_fg = foreground;
        self.osc_default_bg = background;
        if changed {
            self.dirty = true;
        }
    }

    /// Effective default foreground/background color: the application's OSC
    /// 10/11 override if one is active, otherwise the host UI's color, or
    /// `None` when neither is known. Renderers should resolve cells with
    /// `Color::Default` against this before falling back to their own theme.
    pub fn default_foreground(&self) -> Option<Color> {
        self.osc_default_fg.or(self.theme_default_fg)
    }

    pub fn default_background(&self) -> Option<Color> {
        self.osc_default_bg.or(self.theme_default_bg)
    }

    /// Enable (default) or disable buffering of device query replies in
    /// `pending_query_responses`. Client-side mirrors of a server terminal
    /// stream disable this: only the PTY-owning emulator should answer
    /// probes, and a mirror would otherwise accumulate replies it can never
    /// deliver.
    pub fn set_query_responder(&mut self, enabled: bool) {
        self.query_responder = enabled;
        if !enabled {
            self.pending_query_responses.clear();
        }
    }

    /// Handle OSC 10/11 (set or query the default foreground/background
    /// color) and OSC 110/111 (reset them to the host's colors) — the
    /// xterm color protocol TUIs such as OpenCode probe at startup to pick
    /// a light or dark theme.
    pub(super) fn handle_default_color_osc(&mut self, code: &[u8], value: Option<&[u8]>) {
        let is_background = matches!(code, b"11" | b"111");
        let slot = if is_background {
            &mut self.osc_default_bg
        } else {
            &mut self.osc_default_fg
        };

        if matches!(code, b"110" | b"111") {
            if slot.take().is_some() {
                self.dirty = true;
            }
            return;
        }

        // xterm treats an omitted or empty value as a query (`OSC 11;? ST`).
        let value = value.unwrap_or(b"?");
        if value.is_empty() || value == b"?" {
            if !self.query_responder {
                return;
            }
            let effective = if is_background {
                self.default_background()
            } else {
                self.default_foreground()
            };
            let Some(Color::Rgb(r, g, b)) = effective else {
                return;
            };
            // xterm's 16-bit-per-channel reply shape (each 8-bit component
            // duplicated), e.g. `ESC ] 11 ; rgb:ffff/ffff/ffff ESC \` for
            // white. This is the format color-probing apps parse.
            self.queue_query_response(
                format!(
                    "\x1b]{};rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}\x1b\\",
                    if is_background { 11 } else { 10 },
                    r,
                    r,
                    g,
                    g,
                    b,
                    b
                )
                .as_bytes(),
            );
            return;
        }

        let parsed = std::str::from_utf8(value).ok().and_then(parse_osc_color);
        match parsed {
            Some(color) if *slot != Some(color) => {
                *slot = Some(color);
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Report the real character cell pixel metrics of the front-end, used
    /// when answering XTWINOPS pixel-size queries (`CSI 14/15/16 t`).
    pub fn set_cell_pixel_size(&mut self, width_px: u16, height_px: u16) {
        if width_px > 0 && height_px > 0 {
            self.cell_width_px = width_px;
            self.cell_height_px = height_px;
        }
    }

    pub fn terminal_type(&self) -> &'static str {
        self.terminal_type.as_str()
    }

    /// Check if terminal needs redraw
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark terminal as clean (redrawn)
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Mark terminal as dirty
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        let grid = if self.using_alt_buffer {
            &self.alt_grid
        } else {
            &self.grid
        };
        grid.get(row).and_then(|r| r.get(col))
    }

    /// Get the current visible grid
    pub fn grid(&self) -> &Vec<Vec<Cell>> {
        if self.using_alt_buffer {
            &self.alt_grid
        } else {
            &self.grid
        }
    }

    /// Return the current content revision for an absolute line. A renderer
    /// can compare this O(1) value with its cached run revision instead of
    /// scanning and hashing every cell on every paint.
    pub fn line_revision(&self, line_index: usize) -> Option<u64> {
        if self.using_alt_buffer {
            self.alt_grid_revisions.get(line_index).copied()
        } else if line_index < self.scrollback.len() {
            self.scrollback_revisions.get(line_index).copied()
        } else {
            self.grid_revisions
                .get(line_index - self.scrollback.len())
                .copied()
        }
    }

    /// True if the given row is soft-wrapped (its content continues on
    /// the next row).
    pub fn row_wrapped(&self, row: usize) -> bool {
        let wrapped = if self.using_alt_buffer {
            &self.alt_row_wrapped
        } else {
            &self.row_wrapped
        };
        wrapped.get(row).copied().unwrap_or(false)
    }

    /// True if the line at the given absolute line index is soft-wrapped
    /// (its content continues on the next line). Resolves across the
    /// scrollback and visible grid for the main buffer, or the alt buffer.
    pub fn line_wrapped(&self, line_index: usize) -> bool {
        if self.using_alt_buffer {
            self.alt_row_wrapped
                .get(line_index)
                .copied()
                .unwrap_or(false)
        } else if line_index < self.scrollback.len() {
            self.scrollback_wrapped
                .get(line_index)
                .copied()
                .unwrap_or(false)
        } else {
            self.row_wrapped
                .get(line_index - self.scrollback.len())
                .copied()
                .unwrap_or(false)
        }
    }

    /// True if the cursor is at the last column of a row and the next
    /// printable character should wrap to the following row.
    pub fn pending_wrap(&self) -> bool {
        self.pending_wrap
    }

    /// Current SGR attributes (for debugging / dump tools).
    pub fn current_attributes(&self) -> crate::CellAttributes {
        self.attributes
    }

    /// Current fg color (for debugging / dump tools).
    pub fn current_fg(&self) -> crate::Color {
        self.fg_color
    }

    /// Current bg color (for debugging / dump tools).
    pub fn current_bg(&self) -> crate::Color {
        self.bg_color
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Get the current working directory reported by the shell.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Take and clear the cwd dirty flag.
    pub fn take_cwd_dirty(&mut self) -> bool {
        let was_dirty = self.cwd_dirty;
        self.cwd_dirty = false;
        was_dirty
    }
}
