use super::*;
use crate::{Cell, CellAttributes, TerminalSize};
use std::sync::{Arc, Mutex};

fn assert_no_broken_wide_cells(line: &[Cell]) {
    for (index, cell) in line.iter().enumerate() {
        if cell.is_wide_continuation() {
            assert!(index > 0);
            assert!(line[index - 1].display_width() >= 2);
            continue;
        }

        let width = cell.display_width();
        if width < 2 {
            continue;
        }

        for offset in 1..width {
            assert!(index + offset < line.len());
            assert!(line[index + offset].is_wide_continuation());
        }
    }
}

fn assert_line_revision_invariants(term: &Terminal) {
    assert_ne!(term.instance_id(), 0);
    assert_eq!(term.grid_revisions.len(), term.grid.len());
    assert_eq!(term.alt_grid_revisions.len(), term.alt_grid.len());
    assert_eq!(term.scrollback_revisions.len(), term.scrollback.len());

    let mut revisions = std::collections::HashSet::new();
    for revision in term
        .grid_revisions
        .iter()
        .chain(&term.alt_grid_revisions)
        .chain(&term.scrollback_revisions)
    {
        assert_ne!(*revision, 0);
        assert!(revisions.insert(*revision), "duplicate revision {revision}");
    }

    let active_revisions: Vec<_> = (0..term.total_line_count())
        .map(|line| term.line_revision(line).expect("line revision"))
        .collect();
    if term.using_alt_buffer {
        assert_eq!(active_revisions, term.alt_grid_revisions);
    } else {
        let expected: Vec<_> = term
            .scrollback_revisions
            .iter()
            .chain(&term.grid_revisions)
            .copied()
            .collect();
        assert_eq!(active_revisions, expected);
    }
}

#[test]
fn test_terminal_creation() {
    let term = Terminal::new(TerminalSize::new(24, 80));
    assert_eq!(term.size.rows, 24);
    assert_eq!(term.size.cols, 80);
    assert_eq!(term.cursor_pos(), (0, 0));
}

#[test]
fn test_simple_text() {
    let mut term = Terminal::new(TerminalSize::new(24, 80));
    term.process(b"Hello, World!");

    let grid = term.grid();
    assert_eq!(grid[0][0].character, 'H');
    assert_eq!(grid[0][1].character, 'e');
    assert_eq!(grid[0][12].character, '!');
}

#[test]
fn test_gbk_stream_decodes_across_chunks() {
    let mut term =
        Terminal::new_with_type_and_encoding(TerminalSize::new(24, 80), None, Some("gbk"));
    term.process(&[0xC4]);
    term.process(&[0xE3, 0xBA, 0xC3]);

    assert_eq!(term.grid()[0][0].character, '你');
    assert_eq!(term.grid()[0][2].character, '好');
}

#[test]
fn test_custom_single_byte_stream_encoding() {
    let mut term =
        Terminal::new_with_type_and_encoding(TerminalSize::new(24, 80), None, Some("cp437"));
    term.process(&[0x48, 0x82]);

    assert_eq!(term.grid()[0][0].character, 'H');
    assert_eq!(term.grid()[0][1].character, 'é');
}

#[test]
fn test_cursor_movement() {
    let mut term = Terminal::new(TerminalSize::new(24, 80));

    // Test carriage return and linefeed
    term.process(b"Hello\r\nWorld");

    assert_eq!(term.grid()[0][0].character, 'H');
    assert_eq!(term.grid()[1][0].character, 'W');
}

#[test]
fn test_clear_screen() {
    let mut term = Terminal::new(TerminalSize::new(24, 80));
    term.process(b"Hello");
    term.process(b"\x1b[2J"); // Clear screen

    assert_eq!(term.grid()[0][0].character, ' ');
}

#[test]
fn test_current_line_text_unwraps_soft_wrapped_rows() {
    let mut term = Terminal::new(TerminalSize::new(4, 8));

    term.process(b"C:/Projects/vibe-terminal");

    assert_eq!(
        term.current_line_text().trim_end(),
        "C:/Projects/vibe-terminal"
    );
}

#[test]
fn test_current_line_text_unwraps_from_scrollback() {
    let mut term = Terminal::new(TerminalSize::new(2, 8));

    term.process(b"C:/Projects/vibe-terminal");

    assert_eq!(term.scrollback().len(), 2);
    assert_eq!(
        term.current_line_text().trim_end(),
        "C:/Projects/vibe-terminal"
    );
}

#[test]
fn test_current_line_text_does_not_join_full_hard_line() {
    let mut term = Terminal::new(TerminalSize::new(3, 4));

    // Filling the previous row does not make it a soft wrap when an explicit
    // cursor move starts the next row.
    term.process(b"ABCD\x1b[2;1Hx");

    assert!(!term.line_wrapped(0));
    assert_eq!(term.current_line_text().trim_end(), "x");
}

