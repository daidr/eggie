//! OSC 7501 program status reports. The parser validates a whole report before the tracker changes.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use eggie_protocol::{TerminalProgramBlockKind, TerminalProgramState, TerminalProgramStatus};
use std::collections::HashMap;

const MAX_BODY_BYTES: usize = 4096 - b"\x1b]7501;\x1b\\".len();
const MAX_RECORDS: usize = 256;

pub(crate) enum ProgramStatusCommand {
    Query,
    Report {
        id: String,
        status: Option<TerminalProgramStatus>,
    },
}

pub(crate) fn parse(body: &str) -> Option<ProgramStatusCommand> {
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    if body == "?" {
        return Some(ProgramStatusCommand::Query);
    }

    let (mut state, mut id, mut kind, mut progress, mut app, mut title, mut message) =
        (None, None, None, None, None, None, None);
    for pair in body.split(':') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = raw_key.trim_ascii();
        let value = raw_value.trim_ascii();
        if key.len() > 16 {
            return None;
        }
        match key {
            "id" => {
                if !valid_id(value) {
                    return None;
                }
                id = Some(value);
            }
            "app" if value.len() > 32 => return None,
            "title" if value.len() > 256 => return None,
            "msg" if value.len() > 2732 => return None,
            _ => {}
        }
        if !key.bytes().all(|byte| byte.is_ascii_lowercase())
            || !value.bytes().all(valid_value_byte)
        {
            continue;
        }
        match key {
            "state" => state = Some(value),
            "kind" => kind = Some(value),
            "progress" => progress = Some(value),
            "app" => app = Some(value),
            "title" => title = Some(value),
            "msg" => message = Some(value),
            _ => {}
        }
    }

    let id = id.unwrap_or("").to_owned();
    let title = decode_text(title, 192)?;
    let message = decode_text(message, 2048)?;
    if state == Some("clear") {
        return Some(ProgramStatusCommand::Report { id, status: None });
    }
    let state = match state? {
        "idle" => TerminalProgramState::Idle,
        "working" => TerminalProgramState::Working,
        "done" => TerminalProgramState::Done,
        "blocked" => TerminalProgramState::Blocked,
        "error" => TerminalProgramState::Error,
        _ => return None,
    };
    let app = app.filter(|value| valid_name(value)).map(str::to_owned);
    let kind = if state == TerminalProgramState::Blocked {
        match kind {
            Some("permission") => Some(TerminalProgramBlockKind::Permission),
            Some("question") => Some(TerminalProgramBlockKind::Question),
            Some("auth") => Some(TerminalProgramBlockKind::Auth),
            _ => None,
        }
    } else {
        None
    };
    let progress = if matches!(
        state,
        TerminalProgramState::Working | TerminalProgramState::Blocked
    ) {
        progress
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|value| *value <= 100)
    } else {
        None
    };
    Some(ProgramStatusCommand::Report {
        id,
        status: Some(TerminalProgramStatus {
            state,
            kind,
            app,
            title,
            message,
            progress,
        }),
    })
}

fn valid_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
}

fn valid_id(value: &str) -> bool {
    value.len() <= 128
        && value.split('/').count() <= 8
        && value
            .split('/')
            .all(|segment| segment.len() <= 32 && valid_name(segment))
}

