//! One open request tab: editable request (method, URL, params, headers,
//! auth, body, vars) on the left, response on the right.
//!
//! Every input writes straight into `self.request` on change, so the dirty
//! indicator, saving and sending always see the latest edits. Programmatic
//! updates (e.g. URL ↔ query-param sync) are made idempotent by comparing
//! against the model first, because input change events are delivered after
//! the update that caused them.

use std::path::PathBuf;
use std::sync::Arc;

use davi_core::CoreError;
use davi_core::collection;
use davi_core::env::VarScope;
use davi_core::model::{ApiKeyPlacement, Auth, BodyMode, HttpMethod, HttpRequest, KeyValue};
use davi_net::{Canceller, HttpEngine, NetError};
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, SharedString, Subscription,
    Window, deferred, div, prelude::*, px,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Sizable, WindowExt};

use crate::kv_editor::{KvEditor, KvEvent};
use crate::response_view::ResponseView;
use crate::theme::{self, c};

/// Which `Vec<KeyValue>` of the request a key/value editor writes into.
type KvField = fn(&mut HttpRequest) -> &mut Vec<KeyValue>;

pub enum EditorEvent {
    /// The request model changed (dirty state, title or method may differ).
    Changed,
    /// The user pressed Enter / Ctrl+Enter in the URL bar.
    SendRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorTab {
    Params,
    Headers,
    Auth,
    Body,
    Vars,
}

impl EditorTab {
    const ALL: [EditorTab; 5] = [
        EditorTab::Params,
        EditorTab::Headers,
        EditorTab::Auth,
        EditorTab::Body,
        EditorTab::Vars,
    ];

    fn label(self) -> &'static str {
        match self {
            EditorTab::Params => "Params",
            EditorTab::Headers => "Headers",
            EditorTab::Auth => "Auth",
            EditorTab::Body => "Body",
            EditorTab::Vars => "Vars",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseTab {
    Body,
    Headers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthKind {
    None,
    Inherit,
    Bearer,
    Basic,
    ApiKey,
}

impl AuthKind {
    const ALL: [(AuthKind, &'static str); 5] = [
        (AuthKind::None, "No Auth"),
        (AuthKind::Inherit, "Inherit"),
        (AuthKind::Bearer, "Bearer"),
        (AuthKind::Basic, "Basic"),
        (AuthKind::ApiKey, "API Key"),
    ];
}

const BODY_MODES: [(BodyMode, &str); 6] = [
    (BodyMode::None, "None"),
    (BodyMode::Json, "JSON"),
    (BodyMode::Text, "Text"),
    (BodyMode::Xml, "XML"),
    (BodyMode::FormUrlEncoded, "Form URL Encoded"),
    (BodyMode::MultipartForm, "Multipart"),
];

enum ResponseState {
    Idle,
    Loading { id: u64, canceller: Canceller },
    Ready(Arc<ResponseView>),
    Failed(SharedString),
}

struct AuthInputs {
    token: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    key: Entity<InputState>,
    value: Entity<InputState>,
}

pub struct RequestEditor {
    path: PathBuf,
    saved: HttpRequest,
    request: HttpRequest,

    url: Entity<InputState>,
    query: Entity<KvEditor>,
    path_params: Entity<KvEditor>,
    headers: Entity<KvEditor>,
    auth: AuthInputs,
    body_text: Entity<InputState>,
    form: Entity<KvEditor>,
    multipart: Entity<KvEditor>,
    vars_pre: Entity<KvEditor>,
    vars_post: Entity<KvEditor>,

    editor_tab: EditorTab,
    response_tab: ResponseTab,
    response: ResponseState,
    /// Read-only, selectable viewers for the last response.
    response_body: Entity<InputState>,
    response_headers: Entity<InputState>,
    response_error: Entity<InputState>,
    method_menu_open: bool,
    next_request_id: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for RequestEditor {}

impl RequestEditor {
    pub fn new(
        path: PathBuf,
        request: HttpRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = |value: &str,
                     placeholder: &'static str,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            let value = value.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let kv = |entries: &[KeyValue],
                  name: &'static str,
                  value: &'static str,
                  window: &mut Window,
                  cx: &mut Context<Self>| {
            let entries = entries.to_vec();
            cx.new(|cx| KvEditor::new(&entries, name, value, window, cx))
        };

        let url = input(
            &request.url,
            "https://api.example.com/users or {{baseUrl}}/users",
            window,
            cx,
        );
        let query = kv(&request.query_params, "param", "value", window, cx);
        let path_params = kv(&request.path_params, "param", "value", window, cx);
        let headers = kv(&request.headers, "Header", "value", window, cx);
        let form = kv(&request.body.form_urlencoded, "key", "value", window, cx);
        let multipart = kv(
            &request.body.multipart_form,
            "key",
            "value or @file(path)",
            window,
            cx,
        );
        let vars_pre = kv(&request.vars.pre_request, "variable", "value", window, cx);
        let vars_post = kv(
            &request.vars.post_response,
            "variable",
            "res.body.path",
            window,
            cx,
        );

        let (token, username, password, key, value, _placement) = auth_fields(&request.auth);
        let auth = AuthInputs {
            token: input(&token, "token or {{token}}", window, cx),
            username: input(&username, "username", window, cx),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("password")
                    .masked(true)
                    .default_value(password)
            }),
            key: input(&key, "X-API-Key", window, cx),
            value: input(&value, "key value", window, cx),
        };

        let body_language = body_language(&request.body.mode);
        let body_value = body_text(&request.body).unwrap_or_default().to_owned();
        let body_text = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor(body_language)
                .line_number(true)
                .default_value(body_value)
        });

        let viewer = |language: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .code_editor(language)
                    .line_number(language != "text")
                    .soft_wrap(false)
                    .searchable(true)
            })
        };
        let response_body = viewer("json", window, cx);
        let response_headers = viewer("text", window, cx);
        let response_error =
            cx.new(|cx| InputState::new(window, cx).multi_line(true).soft_wrap(true));

