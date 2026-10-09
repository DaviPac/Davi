//! Environment manager, shown in a dialog: the collection's environments on
//! the left; name, variables and secrets of the selected one on the right.
//!
//! Edits (including creating, renaming and deleting environments) stay in
//! memory until [`EnvironmentManager::apply`] writes them, so the dialog's
//! Cancel discards everything.

use std::path::PathBuf;

use davi_core::collection;
use davi_core::env::{EnvVariable, Environment};
use davi_core::model::KeyValue;
use gpui::{Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};
use gpui_component::Sizable;
use gpui_component::input::{Input, InputEvent, InputState};

use crate::kv_editor::{KvEditor, KvEvent};
use crate::secrets::SecretStore;
use crate::theme::{self, c};

struct Draft {
    /// The environment as it is on disk; `None` if created in this dialog.
    saved: Option<Environment>,
    env: Environment,
}

pub struct EnvironmentManager {
    root: PathBuf,
    drafts: Vec<Draft>,
    /// Names of saved environments deleted in this dialog.
    deleted: Vec<String>,
    selected: Option<usize>,
    name: Entity<InputState>,
    vars: Entity<KvEditor>,
    secrets: Entity<KvEditor>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EnvironmentManager {
    /// `environments` must already have their secret values filled in.
    /// With no environments, a new one is started so there is something to
    /// type into.
    pub fn new(
        root: PathBuf,
        environments: Vec<Environment>,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Environment name"));
        let vars = cx.new(|cx| KvEditor::new(&[], "variable", "value", window, cx));
        let secrets = cx.new(|cx| {
            let mut editor = KvEditor::new(&[], "variable", "secret value", window, cx);
            editor.mask_values(window, cx);
            editor
        });

        let subscriptions = vec![
            cx.subscribe_in(&name, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let Some(draft) = this.current_mut()
                        && draft.env.name != value
                    {
                        draft.env.name = value;
                        this.error = None;
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&vars, window, |this, _, _: &KvEvent, _, cx| this.commit(cx)),
            cx.subscribe_in(&secrets, window, |this, _, _: &KvEvent, _, cx| {
                this.commit(cx)
            }),
        ];

        let mut this = Self {
            root,
            drafts: environments
                .into_iter()
                .map(|env| Draft {
                    saved: Some(env.clone()),
                    env,
                })
                .collect(),
            deleted: Vec::new(),
            selected: None,
            name,
            vars,
            secrets,
            error: None,
            _subscriptions: subscriptions,
        };
        if this.drafts.is_empty() {
            this.add(String::from("Local"), Vec::new(), window, cx);
        } else {
            this.select(selected.unwrap_or(0).min(this.drafts.len() - 1), window, cx);
        }
        this
    }

    fn current_mut(&mut self) -> Option<&mut Draft> {
        self.selected.and_then(|ix| self.drafts.get_mut(ix))
    }

    /// Copy the editors' rows into the selected draft.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let to_vars = |entries: Vec<KeyValue>, secret: bool| {
            entries
                .into_iter()
                .filter(|kv| !kv.name.is_empty())
                .map(move |kv| EnvVariable {
                    name: kv.name,
                    value: kv.value,
                    enabled: kv.enabled,
                    secret,
                })
        };
        let variables: Vec<EnvVariable> = to_vars(self.vars.read(cx).entries(cx), false)
            .chain(to_vars(self.secrets.read(cx).entries(cx), true))
            .collect();
        if let Some(draft) = self.current_mut() {
            draft.env.variables = variables;
        }
        cx.notify();
    }

    fn select(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.drafts.get(ix) else {
            return;
        };
        let (secrets, plain): (Vec<_>, Vec<_>) = draft.env.variables.iter().partition(|v| v.secret);
        let rows = |vars: Vec<&EnvVariable>| -> Vec<KeyValue> {
            vars.into_iter()
                .map(|v| KeyValue {
                    name: v.name.clone(),
                    value: v.value.clone(),
                    enabled: v.enabled,
                })
                .collect()
        };
        let (plain, secrets, name) = (rows(plain), rows(secrets), draft.env.name.clone());
        self.selected = Some(ix);
        self.name
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.vars
            .update(cx, |editor, cx| editor.set_entries(&plain, window, cx));
        self.secrets
            .update(cx, |editor, cx| editor.set_entries(&secrets, window, cx));
        cx.notify();
    }

    /// Add an environment named `base` (or `base 2`, ...) and select it.
    fn add(
        &mut self,
        base: String,
        variables: Vec<EnvVariable>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let taken = |name: &str| self.drafts.iter().any(|d| same_name(&d.env.name, name));
        let mut name = base.clone();
        let mut n = 2;
        while taken(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        self.drafts.push(Draft {
            saved: None,
            env: Environment { name, variables },
        });
        self.select(self.drafts.len() - 1, window, cx);
        self.name.update(cx, |input, cx| input.focus(window, cx));
    }

    fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(env) = self
            .selected
            .and_then(|ix| self.drafts.get(ix))
            .map(|d| &d.env)
        else {
            return;
        };
        let (base, variables) = (format!("{} Copy", env.name.trim()), env.variables.clone());
        self.add(base, variables, window, cx);
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected else {
            return;
        };
        let draft = self.drafts.remove(ix);
        if let Some(saved) = draft.saved {
            self.deleted.push(saved.name);
        }
        self.error = None;
        if self.drafts.is_empty() {
            self.selected = None;
            cx.notify();
        } else {
            self.select(ix.min(self.drafts.len() - 1), window, cx);
        }
    }

