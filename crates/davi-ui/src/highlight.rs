//! Single-pass JSON line highlighter producing GPUI highlight ranges.
//!
//! It runs on pretty-printed output one line at a time, so it needs no
//! cross-line state and can be computed on a background thread. A tree-sitter
//! based highlighter can later implement the same `highlight_line` contract
//! for XML/HTML.

use std::ops::Range;

use gpui::HighlightStyle;

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Key,
    String,
    Number,
    Keyword,
    Punct,
}

impl Token {
    fn style(self) -> HighlightStyle {
        let color = match self {
            Token::Key => theme::SYN_KEY,
            Token::String => theme::SYN_STRING,
            Token::Number => theme::SYN_NUMBER,
            Token::Keyword => theme::SYN_KEYWORD,
            Token::Punct => theme::SYN_PUNCT,
        };
        HighlightStyle {
            color: Some(theme::c(color).into()),
            ..Default::default()
        }
    }
}

pub fn tokenize_json_line(line: &str) -> Vec<(Range<usize>, Token)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(bytes.len());
                let rest = line[i..].trim_start();
                let kind = if rest.starts_with(':') {
                    Token::Key
                } else {
                    Token::String
                };
                out.push((start..i, kind));
            }
            b'-' | b'0'..=b'9' => {
                while i < bytes.len()
                    && matches!(bytes[i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    i += 1;
                }
                out.push((start..i, Token::Number));
            }
            b't' | b'f' | b'n' => {
                let word = ["true", "false", "null"]
                    .into_iter()
                    .find(|w| line[i..].starts_with(w));
                i += word.map_or(1, str::len);
                if word.is_some() {
                    out.push((start..i, Token::Keyword));
                }
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                i += 1;
                out.push((start..i, Token::Punct));
            }
            _ => i += 1,
        }
    }
    out
}

pub fn highlight_json_line(line: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    tokenize_json_line(line)
        .into_iter()
        .map(|(range, token)| (range, token.style()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_tokens() {
        let line = r#"  "id": 42, "ok": true, "name": "a \"q\"", "x": null"#;
        let kinds: Vec<_> = tokenize_json_line(line)
            .into_iter()
            .map(|(r, t)| (&line[r], t))
            .filter(|(_, t)| *t != Token::Punct)
            .collect();
        assert_eq!(
            kinds,
            [
                ("\"id\"", Token::Key),
                ("42", Token::Number),
                ("\"ok\"", Token::Key),
                ("true", Token::Keyword),
                ("\"name\"", Token::Key),
                ("\"a \\\"q\\\"\"", Token::String),
                ("\"x\"", Token::Key),
                ("null", Token::Keyword),
            ]
        );
    }

    #[test]
    fn ranges_stay_on_char_boundaries() {
        for line in ["\"é\": \"ü\"", "\"unterminated", "\"trailing\\"] {
            for (r, _) in tokenize_json_line(line) {
                assert!(line.get(r).is_some());
            }
        }
    }
}