        let mut subscriptions = Vec::new();

        subscriptions.push(
            cx.subscribe_in(&url, window, |this, input, event, window, cx| match event {
                InputEvent::Change => {
                    let value = input.read(cx).value().to_string();
                    this.on_url_changed(value, window, cx);
                }
                InputEvent::PressEnter { .. } => cx.emit(EditorEvent::SendRequested),
                _ => {}
            }),
        );
        subscriptions.push(
            cx.subscribe_in(&body_text, window, |this, input, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = input.read(cx).value().to_string();
                    if let Some(slot) = body_text_mut(&mut this.request.body)
                        && slot.as_deref() != Some(value.as_str())
                    {
                        *slot = Some(value);
                        this.changed(cx);
                    }
                }
            }),
        );
        for field in [
            &auth.token,
            &auth.username,
            &auth.password,
            &auth.key,
            &auth.value,
        ] {
            subscriptions.push(cx.subscribe_in(field, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.rebuild_auth(cx);
                }
            }));
        }

        subscriptions.push(cx.subscribe_in(
            &query,
            window,
            |this, editor, _: &KvEvent, window, cx| {
                this.request.query_params = editor.read(cx).entries(cx);
                this.sync_url_from_params(window, cx);
                this.changed(cx);
            },
        ));
        let kv_targets: [(&Entity<KvEditor>, KvField); 6] = [
            (&path_params, |r| &mut r.path_params),
            (&headers, |r| &mut r.headers),
            (&form, |r| &mut r.body.form_urlencoded),
            (&multipart, |r| &mut r.body.multipart_form),
            (&vars_pre, |r| &mut r.vars.pre_request),
            (&vars_post, |r| &mut r.vars.post_response),
        ];
        for (editor, field) in kv_targets {
            subscriptions.push(cx.subscribe_in(
                editor,
                window,
                move |this, editor, _: &KvEvent, _, cx| {
                    *field(&mut this.request) = editor.read(cx).entries(cx);
                    this.changed(cx);
                },
            ));
        }

        Self {
            path,
            saved: request.clone(),
            request,
            url,
            query,
            path_params,
            headers,
            auth,
            body_text,
            form,
            multipart,
            vars_pre,
            vars_post,
            editor_tab: EditorTab::Params,
            response_tab: ResponseTab::Body,
            response_body,
            response_headers,
            response_error,
            response: ResponseState::Idle,
            method_menu_open: false,
            next_request_id: 0,
            _subscriptions: subscriptions,
        }
    }

    // -- accessors -----------------------------------------------------------

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn method(&self) -> HttpMethod {
        self.request.method
    }

    pub fn is_dirty(&self) -> bool {
        self.request != self.saved
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.response, ResponseState::Loading { .. })
    }

    pub fn title(&self) -> SharedString {
        if self.request.meta.name.is_empty() {
            self.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into()
        } else {
            self.request.meta.name.clone().into()
        }
    }

    pub fn focus_url(&self, window: &mut Window, cx: &mut App) {
        self.url.update(cx, |input, cx| input.focus(window, cx));
    }

    // -- model updates -------------------------------------------------------

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn on_url_changed(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if url == self.request.url {
            return;
        }
        // Query params mirror the URL's query string; disabled params (which
        // are not part of the URL) are kept.
        let mut params: Vec<KeyValue> = parse_query(&url)
            .into_iter()
            .map(|(k, v)| KeyValue::new(k, v))
            .collect();
        params.extend(
            self.request
                .query_params
                .iter()
                .filter(|p| !p.enabled)
                .cloned(),
        );
        self.request.url = url;
        if params != self.request.query_params {
            self.request.query_params = params.clone();
            self.query
                .update(cx, |editor, cx| editor.set_entries(&params, window, cx));
        }
        self.changed(cx);
    }

    fn sync_url_from_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = with_query(&self.request.url, &self.request.query_params);
        if url != self.request.url {
            self.request.url = url.clone();
            self.url
                .update(cx, |input, cx| input.set_value(url, window, cx));
        }
    }

    fn set_method(&mut self, method: HttpMethod, cx: &mut Context<Self>) {
        self.method_menu_open = false;
        if self.request.method != method {
            self.request.method = method;
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    fn auth_kind(&self) -> Option<AuthKind> {
        Some(match self.request.auth {
            Auth::None => AuthKind::None,
            Auth::Inherit => AuthKind::Inherit,
            Auth::Bearer { .. } => AuthKind::Bearer,
            Auth::Basic { .. } => AuthKind::Basic,
            Auth::ApiKey { .. } => AuthKind::ApiKey,
            Auth::Unsupported { .. } => return None,
        })
    }

    fn set_auth_kind(&mut self, kind: AuthKind, cx: &mut Context<Self>) {
        self.request.auth = match kind {
            AuthKind::None => Auth::None,
            AuthKind::Inherit => Auth::Inherit,
            // Placeholder variants; `rebuild_auth` fills in the field values.
            AuthKind::Bearer => Auth::Bearer {
                token: String::new(),
            },
            AuthKind::Basic => Auth::Basic {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::ApiKey => Auth::ApiKey {
                key: String::new(),
                value: String::new(),
                placement: ApiKeyPlacement::Header,
            },
        };
        self.rebuild_auth(cx);
    }

    fn rebuild_auth(&mut self, cx: &mut Context<Self>) {
        let read = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let auth = match &self.request.auth {
            Auth::Bearer { .. } => Auth::Bearer {
                token: read(&self.auth.token),
            },
            Auth::Basic { .. } => Auth::Basic {
                username: read(&self.auth.username),
                password: read(&self.auth.password),
            },
            Auth::ApiKey { placement, .. } => Auth::ApiKey {
                key: read(&self.auth.key),
                value: read(&self.auth.value),
                placement: *placement,
            },
            other => other.clone(),
        };
        if auth != self.request.auth {
            self.request.auth = auth;
        }
        self.changed(cx);
    }

    fn toggle_api_key_placement(&mut self, cx: &mut Context<Self>) {
        if let Auth::ApiKey { placement, .. } = &mut self.request.auth {
            *placement = match placement {
                ApiKeyPlacement::Header => ApiKeyPlacement::QueryParams,
                ApiKeyPlacement::QueryParams => ApiKeyPlacement::Header,
            };
            self.changed(cx);
        }
    }

    fn set_body_mode(&mut self, mode: BodyMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.request.body.mode == mode {
            return;
        }
        self.request.body.mode = mode.clone();
        // Bruno keeps one payload per mode; load the new mode's text.
        if let Some(text) = body_text(&self.request.body).map(str::to_owned) {
            let language = body_language(&mode);
            self.body_text.update(cx, |input, cx| {
                input.set_highlighter(language, cx);
                input.set_value(text, window, cx);
            });
        }
        self.changed(cx);
    }

    // -- persistence & execution -------------------------------------------

    pub fn save(&mut self, cx: &mut Context<Self>) -> Result<(), CoreError> {
        collection::save_request(&self.path, &self.request)?;
        self.saved = self.request.clone();
        self.changed(cx);
        Ok(())
    }

    pub fn send(
        &mut self,
        engine: &HttpEngine,
        scope: VarScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let ResponseState::Loading { canceller, .. } = &self.response {
            canceller.cancel();
            self.response = ResponseState::Idle;
            cx.notify();
            return;
        }
        if self.request.url.trim().is_empty() {
            self.show_state(
                ResponseState::Failed("Enter a URL first".into()),
                window,
                cx,
            );
            return;
        }

        let id = self.next_request_id;
        self.next_request_id += 1;
        let handle = engine.send(&self.request, &scope);
        self.response = ResponseState::Loading {
            id,
            canceller: handle.canceller(),
        };
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let state = match handle.response().await {
                Ok(response) => {
                    // Pretty-print + highlight off the UI thread.
                    let view = cx
                        .background_executor()
                        .spawn(async move { ResponseView::build(response) })
                        .await;
                    ResponseState::Ready(Arc::new(view))
                }
                Err(NetError::Cancelled) => ResponseState::Idle,
                Err(e) => ResponseState::Failed(e.to_string().into()),
            };
            this.update_in(cx, |this, window, cx| {
                // Ignore results of requests that were superseded.
                if matches!(this.response, ResponseState::Loading { id: current, .. } if current == id)
                {
                    this.show_state(state, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Load a finished response (or error) into the read-only viewers.
    fn show_state(&mut self, state: ResponseState, window: &mut Window, cx: &mut Context<Self>) {
        match &state {
            ResponseState::Ready(view) => {
                let (body, language, headers) =
                    (view.body.clone(), view.language, view.headers_text());
                self.response_body.update(cx, |input, cx| {
                    input.set_highlighter(language, cx);
                    input.set_value(body, window, cx);
                });
                self.response_headers
                    .update(cx, |input, cx| input.set_value(headers, window, cx));
            }
            ResponseState::Failed(error) => {
                let error = error.to_string();
                self.response_error
                    .update(cx, |input, cx| input.set_value(error, window, cx));
            }
            ResponseState::Idle | ResponseState::Loading { .. } => {}
        }
        self.response = state;
        cx.notify();
    }

    fn copy_response(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ResponseState::Ready(view) = &self.response else {
            return;
        };
        let (text, what) = match self.response_tab {
            ResponseTab::Body => (view.body.clone(), "Response body"),
            ResponseTab::Headers => (view.headers_text(), "Response headers"),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(format!("{what} copied to clipboard"), cx);
    }

    pub fn cancel(&mut self) {
        if let ResponseState::Loading { canceller, .. } = &self.response {
            canceller.cancel();
        }
    }

    // -- rendering -----------------------------------------------------------

    fn render_url_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.is_loading();
        let method = self.request.method;
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        div()
                            .id("method")
                            .w(px(92.))
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .rounded_md()
                            .bg(c(theme::SURFACE))
                            .border_1()
                            .border_color(c(theme::BORDER))
                            .cursor_pointer()
                            .hover(|s| s.border_color(c(theme::HOVER)))
                            .text_sm()
                            .text_color(theme::method_color(method))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.method_menu_open = !this.method_menu_open;
                                cx.notify();
                            }))
                            .child(method.as_str())
                            .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("▾")),
                    )
                    .when(self.method_menu_open, |el| {
                        // Deferred so the menu paints above the editor below it.
                        el.child(deferred(
                            div()
                                .absolute()
                                .top(px(34.))
                                .left_0()
                                .w(px(120.))
                                .py_1()
                                .flex()
                                .flex_col()
                                .bg(c(theme::SURFACE))
                                .border_1()
                                .border_color(c(theme::HOVER))
                                .rounded_md()
                                .shadow_lg()
                                .occlude()
                                .children(HttpMethod::ALL.into_iter().map(|m| {
                                    div()
                                        .id(m.as_str())
                                        .px_3()
                                        .py_1()
                                        .cursor_pointer()
                                        .text_sm()
                                        .text_color(theme::method_color(m))
                                        .hover(|s| s.bg(c(theme::ELEVATED)))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_method(m, cx)
                                            }),
                                        )
                                        .child(m.as_str())
                                })),
                        ))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(theme::MONO_FONT)
                    .child(Input::new(&self.url)),
            )
            .child(
                div()
                    .id("send")
                    .flex_none()
                    .h(px(30.))
                    .px_4()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .bg(c(if loading { theme::ERROR } else { theme::ACCENT }))
                    .text_color(c(theme::SURFACE_ALT))
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(EditorEvent::SendRequested)))
                    .child(if loading { "Cancel" } else { "Send" }),
            )
    }

    fn render_editor_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let req = &self.request;
        let enabled = |kvs: &[KeyValue]| kvs.iter().filter(|kv| kv.enabled).count();
        let count = |t: EditorTab| -> usize {
            match t {
                EditorTab::Params => enabled(&req.query_params) + enabled(&req.path_params),
                EditorTab::Headers => enabled(&req.headers),
                EditorTab::Vars => {
                    enabled(&req.vars.pre_request) + enabled(&req.vars.post_response)
                }
                EditorTab::Auth | EditorTab::Body => 0,
            }
        };
        let marker = |t: EditorTab| -> Option<&'static str> {
            match t {
                EditorTab::Auth if !matches!(req.auth, Auth::None) => Some("•"),
                EditorTab::Body if req.body.mode != BodyMode::None => Some("•"),
                _ => None,
            }
        };
        div()
            .flex()
            .gap_4()
            .px_3()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(EditorTab::ALL.into_iter().map(|t| {
                let selected = self.editor_tab == t;
                let n = count(t);
                let label = match (n, marker(t)) {
                    (n, _) if n > 0 => format!("{} {n}", t.label()),
                    (_, Some(m)) => format!("{} {m}", t.label()),
                    _ => t.label().to_owned(),
                };
                div()
                    .id(t.label())
                    .py_2()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(c(if selected {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    }))
                    .when(selected, |el| {
                        el.border_b_2().border_color(c(theme::ACCENT))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.editor_tab = t;
                        cx.notify();
                    }))
                    .child(label)
            }))
    }

    fn render_editor_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.editor_tab {
            EditorTab::Params => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(section("Query", self.query.clone()))
                .child(section(
                    "Path  (use :name in the URL)",
                    self.path_params.clone(),
                ))
                .into_any_element(),
            EditorTab::Headers => section("Headers", self.headers.clone()).into_any_element(),
            EditorTab::Auth => self.render_auth(cx),
            EditorTab::Body => self.render_body(cx),
            EditorTab::Vars => div()
                .flex()
                .flex_col()
                .gap_4()
                .child(section("Pre Request", self.vars_pre.clone()))
                .child(section("Post Response", self.vars_post.clone()))
                .into_any_element(),
        }
    }

    fn render_auth(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.auth_kind();
        let modes = segmented(
            "auth-mode",
            AuthKind::ALL
                .iter()
                .map(|(kind, label)| (*label, current == Some(*kind))),
            cx.listener(|this, ix: &usize, _, cx| this.set_auth_kind(AuthKind::ALL[*ix].0, cx)),
        );
        let field = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(90.))
                        .flex_none()
                        .text_sm()
                        .text_color(c(theme::TEXT_MUTED))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(Input::new(input).small()))
        };
        let fields: AnyElement = match &self.request.auth {
            Auth::Bearer { .. } => field("Token", &self.auth.token).into_any_element(),
            Auth::Basic { .. } => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(field("Username", &self.auth.username))
                .child(field("Password", &self.auth.password))
                .into_any_element(),
            Auth::ApiKey { placement, .. } => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(field("Key", &self.auth.key))
                .child(field("Value", &self.auth.value))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .w(px(90.))
                                .text_sm()
                                .text_color(c(theme::TEXT_MUTED))
                                .child("Add to"),
                        )
                        .child(segmented(
                            "apikey-placement",
                            [
                                ("Header", *placement == ApiKeyPlacement::Header),
                                ("Query Params", *placement == ApiKeyPlacement::QueryParams),
                            ]
                            .into_iter(),
                            cx.listener(|this, _: &usize, _, cx| this.toggle_api_key_placement(cx)),
                        )),
                )
                .into_any_element(),
            Auth::Inherit => hint("Uses the auth configured on the collection.").into_any_element(),
            Auth::None => hint("This request does not use authorization.").into_any_element(),
            Auth::Unsupported { mode } => hint(format!(
                "Auth mode `{mode}` is preserved but not editable yet."
            ))
            .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(modes)
            .child(fields)
            .into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = &self.request.body.mode;
        let modes = segmented(
            "body-mode",
            BODY_MODES.iter().map(|(m, label)| (*label, m == mode)),
            cx.listener(|this, ix: &usize, window, cx| {
                this.set_body_mode(BODY_MODES[*ix].0.clone(), window, cx)
            }),
        );
        let content: AnyElement = match mode {
            BodyMode::None => hint("This request has no body.").into_any_element(),
            BodyMode::FormUrlEncoded => self.form.clone().into_any_element(),
            BodyMode::MultipartForm => self.multipart.clone().into_any_element(),
            BodyMode::Other(other) => hint(format!(
                "Body mode `{other}` is preserved but not editable yet."
            ))
            .into_any_element(),
            _ => div()
                .h(px(320.))
                .font_family(theme::MONO_FONT)
                .child(Input::new(&self.body_text).h_full())
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(modes)
            .child(content)
            .into_any_element()
    }

    fn render_response(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = div()
            .flex_1()
            .flex_basis(px(0.))
            .min_w_0()
            .overflow_hidden()
            .h_full()
            .flex()
            .flex_col();

        let view = match &self.response {
            ResponseState::Idle => {
                return panel.child(placeholder(format!(
                    "Press {} or click Send",
                    shortcut("Enter")
                )));
            }
            ResponseState::Loading { .. } => return panel.child(placeholder("Sending…".into())),
            ResponseState::Failed(_) => {
                return panel.child(
                    div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_color(c(theme::ERROR)).child("Request failed"))
                        .child(
                            div()
                                .h(px(120.))
                                .font_family(theme::MONO_FONT)
                                .child(Input::new(&self.response_error).disabled(true).h_full()),
                        ),
                );
            }
            ResponseState::Ready(view) => view.clone(),
        };

        let metric = |label: &'static str, value: SharedString| {
            div()
                .flex()
                .gap_1()
                .text_sm()
                .child(div().text_color(c(theme::TEXT_FAINT)).child(label))
                .child(value)
        };
        let header = div()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .h(px(47.))
            .flex_none()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .px_2()
                    .rounded_sm()
                    .text_sm()
                    .text_color(theme::status_color(view.status))
                    .bg(c(theme::SURFACE))
                    .child(view.status_text.clone()),
            )
            .child(metric("Time", view.time.clone()))
            .child(metric("Size", view.size.clone()))
            .when(view.truncated, |el| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(c(theme::WARNING))
                        .child("truncated"),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("copy-response")
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .text_color(c(theme::TEXT_MUTED))
                    .hover(|s| s.bg(c(theme::ELEVATED)).text_color(c(theme::TEXT)))
                    .on_click(cx.listener(|this, _, window, cx| this.copy_response(window, cx)))
                    .child("Copy"),
            );

        let tabs = div()
            .flex()
            .gap_4()
            .px_3()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(
                [
                    (ResponseTab::Body, "Body".to_owned()),
                    (
                        ResponseTab::Headers,
                        format!("Headers {}", view.headers.len()),
                    ),
                ]
                .into_iter()
                .map(|(t, label)| {
                    let selected = self.response_tab == t;
                    div()
                        .id(SharedString::from(format!("response-tab-{label}")))
                        .py_2()
                        .cursor_pointer()
                        .text_sm()
                        .text_color(c(if selected {
                            theme::TEXT
                        } else {
                            theme::TEXT_MUTED
                        }))
                        .when(selected, |el| {
                            el.border_b_2().border_color(c(theme::ACCENT))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.response_tab = t;
                            cx.notify();
                        }))
                        .child(label)
                }),
            );

        // Read-only code editors: selectable, Ctrl+C / Ctrl+A, Ctrl+F search,
        // horizontal scrolling for long lines.
        let viewer = match self.response_tab {
            ResponseTab::Body => &self.response_body,
            ResponseTab::Headers => &self.response_headers,
        };
        let body = div()
            .size_full()
            .font_family(theme::MONO_FONT)
            .child(Input::new(viewer).disabled(true).bordered(false).h_full());

        panel
            .child(header)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
    }
}

