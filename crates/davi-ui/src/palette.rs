//! `Cmd/Ctrl+P` quick-open palette.
//!
//! A self-contained entity: it owns its query input, focus and selection, and
//! talks to the workspace only through [`PaletteEvent`]s. Matching is a small
//! allocation-free fuzzy scorer, fast enough to re-run on every keystroke for
//! thousands of requests.

use std::path::PathBuf;
use std::sync::Arc;

use davi_core::model::HttpMethod;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton,
    SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};

use crate::theme::{self, c};

const MAX_RESULTS: usize = 50;
const CONTEXT: &str = "CommandPalette";

actions!(palette, [SelectPrev, SelectNext, Confirm, Dismiss]);

/// Bind the palette's navigation keys. They are scoped to the palette's text
/// input, so they must be registered after `gpui_component::init` to take
/// precedence over the input's own up/down/enter/escape bindings.
pub fn init(cx: &mut App) {
    let ctx = Some("CommandPalette > Input");
    cx.bind_keys([
        KeyBinding::new("up", SelectPrev, ctx),
        KeyBinding::new("down", SelectNext, ctx),
        KeyBinding::new("enter", Confirm, ctx),
        KeyBinding::new("escape", Dismiss, ctx),
    ]);
}

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
    input: Entity<InputState>,
    items: Arc<[PaletteItem]>,
    matches: Vec<usize>,
    selected: usize,
    _subscription: Subscription,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl CommandPalette {
    pub fn new(items: Arc<[PaletteItem]>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search requests…"));
        let subscription = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.update_matches(cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let mut this = Self {
            input,
            items,
            matches: Vec::new(),
            selected: 0,
            _subscription: subscription,
        };
        this.update_matches(cx);
        this
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_owned();
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| {
                // Name matches outrank folder matches.
                let by_name = fuzzy_score(&query, &item.name).map(|s| s + 1000);
                let by_location = fuzzy_score(&query, &format!("{} {}", item.location, item.name));
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
        cx.notify();
    }

    fn confirm_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(item) = self.matches.get(ix).map(|&i| &self.items[i]) {
            cx.emit(PaletteEvent::Confirmed(item.path.clone()));
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1));
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.confirm_at(self.selected, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Dismissed);
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("command-palette")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::dismiss))
            // Keep clicks inside the palette from reaching the backdrop.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .w(px(560.))
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
                    .p_2()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(Input::new(&self.input).appearance(false)),
            )
            .child(
                div()
                    .id("palette-results")
                    .max_h(px(360.))
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
                            .on_click(cx.listener(move |this, _, _, cx| this.confirm_at(row, cx)))
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
                        el.child(div().px_3().py_2().text_color(c(theme::TEXT_FAINT)).child(
                            if self.items.is_empty() {
                                "This collection has no requests yet"
                            } else {
                                "No matching requests"
                            },
                        ))
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
