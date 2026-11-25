#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalType {
    Ansi,
    Linux,
    Vt100,
    Vt220,
    Xterm,
}

impl TerminalType {
    pub(super) fn from_name(name: Option<&str>) -> Self {
        match name.unwrap_or("xterm").trim().to_ascii_lowercase().as_str() {
            "ansi" => Self::Ansi,
            "linux" => Self::Linux,
            "vt100" => Self::Vt100,
            "vt220" => Self::Vt220,
            "xterm" | "xterm-256color" => Self::Xterm,
            _ => Self::Xterm,
        }
    }

    pub(super) fn supports_dec_private_modes(self) -> bool {
        self != Self::Ansi
    }

    pub(super) fn supports_dec_special_graphics(self) -> bool {
        self != Self::Ansi
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ansi => "ansi",
            Self::Linux => "linux",
            Self::Vt100 => "vt100",
            Self::Vt220 => "vt220",
            Self::Xterm => "xterm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Charset {
    Ascii,
    DecSpecialGraphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActiveCharset {
    G0,
    G1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalControlSequence {
    Osc {
        params: Vec<Vec<u8>>,
        bell_terminated: bool,
    },
    Dcs {
        params: Vec<Vec<u16>>,
        intermediates: Vec<u8>,
        ignore: bool,
        final_char: char,
        data: Vec<u8>,
    },
    Apc {
        data: Vec<u8>,
    },
    Csi {
        params: Vec<Vec<u16>>,
        intermediates: Vec<u8>,
        ignore: bool,
        final_char: char,
    },
    Esc {
        intermediates: Vec<u8>,
        ignore: bool,
        byte: u8,
    },
}

pub type TerminalSequenceCallback = Box<dyn FnMut(String, String, Vec<u8>) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActiveDcs {
    pub(super) params: Vec<Vec<u16>>,
    pub(super) intermediates: Vec<u8>,
    pub(super) ignore: bool,
    pub(super) final_char: char,
    pub(super) data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SavedCursorState {
    pub(super) cursor_pos: (usize, usize),
    pub(super) pending_wrap: bool,
    pub(super) fg_color: crate::Color,
    pub(super) bg_color: crate::Color,
    pub(super) attributes: crate::CellAttributes,
    pub(super) g0_charset: Charset,
    pub(super) g1_charset: Charset,
    pub(super) active_charset: ActiveCharset,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ApcParser {
    pub(super) in_apc: bool,
    pub(super) escape_pending: bool,
    pub(super) data: Vec<u8>,
}

/// Mouse tracking mode, selected by DECSET/DECRST 9, 1000, 1002 and 1003.
/// The modes are mutually exclusive: enabling one replaces the previous one,
/// and resetting any of them disables tracking entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    /// No mouse tracking.
    #[default]
    None,
    /// X10 compatibility mode (9): report button presses only.
    X10,
    /// VT200 normal tracking (1000): report presses, releases and wheel.
    Normal,
    /// Button-event tracking (1002): normal tracking plus motion while a
    /// button is held (drag).
    ButtonEvent,
    /// Any-event tracking (1003): report all motion, even without buttons.
    AnyEvent,
}

/// Mouse report encoding extension, selected by DECSET/DECRST 1005 and 1006.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// X10-compatible `CSI M Cb Cx Cy` with each value offset by 32.
    #[default]
    Default,
    /// UTF-8 extended coordinates (1005), same layout as `Default`.
    Utf8,
    /// SGR extended format (1006): `CSI < Cb ; Cx ; Cy M/m`.
    Sgr,
}

/// Application-controlled interactive modes, captured so they can survive an
/// emulator rebuild that replays only a trimmed history window: the original
/// mode-setting sequences may have scrolled out of the window, but the
/// application still believes the modes are on and will not re-send them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InteractiveModes {
    pub mouse_mode: MouseMode,
    pub mouse_encoding: MouseEncoding,
    pub bracketed_paste: bool,
    pub app_cursor: bool,
    pub app_keypad: bool,
}