#[test]
fn test_resize() {
    let mut term = Terminal::new(TerminalSize::new(24, 80));
    term.process(b"Test");
    term.resize(TerminalSize::new(30, 100));

    assert_eq!(term.size().rows, 30);
    assert_eq!(term.size().cols, 100);
    assert_eq!(term.grid()[0][0].character, 'T');
}

#[test]
fn test_resize_smaller_keeps_bottom_rows_visible() {
    let size = TerminalSize::new(6, 8);
    let mut term = Terminal::new(size);
    let lines = ['A', 'B', 'C', 'D', 'E', 'F']
        .into_iter()
        .map(|character| vec![Cell::new(character); 8])
        .collect();
    term.load_history_state(size, lines, (5, 2));

    term.resize(TerminalSize::new(4, 8));

    assert_eq!(term.scrollback().len(), 2);
    assert_eq!(term.scrollback()[0][0].character, 'A');
    assert_eq!(term.scrollback()[1][0].character, 'B');
    assert_eq!(term.grid()[0][0].character, 'C');
    assert_eq!(term.grid()[1][0].character, 'D');
    assert_eq!(term.grid()[2][0].character, 'E');
    assert_eq!(term.grid()[3][0].character, 'F');
    assert_eq!(term.cursor_pos(), (3, 2));
}

#[test]
fn test_resize_narrower_reflows_scrollback_without_truncating() {
    let mut term = Terminal::new(TerminalSize::new(3, 8));
    term.process(b"abcdefghijk\n");
    term.process(b"second\n");
    term.process(b"third\n");

    assert!(term.scrollback().len() > 0);
    term.resize(TerminalSize::new(3, 4));

    let text: String = (0..term.total_line_count())
        .filter_map(|line_index| term.line_at(line_index))
        .map(|line| {
            let mut text = String::new();
            append_line_text(&mut text, line);
            text
        })
        .collect::<Vec<_>>()
        .join("");

    assert!(text.contains("abcdefghijk"));
    assert!(!text.contains("abcdsecond"));
}

#[test]
fn test_resize_larger_restores_scrollback_above_viewport() {
    let size = TerminalSize::new(6, 8);
    let mut term = Terminal::new(size);
    let lines = ['A', 'B', 'C', 'D', 'E', 'F']
        .into_iter()
        .map(|character| vec![Cell::new(character); 8])
        .collect();
    term.load_history_state(size, lines, (5, 2));
    term.resize(TerminalSize::new(4, 8));

    term.resize(TerminalSize::new(6, 8));

    assert_eq!(term.scrollback().len(), 0);
    assert_eq!(term.grid()[0][0].character, 'A');
    assert_eq!(term.grid()[1][0].character, 'B');
    assert_eq!(term.grid()[2][0].character, 'C');
    assert_eq!(term.grid()[3][0].character, 'D');
    assert_eq!(term.grid()[4][0].character, 'E');
    assert_eq!(term.grid()[5][0].character, 'F');
    assert_eq!(term.cursor_pos(), (5, 2));
}

#[test]
fn test_resize_larger_without_scrollback_keeps_content_at_top() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.process(b"hi");

    term.resize(TerminalSize::new(4, 4));

    assert_eq!(term.grid()[0][0].character, 'h');
    assert_eq!(term.grid()[0][1].character, 'i');
    assert_eq!(term.grid()[1][0].character, ' ');
    assert_eq!(term.grid()[3][0].character, ' ');
    assert_eq!(term.cursor_pos(), (0, 2));
}

#[test]
fn test_selection_normalizes_bounds() {
    let mut term = Terminal::new(TerminalSize::new(24, 80));
    term.process(b"hello");
    term.set_selection(0, 4, 0, 1);

    assert_eq!(term.selection(), Some((0, 1, 0, 4)));
    assert_eq!(term.selected_text().as_deref(), Some("ello"));
    assert!(term.is_selected(0, 2));
    assert!(!term.is_selected(0, 0));
}

#[test]
fn test_selected_text_joins_soft_wrapped_rows() {
    let mut term = Terminal::new(TerminalSize::new(2, 5));
    // 'hello' fills row 0; '!' triggers a soft-wrap onto row 1.
    term.process(b"hello!");
    assert!(term.row_wrapped(0));

    term.set_selection(0, 0, 1, 0);
    assert_eq!(term.selected_text().as_deref(), Some("hello!"));
}

#[test]
fn test_selected_text_breaks_on_real_newline() {
    let mut term = Terminal::new(TerminalSize::new(2, 5));
    term.process(b"hello\r\nworld");
    assert!(!term.row_wrapped(0));

    term.set_selection(0, 0, 1, 4);
    assert_eq!(term.selected_text().as_deref(), Some("hello\nworld"));
}

#[test]
fn test_selected_text_absolute_joins_wrapped_rows_across_scrollback() {
    let mut term = Terminal::new(TerminalSize::new(2, 5));
    // "hello!xyz12" soft-wraps three times across scrollback + visible grid,
    // forming one logical line.
    term.process(b"hello!xyz12");

    // scrollback "hello" wraps, grid[0] "!xyz1" wraps, grid[1] "2" does not.
    assert!(term.line_wrapped(0));
    assert!(term.line_wrapped(1));
    assert!(!term.line_wrapped(2));

    term.set_selection_absolute(0, 0, 2, 0);
    assert_eq!(
        term.selected_text_absolute().as_deref(),
        Some("hello!xyz12")
    );
}