impl Render for RequestEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w_0()
                    .overflow_hidden()
                    .h_full()
                    .flex()
                    .flex_col()
                    .border_r_1()
                    .border_color(c(theme::BORDER))
                    .child(self.render_url_bar(cx))
                    .child(self.render_editor_tabs(cx))
                    .child(
                        div()
                            .id("request-content")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_3()
                            .child(self.render_editor_content(cx)),
                    ),
            )
            .child(self.render_response(cx))
    }
}

// -- helpers -------------------------------------------------------------------

fn auth_fields(auth: &Auth) -> (String, String, String, String, String, ApiKeyPlacement) {
    let mut out = Default::default();
    let (token, username, password, key, value, placement) = &mut out;
    match auth {
        Auth::Bearer { token: t } => *token = t.clone(),
        Auth::Basic {
            username: u,
            password: p,
        } => {
            *username = u.clone();
            *password = p.clone();
        }
        Auth::ApiKey {
            key: k,
            value: v,
            placement: pl,
        } => {
            *key = k.clone();
            *value = v.clone();
            *placement = *pl;
        }
        _ => {}
    }
    out
}

fn body_text(body: &davi_core::model::RequestBody) -> Option<&str> {
    match body.mode {
        BodyMode::Json => Some(body.json.as_deref().unwrap_or_default()),
        BodyMode::Text => Some(body.text.as_deref().unwrap_or_default()),
        BodyMode::Xml => Some(body.xml.as_deref().unwrap_or_default()),
        BodyMode::Sparql => Some(body.sparql.as_deref().unwrap_or_default()),
        BodyMode::Graphql => Some(body.graphql.as_deref().unwrap_or_default()),
        _ => None,
    }
}

