//! # davi-ui
//!
//! GPUI front-end for Davi. The binary entry point lives in `main.rs`; this
//! library exposes the views so they can be embedded or tested.

pub mod highlight;
pub mod kv_editor;
pub mod palette;
pub mod request_editor;
pub mod response_view;
pub mod settings;
pub mod theme;
pub mod workspace;

pub use workspace::Workspace;