#[test]
fn test_selected_text_trims_trailing_whitespace_per_line() {
    let mut term = Terminal::new(TerminalSize::new(24, 10));
    // "hi" only fills 2 of 10 columns; the rest are padding spaces.
    term.process(b"hi");
    term.set_selection(0, 0, 0, 9);
    assert_eq!(term.selected_text().as_deref(), Some("hi"));
}

#[test]
fn test_selected_text_trims_trailing_whitespace_on_last_row_of_wrap() {
    let mut term = Terminal::new(TerminalSize::new(2, 5));
    // "hello!xy" → row 0 "hello" wraps, row 1 "!xy" + 2 padding cells.
    term.process(b"hello!xy");
    assert!(term.row_wrapped(0));

    term.set_selection(0, 0, 1, 4);
    // Mid-wrap content is preserved; only the final tail is trimmed.
    assert_eq!(term.selected_text().as_deref(), Some("hello!xy"));
}

#[test]
fn test_selected_text_trims_each_real_line_independently() {
    let mut term = Terminal::new(TerminalSize::new(24, 10));
    term.process(b"hi\r\nbye");
    term.set_selection(0, 0, 1, 9);
    // Both lines have their trailing padding trimmed; the \n stays.
    assert_eq!(term.selected_text().as_deref(), Some("hi\nbye"));
}

#[test]
fn test_wide_character_occupies_two_cells() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.process("你a".as_bytes());

    assert_eq!(term.grid()[0][0].character, '你');
    assert_eq!(term.grid()[0][1].character, ' ');
    assert!(term.grid()[0][1].is_wide_continuation());
    assert_eq!(term.grid()[0][2].character, 'a');
    assert_eq!(term.cursor_pos(), (0, 3));
}

#[test]
fn test_wide_character_wraps_before_last_column() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.process(b"abc");
    term.process("你".as_bytes());

    assert_eq!(term.grid()[0][0].character, 'a');
    assert_eq!(term.grid()[0][1].character, 'b');
    assert_eq!(term.grid()[0][2].character, 'c');
    assert_eq!(term.grid()[1][0].character, '你');
    assert!(term.grid()[1][1].is_wide_continuation());
    assert_eq!(term.cursor_pos(), (1, 2));
}

#[test]
fn test_selected_text_skips_wide_character_continuation_cells() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.process("你a".as_bytes());
    term.set_selection(0, 0, 0, 2);

    assert_eq!(term.selected_text().as_deref(), Some("你a"));
}

#[test]
fn test_insert_character_normalizes_wide_character_boundaries() {
    let mut term = Terminal::new(TerminalSize::new(1, 5));
    term.process("你a".as_bytes());
    term.process(b"\r\x1b[2G\x1b[@");

    let line = &term.grid()[0];
    assert_no_broken_wide_cells(line);
    assert!(line.iter().any(|cell| cell.character == 'a'));
}

#[test]
fn test_delete_character_normalizes_wide_character_boundaries() {
    let mut term = Terminal::new(TerminalSize::new(1, 5));
    term.process("你a".as_bytes());
    term.process(b"\r\x1b[P");

    let line = &term.grid()[0];
    assert_no_broken_wide_cells(line);
    assert!(line
        .iter()
        .any(|cell| cell.character == 'a' || cell.character == '你'));
}

#[test]
fn test_erase_character_clears_wide_character_without_orphans() {
    let mut term = Terminal::new(TerminalSize::new(1, 5));
    term.process("你a".as_bytes());
    term.process(b"\r\x1b[X");

    let line = &term.grid()[0];
    assert_no_broken_wide_cells(line);
    assert!(line
        .iter()
        .all(|cell| !cell.is_wide_continuation() || cell.character == ' '));
}

#[test]
fn test_private_mode_cursor_visibility() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));

    term.process(b"\x1b[?25l");
    assert!(!term.cursor_visible());

    term.process(b"\x1b[?25h");
    assert!(term.cursor_visible());
}

#[test]
fn test_private_mode_alt_screen_restore() {
    let mut term = Terminal::new_with_type(TerminalSize::new(2, 4), Some("xterm-256color"));
    term.process(b"main");
    assert_eq!(term.grid()[0][0].character, 'm');

    term.process(b"\x1b[?1049h");
    term.process(b"alt");
    assert!(term.using_alt_buffer);
    assert_eq!(term.grid()[1][0].character, 'a');

    term.process(b"\x1b[?1049l");
    assert!(!term.using_alt_buffer);
    assert_eq!(term.grid()[0][0].character, 'm');
}

