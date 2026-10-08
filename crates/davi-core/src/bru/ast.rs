use serde::{Deserialize, Serialize};

use crate::model::KeyValue;

/// A parsed `.bru` document: an ordered list of top-level blocks.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BruFile {
    pub blocks: Vec<Block>,
}

impl BruFile {
    pub fn block(&self, name: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.name == name)
    }

    pub fn dict(&self, name: &str) -> Option<&[KeyValue]> {
        self.block(name).and_then(|b| b.body.as_dict())
    }

    pub fn text(&self, name: &str) -> Option<&str> {
        self.block(name).and_then(|b| b.body.as_text())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// e.g. `meta`, `get`, `headers`, `body:json`, `auth:bearer`.
    pub name: String,
    pub body: BlockBody,
}

impl Block {
    pub fn dict(name: impl Into<String>, entries: Vec<KeyValue>) -> Self {
        Self {
            name: name.into(),
            body: BlockBody::Dict(entries),
        }
    }

    pub fn text(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            body: BlockBody::Text(text.into()),
        }
    }

    pub fn list(name: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            name: name.into(),
            body: BlockBody::List(items),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockBody {
    /// `name { key: value ... }`
    Dict(Vec<KeyValue>),
    /// `name { <free text, indented by two spaces> }` (bodies, scripts, docs).
    Text(String),
    /// `name [ a, b ]` (e.g. `vars:secret` in environment files).
    List(Vec<String>),
}

impl BlockBody {
    pub fn as_dict(&self) -> Option<&[KeyValue]> {
        match self {
            BlockBody::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            BlockBody::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            BlockBody::List(l) => Some(l),
            _ => None,
        }
    }
}

/// How the body of a `{ ... }` block is interpreted, decided by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Dict,
    Text,
    /// Unknown block: try dictionary first, fall back to raw text so that
    /// nothing is lost.
    Unknown,
}

pub(crate) fn block_kind(name: &str) -> BlockKind {
    match name {
        "tests" | "docs" => BlockKind::Text,
        "body:form-urlencoded" | "body:multipart-form" | "body:file" => BlockKind::Dict,
        n if n.starts_with("body:") || n.starts_with("script:") => BlockKind::Text,
        "meta" | "headers" | "assert" | "settings" | "vars" => BlockKind::Dict,
        n if n.starts_with("params:") || n.starts_with("auth:") || n.starts_with("vars:") => {
            BlockKind::Dict
        }
        n if crate::model::HttpMethod::from_bru_block(n).is_some() => BlockKind::Dict,
        _ => BlockKind::Unknown,
    }
}
