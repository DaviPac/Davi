use std::time::Duration;

use bytes::Bytes;

/// A completed HTTP exchange plus the metrics shown in the response panel.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: Option<&'static str>,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub cookies: Vec<ResponseCookie>,
    /// Decoded (decompressed) body. Capped at `EngineConfig::max_body_bytes`.
    pub body: Bytes,
    pub truncated: bool,
    pub timings: Timings,
    pub size: ResponseSize,
    /// Final URL after redirects.
    pub url: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn is_json(&self) -> bool {
        self.content_type().is_some_and(|ct| ct.contains("json"))
    }

    pub fn body_text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Body pretty-printed when it is JSON, verbatim otherwise.
    pub fn pretty_body(&self) -> String {
        let looks_json =
            self.is_json() || self.body.first().is_some_and(|b| matches!(b, b'{' | b'['));
        if looks_json
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&self.body)
            && let Ok(pretty) = serde_json::to_string_pretty(&value)
        {
            return pretty;
        }
        self.body_text().into_owned()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Timings {
    /// Request start until response headers were received.
    pub ttfb: Duration,
    /// Request start until the last body byte was read.
    pub total: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResponseSize {
    /// Approximate size of the header section as received.
    pub headers: u64,
    /// Decoded body size.
    pub body: u64,
}

impl ResponseSize {
    pub fn total(&self) -> u64 {
        self.headers + self.body
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseCookie {
    pub name: String,
    pub value: String,
    /// The full `Set-Cookie` value, attributes included.
    pub raw: String,
}

impl ResponseCookie {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let pair = raw.split(';').next()?;
        let (name, value) = pair.split_once('=')?;
        Some(Self {
            name: name.trim().to_owned(),
            value: value.trim().to_owned(),
            raw: raw.to_owned(),
        })
    }
}

/// Streaming progress, published while a request is in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Progress {
    #[default]
    Connecting,
    Headers {
        status: u16,
        ttfb: Duration,
    },
    Receiving {
        received: u64,
        total: Option<u64>,
    },
    Done,
}
