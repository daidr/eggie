use super::*;
use eggie_protocol::{
    LARGE_PASTE_BYTES, TerminalPasteContext, TerminalPastePhase, TerminalPasteStatus,
};
use std::sync::atomic::AtomicUsize;

const PASTE_CHUNK_BYTES: usize = 4096;

#[derive(Default)]
pub(super) struct PasteState {
    job: Option<Arc<PasteJob>>,
    closed: bool,
    pub(super) deferred_focus: Option<bool>,
}

struct PasteJob {
    id: u64,
    total: usize,
    written: Arc<AtomicUsize>,
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    error: Mutex<Option<String>>,
    bracketed: bool,
    completion_was_blocked: bool,
}

impl PasteState {
    pub(super) fn is_sending(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|job| !job.finished.load(Ordering::Acquire) && !self.closed)
    }
}

impl PasteJob {
    fn status(&self) -> TerminalPasteStatus {
        let written = self.written.load(Ordering::Acquire);
        let mut error = self.error.lock().clone();
        if self.finished.load(Ordering::Acquire)
            && !self.cancelled.load(Ordering::Acquire)
            && written < self.total
            && error.is_none()
        {
            error = Some("terminal process exited".to_owned());
        }
        let phase = if error.is_some() {
            TerminalPastePhase::Failed
        } else if self.cancelled.load(Ordering::Acquire) {
            TerminalPastePhase::Cancelled
        } else if !self.finished.load(Ordering::Acquire) {
            TerminalPastePhase::Sending
        } else {
            TerminalPastePhase::Completed
        };
        TerminalPasteStatus {
            id: self.id,
            total_bytes: self.total,
            written_bytes: written,
            phase,
            error,
        }
    }
}

impl TerminalSession {
    pub(super) fn paste_context(&self) -> TerminalPasteContext {
        let mode = *self.terminal.lock().mode();
        TerminalPasteContext {
            bracketed: mode.contains(TermMode::BRACKETED_PASTE),
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            prompt_editable: matches!(
                self.events.shell_integration.lock().phase,
                TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
            ),
            paste_events: mode.contains(TermMode::PASTE_EVENTS),
        }
    }