fn decode_text(value: Option<&str>, max_decoded: usize) -> Option<Option<String>> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Some(None);
    };
    let bytes = if value.ends_with('=') {
        STANDARD.decode(value).ok()?
    } else {
        STANDARD_NO_PAD.decode(value).ok()?
    };
    if bytes.len() > max_decoded {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    if text
        .chars()
        .any(|ch| matches!(ch as u32, 0..=0x1f | 0x7f..=0x9f))
    {
        return None;
    }
    // Direction controls can make status text appear to impersonate adjacent interface labels.
    let text = text.chars().filter(|ch| {
        !matches!(*ch as u32, 0x061c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
    }).collect::<String>();
    Some((!text.is_empty()).then_some(text))
}

struct Entry {
    status: TerminalProgramStatus,
    updated: u64,
}

#[derive(Default)]
pub(crate) struct ProgramStatusTracker {
    records: HashMap<String, Entry>,
    revision: u64,
}

impl ProgramStatusTracker {
    pub(crate) fn apply(&mut self, id: String, status: Option<TerminalProgramStatus>) {
        let Some(status) = status else {
            if id.is_empty() {
                self.records.clear();
            } else {
                self.records
                    .retain(|key, _| key != &id && !key.starts_with(&format!("{id}/")));
            }
            return;
        };
        self.revision += 1;
        if !self.records.contains_key(&id) && self.records.len() >= MAX_RECORDS {
            if let Some(oldest) = self
                .records
                .iter()
                .min_by_key(|(_, entry)| entry.updated)
                .map(|(id, _)| id.clone())
            {
                self.records.remove(&oldest);
            }
        }
        self.records.insert(
            id,
            Entry {
                status,
                updated: self.revision,
            },
        );
    }

    pub(crate) fn clear_active(&mut self) {
        self.records.retain(|_, entry| {
            matches!(
                entry.status.state,
                TerminalProgramState::Done | TerminalProgramState::Error
            )
        });
    }

    pub(crate) fn clear_all(&mut self) {
        self.records.clear();
    }

    pub(crate) fn display(&self) -> Option<TerminalProgramStatus> {
        let (id, entry) = self
            .records
            .iter()
            .max_by_key(|(_, entry)| (priority(entry.status.state), entry.updated))?;
        let mut status = entry.status.clone();
        if status.app.is_none() {
            let mut parent = id.as_str();
            loop {
                parent = parent.rsplit_once('/').map_or("", |(prefix, _)| prefix);
                if let Some(app) = self
                    .records
                    .get(parent)
                    .and_then(|entry| entry.status.app.as_ref())
                {
                    status.app = Some(app.clone());
                    break;
                }
                if parent.is_empty() {
                    break;
                }
            }
        }
        Some(status)
    }
}

fn priority(state: TerminalProgramState) -> u8 {
    match state {
        TerminalProgramState::Idle => 0,
        TerminalProgramState::Working => 1,
        TerminalProgramState::Done => 2,
        TerminalProgramState::Error => 3,
        TerminalProgramState::Blocked => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(body: &str) -> (String, Option<TerminalProgramStatus>) {
        match parse(body).expect("valid report") {
            ProgramStatusCommand::Report { id, status } => (id, status),
            ProgramStatusCommand::Query => panic!("unexpected query"),
        }
    }

    #[test]
    fn validates_reports_before_changing_records() {
        assert!(matches!(parse("?"), Some(ProgramStatusCommand::Query)));
        assert!(parse("state=blocked:id=a//b").is_none());
        assert!(parse("state=working:msg=YQpi").is_none());
        assert!(parse("state=done:msg=QQ=").is_none());
        assert!(parse("state=clear:msg=YQpi").is_none());
        assert!(parse("state=future").is_none());
        assert!(parse(&format!("state=idle:msg={}", "A".repeat(2733))).is_none());
        let (_, status) = report(
            "state=idle:future=yes:oops:state=blocked:kind=permission:progress=40:msg=SGVsbG8",
        );
        let status = status.unwrap();
        assert_eq!(status.state, TerminalProgramState::Blocked);
        assert_eq!(status.kind, Some(TerminalProgramBlockKind::Permission));
        assert_eq!(status.progress, Some(40));
        assert_eq!(status.message.as_deref(), Some("Hello"));
    }

    #[test]
    fn child_priority_inheritance_clear_and_lifetime() {
        let mut tracker = ProgramStatusTracker::default();
        let (id, status) = report("state=idle:app=claude-code");
        tracker.apply(id, status);
        let (id, status) = report("state=working:id=build:msg=QnVpbGRpbmc=");
        tracker.apply(id, status);
        let (id, status) = report("state=blocked:id=build/test:kind=question:msg=QW5zd2VyPw==");
        tracker.apply(id, status);
        let display = tracker.display().unwrap();
        assert_eq!(display.state, TerminalProgramState::Blocked);
        assert_eq!(display.app.as_deref(), Some("claude-code"));
        tracker.clear_active();
        assert!(tracker.display().is_none());
        let (id, status) = report("state=done:id=build");
        tracker.apply(id, status);
        tracker.clear_active();
        assert_eq!(tracker.display().unwrap().state, TerminalProgramState::Done);
        let (id, status) = report("state=clear:id=build");
        tracker.apply(id, status);
        assert!(tracker.display().is_none());
    }

    #[test]
    fn clear_removes_descendants_and_capacity_evicts_least_recent_record() {
        let mut tracker = ProgramStatusTracker::default();
        for body in [
            "state=done:id=build",
            "state=error:id=build/test",
            "state=done:id=deploy",
        ] {
            let (id, status) = report(body);
            tracker.apply(id, status);
        }
        let (id, status) = report("state=clear:id=build");
        tracker.apply(id, status);
        assert_eq!(tracker.records.len(), 1);
        assert!(tracker.records.contains_key("deploy"));

        for index in 0..255 {
            let (id, status) = report(&format!("state=done:id=job{index}"));
            tracker.apply(id, status);
        }
        assert_eq!(tracker.records.len(), MAX_RECORDS);
        let (id, status) = report("state=done:id=deploy");
        tracker.apply(id, status);
        let (id, status) = report("state=done:id=extra");
        tracker.apply(id, status);
        assert_eq!(tracker.records.len(), MAX_RECORDS);
        assert!(tracker.records.contains_key("deploy"));
        assert!(!tracker.records.contains_key("job0"));
    }
}
