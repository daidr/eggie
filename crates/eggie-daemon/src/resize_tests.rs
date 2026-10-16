use super::*;
use alacritty_terminal::vte::ansi;

#[test]
fn prompt_anchor_follows_viewport_scroll_before_the_next_resize() {
    let mut tracker = ShellIntegrationTracker::default();
    tracker.update(
        SemanticPrompt {
            action: SemanticPromptAction::PromptStart,
            options: String::new(),
        },
        28,
        18,
    );
    tracker.follow_grid_scroll(2);
    tracker.observe_scroll(20, 1000);
    assert_eq!(tracker.prompt_start_line, Some(26));
    tracker.clear_jump_points();
    tracker.rebase_after_resize(-20, 40, 22);
    assert_eq!(tracker.prompt_start_line, Some(6));
    tracker.observe_scroll(40, 1000);
    assert_eq!(
        tracker.prompt_start_line,
        Some(6),
        "resize history must not be counted twice"
    );
    tracker.follow_grid_scroll(23);
    assert_eq!(
        tracker.prompt_start_line,
        Some(5),
        "full history still scrolls the active prompt"
    );
}

#[test]
fn prompt_anchor_tracks_scroll_when_history_is_disabled_or_full() {
    for limit in [0, 2] {
        let size = TerminalSize {
            columns: 40,
            rows: 4,
            ..Default::default()
        };
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
            None,
        ));
        let mut term = Term::new(
            Config {
                scrolling_history: limit,
                ..Default::default()
            },
            &GridSize(size),
            DaemonEventListener(state.clone()),
        );
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"OUT\r\nOUT\r\nOUT\r\nOUT\r\nOUT\r\n");
        parser.advance(
            &mut term,
            b"\x1b[3;1H\x1b]133;A\x07RESIZE_HEAD\r\n> \x1b]133;B\x07",
        );
        assert_eq!(state.shell_integration.lock().prompt_start_line, Some(2));
        let history = term.grid().history_size();
        parser.advance(&mut term, b"\r\n");
        assert_eq!(term.grid().history_size(), history);
        state.publish_terminal(&mut term);
        assert_eq!(state.shell_integration.lock().prompt_start_line, Some(1));
        state.publish_terminal(&mut term);
        assert_eq!(state.shell_integration.lock().prompt_start_line, Some(1));
        resize_terminal_with_history_reflow(
            &mut term,
            TerminalSize {
                columns: 30,
                ..size
            },
            TerminalSemanticPhase::Input,
            state.shell_integration.lock().prompt_start_line,
        );
        assert!(
            !(0..term.screen_lines()).any(|row| (0..term.columns())
                .any(|col| term.grid()[Line(row as i32)][Column(col)].c == 'R')),
            "stale prompt survived the next resize with history limit {limit}"
        );
    }
}

struct ResizeShell {
    session: TerminalSession,
    home: PathBuf,
}
impl Drop for ResizeShell {
    fn drop(&mut self) {
        self.session.terminate();
        std::fs::remove_dir_all(&self.home).ok();
    }
}

fn zsh(size: TerminalSize) -> ResizeShell {
    zsh_with_scrollback(size, 1000)
}

