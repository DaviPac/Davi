//! `Cmd/Ctrl+P` quick-open palette.
//!
//! A self-contained entity: it owns its query, focus and selection, and talks
//! to the workspace only through [`PaletteEvent`]s. Matching is a small
//! allocation-free fuzzy scorer, fast enough to re-run on every keystroke for
//! thousands of requests.

use std::path::PathBuf;
use std::sync::Arc;

use davi_core::model::HttpMethod;
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton, SharedString, Window,
    div, prelude::*, px,
};

use crate::theme::{self, c};

const MAX_RESULTS: usize = 50;

#[derive(Debug, Clone)]
pub struct PaletteItem {
    pub name: SharedString,
    /// Folder path relative to the collection root, e.g. `users/admin`.
    pub location: SharedString,
    pub method: HttpMethod,
    pub path: PathBuf,
}

pub enum PaletteEvent {
    Confirmed(PathBuf),
    Dismissed,
}

pub struct CommandPalette {
    focus_handle: FocusHandle,
    items: Arc<[PaletteItem]>,
    query: String,
    matches: Vec<usize>,
    selected: usize,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPalette {
    pub fn new(items: Arc<[PaletteItem]>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            items,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
        };
        this.update_matches();
        this
    }

    fn update_matches(&mut self) {
        let query = self.query.trim();
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| {
                // Name matches outrank folder matches.
                let by_name = fuzzy_score(query, &item.name).map(|s| s + 1000);
                let by_location = fuzzy_score(query, &format!("{} {}", item.location, item.name));
                by_name.or(by_location).map(|score| (score, ix))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.matches = scored
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(_, ix)| ix)
            .collect();
        self.selected = 0;
    }

    fn confirm(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(item) = self.matches.get(ix).map(|&i| &self.items[i]) {
            cx.emit(PaletteEvent::Confirmed(item.path.clone()));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        match ks.key.as_str() {
            "escape" => cx.emit(PaletteEvent::Dismissed),
            "enter" => self.confirm(self.selected, cx),
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1)),
            "backspace" => {
                self.query.pop();
                self.update_matches();
            }
            _ => match &ks.key_char {
                Some(ch) if !ks.modifiers.control && !ks.modifiers.platform => {
                    self.query.push_str(ch);
                    self.update_matches();
                }
                _ => return,
            },
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query_label: SharedString = if self.query.is_empty() {
            "Search requests…".into()
        } else {
            self.query.clone().into()
        };

        div()
            .id("command-palette")
            .track_focus(&self.focus_handle)
            .key_context("CommandPalette")
            .on_key_down(cx.listener(Self::on_key_down))
            // Keep clicks inside the palette from reaching the backdrop.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(560.))
            .max_h(px(420.))
            .flex()
            .flex_col()
            .bg(c(theme::SURFACE))
            .border_1()
            .border_color(c(theme::HOVER))
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .text_color(c(if self.query.is_empty() {
                        theme::TEXT_FAINT
                    } else {
                        theme::TEXT
                    }))
                    .child(query_label),
            )
            .child(
                div()
                    .id("palette-results")
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .py_1()
                    .children(self.matches.iter().enumerate().map(|(row, &ix)| {
                        let item = &self.items[ix];
                        let selected = row == self.selected;
                        div()
                            .id(row)
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .when(selected, |el| el.bg(c(theme::ELEVATED)))
                            .hover(|s| s.bg(c(theme::ELEVATED)))
                            .on_click(cx.listener(move |this, _, _, cx| this.confirm(row, cx)))
                            .child(
                                div()
                                    .w(px(44.))
                                    .text_xs()
                                    .text_color(theme::method_color(item.method))
                                    .child(theme::method_label(item.method)),
                            )
                            .child(div().text_color(c(theme::TEXT)).child(item.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(c(theme::TEXT_FAINT))
                                    .child(item.location.clone()),
                            )
                    }))
                    .when(self.matches.is_empty(), |el| {
                        el.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_color(c(theme::TEXT_FAINT))
                                .child("No matching requests"),
                        )
                    }),
            )
    }
}

/// Case-insensitive subsequence match. Rewards consecutive runs and matches
/// at word starts; returns `None` when `query` is not a subsequence.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let mut score = 0;
    let mut query_chars = query.chars().flat_map(char::to_lowercase).peekable();
    let mut prev_matched = false;
    let mut prev_char = ' ';
    for ch in candidate.chars() {
        let Some(&q) = query_chars.peek() else { break };
        if ch.to_lowercase().eq(std::iter::once(q)) {
            score += 1;
            if prev_matched {
                score += 5;
            }
            if !prev_char.is_alphanumeric() || (prev_char.is_lowercase() && ch.is_uppercase()) {
                score += 10;
            }
            query_chars.next();
            prev_matched = true;
        } else {
            prev_matched = false;
        }
        prev_char = ch;
    }
    // Shorter candidates win ties.
    query_chars
        .peek()
        .is_none()
        .then(|| score * 4 - candidate.len() as i32)
}

#[cfg(test)]
mod tests {
    use super::fuzzy_score;

    #[test]
    fn fuzzy_matching() {
        assert!(fuzzy_score("cu", "Create User").is_some());
        assert!(fuzzy_score("xyz", "Create User").is_none());
        assert!(fuzzy_score("user", "Get User") > fuzzy_score("user", "Upload Some Every Row"));
        assert!(fuzzy_score("GU", "Get User") > fuzzy_score("GU", "Ping Gu"));
    }
}
