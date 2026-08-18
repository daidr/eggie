//! Stateless parsing/sanitizing helpers for OSC, DCS, and kitty/iterm2 sequence handling.
//!
//! Pure functions (percent-decoding, base64, metadata splitting, path/name sanitizing, clipboard
//! selection mapping). Extracted verbatim from `lib.rs`; callers in `ListenerState` reach them via
//! the crate-root `use osc_util::*` re-export, and tests via `use super::*`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use alacritty_terminal::term::ClipboardType;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use eggie_domain::SessionId;
use eggie_protocol::{TerminalClipboardContent, TerminalClipboardSelection, TerminalReportedLocation};

/// Resolve a command line from an OSC 133 prompt's options, accepting three encodings in
/// priority order: `cmdline_b64` (base64, robust for CJK/newlines/control bytes and what the
/// zsh integration emits), `cmdline_url` (percent-encoded), then raw `cmdline`.
pub(crate) fn command_line_from_options(options: &str) -> Option<String> {
    option_value(options, "cmdline_b64")
        .and_then(|value| decode_base64_text(&value))
        .or_else(|| option_value(options, "cmdline_url").map(percent_decode))
        .or_else(|| option_value(options, "cmdline"))
        .filter(|line| !line.is_empty())
}

pub(crate) fn option_value(options: &str, key: &str) -> Option<String> {
    options.split(';').find_map(|option| {
        let (candidate, value) = option.split_once('=')?;
        (candidate == key).then(|| value.to_owned())
    })
}

pub(crate) fn percent_decode(value: String) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(crate) fn parse_reported_location(value: &str) -> Option<TerminalReportedLocation> {
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix("file://") {
        let (authority, path) = rest
            .split_once('/')
            .map_or((rest, ""), |(host, path)| (host, path));
        let path = percent_decode(format!("/{path}"));
        let (user, host) = authority
            .split_once('@')
            .map_or((None, authority), |(user, host)| {
                (Some(user.to_owned()), host)
            });
        let host = (!host.is_empty()).then(|| host.to_owned());
        return Some(TerminalReportedLocation {
            user,
            local: host.as_deref().is_none_or(host_is_local),
            host,
            path,
        });
    }
    Some(TerminalReportedLocation {
        user: None,
        host: None,
        path: percent_decode(value.to_owned()),
        local: true,
    })
}

pub(crate) fn host_is_local(host: &str) -> bool {
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case(local_hostname())
        || host
            .split_once('.')
            .map(|(short, _)| short)
            .is_some_and(|short| short.eq_ignore_ascii_case(local_hostname()))
}

pub(crate) fn local_hostname() -> &'static str {
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME.get_or_init(|| {
        let mut buffer = [0_u8; 256];
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if result != 0 {
            return String::new();
        }
        let end = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    })
}

pub(crate) fn clipboard_selection(clipboard: ClipboardType) -> TerminalClipboardSelection {
    match clipboard {
        ClipboardType::Clipboard => TerminalClipboardSelection::Clipboard,
        ClipboardType::Selection => TerminalClipboardSelection::Primary,
    }
}

pub(crate) fn generated_notification_id(session_id: SessionId, sequence: &AtomicU64) -> String {
    format!(
        "{}-{}",
        session_id,
        sequence.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn colon_metadata(metadata: &str) -> HashMap<String, String> {
    metadata
        .split(':')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

pub(crate) fn decode_base64_text(value: &str) -> Option<String> {
    BASE64
        .decode(value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

pub(crate) fn sanitize_protocol_id(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "-_+.".contains(*character))
        .take(256)
        .collect()
}

pub(crate) fn protocol_id_field(id: &str) -> String {
    if id.is_empty() {
        String::new()
    } else {
        format!(":id={id}")
    }
}

pub(crate) fn filter_clipboard_contents(
    contents: Vec<TerminalClipboardContent>,
    requested_mime_types: &[String],
) -> Vec<TerminalClipboardContent> {
    contents
        .into_iter()
        .filter(|content| {
            let base = content
                .mime_type
                .split(';')
                .next()
                .unwrap_or(&content.mime_type);
            requested_mime_types.iter().any(|requested| {
                let requested_base = requested.split(';').next().unwrap_or(requested);
                requested == "."
                    || requested == "*/*"
                    || requested == &content.mime_type
                    || requested_base == base
                    || requested
                        .strip_suffix("/*")
                        .is_some_and(|prefix| base.starts_with(prefix))
            })
        })
        .collect()
}

pub(crate) fn sanitize_osc_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(limit)
        .collect()
}

pub(crate) fn append_sanitized_osc_text(output: &mut String, value: &str, limit: usize) {
    let remaining = limit.saturating_sub(output.chars().count());
    output.extend(
        value
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .take(remaining),
    );
}

pub(crate) fn sanitize_file_name(value: &str) -> String {
    let mut name = value
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .take(255)
        .collect::<String>();
    if name.is_empty() || name == "." || name == ".." {
        name = "terminal-download".to_owned();
    }
    name
}

pub(crate) fn safe_kitty_transfer_path(encoded_name: Option<&str>, file_id: &str) -> PathBuf {
    let decoded = encoded_name
        .and_then(|name| BASE64.decode(name).ok())
        .and_then(|name| String::from_utf8(name).ok());
    let Some(decoded) = decoded else {
        return PathBuf::from(sanitize_file_name(file_id));
    };
    let mut safe = PathBuf::new();
    let mut total_length = 0_usize;
    for component in Path::new(&decoded).components().take(64) {
        match component {
            std::path::Component::Normal(component) => {
                let component = sanitize_file_name(&component.to_string_lossy());
                total_length = total_length.saturating_add(component.len());
                if total_length > 4_096 {
                    break;
                }
                safe.push(component);
            }
            std::path::Component::ParentDir => {
                return PathBuf::from(sanitize_file_name(file_id));
            }
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::CurDir => {}
        }
    }
    if safe.as_os_str().is_empty() {
        PathBuf::from(sanitize_file_name(file_id))
    } else {
        safe
    }
}

pub(crate) fn iterm2_file_metadata(metadata: &str) -> (String, bool) {
    let fields = metadata
        .split(';')
        .filter_map(|field| field.split_once('='))
        .collect::<HashMap<_, _>>();
    let suggested_name = fields
        .get("name")
        .and_then(|name| BASE64.decode(name).ok())
        .and_then(|name| String::from_utf8(name).ok())
        .and_then(|name| {
            Path::new(&name)
                .file_name()
                .and_then(|name| name.to_str())
                .map(sanitize_file_name)
        })
        .unwrap_or_else(|| "terminal-download".to_owned());
    (suggested_name, fields.get("inline").copied() == Some("1"))
}
