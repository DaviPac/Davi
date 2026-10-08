use std::error::Error as _;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },
    #[error("invalid header `{name}`: {reason}")]
    InvalidHeader { name: String, reason: String },
    #[error("invalid body: {reason}")]
    InvalidBody { reason: String },
    #[error("cannot read multipart file {}: {source}", path.display())]
    File {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("request timed out")]
    Timeout,
    #[error("{}", chain(.0))]
    Http(#[source] reqwest::Error),
    #[error("request cancelled")]
    Cancelled,
    #[error("failed to start network runtime: {0}")]
    Runtime(#[source] std::io::Error),
}

impl From<reqwest::Error> for NetError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            NetError::Timeout
        } else {
            NetError::Http(e)
        }
    }
}

/// reqwest's top-level message is terse ("error sending request"); the useful
/// part (DNS failure, connection refused, TLS...) lives in the source chain.
fn chain(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}
