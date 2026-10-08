//! Root view: sidebar | (tab bar / request panel | response panel), plus a
//! status bar and the command palette overlay.
//!
//! All application state lives in this one entity for now; render helpers
//! are split per panel. As panels gain editable state (text inputs, code
//! editors) they graduate into their own entities, the way the palette
//! already has.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use davi_core::collection::{self, Collection, Folder, Node};
use davi_core::env::VarScope;
use davi_core::model::{Auth, BodyMode, HttpMethod, HttpRequest, KeyValue};
use davi_net::{Canceller, HttpEngine, NetError};
use gpui::{
    AnyElement, Context, Div, Entity, FocusHandle, Focusable, MouseButton, SharedString, Stateful,
    StyledText, Subscription, Window, actions, div, prelude::*, px, uniform_list,
};

use crate::palette::{CommandPalette, PaletteEvent, PaletteItem};
use crate::response_view::ResponseView;
use crate::theme::{self, c};

actions!(
    davi,
    [
        ToggleCommandPalette,
        SendRequest,
        SaveRequest,
        CloseTab,
        NextEnvironment,
        Quit
    ]
);

const SIDEBAR_WIDTH: f32 = 260.;

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

enum ResponseState {
    Idle,
    Loading { id: u64, canceller: Canceller },
    Ready(Arc<ResponseView>),
    Failed(SharedString),
}

struct RequestTab {
    path: PathBuf,
    /// Last state written to / read from disk, for the dirty indicator.
    saved: HttpRequest,
    request: HttpRequest,
    editor_tab: EditorTab,
    response_tab: ResponseTab,
    response: ResponseState,
}

impl RequestTab {
    fn is_dirty(&self) -> bool {
        self.request != self.saved
    }

    fn title(&self) -> SharedString {
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
}

enum SidebarRow {
    Folder {
        path: PathBuf,
        name: SharedString,
        depth: usize,
        collapsed: bool,
    },
    Request {
        path: PathBuf,
        name: SharedString,
        method: HttpMethod,
        depth: usize,
    },
}

pub struct Workspace {
    focus_handle: FocusHandle,
    engine: HttpEngine,
    collection: Option<Collection>,
    collapsed: HashSet<PathBuf>,
    sidebar_rows: Vec<SidebarRow>,
    tabs: Vec<RequestTab>,
    active_tab: Option<usize>,
    environment: Option<usize>,
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    notice: Option<(SharedString, bool)>,
    next_request_id: u64,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workspace {
    pub fn new(
        engine: HttpEngine,
        collection_dir: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);

        let mut notice = None;
        let collection = collection_dir.and_then(|dir| match Collection::open(&dir) {
            Ok(c) => {
                if !c.issues.is_empty() {
                    for issue in &c.issues {
                        log::warn!("{}", issue.error);
                    }
                    notice = Some((
                        format!("{} file(s) failed to load, see log", c.issues.len()).into(),
                        true,
                    ));
                }
                Some(c)
            }
            Err(e) => {
                notice = Some((e.to_string().into(), true));
                None
            }
        });
        let environment = collection
            .as_ref()
            .and_then(|c| (!c.environments.is_empty()).then_some(0));

        let mut this = Self {
            focus_handle,
            engine,
            collection,
            collapsed: HashSet::new(),
            sidebar_rows: Vec::new(),
            tabs: Vec::new(),
            active_tab: None,
            environment,
            palette: None,
            notice,
            next_request_id: 0,
        };
        this.rebuild_sidebar();
        this
    }

    // -- state ---------------------------------------------------------------

