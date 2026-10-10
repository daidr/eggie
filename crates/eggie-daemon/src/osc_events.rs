use super::*;

pub(super) struct OscEventTracker {
    pub(super) session_id: SessionId,
    pub(super) state: Mutex<OscEventTrackerState>,
    pub(super) changed: Condvar,
}

pub(super) struct OscEventTrackerState {
    pub(super) revision: u64,
    pub(super) events: VecDeque<TerminalOscEvent>,
}

impl OscEventTracker {
    pub(super) fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: Mutex::new(OscEventTrackerState {
                revision: 0,
                events: VecDeque::with_capacity(OSC_EVENT_HISTORY),
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn push(&self, payload: TerminalOscEventPayload) {
        let mut state = self.state.lock();
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        state
            .events
            .push_back(TerminalOscEvent { revision, payload });
        if state.events.len() > OSC_EVENT_HISTORY {
            state.events.pop_front();
        }
        self.changed.notify_all();
    }

    pub(super) fn wait_after(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalOscEventUpdate> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        while state.revision <= after_revision {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            self.changed.wait_for(&mut state, deadline - now);
        }
        let revision = state.revision;
        let events = state
            .events
            .iter()
            .filter(|event| event.revision > after_revision)
            .cloned()
            .collect();
        Some(TerminalOscEventUpdate {
            session_id: self.session_id,
            revision,
            events,
        })
    }

    pub(super) fn revision(&self) -> u64 {
        self.state.lock().revision
    }
}
