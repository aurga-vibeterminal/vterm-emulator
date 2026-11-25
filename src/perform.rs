use crate::Cell;
use tracing::{debug, trace};
use vte::{Params, Perform};

use super::*;

/// Rebuild the OSC payload after the numeric code so callbacks receive the full tail.
fn serialize_osc_payload_tail(params: &[&[u8]]) -> Vec<u8> {
    let mut payload = Vec::new();
    for (index, part) in params.iter().skip(1).enumerate() {
        if index > 0 {
            payload.push(b';');
        }
        payload.extend_from_slice(part);
    }
    payload
}

impl Perform for Terminal {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {
                // BEL - Bell
                trace!("Bell");
            }
            0x08 => self.backspace(),
            0x09 => self.tab(),
            0x0A | 0x0B | 0x0C => self.linefeed(),
            0x0D => self.carriage_return(),
            0x0E => {
                self.active_charset = ActiveCharset::G1;
            }
            0x0F => {
                self.active_charset = ActiveCharset::G0;
            }
            _ => {
                trace!("Unhandled execute byte: {:02X}", byte);
            }
        }
    }

    fn hook(&mut self, params: &Params, intermediates: &[u8], ignore: bool, c: char) {
        // debug!(
        //     "hook: params={:?}, intermediates={:?}, ignore={}, c={}",
        //     params, intermediates, ignore, c
        // );
        self.active_dcs = Some(ActiveDcs {
            params: params.iter().map(|param| param.to_vec()).collect(),
            intermediates: intermediates.to_vec(),
            ignore,
            final_char: c,
            data: Vec::new(),
        });
    }

    fn put(&mut self, byte: u8) {
        trace!("put: byte={:02X}", byte);
        if let Some(active_dcs) = self.active_dcs.as_mut() {
            active_dcs.data.push(byte);
        }
    }

    fn unhook(&mut self) {
        debug!("unhook");
        if let Some(active_dcs) = self.active_dcs.take() {
            self.emit_sequence("dcs", active_dcs.final_char.to_string(), active_dcs.data);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // debug!(
        //     "osc_dispatch: params={:?}, bell_terminated={}",
        //     params, bell_terminated
        // );

        self.emit_sequence(
            "osc",
            params
                .first()
                .map(|p| String::from_utf8_lossy(p).to_string())
                .unwrap_or_default(),
            serialize_osc_payload_tail(params),
        );

        if params.is_empty() {
            return;
        }

        match params[0] {
            b"0" | b"2" => {
                // Set window title
                if params.len() > 1 {
                    if let Ok(title) = std::str::from_utf8(params[1]) {
                        self.title = title.to_string();
                        // debug!("Window title set to: {}", title);
                    }
                }
            }
            b"7" => {
                // Set working directory (OSC 7: file://...)
                if params.len() > 1 {
                    if let Ok(uri) = std::str::from_utf8(params[1]) {
                        if let Some(cwd) = parse_osc7_working_directory(uri) {
                            if self.cwd.as_deref() != Some(cwd.as_str()) {
                                self.cwd = Some(cwd);
                                self.cwd_dirty = true;
                            }
                        }
                    }
                }
            }
            b"8" => {
                // OSC 8 hyperlink: `ESC ] 8 ; params ; URI ST`. An empty URI
                // closes the current hyperlink. URIs may legally contain ';'
                // (e.g. query separators), which vte splits into extra params,
                // so rejoin everything after the params field.
                let uri = params.get(2..).map(|parts| parts.join(&b";"[..]));
                match uri.as_deref().and_then(|uri| std::str::from_utf8(uri).ok()) {
                    Some(uri) if !uri.is_empty() && !uri.contains(char::is_control) => {
                        self.active_hyperlink_id = self.intern_hyperlink(uri);
                    }
                    _ => {
                        // Empty or malformed URI, or link close: stop tagging.
                        self.active_hyperlink_id = 0;
                    }
                }
            }
            b"133" | b"633" => {
                //log_shell_integration_marker(params);
            }
            b"10" | b"11" => {
                // Set or query the default foreground (10) / background (11)
                // color. TUIs like OpenCode probe `11;?` at startup to pick a
                // light or dark theme; see `handle_default_color_osc`.
                self.handle_default_color_osc(params[0], params.get(1).copied());
            }
            b"110" | b"111" => {
                // xterm extension: reset the default foreground (110) or
                // background (111) color back to the host's configured color.
                self.handle_default_color_osc(params[0], None);
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, c: char) {
        self.emit_sequence(
            "csi",
            c.to_string(),
            params
                .iter()
                .flat_map(|param| param.iter().flat_map(|&v| v.to_le_bytes()))
                .collect(),
        );

        // debug!(
        //     "csi_dispatch: params={:?}, intermediates={:?}, c={}",
        //     params, intermediates, c
        // );

        if ignore {
            return;
        }

        // Helper to get the first parameter value with a default
        let get_param = |idx: usize, default: u16| -> u16 {
            let value = params
                .iter()
                .nth(idx)
                .and_then(|p| p.first().copied())
                .unwrap_or(default);
            if value == 0 && default != 0 {
                default
            } else {
                value
            }
        };

        if !intermediates.is_empty() {
            // Private/marked CSI sequences (e.g. `?`, `>`, `<`, `=`, `!`, ` `)
            // have different semantics from their bare-letter counterparts
            // and must not fall through to the main match below. Otherwise
            // things like kitty's `CSI > 4 m` (keyboard progressive
            // enhancement) get misinterpreted as `CSI 4 m` (SGR underline).
            if intermediates == [b'?'] {
                self.dispatch_private_csi(params, c);
            } else if intermediates == [b'>'] && c == 'c' {
                // Secondary Device Attributes. Answer so probes get a reply
                // instead of timing out; claims a VT100-class base terminal.
                self.queue_query_response(b"\x1b[>0;1;0c");
            } else if c == 'u' {
                // Kitty keyboard protocol progressive enhancement.
                let param = |index: usize, default: u16| -> u16 {
                    params
                        .iter()
                        .nth(index)
                        .and_then(|p| p.first().copied())
                        .filter(|v| *v != 0)
                        .unwrap_or(default)
                };
                match intermediates {
                    [b'>'] => self.kitty_keyboard_push(param(0, 0)),
                    [b'<'] => self.kitty_keyboard_pop(param(0, 1)),
                    [b'='] => self.kitty_keyboard_set(param(0, 0), param(1, 1)),
                    _ => {}
                }
            }
            return;
        }

        match c {
            '@' => {
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let col = self.cursor_pos.1;
                let blank = self.blank_cell();
                let grid = if self.using_alt_buffer {
                    &mut self.alt_grid
                } else {
                    &mut self.grid
                };

                insert_blank_cells_with(&mut grid[row], col, n, blank);
                self.mark_active_row_changed(row);
                self.dirty = true;
            }
            'A' => {
                // CUU - Cursor Up
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let min_row = if self.origin_mode { self.top_margin } else { 0 };
                self.cursor_pos.0 = row.saturating_sub(n).max(min_row);
                self.pending_wrap = false;
            }
            'B' => {
                // CUD - Cursor Down
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let max_row = if self.origin_mode {
                    self.bottom_margin
                } else {
                    self.size.rows as usize - 1
                };
                self.cursor_pos.0 = (row + n).min(max_row);
                self.pending_wrap = false;
            }
            'b' => {
                // REP - Repeat preceding graphic character.
                let n = get_param(0, 1) as usize;
                let c = self.last_printed_char;
                for _ in 0..n {
                    self.put_char(c);
                }
            }
            'C' => {
                // CUF - Cursor Forward
                let n = get_param(0, 1) as usize;
                let col = self.cursor_pos.1;
                self.cursor_pos.1 = (col + n).min(self.size.cols as usize - 1);
                self.pending_wrap = false;
            }
            'D' => {
                // CUB - Cursor Back
                let n = get_param(0, 1) as usize;
                let col = self.cursor_pos.1;
                self.cursor_pos.1 = col.saturating_sub(n);
                self.pending_wrap = false;
            }
            'E' => {
                // CNL - Cursor Next Line
                let n = get_param(0, 1) as usize;
                let max_row = if self.origin_mode {
                    self.bottom_margin
                } else {
                    self.size.rows as usize - 1
                };
                self.cursor_pos.0 = (self.cursor_pos.0 + n).min(max_row);
                self.cursor_pos.1 = 0;
                self.pending_wrap = false;
            }
            'F' => {
                // CPL - Cursor Previous Line
                let n = get_param(0, 1) as usize;
                let min_row = if self.origin_mode { self.top_margin } else { 0 };
                self.cursor_pos.0 = self.cursor_pos.0.saturating_sub(n).max(min_row);
                self.cursor_pos.1 = 0;
                self.pending_wrap = false;
            }
            'G' => {
                // CHA - Cursor Horizontal Absolute
                let col = get_param(0, 1) as usize;
                self.cursor_pos.1 = (col.saturating_sub(1)).min(self.size.cols as usize - 1);
                self.pending_wrap = false;
            }
            '`' => {
                // HPA - Horizontal Position Absolute
                let col = get_param(0, 1) as usize;
                self.cursor_pos.1 = (col.saturating_sub(1)).min(self.size.cols as usize - 1);
                self.pending_wrap = false;
            }
            'H' | 'f' => {
                // CUP - Cursor Position / HVP
                let row = get_param(0, 1) as usize;
                let col = get_param(1, 1) as usize;
                self.move_cursor_to(row.saturating_sub(1), col.saturating_sub(1));
            }
            'I' => {
                // CHT - Cursor Forward Tabulation
                let n = get_param(0, 1);
                for _ in 0..n {
                    self.move_forward_tab();
                }
            }
            'J' => {
                // ED - Erase in Display
                let mode = get_param(0, 0);
                self.erase_in_display(mode);
            }
            'K' => {
                // EL - Erase in Line
                let mode = get_param(0, 0);
                self.erase_in_line(mode);
            }
            'L' => {
                // IL - Insert Line
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let bottom_margin = self.bottom_margin;
                let cols = self.size.cols as usize;
                let using_alt = self.using_alt_buffer;

                for _ in 0..n {
                    let blank_revision = self.fresh_line_revision();
                    // Save bottom line to scrollback if needed
                    if !using_alt && bottom_margin == self.size.rows as usize - 1 {
                        let bottom_line = self.grid[bottom_margin].clone();
                        let bottom_revision = self.grid_revisions[bottom_margin];
                        let bottom_wrapped = self
                            .row_wrapped
                            .get(bottom_margin)
                            .copied()
                            .unwrap_or(false);
                        self.push_scrollback_row_with_revision(
                            bottom_line,
                            bottom_wrapped,
                            bottom_revision,
                        );
                    }

                    // Shift lines down
                    let grid = if using_alt {
                        &mut self.alt_grid
                    } else {
                        &mut self.grid
                    };

                    for r in (row..bottom_margin).rev() {
                        grid[r + 1] = grid[r].clone();
                    }

                    // Clear inserted line
                    grid[row] = vec![Cell::empty(); cols];

                    let revisions = if using_alt {
                        &mut self.alt_grid_revisions
                    } else {
                        &mut self.grid_revisions
                    };
                    for r in (row..bottom_margin).rev() {
                        revisions[r + 1] = revisions[r];
                    }
                    revisions[row] = blank_revision;

                    let wrapped = if using_alt {
                        &mut self.alt_row_wrapped
                    } else {
                        &mut self.row_wrapped
                    };
                    for r in (row..bottom_margin).rev() {
                        wrapped[r + 1] = wrapped[r];
                    }
                    wrapped[row] = false;
                }

                self.dirty = true;
            }
            'M' => {
                // DL - Delete Line
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let bottom_margin = self.bottom_margin;
                let cols = self.size.cols as usize;
                let using_alt = self.using_alt_buffer;

                for _ in 0..n {
                    let blank_revision = self.fresh_line_revision();
                    let grid = if using_alt {
                        &mut self.alt_grid
                    } else {
                        &mut self.grid
                    };

                    // Shift lines up
                    for r in row..bottom_margin {
                        grid[r] = grid[r + 1].clone();
                    }

                    // Clear bottom line
                    grid[bottom_margin] = vec![Cell::empty(); cols];

                    let revisions = if using_alt {
                        &mut self.alt_grid_revisions
                    } else {
                        &mut self.grid_revisions
                    };
                    for r in row..bottom_margin {
                        revisions[r] = revisions[r + 1];
                    }
                    revisions[bottom_margin] = blank_revision;

                    let wrapped = if using_alt {
                        &mut self.alt_row_wrapped
                    } else {
                        &mut self.row_wrapped
                    };
                    for r in row..bottom_margin {
                        wrapped[r] = wrapped[r + 1];
                    }
                    wrapped[bottom_margin] = false;
                }

                self.dirty = true;
            }
            'P' => {
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let col = self.cursor_pos.1;
                let blank = self.blank_cell();
                let grid = if self.using_alt_buffer {
                    &mut self.alt_grid
                } else {
                    &mut self.grid
                };

                delete_cells_with(&mut grid[row], col, n, blank);
                self.mark_active_row_changed(row);

                self.dirty = true;
            }
            'S' => {
                // SU - Scroll Up
                let n = get_param(0, 1) as usize;
                self.scroll_up(n);
            }
            'T' => {
                // SD - Scroll Down
                let n = get_param(0, 1) as usize;
                self.scroll_down(n);
            }
            'X' => {
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let col = self.cursor_pos.1;
                let cols = self.size.cols as usize;
                let blank = self.blank_cell();
                let grid = if self.using_alt_buffer {
                    &mut self.alt_grid
                } else {
                    &mut self.grid
                };

                clear_cell_range_with(&mut grid[row], col, (col + n).min(cols), blank);
                self.mark_active_row_changed(row);

                self.dirty = true;
            }
            'Z' => {
                // CBT - Cursor Backward Tabulation
                let n = get_param(0, 1);
                for _ in 0..n {
                    self.move_backward_tab();
                }
            }
            'a' => {
                // HPR - Horizontal Position Relative
                let n = get_param(0, 1) as usize;
                let col = self.cursor_pos.1;
                self.cursor_pos.1 = (col + n).min(self.size.cols as usize - 1);
                self.pending_wrap = false;
            }
            'd' => {
                // VPA - Line Position Absolute
                let row = get_param(0, 1) as usize;
                self.move_cursor_to(row.saturating_sub(1), self.cursor_pos.1);
            }
            'e' => {
                // VPR - Vertical Position Relative
                let n = get_param(0, 1) as usize;
                let row = self.cursor_pos.0;
                let max_row = if self.origin_mode {
                    self.bottom_margin
                } else {
                    self.size.rows as usize - 1
                };
                self.cursor_pos.0 = (row + n).min(max_row);
                self.pending_wrap = false;
            }
            'm' => {
                // SGR - Select Graphic Rendition
                self.set_sgr(params);
            }
            'g' => match get_param(0, 0) {
                0 => {
                    if let Some(tab_stop) = self.tab_stops.get_mut(self.cursor_pos.1) {
                        *tab_stop = false;
                    }
                }
                3 => self.tab_stops.fill(false),
                _ => {}
            },
            'h' => {
                self.set_mode(params);
            }
            'l' => {
                self.reset_mode(params);
            }
            'n' => {
                // DSR - Device Status Report. Applications block on these
                // replies, so they must be answered.
                match get_param(0, 0) {
                    // Terminal status: OK.
                    5 => self.queue_query_response(b"\x1b[0n"),
                    // Cursor position report (1-based row;col).
                    6 => {
                        let row = self.cursor_pos.0 + 1;
                        let col = self.cursor_pos.1 + 1;
                        self.queue_query_response(format!("\x1b[{row};{col}R").as_bytes());
                    }
                    _ => {}
                }
            }
            'c' => {
                // DA - Primary Device Attributes. Answer as a VT102-class
                // terminal (xterm's default reply): applications probe kitty
                // keyboard support by sending `CSI ? u` followed by DA and
                // require BOTH replies, so leaving DA unanswered makes that
                // detection time out and silently disables the protocol.
                self.queue_query_response(b"\x1b[?1;2c");
            }
            'r' => {
                // DECSTBM - Set Scrolling Region
                let top = get_param(0, 1) as usize;
                let bottom = get_param(1, self.size.rows as u16) as usize;

                self.top_margin = top.saturating_sub(1);
                self.bottom_margin = (bottom - 1).min(self.size.rows as usize - 1);

                // Move cursor to home position
                self.move_cursor_to(0, 0);
            }
            's' => {
                // Save cursor position
                self.save_cursor_state();
            }
            'u' => {
                // SCORC - Restore cursor position.
                self.restore_cursor_state();
            }
            't' => {
                // XTWINOPS - Window manipulation (queries are answered,
                // window move/resize commands do not apply and are ignored).
                self.handle_window_manipulation(params);
            }
            _ => {
                trace!("Unhandled CSI sequence: c={}", c);
            }
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        self.emit_sequence("esc", format!("{byte:02X}"), Vec::new());

        // debug!(
        //     "esc_dispatch: intermediates={:?}, byte={:02X}",
        //     intermediates, byte
        // );

        if ignore {
            return;
        }

        match (intermediates, byte) {
            (&[b'('], charset) => {
                self.g0_charset = self.decode_charset(charset);
            }
            (&[b')'], charset) => {
                self.g1_charset = self.decode_charset(charset);
            }
            (&[b'#'], b'8') if self.terminal_type.supports_dec_private_modes() => {
                self.dec_screen_alignment_test();
            }
            (_, b'7') => {
                self.save_cursor_state();
            }
            (_, b'8') => {
                self.restore_cursor_state();
            }
            (_, b'=') => {
                self.app_keypad = true;
            }
            (_, b'>') => {
                self.app_keypad = false;
            }
            (_, b'H') => {
                if let Some(tab_stop) = self.tab_stops.get_mut(self.cursor_pos.1) {
                    *tab_stop = true;
                }
            }
            (_, b'D') => {
                self.linefeed();
            }
            (_, b'E') => {
                self.linefeed();
                self.carriage_return();
            }
            (_, b'M') => {
                let row = self.cursor_pos.0;

                if row == self.top_margin {
                    self.scroll_down(1);
                } else {
                    self.cursor_pos.0 = row.saturating_sub(1);
                    self.pending_wrap = false;
                }
            }
            (_, b'c') => {
                let sequence_callback = self.sequence_callback.take();
                let sequence_callback_types = std::mem::take(&mut self.sequence_callback_types);
                let next_line_revision = self.next_line_revision;
                let theme_default_fg = self.theme_default_fg;
                let theme_default_bg = self.theme_default_bg;
                let query_responder = self.query_responder;
                *self = Terminal::new_with_type(self.size, Some(self.terminal_type.as_str()));
                self.next_line_revision = next_line_revision;
                // RIS drops the app's OSC 10/11 default-color overrides (the
                // fresh instance starts without them) but the host UI's
                // configured colors and responder role survive.
                self.theme_default_fg = theme_default_fg;
                self.theme_default_bg = theme_default_bg;
                self.query_responder = query_responder;
                self.refresh_all_line_revisions();
                self.sequence_callback = sequence_callback;
                self.sequence_callback_types = sequence_callback_types;
            }
            _ => {
                trace!(
                    "Unhandled ESC sequence: intermediates={:?}, byte={:02X}",
                    intermediates,
                    byte
                );
            }
        }
    }
}