#[test]
fn test_private_mode_mouse_tracking() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    assert_eq!(term.mouse_mode(), MouseMode::None);
    assert_eq!(term.mouse_encoding(), MouseEncoding::Default);

    term.process(b"\x1b[?1000h");
    assert_eq!(term.mouse_mode(), MouseMode::Normal);

    // Tracking modes are mutually exclusive: the last one wins.
    term.process(b"\x1b[?1003h");
    assert_eq!(term.mouse_mode(), MouseMode::AnyEvent);

    term.process(b"\x1b[?1002h");
    assert_eq!(term.mouse_mode(), MouseMode::ButtonEvent);

    term.process(b"\x1b[?9h");
    assert_eq!(term.mouse_mode(), MouseMode::X10);

    // Resetting any tracking mode disables tracking entirely.
    term.process(b"\x1b[?1000l");
    assert_eq!(term.mouse_mode(), MouseMode::None);

    term.process(b"\x1b[?1006h");
    assert_eq!(term.mouse_encoding(), MouseEncoding::Sgr);
    term.process(b"\x1b[?1006l");
    assert_eq!(term.mouse_encoding(), MouseEncoding::Default);

    term.process(b"\x1b[?1005h");
    assert_eq!(term.mouse_encoding(), MouseEncoding::Utf8);
    term.process(b"\x1b[?1005l");
    assert_eq!(term.mouse_encoding(), MouseEncoding::Default);
}

#[test]
fn test_osc7_updates_working_directory() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));

    term.process(b"\x1b]7;file:///C:/Projects/vibe-terminal\x07");

    assert_eq!(term.cwd(), Some("/C:/Projects/vibe-terminal"));
    assert!(term.take_cwd_dirty());
    assert!(!term.take_cwd_dirty());
}

#[test]
fn test_sequence_callback_receives_osc_csi_and_esc() {
    let events: Arc<Mutex<Vec<(String, String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    let types = ["osc".to_string(), "csi".to_string(), "esc".to_string()].into();
    term.set_sequence_callback(types, move |seq_type, code, data| {
        captured.lock().unwrap().push((seq_type, code, data));
    });

    term.process(b"\x1b]0;hello\x07\x1b[?25l\x1b(0");

    let events = events.lock().unwrap();
    assert_eq!(
        events[0],
        ("osc".to_string(), "0".to_string(), b"hello".to_vec())
    );
    assert_eq!(events[1], ("csi".to_string(), "l".to_string(), vec![25, 0]));
    assert_eq!(events[2], ("esc".to_string(), "30".to_string(), vec![]));
    assert_eq!(events.len(), 3);
}

#[test]
fn test_sequence_callback_preserves_multi_part_osc_payload() {
    let events: Arc<Mutex<Vec<(String, String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    let types = ["osc".to_string()].into();
    term.set_sequence_callback(types, move |seq_type, code, data| {
        captured.lock().unwrap().push((seq_type, code, data));
    });

    term.process(b"\x1b]133;C;aid=42;cmdline_url=kimi%20--help\x07");

    let events = events.lock().unwrap();
    assert_eq!(
        events[0],
        (
            "osc".to_string(),
            "133".to_string(),
            b"C;aid=42;cmdline_url=kimi%20--help".to_vec()
        )
    );
}

#[test]
fn test_sequence_callback_survives_terminal_reset() {
    let events: Arc<Mutex<Vec<(String, String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    let types = ["osc".to_string(), "esc".to_string()].into();
    term.set_sequence_callback(types, move |seq_type, code, data| {
        captured.lock().unwrap().push((seq_type, code, data));
    });

    term.process(b"\x1bc\x1b]0;after-reset\x07");

    let events = events.lock().unwrap();
    assert_eq!(events[0], ("esc".to_string(), "63".to_string(), vec![]));
    assert_eq!(
        events[1],
        ("osc".to_string(), "0".to_string(), b"after-reset".to_vec())
    );
}

#[test]
fn test_sequence_callback_receives_apc_across_chunks() {
    let events: Arc<Mutex<Vec<(String, String, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    let types = ["apc".to_string()].into();
    term.set_sequence_callback(types, move |seq_type, code, data| {
        captured.lock().unwrap().push((seq_type, code, data));
    });

    term.process(b"\x1b_partial");
    assert!(events.lock().unwrap().is_empty());

    term.process(b"-payload\x1b\\");

    let events = events.lock().unwrap();
    assert_eq!(
        events[0],
        (
            "apc".to_string(),
            String::new(),
            b"partial-payload".to_vec()
        )
    );
}

#[test]
fn test_dec_special_graphics_translation() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));

    term.process(b"\x1b(0");
    term.process(&[0x0F]);
    term.process(b"lqk");

    assert_eq!(term.grid()[0][0].character, '┌');
    assert_eq!(term.grid()[0][1].character, '─');
    assert_eq!(term.grid()[0][2].character, '┐');
}

#[test]
fn test_csi_lt_u_does_not_restore_cursor() {
    let mut term = Terminal::new(TerminalSize::new(24, 80));
    term.process(b"\x1b[10;20H");
    assert_eq!(term.cursor_pos(), (9, 19));
    term.process(b"\x1b[<u");
    assert_eq!(
        term.cursor_pos(),
        (9, 19),
        "CSI <u incorrectly triggered SCORC"
    );
}

#[test]
fn test_scrollback_limit_counts_logical_lines_not_soft_wrap_rows() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.set_max_scrollback_logical_lines(1);

    term.push_scrollback_row(vec![Cell::new('A'); 4], true);
    term.push_scrollback_row(vec![Cell::new('B'); 4], false);

    assert_eq!(term.scrollback().len(), 2);
    assert_eq!(term.scrollback()[0][0].character, 'A');
    assert_eq!(term.scrollback()[1][0].character, 'B');

    term.push_scrollback_row(vec![Cell::new('C'); 4], false);

    assert_eq!(term.scrollback().len(), 1);
    assert_eq!(term.scrollback()[0][0].character, 'C');
}