    fn rebuild_sidebar(&mut self) {
        fn flatten(
            nodes: &[Node],
            depth: usize,
            collapsed: &HashSet<PathBuf>,
            out: &mut Vec<SidebarRow>,
        ) {
            for node in nodes {
                match node {
                    Node::Folder(f) => {
                        let is_collapsed = collapsed.contains(&f.path);
                        out.push(SidebarRow::Folder {
                            path: f.path.clone(),
                            name: f.name.clone().into(),
                            depth,
                            collapsed: is_collapsed,
                        });
                        if !is_collapsed {
                            flatten(&f.children, depth + 1, collapsed, out);
                        }
                    }
                    Node::Request(r) => out.push(SidebarRow::Request {
                        path: r.path.clone(),
                        name: r.name.clone().into(),
                        method: r.method,
                        depth,
                    }),
                }
            }
        }

        self.sidebar_rows.clear();
        if let Some(c) = &self.collection {
            flatten(&c.root.children, 0, &self.collapsed, &mut self.sidebar_rows);
        }
    }

    fn toggle_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_path_buf());
        }
        self.rebuild_sidebar();
        cx.notify();
    }

    fn active(&self) -> Option<&RequestTab> {
        self.active_tab.and_then(|ix| self.tabs.get(ix))
    }

    fn active_mut(&mut self) -> Option<&mut RequestTab> {
        self.active_tab.and_then(|ix| self.tabs.get_mut(ix))
    }

    fn open_request(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|t| t.path == path) {
            self.active_tab = Some(ix);
        } else {
            match collection::load_request(&path) {
                Ok(request) => {
                    self.tabs.push(RequestTab {
                        path,
                        saved: request.clone(),
                        request,
                        editor_tab: EditorTab::Params,
                        response_tab: ResponseTab::Body,
                        response: ResponseState::Idle,
                    });
                    self.active_tab = Some(self.tabs.len() - 1);
                }
                Err(e) => self.notice = Some((e.to_string().into(), true)),
            }
        }
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        if let ResponseState::Loading { canceller, .. } = &self.tabs[ix].response {
            canceller.cancel();
        }
        self.tabs.remove(ix);
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) => Some(active.min(self.tabs.len() - 1)),
            None => None,
        };
        cx.notify();
    }

    fn var_scope(&self) -> VarScope {
        let mut scope = VarScope::new();
        if let Some(env) = self
            .environment
            .and_then(|ix| self.collection.as_ref()?.environments.get(ix))
        {
            scope.push_layer(env.enabled_vars());
        }
        scope
    }

    fn environment_name(&self) -> SharedString {
        self.environment
            .and_then(|ix| self.collection.as_ref()?.environments.get(ix))
            .map_or_else(|| "No Environment".into(), |e| e.name.clone().into())
    }

    fn cycle_method(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.active_mut() {
            let all = HttpMethod::ALL;
            let ix = all
                .iter()
                .position(|m| *m == tab.request.method)
                .unwrap_or(0);
            tab.request.method = all[(ix + 1) % all.len()];
            cx.notify();
        }
    }

    // -- actions -------------------------------------------------------------

    fn send_request(&mut self, _: &SendRequest, _: &mut Window, cx: &mut Context<Self>) {
        let scope = self.var_scope();
        let id = self.next_request_id;
        self.next_request_id += 1;
        let engine = self.engine.clone();
        let Some(tab) = self.active_mut() else { return };

        if let ResponseState::Loading { canceller, .. } = &tab.response {
            // Second press cancels, like the button.
            canceller.cancel();
            tab.response = ResponseState::Idle;
            cx.notify();
            return;
        }

        let handle = engine.send(&tab.request, &scope);
        tab.response = ResponseState::Loading {
            id,
            canceller: handle.canceller(),
        };
        let path = tab.path.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
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
            this.update(cx, |this, cx| {
                let tab = this.tabs.iter_mut().find(|t| t.path == path);
                if let Some(tab) = tab {
                    // Ignore results of requests that were superseded.
                    if matches!(tab.response, ResponseState::Loading { id: current, .. } if current == id)
                    {
                        tab.response = state;
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn save_request(&mut self, _: &SaveRequest, _: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active_mut() else { return };
        match collection::save_request(&tab.path, &tab.request) {
            Ok(()) => {
                tab.saved = tab.request.clone();
                let (path, method) = (tab.path.clone(), tab.request.method);
                let name = tab.title();
                if let Some(c) = &mut self.collection {
                    update_summary(&mut c.root, &path, method);
                }
                self.rebuild_sidebar();
                self.notice = Some((format!("Saved {name}").into(), false));
            }
            Err(e) => self.notice = Some((e.to_string().into(), true)),
        }
        cx.notify();
    }

    fn close_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab {
            self.close_tab_at(ix, cx);
        }
    }

    fn next_environment(&mut self, _: &NextEnvironment, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.collection.as_ref().map_or(0, |c| c.environments.len());
        // Cycles through every environment and then "No Environment".
        self.environment = match self.environment {
            None if count > 0 => Some(0),
            Some(ix) if ix + 1 < count => Some(ix + 1),
            _ => None,
        };
        cx.notify();
    }

    fn toggle_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette.is_some() {
            self.dismiss_palette(window, cx);
            return;
        }
        let Some(collection) = &self.collection else {
            return;
        };
        let root = collection.root.path.clone();
        let items: Arc<[PaletteItem]> = collection
            .requests()
            .map(|r| PaletteItem {
                name: r.name.clone().into(),
                location: r
                    .path
                    .parent()
                    .and_then(|p| p.strip_prefix(&root).ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
                method: r.method,
                path: r.path.clone(),
            })
            .collect();

        let palette = cx.new(|cx| CommandPalette::new(items, cx));
        let subscription = cx.subscribe_in(&palette, window, |this, _, event, window, cx| {
            if let PaletteEvent::Confirmed(path) = event {
                this.open_request(path.clone(), cx);
            }
            this.dismiss_palette(window, cx);
        });
        window.focus(&palette.focus_handle(cx));
        self.palette = Some((palette, subscription));
        cx.notify();
    }

    fn dismiss_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    // -- rendering -----------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = self
            .collection
            .as_ref()
            .map_or_else(|| "No collection".into(), |c| c.name.clone().into());

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .flex_none()
            .bg(c(theme::SURFACE))
            .border_r_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .text_xs()
                    .text_color(c(theme::TEXT_MUTED))
                    .child(title.to_uppercase()),
            )
            .child(
                div().flex_1().min_h_0().child(
                    uniform_list(
                        "sidebar",
                        self.sidebar_rows.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|ix| this.render_sidebar_row(ix, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .size_full(),
                ),
            )
    }

    fn render_sidebar_row(&self, ix: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let active_path = self.active().map(|t| t.path.as_path());
        let row = div()
            .id(ix)
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.))
            .pr_2()
            .cursor_pointer()
            .text_sm()
            .hover(|s| s.bg(c(theme::ELEVATED)));

        match &self.sidebar_rows[ix] {
            SidebarRow::Folder {
                path,
                name,
                depth,
                collapsed,
            } => {
                let path = path.clone();
                row.pl(px(12. + *depth as f32 * 14.))
                    .text_color(c(theme::TEXT))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_folder(&path, cx)))
                    .child(
                        div()
                            .w(px(12.))
                            .text_color(c(theme::TEXT_FAINT))
                            .child(if *collapsed { "▸" } else { "▾" }),
                    )
                    .child(name.clone())
            }
            SidebarRow::Request {
                path,
                name,
                method,
                depth,
            } => {
                let is_active = active_path == Some(path.as_path());
                let path = path.clone();
                row.pl(px(12. + *depth as f32 * 14.))
                    .when(is_active, |el| el.bg(c(theme::ELEVATED)))
                    .text_color(c(theme::TEXT))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_request(path.clone(), cx)),
                    )
                    .child(
                        div()
                            .w(px(36.))
                            .flex_none()
                            .text_xs()
                            .text_color(theme::method_color(*method))
                            .child(theme::method_label(*method)),
                    )
                    .child(div().truncate().child(name.clone()))
            }
        }
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tab-bar")
            .flex()
            .flex_none()
            .h(px(36.))
            .overflow_x_scroll()
            .bg(c(theme::SURFACE_ALT))
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let active = self.active_tab == Some(ix);
                let dirty = tab.is_dirty();
                div()
                    .id(ix)
                    .group("tab")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h_full()
                    .flex_none()
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(c(theme::BORDER))
                    .when(active, |el| {
                        el.bg(c(theme::BG))
                            .border_t_2()
                            .border_color(c(theme::ACCENT))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active_tab = Some(ix);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::method_color(tab.request.method))
                            .child(theme::method_label(tab.request.method)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(c(if active {
                                theme::TEXT
                            } else {
                                theme::TEXT_MUTED
                            }))
                            .child(tab.title()),
                    )
                    .child(
                        // Dirty tabs show a dot that turns into a close button on hover.
                        div()
                            .id(("close", ix))
                            .w(px(16.))
                            .flex()
                            .justify_center()
                            .rounded_sm()
                            .text_color(c(if dirty {
                                theme::WARNING
                            } else {
                                theme::TEXT_FAINT
                            }))
                            .hover(|s| s.bg(c(theme::HOVER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(ix, cx);
                            }))
                            .child(if dirty { "●" } else { "×" }),
                    )
            }))
    }

    fn render_request_panel(&self, tab: &RequestTab, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = matches!(tab.response, ResponseState::Loading { .. });
        let req = &tab.request;

        let url_bar = div()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .child(
                div()
                    .id("method")
                    .flex_none()
                    .w(px(84.))
                    .py_1()
                    .flex()
                    .justify_center()
                    .rounded_md()
                    .bg(c(theme::SURFACE))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .cursor_pointer()
                    .hover(|s| s.border_color(c(theme::HOVER)))
                    .text_sm()
                    .text_color(theme::method_color(req.method))
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_method(cx)))
                    .child(req.method.as_str()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(c(theme::SURFACE_ALT))
                    .border_1()
                    .border_color(c(theme::BORDER))
                    .font_family(theme::MONO_FONT)
                    .text_sm()
                    .truncate()
                    .child(SharedString::from(req.url.clone())),
            )
            .child(
                div()
                    .id("send")
                    .flex_none()
                    .px_4()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .bg(c(if loading { theme::ERROR } else { theme::ACCENT }))
                    .text_color(c(theme::SURFACE_ALT))
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send_request(&SendRequest, window, cx)
                    }))
                    .child(if loading { "Cancel" } else { "Send" }),
            );

        let counts = |t: EditorTab| -> usize {
            let enabled = |kvs: &[KeyValue]| kvs.iter().filter(|kv| kv.enabled).count();
            match t {
                EditorTab::Params => enabled(&req.query_params) + enabled(&req.path_params),
                EditorTab::Headers => enabled(&req.headers),
                EditorTab::Vars => {
                    enabled(&req.vars.pre_request) + enabled(&req.vars.post_response)
                }
                EditorTab::Auth | EditorTab::Body => 0,
            }
        };
        let editor_tabs = div()
            .flex()
            .gap_4()
            .px_3()
            .border_b_1()
            .border_color(c(theme::BORDER))
            .children(EditorTab::ALL.into_iter().map(|t| {
                let selected = tab.editor_tab == t;
                let count = counts(t);
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
                        if let Some(tab) = this.active_mut() {
                            tab.editor_tab = t;
                            cx.notify();
                        }
                    }))
                    .child(if count > 0 {
                        format!("{} {count}", t.label())
                    } else {
                        t.label().to_owned()
                    })
            }));

        let content: AnyElement = match tab.editor_tab {
            EditorTab::Params => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(section("Query", kv_table(&req.query_params)))
                .when(!req.path_params.is_empty(), |el| {
                    el.child(section("Path", kv_table(&req.path_params)))
                })
                .into_any_element(),
            EditorTab::Headers => kv_table(&req.headers),
            EditorTab::Auth => render_auth(&req.auth),
            EditorTab::Body => render_body(req),
            EditorTab::Vars => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(section("Pre Request", kv_table(&req.vars.pre_request)))
                .child(section("Post Response", kv_table(&req.vars.post_response)))
                .into_any_element(),
        };

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(c(theme::BORDER))
            .child(url_bar)
            .child(editor_tabs)
            .child(
                div()
                    .id("request-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(content),
            )
    }

    fn render_response_panel(&self, tab: &RequestTab, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = div().flex_1().min_w_0().h_full().flex().flex_col();

        let view = match &tab.response {
            ResponseState::Idle => {
                return panel.child(placeholder(format!(
                    "Press {} to send the request",
                    shortcut("Enter")
                )));
            }
            ResponseState::Loading { .. } => return panel.child(placeholder("Sending…".into())),
            ResponseState::Failed(error) => {
                return panel.child(
                    div()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_color(c(theme::ERROR)).child("Request failed"))
                        .child(
                            div()
                                .font_family(theme::MONO_FONT)
                                .text_sm()
                                .text_color(c(theme::TEXT_MUTED))
                                .child(error.clone()),
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
            .h(px(45.))
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
            });

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
                    let selected = tab.response_tab == t;
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
                            if let Some(tab) = this.active_mut() {
                                tab.response_tab = t;
                                cx.notify();
                            }
                        }))
                        .child(label)
                }),
            );

        let body: AnyElement = match tab.response_tab {
            ResponseTab::Body => {
                let lines = view.lines.clone();
                // Virtualized: only visible lines are laid out, so multi-MB
                // responses scroll as smoothly as tiny ones.
                uniform_list("response-body", lines.len(), move |range, _, _| {
                    range
                        .map(|ix| {
                            let line = &lines[ix];
                            div().whitespace_nowrap().child(
                                StyledText::new(line.text.clone())
                                    .with_highlights(line.highlights.iter().cloned()),
                            )
                        })
                        .collect()
                })
                .size_full()
                .px_3()
                .py_2()
                .font_family(theme::MONO_FONT)
                .text_sm()
                .into_any_element()
            }
            ResponseTab::Headers => div()
                .id("response-headers")
                .size_full()
                .overflow_y_scroll()
                .p_3()
                .children(view.headers.iter().map(|(k, v)| {
                    div()
                        .flex()
                        .gap_3()
                        .py_0p5()
                        .text_sm()
                        .child(
                            div()
                                .w(px(200.))
                                .flex_none()
                                .text_color(c(theme::SYN_KEY))
                                .child(k.clone()),
                        )
                        .child(div().min_w_0().child(v.clone()))
                }))
                .into_any_element(),
        };

        panel
            .child(header)
            .child(tabs)
            .child(div().flex_1().min_h_0().child(body))
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let request_count = self.collection.as_ref().map_or(0, |c| c.requests().count());
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(26.))
            .px_3()
            .bg(c(theme::SURFACE_ALT))
            .border_t_1()
            .border_color(c(theme::BORDER))
            .text_xs()
            .text_color(c(theme::TEXT_MUTED))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .child(format!("{request_count} requests"))
                    .child(
                        div()
                            .id("env")
                            .cursor_pointer()
                            .hover(|s| s.text_color(c(theme::TEXT)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.next_environment(&NextEnvironment, window, cx)
                            }))
                            .child(format!("⚙ {}", self.environment_name())),
                    )
                    .when_some(self.notice.clone(), |el, (msg, is_error)| {
                        el.child(
                            div()
                                .text_color(c(if is_error {
                                    theme::ERROR
                                } else {
                                    theme::SUCCESS
                                }))
                                .child(msg),
                        )
                    }),
            )
            .child(format!(
                "{} Search  ·  {} Send  ·  {} Save",
                shortcut("P"),
                shortcut("Enter"),
                shortcut("S")
            ))
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let main: AnyElement = match self.active() {
            Some(tab) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.render_request_panel(tab, cx))
                .child(self.render_response_panel(tab, cx))
                .into_any_element(),
            None => placeholder(format!(
                "Open a request from the sidebar or press {}",
                shortcut("P")
            ))
            .into_any_element(),
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("Workspace")
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::send_request))
            .on_action(cx.listener(Self::save_request))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_environment))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .text_color(c(theme::TEXT))
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(self.render_tab_bar(cx))
                            .child(main),
                    ),
            )
            .child(self.render_status_bar(cx))
            .when_some(self.palette.as_ref(), |el, (palette, _)| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .justify_center()
                        .pt(px(72.))
                        .bg(gpui::hsla(0., 0., 0., 0.35))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.dismiss_palette(window, cx)),
                        )
                        .child(palette.clone()),
                )
            })
    }
}

