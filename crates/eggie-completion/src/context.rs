use crate::{CompletionItem, CompletionKind, PromptInput};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionContext {
    pub words: Vec<String>,
    pub query: String,
    pub start: usize,
    pub end: usize,
    pub quote: Option<char>,
    pub command_position: bool,
    pub expand_variable: bool,
}
impl CompletionContext {
    pub fn parse(input: &PromptInput) -> Option<Self> {
        if !input.valid() {
            return None;
        }
        let mut tokens: Vec<(usize, usize, String, Option<char>)> = Vec::new();
        let mut start = None;
        let mut value = String::new();
        let mut quote = None;
        let mut opening = None;
        let mut escaped = false;
        for (i, c) in input.text.char_indices() {
            if escaped {
                value.push(c);
                escaped = false;
                continue;
            }
            if c == '\\' && quote != Some('\'') {
                start.get_or_insert(i);
                escaped = true;
                continue;
            }
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                } else {
                    value.push(c);
                }
                continue;
            }
            if matches!(c, '\'' | '"') {
                start.get_or_insert(i);
                if value.is_empty() {
                    opening = Some(c);
                }
                quote = Some(c);
                continue;
            }
            if c.is_whitespace() || matches!(c, '|' | '&' | ';' | '<' | '>') {
                if let Some(s) = start.take() {
                    tokens.push((s, i, std::mem::take(&mut value), opening.take()));
                }
                if matches!(c, '|' | '&' | ';' | '\n') && i < input.cursor {
                    tokens.clear();
                }
            } else {
                start.get_or_insert(i);
                value.push(c);
            }
        }
        if let Some(s) = start {
            tokens.push((s, input.text.len(), value, opening));
        }
        let current = tokens
            .iter()
            .position(|(s, e, _, _)| *s <= input.cursor && input.cursor <= *e);
        let (start, end, opening) = current
            .map(|i| (tokens[i].0, tokens[i].1, tokens[i].3))
            .unwrap_or((input.cursor, input.cursor, None));
        let raw = &input.text[start..input.cursor];
        let mut query = String::new();
        let mut escaped = false;
        let mut active_quote = None;
        for c in raw.chars() {
            if escaped {
                query.push(c);
                escaped = false;
            } else if c == '\\' && active_quote != Some('\'') {
                escaped = true;
            } else if let Some(q) = active_quote {
                if c == q {
                    active_quote = None;
                } else {
                    query.push(c);
                }
            } else if matches!(c, '\'' | '"') {
                active_quote = Some(c);
            } else {
                query.push(c);
            }
        }
        let words = tokens
            .iter()
            .filter(|(_, e, _, _)| *e < start)
            .map(|(_, _, v, _)| v.clone())
            .collect::<Vec<_>>();
        let command_position =
            words.is_empty() || words.iter().all(|w| w.contains('=') && !w.starts_with('-'));
        Some(Self {
            words,
            query,
            start,
            end,
            quote: opening,
            command_position,
            expand_variable: raw.starts_with('$') || raw.starts_with("\"$"),
        })
    }
    pub fn item(&self, value: &str, description: &str, kind: CompletionKind) -> CompletionItem {
        let mut insert_text = quote_word(value, self.quote);
        if self.expand_variable && value.starts_with('$') {
            let prefix = value.split('/').next().unwrap();
            let name = prefix
                .strip_prefix("${")
                .and_then(|s| s.strip_suffix('}'))
                .or_else(|| prefix.strip_prefix('$'))
                .unwrap_or("");
            if !name.is_empty()
                && name.chars().enumerate().all(|(i, c)| {
                    c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()
                })
            {
                let offset = usize::from(self.quote == Some('"'));
                let escaped = if self.quote == Some('"') {
                    let quoted = quote_word(prefix, self.quote);
                    quoted[1..quoted.len() - 1].to_owned()
                } else {
                    quote_word(prefix, None)
                };
                insert_text.replace_range(offset..offset + escaped.len(), prefix);
            }
        }
        CompletionItem {
            label: value.into(),
            insert_text,
            description: description.into(),
            kind,
            replace_start: self.start,
            replace_end: self.end,
        }
    }
}
pub fn quote_word(value: &str, quote: Option<char>) -> String {
    match quote {
        Some('\'') => format!("'{}'", value.replace('\'', "'\\''")),
        Some('"') => format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$")
                .replace('`', "\\`")
        ),
        _ => value.chars().fold(String::new(), |mut out, c| {
            if !c.is_alphanumeric() && !"/_-.~:@%=+".contains(c) {
                out.push('\\');
            }
            out.push(c);
            out
        }),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input(s: &str, c: usize) -> PromptInput {
        PromptInput {
            text: s.into(),
            cursor: c,
            ..Default::default()
        }
    }
    #[test]
    fn parsing() {
        let c = CompletionContext::parse(&input("git checkout ma", 15)).unwrap();
        assert_eq!(c.words, ["git", "checkout"]);
        assert_eq!(c.query, "ma");
        assert_eq!((c.start, c.end), (13, 15));
        let c = CompletionContext::parse(&input("echo hi | git ch", 16)).unwrap();
        assert_eq!(c.words, ["git"]);
        let c = CompletionContext::parse(&input("cat \"中文 文件\"", "cat \"中文 ".len())).unwrap();
        assert_eq!(c.query, "中文 ");
        assert_eq!(c.end, "cat \"中文 文件\"".len());
        assert_eq!(quote_word("a b'c", Some('\'')), "'a b'\\''c'");
    }
    #[test]
    fn cursor_middle_preserves_suffix() {
        let i = input("git checXX tail", 8);
        let c = CompletionContext::parse(&i).unwrap();
        let item = c.item("checkout", "", CompletionKind::Argument);
        assert_eq!(item.replace_end, 10);
        assert_eq!(
            item.replacement(&i).unwrap(),
            b"\x1b[3~\x1b[3~\x7f\x7f\x7f\x7fcheckout"
        );
    }
    #[test]
    fn invalid_input_is_rejected() {
        assert!(CompletionContext::parse(&input("中", 1)).is_none());
    }
}