    /// Write every change to disk and to `secrets`. Returns the name of the
    /// selected environment, or an error message (and writes nothing) when a
    /// name is empty or duplicated.
    pub fn apply(
        &mut self,
        secrets: &mut SecretStore,
        cx: &mut Context<Self>,
    ) -> Result<Option<String>, SharedString> {
        let result = self.try_apply(secrets);
        if let Err(e) = &result {
            self.error = Some(e.clone());
            cx.notify();
        }
        result
    }

    fn try_apply(&mut self, secrets: &mut SecretStore) -> Result<Option<String>, SharedString> {
        for draft in &mut self.drafts {
            draft.env.name = draft.env.name.trim().to_owned();
        }
        for (ix, draft) in self.drafts.iter().enumerate() {
            let name = &draft.env.name;
            if name.is_empty() {
                return Err("Every environment needs a name.".into());
            }
            if self.drafts[..ix]
                .iter()
                .any(|d| same_name(&d.env.name, name))
            {
                return Err(format!("There are two environments named “{name}”.").into());
            }
        }

        let error = |e: davi_core::CoreError| SharedString::from(e.to_string());
        // Remove old files first, so renames can swap names.
        let renamed = self.drafts.iter().filter_map(|d| {
            let saved = d.saved.as_ref()?;
            // Exact comparison: a case-only rename must replace the file too.
            (saved.name != d.env.name).then_some(&saved.name)
        });
        for name in self.deleted.iter().chain(renamed) {
            collection::delete_environment(&self.root, name).map_err(error)?;
            secrets.remove(&self.root, name);
        }
        for draft in &mut self.drafts {
            if draft.saved.as_ref() != Some(&draft.env) {
                collection::save_environment(&self.root, &draft.env).map_err(error)?;
            }
            let previous = draft.saved.as_ref().map(|s| s.name.as_str());
            secrets.store(&self.root, previous, &draft.env);
            draft.saved = Some(draft.env.clone());
        }
        self.deleted.clear();

        Ok(self
            .selected
            .and_then(|ix| self.drafts.get(ix))
            .map(|d| d.env.name.clone()))
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(180.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_0p5()
            .children(self.drafts.iter().enumerate().map(|(ix, draft)| {
                let selected = self.selected == Some(ix);
                let name = draft.env.name.trim();
                let label: SharedString = if name.is_empty() {
                    "Untitled".into()
                } else {
                    name.to_owned().into()
                };
                div()
                    .id(("env-item", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .truncate()
                    .text_sm()
                    .when(selected, |el| {
                        el.bg(c(theme::ELEVATED)).text_color(c(theme::TEXT))
                    })
                    .when(!selected, |el| {
                        el.text_color(c(theme::TEXT_MUTED))
                            .hover(|s| s.bg(c(theme::ELEVATED)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.select(ix, window, cx)))
                    .child(label)
            }))
            .child(link("env-new", "+ New Environment").on_click(cx.listener(
                |this, _, window, cx| this.add("New Environment".into(), Vec::new(), window, cx),
            )))
    }

    fn render_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.selected.is_none() {
            return div()
                .flex_1()
                .py_2()
                .text_sm()
                .text_color(c(theme::TEXT_FAINT))
                .child("No environments. Create one to define variables like {{baseUrl}}.");
        }
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.name).small()),
                    )
                    .child(
                        link("env-duplicate", "Duplicate").on_click(
                            cx.listener(|this, _, window, cx| this.duplicate(window, cx)),
                        ),
                    )
                    .child(
                        link("env-delete", "Delete")
                            .text_color(c(theme::ERROR))
                            .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
                    ),
            )
            .child(section(
                "VARIABLES",
                "Use them as {{name}} in URLs, params, headers, auth and bodies.",
                self.vars.clone(),
            ))
            .child(section(
                "SECRETS",
                "Only the names go into the environment file; values stay on this computer.",
                self.secrets.clone(),
            ))
    }
}

impl Render for EnvironmentManager {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .gap_4()
                    .min_h(px(280.))
                    .child(self.render_list(cx))
                    .child(div().w(px(1.)).flex_none().bg(c(theme::BORDER)))
                    .child(self.render_details(cx)),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_sm().text_color(c(theme::ERROR)).child(error))
            })
    }
}

/// Environment names map to file names, so compare them the way the file
/// system will see them.
fn same_name(a: &str, b: &str) -> bool {
    collection::sanitize_file_name(a).to_lowercase()
        == collection::sanitize_file_name(b).to_lowercase()
}

fn section(
    title: &'static str,
    description: &'static str,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(c(theme::TEXT_FAINT))
                .child(title),
        )
        .child(
            div()
                .pb_1()
                .text_xs()
                .text_color(c(theme::TEXT_MUTED))
                .child(description),
        )
        .child(content)
}

fn link(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_2()
        .py_0p5()
        .rounded_md()
        .cursor_pointer()
        .text_xs()
        .text_color(c(theme::ACCENT))
        .hover(|s| s.bg(c(theme::ELEVATED)))
        .child(label)
}

/// Explains where folder (or collection) variables apply.
pub fn folder_vars_hint(is_root: bool) -> &'static str {
    if is_root {
        "Available to every request in the collection. Environment variables take precedence over these."
    } else {
        "Available to every request in this folder and its subfolders. They override environment and collection variables; deeper folders win."
    }
}