#[test]
fn test_completed_scrollback_text_waits_for_soft_wrap_end() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));

    term.push_scrollback_row(vec![Cell::new('A'); 4], true);
    let (logged_rows, lines) = term.completed_scrollback_text_from(0);
    assert_eq!(logged_rows, 0);
    assert!(lines.is_empty());

    term.push_scrollback_row(vec![Cell::new('B'); 4], false);
    let (logged_rows, lines) = term.completed_scrollback_text_from(0);
    assert_eq!(logged_rows, 2);
    assert_eq!(lines, vec!["AAAABBBB".to_string()]);
}

#[test]
fn test_sgr_dim_sets_and_clears_attribute() {
    let mut term = Terminal::new(TerminalSize::new(1, 8));

    // SGR 2 (dim) should mark the following cells as DIM.
    term.process(b"\x1b[2mX");
    assert!(
        term.grid()[0][0].attributes.contains(CellAttributes::DIM),
        "SGR 2 should set the DIM attribute"
    );

    // SGR 22 (normal intensity) should clear DIM (and BOLD).
    term.process(b"\x1b[22mY");
    assert!(
        !term.grid()[0][1].attributes.contains(CellAttributes::DIM),
        "SGR 22 should clear the DIM attribute"
    );
}

#[test]
fn test_write_cell_overwrites_wide_cell_halves_without_orphans() {
    // Overwrite the lead half of a wide cell with a narrow char: the
    // continuation must be cleared.
    let mut term = Terminal::new(TerminalSize::new(1, 5));
    term.process("你a".as_bytes());
    term.process(b"\r\x1b[1GZ");
    assert_no_broken_wide_cells(term.grid()[0].as_slice());
    assert_eq!(term.grid()[0][0].character, 'Z');
    assert_eq!(term.grid()[0][1].character, ' ');
    assert!(!term.grid()[0][1].is_wide_continuation());
    assert_eq!(term.grid()[0][2].character, 'a');

    // Overwrite the continuation half: the lead must be cleared.
    let mut term = Terminal::new(TerminalSize::new(1, 5));
    term.process("你a".as_bytes());
    term.process(b"\r\x1b[2GZ");
    assert_no_broken_wide_cells(term.grid()[0].as_slice());
    assert_eq!(term.grid()[0][0].character, ' ');
    assert_eq!(term.grid()[0][1].character, 'Z');

    // Overwrite a wide cell with another wide cell (also across its tail).
    let mut term = Terminal::new(TerminalSize::new(1, 5));
    term.process("ab好cd".as_bytes());
    term.process(b"\r\x1b[3G");
    term.process("你".as_bytes());
    assert_no_broken_wide_cells(term.grid()[0].as_slice());
    assert_eq!(term.grid()[0][2].character, '你');
    assert!(term.grid()[0][3].is_wide_continuation());
}

#[test]
fn test_wide_character_in_single_column_terminal_is_cleared() {
    // A wide char that cannot fit its continuation is dropped entirely.
    let mut term = Terminal::new(TerminalSize::new(1, 1));
    term.process("你".as_bytes());
    assert_no_broken_wide_cells(term.grid()[0].as_slice());
    assert_eq!(term.grid()[0][0].character, ' ');
}

#[test]
fn test_scroll_up_beyond_region_blanks_without_scrollback_garbage() {
    // SU larger than the scroll region must not push blank padding rows
    // into the scrollback.
    let mut term = Terminal::new(TerminalSize::new(3, 4));
    term.process(b"one\r\ntwo\r\nthree");
    let before = term.scrollback().len();
    term.process(b"\x1b[99S");
    assert_eq!(term.scrollback().len(), before + 3);
    assert!(term
        .grid()
        .iter()
        .all(|row| row.iter().all(|cell| cell.character == ' ')));
}

#[test]
fn test_scroll_down_beyond_region_blanks_region() {
    let mut term = Terminal::new(TerminalSize::new(3, 4));
    term.process(b"one\r\ntwo\r\nthree");
    term.process(b"\x1b[99T");
    assert!(term
        .grid()
        .iter()
        .all(|row| row.iter().all(|cell| cell.character == ' ')));
}

