//! Link resolution for terminal cells.
//!
//! Two mechanisms locate a link under a cell position, mirroring Ghostty:
//!
//! 1. OSC 8 hyperlinks — applications emit `\x1b]8;;URI\x1b\\ ... \x1b]8;;\x1b\\`
//!    around link text; every printed cell records the link id and this
//!    module resolves the id back to the URI.
//! 2. Plain-text URL detection — a scanner over one line's cells finds
//!    URLs (http/https/ftp/mailto/...) with heuristics for trailing
//!    punctuation and unbalanced parentheses, like Ghostty's default
//!    `link-url` regex.

use std::collections::HashSet;
use std::sync::Arc;

use crate::Cell;

use super::Terminal;

/// A clickable link resolved at a cell position.
///
/// A link may span several rows when it was soft-wrapped; each row's
/// portion is one segment, ordered top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLink {
    /// Row segments of the link, top to bottom.
    pub segments: Vec<LinkSegment>,
    /// URI to open — the OSC 8 target URI, or the matched text for
    /// detected URLs.
    pub uri: Arc<str>,
}

/// One row's column range of a [`TerminalLink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkSegment {
    /// Absolute line index of this row.
    pub line: usize,
    /// First column of the segment (inclusive).
    pub start_col: usize,
    /// End column of the segment (exclusive).
    pub end_col: usize,
}

/// Registry size at which dead hyperlink URIs are garbage-collected.
/// GC scans all buffers once, so it triggers rarely relative to the
/// number of distinct links an application can emit.
const HYPERLINK_GC_THRESHOLD: usize = 1024;

impl Terminal {
    /// Resolve the link under a cell position, or `None` when the cell is
    /// not part of a link. OSC 8 hyperlinks win over detected URLs. Both
    /// follow soft-wrapped rows, so a URL that wrapped mid-line is found
    /// from any row it covers.
    pub fn link_at(&self, line_index: usize, col: usize) -> Option<TerminalLink> {
        let line = self.line_at(line_index)?;
        let cell = line.get(col)?;

        if cell.hyperlink_id != 0 {
            return self.osc8_link_at(line_index, col, cell.hyperlink_id);
        }

        // Wide continuation cells inherit the primary cell's hyperlink id;
        // for text detection step back so the scan starts on the primary.
        let col = if cell.is_wide_continuation() {
            col.saturating_sub(1)
        } else {
            col
        };

        // Stitch the logical line: contiguous rows joined by soft-wrap.
        let first_line = self.logical_line_start(line_index);
        let last_line = self.logical_line_end(line_index);

        // Compact all rows into (line, col, char), skipping wide
        // continuation cells so column math stays in cell coordinates.
        let cell_capacity = (first_line..=last_line)
            .map(|line| self.line_at(line).map_or(0, |cells| cells.len()))
            .sum();
        let mut compact: Vec<CompactChar> = Vec::with_capacity(cell_capacity);
        for li in first_line..=last_line {
            let line = self.line_at(li)?;
            for (cell_col, cell) in line.iter().enumerate() {
                if cell.is_wide_continuation() {
                    continue;
                }
                compact.push(CompactChar {
                    line: li,
                    col: cell_col,
                    ch: cell.character,
                });
            }
        }
        if compact.is_empty() {
            return None;
        }

        // Index of the compacted char that owns the hover column.
        let target = compact
            .iter()
            .rposition(|c| c.line == line_index && c.col <= col && col < c.col + c.width())
            .unwrap_or(0);
        let chars: Vec<char> = compact.iter().map(|c| c.ch).collect();

        let (start, end) = find_url_range(&chars, target)?;

        // Group the matched range into one segment per row.
        let mut segments: Vec<LinkSegment> = Vec::new();
        for c in &compact[start..end] {
            match segments.last_mut() {
                Some(segment) if segment.line == c.line => segment.end_col = c.col + c.width(),
                _ => segments.push(LinkSegment {
                    line: c.line,
                    start_col: c.col,
                    end_col: c.col + c.width(),
                }),
            }
        }

        let url: String = chars[start..end].iter().collect();
        Some(TerminalLink {
            segments,
            uri: Arc::from(url.as_str()),
        })
    }

