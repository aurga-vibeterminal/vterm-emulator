use std::fs;
use std::path::PathBuf;

use vterm_emulator::{Cell, TerminalSize};
use vterm_emulator::Terminal;

const DEFAULT_RAW_HISTORY_FILE: &str = "debug_692fbddb_client_buffer_42x125.raw";
const DEFAULT_ROWS: u16 = 42;
const DEFAULT_COLS: u16 = 125;

#[test]
#[ignore = "requires a local raw PTY history fixture; set VTERM_RAW_HISTORY_PATH or use the default debug raw path"]
fn raw_history_replay_does_not_create_adjacent_duplicate_listing_blocks() {
    let raw = read_raw_history_fixture();
    let size = raw_history_size();
    let terminal = replay(&raw, size);
    let lines = terminal_text_lines(&terminal);

    assert_no_adjacent_duplicate_blocks(&lines);
    assert_no_orphaned_powershell_listing_fragments(&lines);
}

#[test]
#[ignore = "requires a local raw PTY history fixture; set VTERM_RAW_HISTORY_PATH or use the default debug raw path"]
fn raw_history_replay_survives_deterministic_resize_and_scroll_probes() {
    let raw = read_raw_history_fixture();
    let mut rng = Lcg::new(0x692f_bddb_42_125);

    for _ in 0..64 {
        let rows = rng.range(18, 52) as u16;
        let cols = rng.range(64, 132) as u16;
        let terminal = replay(&raw, TerminalSize::new(rows, cols));
        let all_lines = terminal_text_lines(&terminal);
        assert_no_adjacent_duplicate_blocks(&all_lines);
        assert_no_orphaned_powershell_listing_fragments(&all_lines);

        let total = terminal.total_line_count();
        if total == 0 {
            continue;
        }

        for _ in 0..8 {
            let before = rng.range(1, total + 1);
            let max_lines = rng.range(1, rows as usize + 1);
            let (_, page) = terminal.history_page(before, max_lines);
            let page_lines = page.iter().map(|line| line_text(line)).collect::<Vec<_>>();
            assert_no_adjacent_duplicate_blocks(&page_lines);
            assert_no_orphaned_powershell_listing_fragments(&page_lines);
        }
    }
}

#[test]
#[ignore = "diagnostic comparison for resize-vs-full-replay behavior on the local raw PTY history fixture"]
fn resizing_existing_terminal_matches_full_raw_replay_for_visible_text() {
    let raw = read_raw_history_fixture();
    let mut resized = replay(&raw, raw_history_size());

    for size in [
        TerminalSize::new(42, 125),
        TerminalSize::new(32, 90),
        TerminalSize::new(24, 80),
        TerminalSize::new(48, 120),
        TerminalSize::new(30, 70),
    ] {
        resized.resize(size);
        let rebuilt = replay(&raw, size);

        assert_eq!(
            visible_text(&resized),
            visible_text(&rebuilt),
            "existing resize diverged from full raw replay at size {}x{}",
            size.rows,
            size.cols
        );
    }
}

fn read_raw_history_fixture() -> Vec<u8> {
    let path = std::env::var_os("VTERM_RAW_HISTORY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            // ~/.<app>/debug_<...>.raw, resolved from the user's home directory.
            let home = std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(PathBuf::from)
                .unwrap_or_default();
            home.join(".vibeterminal").join(DEFAULT_RAW_HISTORY_FILE)
        });
    fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read raw history fixture at {}: {error}",
            path.display()
        )
    })
}

fn raw_history_size() -> TerminalSize {
    let rows = std::env::var("VTERM_RAW_HISTORY_ROWS")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_ROWS);
    let cols = std::env::var("VTERM_RAW_HISTORY_COLS")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(DEFAULT_COLS);
    TerminalSize::new(rows, cols)
}

fn replay(raw: &[u8], size: TerminalSize) -> Terminal {
    let mut terminal = Terminal::new_with_type(size, Some("xterm-256color"));
    terminal.process(raw);
    terminal
}

fn terminal_text_lines(terminal: &Terminal) -> Vec<String> {
    (0..terminal.total_line_count())
        .filter_map(|index| terminal.line_at(index))
        .map(|line| line_text(line))
        .collect()
}

fn visible_text(terminal: &Terminal) -> String {
    terminal
        .grid()
        .iter()
        .map(|line| line_text(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_text(line: &[Cell]) -> String {
    let mut text = String::new();
    for cell in line {
        if !cell.is_wide_continuation() {
            text.push(cell.character);
        }
    }
    text.trim_end().to_string()
}

fn assert_no_adjacent_duplicate_blocks(lines: &[String]) {
    let normalized = lines
        .iter()
        .map(|line| normalize_line(line))
        .collect::<Vec<_>>();

    for start in 0..normalized.len() {
        for block_len in 3..=10 {
            let second = start + block_len;
            let end = second + block_len;
            if end > normalized.len() {
                continue;
            }

            let first_block = &normalized[start..second];
            if first_block.iter().filter(|line| !line.is_empty()).count() < 3 {
                continue;
            }
            if !first_block
                .iter()
                .any(|line| looks_like_powershell_listing_record(line))
            {
                continue;
            }

            if first_block == &normalized[second..end] {
                panic!(
                    "adjacent duplicate terminal history block at lines {}..{}:\n{}",
                    start,
                    end,
                    lines[start..end].join("\n")
                );
            }
        }
    }
}

fn assert_no_orphaned_powershell_listing_fragments(lines: &[String]) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if line.starts_with(' ') && looks_like_orphaned_listing_fragment(trimmed) {
            panic!("orphaned PowerShell listing fragment at line {index}: {line}");
        }
    }
}

fn looks_like_orphaned_listing_fragment(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }

    let starts_with_size_then_dotfile = trimmed
        .split_once(' ')
        .and_then(|(head, tail)| head.parse::<u64>().ok().map(|_| tail.trim_start()))
        .is_some_and(|tail| tail.starts_with('.'));

    starts_with_size_then_dotfile
        || trimmed == "hon_history"
        || trimmed == "python_history"
        || trimmed == ".npmrc"
}

fn normalize_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_powershell_listing_record(line: &str) -> bool {
    line.starts_with("d---- ")
        || line.starts_with("d-r-- ")
        || line.starts_with("dar-- ")
        || line.starts_with("-a--- ")
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn range(&mut self, start: usize, end: usize) -> usize {
        assert!(start < end);
        start + (self.next() as usize % (end - start))
    }
}
