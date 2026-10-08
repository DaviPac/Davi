//! `.bru` serializer. Output mirrors Bruno's own writer (two-space indent,
//! blocks separated by a blank line) so files round-trip with minimal diffs.

use std::fmt::Write as _;

use super::ast::{Block, BlockBody, BruFile};
use crate::model::KeyValue;

pub fn write(file: &BruFile) -> String {
    let mut out = String::with_capacity(512);
    for (i, block) in file.blocks.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        write_block(&mut out, block);
    }
    out
}

fn write_block(out: &mut String, block: &Block) {
    match &block.body {
        BlockBody::Dict(entries) => {
            let _ = writeln!(out, "{} {{", block.name);
            for entry in entries {
                write_entry(out, entry);
            }
            out.push_str("}\n");
        }
        BlockBody::Text(text) => {
            let _ = writeln!(out, "{} {{", block.name);
            if !text.is_empty() {
                for line in text.split('\n') {
                    let _ = writeln!(out, "  {line}");
                }
            }
            out.push_str("}\n");
        }
        BlockBody::List(items) => {
            let _ = writeln!(out, "{} [", block.name);
            let last = items.len().saturating_sub(1);
            for (i, item) in items.iter().enumerate() {
                let sep = if i == last { "" } else { "," };
                let _ = writeln!(out, "  {item}{sep}");
            }
            out.push_str("]\n");
        }
    }
}

fn write_entry(out: &mut String, entry: &KeyValue) {
    out.push_str("  ");
    if !entry.enabled {
        out.push('~');
    }
    if needs_quotes(&entry.name) {
        let _ = write!(out, "\"{}\"", entry.name);
    } else {
        out.push_str(&entry.name);
    }
    if entry.value.contains('\n') {
        out.push_str(": '''\n");
        for line in entry.value.split('\n') {
            let _ = writeln!(out, "    {line}");
        }
        out.push_str("  '''\n");
    } else {
        // Bruno always writes `key: value`, even for empty values; match it
        // byte-for-byte to keep Git diffs clean across both tools.
        let _ = writeln!(out, ": {}", entry.value);
    }
}

fn needs_quotes(key: &str) -> bool {
    key.is_empty()
        || key.contains(':')
        || key.starts_with('~')
        || key.starts_with('"')
        || key.starts_with(char::is_whitespace)
        || key.ends_with(char::is_whitespace)
}