    /// First row of the soft-wrapped logical line containing `line_index`.
    fn logical_line_start(&self, line_index: usize) -> usize {
        let mut first = line_index;
        loop {
            if first == 0 || !self.line_wrapped(first - 1) {
                break;
            }
            first -= 1;
        }
        first
    }

    /// Last row of the soft-wrapped logical line containing `line_index`.
    fn logical_line_end(&self, line_index: usize) -> usize {
        let mut last = line_index;
        loop {
            if !self.line_wrapped(last) || self.line_at(last + 1).is_none() {
                break;
            }
            last += 1;
        }
        last
    }

    /// Resolve the URI behind an OSC 8 hyperlink id, if still registered.
    pub fn hyperlink_uri(&self, id: u32) -> Option<Arc<str>> {
        self.hyperlink_uris.get(&id).cloned()
    }

    /// OSC 8 links are stored per cell, so a wrapped link simply continues
    /// on the next row's cells; extend the span across wrap boundaries.
    fn osc8_link_at(&self, line_index: usize, col: usize, id: u32) -> Option<TerminalLink> {
        let uri = self.hyperlink_uris.get(&id)?.clone();
        let line = self.line_at(line_index)?;
        let mut start_col = col;
        while start_col > 0 && line[start_col - 1].hyperlink_id == id {
            start_col -= 1;
        }
        let mut end_col = col + 1;
        while end_col < line.len() && line[end_col].hyperlink_id == id {
            end_col += 1;
        }
        let mut segments = vec![LinkSegment {
            line: line_index,
            start_col,
            end_col,
        }];

        // Extend downward across soft-wrapped rows.
        let mut last = line_index;
        loop {
            if !self.line_wrapped(last) {
                break;
            }
            let next = self.line_at(last + 1)?;
            let mut next_end = 0;
            while next_end < next.len() && next[next_end].hyperlink_id == id {
                next_end += 1;
            }
            if next_end == 0 {
                break;
            }
            segments.push(LinkSegment {
                line: last + 1,
                start_col: 0,
                end_col: next_end,
            });
            last += 1;
        }

        // Extend upward across soft-wrapped rows.
        let mut first = line_index;
        loop {
            if first == 0 || !self.line_wrapped(first - 1) {
                break;
            }
            let prev = self.line_at(first - 1)?;
            if prev.last().is_none_or(|cell| cell.hyperlink_id != id) {
                break;
            }
            let mut prev_start = prev.len();
            while prev_start > 0 && prev[prev_start - 1].hyperlink_id == id {
                prev_start -= 1;
            }
            if prev_start == prev.len() {
                break;
            }
            segments.insert(
                0,
                LinkSegment {
                    line: first - 1,
                    start_col: prev_start,
                    end_col: prev.len(),
                },
            );
            first -= 1;
        }

        Some(TerminalLink { segments, uri })
    }

    /// Intern a URI for OSC 8 hyperlink cells. Equal URIs share one id so a
    /// repeated link (e.g. a CI log full of the same URL) costs one entry.
    pub(super) fn intern_hyperlink(&mut self, uri: &str) -> u32 {
        if let Some((&id, _)) = self.hyperlink_uris.iter().find(|(_, u)| ***u == *uri) {
            return id;
        }

        // Garbage-collect dead ids before allocating past the threshold.
        // Cells own their ids, so liveness is simply "any cell still
        // carries it"; this avoids per-erase refcounting.
        if self.hyperlink_uris.len() >= HYPERLINK_GC_THRESHOLD {
            self.gc_hyperlinks();
            if let Some((&id, _)) = self.hyperlink_uris.iter().find(|(_, u)| ***u == *uri) {
                return id;
            }
        }

        let id = self.next_hyperlink_id;
        self.next_hyperlink_id = self.next_hyperlink_id.saturating_add(1);
        self.hyperlink_uris.insert(id, Arc::from(uri));
        id
    }

