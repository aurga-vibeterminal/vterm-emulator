//! Dump the emulator's grid for a recorded PTY byte stream as TSV.
//!
//! Mirrors the output format of ghostty's `ghostty-vtdump` so the two
//! can be diffed cell-by-cell. See ghostty/src/cli/vtdump.zig for the
//! column spec.
//!
//! Usage: cargo run --release --example dump_grid -- <path-to-raw-file>

use std::env;
use std::fs;
use std::io::{self, Write};

use vterm_emulator::{Cell, CellAttributes, Color, TerminalSize};
use vterm_emulator::Terminal;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        let _ = writeln!(
            io::stderr(),
            "usage: {} <path-to-raw-file>",
            args.get(0).map(|s| s.as_str()).unwrap_or("dump_grid")
        );
        std::process::exit(1);
    }
    let path = &args[1];

    let (rows, cols) = match parse_dims_from_path(path) {
        Some(d) => d,
        None => {
            let _ = writeln!(
                io::stderr(),
                "error: could not parse <rows>x<cols> from filename: {}",
                path
            );
            std::process::exit(1);
        }
    };

    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            let _ = writeln!(io::stderr(), "error: failed to read {}: {}", path, e);
            std::process::exit(1);
        }
    };

    let mut term = Terminal::new(TerminalSize::new(rows as u16, cols as u16));
    term.process(&bytes);

    let stdout = io::stdout();
    let mut out = stdout.lock();

    let (cy, cx) = term.cursor_pos();
    let _ = writeln!(
        out,
        "# src={}\n# rows={} cols={}\n# cursor y={} x={} pending_wrap={}",
        path,
        rows,
        cols,
        cy,
        cx,
        term.pending_wrap()
    );
    let _ = writeln!(out, "y\tx\tchar\ttext\ttag\twide\tflags\tfg\tbg\tul\twrap");

    let grid = term.grid();
    let empty_row: Vec<Cell> = Vec::new();
    let empty_cell = Cell::empty();
    for y in 0..rows {
        let row = grid.get(y).unwrap_or(&empty_row);
        let wrapped = term.row_wrapped(y);
        for x in 0..cols {
            let cell = row.get(x).unwrap_or(&empty_cell);
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                y,
                x,
                fmt_char(cell),
                fmt_text(cell),
                "cp",
                fmt_wide(cell),
                fmt_flags(cell.attributes),
                fmt_color(cell.fg_color),
                fmt_color(cell.bg_color),
                ".",
                if wrapped { 1 } else { 0 },
            );
        }
    }
}

fn parse_dims_from_path(path: &str) -> Option<(usize, usize)> {
    let basename = path.rsplit('/').next()?;
    let stripped = basename.strip_suffix(".raw").unwrap_or(basename);

    // Find the last `x` flanked by digits.
    let bytes = stripped.as_bytes();
    let mut xp: Option<usize> = None;
    for i in (0..bytes.len()).rev() {
        if bytes[i] != b'x' {
            continue;
        }
        if i == 0 || i + 1 >= bytes.len() {
            continue;
        }
        if !bytes[i - 1].is_ascii_digit() || !bytes[i + 1].is_ascii_digit() {
            continue;
        }
        xp = Some(i);
        break;
    }
    let xp = xp?;

    let mut rows_start = xp;
    while rows_start > 0 && bytes[rows_start - 1].is_ascii_digit() {
        rows_start -= 1;
    }
    if rows_start == xp {
        return None;
    }

    let mut cols_end = xp + 1;
    while cols_end < bytes.len() && bytes[cols_end].is_ascii_digit() {
        cols_end += 1;
    }
    if cols_end == xp + 1 {
        return None;
    }
    if cols_end < bytes.len() && bytes[cols_end].is_ascii_digit() {
        return None;
    }

    let rows = stripped[rows_start..xp].parse::<usize>().ok()?;
    let cols = stripped[xp + 1..cols_end].parse::<usize>().ok()?;
    if rows == 0 || cols == 0 {
        return None;
    }
    Some((rows, cols))
}

fn fmt_char(cell: &Cell) -> String {
    // Use 0 only for the truly zero-initialized cell (matches ghostty's
    // isZero). vibe's Cell::empty() defaults character to ' ' (0x20) which
    // is what gets written when PTY emits spaces, so we surface that
    // faithfully.
    if cell.character == '\0' {
        return "0".to_string();
    }
    format!("0x{:x}", cell.character as u32)
}

fn fmt_text(cell: &Cell) -> String {
    if cell.character == '\0' {
        return String::new();
    }
    let mut buf = [0u8; 4];
    let s = cell.character.encode_utf8(&mut buf);
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn fmt_wide(cell: &Cell) -> &'static str {
    if cell.attributes.contains(CellAttributes::WIDE_CONTINUATION) {
        "st"
    } else {
        match Cell::display_width_for_char(cell.character) {
            0 | 1 => "n",
            _ => "w",
        }
    }
}

fn fmt_flags(attrs: CellAttributes) -> String {
    let mut s = String::with_capacity(9);
    s.push(if attrs.contains(CellAttributes::BOLD) {
        'b'
    } else {
        '-'
    });
    s.push(if attrs.contains(CellAttributes::ITALIC) {
        'i'
    } else {
        '-'
    });
    s.push('-'); // faint, not supported
    s.push(if attrs.contains(CellAttributes::BLINK) {
        'l'
    } else {
        '-'
    });
    s.push(if attrs.contains(CellAttributes::INVERSE) {
        'r'
    } else {
        '-'
    });
    s.push(if attrs.contains(CellAttributes::HIDDEN) {
        'v'
    } else {
        '-'
    });
    s.push(if attrs.contains(CellAttributes::STRIKETHROUGH) {
        's'
    } else {
        '-'
    });
    s.push('-'); // overline, not supported
    s.push(if attrs.contains(CellAttributes::UNDERLINE) {
        'u'
    } else {
        '-'
    });
    s
}

fn fmt_color(c: Color) -> String {
    match c {
        Color::Default => ".".to_string(),
        Color::Black => "p0".to_string(),
        Color::Red => "p1".to_string(),
        Color::Green => "p2".to_string(),
        Color::Yellow => "p3".to_string(),
        Color::Blue => "p4".to_string(),
        Color::Magenta => "p5".to_string(),
        Color::Cyan => "p6".to_string(),
        Color::White => "p7".to_string(),
        Color::BrightBlack => "p8".to_string(),
        Color::BrightRed => "p9".to_string(),
        Color::BrightGreen => "p10".to_string(),
        Color::BrightYellow => "p11".to_string(),
        Color::BrightBlue => "p12".to_string(),
        Color::BrightMagenta => "p13".to_string(),
        Color::BrightCyan => "p14".to_string(),
        Color::BrightWhite => "p15".to_string(),
        Color::Indexed(n) => format!("p{}", n),
        Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
    }
}
