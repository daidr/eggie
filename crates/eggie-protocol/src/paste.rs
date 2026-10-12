use super::*;

pub const LARGE_PASTE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPasteContext {
    pub bracketed: bool,
    pub alternate_screen: bool,
    pub prompt_editable: bool,
    pub paste_events: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalPastePhase {
    Sending,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPasteStatus {
    pub id: u64,
    pub total_bytes: usize,
    /// Streamed payload bytes written to the PTY, excluding delimiters. Short pastes and clipboard
    /// grants acknowledge acceptance without a progress subscription.
    pub written_bytes: usize,
    pub phase: TerminalPastePhase,
    pub error: Option<String>,
}

/// Preserve text and tabs, normalize line endings, and exclude terminal control characters.
pub fn sanitize_paste(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                result.push('\n');
            }
            '\n' | '\t' => result.push(character),
            character if !character.is_control() => result.push(character),
            _ => {}
        }
    }
    result
}

pub fn paste_has_controls(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
}

pub fn sanitize_paste_owned(text: String) -> String {
    if text.contains('\r') || paste_has_controls(&text) {
        sanitize_paste(&text)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_cannot_inject_control_keys_or_a_bracketed_paste_terminator() {
        assert_eq!(
            sanitize_paste("echo a\u{3}\u{1b}[201~\u{0}\u{7f}"),
            "echo a[201~"
        );
        assert!(paste_has_controls("a\u{3}"));
        assert!(!paste_has_controls("a\r\n\tb"));
    }

    #[test]
    fn paste_preserves_unicode_indentation_and_trailing_line_breaks() {
        assert_eq!(sanitize_paste("中文👩🏽‍💻\r\n\t  b\rc\n"), "中文👩🏽‍💻\n\t  b\nc\n");
    }
}