    fn gc_hyperlinks(&mut self) {
        let mut live: HashSet<u32> = HashSet::new();
        for row in self.grid.iter().chain(self.alt_grid.iter()) {
            live.extend(row.iter().map(|cell| cell.hyperlink_id));
        }
        for row in self.scrollback.iter() {
            live.extend(row.iter().map(|cell| cell.hyperlink_id));
        }
        live.remove(&0);
        self.hyperlink_uris.retain(|id, _| live.contains(id));
    }
}

/// URL schemes recognized by the text scanner. Mirrors Ghostty's default
/// link-url scheme set.
const URL_SCHEMES: &[&str] = &[
    "https://",
    "http://",
    "ftp://",
    "file://",
    "ssh://",
    "git://",
    "sftp://",
    "ipfs://",
    "ipns://",
    "gemini://",
    "gopher://",
    "mailto:",
    "tel:",
    "magnet:",
    "news:",
];

/// Characters allowed in the body of a matched URL. CJK prose and fullwidth
/// punctuation terminate the match so a URL embedded in Chinese/Japanese/Korean
/// text does not absorb the following sentence. Other Unicode URL characters
/// remain accepted, along with `[]` for IPv6 and balanced ASCII parentheses.
fn is_url_char(c: char) -> bool {
    if c.is_whitespace() || c.is_control() || is_cjk_prose(c) {
        return false;
    }
    !c.is_ascii() || c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&*+,;=%()".contains(c)
}

fn is_cjk_prose(c: char) -> bool {
    matches!(
        c as u32,
        0x3000..=0x303f
            | 0x3040..=0x30ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xac00..=0xd7af
            | 0xf900..=0xfaff
            | 0xff00..=0xffef
            | 0x20000..=0x2fa1f
    )
}

/// Trailing sentence punctuation stripped from a URL match. Terminal output
/// often embeds URLs in prose ("see https://x.com."), so punctuation that
/// is technically legal in a URL is excluded, like Ghostty's
/// `no_trailing_punctuation` rule.
fn is_trimmed_trailing_char(c: char) -> bool {
    matches!(c, '.' | ',' | ';' | ':' | '!' | '?')
}

/// A URL match within one terminal line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlMatch {
    /// First column of the match (inclusive).
    pub start_col: usize,
    /// End column of the match (exclusive), in cell columns.
    pub end_col: usize,
    /// Matched URL text.
    pub url: String,
}

/// A compacted grid char with its absolute position. Wide continuation
/// cells are skipped, so column math stays in cell coordinates.
struct CompactChar {
    line: usize,
    col: usize,
    ch: char,
}

impl CompactChar {
    /// Cell columns this char occupies (1 or 2 for wide chars).
    fn width(&self) -> usize {
        Cell::display_width_for_char(self.ch)
    }
}

/// Find the URL containing `col` within a single line of cells.
///
/// The line is scanned for scheme occurrences (case-insensitive); each
/// candidate extends to the first non-URL character and is then trimmed of
/// trailing sentence punctuation and closing parens that have no matching
/// open paren in the body — so `(https://example.com)` keeps its parens
/// out while `https://en.wikipedia.org/wiki/Rust_(video_game)` keeps them
/// in. Returns the match whose column range covers `col`.
pub fn find_url_at(cells: &[Cell], col: usize) -> Option<UrlMatch> {
    if cells.is_empty() || col >= cells.len() {
        return None;
    }

    let mut compact: Vec<CompactChar> = Vec::with_capacity(cells.len());
    for (cell_col, cell) in cells.iter().enumerate() {
        if !cell.is_wide_continuation() {
            compact.push(CompactChar {
                line: 0,
                col: cell_col,
                ch: cell.character,
            });
        }
    }

    // Index of the compacted char that owns the hover column: a wide char
    // covering [col, col+2) accepts either column.
    let target = compact
        .iter()
        .rposition(|c| c.col <= col && col < c.col + c.width())
        .unwrap_or(0);
    let chars: Vec<char> = compact.iter().map(|c| c.ch).collect();

    let (start, end) = find_url_range(&chars, target)?;

    let start_col = compact[start].col;
    let end_col = if end < compact.len() {
        compact[end].col
    } else {
        // Match runs to the line end; include the last char's full
        // display width so wide chars underline completely.
        let last = &compact[compact.len() - 1];
        last.col + last.width()
    };

    let url: String = chars[start..end].iter().collect();
    Some(UrlMatch {
        start_col,
        end_col,
        url,
    })
}