#[test]
fn test_resize_reflow_clears_wide_chars_split_at_boundary() {
    // "ab你好cd" occupies 8 cells; reflowing from 6 to 5 columns splits 好
    // across the boundary. The severed halves must be cleared.
    let mut term = Terminal::new(TerminalSize::new(4, 6));
    term.process("ab你好cd".as_bytes());
    term.resize(TerminalSize::new(4, 5));
    for line_index in 0..term.total_line_count() {
        if let Some(line) = term.line_at(line_index) {
            assert_no_broken_wide_cells(line);
        }
    }
}

#[test]
fn test_scrollback_logical_count_stays_consistent_through_ed3_clear() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.process(b"one\r\ntwo\r\nthree\r\nfour");
    assert!(term.scrollback().len() >= 2);
    // ED 3 clears the scrollback; the logical line counter must reset too,
    // otherwise later truncation pops phantom lines.
    term.process(b"\x1b[3J");
    assert_eq!(term.scrollback().len(), 0);
    term.set_max_scrollback_logical_lines(1);
    term.process(b"five\r\nsix\r\nseven");
    assert!(term.scrollback().len() <= 2);
}

// -- zsh PROMPT_SP end-of-line marker filter (eol_mark.rs) --

fn eol_mark_sequence(fill_spaces: usize) -> Vec<u8> {
    let mut data = b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m".to_vec();
    data.extend(std::iter::repeat(b' ').take(fill_spaces));
    data.extend_from_slice(b"\r \r");
    data
}