    pub(super) fn paste_text(
        self: &Arc<Self>,
        text: String,
        sequence: u64,
        expected: Option<TerminalPasteContext>,
    ) -> Result<TerminalPasteStatus> {
        let mut state = self.paste_state.lock();
        let context = self.paste_context();
        anyhow::ensure!(
            expected.is_none_or(|expected| expected == context),
            "paste target changed"
        );
        anyhow::ensure!(
            *self.events.status.read() == SessionStatus::Running,
            "terminal process exited"
        );
        self.finish_paste_locked(&mut state, true)?;
        let text = eggie_protocol::sanitize_paste_owned(text);
        let total = text.len();
        if total < LARGE_PASTE_BYTES || context.paste_events {
            // Rich-paste applications retain their protocol; text-only preview uses the same grant.
            if context.paste_events {
                self.last_input_sequence.store(sequence, Ordering::Release);
                self.events.paste_clipboard(
                    TerminalClipboardSelection::Clipboard,
                    vec![TerminalClipboardContent {
                        mime_type: "text/plain".to_owned(),
                        data: text.into_bytes(),
                    }],
                );
            } else {
                self.input_unchecked(paste_bytes(&text, context.bracketed), sequence)?;
            }
            return Ok(TerminalPasteStatus {
                id: sequence,
                total_bytes: total,
                written_bytes: total,
                phase: TerminalPastePhase::Completed,
                error: None,
            });
        }

        let mut bytes = text.into_bytes();
        if !context.bracketed {
            for byte in &mut bytes {
                if *byte == b'\n' {
                    *byte = b'\r';
                }
            }
        }
        let job = Arc::new(PasteJob {
            id: sequence,
            total: bytes.len(),
            written: Arc::new(AtomicUsize::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicBool::new(false)),
            error: Mutex::new(None),
            bracketed: context.bracketed,
            completion_was_blocked: self
                .events
                .completion_query_blocked
                .swap(true, Ordering::AcqRel),
        });
        state.job = Some(job.clone());
        state.closed = false;
        *self.events.paste_replies.lock() = Some(VecDeque::new());
        if let Err(error) = self.input_unchecked(
            if context.bracketed {
                b"\x1b[200~".to_vec()
            } else {
                Vec::new()
            },
            sequence,
        ) {
            self.finish_paste_locked(&mut state, true).ok();
            return Err(error);
        }
        let weak = Arc::downgrade(self);
        let worker_job = job.clone();
        let spawn = thread::Builder::new()
            .name("eggie-paste".to_owned())
            .spawn(move || {
                let mut offset = 0;
                while offset < bytes.len() && !worker_job.cancelled.load(Ordering::Acquire) {
                    let Some(session) = weak.upgrade() else { break };
                    let state = session.paste_state.lock();
                    if state.closed
                        || state
                            .job
                            .as_ref()
                            .is_none_or(|job| !Arc::ptr_eq(job, &worker_job))
                    {
                        break;
                    }
                    // Never split a UTF-8 code point between PTY writes.
                    let mut end = (offset + PASTE_CHUNK_BYTES).min(bytes.len());
                    while end < bytes.len() && bytes[end] & 0xc0 == 0x80 {
                        end -= 1;
                    }
                    let finished = Arc::new(AtomicBool::new(false));
                    let result = session.sender.send(Msg::TrackedInput {
                        bytes: bytes[offset..end].to_vec().into(),
                        written: worker_job.written.clone(),
                        cancelled: worker_job.cancelled.clone(),
                        finished: finished.clone(),
                    });
                    drop(state);
                    drop(session);
                    if let Err(error) = result {
                        *worker_job.error.lock() = Some(error.to_string());
                        break;
                    }
                    // One chunk in flight: progress measures PTY writes, and cancellation never has
                    // megabytes waiting in an unbounded input queue.
                    while !finished.load(Ordering::Acquire)
                        && !worker_job.cancelled.load(Ordering::Acquire)
                    {
                        thread::sleep(Duration::from_millis(2));
                        if weak.upgrade().is_none_or(|session| {
                            *session.events.status.read() != SessionStatus::Running
                        }) {
                            *worker_job.error.lock() = Some("terminal process exited".to_owned());
                            worker_job.cancelled.store(true, Ordering::Release);
                            break;
                        }
                    }
                    if !worker_job.cancelled.load(Ordering::Acquire)
                        && worker_job.written.load(Ordering::Acquire) < end
                    {
                        *worker_job.error.lock() = Some("terminal process exited".to_owned());
                        break;
                    }
                    offset = end;
                }
                if let Some(session) = weak.upgrade() {
                    let mut state = session.paste_state.lock();
                    if state
                        .job
                        .as_ref()
                        .is_some_and(|job| Arc::ptr_eq(job, &worker_job))
                    {
                        if let Err(error) = session.finish_paste_locked(&mut state, false) {
                            *worker_job.error.lock() = Some(error.to_string());
                        }
                    }
                } else {
                    *worker_job.error.lock() = Some("terminal process exited".to_owned());
                }
            });
        if let Err(error) = spawn {
            *job.error.lock() = Some(error.to_string());
            self.finish_paste_locked(&mut state, true)?;
        }
        Ok(job.status())
    }

