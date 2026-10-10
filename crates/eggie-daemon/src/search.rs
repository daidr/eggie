use super::*;

/// Escape a literal search string so it can be compiled as a regex that matches it verbatim.

pub(super) fn regex_escape(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Count every match in the whole buffer and return `(total, index_of_active)`. `index` is the
/// 0-based ordinal of the match starting at `active_start`; it defaults to 0 if that match is not
/// found during the walk (which should not happen for a match the caller just located).
pub(super) fn count_all_matches<T>(
    terminal: &Term<T>,
    regex: &mut RegexSearch,
    active_start: Point,
) -> (usize, usize) {
    let start = Point::new(terminal.topmost_line(), Column(0));
    let end = Point::new(terminal.bottommost_line(), terminal.grid().last_column());
    let mut total = 0usize;
    let mut index = 0usize;
    for regex_match in RegexIter::new(start, end, Direction::Right, terminal, regex) {
        if *regex_match.start() == active_start {
            index = total;
        }
        total += 1;
    }
    (total, index)
}

/// The `display_offset` that brings the match starting at `active_start` fully on screen. If the
/// match is already visible the current offset is returned unchanged; otherwise the match line is
/// positioned near the top of the viewport.
pub(super) fn active_match_display_offset<T>(terminal: &Term<T>, active_start: Point) -> usize {
    let display_offset = terminal.grid().display_offset();
    let screen_lines = terminal.grid().screen_lines() as i32;
    let history_size = terminal.grid().history_size();
    let viewport_line = active_start.line.0 + display_offset as i32;
    if viewport_line >= 0 && viewport_line < screen_lines {
        return display_offset;
    }
    // Place the match line at the viewport top: viewport_line == 0 => offset == -line.
    let target = (-active_start.line.0).clamp(0, history_size as i32);
    target as usize
}

/// Convert a viewport cell position into a grid-absolute point using the current scroll offset.
/// Viewport row `r` maps to absolute line `r - display_offset`.
pub(super) fn viewport_point_to_absolute<T>(
    terminal: &Term<T>,
    point: TerminalCellPosition,
) -> Point {
    let display_offset = terminal.grid().display_offset() as i32;
    Point::new(
        Line(point.line as i32 - display_offset),
        Column(point.column as usize),
    )
}

/// Map a protocol selection side to the terminal core's `Side`.
pub(super) fn selection_side(side: TerminalSelectionSide) -> Side {
    match side {
        TerminalSelectionSide::Left => Side::Left,
        TerminalSelectionSide::Right => Side::Right,
    }
}

/// Project the terminal core's authoritative selection into the current viewport. Returns `None`
/// when there is no selection or it is entirely scrolled out of view.
pub(super) fn project_selection<T>(
    terminal: &Term<T>,
    display_offset: usize,
    size: TerminalSize,
) -> Option<TerminalSelectionRange> {
    let range = terminal.selection.as_ref()?.to_range(terminal)?;
    let is_block = range.is_block;
    let (start, end) = clamp_range_to_viewport(
        range.start..=range.end,
        display_offset,
        size.rows as usize,
        size.columns as usize,
    )?;
    Some(TerminalSelectionRange {
        start,
        end,
        is_block,
    })
}

/// Clamp a grid-absolute inclusive point range into the visible viewport rows, returning the
/// viewport-relative endpoints. Returns `None` if the range is entirely off screen. A clamped start
/// snaps to column 0 of the top row; a clamped end snaps to the last column of the bottom row, so a
/// highlight rectangle covers the visible portion. Shared by search-match and selection projection.
pub(super) fn clamp_range_to_viewport(
    range: RangeInclusive<Point>,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Option<(TerminalCellPosition, TerminalCellPosition)> {
    if screen_lines == 0 || columns == 0 {
        return None;
    }
    let last_line = (screen_lines - 1) as i32;
    let last_column = (columns - 1) as u16;
    let start = *range.start();
    let end = *range.end();
    let start_line = start.line.0 + display_offset as i32;
    let end_line = end.line.0 + display_offset as i32;
    // Reject a range whose start is below the viewport or whose end is above it (fully off screen).
    if start_line > last_line || end_line < 0 {
        return None;
    }
    let (start_line, start_column) = if start_line < 0 {
        (0u16, 0u16)
    } else {
        (start_line as u16, start.column.0 as u16)
    };
    let (end_line, end_column) = if end_line > last_line {
        (last_line as u16, last_column)
    } else {
        (end_line as u16, end.column.0 as u16)
    };
    Some((
        TerminalCellPosition {
            line: start_line,
            column: start_column.min(last_column),
        },
        TerminalCellPosition {
            line: end_line,
            column: end_column.min(last_column),
        },
    ))
}

/// Convert a grid-absolute match into a viewport-relative highlight, clamping endpoints to the
/// visible rows. Returns `None` if the match start is below the viewport or the whole match is
/// above it. `columns` is the grid width, used to extend a clamped end to the row edge.
pub(super) fn viewport_match_inner(
    regex_match: RangeInclusive<Point>,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Option<TerminalSearchMatch> {
    let (start, end) = clamp_range_to_viewport(regex_match, display_offset, screen_lines, columns)?;
    Some(TerminalSearchMatch { start, end })
}

/// Collect all matches whose start falls within the current viewport rows, in viewport coordinates.
pub(super) fn collect_viewport_matches<T>(
    terminal: &Term<T>,
    regex: &mut RegexSearch,
    display_offset: usize,
    screen_lines: usize,
) -> Vec<TerminalSearchMatch> {
    // Viewport rows map to grid lines [-display_offset, -display_offset + screen_lines).
    let top_line = Line(-(display_offset as i32));
    let bottom_line = Line(screen_lines as i32 - 1 - display_offset as i32);
    let start = Point::new(top_line, Column(0));
    let end = Point::new(bottom_line, terminal.grid().last_column());
    let columns = terminal.grid().columns();
    RegexIter::new(start, end, Direction::Right, terminal, regex)
        .filter_map(|regex_match| {
            viewport_match_inner(regex_match, display_offset, screen_lines, columns)
        })
        .collect()
}