/// Scan a compacted char sequence for the URL whose range covers `target`.
/// Returns compacted `[start, end)` indices of the match.
fn find_url_range(chars: &[char], target: usize) -> Option<(usize, usize)> {
    for start in 0..chars.len() {
        let Some(scheme_len) = match_scheme_at(chars, start) else {
            continue;
        };

        // The scheme must start a token in ASCII terms: "xhttps://" is not
        // a URL, but "你好https://..." is — CJK text is not a scheme word.
        if start > 0 && chars[start - 1].is_ascii_alphanumeric() {
            continue;
        }

        let mut end = start + scheme_len;
        while end < chars.len() && is_url_char(chars[end]) {
            end += 1;
        }
        // Trim trailing sentence punctuation and unmatched closing parens.
        while end > start + scheme_len {
            let last = chars[end - 1];
            if is_trimmed_trailing_char(last) {
                end -= 1;
            } else if last == ')' && !has_unmatched_open_paren(&chars[start..end - 1]) {
                end -= 1;
            } else {
                break;
            }
        }

        // Require at least one body character beyond the scheme.
        if end <= start + scheme_len {
            continue;
        }

        if target < start || target >= end {
            continue;
        }

        return Some((start, end));
    }

    None
}

/// Case-insensitive scheme match at `start`; returns the scheme length.
fn match_scheme_at(chars: &[char], start: usize) -> Option<usize> {
    URL_SCHEMES.iter().copied().find_map(|scheme| {
        if chars.len() >= start + scheme.len()
            && chars[start..start + scheme.len()]
                .iter()
                .zip(scheme.chars())
                .all(|(c, s)| c.to_ascii_lowercase() == s)
        {
            Some(scheme.len())
        } else {
            None
        }
    })
}

