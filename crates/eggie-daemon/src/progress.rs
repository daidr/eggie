use super::*;

pub(super) struct ProgressTracker {
    pub(super) session_id: SessionId,
    pub(super) state: Mutex<ProgressTrackerState>,
    pub(super) changed: Condvar,
}

pub(super) struct ProgressTrackerState {
    pub(super) authoritative: Option<TerminalProgress>,
    pub(super) authoritative_updated_at: Option<Instant>,
    pub(super) published: Option<TerminalProgress>,
    pub(super) revision: u64,
    pub(super) dirty: bool,
    pub(super) last_publish_at: Option<Instant>,
    pub(super) timeouts: TerminalProgressTimeouts,
}

impl ProgressTracker {
    pub(super) fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: Mutex::new(ProgressTrackerState {
                authoritative: None,
                authoritative_updated_at: None,
                published: None,
                revision: 0,
                dirty: false,
                last_publish_at: None,
                timeouts: TerminalProgressTimeouts::default(),
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn report(&self, report: Option<alacritty_terminal::vte::ansi::ProgressReport>) {
        let now = Instant::now();
        let mut state = self.state.lock();
        let progress = report.map(|report| TerminalProgress {
            state: match report.state {
                VteProgressState::Normal => TerminalProgressState::Normal,
                VteProgressState::Error => TerminalProgressState::Error,
                VteProgressState::Indeterminate => TerminalProgressState::Indeterminate,
                VteProgressState::Paused => TerminalProgressState::Paused,
            },
            percent: report.percent,
            updated_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        });

        // A clear while already clear is intentionally a no-op. RIS during startup should not
        // manufacture a progress revision, while every active report refreshes the idle timeout.
        if progress.is_none() && state.authoritative.is_none() {
            return;
        }
        state.authoritative = progress;
        state.authoritative_updated_at = progress.map(|_| now);
        state.dirty = true;
        self.changed.notify_all();
    }

    pub(super) fn set_timeouts(&self, timeouts: TerminalProgressTimeouts) {
        let mut state = self.state.lock();
        state.timeouts = TerminalProgressTimeouts {
            completed_ms: timeouts.completed_ms.max(100),
            stale_ms: timeouts.stale_ms.max(100),
        };
        self.changed.notify_all();
    }

    pub(super) fn wait_after(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalProgressUpdate> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        loop {
            let now = Instant::now();
            Self::expire_if_needed(&mut state, now);

            if state.revision > after_revision {
                return Some(TerminalProgressUpdate {
                    session_id: self.session_id,
                    revision: state.revision,
                    progress: state.published,
                });
            }

            let next_publish = state
                .last_publish_at
                .map_or(now, |last| last + PROGRESS_FRAME_INTERVAL);
            if state.dirty && now >= next_publish {
                state.published = state.authoritative;
                state.revision = state.revision.wrapping_add(1).max(1);
                state.dirty = false;
                state.last_publish_at = Some(now);
                self.changed.notify_all();
                return Some(TerminalProgressUpdate {
                    session_id: self.session_id,
                    revision: state.revision,
                    progress: state.published,
                });
            }

            if now >= deadline {
                return None;
            }
            let wake_at = Self::expiry_at(&state)
                .into_iter()
                .chain(state.dirty.then_some(next_publish))
                .chain(Some(deadline))
                .min()
                .unwrap_or(deadline);
            self.changed
                .wait_for(&mut state, wake_at.saturating_duration_since(now));
        }
    }

    pub(super) fn revision(&self) -> u64 {
        self.state.lock().revision
    }

    pub(super) fn expiry_at(state: &ProgressTrackerState) -> Option<Instant> {
        let progress = state.authoritative?;
        let updated_at = state.authoritative_updated_at?;
        let timeout_ms =
            if progress.state == TerminalProgressState::Normal && progress.percent == Some(100) {
                state.timeouts.completed_ms
            } else {
                state.timeouts.stale_ms
            };
        Some(updated_at + Duration::from_millis(u64::from(timeout_ms)))
    }

    pub(super) fn expire_if_needed(state: &mut ProgressTrackerState, now: Instant) {
        if Self::expiry_at(state).is_some_and(|expires| now >= expires) {
            state.authoritative = None;
            state.authoritative_updated_at = None;
            state.dirty = true;
        }
    }
}
