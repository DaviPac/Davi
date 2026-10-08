use std::fmt;
use std::path::PathBuf;

/// A syntax error in a `.bru` document, with a 1-based position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    pub(crate) fn at(source: &str, offset: usize, message: impl Into<String>) -> Self {
        let before = &source[..offset.min(source.len())];
        let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].chars().count() + 1;
        Self {
            line,
            column,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for ParseError {}

/// The document parsed but doesn't describe a valid request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LowerError {
    #[error("request has no HTTP method block (e.g. `get {{ url: ... }}`)")]
    MissingMethodBlock,
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Lower(LowerError),
    #[error("{path}: {source}")]
    File {
        path: PathBuf,
        #[source]
        source: Box<CoreError>,
    },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl CoreError {
    pub(crate) fn in_file(self, path: impl Into<PathBuf>) -> Self {
        CoreError::File {
            path: path.into(),
            source: Box::new(self),
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io {
            path: path.into(),
            source,
        }
    }
}
