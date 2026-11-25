use std::collections::VecDeque;
use tracing::debug;
use crate::{Cell, TerminalSize};

use super::*;

/// Hard cap on scrollback PHYSICAL rows, as a multiple of the logical-line
/// limit. The logical trim below counts logical (unwrapped) lines, so a
/// stream with no newlines at all — `cat`ing a minified file, a code agent
/// emitting one giant line — forms a single logical line and bypasses it
/// entirely, growing the scrollback (and RSS) without bound. Physical rows
/// also bound the worst wrapped-row blowup at narrow widths.
pub(super) const MAX_SCROLLBACK_PHYSICAL_ROWS_FACTOR: usize = 4;

impl Terminal {
    pub fn resize(&mut self, new_size: TerminalSize) {
        let old_rows = self.size.rows as usize;
        let old_cols = self.size.cols as usize;
        let new_rows = new_size.rows as usize;
        let new_cols = new_size.cols as usize;

        if old_rows == new_rows && old_cols == new_cols {
            return;
        }

        // The eol-mark filter's wrap decision depends on the current width;
        // release any held candidate verbatim under the old width first.
        let held = self.eol_mark_filter.take_held();
        if !held.is_empty() {
            if let Some(mut parser) = self.parser.take() {
                parser.advance(self, &held);
                self.parser = Some(parser);
            }
        }

        resize_main_grid(
            &mut self.grid,
            &mut self.row_wrapped,
            &mut self.scrollback,
            &mut self.scrollback_wrapped,
            old_rows,
            old_cols,
            new_rows,
            new_cols,
            &mut self.cursor_pos.0,
            &mut self.cursor_pos.1,
            self.max_scrollback,
        );
        // The reflow rewrote the whole scrollback; rebuild the line counter.
        self.recount_scrollback_logical_lines();
        resize_grid(&mut self.alt_grid, old_rows, old_cols, new_rows, new_cols);

        self.alt_row_wrapped = vec![false; new_rows];
        self.refresh_all_line_revisions();

        // Update tab stops
        self.tab_stops.resize(new_cols, false);
        for i in (0..new_cols).step_by(8) {
            self.tab_stops[i] = true;
        }

        // Update margins
        self.bottom_margin = new_rows - 1;

        // Clamp cursor position
        self.cursor_pos.0 = self.cursor_pos.0.min(new_rows - 1);
        self.cursor_pos.1 = self.cursor_pos.1.min(new_cols - 1);
        self.pending_wrap = false;

        self.size = new_size;
        self.dirty = true;

//         debug!(
//             "[Emulator] resize: {}x{} -> {}x{}, scrollback_lines={}",
//             old_rows,
//             old_cols,
//             new_rows,
//             new_cols,
//             self.scrollback.len()
//         );
    }
}

pub(super) fn resize_grid(
    grid: &mut Vec<Vec<Cell>>,
    old_rows: usize,
    old_cols: usize,
    new_rows: usize,
    new_cols: usize,
) {
    // Resize rows
    if new_rows > old_rows {
        grid.extend((0..(new_rows - old_rows)).map(|_| vec![Cell::empty(); new_cols]));
    } else if new_rows < old_rows {
        grid.truncate(new_rows);
    }

    // Resize columns
    for row in grid.iter_mut() {
        if new_cols > old_cols {
            row.extend((0..(new_cols - old_cols)).map(|_| Cell::empty()));
        } else if new_cols < old_cols {
            row.truncate(new_cols);
        }
    }
}