fn body_text_mut(body: &mut davi_core::model::RequestBody) -> Option<&mut Option<String>> {
    match body.mode {
        BodyMode::Json => Some(&mut body.json),
        BodyMode::Text => Some(&mut body.text),
        BodyMode::Xml => Some(&mut body.xml),
        BodyMode::Sparql => Some(&mut body.sparql),
        BodyMode::Graphql => Some(&mut body.graphql),
        _ => None,
    }
}

fn body_language(mode: &BodyMode) -> &'static str {
    match mode {
        BodyMode::Json => "json",
        BodyMode::Xml => "html",
        BodyMode::Graphql => "graphql",
        _ => "text",
    }
}

/// `a=1&b=2` pairs of a URL's query string (fragment excluded), raw so that
/// `{{vars}}` survive untouched.
pub fn parse_query(url: &str) -> Vec<(String, String)> {
    let Some((_, query)) = url.split_once('?') else {
        return Vec::new();
    };
    let query = query.split('#').next().unwrap_or_default();
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_owned(), v.to_owned()),
            None => (pair.to_owned(), String::new()),
        })
        .collect()
}

/// `url` with its query string rebuilt from the enabled `params`.
pub fn with_query(url: &str, params: &[KeyValue]) -> String {
    let (base, rest) = match url.find(['?', '#']) {
        Some(i) => url.split_at(i),
        None => (url, ""),
    };
    let fragment = rest.find('#').map(|i| &rest[i..]).unwrap_or_default();
    let query: Vec<String> = params
        .iter()
        .filter(|p| p.enabled && !p.name.is_empty())
        .map(|p| {
            if p.value.is_empty() {
                p.name.clone()
            } else {
                format!("{}={}", p.name, p.value)
            }
        })
        .collect();
    if query.is_empty() {
        format!("{base}{fragment}")
    } else {
        format!("{base}?{}{fragment}", query.join("&"))
    }
}

