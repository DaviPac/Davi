//! # davi-net
//!
//! Async HTTP execution engine for Davi.
//!
//! 1. [`prepare`] turns a [`davi_core::model::HttpRequest`] and a
//!    [`davi_core::env::VarScope`] into a fully interpolated
//!    [`PreparedRequest`] (pure, no I/O).
//! 2. [`HttpEngine`] sends it on a dedicated tokio runtime with a shared
//!    connection pool and cookie jar, streaming the body while publishing
//!    [`Progress`] and measuring [`Timings`] / [`ResponseSize`].
//!
//! The returned [`RequestHandle`] can be awaited from any executor (GPUI's
//! included), which keeps tokio an implementation detail of this crate.

mod engine;
mod error;
pub mod prepare;
mod response;

pub use engine::{Canceller, EngineConfig, HttpEngine, RequestHandle};
pub use error::NetError;
pub use prepare::{PreparedBody, PreparedRequest, prepare};
pub use response::{HttpResponse, Progress, ResponseCookie, ResponseSize, Timings};
