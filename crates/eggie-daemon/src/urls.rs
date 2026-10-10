use super::*;

/// Scan the visible viewport for bare URLs and return them in viewport-relative coordinates. A URL
/// that soft-wraps across rows is split into one range per row so each row can be drawn and
/// hit-tested independently. The scan mirrors `collect_viewport_matches`: the viewport maps to grid
/// lines `[-display_offset, screen_lines - display_offset)`.

pub(super) fn detect_viewport_urls<T>(
    terminal: &Term<T>,
    regex: &mut RegexSearch,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Vec<TerminalLinkRange> {
    if screen_lines == 0 || columns == 0 {
        return Vec::new();
    }
    let top_line = Line(-(display_offset as i32));
    let bottom_line = Line(screen_lines as i32 - 1 - display_offset as i32);
    let start = Point::new(top_line, Column(0));
    let end = Point::new(bottom_line, terminal.grid().last_column());
    let mut links = Vec::new();
    for regex_match in RegexIter::new(start, end, Direction::Right, terminal, regex) {
        let Some((url, refined)) = refine_url_range(terminal, regex_match) else {
            continue;
        };
        // Split a multi-row (soft-wrapped) match into one range per visible row.
        let start = *refined.start();
        let end = *refined.end();
        for line in start.line.0..=end.line.0 {
            let row_start_col = if line == start.line.0 {
                start.column.0
            } else {
                0
            };
            let row_end_col = if line == end.line.0 {
                end.column.0
            } else {
                columns - 1
            };
            let range = Point::new(Line(line), Column(row_start_col))
                ..=Point::new(Line(line), Column(row_end_col));
            if let Some((vp_start, vp_end)) =
                clamp_range_to_viewport(range, display_offset, screen_lines, columns)
            {
                links.push(TerminalLinkRange {
                    start: vp_start,
                    end: vp_end,
                    url: url.clone(),
                });
            }
        }
    }
    links
}

/// Trim trailing punctuation and unbalanced closing brackets from a raw URL match. Handles the
/// common cases: a sentence-ending `.`/`,`, a wrapping `(url)`, while keeping balanced brackets like
/// `wiki_(foo)`. Returns the cleaned prefix (possibly empty).
pub(super) fn trim_url_trailing(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 0 {
        let ch = chars[keep - 1];
        let strip = match ch {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' | ']' | '}' => {
                let (open, close) = match ch {
                    ')' => ('(', ')'),
                    ']' => ('[', ']'),
                    _ => ('{', '}'),
                };
                let opens = chars[..keep].iter().filter(|&&c| c == open).count();
                let closes = chars[..keep].iter().filter(|&&c| c == close).count();
                closes > opens
            }
            _ => false,
        };
        if strip {
            keep -= 1;
        } else {
            break;
        }
    }
    chars[..keep].iter().collect()
}

/// Trim trailing punctuation from a raw regex match, returning the cleaned URL text and the grid
/// range that still covers it. Returns `None` if nothing usable remains.
pub(super) fn refine_url_range<T>(
    terminal: &Term<T>,
    regex_match: RangeInclusive<Point>,
) -> Option<(String, RangeInclusive<Point>)> {
    let start = *regex_match.start();
    let end = *regex_match.end();
    let text = terminal.bounds_to_string(start, end);
    let url = trim_url_trailing(&text);
    if url.is_empty() {
        return None;
    }
    // Shrink the grid end by the number of stripped chars. bounds_to_string emits one char per
    // (non-wide-spacer) cell on a line, and the trimmed punctuation set is all single-column ASCII,
    // so the column shift maps 1:1.
    let trimmed_cols = text.chars().count() - url.chars().count();
    let new_end = if trimmed_cols == 0 {
        end
    } else {
        let mut point = end;
        for _ in 0..trimmed_cols {
            point = point.sub(terminal, Boundary::Grid, 1);
        }
        point
    };
    Some((url, start..=new_end))
}