// -- small stateless helpers -------------------------------------------------

fn shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

fn placeholder(text: String) -> Div {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(c(theme::TEXT_FAINT))
        .child(text)
}

fn kv_table(entries: &[KeyValue]) -> AnyElement {
    if entries.is_empty() {
        return div()
            .text_color(c(theme::TEXT_FAINT))
            .child("None")
            .into_any_element();
    }
    div()
        .flex()
        .flex_col()
        .border_1()
        .border_color(c(theme::BORDER))
        .rounded_md()
        .children(entries.iter().map(|kv| {
            div()
                .flex()
                .border_b_1()
                .border_color(c(theme::BORDER))
                .font_family(theme::MONO_FONT)
                .text_sm()
                .when(!kv.enabled, |el| {
                    el.text_color(c(theme::TEXT_FAINT)).line_through()
                })
                .child(
                    div()
                        .w(px(180.))
                        .flex_none()
                        .px_2()
                        .py_1()
                        .border_r_1()
                        .border_color(c(theme::BORDER))
                        .truncate()
                        .child(SharedString::from(kv.name.clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .truncate()
                        .child(SharedString::from(kv.value.clone())),
                )
        }))
        .into_any_element()
}

fn render_auth(auth: &Auth) -> AnyElement {
    let field = |k: &str, v: &str| KeyValue::new(k, v);
    let mask = |s: &str| "•".repeat(s.chars().count().min(16));
    let (mode, fields) = match auth {
        Auth::None => ("No Auth", vec![]),
        Auth::Inherit => ("Inherit from collection", vec![]),
        Auth::Bearer { token } => ("Bearer Token", vec![field("Token", token)]),
        Auth::Basic { username, password } => (
            "Basic Auth",
            vec![
                field("Username", username),
                field("Password", &mask(password)),
            ],
        ),
        Auth::ApiKey {
            key,
            value,
            placement,
        } => (
            "API Key",
            vec![
                field("Key", key),
                field("Value", value),
                field("Add to", placement.as_str()),
            ],
        ),
        Auth::Unsupported { mode } => (mode.as_str(), vec![]),
    };
    section(mode, kv_table(&fields))
}

fn render_body(req: &HttpRequest) -> AnyElement {
    let body = &req.body;
    let text_block = |text: &Option<String>| -> AnyElement {
        div()
            .p_2()
            .rounded_md()
            .bg(c(theme::SURFACE_ALT))
            .border_1()
            .border_color(c(theme::BORDER))
            .font_family(theme::MONO_FONT)
            .text_sm()
            .children(text.as_deref().unwrap_or_default().lines().map(|l| {
                div()
                    .whitespace_nowrap()
                    .child(SharedString::from(l.to_owned()))
            }))
            .into_any_element()
    };
    let content = match &body.mode {
        BodyMode::None | BodyMode::Other(_) => kv_table(&[]),
        BodyMode::Json => text_block(&body.json),
        BodyMode::Text => text_block(&body.text),
        BodyMode::Xml => text_block(&body.xml),
        BodyMode::Sparql => text_block(&body.sparql),
        BodyMode::Graphql => text_block(&body.graphql),
        BodyMode::FormUrlEncoded => kv_table(&body.form_urlencoded),
        BodyMode::MultipartForm => kv_table(&body.multipart_form),
    };
    section(body.mode.as_str(), content)
}

fn section(title: &str, content: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(c(theme::TEXT_FAINT))
                .child(SharedString::from(title.to_owned())),
        )
        .child(content)
        .into_any_element()
}

fn update_summary(folder: &mut Folder, path: &Path, method: HttpMethod) -> bool {
    folder.children.iter_mut().any(|node| match node {
        Node::Request(r) if r.path == path => {
            r.method = method;
            true
        }
        Node::Folder(f) => update_summary(f, path, method),
        Node::Request(_) => false,
    })
}
