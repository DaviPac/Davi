//! Editable `name: value` table used for params, headers, form bodies and
//! vars. Each row owns two text inputs; any edit emits [`KvEvent::Changed`].

use davi_core::model::KeyValue;
use gpui::{
    App, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::Sizable;
use gpui_component::input::{Input, InputEvent, InputState};

use crate::theme::{self, c};

pub enum KvEvent {
    Changed,
}

struct KvRow {
    id: usize,
    enabled: bool,
    name: Entity<InputState>,
    value: Entity<InputState>,
}

pub struct KvEditor {
    rows: Vec<KvRow>,
    next_id: usize,
    name_placeholder: SharedString,
    value_placeholder: SharedString,
    /// Hide values as `•••` (secrets).
    masked: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<KvEvent> for KvEditor {}

impl KvEditor {
    pub fn new(
        entries: &[KeyValue],
        name_placeholder: &'static str,
        value_placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            rows: Vec::new(),
            next_id: 0,
            name_placeholder: name_placeholder.into(),
            value_placeholder: value_placeholder.into(),
            masked: false,
            _subscriptions: Vec::new(),
        };
        this.set_entries(entries, window, cx);
        this
    }

    /// Replace all rows without emitting [`KvEvent::Changed`].
    pub fn set_entries(
        &mut self,
        entries: &[KeyValue],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rows.clear();
        self._subscriptions.clear();
        for kv in entries {
            self.push_row(kv, window, cx);
        }
        cx.notify();
    }

    /// Mask every value input, now and for rows added later.
    pub fn mask_values(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.masked = true;
        for row in &self.rows {
            row.value
                .update(cx, |input, cx| input.set_masked(true, window, cx));
        }
    }

    /// Current rows, skipping rows whose name and value are both empty.
    pub fn entries(&self, cx: &App) -> Vec<KeyValue> {
        self.rows
            .iter()
            .filter_map(|row| {
                let name = row.name.read(cx).value().trim().to_owned();
                let value = row.value.read(cx).value().to_string();
                (!name.is_empty() || !value.is_empty()).then_some(KeyValue {
                    name,
                    value,
                    enabled: row.enabled,
                })
            })
            .collect()
    }

    fn push_row(&mut self, kv: &KeyValue, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let (name_ph, value_ph) = (
            self.name_placeholder.clone(),
            self.value_placeholder.clone(),
        );
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(name_ph)
                .default_value(kv.name.clone())
        });
        let masked = self.masked;
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(value_ph)
                .masked(masked)
                .default_value(kv.value.clone())
        });
        for input in [&name, &value] {
            self._subscriptions.push(cx.subscribe_in(
                input,
                window,
                |_, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.emit(KvEvent::Changed);
                    }
                },
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.rows.push(KvRow {
            id,
            enabled: kv.enabled,
            name,
            value,
        });
        id
    }

    fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_row(&KeyValue::new("", ""), window, cx);
        if let Some(row) = self.rows.last() {
            row.name.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn remove_row(&mut self, id: usize, cx: &mut Context<Self>) {
        self.rows.retain(|r| r.id != id);
        cx.emit(KvEvent::Changed);
        cx.notify();
    }

    fn toggle_row(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.id == id) {
            row.enabled = !row.enabled;
            cx.emit(KvEvent::Changed);
            cx.notify();
        }
    }
}

impl Render for KvEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(self.rows.iter().map(|row| {
                let id = row.id;
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        toggle(("kv-toggle", id), row.enabled)
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle_row(id, cx))),
                    )
                    .child(
                        div()
                            .w(px(200.))
                            .flex_none()
                            .child(Input::new(&row.name).small()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&row.value).small()),
                    )
                    .child(
                        div()
                            .id(("kv-remove", id))
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(c(theme::TEXT_FAINT))
                            .hover(|s| s.text_color(c(theme::ERROR)).bg(c(theme::ELEVATED)))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_row(id, cx)))
                            .child("×"),
                    )
            }))
            .child(
                div()
                    .id("kv-add")
                    .mt_1()
                    .w(px(64.))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_xs()
                    .text_color(c(theme::ACCENT))
                    .hover(|s| s.bg(c(theme::ELEVATED)))
                    .on_click(cx.listener(|this, _, window, cx| this.add_row(window, cx)))
                    .child("+ Add"),
            )
    }
}

/// A small checkbox drawn without icon assets.
pub fn toggle(id: impl Into<gpui::ElementId>, on: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .cursor_pointer()
        .text_xs()
        .when(on, |el| {
            el.bg(c(theme::ACCENT))
                .border_color(c(theme::ACCENT))
                .text_color(c(theme::SURFACE_ALT))
                .child("✓")
        })
        .when(!on, |el| el.border_color(c(theme::HOVER)))
}