fn screen_line(term: &Terminal, row: usize) -> String {
    term.grid()[row]
        .iter()
        .map(|c| c.character)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn eol_mark_self_erases_when_fill_fits_width() {
    // Width 10, fill 8 (fits): the sequence passes through and the prompt
    // redraw overwrites it, exactly like an unfiltered terminal.
    let mut term = Terminal::new(TerminalSize::new(3, 10));
    let mut data = eol_mark_sequence(8);
    data.extend_from_slice(b"prompt % ");
    term.process(&data);
    assert_eq!(screen_line(&term, 0), "prompt %");
    assert!(!term.grid()[0][0]
        .attributes
        .contains(CellAttributes::INVERSE));
}

#[test]
fn eol_mark_is_dropped_when_fill_wraps() {
    // Same marker emitted for a wider terminal (fill 12) replayed at width
    // 10: unfiltered it wraps and leaves a stray inverse `%` line. The
    // filter drops it so the prompt lands cleanly.
    let mut term = Terminal::new(TerminalSize::new(3, 10));
    let mut data = eol_mark_sequence(12);
    data.extend_from_slice(b"prompt % ");
    term.process(&data);
    assert_eq!(screen_line(&term, 0), "prompt %");
    assert_eq!(screen_line(&term, 1), "");
}

#[test]
fn eol_mark_filter_handles_split_chunks() {
    let mut data = eol_mark_sequence(12);
    data.extend_from_slice(b"prompt % ");

    let mut whole = Terminal::new(TerminalSize::new(3, 10));
    whole.process(&data);

    let mut split = Terminal::new(TerminalSize::new(3, 10));
    for byte in &data {
        split.process(&[*byte]);
    }

    for row in 0..3 {
        assert_eq!(screen_line(&whole, row), screen_line(&split, row));
    }
}

#[test]
fn eol_mark_preserves_partial_line_marker() {
    // Genuine use: output without trailing newline ("foo") keeps its marker
    // when the width matches (mark at column 3 → always passes through).
    let mut term = Terminal::new(TerminalSize::new(3, 10));
    let mut data = b"foo".to_vec();
    data.extend(eol_mark_sequence(8));
    data.extend_from_slice(b"\x1b[Jprompt % ");
    term.process(&data);
    assert_eq!(screen_line(&term, 0), "foo%");
    assert!(term.grid()[0][3]
        .attributes
        .contains(CellAttributes::INVERSE));
    assert_eq!(screen_line(&term, 1), "prompt %");
}

#[test]
fn eol_mark_filter_passes_similar_non_marker_bytes() {
    // Same SGR prefix but a different payload must pass through untouched.
    let mut term = Terminal::new(TerminalSize::new(3, 10));
    term.process(b"\x1b[1m\x1b[7mX\x1b[27m\x1b[1m\x1b[0m ok");
    assert_eq!(screen_line(&term, 0), "X ok");
    assert!(term.grid()[0][0]
        .attributes
        .contains(CellAttributes::INVERSE));
}

#[test]
fn test_resize_rows_shrink_and_regrow_keeps_cursor_with_content() {
    // Shell-like layout: a few content rows at the top of a tall grid with
    // the cursor on the last one. Shrinking absorbs the blank bottom rows
    // without scrolling content into scrollback, and growing back restores
    // the blank padding — the cursor must stay on its content row
    // throughout, otherwise the next prompt prints several rows lower (the
    // "blank gap" artifact).
    let mut term = Terminal::new(TerminalSize::new(38, 153));
    term.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    assert_eq!(term.cursor_pos(), (4, 4));

    term.resize(TerminalSize::new(30, 153));
    assert_eq!(term.cursor_pos(), (4, 4));
    assert_eq!(term.grid()[4][3].character, 'e');
    assert!(term.scrollback().is_empty());

    term.resize(TerminalSize::new(38, 153));
    assert_eq!(term.cursor_pos(), (4, 4));
    let line4: String = term.grid()[4].iter().take(4).map(|c| c.character).collect();
    assert_eq!(line4, "five");
}

#[test]
fn snapshot_roundtrip_reproduces_visible_state() {
    let mut term = Terminal::new(TerminalSize::new(6, 20));
    term.process(b"plain text\r\n");
    term.process(b"\x1b[1;31mbold red\x1b[0m\r\n");
    term.process(b"\x1b[48;5;27m\x1b[38;2;1;2;3mstyled\x1b[0m\r\n");
    term.process(b"wide \xE4\xB8\xAD char\r\n");
    term.process(b"\x1b[7minverse\x1b[0m");
    let snapshot = term.render_snapshot();

    let mut restored = Terminal::new(TerminalSize::new(6, 20));
    restored.process(&snapshot);

    assert_eq!(restored.grid(), term.grid());
    assert_eq!(restored.cursor_pos(), term.cursor_pos());
    assert!(!restored.using_alt_buffer);
}

#[test]
fn snapshot_roundtrip_preserves_alt_buffer_and_modes() {
    let mut term = Terminal::new(TerminalSize::new(6, 20));
    term.process(b"main screen");
    term.process(b"\x1b[?1049h");
    term.process(b"\x1b[2;5r\x1b[?6h");
    term.process(b"\x1b[1;1H\x1b[44malt content\x1b[0m");
    let snapshot = term.render_snapshot();

    let mut restored = Terminal::new(TerminalSize::new(6, 20));
    restored.process(&snapshot);

    assert!(restored.using_alt_buffer);
    assert_eq!(restored.grid(), term.grid());
    assert_eq!(restored.cursor_pos(), term.cursor_pos());
    assert_eq!(restored.top_margin, term.top_margin);
    assert_eq!(restored.bottom_margin, term.bottom_margin);
    assert!(restored.origin_mode);
}

#[test]
fn line_revision_changes_only_for_written_row() {
    let mut term = Terminal::new(TerminalSize::new(3, 4));
    let initial: Vec<_> = (0..3)
        .map(|line| term.line_revision(line).unwrap())
        .collect();

    term.process(b"\x1b[2;1HX");

    assert_eq!(term.line_revision(0), Some(initial[0]));
    assert_ne!(term.line_revision(1), Some(initial[1]));
    assert_eq!(term.line_revision(2), Some(initial[2]));
    assert_line_revision_invariants(&term);
}

#[test]
fn content_revision_changes_for_every_write() {
    let mut term = Terminal::new(TerminalSize::new(3, 4));
    let initial = term.content_revision();

    term.process(b"X");
    let after_first_write = term.content_revision();
    assert_ne!(after_first_write, initial);

    term.process(b"\x1b[1;1HY");
    assert_ne!(term.content_revision(), after_first_write);
}

#[test]
fn line_revisions_move_with_scrolled_and_prepended_rows() {
    let mut term = Terminal::new(TerminalSize::new(3, 4));
    term.process(b"\x1b[1;1HA\x1b[2;1HB\x1b[3;1HC");
    let before_scroll: Vec<_> = (0..3)
        .map(|line| term.line_revision(line).unwrap())
        .collect();

    term.scroll_up(1);

    assert_eq!(term.total_line_count(), 4);
    assert_eq!(term.line_revision(0), Some(before_scroll[0]));
    assert_eq!(term.line_revision(1), Some(before_scroll[1]));
    assert_eq!(term.line_revision(2), Some(before_scroll[2]));
    assert!(!before_scroll.contains(&term.line_revision(3).unwrap()));
    assert_line_revision_invariants(&term);

    let before_prepend: Vec<_> = (0..term.total_line_count())
        .map(|line| term.line_revision(line).unwrap())
        .collect();
    term.prepend_scrollback(vec![vec![Cell::new('x'); 4], vec![Cell::new('y'); 4]]);

    for (old_index, revision) in before_prepend.into_iter().enumerate() {
        assert_eq!(term.line_revision(old_index + 2), Some(revision));
    }
    assert_line_revision_invariants(&term);
}

#[test]
fn wholesale_rewrites_and_reset_refresh_line_revisions() {
    let mut term = Terminal::new(TerminalSize::new(3, 4));
    term.process(b"abc\r\ndef");
    let before_resize: std::collections::HashSet<_> = term
        .grid_revisions
        .iter()
        .chain(&term.alt_grid_revisions)
        .copied()
        .collect();

    term.resize(TerminalSize::new(4, 6));
    assert!(term
        .grid_revisions
        .iter()
        .chain(&term.alt_grid_revisions)
        .all(|revision| !before_resize.contains(revision)));
    assert_line_revision_invariants(&term);

    term.process(b"\x1b[?1049hALT");
    assert_line_revision_invariants(&term);
    term.process(b"\x1b[?1049l");

    let before_reset_instance = term.instance_id();
    let before_reset: std::collections::HashSet<_> = term
        .grid_revisions
        .iter()
        .chain(&term.alt_grid_revisions)
        .copied()
        .collect();
    term.process(b"\x1bc");
    assert_ne!(term.instance_id(), before_reset_instance);
    assert!(term
        .grid_revisions
        .iter()
        .chain(&term.alt_grid_revisions)
        .all(|revision| !before_reset.contains(revision)));
    assert_line_revision_invariants(&term);
}

#[test]
fn cloned_terminal_has_distinct_cache_instance_identity() {
    let term = Terminal::new(TerminalSize::new(2, 4));
    let cloned = term.clone();

    assert_ne!(term.instance_id(), cloned.instance_id());
    assert_eq!(term.grid_revisions, cloned.grid_revisions);
}

#[test]
fn test_osc10_osc11_queries_answer_with_theme_colors() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    term.set_theme_default_colors(
        Some(Color::Rgb(0x1d, 0x1d, 0x1f)),
        Some(Color::Rgb(0xf5, 0xf5, 0xf7)),
    );

    term.process(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\");

    // xterm reply shape: each 8-bit component duplicated into 4 hexits.
    assert_eq!(
        term.take_pending_query_responses(),
        b"\x1b]10;rgb:1d1d/1d1d/1f1f\x1b\\\x1b]11;rgb:f5f5/f5f5/f7f7\x1b\\"
    );
}

#[test]
fn test_osc11_query_stays_silent_while_background_is_unknown() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));

    term.process(b"\x1b]11;?\x1b\\");

    assert!(term.take_pending_query_responses().is_empty());
}

