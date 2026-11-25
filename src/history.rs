use std::collections::VecDeque;
use crate::{Cell, CellAttributes, Color, TerminalSize};

use super::*;

impl Terminal {
    pub fn load_history_state(
        &mut self,
        size: TerminalSize,
        mut lines: Vec<Vec<Cell>>,
        cursor_pos: (u16, u16),
    ) {
        let rows = size.rows as usize;
        let cols = size.cols as usize;

        lines.resize(rows, vec![Cell::empty(); cols]);
        for row in &mut lines {
            row.resize(cols, Cell::empty());
        }

        let split_index = lines.len().saturating_sub(rows);
        let mut scrollback = VecDeque::with_capacity(split_index.max(1));
        for line in lines.drain(..split_index) {
            scrollback.push_back(line);
        }
        let scrollback_wrapped = VecDeque::from(vec![false; scrollback.len()]);

        self.size = size;
        self.grid = lines;
        self.alt_grid = create_grid(rows, cols);
        self.row_wrapped = vec![false; rows];
        self.alt_row_wrapped = vec![false; rows];
        self.using_alt_buffer = false;
        self.scrollback = scrollback;
        self.scrollback_wrapped = scrollback_wrapped;
        self.refresh_all_line_revisions();
        self.recount_scrollback_logical_lines();
        self.truncate_scrollback_to_logical_limit();
        self.tab_stops = vec![false; cols];
        for index in (0..cols).step_by(8) {
            self.tab_stops[index] = true;
        }
        self.cursor_pos = (
            cursor_pos.0.min(size.rows.saturating_sub(1)) as usize,
            cursor_pos.1.min(size.cols.saturating_sub(1)) as usize,
        );
        self.pending_wrap = false;
        self.saved_cursor_pos = self.cursor_pos;
        self.saved_cursor_state.cursor_pos = self.cursor_pos;
        self.saved_cursor_state.pending_wrap = false;
        self.cursor_visible = true;
        self.fg_color = Color::Default;
        self.bg_color = Color::Default;
        self.attributes = CellAttributes::empty();
        self.active_hyperlink_id = 0;
        self.last_printed_char = ' ';
        self.auto_wrap = true;
        self.insert_mode = false;
        self.origin_mode = false;
        self.selection = None;
        self.app_keypad = false;
        self.app_cursor = false;
        self.mouse_mode = MouseMode::None;
        self.mouse_encoding = MouseEncoding::Default;
        self.bracketed_paste = false;
        self.kitty_keyboard_flags = 0;
        self.kitty_keyboard_stack.clear();
        self.inactive_kitty_keyboard = (0, Vec::new());
        self.g0_charset = Charset::Ascii;
        self.g1_charset = Charset::Ascii;
        self.active_charset = ActiveCharset::G0;
        self.title.clear();
        self.bottom_margin = rows.saturating_sub(1);
        self.dirty = true;
    }

    /// Get scrollback buffer
    pub fn scrollback(&self) -> &VecDeque<Vec<Cell>> {
        &self.scrollback
    }

    pub fn total_line_count(&self) -> usize {
        if self.using_alt_buffer {
            self.alt_grid.len()
        } else {
            self.scrollback.len() + self.grid.len()
        }
    }

    /// Whether the alternate screen buffer is currently active (DECSET
    /// 47/1047/1049). The alternate grid keeps no scrollback, so embedders
    /// use this to disable scrollback UI while a full-screen TUI is showing.
    pub fn is_alternate_screen(&self) -> bool {
        self.using_alt_buffer
    }

    pub fn line_at(&self, line_index: usize) -> Option<&Vec<Cell>> {
        if self.using_alt_buffer {
            self.alt_grid.get(line_index)
        } else if line_index < self.scrollback.len() {
            self.scrollback.get(line_index)
        } else {
            self.grid.get(line_index - self.scrollback.len())
        }
    }

    pub fn cursor_line_index(&self) -> usize {
        if self.using_alt_buffer {
            self.cursor_pos.0
        } else {
            self.scrollback.len() + self.cursor_pos.0
        }
    }

    pub fn history_page(&self, before_line: usize, max_lines: usize) -> (usize, Vec<Vec<Cell>>) {
        let total_lines = self.total_line_count();
        let end_line = before_line.min(total_lines);
        let start_line = end_line.saturating_sub(max_lines);
        let mut lines = Vec::with_capacity(end_line.saturating_sub(start_line));

        for line_index in start_line..end_line {
            if let Some(line) = self.line_at(line_index) {
                lines.push(line.clone());
            }
        }

        (start_line, lines)
    }

    pub fn latest_history_page(&self, max_lines: usize) -> (usize, Vec<Vec<Cell>>) {
        self.history_page(self.total_line_count(), max_lines)
    }

    pub fn prepend_scrollback(&mut self, mut lines: Vec<Vec<Cell>>) {
        // All prepended rows are marked non-wrapped, so each of them closes
        // one logical line in the counter.
        self.scrollback_logical_lines += lines.len();
        while let Some(line) = lines.pop() {
            let revision = self.fresh_line_revision();
            self.scrollback.push_front(line);
            self.scrollback_revisions.push_front(revision);
            self.scrollback_wrapped.push_front(false);
        }
        self.truncate_scrollback_to_logical_limit();
        self.dirty = true;
    }
}
