use super::*;

impl Terminal {
    pub fn current_line_text(&self) -> String {
        let current_line = self.cursor_line_index();
        if self.line_at(current_line).is_none() {
            return String::new();
        }

        let mut start_line = current_line;
        while start_line > 0 && self.line_wrapped(start_line - 1) {
            start_line -= 1;
        }

        let mut text = String::new();
        for line_index in start_line..=current_line {
            if let Some(line) = self.line_at(line_index) {
                append_line_text(&mut text, line);
            }
        }
        text
    }

    pub fn completed_scrollback_text_from(&self, start_row: usize) -> (usize, Vec<String>) {
        let start_row = start_row.min(self.scrollback.len());
        let mut committed_row = start_row;
        let mut lines = Vec::new();
        let mut line_buf = String::new();

        for row in start_row..self.scrollback.len() {
            if let Some(line) = self.scrollback.get(row) {
                append_line_text(&mut line_buf, line);
            }

            if !self.scrollback_wrapped.get(row).copied().unwrap_or(false) {
                lines.push(line_buf.trim_end().to_string());
                line_buf.clear();
                committed_row = row + 1;
            }
        }

        (committed_row, lines)
    }

    /// Get the current grid as a string (for debugging)
    pub fn grid_to_string(&self) -> String {
        let grid = self.grid();
        let mut result = String::new();
        for row in grid {
            for cell in row {
                if !cell.is_wide_continuation() {
                    result.push(cell.character);
                }
            }
            result.push('\n');
        }
        result
    }

    /// Get the screen text with soft-wrapped and indented-continuation lines joined,
    /// trailing whitespace trimmed per row.
    pub fn unwrapped_screen_text(&self) -> String {
        let grid = self.grid();
        let wrapped = if self.using_alt_buffer {
            &self.alt_row_wrapped
        } else {
            &self.row_wrapped
        };

        // Phase 1: Build lines from grid, joining terminal soft-wrapped rows.
        let mut raw_lines: Vec<String> = Vec::new();
        let mut i = 0;
        while i < grid.len() {
            let mut line_text = String::new();
            append_line_text(&mut line_text, &grid[i]);

            // Join consecutive soft-wrapped rows
            while wrapped.get(i) == Some(&true) && i + 1 < grid.len() {
                i += 1;
                append_line_text(&mut line_text, &grid[i]);
            }

            let trimmed = line_text.trim_end();
            if !trimmed.is_empty() {
                raw_lines.push(trimmed.to_string());
            }
            i += 1;
        }

        // Phase 2: Heuristic — join lines that start with spaces (paragraph continuation).
        // Many CLI apps (e.g. Claude Code via Ink) handle line wrapping themselves and
        // emit explicit newlines.  Continuation lines are indented with leading spaces.
        let mut joined: Vec<String> = Vec::new();
        for line in raw_lines {
            if let Some(prev) = joined.last_mut() {
                if line.starts_with(' ') && !prev.is_empty() {
                    prev.push(' ');
                    prev.push_str(line.trim_start());
                    continue;
                }
            }
            joined.push(line);
        }

        let mut result = String::new();
        for line in &joined {
            result.push_str(line);
            result.push('\n');
        }
        result
    }
}
