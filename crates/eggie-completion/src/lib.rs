//! Shell-independent completion contracts, parsing, ranking and bounded local providers.
mod context;
#[cfg(feature = "local-providers")]
mod providers;
pub use context::*;
#[cfg(feature = "local-providers")]
pub use providers::CompletionEngine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MAX_INPUT: usize = 32_768;
pub const MAX_CANDIDATES: usize = 2_000;
pub const NATIVE_REQUEST: &[u8] = b"\x18\x1f";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptInput {
    pub text: String,
    /// UTF-8 byte offset, always a character boundary.
    pub cursor: usize,
    pub shell: String,
    pub cwd: PathBuf,
    pub search_path: String,
    pub home: PathBuf,
    pub native: bool,
    #[serde(default)]
    pub native_revision: u64,
}
impl PromptInput {
    pub fn valid(&self) -> bool {
        self.text.len() <= MAX_INPUT
            && self.text.is_char_boundary(self.cursor)
            && !self.text.contains(['\x1b', '\0', '\x07'])
    }
    pub fn same_edit(&self, other: &Self) -> bool {
        self.text == other.text && self.cursor == other.cursor && self.cwd == other.cwd
    }
    /// Builtin shell encoders escape delimiters and controls without a process per keystroke.
    pub fn from_report(report: &str) -> Option<Self> {
        if report.len() > MAX_INPUT * 6 {
            return None;
        }
        let fields: Vec<_> = report.splitn(7, ';').collect();
        if fields.len() != 7 {
            return None;
        }
        let text = percent_decode(fields[6])?;
        let chars: usize = fields[1].parse().ok()?;
        let cursor = text
            .char_indices()
            .nth(chars)
            .map_or(text.len(), |(i, _)| i);
        if chars > text.chars().count() {
            return None;
        }
        let input = Self {
            text,
            cursor,
            shell: fields[0].into(),
            cwd: percent_decode(fields[2])?.into(),
            search_path: percent_decode(fields[3])?,
            home: percent_decode(fields[4])?.into(),
            native: fields[5] == "1",
            native_revision: 0,
        };
        input.valid().then_some(input)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    Command,
    Option,
    Argument,
    File,
    Directory,
    History,
    Native,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub description: String,
    pub kind: CompletionKind,
    pub replace_start: usize,
    pub replace_end: usize,
}
impl CompletionItem {
    pub fn replacement(&self, input: &PromptInput) -> Option<Vec<u8>> {
        use unicode_segmentation::UnicodeSegmentation;
        if !input.valid()
            || self.replace_start > input.cursor
            || self.replace_end < input.cursor
            || !input.text.is_char_boundary(self.replace_start)
            || !input.text.is_char_boundary(self.replace_end)
            || self.insert_text.chars().any(char::is_control)
        {
            return None;
        }
        if input.native {
            let cursor = input.text[..input.cursor].chars().count();
            let start = input.text[..self.replace_start].chars().count();
            let end = input.text[..self.replace_end].chars().count();
            let terminator = if input.shell == "fish" { '\0' } else { '\n' };
            return Some(
                format!(
                    "\x18\x1e{};{cursor};{start};{end};{}{terminator}",
                    percent_encode(&input.text),
                    percent_encode(&self.insert_text)
                )
                .into_bytes(),
            );
        }
        let right = input.text[input.cursor..self.replace_end]
            .graphemes(true)
            .count();
        let left = input.text[self.replace_start..input.cursor]
            .graphemes(true)
            .count();
        let mut bytes = Vec::new();
        for _ in 0..right {
            bytes.extend_from_slice(b"\x1b[3~");
        }
        bytes.extend(std::iter::repeat_n(0x7f, left));
        bytes.extend_from_slice(self.insert_text.as_bytes());
        Some(bytes)
    }
}
pub fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut it = text.bytes();
    while let Some(b) = it.next() {
        bytes.push(if b == b'%' {
            let hi = char::from(it.next()?).to_digit(16)?;
            let lo = char::from(it.next()?).to_digit(16)?;
            (hi * 16 + lo) as u8
        } else {
            b
        });
    }
    String::from_utf8(bytes).ok()
}
/// Deterministic fuzzy ranking; prefix matches win, then compact subsequence matches.
pub fn rank(items: Vec<CompletionItem>, query: &str) -> Vec<CompletionItem> {
    let query = query.to_lowercase();
    let mut ranked: Vec<_> = items
        .into_iter()
        .filter_map(|item| match_score(&item.label, &query).map(|score| (score, item)))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    let mut seen = std::collections::HashSet::new();
    ranked
        .into_iter()
        .map(|(_, i)| i)
        .filter(|i| seen.insert((i.insert_text.clone(), i.replace_start, i.replace_end)))
        .take(MAX_CANDIDATES)
        .collect()
}

/// Refilter an immutable candidate pool without cloning labels, descriptions or shell edits.
pub fn rank_indices(items: &[CompletionItem], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    let mut ranked: Vec<_> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match_score(&item.label, &query).map(|score| (score, i)))
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| items[a.1].label.cmp(&items[b.1].label))
    });
    let mut seen = std::collections::HashSet::new();
    ranked
        .into_iter()
        .map(|(_, i)| i)
        .filter(|&i| {
            let item = &items[i];
            seen.insert((
                item.insert_text.as_str(),
                item.replace_start,
                item.replace_end,
            ))
        })
        .take(MAX_CANDIDATES)
        .collect()
}

