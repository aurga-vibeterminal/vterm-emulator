use crate::{Cell, CellAttributes, Color};
use tracing::debug;

pub(super) fn log_shell_integration_marker(params: &[&[u8]]) {
    let Some(marker) = params
        .get(1)
        .and_then(|value| std::str::from_utf8(value).ok())
    else {
        return;
    };

    let namespace = std::str::from_utf8(params[0]).unwrap_or("<invalid>");
    match marker {
        "C" => {
            let cmdline = read_shell_integration_cmdline(params);
            let command_name = cmdline.as_deref().and_then(extract_command_name);
            debug!(
                "OSC {} shell integration command started: command_name={:?} cmdline={:?}",
                namespace, command_name, cmdline
            );
        }
        "D" => {
            let exit_code = params.get(2).and_then(|value| {
                std::str::from_utf8(value)
                    .ok()
                    .filter(|value| !value.is_empty())
            });
            debug!(
                "OSC {} shell integration command finished: exit_code={:?}",
                namespace, exit_code
            );
        }
        _ => {}
    }
}

fn read_shell_integration_cmdline(params: &[&[u8]]) -> Option<String> {
    for raw in params.iter().skip(2) {
        let raw = std::str::from_utf8(raw).ok()?;
        if let Some(value) = raw.strip_prefix("cmdline_url=") {
            return percent_decode(value);
        }
        if let Some(value) = raw.strip_prefix("cmdline=") {
            return Some(value.to_string());
        }
    }
    None
}

fn extract_command_name(cmdline: &str) -> Option<&str> {
    let trimmed = cmdline.trim();
    if trimmed.is_empty() {
        return None;
    }

    let trimmed = trimmed
        .strip_prefix("& ")
        .or_else(|| trimmed.strip_prefix(". "))
        .unwrap_or(trimmed)
        .trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix('"') {
        return rest.split('"').next().filter(|value| !value.is_empty());
    }
    if let Some(rest) = trimmed.strip_prefix('\'') {
        return rest.split('\'').next().filter(|value| !value.is_empty());
    }

    trimmed.split_whitespace().next()
}

/// Parse an OSC 7 file URI into a local filesystem path.
pub(super) fn parse_osc7_working_directory(uri: &str) -> Option<String> {
    let without_scheme = uri.strip_prefix("file://")?;
    let path_start = without_scheme.find('/').unwrap_or(without_scheme.len());
    let raw_path = &without_scheme[path_start..];
    if raw_path.is_empty() {
        return None;
    }

    percent_decode(raw_path)
}

/// Parse an OSC 10/11 default-color specification: xterm's `rgb:rr/gg/bb`
/// form (1–4 hexit per component, scaled by leading-bit replication like
/// XParseColor) or a `#rgb`/`#rrggbb`/`#rrggbbaa` web color. Returns an RGB
/// cell color; palette indices have no meaning for default colors.
pub(super) fn parse_osc_color(value: &str) -> Option<Color> {
    if let Some(rgb) = value
        .strip_prefix("rgb:")
        .or_else(|| value.strip_prefix("RGB:"))
    {
        let mut components = rgb.split('/');
        let r = parse_hexit_color_component(components.next()?)?;
        let g = parse_hexit_color_component(components.next()?)?;
        let b = parse_hexit_color_component(components.next()?)?;
        return (components.next().is_none()).then_some(Color::Rgb(r, g, b));
    }
    let hex = value.strip_prefix('#')?;
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let hexit = |chars: &str| u8::from_str_radix(chars, 16).ok();
    match hex.len() {
        3 => Some(Color::Rgb(
            u8::from_str_radix(&hex[0..1], 16).ok()? * 17,
            u8::from_str_radix(&hex[1..2], 16).ok()? * 17,
            u8::from_str_radix(&hex[2..3], 16).ok()? * 17,
        )),
        6 => Some(Color::Rgb(
            hexit(&hex[0..2])?,
            hexit(&hex[2..4])?,
            hexit(&hex[4..6])?,
        )),
        8 => Some(Color::Rgb(
            hexit(&hex[0..2])?,
            hexit(&hex[2..4])?,
            hexit(&hex[4..6])?,
        )),
        _ => None,
    }
}

