//! `.bru` parser.
//!
//! The grammar is line oriented, so the parser is split in two layers:
//! small `nom` combinators recognise the tokens of a single line (block
//! headers, `key: value` entries, list items), and a thin hand-written driver
//! walks the document line by line. This keeps parsing zero-copy until the
//! final AST is built and lets every error carry an exact line/column plus the
//! block it occurred in.
//!
//! ```text
//! file      := (blank* block)* blank* EOF
//! block     := name ws* ( '{' dict_body | '{' text_body | '[' list_body )
//! dict_body := NL ( blank | entry )* '}'
//! entry     := ws* '~'? key ws* ':' ws* value NL
//!            | ws* '~'? key ws* ':' ws* "'''" NL line* ws* "'''" NL
//! text_body := NL ( '  ' line NL )* '}'          -- closing brace at column 0
//! list_body := NL ( ws* item ','? NL )* ']'
//! ```

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_till, take_till1, take_while1},
    character::complete::{char, line_ending, not_line_ending, space0},
    combinator::{eof, opt, recognize},
    sequence::{delimited, terminated},
};

use super::ast::{Block, BlockBody, BlockKind, BruFile, block_kind};
use crate::error::ParseError;
use crate::model::KeyValue;

type Res<'a, T> = IResult<&'a str, T>;

const MULTILINE_QUOTE: &str = "'''";

/// Parse a `.bru` document into its block AST.
pub fn parse(source: &str) -> Result<BruFile, ParseError> {
    let src = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut cursor = Cursor { src, rest: src };
    let mut blocks = Vec::new();

    loop {
        cursor.skip_blank_lines();
        if cursor.rest.is_empty() {
            break;
        }
        blocks.push(cursor.block()?);
    }

    Ok(BruFile { blocks })
}

// ---------------------------------------------------------------------------
// Line-level combinators
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    Brace,
    Bracket,
}

fn block_name(i: &str) -> Res<'_, &str> {
    take_while1(|c: char| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.')).parse(i)
}

/// `name {` / `name [` up to and including the line terminator.
/// A `name []` on one line is accepted as an empty list.
fn block_header(i: &str) -> Res<'_, (&str, Opener, bool)> {
    let (i, name) = block_name(i)?;
    let (i, _) = space0(i)?;
    let (i, opener) = alt((
        char('{').map(|_| Opener::Brace),
        char('[').map(|_| Opener::Bracket),
    ))
    .parse(i)?;
    let (i, _) = space0(i)?;
    let (i, closed_inline) = opt(char(if opener == Opener::Brace { '}' } else { ']' }))
        .map(|c| c.is_some())
        .parse(i)?;
    let (i, _) = space0(i)?;
    let (i, _) = alt((line_ending, eof)).parse(i)?;
    Ok((i, (name, opener, closed_inline)))
}

/// One physical line without its terminator; consumes the terminator.
fn line(i: &str) -> Res<'_, &str> {
    terminated(not_line_ending, alt((line_ending, eof))).parse(i)
}

fn quoted_key(i: &str) -> Res<'_, &str> {
    delimited(char('"'), take_till(|c| c == '"' || c == '\n'), char('"')).parse(i)
}

fn bare_key(i: &str) -> Res<'_, &str> {
    take_till1(|c| c == ':' || c == '\n' || c == '\r').parse(i)
}

/// `  ~key: value` → `(enabled, key, value)` (value is right-trimmed).
fn dict_entry(i: &str) -> Res<'_, (bool, &str, &str)> {
    let (i, _) = space0(i)?;
    let (i, disabled) = opt(char('~')).map(|c| c.is_some()).parse(i)?;
    let (i, key) = alt((quoted_key, bare_key.map(str::trim_end))).parse(i)?;
    let (i, _) = space0(i)?;
    let (i, _) = char(':')(i)?;
    let (i, _) = space0(i)?;
    let (i, value) = not_line_ending(i)?;
    Ok((i, (!disabled, key, value.trim_end())))
}

/// `  item,` → `item`
fn list_item(i: &str) -> Res<'_, &str> {
    let (i, _) = space0(i)?;
    let (i, item) = recognize(take_till1(|c| c == ',' || c == '\n' || c == '\r')).parse(i)?;
    let (i, _) = opt(tag(",")).parse(i)?;
    Ok((i, item.trim_end()))
}

// ---------------------------------------------------------------------------
// Document driver
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    src: &'a str,
    rest: &'a str,
}

impl<'a> Cursor<'a> {
    fn offset(&self, at: &str) -> usize {
        self.src.len() - at.len()
    }

    fn error_at(&self, at: &str, message: impl Into<String>) -> ParseError {
        ParseError::at(self.src, self.offset(at), message)
    }

    fn skip_blank_lines(&mut self) {
        while !self.rest.is_empty() {
            match line(self.rest) {
                Ok((next, l)) if l.trim().is_empty() => self.rest = next,
                _ => break,
            }
        }
    }

