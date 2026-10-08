//! Typed, UI- and engine-facing request model.
//!
//! These types are deliberately independent of the `.bru` syntax: the
//! [`crate::bru`] module lowers the generic block AST into an [`HttpRequest`]
//! and raises it back. Anything Davi does not understand yet is preserved in
//! [`HttpRequest::extra_blocks`] so that saving a file never silently drops
//! data written by Bruno or by hand.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::bru::Block;

/// A `name: value` pair. `enabled == false` is written as `~name: value`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub name: String,
    pub value: String,
    pub enabled: bool,
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }

    pub fn disabled(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: false,
            ..Self::new(name, value)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Connect,
    Trace,
}

impl HttpMethod {
    pub const ALL: [HttpMethod; 9] = [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
        HttpMethod::Head,
        HttpMethod::Options,
        HttpMethod::Connect,
        HttpMethod::Trace,
    ];

    /// Canonical upper-case wire representation (`GET`, `POST`, ...).
    pub const fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Connect => "CONNECT",
            HttpMethod::Trace => "TRACE",
        }
    }

    /// Lower-case block name used in `.bru` files (`get { ... }`).
    pub const fn bru_block(self) -> &'static str {
        match self {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
            HttpMethod::Head => "head",
            HttpMethod::Options => "options",
            HttpMethod::Connect => "connect",
            HttpMethod::Trace => "trace",
        }
    }

    pub fn from_bru_block(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.bru_block() == name)
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HttpMethod {
    type Err = UnknownMethod;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|m| m.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| UnknownMethod(s.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown HTTP method `{0}`")]
pub struct UnknownMethod(pub String);

/// `meta { type: ... }`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RequestKind {
    #[default]
    Http,
    Graphql,
    Other(String),
}

impl RequestKind {
    pub fn as_str(&self) -> &str {
        match self {
            RequestKind::Http => "http",
            RequestKind::Graphql => "graphql",
            RequestKind::Other(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "http" => RequestKind::Http,
            "graphql" => RequestKind::Graphql,
            other => RequestKind::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    pub kind: RequestKind,
    pub seq: Option<u32>,
    /// Unknown `meta` keys (e.g. `tags`), kept for lossless round-trips.
    pub extra: Vec<KeyValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ApiKeyPlacement {
    #[default]
    Header,
    QueryParams,
}

impl ApiKeyPlacement {
    pub const fn as_str(self) -> &'static str {
        match self {
            ApiKeyPlacement::Header => "header",
            ApiKeyPlacement::QueryParams => "queryparams",
        }
    }
}

/// The active authentication strategy of a request.
///
/// Only the strategy selected by the `auth:` field of the method block is
/// lowered here; other `auth:*` blocks stay in `extra_blocks`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Auth {
    #[default]
    None,
    /// Use the folder/collection auth.
    Inherit,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        placement: ApiKeyPlacement,
    },
    /// A mode Davi doesn't execute yet (digest, oauth2, awsv4, ...). Its
    /// block is preserved verbatim in `extra_blocks`.
    Unsupported {
        mode: String,
    },
}

impl Auth {
    /// Value of the `auth:` key in the method block.
    pub fn mode(&self) -> &str {
        match self {
            Auth::None => "none",
            Auth::Inherit => "inherit",
            Auth::Bearer { .. } => "bearer",
            Auth::Basic { .. } => "basic",
            Auth::ApiKey { .. } => "apikey",
            Auth::Unsupported { mode } => mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BodyMode {
    #[default]
    None,
    Json,
    Text,
    Xml,
    Sparql,
    FormUrlEncoded,
    MultipartForm,
    Graphql,
    Other(String),
}

impl BodyMode {
    /// Value of the `body:` key in the method block.
    pub fn as_str(&self) -> &str {
        match self {
            BodyMode::None => "none",
            BodyMode::Json => "json",
            BodyMode::Text => "text",
            BodyMode::Xml => "xml",
            BodyMode::Sparql => "sparql",
            BodyMode::FormUrlEncoded => "formUrlEncoded",
            BodyMode::MultipartForm => "multipartForm",
            BodyMode::Graphql => "graphql",
            BodyMode::Other(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "none" | "" => BodyMode::None,
            "json" => BodyMode::Json,
            "text" => BodyMode::Text,
            "xml" => BodyMode::Xml,
            "sparql" => BodyMode::Sparql,
            "formUrlEncoded" => BodyMode::FormUrlEncoded,
            "multipartForm" => BodyMode::MultipartForm,
            "graphql" => BodyMode::Graphql,
            other => BodyMode::Other(other.to_owned()),
        }
    }
}

/// All body payloads stored in the file. Bruno keeps the payload of every
/// mode the user has filled in, while `mode` selects which one is sent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequestBody {
    pub mode: BodyMode,
    pub json: Option<String>,
    pub text: Option<String>,
    pub xml: Option<String>,
    pub sparql: Option<String>,
    pub graphql: Option<String>,
    pub graphql_vars: Option<String>,
    pub form_urlencoded: Vec<KeyValue>,
    /// Values of the form `@file(path)` denote file parts.
    pub multipart_form: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequestVars {
    pub pre_request: Vec<KeyValue>,
    pub post_response: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scripts {
    pub pre_request: Option<String>,
    pub post_response: Option<String>,
}

/// A fully-lowered `.bru` request.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HttpRequest {
    pub meta: Meta,
    pub method: HttpMethod,
    /// URL as written, including its query string and `{{vars}}`. Bruno keeps
    /// `query_params` in sync with this, the URL is what gets sent.
    pub url: String,
    pub query_params: Vec<KeyValue>,
    pub path_params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub auth: Auth,
    pub body: RequestBody,
    pub vars: RequestVars,
    pub assertions: Vec<KeyValue>,
    pub scripts: Scripts,
    pub tests: Option<String>,
    pub docs: Option<String>,
    /// Blocks Davi doesn't model yet, written back untouched.
    pub extra_blocks: Vec<Block>,
}

impl HttpRequest {
    pub fn new(name: impl Into<String>, method: HttpMethod, url: impl Into<String>) -> Self {
        Self {
            meta: Meta {
                name: name.into(),
                ..Meta::default()
            },
            method,
            url: url.into(),
            ..Self::default()
        }
    }

    pub fn enabled_headers(&self) -> impl Iterator<Item = &KeyValue> {
        self.headers.iter().filter(|h| h.enabled)
    }
}
