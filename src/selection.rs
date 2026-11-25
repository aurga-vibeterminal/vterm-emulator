use super::*;

impl Terminal {
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.dirty = true;
    }

    pub fn selection(&self) -> Option<(usize, usize, usize, usize)> {
        self.selection
            .map(|(start_row, start_col, end_row, end_col)| {
                normalize_selection(start_row, start_col, end_row, end_col)
            })
    }

    /// Set selection
    pub fn set_selection(
        &mut self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) {
        self.selection = Some((start_row, start_col, end_row, end_col));
        self.dirty = true;
    }

    pub fn is_selected(&self, row: usize, col: usize) -> bool {
        self.selection()
            .map(|(start_row, start_col, end_row, end_col)| {
                if row < start_row || row > end_row {
                    return false;
                }

                if start_row == end_row {
                    return row == start_row && col >= start_col && col <= end_col;
                }

                if row == start_row {
                    return col >= start_col;
                }

                if row == end_row {
                    return col <= end_col;
                }

                true
            })
            .unwrap_or(false)
    }

    /// Get selected text
    pub fn selected_text(&self) -> Option<String> {
        self.selection()
            .map(|(start_row, start_col, end_row, end_col)| {
                let mut text = String::new();
                let mut line_buf = String::new();
                let grid = self.grid();

                for row in start_row..=end_row {
                    if let Some(line) = grid.get(row) {
                        let start = if row == start_row { start_col } else { 0 };
                        let end = if row == end_row {
                            end_col
                        } else {
                            line.len().saturating_sub(1)
                        };

                        for col in start..=end {
                            if let Some(cell) = line.get(col) {
                                if !cell.is_wide_continuation() {
                                    line_buf.push(cell.character);
                                }
                            }
                        }

                        let is_last = row == end_row;
                        if is_last || !self.row_wrapped(row) {
                            text.push_str(line_buf.trim_end());
                            line_buf.clear();
                            if !is_last {
                                text.push('\n');
                            }
                        }
                    }
                }

                text
            })
    }

    pub fn set_selection_absolute(
        &mut self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) {
        self.selection = Some((start_row, start_col, end_row, end_col));
        self.dirty = true;
    }

    pub fn selection_absolute(&self) -> Option<(usize, usize, usize, usize)> {
        self.selection
            .map(|(start_row, start_col, end_row, end_col)| {
                normalize_selection(start_row, start_col, end_row, end_col)
            })
    }

    pub fn is_selected_absolute(&self, row: usize, col: usize) -> bool {
        self.selection_absolute()
            .map(|(start_row, start_col, end_row, end_col)| {
                if row < start_row || row > end_row {
                    return false;
                }

                if start_row == end_row {
                    return row == start_row && col >= start_col && col <= end_col;
                }

                if row == start_row {
                    return col >= start_col;
                }

                if row == end_row {
                    return col <= end_col;
                }

                true
            })
            .unwrap_or(false)
    }

    pub fn selected_text_absolute(&self) -> Option<String> {
        self.selection_absolute()
            .map(|(start_row, start_col, end_row, end_col)| {
                let mut text = String::new();
                let mut line_buf = String::new();

                for row in start_row..=end_row {
                    if let Some(line) = self.line_at(row) {
                        let start = if row == start_row { start_col } else { 0 };
                        let end = if row == end_row {
                            end_col
                        } else {
                            line.len().saturating_sub(1)
                        };

                        for col in start..=end {
                            if let Some(cell) = line.get(col) {
                                if !cell.is_wide_continuation() {
                                    line_buf.push(cell.character);
                                }
                            }
                        }

                        // Only break to a new line when the current row did
                        // not soft-wrap onto the next — wrapped rows are
                        // joined because the original content was a single
                        // logical line. Trailing whitespace is trimmed from
                        // each logical line so the clipboard doesn't pick up
                        // the empty cells that pad rows to the column width.
                        let is_last = row == end_row;
                        if is_last || !self.line_wrapped(row) {
                            text.push_str(line_buf.trim_end());
                            line_buf.clear();
                            if !is_last {
                                text.push('\n');
                            }
                        }
                    }
                }

                text
            })
    }

    /// Extract the entire terminal buffer as text, joining soft-wrapped rows
    /// into single logical lines and trimming trailing whitespace per line.
    pub fn full_text_absolute(&self) -> String {
        let total = self.total_line_count();
        let mut text = String::new();
        let mut line_buf = String::new();

        for row in 0..total {
            if let Some(line) = self.line_at(row) {
                for cell in line.iter() {
                    if !cell.is_wide_continuation() {
                        line_buf.push(cell.character);
                    }
                }

                let is_last = row == total.saturating_sub(1);
                if is_last || !self.line_wrapped(row) {
                    text.push_str(line_buf.trim_end());
                    line_buf.clear();
                    if !is_last {
                        text.push('\n');
                    }
                }
            }
        }

        text
    }

    // Internal helper methods - avoid borrowing conflicts by extracting values first
}
