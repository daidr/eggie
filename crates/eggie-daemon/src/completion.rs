use super::*;
use eggie_completion::{
    CompletionContext, CompletionItem, CompletionKind, MAX_CANDIDATES, NATIVE_REQUEST, PromptInput,
    percent_decode, rank,
};

/// Only Enter outside a checked edit or bracketed paste submits a shell command.
pub(super) fn submits_command(bytes: &[u8]) -> bool {
    let ordinary = if bytes.starts_with(b"\x18\x1e") {
        bytes
            .iter()
            .position(|b| matches!(b, b'\n' | 0))
            .map_or(&[][..], |i| &bytes[i + 1..])
    } else if bytes.starts_with(b"\x1b[200~") {
        bytes
            .windows(6)
            .position(|w| w == b"\x1b[201~")
            .map_or(&[][..], |i| &bytes[i + 6..])
    } else {
        bytes
    };
    ordinary.iter().any(|b| matches!(b, b'\r' | b'\n'))
}

impl ListenerState {
    pub(super) fn handle_completion_report(&self, payload: &str) -> bool {
        if let Some(report) = payload.strip_prefix("EggieShell=") {
            let fields: Vec<_> = report.split(';').collect();
            if fields.len() >= 2 {
                let shell = fields[0];
                let native = fields[1];
                let mut tracker = self.shell_integration.lock();
                tracker.completion_shell = shell.into();
                tracker.completion_native = native == "1";
                if fields.len() == 5 {
                    tracker.completion_environment = PromptInput::from_report(&format!(
                        "{shell};0;{};{};{};{native};",
                        fields[2], fields[3], fields[4]
                    ));
                }
            }
            return true;
        }
        if let Some(report) = payload.strip_prefix("EggieInput=") {
            if let Some(mut input) = PromptInput::from_report(report) {
                input.native_revision = self.native_result_revision.load(Ordering::Acquire);
                let mut tracker = self.shell_integration.lock();
                tracker.completion_shell = input.shell.clone();
                tracker.completion_native = input.native;
                tracker.prompt_input = Some(input);
                tracker.phase = TerminalSemanticPhase::Input;
            } else {
                self.shell_integration.lock().prompt_input = None;
            }
            return true;
        }
        if let Some(report) = payload.strip_prefix("EggieCompletions=") {
            if report.len() > 512 * 1024 {
                return true;
            }
            let mut fields = report.split(';');
            let Some(text) = fields.next().and_then(percent_decode) else {
                return true;
            };
            let Some(cursor) = fields.next().and_then(|s| s.parse::<usize>().ok()) else {
                return true;
            };
            let mut tracker = self.shell_integration.lock();
            let Some(mut input) = tracker
                .prompt_input
                .as_ref()
                .filter(|p| p.text == text && p.text[..p.cursor].chars().count() == cursor)
                .cloned()
            else {
                return true;
            };
            let Some(ctx) = CompletionContext::parse(&input) else {
                return true;
            };
            let items = fields
                .take(MAX_CANDIDATES)
                .filter_map(percent_decode)
                .filter(|s| !s.is_empty() && !s.contains(['\r', '\n', '\x1b', '\x07']))
                .map(|s| {
                    let (label, description) = s.split_once('\t').unwrap_or((&s, ""));
                    ctx.item(
                        label,
                        description,
                        if label.ends_with('/') {
                            CompletionKind::Directory
                        } else {
                            CompletionKind::Native
                        },
                    )
                })
                .collect::<Vec<_>>();
            input.native_revision = self
                .native_result_revision
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            tracker.prompt_input = Some(input.clone());
            *self.native_completions.lock() = Some((input, items));
            return true;
        }
        false
    }
    /// Runs under the terminal lock, so cursor and prompt markers describe the same frame.
    pub(super) fn prompt_input_for_frame(
        &self,
        terminal: &Term<DaemonEventListener>,
    ) -> Option<PromptInput> {
        let tracker = self.shell_integration.lock();
        if self.completion_query_blocked.load(Ordering::Acquire)
            || !matches!(
                tracker.phase,
                TerminalSemanticPhase::Input | TerminalSemanticPhase::Prompt
            )
            || terminal.mode().contains(TermMode::ALT_SCREEN)
        {
            return None;
        }
        if let Some(input) = &tracker.prompt_input {
            return Some(input.clone());
        }
        // ZLE reports before every redraw; a half-painted grid is not an editor buffer.
        if tracker.completion_shell == "zsh" && tracker.completion_native {
            return None;
        }
        let (line, col) = *tracker.input_starts.first()?;
        let offset = tracker.total_scrolled_lines as i64;
        let first = line as i64 - offset;
        let cursor = terminal.grid().cursor.point;
        if first < -(terminal.grid().history_size() as i64)
            || first > cursor.line.0 as i64
            || cursor.line.0 as i64 - first > 128
        {
            return None;
        }
        let mut text = String::new();
        let mut cursor_byte = 0;
        for y in first as i32..=cursor.line.0 {
            let row = &terminal.grid()[Line(y)];
            let from = tracker
                .input_starts
                .iter()
                .find(|(l, _)| *l as i64 == offset + y as i64)
                .map(|(_, c)| *c)
                .unwrap_or(if y == first as i32 { col } else { 0 });
            let mut last = row[..]
                .iter()
                .rposition(|c| c.c != ' ' || c.zerowidth().is_some())
                .map_or(from, |x| x + 1)
                .max(if y == cursor.line.0 {
                    cursor.column.0
                } else {
                    0
                });
            // Fish's unaccepted autosuggestion shares the row but is outside the editor.
            // A native query supplies the full buffer, including a real suffix when editing in the middle.
            if tracker.completion_shell == "fish" && y == cursor.line.0 {
                last = cursor.column.0;
            }
            for x in from..last.min(row.len()) {
                if y == cursor.line.0 && x == cursor.column.0 {
                    cursor_byte = text.len();
                }
                let cell = &row[Column(x)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                // Shell autosuggestions are dim/italic; keep them outside editable input.
                if cell.flags.intersects(Flags::DIM | Flags::ITALIC)
                    && y == cursor.line.0
                    && x >= cursor.column.0
                {
                    break;
                }
                text.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    text.extend(extra);
                }
            }
            if y == cursor.line.0 && cursor.column.0 >= last {
                cursor_byte = text.len();
            }
            if y < cursor.line.0 && !row[Column(row.len() - 1)].flags.contains(Flags::WRAPLINE) {
                text.push('\n');
            }
        }
        let cwd = self
            .reported_location
            .read()
            .as_ref()
            .map(|l| l.path.clone().into())
            .unwrap_or_default();
        let env = tracker.completion_environment.as_ref();
        Some(PromptInput {
            text,
            cursor: cursor_byte,
            cwd: env.map(|v| v.cwd.clone()).unwrap_or(cwd),
            shell: tracker.completion_shell.clone(),
            search_path: env
                .map(|v| v.search_path.clone())
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default()),
            home: env
                .map(|v| v.home.clone())
                .unwrap_or_else(|| std::env::var_os("HOME").unwrap_or_default().into()),
            native: tracker.completion_native,
            native_revision: self.native_result_revision.load(Ordering::Acquire),
        })
    }
}
impl TerminalSession {
    pub(super) fn completions(&self, input: PromptInput, native: bool) -> Vec<CompletionItem> {
        let current = self.snapshot();
        if !input.valid()
            || current
                .prompt_input
                .as_ref()
                .is_none_or(|p| !p.same_edit(&input))
        {
            return vec![];
        }
        if native {
            if input.native {
                self.request_native_completion(&input);
            }
            // Native results arrive through the input revision, without holding an RPC open.
            return vec![];
        }
        let mut history = self
            .events
            .shell_integration
            .lock()
            .history
            .iter()
            .rev()
            .take(200)
            .filter_map(|c| c.command_line.clone())
            .collect::<Vec<_>>();
        history.reverse();
        let mut local = input.clone();
        if local.cwd.as_os_str().is_empty() {
            local.cwd = self.initial_directory.clone();
        }
        let mut items = if self.remote.is_none()
            && self
                .events
                .reported_location
                .read()
                .as_ref()
                .is_none_or(|l| l.local)
        {
            self.events.completion_engine.complete(&local, &history)
        } else {
            vec![]
        };
        if let Some((p, cached)) = self.events.native_completions.lock().as_ref()
            && p.same_edit(&input)
        {
            items.extend(cached.iter().cloned());
            let query = CompletionContext::parse(&input)
                .map(|c| c.query)
                .unwrap_or_default();
            return rank(items, &query);
        }
        items
    }

