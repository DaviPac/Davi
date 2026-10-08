//! Color palette and typography. Plain constants for now; a runtime-switchable
//! `Theme` global can replace this module without touching the views.

use davi_core::model::HttpMethod;
use gpui::{Rgba, rgb};

pub const BG: u32 = 0x1e1e2e;
pub const SURFACE: u32 = 0x181825;
pub const SURFACE_ALT: u32 = 0x11111b;
pub const ELEVATED: u32 = 0x313244;
pub const BORDER: u32 = 0x313244;
pub const HOVER: u32 = 0x45475a;
pub const TEXT: u32 = 0xcdd6f4;
pub const TEXT_MUTED: u32 = 0x9399b2;
pub const TEXT_FAINT: u32 = 0x6c7086;
pub const ACCENT: u32 = 0x89b4fa;
pub const SUCCESS: u32 = 0xa6e3a1;
pub const WARNING: u32 = 0xf9e2af;
pub const ERROR: u32 = 0xf38ba8;

// JSON syntax colors.
pub const SYN_KEY: u32 = 0x89b4fa;
pub const SYN_STRING: u32 = 0xa6e3a1;
pub const SYN_NUMBER: u32 = 0xfab387;
pub const SYN_KEYWORD: u32 = 0xcba6f7;
pub const SYN_PUNCT: u32 = 0x9399b2;

#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "Menlo";
#[cfg(target_os = "windows")]
pub const MONO_FONT: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub const MONO_FONT: &str = "DejaVu Sans Mono";

pub fn c(hex: u32) -> Rgba {
    rgb(hex)
}

pub fn method_color(method: HttpMethod) -> Rgba {
    rgb(match method {
        HttpMethod::Get => 0xa6e3a1,
        HttpMethod::Post => 0xf9e2af,
        HttpMethod::Put => 0x89b4fa,
        HttpMethod::Patch => 0xcba6f7,
        HttpMethod::Delete => 0xf38ba8,
        HttpMethod::Head | HttpMethod::Options => 0x94e2d5,
        HttpMethod::Connect | HttpMethod::Trace => 0x9399b2,
    })
}

pub fn status_color(status: u16) -> Rgba {
    rgb(match status {
        200..=299 => SUCCESS,
        300..=399 => ACCENT,
        400..=499 => WARNING,
        _ => ERROR,
    })
}

/// Short method label that fits a fixed-width sidebar column.
pub fn method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Delete => "DEL",
        HttpMethod::Options => "OPT",
        HttpMethod::Connect => "CONN",
        HttpMethod::Trace => "TRC",
        m => m.as_str(),
    }
}
