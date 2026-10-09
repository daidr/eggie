use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eggie_domain::SessionId;
use eggie_protocol::{EventRecord, EventUpdate};
use parking_lot::{Condvar, Mutex};

const HISTORY: usize = 1024;

pub(super) struct EventBus {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    sequence: u64,
    events: VecDeque<EventRecord>,
}

impl EventBus {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                sequence: 0,
                events: VecDeque::with_capacity(HISTORY),
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn publish(
        &self,
        name: &str,
        session_id: Option<SessionId>,
        payload: serde_json::Value,
    ) {
        let mut state = self.state.lock();
        state.sequence = state.sequence.wrapping_add(1);
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        let sequence = state.sequence;
        state.events.push_back(EventRecord {
            sequence,
            timestamp_ms,
            name: name.to_owned(),
            session_id,
            payload,
        });
        if state.events.len() > HISTORY {
            state.events.pop_front();
        }
        self.changed.notify_all();
    }

    pub(super) fn sequence(&self) -> u64 {
        self.state.lock().sequence
    }

    pub(super) fn wait_after(
        &self,
        after: u64,
        session_id: Option<SessionId>,
        timeout: Duration,
    ) -> Option<EventUpdate> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        while state.sequence <= after {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            self.changed.wait_for(&mut state, deadline - now);
        }
        let gap = state
            .events
            .front()
            .is_some_and(|first| after.saturating_add(1) < first.sequence);
        Some(EventUpdate {
            sequence: state.sequence,
            gap,
            events: state
                .events
                .iter()
                .filter(|event| {
                    event.sequence > after
                        && session_id.is_none_or(|id| event.session_id == Some(id))
                })
                .cloned()
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_and_gap_are_explicit() {
        let bus = EventBus::new();
        for index in 0..HISTORY + 1 {
            bus.publish("terminal.bell", None, serde_json::json!({ "index": index }));
        }
        let update = bus.wait_after(0, None, Duration::ZERO).unwrap();
        assert!(update.gap);
        assert_eq!(update.events.len(), HISTORY);
        assert_eq!(update.sequence, (HISTORY + 1) as u64);
    }
}
