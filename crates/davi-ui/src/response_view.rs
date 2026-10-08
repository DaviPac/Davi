//! Render-ready snapshot of an [`HttpResponse`].
//!
//! Built once per response on a background thread: pretty-printing,
//! line-splitting and highlighting never happen inside `render`, which only
//! slices the visible lines out of a virtualized list.

use std::ops::Range;
use std::sync::Arc;

use davi_net::HttpResponse;
use gpui::{HighlightStyle, SharedString};

use crate::highlight::highlight_json_line;

/// Extremely long lines (minified HTML, base64...) are clipped for layout;
/// the full body stays available for copy/save.
const MAX_LINE_CHARS: usize = 4_000;

pub struct BodyLine {
    pub text: SharedString,
    pub highlights: Arc<[(Range<usize>, HighlightStyle)]>,
}

pub struct ResponseView {
    pub status: u16,
    pub status_text: SharedString,
    pub time: SharedString,
    pub size: SharedString,
    pub headers: Vec<(SharedString, SharedString)>,
    pub lines: Arc<[BodyLine]>,
    pub truncated: bool,
}

impl ResponseView {
    pub fn build(response: HttpResponse) -> Self {
        let pretty = response.pretty_body();
        let is_json = response.is_json() || pretty.starts_with(['{', '[']);
        let lines: Arc<[BodyLine]> = pretty
            .lines()
            .map(|line| {
                let line = clip(line);
                BodyLine {
                    highlights: if is_json {
                        highlight_json_line(line).into()
                    } else {
                        Arc::from([])
                    },
                    text: SharedString::from(line.to_owned()),
                }
            })
            .collect();

        Self {
            status: response.status,
            status_text: format!(
                "{} {}",
                response.status,
                response.reason.unwrap_or_default()
            )
            .trim_end()
            .to_owned()
            .into(),
            time: format_duration_ms(response.timings.total.as_secs_f64() * 1000.0).into(),
            size: format_bytes(response.size.body).into(),
            headers: response
                .headers
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            lines,
            truncated: response.truncated,
        }
    }
}

fn clip(line: &str) -> &str {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((ix, _)) => &line[..ix],
        None => line,
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_duration_ms(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else {
        format!("{ms:.0} ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_metrics() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_duration_ms(12.4), "12 ms");
        assert_eq!(format_duration_ms(1534.0), "1.53 s");
    }
}