pub(super) fn resize_main_grid(
    grid: &mut Vec<Vec<Cell>>,
    row_wrapped: &mut Vec<bool>,
    scrollback: &mut VecDeque<Vec<Cell>>,
    scrollback_wrapped: &mut VecDeque<bool>,
    _old_rows: usize,
    old_cols: usize,
    new_rows: usize,
    new_cols: usize,
    cursor_row: &mut usize,
    cursor_col: &mut usize,
    max_scrollback: usize,
) {
    let old_scrollback_len = scrollback.len();
    let old_cursor_abs_row = old_scrollback_len + *cursor_row;
    let old_cursor_col = *cursor_col;

    let mut rows = Vec::with_capacity(scrollback.len() + grid.len());
    let mut wrapped = Vec::with_capacity(scrollback_wrapped.len() + row_wrapped.len());
    rows.extend(scrollback.drain(..));
    rows.append(grid);
    wrapped.extend(scrollback_wrapped.drain(..));
    wrapped.append(row_wrapped);
    wrapped.resize(rows.len(), false);

    let (mut resized_rows, mut resized_wrapped, cursor_abs_row, resized_cursor_col) =
        reflow_visual_rows(
            rows,
            wrapped,
            old_cols,
            new_cols,
            old_cursor_abs_row,
            old_cursor_col,
        );

    if resized_rows.len() < new_rows {
        resized_rows
            .extend((0..(new_rows - resized_rows.len())).map(|_| vec![Cell::empty(); new_cols]));
        resized_wrapped.extend((0..(new_rows - resized_wrapped.len())).map(|_| false));
    }

    // When shrinking, choose which rows leave the visible grid:
    // 1. Drop blank rows below the cursor first (the usual shell case —
    //    prompts at the top, blank padding at the bottom): nothing scrolls
    //    into scrollback and the cursor stays put.
    // 2. Then push rows above the cursor into scrollback — but never past
    //    the cursor line, otherwise a shrink+regrow cycle loses the cursor
    //    row and the next prompt prints several rows lower (the "blank
    //    gap" artifact).
    // 3. Any remaining excess below the cursor is discarded, mirroring
    //    xterm's resize behavior.
    let need = resized_rows.len().saturating_sub(new_rows);
    let trailing_blanks = resized_rows
        .iter()
        .rev()
        .take(resized_rows.len().saturating_sub(cursor_abs_row + 1))
        .take_while(|row| occupied_len(row) == 0)
        .count();
    let drop_bottom = need.min(trailing_blanks);
    let split_index = (need - drop_bottom).min(cursor_abs_row);
    let visible_end = resized_rows.len() - (need - split_index);

    let mut new_grid: Vec<Vec<Cell>> = resized_rows.drain(split_index..visible_end).collect();
    let mut new_row_wrapped: Vec<bool> = resized_wrapped.drain(split_index..visible_end).collect();
    resized_rows.truncate(split_index);
    resized_wrapped.truncate(split_index);

    if new_grid.len() < new_rows {
        new_grid.extend((0..(new_rows - new_grid.len())).map(|_| vec![Cell::empty(); new_cols]));
        new_row_wrapped.extend((0..(new_rows - new_row_wrapped.len())).map(|_| false));
    }

    scrollback.extend(resized_rows);
    scrollback_wrapped.extend(resized_wrapped);
    truncate_scrollback_to_logical_limit(scrollback, scrollback_wrapped, max_scrollback);

    *cursor_row = cursor_abs_row.saturating_sub(split_index).min(new_rows - 1);
    *cursor_col = resized_cursor_col.min(new_cols - 1);
    *grid = new_grid;
    *row_wrapped = new_row_wrapped;
}

pub(super) fn reflow_visual_rows(
    rows: Vec<Vec<Cell>>,
    wrapped: Vec<bool>,
    old_cols: usize,
    new_cols: usize,
    cursor_abs_row: usize,
    cursor_col: usize,
) -> (Vec<Vec<Cell>>, Vec<bool>, usize, usize) {
    if rows.is_empty() {
        return (vec![vec![Cell::empty(); new_cols]], vec![false], 0, 0);
    }

    let mut output_rows = Vec::new();
    let mut output_wrapped = Vec::new();
    let mut cursor_result = None;
    let mut logical = Vec::new();
    let mut logical_start_output_row = 0;
    let mut cursor_offset_in_logical = None;

    for (row_index, row) in rows.into_iter().enumerate() {
        if logical.is_empty() {
            logical_start_output_row = output_rows.len();
        }

        let is_wrapped = wrapped.get(row_index).copied().unwrap_or(false);
        let row_len = if is_wrapped {
            old_cols.min(row.len())
        } else {
            occupied_len(&row)
        };

        if row_index == cursor_abs_row {
            cursor_offset_in_logical = Some(logical.len() + cursor_col.min(row_len));
        }

        logical.extend(row.into_iter().take(row_len));

        if !is_wrapped {
            flush_reflowed_logical_row(
                &mut output_rows,
                &mut output_wrapped,
                &mut cursor_result,
                logical_start_output_row,
                cursor_offset_in_logical.take(),
                &mut logical,
                new_cols,
            );
        }
    }

    if !logical.is_empty() || cursor_offset_in_logical.is_some() {
        flush_reflowed_logical_row(
            &mut output_rows,
            &mut output_wrapped,
            &mut cursor_result,
            logical_start_output_row,
            cursor_offset_in_logical,
            &mut logical,
            new_cols,
        );
    }

    if output_rows.is_empty() {
        output_rows.push(vec![Cell::empty(); new_cols]);
        output_wrapped.push(false);
    }

    let (cursor_row, cursor_col) =
        cursor_result.unwrap_or_else(|| (output_rows.len().saturating_sub(1), 0));
    (output_rows, output_wrapped, cursor_row, cursor_col)
}