fn zsh_with_scrollback(size: TerminalSize, scrollback_limit: usize) -> ResizeShell {
    let home = std::env::temp_dir().join(format!("eggie-resize-{}", SessionId::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    let session = TerminalSession::spawn(SessionSpawnConfig {
        project_id: ProjectId::new_v4(),
        cwd: home.clone(),
        size,
        appearance: TerminalAppearance::default(),
        scrollback_limit,
        shell_program: Some("/bin/zsh".into()),
        shell_args: Some(vec!["-d".into(), "-f".into(), "-i".into()]),
        shell_features: String::new(),
        profile_env: BTreeMap::from([
            ("HOME".into(), home.display().to_string()),
            (
                "XDG_CACHE_HOME".into(),
                home.join(".cache").display().to_string(),
            ),
        ]),
        remote: None,
        remote_install_consented: false,
        remote_cwd: None,
        event_bus: None,
    })
    .unwrap();
    let fixture = ResizeShell { session, home };
    let deadline = Instant::now() + Duration::from_secs(5);
    while fixture.session.snapshot().cursor_column == 0 {
        assert!(
            Instant::now() < deadline,
            "zsh did not reach its initial prompt"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture
}

fn prompt_gap(session: &TerminalSession) -> (usize, usize) {
    let term = session.terminal.lock();
    let mut last_output = None;
    let mut gap = 0;
    let mut prompts = 0;
    for row in -(term.grid().history_size() as i32)..term.screen_lines() as i32 {
        let text: String = (0..term.columns())
            .map(|col| term.grid()[Line(row)][Column(col)].c)
            .collect();
        if text.starts_with("HIST_SENTINEL") {
            last_output = Some(row);
        }
        if text.contains("RESIZE_HEAD") {
            prompts += 1;
            gap = last_output.map_or(0, |output| (row - output - 1).max(0) as usize);
        }
    }
    (gap, prompts)
}

fn prompt_position(session: &TerminalSession) -> (i32, i32, usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stable = None;
    loop {
        {
            let term = session.terminal.lock();
            let shell = session.events.shell_integration.lock();
            if shell.phase == TerminalSemanticPhase::Input
                && shell
                    .prompt_input
                    .as_ref()
                    .is_some_and(|input| input.text.is_empty())
            {
                for row in 0..term.screen_lines() {
                    let text: String = (0..term.columns())
                        .map(|col| term.grid()[Line(row as i32)][Column(col)].c)
                        .collect();
                    if text.contains("RESIZE_HEAD") {
                        let position = (
                            row as i32,
                            term.grid().cursor.point.line.0,
                            term.grid().history_size(),
                        );
                        match stable {
                            Some((previous, since)) if previous == position => {
                                if Instant::now().duration_since(since)
                                    >= Duration::from_millis(100)
                                {
                                    return position;
                                }
                            }
                            _ => stable = Some((position, Instant::now())),
                        }
                        break;
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            let term = session.terminal.lock();
            let shell = session.events.shell_integration.lock();
            eprintln!(
                "phase={:?} prompt={:?} cursor={:?}",
                shell.phase,
                shell.prompt_start_line,
                term.grid().cursor.point
            );
            for row in 0..term.screen_lines() {
                let text: String = (0..term.columns())
                    .map(|col| term.grid()[Line(row as i32)][Column(col)].c)
                    .collect();
                if !text.trim().is_empty() {
                    eprintln!("{row}: {text}");
                }
            }
            panic!("shell did not redraw its prompt");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_size(session: &TerminalSession, columns: u16, rows: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot();
        if snapshot.size.columns == columns && snapshot.size.rows == rows {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "queued resize was not applied: {columns}x{rows}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn rapid_prompt_shrink_preserves_rows_and_applies_the_latest_size_before_input() {
    let _lock = tests::PTY_TEST_LOCK.lock();
    let initial = TerminalSize {
        columns: 220,
        rows: 30,
        ..Default::default()
    };
    let fixture = zsh(initial);
    let integration =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("shell-integration/zsh/eggie-integration");
    let setup = format!(
        concat!(
            "source '{}'; eggie-integration; setopt promptsubst; RPROMPT=''; ",
            "PROMPT=$'RESIZE_HEAD${{(l:$((COLUMNS-11))::─:)}}\\n> '; ",
            "printf '\\033[2J\\033[3J\\033[H'; repeat 45 print -r -- HIST_SENTINEL\n"
        ),
        integration.display()
    );
    fixture.session.input(setup.into_bytes(), 1).unwrap();
    prompt_position(&fixture.session);
    let before = prompt_gap(&fixture.session);
    for delay_ms in [0, 4, 16] {
        for columns in (40..=218).rev().step_by(2) {
            fixture
                .session
                .resize(TerminalSize { columns, ..initial })
                .unwrap();
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        wait_for_size(&fixture.session, 40, initial.rows);
        prompt_position(&fixture.session);
        assert_eq!(
            prompt_gap(&fixture.session),
            before,
            "prompt drifted at {delay_ms}ms"
        );
        fixture.session.resize(initial).unwrap();
        wait_for_size(&fixture.session, initial.columns, initial.rows);
        prompt_position(&fixture.session);
    }
    fixture
        .session
        .resize(TerminalSize {
            columns: 40,
            ..initial
        })
        .unwrap();
    fixture.session.resize(initial).unwrap();
    fixture
        .session
        .input(b"printf '\\nWIDTH_%s_DONE\\n' \"$COLUMNS\"\n".to_vec(), 2)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let term = fixture.session.terminal.lock();
        let found = (0..term.screen_lines()).any(|row| {
            (0..term.columns())
                .map(|col| term.grid()[Line(row as i32)][Column(col)].c)
                .collect::<String>()
                .starts_with("WIDTH_220_DONE")
        });
        drop(term);
        if found {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "input did not use the latest PTY size"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // Let the cancelled shrink's deadline pass: it must not restore the old width later.
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(fixture.session.snapshot().size.columns, 220);
}

#[test]
#[ignore = "requires EGGIE_TEST_P10K_THEME and EGGIE_TEST_P10K_CONFIG"]
fn powerlevel10k_resize_keeps_prompt_anchor() {
    let _lock = tests::PTY_TEST_LOCK.lock();
    let theme = std::env::var("EGGIE_TEST_P10K_THEME").unwrap();
    let config = std::env::var("EGGIE_TEST_P10K_CONFIG").unwrap();
    let initial = TerminalSize {
        columns: 103,
        rows: 30,
        ..Default::default()
    };
    let limit = std::env::var("EGGIE_TEST_SCROLLBACK")
        .ok()
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(1000);
    let fixture = zsh_with_scrollback(initial, limit);
    let integration =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("shell-integration/zsh/eggie-integration");
    let setup = format!(
        concat!(
            "source '{}'; eggie-integration; ",
            "typeset -g POWERLEVEL9K_DISABLE_GITSTATUS=true; ",
            "source '{}'; source '{}'; ",
            "typeset -g POWERLEVEL9K_DISABLE_GITSTATUS=true; ",
            "function prompt_resize_head() {{ p10k segment -t RESIZE_HEAD; }}; ",
            "typeset -ga POWERLEVEL9K_LEFT_PROMPT_ELEMENTS=(resize_head $POWERLEVEL9K_LEFT_PROMPT_ELEMENTS); ",
            "p10k reload; printf '\\033[2J\\033[3J\\033[H'; ",
            "repeat 45 print -r -- HIST_SENTINEL\n"
        ),
        integration.display(),
        theme,
        config,
    );
    fixture.session.input(setup.into_bytes(), 1).unwrap();
    let before = prompt_position(&fixture.session);
    let before_gap = prompt_gap(&fixture.session);
    eprintln!("before {before:?}");
    for (index, (columns, rows)) in [
        (80, 30),
        (140, 30),
        (60, 30),
        (103, 30),
        (103, 10),
        (103, 40),
    ]
    .into_iter()
    .cycle()
    .take(24)
    .enumerate()
    {
        fixture
            .session
            .resize(TerminalSize {
                columns,
                rows,
                ..initial
            })
            .unwrap();
        wait_for_size(&fixture.session, columns, rows);
        let after = prompt_position(&fixture.session);
        eprintln!("resize {index}: {columns}x{rows}: {after:?}");
        if prompt_gap(&fixture.session) != before_gap
            || (limit > 100 && after.0 as usize + after.2 != before.0 as usize + before.2)
        {
            let term = fixture.session.terminal.lock();
            for row in -(term.grid().history_size() as i32)..term.screen_lines() as i32 {
                let text: String = (0..term.columns())
                    .map(|col| term.grid()[Line(row)][Column(col)].c)
                    .collect();
                if !text.trim().is_empty() {
                    eprintln!("{row}: {text}");
                }
            }
        }
        assert_eq!(
            prompt_gap(&fixture.session),
            before_gap,
            "prompt gap or copies changed after resize {index}"
        );
        if limit > 100 {
            assert_eq!(
                after.0 as usize + after.2,
                before.0 as usize + before.2,
                "prompt drifted after resize {index}"
            );
        }
    }
    for delay_ms in [0, 1, 4, 8, 16] {
        fixture
            .session
            .resize(TerminalSize {
                columns: 220,
                ..initial
            })
            .unwrap();
        wait_for_size(&fixture.session, 220, initial.rows);
        prompt_position(&fixture.session);
        for columns in (40..=218).rev().step_by(2) {
            fixture
                .session
                .resize(TerminalSize { columns, ..initial })
                .unwrap();
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        wait_for_size(&fixture.session, 40, initial.rows);
        let after = prompt_position(&fixture.session);
        eprintln!(
            "fast shrink at {delay_ms}ms: {after:?}, gap={:?}",
            prompt_gap(&fixture.session)
        );
        assert_eq!(
            prompt_gap(&fixture.session),
            before_gap,
            "prompt drifted during monotonic shrink at {delay_ms}ms"
        );
        fixture.session.resize(initial).unwrap();
        wait_for_size(&fixture.session, initial.columns, initial.rows);
        prompt_position(&fixture.session);
    }
    for burst in 0..4 {
        for step in 0..12 {
            fixture
                .session
                .resize(TerminalSize {
                    columns: 60 + (step * 17 % 80),
                    rows: 20 + step % 5,
                    ..initial
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(8));
        }
        fixture.session.resize(initial).unwrap();
        wait_for_size(&fixture.session, initial.columns, initial.rows);
        prompt_position(&fixture.session);
        assert_eq!(
            prompt_gap(&fixture.session),
            before_gap,
            "prompt drifted after rapid resize burst {burst}"
        );
    }
}

#[test]
fn repeated_zsh_width_changes_do_not_add_blank_lines_above_multiline_prompt() {
    let _lock = tests::PTY_TEST_LOCK.lock();
    for prompt in [
        "RESIZE_HEAD\\n> ",
        "RESIZE_HEAD${(l:$((COLUMNS-11))::─:)}\\n> ",
    ] {
        let initial = TerminalSize {
            columns: 120,
            rows: 30,
            ..Default::default()
        };
        let fixture = zsh(initial);
        let integration =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("shell-integration/zsh/eggie-integration");
        let setup = format!(
            concat!(
                "source '{}'; eggie-integration; setopt promptsubst; RPROMPT=''; ",
                "PROMPT=$'{}'; printf '\\033[2J\\033[3J\\033[H'\n"
            ),
            integration.display(),
            prompt,
        );
        fixture.session.input(setup.into_bytes(), 1).unwrap();
        let before = prompt_position(&fixture.session);
        eprintln!("before {before:?}, {prompt}");
        for (index, columns) in [80, 140, 60, 120].into_iter().cycle().take(16).enumerate() {
            fixture
                .session
                .resize(TerminalSize { columns, ..initial })
                .unwrap();
            wait_for_size(&fixture.session, columns, initial.rows);
            let after = prompt_position(&fixture.session);
            eprintln!("resize {index}: {columns}: {after:?}");
            assert_eq!(
                after.0, before.0,
                "prompt drifted after resize {index} to {columns} columns: {prompt}"
            );
            assert_eq!(after.2, before.2, "resize added scrollback");
        }
    }
}