/// Scale one `rgb:` component (1–4 hexits) down to 8 bits the way X11 does:
/// the hexits are the most significant bits of a 16-bit component.
fn parse_hexit_color_component(component: &str) -> Option<u8> {
    if component.is_empty()
        || component.len() > 4
        || !component.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    Some(match component.len() {
        1 => (value * 17) as u8,
        2 => value as u8,
        3 => (value >> 4) as u8,
        _ => (value >> 8) as u8,
    })
}

/// Decode percent-escaped bytes used inside OSC 7 file URIs.
pub(super) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).ok()
}

pub(super) fn normalize_selection(
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) -> (usize, usize, usize, usize) {
    if (start_row, start_col) <= (end_row, end_col) {
        (start_row, start_col, end_row, end_col)
    } else {
        (end_row, end_col, start_row, start_col)
    }
}

// Helper functions

pub(super) fn append_line_text(text: &mut String, line: &[Cell]) {
    text.extend(
        line.iter()
            .filter(|cell| !cell.is_wide_continuation())
            .map(|cell| cell.character),
    );
}

pub(super) fn clear_wide_cell_at(line: &mut [Cell], col: usize) {
    clear_wide_cell_at_with(line, col, Cell::empty());
}

pub(super) fn clear_wide_cell_at_with(line: &mut [Cell], col: usize, blank: Cell) {
    let Some(cell) = line.get(col).copied() else {
        return;
    };

    if cell.is_wide_continuation() {
        if col > 0 {
            line[col - 1] = blank;
        }
        line[col] = blank;
        return;
    }

    let width = cell.display_width();
    line[col] = blank;
    for offset in 1..width {
        if col + offset < line.len() && line[col + offset].is_wide_continuation() {
            line[col + offset] = blank;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_command_name, percent_decode, read_shell_integration_cmdline};

    #[test]
    fn shell_integration_cmdline_prefers_decoded_cmdline_url() {
        let params: Vec<&[u8]> = vec![
            b"133".as_slice(),
            b"C".as_slice(),
            b"aid=42".as_slice(),
            b"cmdline_url=claude%20--dangerously-skip-permissions".as_slice(),
        ];
        assert_eq!(
            read_shell_integration_cmdline(&params),
            Some("claude --dangerously-skip-permissions".to_string())
        );
    }

    #[test]
    fn extract_command_name_handles_plain_and_quoted_commands() {
        assert_eq!(extract_command_name("claude --help"), Some("claude"));
        assert_eq!(
            extract_command_name(r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo"#),
            Some(r#"C:\Program Files\PowerShell\7\pwsh.exe"#)
        );
        assert_eq!(
            extract_command_name("& 'C:\\tool\\run.ps1' foo"),
            Some("C:\\tool\\run.ps1")
        );
    }

    #[test]
    fn percent_decode_handles_spaces() {
        assert_eq!(
            percent_decode("hello%20world"),
            Some("hello world".to_string())
        );
    }
}

pub(super) fn wide_continuation_cell(cell: Cell) -> Cell {
    let mut continuation = cell;
    continuation.character = ' ';
    continuation
        .attributes
        .insert(CellAttributes::WIDE_CONTINUATION);
    continuation
}

pub(super) fn normalize_line_wide_cells(line: &mut [Cell]) {
    for col in 0..line.len() {
        if line[col].is_wide_continuation() {
            if col == 0 || line[col - 1].display_width() < 2 {
                line[col] = Cell::empty();
            }
            continue;
        }

        let width = line[col].display_width();
        if width < 2 {
            continue;
        }

        let mut valid = true;
        for offset in 1..width {
            if col + offset >= line.len() || !line[col + offset].is_wide_continuation() {
                valid = false;
                break;
            }
        }

        if !valid {
            clear_wide_cell_at(line, col);
        }
    }
}

pub(super) fn clear_cell_range_with(line: &mut [Cell], start: usize, end: usize, blank: Cell) {
    if start >= end || start >= line.len() {
        return;
    }

    for col in start..end.min(line.len()) {
        clear_wide_cell_at_with(line, col, blank);
    }
    normalize_line_wide_cells(line);
}

pub(super) fn insert_blank_cells_with(line: &mut [Cell], col: usize, count: usize, blank: Cell) {
    if count == 0 || col >= line.len() {
        return;
    }

    for _ in 0..count.min(line.len() - col) {
        for index in (col..line.len() - 1).rev() {
            line[index + 1] = line[index];
        }
        line[col] = blank;
    }
    normalize_line_wide_cells(line);
}

pub(super) fn delete_cells_with(line: &mut [Cell], col: usize, count: usize, blank: Cell) {
    if count == 0 || col >= line.len() {
        return;
    }

    for _ in 0..count.min(line.len() - col) {
        for index in col..line.len() - 1 {
            line[index] = line[index + 1];
        }
        line[line.len() - 1] = blank;
    }
    normalize_line_wide_cells(line);
}

pub(super) fn write_cell(line: &mut [Cell], col: usize, cell: Cell) {
    if col >= line.len() {
        return;
    }

    let width = cell.display_width().min(line.len() - col).max(1);
    // Clear the overwritten cells with wide-cell awareness, which also repairs
    // any wide pair whose other half sits just outside the overwritten range.
    // This replaces the previous clear + full-line normalize pass (O(cols) per
    // written character) with O(1) local fixups.
    clear_wide_cell_at(line, col);
    let last = col + width - 1;
    if last > col {
        clear_wide_cell_at(line, last);
    }
    line[col] = cell;
    for offset in 1..width {
        line[col + offset] = wide_continuation_cell(cell);
    }
    // A wide cell squeezed into a single column at the end of the line has no
    // room for its continuation; the old normalize pass cleared such orphans.
    if width == 1 && line[col].display_width() > 1 {
        line[col] = Cell::empty();
    }
}

pub(super) fn create_grid(rows: usize, cols: usize) -> Vec<Vec<Cell>> {
    (0..rows).map(|_| vec![Cell::empty(); cols]).collect()
}

pub(super) fn occupied_len(row: &[Cell]) -> usize {
    row.iter()
        .rposition(|cell| !is_empty_cell(cell) || cell.is_wide_continuation())
        .map(|index| index + 1)
        .unwrap_or(0)
}

pub(super) fn is_empty_cell(cell: &Cell) -> bool {
    cell.character == ' '
        && cell.fg_color == Color::Default
        && cell.bg_color == Color::Default
        && cell.attributes.is_empty()
}

pub(super) fn ansi_color_to_color(code: u16, bright: bool) -> Color {
    match code {
        0 => {
            if bright {
                Color::BrightBlack
            } else {
                Color::Black
            }
        }
        1 => {
            if bright {
                Color::BrightRed
            } else {
                Color::Red
            }
        }
        2 => {
            if bright {
                Color::BrightGreen
            } else {
                Color::Green
            }
        }
        3 => {
            if bright {
                Color::BrightYellow
            } else {
                Color::Yellow
            }
        }
        4 => {
            if bright {
                Color::BrightBlue
            } else {
                Color::Blue
            }
        }
        5 => {
            if bright {
                Color::BrightMagenta
            } else {
                Color::Magenta
            }
        }
        6 => {
            if bright {
                Color::BrightCyan
            } else {
                Color::Cyan
            }
        }
        7 => {
            if bright {
                Color::BrightWhite
            } else {
                Color::White
            }
        }
        _ => Color::Default,
    }
}

pub(super) fn dec_special_graphics(c: char) -> char {
    match c {
        '_' => ' ',
        '`' => '◆',
        'a' => '▒',
        'b' => '\u{2409}',
        'c' => '\u{240c}',
        'd' => '\u{240d}',
        'e' => '\u{240a}',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        _ => c,
    }
}