    pub(super) fn finish_paste_locked(&self, state: &mut PasteState, cancel: bool) -> Result<()> {
        let Some(job) = state.job.as_ref().filter(|_| !state.closed) else {
            return Ok(());
        };
        state.closed = true;
        if cancel {
            job.cancelled.store(true, Ordering::Release);
        }
        // This non-cancellable delimiter is enqueued before any later user input.
        let result = self.sender.send(Msg::TrackedInput {
            bytes: if job.bracketed {
                b"\x1b[201~".to_vec()
            } else {
                Vec::new()
            }
            .into(),
            written: Arc::new(AtomicUsize::new(0)),
            cancelled: Arc::new(AtomicBool::new(false)),
            finished: job.finished.clone(),
        });
        self.events.release_paste_replies();
        if let Some(focused) = state.deferred_focus.take() {
            let bytes = if focused { b"\x1b[I" } else { b"\x1b[O" };
            let _ = self.sender.send(Msg::Input(bytes.to_vec().into()));
        }
        let blocked = if job.bracketed {
            job.completion_was_blocked
        } else {
            !self.paste_context().prompt_editable
        };
        self.events
            .completion_query_blocked
            .store(blocked, Ordering::Release);
        if let Err(error) = &result {
            *job.error.lock() = Some(error.to_string());
        }
        result.context("failed to finish terminal paste")
    }

    pub(super) fn paste_status(&self, id: u64) -> Result<TerminalPasteStatus> {
        let state = self.paste_state.lock();
        let job = state
            .job
            .as_ref()
            .filter(|job| job.id == id)
            .context("paste not found")?;
        if !job.finished.load(Ordering::Acquire)
            && *self.events.status.read() != SessionStatus::Running
        {
            *job.error.lock() = Some("terminal process exited".to_owned());
        }
        Ok(job.status())
    }