fn section(title: &'static str, content: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(c(theme::TEXT_FAINT))
                .child(title),
        )
        .child(content)
}

fn hint(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .py_2()
        .text_sm()
        .text_color(c(theme::TEXT_FAINT))
        .child(text.into())
}

fn placeholder(text: String) -> impl IntoElement {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(c(theme::TEXT_FAINT))
        .child(text)
}

pub fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// A row of mutually exclusive pill buttons. `on_select` receives the index.
fn segmented<'a>(
    id: &'static str,
    options: impl Iterator<Item = (&'a str, bool)>,
    on_select: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_select = std::rc::Rc::new(on_select);
    div()
        .flex()
        .flex_wrap()
        .gap_1()
        .children(options.enumerate().map(|(ix, (label, selected))| {
            let on_select = on_select.clone();
            div()
                .id((id, ix))
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .border_1()
                .when(selected, |el| {
                    el.bg(c(theme::ELEVATED))
                        .border_color(c(theme::ACCENT))
                        .text_color(c(theme::TEXT))
                })
                .when(!selected, |el| {
                    el.border_color(c(theme::BORDER))
                        .text_color(c(theme::TEXT_MUTED))
                        .hover(|s| s.bg(c(theme::ELEVATED)))
                })
                .on_click(move |_, window, cx| on_select(&ix, window, cx))
                .child(SharedString::from(label.to_owned()))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_round_trip() {
        let url = "{{baseUrl}}/users?page=1&q={{term}}&flag#top";
        assert_eq!(
            parse_query(url),
            [
                ("page".into(), "1".into()),
                ("q".into(), "{{term}}".into()),
                ("flag".into(), String::new())
            ]
        );
        let params = vec![
            KeyValue::new("page", "2"),
            KeyValue::disabled("debug", "1"),
            KeyValue::new("flag", ""),
        ];
        assert_eq!(
            with_query(url, &params),
            "{{baseUrl}}/users?page=2&flag#top"
        );
        assert_eq!(with_query("http://a/b?x=1", &[]), "http://a/b");
        assert_eq!(
            with_query("http://a/b", &[KeyValue::new("x", "1")]),
            "http://a/b?x=1"
        );
    }
}