#[test]
fn test_osc11_set_beats_theme_default_and_resets_back() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    term.set_theme_default_colors(None, Some(Color::Rgb(0x1e, 0x1e, 0x1e)));

    // OpenCode-style set with a 2-hexit-per-component rgb: value.
    term.process(b"\x1b]11;rgb:24/27/3a\x07");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0x24, 0x27, 0x3a))
    );
    term.process(b"\x1b]11;?\x1b\\");
    assert_eq!(
        term.take_pending_query_responses(),
        b"\x1b]11;rgb:2424/2727/3a3a\x1b\\"
    );

    // OSC 111 resets the override; the host color becomes effective again.
    term.process(b"\x1b]111\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0x1e, 0x1e, 0x1e))
    );
    term.process(b"\x1b]11;?\x1b\\");
    assert_eq!(
        term.take_pending_query_responses(),
        b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\"
    );
}

#[test]
fn test_osc11_set_parses_xterm_and_web_color_forms() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));

    term.process(b"\x1b]11;rgb:f/00/0\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0xff, 0x00, 0x00))
    );

    term.process(b"\x1b]11;rgb:1234/5678/9abc\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0x12, 0x56, 0x9a))
    );

    term.process(b"\x1b]11;#abcdef\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0xab, 0xcd, 0xef))
    );

    term.process(b"\x1b]11;#fff\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0xff, 0xff, 0xff))
    );

    term.process(b"\x1b]11;bogus\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0xff, 0xff, 0xff))
    );
}

#[test]
fn test_osc_color_parser_rejects_non_ascii_without_panicking() {
    let mut term = Terminal::new(TerminalSize::new(2, 4));
    term.process(b"\x1b]11;#fff\x1b\\");

    for value in ["#éx", "#aébcd", "#aébcdef"] {
        term.process(format!("\x1b]11;{value}\x1b\\").as_bytes());
        assert_eq!(
            term.default_background(),
            Some(Color::Rgb(0xff, 0xff, 0xff))
        );
    }
}

#[test]
fn test_query_responder_off_suppresses_replies_but_keeps_state() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    term.set_theme_default_colors(None, Some(Color::Rgb(0x1e, 0x1e, 0x1e)));
    term.set_query_responder(false);

    term.process(b"\x1b]11;rgb:ff/ff/ff\x1b\\\x1b]11;?\x1b\\");
    term.process(b"\x1b[c\x1b[>c\x1b[5n\x1b[6n\x1b[?u\x1b[18t");

    assert!(term.take_pending_query_responses().is_empty());
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0xff, 0xff, 0xff))
    );
}

#[test]
fn test_ris_keeps_theme_colors_but_drops_osc_overrides() {
    let mut term = Terminal::new_with_type(TerminalSize::new(24, 80), Some("xterm-256color"));
    term.set_theme_default_colors(None, Some(Color::Rgb(0x1e, 0x1e, 0x1e)));
    term.process(b"\x1b]11;rgb:ff/ff/ff\x1b\\");
    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0xff, 0xff, 0xff))
    );

    term.process(b"\x1bc");

    assert_eq!(
        term.default_background(),
        Some(Color::Rgb(0x1e, 0x1e, 0x1e))
    );
}