    /// Next line, or `None` at end of input.
    fn next_line(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let (next, l) = line(self.rest).ok()?;
        self.rest = next;
        Some(l)
    }

    fn block(&mut self) -> Result<Block, ParseError> {
        let start = self.rest;
        let (after, (name, opener, closed_inline)) = block_header(self.rest).map_err(|_| {
            // Point at the offending token rather than at its indentation.
            self.error_at(
                start.trim_start_matches([' ', '\t']),
                "expected a block header such as `meta {` or `vars:secret [`",
            )
        })?;
        self.rest = after;
        let name = name.to_owned();

        if closed_inline {
            let body = match opener {
                Opener::Brace if block_kind(&name) == BlockKind::Text => {
                    BlockBody::Text(String::new())
                }
                Opener::Brace => BlockBody::Dict(Vec::new()),
                Opener::Bracket => BlockBody::List(Vec::new()),
            };
            return Ok(Block { name, body });
        }

        let body = match (opener, block_kind(&name)) {
            (Opener::Bracket, _) => BlockBody::List(self.list_body(&name, start)?),
            (Opener::Brace, BlockKind::Text) => BlockBody::Text(self.text_body(&name, start)?),
            (Opener::Brace, BlockKind::Dict) => BlockBody::Dict(self.dict_body(&name, start)?),
            (Opener::Brace, BlockKind::Unknown) => {
                let checkpoint = self.rest;
                match self.dict_body(&name, start) {
                    Ok(entries) => BlockBody::Dict(entries),
                    Err(_) => {
                        self.rest = checkpoint;
                        BlockBody::Text(self.text_body(&name, start)?)
                    }
                }
            }
        };
        Ok(Block { name, body })
    }

    fn unterminated(&self, name: &str, start: &str, closer: char) -> ParseError {
        self.error_at(
            start,
            format!("block `{name}` is never closed (missing `{closer}`)"),
        )
    }

    fn dict_body(&mut self, name: &str, start: &'a str) -> Result<Vec<KeyValue>, ParseError> {
        let mut entries = Vec::new();
        loop {
            let line_start = self.rest;
            let Some(l) = self.next_line() else {
                return Err(self.unterminated(name, start, '}'));
            };
            let trimmed = l.trim();
            if trimmed == "}" {
                return Ok(entries);
            }
            if trimmed.is_empty() {
                continue;
            }
            // An unindented block header means the previous `}` is missing.
            if !l.starts_with(char::is_whitespace) && block_header(l).is_ok() {
                return Err(self.unterminated(name, start, '}'));
            }
            let (_, (enabled, key, value)) = dict_entry(l).map_err(|_| {
                self.error_at(line_start, format!("expected `key: value` inside `{name}`"))
            })?;
            let value = if value == MULTILINE_QUOTE {
                self.multiline_value(name, line_start)?
            } else {
                value.to_owned()
            };
            entries.push(KeyValue {
                name: key.to_owned(),
                value,
                enabled,
            });
        }
    }

    /// Body of a `key: '''` value, dedented by four spaces (entry indent +
    /// value indent), up to the closing `'''` line.
    fn multiline_value(&mut self, name: &str, start: &'a str) -> Result<String, ParseError> {
        let mut out = String::new();
        let mut first = true;
        loop {
            let Some(l) = self.next_line() else {
                return Err(self.error_at(start, format!("unterminated `'''` value in `{name}`")));
            };
            if l.trim() == MULTILINE_QUOTE {
                return Ok(out);
            }
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(dedent(l, 4));
        }
    }

    fn text_body(&mut self, name: &str, start: &'a str) -> Result<String, ParseError> {
        let mut out = String::new();
        let mut first = true;
        loop {
            let Some(l) = self.next_line() else {
                return Err(self.unterminated(name, start, '}'));
            };
            // Content is indented, so a `}` in column 0 always closes the block.
            if l.trim_end() == "}" {
                return Ok(out);
            }
            if !first {
                out.push('\n');
            }
            first = false;
            out.push_str(dedent(l, 2));
        }
    }

    fn list_body(&mut self, name: &str, start: &'a str) -> Result<Vec<String>, ParseError> {
        let mut items = Vec::new();
        loop {
            let line_start = self.rest;
            let Some(l) = self.next_line() else {
                return Err(self.unterminated(name, start, ']'));
            };
            let trimmed = l.trim();
            if trimmed == "]" {
                return Ok(items);
            }
            if trimmed.is_empty() {
                continue;
            }
            let (_, item) = list_item(l).map_err(|_| {
                self.error_at(line_start, format!("expected a list item inside `{name}`"))
            })?;
            items.push(item.to_owned());
        }
    }
}

/// Strip up to `n` leading spaces (never more than are present).
fn dedent(line: &str, n: usize) -> &str {
    let spaces = line.bytes().take(n).take_while(|b| *b == b' ').count();
    &line[spaces..]
}