pub(super) fn flush_reflowed_logical_row(
    output_rows: &mut Vec<Vec<Cell>>,
    output_wrapped: &mut Vec<bool>,
    cursor_result: &mut Option<(usize, usize)>,
    logical_start_output_row: usize,
    cursor_offset_in_logical: Option<usize>,
    logical: &mut Vec<Cell>,
    new_cols: usize,
) {
    let row_count = logical.len().max(1).div_ceil(new_cols);
    let first_output_row = output_rows.len();

    for row_index in 0..row_count {
        let start = row_index * new_cols;
        let end = ((row_index + 1) * new_cols).min(logical.len());
        let mut row = Vec::with_capacity(new_cols);
        if start < logical.len() {
            row.extend_from_slice(&logical[start..end]);
        }
        row.resize(new_cols, Cell::empty());
        fix_reflow_row_wide_boundary(&mut row);
        output_rows.push(row);
        output_wrapped.push(row_index + 1 < row_count);
    }

    if let Some(cursor_offset) = cursor_offset_in_logical {
        let cursor_row_delta = cursor_offset / new_cols;
        let cursor_col = cursor_offset % new_cols;
        *cursor_result = Some((
            logical_start_output_row.max(first_output_row) + cursor_row_delta.min(row_count - 1),
            cursor_col,
        ));
    }

    logical.clear();
}

/// A reflow split can cut a wide (CJK) cell across the row boundary. The row
/// interior is already consistent, so only the two boundary cells need
/// repair: an orphan continuation at the start or a dangling wide lead at
/// the end is cleared. This replaces a full-row normalize scan per row.
fn fix_reflow_row_wide_boundary(row: &mut [Cell]) {
    if row.first().is_some_and(|cell| cell.is_wide_continuation()) {
        row[0] = Cell::empty();
    }
    if row
        .last()
        .is_some_and(|cell| !cell.is_wide_continuation() && cell.display_width() >= 2)
    {
        let last = row.len() - 1;
        row[last] = Cell::empty();
    }
}

pub(super) fn truncate_scrollback_to_logical_limit(
    scrollback: &mut VecDeque<Vec<Cell>>,
    scrollback_wrapped: &mut VecDeque<bool>,
    max_scrollback: usize,
) {
    // Count once, then pop exactly the excess: the previous loop recounted
    // the whole buffer after every popped logical line (O(N) per line).
    let mut excess = scrollback_logical_line_count(scrollback, scrollback_wrapped)
        .saturating_sub(max_scrollback.max(1));
    while excess > 0 {
        while scrollback.pop_front().is_some() {
            let wrapped = scrollback_wrapped.pop_front().unwrap_or(false);
            if !wrapped {
                break;
            }
        }
        excess -= 1;
    }

    // Hard physical-row cap: a single logical line without newlines bypasses
    // the logical trim above entirely, so the buffer must also be bounded by
    // raw row count (see MAX_SCROLLBACK_PHYSICAL_ROWS_FACTOR).
    let physical_cap = (max_scrollback.max(1))
        .saturating_mul(MAX_SCROLLBACK_PHYSICAL_ROWS_FACTOR);
    if scrollback.len() > physical_cap {
        let overflow = scrollback.len() - physical_cap;
        for _ in 0..overflow {
            scrollback.pop_front();
            scrollback_wrapped.pop_front();
        }
        // The new head may begin mid-logical-line; make it a logical start
        // so the logical-line accounting above stays sound.
        if scrollback_wrapped.front() == Some(&true) {
            if let Some(front) = scrollback_wrapped.front_mut() {
                *front = false;
            }
        }
    }
}

fn scrollback_logical_line_count(
    scrollback: &VecDeque<Vec<Cell>>,
    scrollback_wrapped: &VecDeque<bool>,
) -> usize {
    if scrollback.is_empty() {
        return 0;
    }

    let mut lines = 0;
    let mut continues = false;
    for wrapped in scrollback_wrapped.iter().copied() {
        if !wrapped {
            lines += 1;
            continues = false;
        } else {
            continues = true;
        }
    }

    if continues {
        lines += 1;
    }
    lines
}
