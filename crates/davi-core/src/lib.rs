//! # davi-core
//!
//! Pure, I/O-light building blocks shared by the engine and the UI:
//!
//! * [`model`] – typed request model ([`model::HttpRequest`]).
//! * [`bru`] – Bruno `.bru` parser/writer with lossless round-tripping.
//! * [`env`] – environments and `{{variable}}` interpolation.
//! * [`collection`] – loading a collection directory into a sidebar tree.
//!
//! This crate has no async runtime and no UI dependency, so it compiles fast
//! and can be reused by a future CLI runner.

pub mod bru;
pub mod collection;
pub mod env;
pub mod error;
pub mod model;

pub use error::{CoreError, LowerError, ParseError};