    pub(super) fn cancel_paste(&self, id: u64) -> Result<TerminalPasteStatus> {
        let mut state = self.paste_state.lock();
        anyhow::ensure!(
            state.job.as_ref().is_some_and(|job| job.id == id),
            "paste not found"
        );
        self.finish_paste_locked(&mut state, true)?;
        Ok(state.job.as_ref().unwrap().status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        session: Arc<TerminalSession>,
        directory: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.session.terminate();
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
    fn wait(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !condition() {
            assert!(Instant::now() < deadline, "paste fixture timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn fixture(delay: f64, read_after: bool) -> Fixture {
        let directory =
            std::env::temp_dir().join(format!("eggie-paste-test-{}", SessionId::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("reader.py"),
            format!(
                r#"import os,sys,tty,termios,time
saved=termios.tcgetattr(0)
tty.setraw(0)
os.write(1,b'\x1b[?2004hPASTE_READY')
time.sleep({delay})
data=b''
while b'\x1b[201~' not in data:
    data+=os.read(0,65536)
start,payload=data.split(b'\x1b[200~',1)
body,end,after=payload.partition(b'\x1b[201~')
open('payload.tmp','wb').write(body)
os.replace('payload.tmp','payload')
if {read_after}:
    while len(after)<5: after+=os.read(0,65536)
open('after.tmp','wb').write(after)
os.replace('after.tmp','after')
termios.tcsetattr(0,termios.TCSANOW,saved)
os.write(1,b'\x1b[?2004lPASTE_DONE\r\n')
"#,
                read_after = if read_after { "True" } else { "False" }
            ),
        )
        .unwrap();
        let session = Arc::new(
            TerminalSession::spawn(SessionSpawnConfig {
                project_id: ProjectId::new_v4(),
                cwd: directory.clone(),
                size: TerminalSize::default(),
                appearance: TerminalAppearance::default(),
                scrollback_limit: TERMINAL_SCROLLBACK_LIMIT,
                shell_program: Some("/usr/bin/python3".to_owned()),
                shell_args: Some(vec!["-u".to_owned(), "reader.py".to_owned()]),
                shell_features: String::new(),
                profile_env: BTreeMap::new(),
                remote: None,
                remote_install_consented: false,
                remote_cwd: None,
                event_bus: None,
            })
            .unwrap(),
        );
        wait(|| session.paste_context().bracketed);
        Fixture { session, directory }
    }

    #[test]
    fn large_paste_writes_one_delimiter_pair_and_preserves_unicode() {
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let fixture = fixture(0., false);
        let text = "中文👩🏽‍💻\r\n\tvalue\u{3}\n".repeat(6000);
        let expected = eggie_protocol::sanitize_paste(&text);
        let status = fixture
            .session
            .paste_text(text, 2, Some(fixture.session.paste_context()))
            .unwrap();
        assert_eq!(status.total_bytes, expected.len());
        wait(|| fixture.session.paste_status(2).unwrap().phase == TerminalPastePhase::Completed);
        wait(|| fixture.directory.join("after").exists());
        assert_eq!(
            fs::read(fixture.directory.join("payload")).unwrap(),
            expected.as_bytes()
        );
        let status = fixture.session.paste_status(2).unwrap();
        assert_eq!(status.written_bytes, expected.len());
    }

    #[test]
    fn cancelling_paste_keeps_following_input_outside_the_delimiters() {
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let fixture = fixture(0.3, true);
        let text = "中文👩🏽‍💻".repeat(100000);
        fixture.session.paste_text(text.clone(), 2, None).unwrap();
        wait(|| fixture.session.paste_status(2).unwrap().written_bytes > 0);
        let status = fixture.session.cancel_paste(2).unwrap();
        assert_eq!(status.phase, TerminalPastePhase::Cancelled);
        fixture.session.input(b"after".to_vec(), 3).unwrap();
        wait(|| fixture.directory.join("after").exists());
        let payload = fs::read(fixture.directory.join("payload")).unwrap();
        assert!(payload.len() < text.len());
        assert!(text.as_bytes().starts_with(&payload));
        assert!(std::str::from_utf8(&payload).is_ok());
        assert_eq!(fs::read(fixture.directory.join("after")).unwrap(), b"after");
    }

    #[test]
    fn terminal_replies_cannot_become_part_of_pasted_text() {
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let fixture = fixture(0.3, true);
        let text = "a".repeat(128 * 1024);
        fixture.session.paste_text(text.clone(), 2, None).unwrap();
        wait(|| fixture.session.paste_status(2).unwrap().written_bytes > 0);
        fixture.session.events.write_to_pty(b"reply".to_vec());
        wait(|| fixture.directory.join("after").exists());
        assert_eq!(
            fs::read(fixture.directory.join("payload")).unwrap(),
            text.as_bytes()
        );
        assert_eq!(fs::read(fixture.directory.join("after")).unwrap(), b"reply");
    }

    #[test]
    fn typing_during_paste_cancels_the_remainder_without_interleaving() {
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let fixture = fixture(0.3, true);
        fixture
            .session
            .paste_text("a".repeat(2 * 1024 * 1024), 2, None)
            .unwrap();
        fixture.session.input(b"after".to_vec(), 3).unwrap();
        wait(|| fixture.directory.join("after").exists());
        assert_eq!(
            fixture.session.paste_status(2).unwrap().phase,
            TerminalPastePhase::Cancelled
        );
        assert_eq!(fs::read(fixture.directory.join("after")).unwrap(), b"after");
    }

    #[test]
    fn preview_cannot_paste_into_a_different_application() {
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let fixture = fixture(0., false);
        let mut old = fixture.session.paste_context();
        old.bracketed = false;
        assert!(
            fixture
                .session
                .paste_text("do not send".to_owned(), 2, Some(old))
                .is_err()
        );
        assert!(!fixture.directory.join("payload").exists());
    }

    #[test]
    fn exiting_process_ends_paste_monitoring() {
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let fixture = fixture(1., false);
        fixture
            .session
            .paste_text("a".repeat(2 * 1024 * 1024), 2, None)
            .unwrap();
        unsafe {
            assert_eq!(
                libc::kill(fixture.session.shell_pid as i32, libc::SIGKILL),
                0
            );
        }
        wait(|| fixture.session.paste_status(2).unwrap().phase == TerminalPastePhase::Failed);
    }
}
