//! Bruno `.bru` file format: AST, parser, writer and typed lowering.
//!
//! ```
//! use davi_core::bru;
//! use davi_core::model::HttpRequest;
//!
//! let src = "meta {\n  name: Ping\n  type: http\n  seq: 1\n}\n\nget {\n  url: {{baseUrl}}/ping\n  body: none\n  auth: none\n}\n";
//! let file = bru::parse(src).unwrap();
//! let req = HttpRequest::from_bru(&file).unwrap();
//! assert_eq!(req.url, "{{baseUrl}}/ping");
//! assert_eq!(bru::write(&req.to_bru()), src);
//! ```

mod ast;
mod lower;
mod parser;
mod writer;

pub use ast::{Block, BlockBody, BruFile};
pub use parser::parse;
pub use writer::write;

use crate::error::{CoreError, LowerError};
use crate::model::HttpRequest;

/// Parse and lower a request file in one step.
pub fn parse_request(source: &str) -> Result<HttpRequest, CoreError> {
    let file = parse(source)?;
    Ok(HttpRequest::from_bru(&file)?)
}

/// Serialize a request to `.bru` text.
pub fn write_request(request: &HttpRequest) -> String {
    write(&request.to_bru())
}

impl From<LowerError> for CoreError {
    fn from(e: LowerError) -> Self {
        CoreError::Lower(e)
    }
}

#[cfg(test)]
mod tests;
