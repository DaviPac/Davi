//! Render-ready snapshot of an [`HttpResponse`].
//!
//! Built once per response on a background thread: pretty-printing and
//! choosing a highlight language never happen on the UI thread. The text is
//! then shown in a read-only code editor (selectable, copyable, searchable,
//! horizontally scrollable).

use davi_net::HttpResponse;
use gpui::SharedString;

/// Above this size, syntax highlighting is skipped to keep huge responses
/// responsive.
const MAX_HIGHLIGHT_BYTES: usize = 2 * 1024 * 1024;

pub struct ResponseView {
    pub status: u16,
    pub status_text: SharedString,
    pub time: SharedString,
    pub size: SharedString,
    pub headers: Vec<(SharedString, SharedString)>,
    /// Pretty-printed body (JSON) or the raw body as text.
    pub body: String,
    /// `gpui-component` highlighter language for `body`.
    pub language: &'static str,
    pub truncated: bool,
}

impl ResponseView {
    pub fn build(response: HttpResponse) -> Self {
        let body = response.pretty_body();
        let content_type = response
            .content_type()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let language = if body.len() > MAX_HIGHLIGHT_BYTES {
            "text"
        } else if response.is_json() || body.starts_with(['{', '[']) {
            "json"
        } else if content_type.contains("html") || content_type.contains("xml") {
            "html"
        } else {
            "text"
        };

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
            body,
            language,
            truncated: response.truncated,
        }
    }

    /// Headers as `Name: value` lines, for the read-only headers view.
    pub fn headers_text(&self) -> String {
        self.headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n")
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