    fn request_native_completion(&self, input: &PromptInput) {
        let _gate = self.events.completion_input_gate.lock();
        let current = self.snapshot();
        if self.events.completion_query_blocked.load(Ordering::Acquire)
            || current.last_input_sequence != self.last_input_sequence.load(Ordering::Acquire)
            || current
                .prompt_input
                .as_ref()
                .is_none_or(|p| !p.same_edit(input))
            || self
                .events
                .native_completions
                .lock()
                .as_ref()
                .is_some_and(|(p, _)| p.same_edit(input))
        {
            return;
        }
        let mut pending = self.events.native_completion_request.lock();
        if pending.as_ref().is_some_and(|(p, started)| {
            p.same_edit(input) && started.elapsed() < Duration::from_secs(2)
        }) {
            return;
        }
        *pending = Some((input.clone(), Instant::now()));
        self.events.write_to_pty(NATIVE_REQUEST.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::PTY_TEST_LOCK;
    #[test]
    fn batched_accept_or_paste_followed_by_enter_blocks_native_queries() {
        assert!(!submits_command(b"\x18\x1echecked;edit\n"));
        assert!(submits_command(b"\x18\x1echecked;edit\n\r"));
        assert!(!submits_command(b"\x18\x1echecked;edit\0"));
        assert!(submits_command(b"\x18\x1echecked;edit\0\r"));
        assert!(!submits_command(b"\x1b[200~one\ntwo\x1b[201~"));
        assert!(submits_command(b"\x1b[200~one\ntwo\x1b[201~\r"));
    }
    #[test]
    fn osc_input_candidates_and_stale_results() {
        let state = ListenerState::new(
            SessionId::new_v4(),
            TerminalSize::default(),
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
            None,
        );
        assert!(state.handle_completion_report("EggieInput=zsh;5;/tmp;/bin;/tmp;1;cat%20中"));
        let input = state.shell_integration.lock().prompt_input.clone().unwrap();
        assert_eq!(input.cursor, input.text.len());
        state.handle_completion_report("EggieCompletions=cat%20中;5;中文.txt");
        let cache = state.native_completions.lock();
        assert_eq!(cache.as_ref().unwrap().1[0].insert_text, "中文.txt");
        drop(cache);
        state.handle_completion_report("EggieCompletions=cat%20old;7;wrong");
        assert_eq!(
            state.native_completions.lock().as_ref().unwrap().1[0].label,
            "中文.txt"
        );
    }
    #[test]
    fn redraw_frames_keep_prompt_ownership_without_reconstructing_partial_zle_input() {
        let size = TerminalSize::default();
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
            None,
        ));
        let mut terminal = Term::new(
            Config::default(),
            &GridSize(size),
            DaemonEventListener(state.clone()),
        );
        {
            let mut tracker = state.shell_integration.lock();
            tracker.phase = TerminalSemanticPhase::Input;
            tracker.completion_shell = "zsh".into();
            tracker.completion_native = true;
            tracker.input_starts.push((0, 0));
        }
        for (column, character) in "stale-grid".chars().enumerate() {
            terminal.grid_mut()[Line(0)][Column(column)].c = character;
        }
        terminal.grid_mut().cursor.point = Point::new(Line(0), Column(4));
        state.publish_terminal(&mut terminal);
        let frame = state.published_snapshots.read().back().unwrap().clone();
        assert!(frame.prompt_editable);
        assert!(
            frame.prompt_input.is_none(),
            "partial redraw leaked into the editor buffer"
        );
        state.handle_completion_report("EggieInput=zsh;6;/tmp;/bin;/tmp;1;git%20ch");
        state.publish_terminal(&mut terminal);
        let frame = state.published_snapshots.read().back().unwrap().clone();
        assert!(frame.prompt_editable);
        assert_eq!(frame.prompt_input.as_ref().unwrap().text, "git ch");
        state.shell_integration.lock().prompt_input = None;
        state.publish_terminal(&mut terminal);
        assert!(
            state
                .published_snapshots
                .read()
                .back()
                .unwrap()
                .prompt_editable
        );
        state
            .completion_query_blocked
            .store(true, Ordering::Release);
        state.publish_terminal(&mut terminal);
        let frame = state.published_snapshots.read().back().unwrap().clone();
        assert!(!frame.prompt_editable && frame.prompt_input.is_none());
    }
    #[test]
    fn real_shell_input_native_completion_replacement_and_running_process_isolation() {
        let _guard = PTY_TEST_LOCK.lock();
        for program in [
            "/bin/zsh",
            "/opt/homebrew/bin/bash",
            "/opt/homebrew/bin/fish",
        ] {
            if !std::path::Path::new(program).is_file() {
                continue;
            }
            let dir =
                std::env::temp_dir().join(format!("eggie-completion-{}", SessionId::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("中文 file.txt"), "").unwrap();
            let session = TerminalSession::spawn(SessionSpawnConfig {
                project_id: ProjectId::new_v4(),
                cwd: dir.clone(),
                size: TerminalSize::default(),
                appearance: TerminalAppearance::default(),
                scrollback_limit: 1000,
                shell_program: Some(program.into()),
                shell_args: Some(vec!["-i".into()]),
                shell_features: String::new(),
                profile_env: BTreeMap::from([("HOME".into(), dir.to_string_lossy().into_owned())]),
                remote: None,
                remote_install_consented: false,
                remote_cwd: None,
                event_bus: None,
            })
            .unwrap();
            let wait = |text: &str| {
                let deadline = Instant::now() + Duration::from_secs(8);
                loop {
                    if let Some(input) = session.snapshot().prompt_input.as_ref()
                        && input.text == text
                    {
                        return input.clone();
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{program}: missing edit {text:?}: {:?}; input={:?}, starts_phase={:?}, blocked={}",
                        session.snapshot().plain_lines(),
                        session.snapshot().prompt_input,
                        {
                            let t = session.events.shell_integration.lock();
                            (t.input_starts.clone(), t.phase)
                        },
                        session
                            .events
                            .completion_query_blocked
                            .load(Ordering::Acquire)
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            };
            wait("");
            session.input(b"cat ".to_vec(), 1).unwrap();
            let input = wait("cat ");
            assert!(
                session
                    .completions(input.clone(), false)
                    .iter()
                    .any(|i| i.label == "中文 file.txt")
            );
            *session.events.reported_location.write() = Some(TerminalReportedLocation {
                user: None,
                host: Some("remote.invalid".into()),
                path: "/srv/project".into(),
                local: false,
            });
            assert!(
                session.completions(input.clone(), false).is_empty(),
                "a remote prompt cannot expose local files"
            );
            *session.events.reported_location.write() = None;
            let revision = input.native_revision;
            let started = Instant::now();
            assert!(session.completions(input.clone(), true).is_empty());
            assert!(
                started.elapsed() < Duration::from_millis(200),
                "native dispatch waited for the shell"
            );
            let pending = session
                .events
                .native_completion_request
                .lock()
                .as_ref()
                .unwrap()
                .1;
            session.completions(input.clone(), true);
            assert_eq!(
                session
                    .events
                    .native_completion_request
                    .lock()
                    .as_ref()
                    .unwrap()
                    .1,
                pending,
                "the same edit must not enqueue another query"
            );
            let deadline = Instant::now() + Duration::from_secs(8);
            while session
                .snapshot()
                .prompt_input
                .as_ref()
                .is_none_or(|p| p.native_revision <= revision)
            {
                assert!(
                    Instant::now() < deadline,
                    "{program}: no asynchronous native result"
                );
                thread::sleep(Duration::from_millis(5));
            }
            let items = session.completions(input.clone(), false);
            let item = items
                .iter()
                .find(|i| i.label == "中文 file.txt")
                .expect("native/path candidate");
            session.input(item.replacement(&input).unwrap(), 2).unwrap();
            wait("cat 中文\\ file.txt");
            let stale = session.completions(input, false);
            assert!(stale.is_empty());
            let interrupt = |sequence| {
                // Fish negotiates kitty keys; use the same Ctrl-C bytes as the GUI in that mode.
                let bytes = if session.snapshot().input_modes.kitty_keyboard_flags != 0 {
                    b"\x1b[99;5u".to_vec()
                } else {
                    b"\x03".to_vec()
                };
                session.input(bytes, sequence).unwrap();
            };
            let clear_edit = |sequence| {
                let bytes = if session.snapshot().input_modes.kitty_keyboard_flags != 0 {
                    b"\x1b[117;5u".to_vec()
                } else {
                    b"\x15".to_vec()
                };
                session.input(bytes, sequence).unwrap();
            };
            clear_edit(3);
            wait("");
            session.input("echo 中".as_bytes().to_vec(), 4).unwrap();
            let input = wait("echo 中");
            assert_eq!(input.cursor, input.text.len());
            clear_edit(5);
            wait("");
            session.input(b"sleep 10".to_vec(), 6).unwrap();
            let input = wait("sleep 10");
            let checked = CompletionContext::parse(&input)
                .unwrap()
                .item("100", "", CompletionKind::Argument)
                .replacement(&input)
                .unwrap();
            session.input(b"\r".to_vec(), 7).unwrap();
            assert!(
                session.input(checked, 8).is_err(),
                "an old edit cannot enter a running process"
            );
            assert!(session.completions(input, true).is_empty());
            let started = Instant::now() + Duration::from_secs(2);
            while unsafe { libc::tcgetpgrp(session.pty_fd) } == session.shell_pid as i32 {
                assert!(Instant::now() < started, "sleep did not acquire the PTY");
                thread::sleep(Duration::from_millis(5));
            }
            interrupt(9);
            wait("");
            session.input(b"exit\r".to_vec(), 10).unwrap();
            let deadline = Instant::now() + Duration::from_secs(4);
            while matches!(*session.events.status.read(), SessionStatus::Running) {
                assert!(Instant::now() < deadline, "shell did not exit");
                thread::sleep(Duration::from_millis(10));
            }
            session.terminate();
            drop(session);
            fs::remove_dir_all(dir).unwrap();
        }
    }
}
