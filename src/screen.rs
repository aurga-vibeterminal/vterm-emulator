use crate::{Cell, CellAttributes, Color};
use tracing::debug;
use unicode_width::UnicodeWidthChar;
use vte::Params;

use super::*;

impl Terminal {
    pub(super) fn scroll_up(&mut self, lines: usize) {
        let top_margin = self.top_margin;
        let bottom_margin = self.bottom_margin;
        let using_alt = self.using_alt_buffer;

        let region_len = bottom_margin.saturating_add(1).saturating_sub(top_margin);
        // Scrolling by more than the region height just blanks the region.
        let lines = lines.min(region_len);
        if lines == 0 {
            return;
        }

        // Save the rows leaving the region at the top to scrollback
        // (main buffer only).
        if !using_alt {
            for i in 0..lines {
                let top_line = self.grid[top_margin + i].clone();
                let top_revision = self.grid_revisions[top_margin + i];
                let top_wrapped = self
                    .row_wrapped
                    .get(top_margin + i)
                    .copied()
                    .unwrap_or(false);
                self.push_scrollback_row_with_revision(top_line, top_wrapped, top_revision);
            }
        }

        // Rotate the scroll region instead of cloning every row per scrolled
        // line: `rotate_left` moves the row headers in bulk and the vacated
        // rows at the bottom are blanked in place, reusing their allocation.
        let grid = if using_alt {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        let region = &mut grid[top_margin..=bottom_margin];
        region.rotate_left(lines);
        for row in &mut region[region_len - lines..] {
            row.fill(Cell::empty());
        }

        let revisions = if using_alt {
            &mut self.alt_grid_revisions
        } else {
            &mut self.grid_revisions
        };
        let revision_region = &mut revisions[top_margin..=bottom_margin];
        revision_region.rotate_left(lines);
        self.mark_active_rows_changed(bottom_margin + 1 - lines, bottom_margin + 1);

        // Scroll wrapped flags in sync with the grid
        let wrapped = if using_alt {
            &mut self.alt_row_wrapped
        } else {
            &mut self.row_wrapped
        };
        let wrapped_region = &mut wrapped[top_margin..=bottom_margin];
        wrapped_region.rotate_left(lines);
        for flag in &mut wrapped_region[region_len - lines..] {
            *flag = false;
        }

        self.dirty = true;
    }

    pub(super) fn scroll_down(&mut self, lines: usize) {
        let top_margin = self.top_margin;
        let bottom_margin = self.bottom_margin;
        let using_alt = self.using_alt_buffer;

        let region_len = bottom_margin.saturating_add(1).saturating_sub(top_margin);
        // Scrolling by more than the region height just blanks the region.
        let lines = lines.min(region_len);
        if lines == 0 {
            return;
        }

        // Rotate instead of cloning every row per scrolled line; the vacated
        // rows at the top are blanked in place, reusing their allocation.
        let grid = if using_alt {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        let region = &mut grid[top_margin..=bottom_margin];
        region.rotate_right(lines);
        for row in &mut region[..lines] {
            row.fill(Cell::empty());
        }

        let revisions = if using_alt {
            &mut self.alt_grid_revisions
        } else {
            &mut self.grid_revisions
        };
        let revision_region = &mut revisions[top_margin..=bottom_margin];
        revision_region.rotate_right(lines);
        self.mark_active_rows_changed(top_margin, top_margin + lines);

        self.dirty = true;
    }

    pub(super) fn move_cursor_to(&mut self, row: usize, col: usize) {
        let max_row = self.size.rows as usize - 1;
        let max_col = self.size.cols as usize - 1;
        let target_row = if self.origin_mode {
            let max_origin_row = self.bottom_margin.saturating_sub(self.top_margin);
            self.top_margin + row.min(max_origin_row)
        } else {
            row.min(max_row)
        };

        self.cursor_pos = (target_row.min(max_row), col.min(max_col));
        self.pending_wrap = false;
    }

    pub(super) fn carriage_return(&mut self) {
        self.cursor_pos.1 = 0;
        self.pending_wrap = false;
    }

    pub(super) fn linefeed(&mut self) {
        let row = self.cursor_pos.0;
        let bottom_margin = self.bottom_margin;

        if row == bottom_margin {
            self.scroll_up(1);
        } else {
            self.cursor_pos.0 = (row + 1).min(self.size.rows as usize - 1);
        }
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor_pos.1 > 0 {
            self.cursor_pos.1 -= 1;
            self.pending_wrap = false;
        }
    }

    pub(super) fn tab(&mut self) {
        if self.pending_wrap {
            self.wrapline();
            return;
        }

        let col = self.cursor_pos.1;
        let row = self.cursor_pos.0;
        let max_col = self.size.cols as usize - 1;

        let cell = Cell {
            character: '\t',
            fg_color: self.fg_color,
            bg_color: self.bg_color,
            attributes: self.attributes,
            hyperlink_id: self.active_hyperlink_id,
        };
        let grid = if self.using_alt_buffer {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        write_cell(&mut grid[row], col, cell);
        self.mark_active_row_changed(row);

        for i in (col + 1)..=max_col {
            if *self.tab_stops.get(i).unwrap_or(&false) {
                self.cursor_pos.1 = i;
                self.dirty = true;
                return;
            }
        }

        self.cursor_pos.1 = max_col;
        self.dirty = true;
    }

    pub(super) fn move_forward_tab(&mut self) {
        let col = self.cursor_pos.1;
        let max_col = self.size.cols as usize - 1;

        for i in (col + 1)..=max_col {
            if *self.tab_stops.get(i).unwrap_or(&false) {
                self.cursor_pos.1 = i;
                self.pending_wrap = false;
                return;
            }
        }

        self.cursor_pos.1 = max_col;
        self.pending_wrap = false;
    }

    pub(super) fn move_backward_tab(&mut self) {
        let col = self.cursor_pos.1;
        for i in (0..col).rev() {
            if *self.tab_stops.get(i).unwrap_or(&false) {
                self.cursor_pos.1 = i;
                self.pending_wrap = false;
                return;
            }
        }

        self.cursor_pos.1 = 0;
        self.pending_wrap = false;
    }

    pub(super) fn save_cursor_state(&mut self) {
        let state = SavedCursorState {
            cursor_pos: self.cursor_pos,
            pending_wrap: self.pending_wrap,
            fg_color: self.fg_color,
            bg_color: self.bg_color,
            attributes: self.attributes,
            g0_charset: self.g0_charset,
            g1_charset: self.g1_charset,
            active_charset: self.active_charset,
        };
        if self.using_alt_buffer {
            self.alt_saved_cursor_state = state;
        } else {
            self.saved_cursor_state = state;
            self.saved_cursor_pos = self.cursor_pos;
        }
    }

    pub(super) fn restore_cursor_state(&mut self) {
        let state = if self.using_alt_buffer {
            self.alt_saved_cursor_state
        } else {
            self.saved_cursor_state
        };
        self.cursor_pos = state.cursor_pos;
        self.pending_wrap = state.pending_wrap;
        if !self.using_alt_buffer {
            self.saved_cursor_pos = state.cursor_pos;
        }
        self.fg_color = state.fg_color;
        self.bg_color = state.bg_color;
        self.attributes = state.attributes;
        self.g0_charset = state.g0_charset;
        self.g1_charset = state.g1_charset;
        self.active_charset = state.active_charset;
        self.dirty = true;
    }

    pub(super) fn blank_cell(&self) -> Cell {
        Cell {
            character: ' ',
            fg_color: Color::Default,
            bg_color: self.bg_color,
            attributes: CellAttributes::empty(),
            hyperlink_id: 0,
        }
    }

    pub(super) fn put_char(&mut self, c: char) {
        let mut row = self.cursor_pos.0;
        let mut col = self.cursor_pos.1;
        let cols = self.size.cols as usize;
        let c = self.translate_char(c);
        if UnicodeWidthChar::width(c) == Some(0) {
            self.dirty = true;
            return;
        }
        self.last_printed_char = c;
        let char_width = Cell::display_width_for_char(c).min(cols.max(1));

        if self.pending_wrap {
            self.wrapline();
            row = self.cursor_pos.0;
            col = self.cursor_pos.1;
        }

        // Handle auto-wrap
        if char_width > 1 && col + char_width > cols {
            if self.auto_wrap {
                let spacer = Cell {
                    character: ' ',
                    fg_color: self.fg_color,
                    bg_color: self.bg_color,
                    attributes: self.attributes,
                    hyperlink_id: self.active_hyperlink_id,
                };
                let grid = if self.using_alt_buffer {
                    &mut self.alt_grid
                } else {
                    &mut self.grid
                };
                write_cell(&mut grid[row], col, spacer);
                self.mark_active_row_changed(row);
                self.wrapline();
                row = self.cursor_pos.0;
                col = self.cursor_pos.1;
            } else {
                self.cursor_pos.1 = cols.saturating_sub(1);
                self.pending_wrap = true;
                return;
            }
        }

        // Create cell with current attributes
        let cell = Cell {
            character: c,
            fg_color: self.fg_color,
            bg_color: self.bg_color,
            attributes: self.attributes,
            hyperlink_id: self.active_hyperlink_id,
        };
        let blank = self.blank_cell();

        let grid = if self.using_alt_buffer {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };

        if self.insert_mode {
            insert_blank_cells_with(&mut grid[row], col, char_width, blank);
        }

        write_cell(&mut grid[row], col, cell);
        self.mark_active_row_changed(row);
        if col + char_width < cols {
            self.cursor_pos.1 = col + char_width;
            self.pending_wrap = false;
        } else {
            self.cursor_pos.1 = cols.saturating_sub(1);
            self.pending_wrap = true;
        }
        self.dirty = true;
    }

    pub(super) fn wrapline(&mut self) {
        if !self.auto_wrap {
            return;
        }

        let row = self.cursor_pos.0;
        let wrapped = if self.using_alt_buffer {
            &mut self.alt_row_wrapped
        } else {
            &mut self.row_wrapped
        };
        if row < wrapped.len() {
            wrapped[row] = true;
        }
        self.linefeed();
        self.cursor_pos.1 = 0;
        self.pending_wrap = false;
    }

    pub(super) fn erase_in_display(&mut self, mode: u16) {
        let row = self.cursor_pos.0;
        let col = self.cursor_pos.1;
        let rows = self.size.rows as usize;
        let cols = self.size.cols as usize;
        let blank = self.blank_cell();

        let grid = if self.using_alt_buffer {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        let wrapped = if self.using_alt_buffer {
            &mut self.alt_row_wrapped
        } else {
            &mut self.row_wrapped
        };

        let changed_rows = match mode {
            0 => {
                clear_cell_range_with(&mut grid[row], col, cols, blank);
                wrapped[row] = false;
                for r in (row + 1)..rows {
                    grid[r].fill(Cell::empty());
                    wrapped[r] = false;
                }
                Some((row, rows))
            }
            1 => {
                for r in 0..row {
                    grid[r].fill(Cell::empty());
                    wrapped[r] = false;
                }
                clear_cell_range_with(&mut grid[row], 0, col.saturating_add(1), blank);
                wrapped[row] = false;
                Some((0, row + 1))
            }
            2 | 3 => {
                for r in 0..rows {
                    grid[r].fill(Cell::empty());
                    wrapped[r] = false;
                }
                if mode == 3 && !self.using_alt_buffer {
                    self.scrollback.clear();
                    self.scrollback_revisions.clear();
                    self.scrollback_wrapped.clear();
                    self.scrollback_logical_lines = 0;
                }
                Some((0, rows))
            }
            _ => None,
        };

        if let Some((start, end)) = changed_rows {
            self.mark_active_rows_changed(start, end);
        }

        self.dirty = true;
    }

    pub(super) fn erase_in_line(&mut self, mode: u16) {
        let row = self.cursor_pos.0;
        let col = self.cursor_pos.1;
        let cols = self.size.cols as usize;
        let blank = self.blank_cell();

        let grid = if self.using_alt_buffer {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };

        let changed = match mode {
            0 if self.pending_wrap => false,
            0 => {
                clear_cell_range_with(&mut grid[row], col, cols, blank);
                true
            }
            1 => {
                clear_cell_range_with(&mut grid[row], 0, col.saturating_add(1), blank);
                true
            }
            2 => {
                grid[row].fill(blank);
                true
            }
            _ => false,
        };
        if changed {
            self.mark_active_row_changed(row);
        }

        self.dirty = true;
    }

    pub(super) fn set_sgr(&mut self, params: &Params) {
        let mut iter = params.iter().peekable();

        while let Some(param) = iter.next() {
            let first = param.first().copied().unwrap_or(0);
            match first {
                0 => {
                    // Reset
                    self.fg_color = Color::Default;
                    self.bg_color = Color::Default;
                    self.attributes = CellAttributes::empty();
                }
                1 => self.attributes |= CellAttributes::BOLD,
                2 => self.attributes |= CellAttributes::DIM,
                3 => self.attributes |= CellAttributes::ITALIC,
                4 => self.attributes |= CellAttributes::UNDERLINE,
                5 => self.attributes |= CellAttributes::BLINK,
                7 => self.attributes |= CellAttributes::INVERSE,
                8 => self.attributes |= CellAttributes::HIDDEN,
                9 => self.attributes |= CellAttributes::STRIKETHROUGH,
                22 => self.attributes &= !(CellAttributes::BOLD | CellAttributes::DIM),
                23 => self.attributes &= !CellAttributes::ITALIC,
                24 => self.attributes &= !CellAttributes::UNDERLINE,
                25 => self.attributes &= !CellAttributes::BLINK,
                27 => self.attributes &= !CellAttributes::INVERSE,
                28 => self.attributes &= !CellAttributes::HIDDEN,
                29 => self.attributes &= !CellAttributes::STRIKETHROUGH,
                30..=37 => {
                    // Foreground color (standard)
                    self.fg_color = ansi_color_to_color(first - 30, false);
                }
                38 => {
                    // Foreground true color or 256 color
                    if let Some(next_param) = iter.peek() {
                        if let Some(&2) = next_param.first() {
                            // True color
                            iter.next();
                            let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            self.fg_color = Color::Rgb(r, g, b);
                        } else if let Some(&5) = next_param.first() {
                            // 256 color
                            iter.next();
                            let idx =
                                iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            self.fg_color = Color::Indexed(idx);
                        }
                    }
                }
                39 => self.fg_color = Color::Default,
                40..=47 => {
                    // Background color (standard)
                    self.bg_color = ansi_color_to_color(first - 40, false);
                }
                48 => {
                    // Background true color or 256 color
                    if let Some(next_param) = iter.peek() {
                        if let Some(&2) = next_param.first() {
                            // True color
                            iter.next();
                            let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            self.bg_color = Color::Rgb(r, g, b);
                        } else if let Some(&5) = next_param.first() {
                            // 256 color
                            iter.next();
                            let idx =
                                iter.next().and_then(|p| p.first().copied()).unwrap_or(0) as u8;
                            self.bg_color = Color::Indexed(idx);
                        }
                    }
                }
                49 => self.bg_color = Color::Default,
                90..=97 => {
                    // Bright foreground
                    self.fg_color = ansi_color_to_color(first - 90, true);
                }
                100..=107 => {
                    // Bright background
                    self.bg_color = ansi_color_to_color(first - 100, true);
                }
                _ => {}
            }
        }
    }

    pub(super) fn decode_charset(&self, byte: u8) -> Charset {
        match byte {
            b'0' if self.terminal_type.supports_dec_special_graphics() => {
                Charset::DecSpecialGraphics
            }
            _ => Charset::Ascii,
        }
    }

    pub(super) fn active_charset_definition(&self) -> Charset {
        match self.active_charset {
            ActiveCharset::G0 => self.g0_charset,
            ActiveCharset::G1 => self.g1_charset,
        }
    }

    pub(super) fn translate_char(&self, c: char) -> char {
        match self.active_charset_definition() {
            Charset::Ascii => c,
            Charset::DecSpecialGraphics => dec_special_graphics(c),
        }
    }

    pub(super) fn set_mode(&mut self, params: &Params) {
        for param in params.iter() {
            match param.first().copied().unwrap_or(0) {
                4 => self.insert_mode = true,
                _ => {}
            }
        }
    }

    pub(super) fn reset_mode(&mut self, params: &Params) {
        for param in params.iter() {
            match param.first().copied().unwrap_or(0) {
                4 => self.insert_mode = false,
                _ => {}
            }
        }
    }

    pub(super) fn dispatch_private_csi(&mut self, params: &Params, command: char) {
        if !self.terminal_type.supports_dec_private_modes() {
            return;
        }

        // The kitty keyboard flags query (`CSI ? u`) carries no mode params,
        // so it must be handled outside the per-param loop below.
        if command == 'u' {
            self.queue_query_response(format!("\x1b[?{}u", self.kitty_keyboard_flags).as_bytes());
            return;
        }

        for param in params.iter() {
            let mode = param.first().copied().unwrap_or(0);
            match command {
                'h' => self.set_private_mode(mode),
                'l' => self.reset_private_mode(mode),
                _ => {}
            }
        }
    }

    /// XTWINOPS window manipulation (`CSI Ps ; … t`). Commands that move,
    /// resize, or (de)iconify the host window do not apply to a virtual
    /// terminal and are ignored; the query forms must still be answered
    /// because TUI applications block on the reply (e.g. cell-size probes
    /// via `CSI 16 t`).
    pub(super) fn handle_window_manipulation(&mut self, params: &Params) {
        let param = |index: usize| -> u16 {
            params
                .iter()
                .nth(index)
                .and_then(|p| p.first().copied())
                .unwrap_or(0)
        };
        let query = param(0);
        let response = match query {
            // Report window state: not iconified.
            11 => Some("\x1b[1t".to_string()),
            // Report window position in pixels.
            13 => Some("\x1b[3;0;0t".to_string()),
            // Report text-area (14) / whole-screen (15) size in pixels.
            14 | 15 => {
                let lead = if query == 14 { 4 } else { 5 };
                let height = self.size.rows as u32 * self.cell_height_px as u32;
                let width = self.size.cols as u32 * self.cell_width_px as u32;
                Some(format!("\x1b[{lead};{height};{width}t"))
            }
            // Report character cell size in pixels.
            16 => Some(format!(
                "\x1b[6;{};{}t",
                self.cell_height_px, self.cell_width_px
            )),
            // Report text-area (18) / whole-screen (19) size in characters.
            18 | 19 => {
                let lead = if query == 18 { 8 } else { 9 };
                Some(format!(
                    "\x1b[{lead};{};{}t",
                    self.size.rows, self.size.cols
                ))
            }
            _ => None,
        };
        if let Some(response) = response {
            self.queue_query_response(response.as_bytes());
        }
    }

    /// Kitty keyboard protocol: push the active screen's current flags onto
    /// its stack and set the new flags (`CSI > flags u`, default 0). A full
    /// stack evicts the oldest entry, per the spec.
    pub(super) fn kitty_keyboard_push(&mut self, flags: u16) {
        const MAX_STACK: usize = 32;
        if self.kitty_keyboard_stack.len() >= MAX_STACK {
            self.kitty_keyboard_stack.remove(0);
        }
        self.kitty_keyboard_stack.push(self.kitty_keyboard_flags);
        self.kitty_keyboard_flags = flags;
    }

    /// Kitty keyboard protocol: pop `count` stack entries (`CSI < number u`,
    /// default 1). Each popped value becomes the current flags; a pop that
    /// finds the stack empty resets all flags.
    pub(super) fn kitty_keyboard_pop(&mut self, count: u16) {
        for _ in 0..count.max(1) {
            match self.kitty_keyboard_stack.pop() {
                Some(flags) => self.kitty_keyboard_flags = flags,
                None => {
                    self.kitty_keyboard_flags = 0;
                    break;
                }
            }
        }
    }

    /// Kitty keyboard protocol: apply flags (`CSI = flags ; mode u`).
    /// Mode 1 (default): set exactly; 2: set the given bits; 3: reset the
    /// given bits.
    pub(super) fn kitty_keyboard_set(&mut self, flags: u16, mode: u16) {
        match mode {
            2 => self.kitty_keyboard_flags |= flags,
            3 => self.kitty_keyboard_flags &= !flags,
            _ => self.kitty_keyboard_flags = flags,
        }
    }

    pub(super) fn set_private_mode(&mut self, mode: u16) {
        match mode {
            1 => self.app_cursor = true,
            3 => self.deccolm(),
            6 => {
                self.origin_mode = true;
                self.move_cursor_to(0, 0);
            }
            7 => self.auto_wrap = true,
            25 => self.cursor_visible = true,
            47 | 1047 => {
                //                 if mode == 47 {
                //                     debug!("ANSI private mode 47 set: enter alternate screen");
                //                 }
                self.enter_alt_buffer(false);
            }
            1048 => self.save_cursor_state(),
            1049 => {
                //                 debug!("ANSI private mode 1049 set: save cursor and enter alternate screen");
                self.save_cursor_state();
                self.enter_alt_buffer(true);
            }
            2004 => self.bracketed_paste = true,
            9 => self.mouse_mode = MouseMode::X10,
            1000 => self.mouse_mode = MouseMode::Normal,
            1002 => self.mouse_mode = MouseMode::ButtonEvent,
            1003 => self.mouse_mode = MouseMode::AnyEvent,
            1005 => self.mouse_encoding = MouseEncoding::Utf8,
            1006 => self.mouse_encoding = MouseEncoding::Sgr,
            _ => {}
        }
    }

    pub(super) fn reset_private_mode(&mut self, mode: u16) {
        match mode {
            1 => self.app_cursor = false,
            3 => self.deccolm(),
            6 => {
                self.origin_mode = false;
                self.move_cursor_to(0, 0);
            }
            7 => self.auto_wrap = false,
            25 => self.cursor_visible = false,
            47 | 1047 => {
                //                 if mode == 47 {
                //                     debug!("ANSI private mode 47 reset: exit alternate screen");
                //                 }
                self.exit_alt_buffer(false);
            }
            1048 => self.restore_cursor_state(),
            1049 => {
                //                 debug!("ANSI private mode 1049 reset: exit alternate screen and restore cursor");
                self.exit_alt_buffer(true);
            }
            2004 => self.bracketed_paste = false,
            9 | 1000 | 1002 | 1003 => self.mouse_mode = MouseMode::None,
            1005 | 1006 => self.mouse_encoding = MouseEncoding::Default,
            _ => {}
        }
    }

    pub(super) fn deccolm(&mut self) {
        self.top_margin = 0;
        self.bottom_margin = self.size.rows as usize - 1;
        self.move_cursor_to(0, 0);
        let rows = self.size.rows as usize;
        let cols = self.size.cols as usize;
        let grid = if self.using_alt_buffer {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        for row in grid.iter_mut().take(rows) {
            *row = vec![Cell::empty(); cols];
        }
        let wrapped = if self.using_alt_buffer {
            &mut self.alt_row_wrapped
        } else {
            &mut self.row_wrapped
        };
        wrapped.fill(false);
        self.mark_active_rows_changed(0, rows);
        self.dirty = true;
    }

    pub(super) fn enter_alt_buffer(&mut self, clear: bool) {
        if clear {
            self.alt_grid = create_grid(self.size.rows as usize, self.size.cols as usize);
            self.alt_row_wrapped = vec![false; self.size.rows as usize];
            self.alt_grid_revisions = self.fresh_line_revisions(self.size.rows as usize);
        }
        if !self.using_alt_buffer {
            // Kitty keyboard protocol: main and alternate screens keep
            // independent flag stacks; swap to the alternate screen's state.
            let active = (
                self.kitty_keyboard_flags,
                std::mem::take(&mut self.kitty_keyboard_stack),
            );
            let inactive = std::mem::replace(&mut self.inactive_kitty_keyboard, active);
            self.kitty_keyboard_flags = inactive.0;
            self.kitty_keyboard_stack = inactive.1;
        }
        self.using_alt_buffer = true;
        self.selection = None;
        self.dirty = true;
    }

    pub(super) fn exit_alt_buffer(&mut self, restore_cursor: bool) {
        if self.using_alt_buffer {
            // Swap the kitty keyboard state back to the main screen's.
            let active = (
                self.kitty_keyboard_flags,
                std::mem::take(&mut self.kitty_keyboard_stack),
            );
            let inactive = std::mem::replace(&mut self.inactive_kitty_keyboard, active);
            self.kitty_keyboard_flags = inactive.0;
            self.kitty_keyboard_stack = inactive.1;
        }
        self.using_alt_buffer = false;
        if restore_cursor {
            self.restore_cursor_state();
        }
        self.selection = None;
        self.dirty = true;
    }

    pub(super) fn dec_screen_alignment_test(&mut self) {
        let cell = Cell {
            character: 'E',
            fg_color: self.fg_color,
            bg_color: self.bg_color,
            attributes: self.attributes,
            hyperlink_id: self.active_hyperlink_id,
        };
        let grid = if self.using_alt_buffer {
            &mut self.alt_grid
        } else {
            &mut self.grid
        };
        for row in grid.iter_mut() {
            row.fill(cell);
        }
        self.mark_active_rows_changed(0, self.size.rows as usize);
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::Terminal;
    use crate::TerminalSize;

    fn terminal() -> Terminal {
        Terminal::new(TerminalSize::new(24, 80))
    }

    #[test]
    fn kitty_push_set_pop_flags() {
        let mut term = terminal();
        assert_eq!(term.kitty_keyboard_flags(), 0);

        term.process(b"\x1b[>1u");
        assert_eq!(term.kitty_keyboard_flags(), 1);

        // Mode 2: set the given bits, leave others unchanged.
        term.process(b"\x1b[=4;2u");
        assert_eq!(term.kitty_keyboard_flags(), 5);

        // Mode 3: reset the given bits.
        term.process(b"\x1b[=1;3u");
        assert_eq!(term.kitty_keyboard_flags(), 4);

        // Pop restores the pushed flags; a pop on an empty stack resets.
        term.process(b"\x1b[<u");
        assert_eq!(term.kitty_keyboard_flags(), 0);
        term.process(b"\x1b[>9u");
        term.process(b"\x1b[<2u");
        assert_eq!(term.kitty_keyboard_flags(), 0);
    }

    #[test]
    fn kitty_set_mode_one_replaces_flags() {
        let mut term = terminal();
        term.process(b"\x1b[=7u");
        assert_eq!(term.kitty_keyboard_flags(), 7);
        term.process(b"\x1b[=2;1u");
        assert_eq!(term.kitty_keyboard_flags(), 2);
    }

    #[test]
    fn kitty_query_reports_current_flags() {
        let mut term = terminal();
        term.process(b"\x1b[>11u");
        assert!(term.take_pending_query_responses().is_empty());
        term.process(b"\x1b[?u");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[?11u");
        assert!(term.take_pending_query_responses().is_empty());
    }

    #[test]
    fn kitty_stacks_are_independent_per_screen() {
        let mut term = terminal();
        term.process(b"\x1b[>1u");
        assert_eq!(term.kitty_keyboard_flags(), 1);

        // Enter the alternate screen: it starts with cleared flags/stack.
        term.process(b"\x1b[?1049h");
        assert_eq!(term.kitty_keyboard_flags(), 0);
        term.process(b"\x1b[>8u");
        assert_eq!(term.kitty_keyboard_flags(), 8);

        // Leaving the alternate screen restores the main screen's state.
        term.process(b"\x1b[?1049l");
        assert_eq!(term.kitty_keyboard_flags(), 1);

        // The alternate screen kept its own stack: re-enter and pop.
        term.process(b"\x1b[?1049h");
        assert_eq!(term.kitty_keyboard_flags(), 8);
        term.process(b"\x1b[<u");
        assert_eq!(term.kitty_keyboard_flags(), 0);
    }

    #[test]
    fn kitty_push_with_full_stack_evicts_oldest() {
        let mut term = terminal();
        for _ in 0..40 {
            term.process(b"\x1b[>1u");
        }
        // 40 pushes into a 32-deep stack: only the last 32 entries survive,
        // so 32 pops return to flags 0 without exhausting early.
        term.process(b"\x1b[=7u");
        for _ in 0..32 {
            term.process(b"\x1b[<u");
        }
        assert_eq!(term.kitty_keyboard_flags(), 1);
        term.process(b"\x1b[<u");
        assert_eq!(term.kitty_keyboard_flags(), 0);
    }

    #[test]
    fn kitty_state_does_not_leak_through_legacy_csi() {
        let mut term = terminal();
        // `CSI > 4 m` (kitty text sizing) must not become SGR underline.
        term.process(b"\x1b[>4mX");
        let cell = term.cell(0, 0).unwrap();
        assert_eq!(cell.character, 'X');
        assert!(!cell.attributes.contains(crate::CellAttributes::UNDERLINE));
    }

    #[test]
    fn primary_device_attributes_are_answered() {
        let mut term = terminal();
        assert!(term.take_pending_query_responses().is_empty());
        term.process(b"\x1b[c");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[?1;2c");
        // Kitty support probe: both replies arrive in query order.
        term.process(b"\x1b[?u\x1b[c");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[?0u\x1b[?1;2c");
    }

    #[test]
    fn secondary_device_attributes_are_answered() {
        let mut term = terminal();
        term.process(b"\x1b[>c");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[>0;1;0c");
    }

    #[test]
    fn device_status_reports_are_answered() {
        let mut term = terminal();
        term.process(b"\x1b[5n");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[0n");
        // CPR uses 1-based row;col of the current cursor position.
        term.process(b"\x1b[3;5H\x1b[6n");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[3;5R");
    }

    #[test]
    fn xtwinops_queries_are_answered() {
        let mut term = terminal();
        term.process(b"\x1b[18t");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[8;24;80t");
        term.process(b"\x1b[16t");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[6;20;10t");
        term.process(b"\x1b[14t");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[4;480;800t");
        term.process(b"\x1b[11t");
        assert_eq!(term.take_pending_query_responses(), b"\x1b[1t");
    }

    #[test]
    fn xtwinops_commands_are_consumed_silently() {
        let mut term = terminal();
        // A window manipulation command (or a stray host-terminal reply such
        // as `CSI 6;20;10t`) must neither reach the grid nor be answered.
        term.process(b"\x1b[6;20;10t");
        assert_eq!(term.cell(0, 0).unwrap().character, ' ');
        assert!(term.take_pending_query_responses().is_empty());
        // Resize to 20x10 via XTWINOPS is not honored either.
        assert_eq!(term.size().rows, 24);
    }

    #[test]
    fn ris_resets_kitty_keyboard_state() {
        let mut term = terminal();
        term.process(b"\x1b[>15u");
        assert_eq!(term.kitty_keyboard_flags(), 15);
        term.process(b"\x1bc");
        assert_eq!(term.kitty_keyboard_flags(), 0);
    }
}
