use super::*;

const MAX_TEXT_BYTES: usize = 1024 * 1024;

fn read_range(
    terminal: &Term<DaemonEventListener>,
    start: Point,
    end: Point,
    limit: usize,
) -> Result<(String, bool)> {
    if end < start {
        return Ok((String::new(), false));
    }
    anyhow::ensure!(
        start.line >= terminal.topmost_line() && end.line <= terminal.bottommost_line(),
        "Output is no longer in scrollback"
    );
    let limit = limit.clamp(1, MAX_TEXT_BYTES);
    let mut text = String::new();
    for line in start.line.0..=end.line.0 {
        let line = Line(line);
        let first = if line == start.line {
            start.column
        } else {
            Column(0)
        };
        let last = if line == end.line {
            end.column
        } else {
            terminal.grid().last_column()
        };
        let mut part = terminal.bounds_to_string(Point::new(line, first), Point::new(line, last));
        if line < end.line
            && !terminal.grid()[line][terminal.grid().last_column()]
                .flags
                .contains(Flags::WRAPLINE)
        {
            part.push('\n');
        }
        let remaining = limit.saturating_sub(text.len());
        if part.len() > remaining {
            let mut cut = remaining;
            while !part.is_char_boundary(cut) {
                cut -= 1;
            }
            text.push_str(&part[..cut]);
            return Ok((text, true));
        }
        text.push_str(&part);
    }
    Ok((text, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi;

    fn fixture(
        columns: u16,
        rows: u16,
        history: usize,
    ) -> (Arc<ListenerState>, Term<DaemonEventListener>) {
        let size = TerminalSize {
            columns,
            rows,
            ..Default::default()
        };
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
            None,
        ));
        let term = Term::new(
            Config {
                scrolling_history: history,
                ..Default::default()
            },
            &GridSize(size),
            DaemonEventListener(state.clone()),
        );
        (state, term)
    }

    fn output(state: &ListenerState, term: &Term<DaemonEventListener>) -> String {
        let mut shell = state.shell_integration.lock();
        shell.follow_grid_scroll(term.grid().scroll_position());
        let range = shell.command_output_ranges.back().unwrap();
        read_range(
            term,
            range.start.unwrap(),
            range.end.unwrap(),
            MAX_TEXT_BYTES,
        )
        .unwrap()
        .0
    }

    #[test]
    fn output_anchors_survive_width_height_reflow_and_scrolling() {
        let (state, mut term) = fixture(40, 6, 100);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(
            &mut term,
            b"\x1b]133;A\x07> \x1b]133;B\x07echo test\r\n\x1b]133;C\x07",
        );
        parser.advance(
            &mut term,
            "abc中文defghijklmnopqrstuvwxyz0123456789\r\nsecond\r\n".as_bytes(),
        );
        parser.advance(&mut term, b"\x1b]133;D;0\x07\x1b]133;A\x07> \x1b]133;B\x07");
        let expected = output(&state, &term);
        assert_eq!(expected, "abc中文defghijklmnopqrstuvwxyz0123456789\nsecond");
        for (columns, rows) in [(8, 6), (21, 4), (21, 9), (80, 9), (12, 5), (40, 6)] {
            state.resize_terminal(
                &mut term,
                TerminalSize {
                    columns,
                    rows,
                    ..Default::default()
                },
            );
            assert_eq!(output(&state, &term), expected, "{columns}x{rows}");
            parser.advance(&mut term, b"\r\x1b[2K\x1b]133;A\x07> \x1b]133;B\x07");
        }
    }

    #[test]
    fn empty_output_does_not_pin_cleared_prompt_rows() {
        let (state, mut term) = fixture(60, 8, 100);
        let (control, mut baseline) = fixture(60, 8, 100);
        let mut parser: ansi::Processor = ansi::Processor::new();
        let mut baseline_parser: ansi::Processor = ansi::Processor::new();
        let seed = b"\x1b]133;A\x07> \x1b]133;B\x07true\r\n\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07LONG_PROMPT > \x1b]133;B\x07";
        parser.advance(&mut term, seed);
        baseline_parser.advance(&mut baseline, seed);
        control
            .shell_integration
            .lock()
            .command_output_ranges
            .clear();
        assert!(
            state
                .shell_integration
                .lock()
                .command_output_ranges
                .back()
                .unwrap()
                .empty
        );
        for columns in [8, 30, 6, 60] {
            state.resize_terminal(
                &mut term,
                TerminalSize {
                    columns,
                    rows: 8,
                    ..Default::default()
                },
            );
            control.resize_terminal(
                &mut baseline,
                TerminalSize {
                    columns,
                    rows: 8,
                    ..Default::default()
                },
            );
            parser.advance(
                &mut term,
                b"\r\x1b[2K\x1b]133;A\x07LONG_PROMPT > \x1b]133;B\x07",
            );
            baseline_parser.advance(
                &mut baseline,
                b"\r\x1b[2K\x1b]133;A\x07LONG_PROMPT > \x1b]133;B\x07",
            );
            assert_eq!(term.grid().history_size(), baseline.grid().history_size());
            assert_eq!(term.grid().cursor.point, baseline.grid().cursor.point);
        }
    }

    #[test]
    fn text_limit_preserves_utf8_and_evicted_output_is_rejected() {
        let (_, mut term) = fixture(20, 3, 0);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, "中文123".as_bytes());
        let start = Point::new(Line(0), Column(0));
        let end = Point::new(Line(0), Column(6));
        assert_eq!(
            read_range(&term, start, end, 4).unwrap(),
            ("中".into(), true)
        );
        assert!(read_range(&term, Point::new(Line(-1), Column(0)), end, 100).is_err());
    }
}