/// True when the slice contains a `(` without a matching `)`, meaning a
/// trailing `)` closes it and must be kept.
fn has_unmatched_open_paren(chars: &[char]) -> bool {
    let mut depth: i32 = 0;
    for &c in chars {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellAttributes;
    use crate::TerminalSize;

    fn line(text: &str) -> Vec<Cell> {
        text.chars().map(Cell::new).collect()
    }

    /// Build cells for text where each CJK char occupies two columns.
    fn wide_line(text: &str) -> Vec<Cell> {
        let mut cells = Vec::new();
        for c in text.chars() {
            let width = Cell::display_width_for_char(c);
            cells.push(Cell::new(c));
            for _ in 1..width {
                let mut cont = Cell::new(' ');
                cont.attributes.insert(CellAttributes::WIDE_CONTINUATION);
                cells.push(cont);
            }
        }
        cells
    }

    fn assert_match(text: &str, col: usize, expected: &str) {
        let cells = line(text);
        let matched =
            find_url_at(&cells, col).unwrap_or_else(|| panic!("no url at col {col} in {text:?}"));
        assert_eq!(matched.url, expected, "in {text:?} at col {col}");
        assert!(
            matched.start_col <= col && col < matched.end_col,
            "match [{}, {}) does not cover col {col} in {text:?}",
            matched.start_col,
            matched.end_col
        );
    }

    fn assert_no_match(text: &str, col: usize) {
        let cells = line(text);
        assert!(
            find_url_at(&cells, col).is_none(),
            "unexpected match at col {col} in {text:?}"
        );
    }

    #[test]
    fn detects_http_url() {
        assert_match(
            "see https://example.com/x?y=1 now",
            5,
            "https://example.com/x?y=1",
        );
        assert_match(
            "see https://example.com/x?y=1 now",
            28,
            "https://example.com/x?y=1",
        );
        assert_no_match("see https://example.com/x?y=1 now", 3);
        assert_no_match("see https://example.com/x?y=1 now", 29);
    }

    #[test]
    fn detects_second_url_in_line() {
        let text = "a https://one.com b http://two.com c";
        assert_match(text, 10, "https://one.com");
        assert_match(text, 28, "http://two.com");
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_match("go HTTPS://Example.COM/Path", 6, "HTTPS://Example.COM/Path");
    }

    #[test]
    fn trims_sentence_punctuation() {
        assert_match("link https://example.com. done", 7, "https://example.com");
        assert_match("link https://example.com, done", 7, "https://example.com");
        assert_match("link https://example.com!: done", 7, "https://example.com");
    }

    #[test]
    fn keeps_balanced_parens_trims_unmatched() {
        // Wikipedia-style parenthesized path segment stays intact.
        assert_match(
            "(see https://en.wikipedia.org/wiki/Rust_(video_game))",
            8,
            "https://en.wikipedia.org/wiki/Rust_(video_game)",
        );
        // Wrapping prose parens are excluded.
        assert_match("(https://example.com)", 3, "https://example.com");
    }

    #[test]
    fn detects_other_schemes() {
        assert_match("contact mailto:a@b.com now", 10, "mailto:a@b.com");
        assert_match("clone git://host/repo.git ok", 8, "git://host/repo.git");
        assert_match("ssh ssh://user@host/repo ok", 4, "ssh://user@host/repo");
        assert_match("file file:///C:/temp/x.txt ok", 5, "file:///C:/temp/x.txt");
    }

    #[test]
    fn rejects_bare_scheme() {
        assert_no_match("just https:// here", 5);
        assert_no_match("just mailto: here", 5);
    }

    #[test]
    fn scheme_must_start_token() {
        assert_no_match("xhttps://example.com", 1);
    }

    #[test]
    fn url_with_port_and_ipv6() {
        assert_match("api https://[::1]:8080/v1 ok", 4, "https://[::1]:8080/v1");
    }

    #[test]
    fn wide_chars_count_columns() {
        let cells = wide_line("你好https://x.com");
        // 'h' of the scheme is at column 4.
        let matched = find_url_at(&cells, 4).unwrap();
        assert_eq!(matched.url, "https://x.com");
        assert_eq!(matched.start_col, 4);
        // Hovering the second half of a wide char before the URL must not
        // match (it belongs to 你, not the URL).
        assert!(find_url_at(&cells, 1).is_none());
    }

    #[test]
    fn url_stops_before_cjk_and_fullwidth_prose() {
        assert_match("http://localhost:5173）这种", 0, "http://localhost:5173");
        assert_match("（https://example.com）测试", 1, "https://example.com");

        let cells = wide_line("https://x.com/你好");
        let matched = find_url_at(&cells, 0).unwrap();
        assert_eq!(matched.url, "https://x.com/");
        assert_eq!(matched.end_col, 14);
        assert!(find_url_at(&cells, 14).is_none());
    }

    #[test]
    fn osc8_link_resolves_and_spans_cells() {
        let mut term = Terminal::new(TerminalSize::new(5, 40));
        term.process(b"before \x1b]8;;https://ghostty.org\x1b\\ghostty\x1b]8;;\x1b\\ after");

        let line = term.cursor_line_index();
        // "before " is 7 cols; the link covers "ghostty" (cols 7..14).
        let link = term.link_at(line, 9).expect("link under ghostty text");
        assert_eq!(link.uri.as_ref(), "https://ghostty.org");
        assert_eq!(
            link.segments,
            vec![LinkSegment {
                line,
                start_col: 7,
                end_col: 14
            }]
        );

        assert!(term.link_at(line, 2).is_none(), "plain text is not a link");
        assert!(
            term.link_at(line, 15).is_none(),
            "text after close is not a link"
        );
    }

    #[test]
    fn osc8_equal_uris_share_registry_entry() {
        let mut term = Terminal::new(TerminalSize::new(3, 40));
        term.process(b"\x1b]8;;https://a.dev\x1b\\a\x1b]8;;\x1b\\ \x1b]8;;https://a.dev\x1b\\b\x1b]8;;\x1b\\");
        assert_eq!(term.hyperlink_uris.len(), 1);
    }

    #[test]
    fn osc8_ends_on_empty_uri() {
        let mut term = Terminal::new(TerminalSize::new(3, 40));
        term.process(b"\x1b]8;;https://a.dev\x1b\\link\x1b]8;;\x1b\\tail");
        let line = term.cursor_line_index();
        // "link" (cols 0..4) carries the hyperlink, "tail" (cols 4..8) does not.
        assert!(term.link_at(line, 1).is_some());
        assert!(term.link_at(line, 5).is_none());
    }

    #[test]
    fn osc8_link_survives_scrollback() {
        let mut term = Terminal::new(TerminalSize::new(2, 40));
        term.process(b"\x1b]8;;https://scrolled.dev\x1b\\scrollme\x1b]8;;\x1b\\\r\n\r\n");
        // Row 0 scrolled into scrollback; the link moves with its cells.
        let link = term.link_at(0, 3).expect("link in scrollback");
        assert_eq!(link.uri.as_ref(), "https://scrolled.dev");
    }

    #[test]
    fn snapshot_preserves_osc8_links() {
        let mut term = Terminal::new(TerminalSize::new(3, 40));
        term.process(b"\x1b]8;;https://snap.dev\x1b\\clickme\x1b]8;;\x1b\\ tail");
        let snapshot = term.render_snapshot();

        let mut replayed = Terminal::new(TerminalSize::new(3, 40));
        replayed.process(&snapshot);

        let link = replayed
            .link_at(0, 2)
            .expect("link survives snapshot replay");
        assert_eq!(link.uri.as_ref(), "https://snap.dev");
        assert!(replayed.link_at(0, 10).is_none(), "tail stays unlinkable");
    }

    #[test]
    fn text_url_detection_via_link_at() {
        let mut term = Terminal::new(TerminalSize::new(3, 60));
        term.process(b"docs: https://docs.example.com/setup guide");
        let line = term.cursor_line_index();
        let link = term.link_at(line, 10).expect("url detected");
        assert_eq!(link.uri.as_ref(), "https://docs.example.com/setup");
        assert_eq!(link.segments[0].start_col, 6);
    }

    #[test]
    fn wrapped_text_url_spans_rows() {
        // 10-col rows: "see https://example.com/very/long/path" wraps
        // across all four rows: "see https:" / "//example." / "com/very/l"
        // / "ong/path".
        let mut term = Terminal::new(TerminalSize::new(4, 10));
        term.process(b"see https://example.com/very/long/path");

        let url = "https://example.com/very/long/path";
        // Hover a continuation row.
        let link = term.link_at(1, 0).expect("wrapped url detected from row 2");
        assert_eq!(link.uri.as_ref(), url);
        assert_eq!(link.segments.len(), 4);
        // Segment on row 0 starts at the scheme ("see " = 4 cols) and
        // covers the rest of that row.
        assert_eq!(link.segments[0].line, 0);
        assert_eq!(link.segments[0].start_col, 4);
        assert_eq!(link.segments[0].end_col, 10);
        // Continuation segments span each row fully.
        assert_eq!(link.segments[1].line, 1);
        assert_eq!(link.segments[1].start_col, 0);
        assert_eq!(link.segments[1].end_col, 10);
        assert_eq!(link.segments[3].line, 3);

        // Hovering the first row resolves the same multi-row link.
        let head = term.link_at(0, 8).expect("wrapped url detected from row 1");
        assert_eq!(head.uri.as_ref(), url);
        assert_eq!(head.segments.len(), 4);
    }

    #[test]
    fn wrapped_text_url_click_from_tail_matches_head() {
        let mut term = Terminal::new(TerminalSize::new(4, 10));
        term.process(b"see https://example.com/very/long/path");
        let head = term.link_at(0, 8).unwrap();
        let tail = term.link_at(1, 3).unwrap();
        assert_eq!(head.uri, tail.uri);
        // The same logical link shares at least one row.
        assert!(head.segments.iter().any(|s| tail.segments.contains(s)));
    }

    #[test]
    fn wrapped_text_url_is_not_truncated_after_64_rows() {
        let url = format!("https://example.com/{}", "a".repeat(400));
        let mut term = Terminal::new(TerminalSize::new(4, 4));
        term.process(url.as_bytes());

        let link = term.link_at(70, 0).expect("url beyond 64 wrapped rows");
        assert_eq!(link.uri.as_ref(), url);
        assert!(link.segments.len() > 100);
        assert_eq!(link.segments.first().map(|s| s.line), Some(0));
        assert!(link.segments.iter().any(|s| s.line == 70));
    }

    #[test]
    fn wrapped_url_stops_at_end_of_logical_line() {
        // 35 chars on 10 cols wrap across four rows, then \r\n starts a new
        // logical line. After the scroll the URL occupies absolute lines
        // 0..3 and "next line" sits on line 4.
        let mut term = Terminal::new(TerminalSize::new(4, 10));
        term.process(b"https://example.com/very/long/path.\r\nnext line");
        let link = term.link_at(1, 2).expect("wrapped url detected");
        assert_eq!(link.uri.as_ref(), "https://example.com/very/long/path");
        // The trailing '.' is trimmed and the link never crosses the
        // hard newline onto "next line".
        assert!(link.segments.iter().all(|s| s.line <= 3));
        assert!(!link.segments.iter().any(|s| s.line == 4));
    }

    #[test]
    fn wrapped_osc8_link_spans_rows() {
        let mut term = Terminal::new(TerminalSize::new(4, 10));
        term.process(
            b"\x1b]8;;https://wrapped.dev/very/long/target\x1b\\click here for more\x1b]8;;\x1b\\",
        );
        // The link text wraps across rows; hover the continuation row.
        let link = term.link_at(1, 2).expect("osc8 link on wrapped row");
        assert_eq!(link.uri.as_ref(), "https://wrapped.dev/very/long/target");
        assert!(link.segments.len() >= 2);
        assert_eq!(link.segments[0].line, 0);
        assert_eq!(link.segments[0].start_col, 0);
        assert!(link
            .segments
            .iter()
            .any(|s| s.line == 1 && s.start_col == 0));
    }

    #[test]
    fn registry_gc_drops_dead_links() {
        let mut term = Terminal::new(TerminalSize::new(3, 40));
        // Emit more than the GC threshold distinct links, each overwritten
        // by the next write, so only the final link stays alive. GC is
        // threshold-based, so a few recently-dead ids may linger until the
        // next collection — the registry just must stay bounded.
        for i in 0..(HYPERLINK_GC_THRESHOLD + 8) {
            let uri = format!("https://unique.example/{i}");
            let seq = format!("\x1b[1;1H\x1b]8;;{uri}\x1b\\x\x1b]8;;\x1b\\");
            term.process(seq.as_bytes());
        }
        assert!(
            term.hyperlink_uris.len() < HYPERLINK_GC_THRESHOLD,
            "registry holds {} entries past the threshold",
            term.hyperlink_uris.len()
        );
    }
}