fn match_score(label: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    if label.starts_with(query) {
        return Some(10_000 - label.len() as i64);
    }
    let mut search = label.char_indices();
    let mut first = None;
    let mut last = 0;
    for c in query.chars() {
        let (i, _) = search.find(|(_, v)| *v == c)?;
        first.get_or_insert(i);
        last = i;
    }
    Some(1_000 - (last - first.unwrap_or(0)) as i64 - label.len() as i64)
}

pub fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/_-. ~:@=+".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn borrowed_ranking_preserves_fuzzy_unicode_order_and_deduplication() {
        let context = CompletionContext::parse(&PromptInput::default()).unwrap();
        let items = [
            "FooBar",
            "fb",
            "foo-bar",
            "fb",
            "中文.txt",
            "中文.md",
            "zebra",
        ]
        .map(|name| context.item(name, "description", CompletionKind::Command));
        for query in ["", "fb", "FO", "中", "文t", "absent"] {
            let indices = rank_indices(&items, query);
            let ranked: Vec<_> = indices.iter().map(|&i| items[i].clone()).collect();
            assert_eq!(ranked, rank(items.to_vec(), query), "{query}");
        }
    }

    #[test]
    fn native_edit_keeps_multiline_unicode_and_quoted_suffix_as_an_atomic_checked_edit() {
        let text = "echo 中\ncat 'a b.txt' tail";
        let input = PromptInput {
            text: text.into(),
            cursor: "echo 中\ncat 'a".len(),
            native: true,
            ..Default::default()
        };
        let context = CompletionContext::parse(&input).unwrap();
        let item = context.item("a new.txt", "", CompletionKind::File);
        let packet = String::from_utf8(item.replacement(&input).unwrap()).unwrap();
        assert!(packet.starts_with("\x18\x1e"));
        let fields: Vec<_> = packet[2..].trim_end_matches('\n').split(';').collect();
        assert_eq!(percent_decode(fields[0]).as_deref(), Some(text));
        assert_eq!(
            fields[1],
            input.text[..input.cursor].chars().count().to_string()
        );
        assert_eq!(percent_decode(fields[4]).as_deref(), Some("'a new.txt'"));
        let start: usize = fields[2].parse().unwrap();
        let end: usize = fields[3].parse().unwrap();
        let left: String = text.chars().take(start).collect();
        let right: String = text.chars().skip(end).collect();
        assert_eq!(
            format!("{left}{}{right}", percent_decode(fields[4]).unwrap()),
            "echo 中\ncat 'a new.txt' tail"
        );
    }

    #[test]
    fn malformed_reports_and_control_characters_never_become_terminal_input() {
        for report in [
            "zsh;9;/;/bin;/;1;abc",
            "zsh;0;/;/bin;/;1;%GG",
            "zsh;0;/;/bin;/;1;%1B",
        ] {
            assert!(PromptInput::from_report(report).is_none());
        }
        let input = PromptInput {
            text: "cat x".into(),
            cursor: 5,
            native: true,
            ..Default::default()
        };
        let context = CompletionContext::parse(&input).unwrap();
        for text in ["x\t", "x\x03", "x\n", "x\x1b", "x\0"] {
            assert!(
                context
                    .item(text, "", CompletionKind::File)
                    .replacement(&input)
                    .is_none()
            );
        }
        let encoded = percent_encode("中文;\n'\\%$ ");
        assert_eq!(percent_decode(&encoded).as_deref(), Some("中文;\n'\\%$ "));
    }

    #[test]
    fn literal_quotes_inside_a_word_and_in_single_quotes_are_preserved() {
        for (text, query) in [
            ("cat 'a\"b'", "a\"b"),
            ("cat a\\\"b", "a\"b"),
            ("cat \"a'b\"", "a'b"),
        ] {
            let input = PromptInput {
                text: text.into(),
                cursor: text.len(),
                ..Default::default()
            };
            assert_eq!(CompletionContext::parse(&input).unwrap().query, query);
        }
    }
}

#[cfg(test)]
mod expansion_tests {
    use super::*;
    #[test]
    fn variable_paths_expand_only_when_the_user_typed_an_expansion() {
        for (text, expected) in [
            ("cat $HOME/a", "$HOME/a\\ file.txt"),
            ("cat \"$HOME/a", "\"$HOME/a file.txt\""),
            ("cat \\$HOME/a", "\\$HOME/a\\ file.txt"),
            ("cat '$HOME/a", "'$HOME/a file.txt'"),
        ] {
            let input = PromptInput {
                text: text.into(),
                cursor: text.len(),
                ..Default::default()
            };
            assert_eq!(
                CompletionContext::parse(&input)
                    .unwrap()
                    .item("$HOME/a file.txt", "", CompletionKind::File)
                    .insert_text,
                expected
            );
        }
    }
}