fn output_end(terminal: &Term<DaemonEventListener>) -> Point {
    let cursor = &terminal.grid().cursor;
    if cursor.input_needs_wrap {
        cursor.point
    } else if cursor.point.column.0 > 0 {
        Point::new(cursor.point.line, cursor.point.column - 1)
    } else {
        Point::new(cursor.point.line - 1, terminal.grid().last_column())
    }
}

impl TerminalSession {
    pub(super) fn read_terminal_text(&self, tail_lines: u32) -> Result<(String, bool)> {
        let terminal = self.terminal.lock();
        let end = if terminal.mode().contains(TermMode::ALT_SCREEN) {
            terminal.bottommost_line()
        } else {
            terminal.grid().cursor.point.line
        };
        let first =
            Line((end.0 - tail_lines.clamp(1, 10_000) as i32 + 1).max(terminal.topmost_line().0));
        let (text, bytes_truncated) = read_range(
            &terminal,
            Point::new(first, Column(0)),
            Point::new(end, terminal.grid().last_column()),
            MAX_TEXT_BYTES,
        )?;
        Ok((text, bytes_truncated || first > terminal.topmost_line()))
    }

    pub(super) fn read_command_output(
        &self,
        command_id: u64,
        max_bytes: u32,
    ) -> Result<(String, bool)> {
        let terminal = self.terminal.lock();
        anyhow::ensure!(
            !terminal.mode().contains(TermMode::ALT_SCREEN),
            "Command output is unavailable in an alternate screen"
        );
        let mut shell = self.events.shell_integration.lock();
        shell.follow_grid_scroll(terminal.grid().scroll_position());
        let range = shell
            .command_output_ranges
            .iter()
            .find(|range| range.id == command_id)
            .cloned()
            .context("Command output is unavailable")?;
        drop(shell);
        if range.empty {
            return Ok((String::new(), false));
        }
        let start = range.start.context("Output is no longer in scrollback")?;
        let end = if range.finished {
            range.end.context("Output is no longer in scrollback")?
        } else {
            output_end(&terminal)
        };
        read_range(&terminal, start, end, max_bytes as usize)
    }

    pub(super) fn locate_command_output(&self, command_id: u64) -> Result<()> {
        let mut terminal = self.terminal.lock();
        anyhow::ensure!(
            !terminal.mode().contains(TermMode::ALT_SCREEN),
            "Command output is unavailable in an alternate screen"
        );
        let mut shell = self.events.shell_integration.lock();
        shell.follow_grid_scroll(terminal.grid().scroll_position());
        let start = shell
            .command_output_ranges
            .iter()
            .find(|range| range.id == command_id)
            .and_then(|range| range.start)
            .context("Output is no longer in scrollback")?;
        anyhow::ensure!(
            start.line >= terminal.topmost_line(),
            "Output is no longer in scrollback"
        );
        drop(shell);
        let offset = active_match_display_offset(&terminal, start);
        let previous = terminal.grid().display_offset();
        terminal.scroll_display(Scroll::Delta(offset as i32 - previous as i32));
        self.events.publish_terminal(&mut terminal);
        Ok(())
    }
}
