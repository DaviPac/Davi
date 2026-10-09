//! Root view: sidebar | (tab bar / active request editor), a status bar,
//! the welcome screen when no collection is open, the command palette and
//! dialog layers.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use davi_core::collection::{self, Collection, Node};
use davi_core::env::VarScope;
use davi_core::model::HttpMethod;
use davi_net::HttpEngine;
use gpui::{
    AnyElement, App, Context, Div, Entity, FocusHandle, Focusable, MouseButton, PathPromptOptions,
    SharedString, Stateful, Subscription, Window, actions, div, img, prelude::*, px, uniform_list,
};
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{Input, InputState};
use gpui_component::{Root, WindowExt};

use crate::palette::{CommandPalette, PaletteEvent, PaletteItem};
use crate::request_editor::{EditorEvent, RequestEditor, shortcut};
use crate::settings::Settings;
use crate::theme::{self, c};

actions!(
    davi,
    [
        ToggleCommandPalette,
        SendRequest,
        SaveRequest,
        CloseTab,
        NextEnvironment,
        NewRequest,
        NewFolder,
        OpenCollection,
        NewCollection,
        Quit
    ]
);

const SIDEBAR_WIDTH: f32 = 270.;

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

type NameCallback = Rc<dyn Fn(&mut Workspace, String, &mut Window, &mut Context<Workspace>)>;

pub struct Workspace {
    focus_handle: FocusHandle,
    engine: HttpEngine,
    settings: Settings,
    collection: Option<Collection>,
    collapsed: HashSet<PathBuf>,
    /// Folder new requests/folders are created in (`None` = collection root).
    selected_folder: Option<PathBuf>,
    sidebar_rows: Vec<SidebarRow>,
    tabs: Vec<Entity<RequestEditor>>,
    active_tab: Option<usize>,
    environment: Option<usize>,
    palette: Option<(Entity<CommandPalette>, Subscription)>,
    notice: Option<(SharedString, bool)>,
    _tab_subscriptions: Vec<(gpui::EntityId, Subscription)>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
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
        let settings = Settings::load();
        let initial = collection_dir.or_else(|| settings.last_collection());

        let mut this = Self {
            focus_handle,
            engine,
            settings,
            collection: None,
            collapsed: HashSet::new(),
            selected_folder: None,
            sidebar_rows: Vec::new(),
            tabs: Vec::new(),
            active_tab: None,
            environment: None,
            palette: None,
            notice: None,
            _tab_subscriptions: Vec::new(),
        };
        if let Some(dir) = initial {
            this.open_collection(dir, window, cx);
        }
        this
    }

    // -- collection ------------------------------------------------------------

    fn open_collection(&mut self, dir: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        let result = collection::ensure_collection(&dir).and_then(|()| Collection::open(&dir));
        match result {
            Ok(c) => {
                for tab in &self.tabs {
                    tab.update(cx, |t, _| t.cancel());
                }
                self.tabs.clear();
                self._tab_subscriptions.clear();
                self.active_tab = None;
                self.collapsed.clear();
                self.selected_folder = None;
                self.environment = (!c.environments.is_empty()).then_some(0);
                self.notice = (!c.issues.is_empty()).then(|| {
                    for issue in &c.issues {
                        log::warn!("{}", issue.error);
                    }
                    (
                        format!("{} file(s) failed to load", c.issues.len()).into(),
                        true,
                    )
                });
                self.collection = Some(c);
                self.settings.remember_collection(dir);
                self.rebuild_sidebar();
            }
            Err(e) => self.notice = Some((e.to_string().into(), true)),
        }
        cx.notify();
    }

    /// Re-read the tree from disk, keeping tabs and the selected environment.
    fn reload_collection(&mut self, cx: &mut Context<Self>) {
        let Some(current) = &self.collection else {
            return;
        };
        let env_name = self
            .environment
            .and_then(|ix| current.environments.get(ix))
            .map(|e| e.name.clone());
        match Collection::open(&current.root.path) {
            Ok(c) => {
                self.environment = env_name
                    .and_then(|name| c.environments.iter().position(|e| e.name == name))
                    .or((!c.environments.is_empty()).then_some(0));
                self.collection = Some(c);
                self.rebuild_sidebar();
            }
            Err(e) => self.notice = Some((e.to_string().into(), true)),
        }
        cx.notify();
    }

    fn pick_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Collection".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_collection(dir, window, cx))
                .ok();
        })
        .detach();
    }

    fn new_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_name(
            "New Collection",
            "My Collection",
            "Choose Location…",
            Rc::new(|_, name, window, cx| {
                let paths = cx.prompt_for_paths(PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some("Create Collection Here".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(paths))) = paths.await else {
                        return;
                    };
                    let Some(parent) = paths.into_iter().next() else {
                        return;
                    };
                    this.update_in(cx, |this, window, cx| {
                        match collection::create_collection(&parent, &name) {
                            Ok(dir) => this.open_collection(dir, window, cx),
                            Err(e) => {
                                this.notice = Some((e.to_string().into(), true));
                                cx.notify();
                            }
                        }
                    })
                    .ok();
                })
                .detach();
            }),
            window,
            cx,
        );
    }

    fn target_dir(&self) -> Option<PathBuf> {
        let root = &self.collection.as_ref()?.root.path;
        Some(
            self.selected_folder
                .clone()
                .filter(|p| p.is_dir())
                .unwrap_or_else(|| root.clone()),
        )
    }

    fn new_request_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_name(
            "New Request",
            "New Request",
            "Create",
            Rc::new(
                move |this, name, window, cx| match collection::create_request(&dir, &name) {
                    Ok(path) => {
                        this.reload_collection(cx);
                        this.open_request(path, window, cx);
                        // Deferred: the closing dialog restores the previous focus.
                        if let Some(tab) = this.active().cloned() {
                            window.defer(cx, move |window, cx| {
                                tab.update(cx, |tab, cx| tab.focus_url(window, cx));
                            });
                        }
                    }
                    Err(e) => {
                        this.notice = Some((e.to_string().into(), true));
                        cx.notify();
                    }
                },
            ),
            window,
            cx,
        );
    }

    fn new_folder_in(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_name(
            "New Folder",
            "New Folder",
            "Create",
            Rc::new(
                move |this, name, _, cx| match collection::create_folder(&dir, &name) {
                    Ok(path) => {
                        this.collapsed.remove(&path);
                        this.selected_folder = Some(path);
                        this.reload_collection(cx);
                    }
                    Err(e) => {
                        this.notice = Some((e.to_string().into(), true));
                        cx.notify();
                    }
                },
            ),
            window,
            cx,
        );
    }

    /// A small dialog asking for a name. Enter or the OK button confirms.
    fn prompt_name(
        &mut self,
        title: &'static str,
        default: &'static str,
        ok_label: &'static str,
        on_confirm: NameCallback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(default));

        // Enter in the input propagates to the dialog's Confirm, so `on_ok`
        // covers both the keyboard and the button.
        let weak = cx.entity().downgrade();
        let focus_target = input.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let (input, weak, on_confirm) = (input.clone(), weak.clone(), on_confirm.clone());
            dialog
                .title(title)
                .w(px(420.))
                .child(Input::new(&input))
                .confirm()
                .button_props(DialogButtonProps::default().ok_text(ok_label))
                .on_ok(move |_, window, cx| {
                    let name = input.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    weak.update(cx, |this, cx| on_confirm(this, name, window, cx))
                        .ok();
                    true
                })
        });
        // The dialog takes focus when it opens; hand it to the input after.
        window.defer(cx, move |window, cx| {
            focus_target.update(cx, |input, cx| input.focus(window, cx));
        });
    }

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

    fn click_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_path_buf());
        }
        self.selected_folder = Some(path.to_path_buf());
        self.rebuild_sidebar();
        cx.notify();
    }

    // -- tabs ------------------------------------------------------------------

    fn active(&self) -> Option<&Entity<RequestEditor>> {
        self.active_tab.and_then(|ix| self.tabs.get(ix))
    }

    fn open_request(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // New items go next to the request the user is looking at.
        self.selected_folder = path.parent().map(Path::to_path_buf);
        if let Some(ix) = self.tabs.iter().position(|t| t.read(cx).path() == &path) {
            self.active_tab = Some(ix);
            cx.notify();
            return;
        }
        match collection::load_request(&path) {
            Ok(request) => {
                let editor = cx.new(|cx| RequestEditor::new(path, request, window, cx));
                let subscription = cx.subscribe_in(
                    &editor,
                    window,
                    |this, editor, event, window, cx| match event {
                        EditorEvent::Changed => cx.notify(),
                        EditorEvent::SendRequested => this.send(editor.clone(), window, cx),
                    },
                );
                self._tab_subscriptions
                    .push((editor.entity_id(), subscription));
                self.tabs.push(editor);
                self.active_tab = Some(self.tabs.len() - 1);
            }
            Err(e) => self.notice = Some((e.to_string().into(), true)),
        }
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix).cloned() else {
            return;
        };
        if tab.read(cx).is_dirty() {
            let weak = cx.entity().downgrade();
            let name = tab.read(cx).title();
            window.open_dialog(cx, move |dialog, _, _| {
                let (weak, tab) = (weak.clone(), tab.clone());
                dialog
                    .title(format!("Discard changes to “{name}”?"))
                    .w(px(420.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(c(theme::TEXT_MUTED))
                            .child("Your unsaved edits to this request will be lost."),
                    )
                    .confirm()
                    .button_props(DialogButtonProps::default().ok_text("Discard"))
                    .on_ok(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.force_close(&tab, cx)).ok();
                        true
                    })
            });
        } else {
            self.force_close(&tab, cx);
        }
    }

    fn force_close(&mut self, tab: &Entity<RequestEditor>, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|t| t == tab) else {
            return;
        };
        tab.update(cx, |t, _| t.cancel());
        let id = tab.entity_id();
        self._tab_subscriptions.retain(|(eid, _)| *eid != id);
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

    fn send(&mut self, editor: Entity<RequestEditor>, window: &mut Window, cx: &mut Context<Self>) {
        let scope = self.var_scope();
        let engine = self.engine.clone();
        editor.update(cx, |editor, cx| editor.send(&engine, scope, window, cx));
    }

    // -- actions ---------------------------------------------------------------

    fn on_send(&mut self, _: &SendRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active().cloned() {
            self.send(tab, window, cx);
        }
    }

    fn on_save(&mut self, _: &SaveRequest, _: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active().cloned() else {
            return;
        };
        match tab.update(cx, |t, cx| t.save(cx)) {
            Ok(()) => {
                self.notice = Some((format!("Saved {}", tab.read(cx).title()).into(), false));
                self.reload_collection(cx);
            }
            Err(e) => self.notice = Some((e.to_string().into(), true)),
        }
        cx.notify();
    }

    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab {
            self.close_tab_at(ix, window, cx);
        }
    }

    fn on_next_environment(&mut self, _: &NextEnvironment, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.collection.as_ref().map_or(0, |c| c.environments.len());
        // Cycles through every environment and then "No Environment".
        self.environment = match self.environment {
            None if count > 0 => Some(0),
            Some(ix) if ix + 1 < count => Some(ix + 1),
            _ => None,
        };
        cx.notify();
    }

    fn on_new_request(&mut self, _: &NewRequest, window: &mut Window, cx: &mut Context<Self>) {
        match self.target_dir() {
            Some(dir) => self.new_request_in(dir, window, cx),
            None => self.pick_collection(window, cx),
        }
    }

    fn on_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dir) = self.target_dir() {
            self.new_folder_in(dir, window, cx);
        }
    }

    fn on_open_collection(
        &mut self,
        _: &OpenCollection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pick_collection(window, cx);
    }

    fn on_new_collection(
        &mut self,
        _: &NewCollection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_collection(window, cx);
    }

    fn on_toggle_palette(
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
            // Nothing to search yet: the most useful thing is to open one.
            self.pick_collection(window, cx);
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

        let palette = cx.new(|cx| CommandPalette::new(items, window, cx));
        let subscription = cx.subscribe_in(&palette, window, |this, _, event, window, cx| {
            if let PaletteEvent::Confirmed(path) = event {
                this.open_request(path.clone(), window, cx);
            }
            this.dismiss_palette(window, cx);
        });
        self.palette = Some((palette, subscription));
        cx.notify();
    }

    fn dismiss_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    // -- rendering -------------------------------------------------------------

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let recent: Vec<PathBuf> = self
            .settings
            .recent_collections
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(460.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(img(app_icon()).size(px(56.)))
                            .child(div().text_2xl().child("Davi")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(c(theme::TEXT_MUTED))
                            .child("A fast, Git-friendly API client. Collections are folders of plain-text .bru files, compatible with Bruno."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button("welcome-new", "New Collection", true).on_click(
                                cx.listener(|this, _, window, cx| this.new_collection(window, cx)),
                            ))
                            .child(button("welcome-open", "Open Collection…", false).on_click(
                                cx.listener(|this, _, window, cx| this.pick_collection(window, cx)),
                            )),
                    )
                    .when(!recent.is_empty(), |el| {
                        el.child(
                            div()
                                .mt_4()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().text_xs().text_color(c(theme::TEXT_FAINT)).child("RECENT"))
                                .children(recent.into_iter().enumerate().map(|(ix, path)| {
                                    let label: SharedString = path.display().to_string().into();
                                    div()
                                        .id(("recent", ix))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .text_sm()
                                        .text_color(c(theme::ACCENT))
                                        .hover(|s| s.bg(c(theme::ELEVATED)))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.open_collection(path.clone(), window, cx)
                                        }))
                                        .child(label)
                                })),
                        )
                    })
                    .when_some(self.notice.clone(), |el, (msg, _)| {
                        el.child(div().text_sm().text_color(c(theme::ERROR)).child(msg))
                    })
                    .child(
                        div()
                            .mt_4()
                            .text_xs()
                            .text_color(c(theme::TEXT_FAINT))
                            .child(format!(
                                "{} open collection  ·  {} new request  ·  {} search",
                                shortcut("O"),
                                shortcut("N"),
                                shortcut("P")
                            )),
                    ),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = self
            .collection
            .as_ref()
            .map_or_else(|| "No collection".into(), |c| c.name.clone().into());

        let header_button = |id: &'static str, label: &'static str, tooltip: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .rounded_sm()
                .cursor_pointer()
                .text_xs()
                .text_color(c(theme::TEXT_MUTED))
                .hover(|s| s.bg(c(theme::ELEVATED)).text_color(c(theme::TEXT)))
                .child(label)
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx)
                })
        };

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
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .pl_3()
                    .pr_2()
                    .h(px(36.))
                    .border_b_1()
                    .border_color(c(theme::BORDER))
                    .child(
                        div()
                            .id("collection-title")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(c(theme::TEXT_MUTED))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_folder = None;
                                cx.notify();
                            }))
                            .child(title.to_uppercase()),
                    )
                    .child(
                        header_button("new-request", "+ Request", "New request (Ctrl+N)").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.on_new_request(&NewRequest, window, cx)
                            }),
                        ),
                    )
                    .child(
                        header_button("new-folder", "+ Folder", "New folder").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.on_new_folder(&NewFolder, window, cx)
                            }),
                        ),
                    )
                    .child(
                        header_button("switch-collection", "⇄", "Open another collection (Ctrl+O)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_collection(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar_rows.is_empty(), |el| {
                        el.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(c(theme::TEXT_FAINT))
                                .child("No requests yet. Click “+ Request” to create one."),
                        )
                    })
                    .child(
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
        let active_path = self.active().map(|t| t.read(cx).path().clone());
        let row = div()
            .id(ix)
            .group("sidebar-row")
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
                let is_selected = self.selected_folder.as_deref() == Some(path.as_path());
                let (toggle_path, add_path) = (path.clone(), path.clone());
                row.pl(px(12. + *depth as f32 * 14.))
                    .when(is_selected, |el| el.bg(c(theme::ELEVATED)))
                    .text_color(c(theme::TEXT))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.click_folder(&toggle_path, cx)),
                    )
                    .child(
                        div()
                            .w(px(12.))
                            .text_color(c(theme::TEXT_FAINT))
                            .child(if *collapsed { "▸" } else { "▾" }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .child(
                        div()
                            .id(("folder-add", ix))
                            .invisible()
                            .group_hover("sidebar-row", |s| s.visible())
                            .px_1()
                            .rounded_sm()
                            .text_color(c(theme::TEXT_MUTED))
                            .hover(|s| s.bg(c(theme::HOVER)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.new_request_in(add_path.clone(), window, cx);
                            }))
                            .child("+"),
                    )
            }
            SidebarRow::Request {
                path,
                name,
                method,
                depth,
            } => {
                let is_active = active_path.as_ref() == Some(path);
                let path = path.clone();
                row.pl(px(12. + *depth as f32 * 14.))
                    .when(is_active, |el| el.bg(c(theme::ELEVATED)))
                    .text_color(c(theme::TEXT))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_request(path.clone(), window, cx)
                    }))
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
                let tab = tab.read(cx);
                let active = self.active_tab == Some(ix);
                let dirty = tab.is_dirty();
                div()
                    .id(ix)
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
                            .text_color(theme::method_color(tab.method()))
                            .child(theme::method_label(tab.method())),
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab_at(ix, window, cx);
                            }))
                            .child(if dirty { "●" } else { "×" }),
                    )
            }))
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
                                this.on_next_environment(&NextEnvironment, window, cx)
                            }))
                            .child(format!("Env: {}", self.environment_name())),
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
                "{} Search  ·  {} New  ·  {} Send  ·  {} Save",
                shortcut("P"),
                shortcut("N"),
                shortcut("Enter"),
                shortcut("S")
            ))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Global shortcuts dispatch along the focus path, so never leave the
        // window without focus (e.g. after a dialog closes).
        if window.focused(cx).is_none() {
            let handle = self.focus_handle.clone();
            window.defer(cx, move |window, _| window.focus(&handle));
        }

        let body: AnyElement = if self.collection.is_none() {
            self.render_welcome(cx).into_any_element()
        } else {
            let main: AnyElement = match self.active() {
                Some(tab) => div()
                    .flex_1()
                    .min_h_0()
                    .child(tab.clone())
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(c(theme::TEXT_FAINT))
                    .child(format!(
                        "Open a request from the sidebar, press {} to search, or",
                        shortcut("P")
                    ))
                    .child(
                        button("empty-new-request", "New Request", true).on_click(cx.listener(
                            |this, _, window, cx| this.on_new_request(&NewRequest, window, cx),
                        )),
                    )
                    .into_any_element(),
            };
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
                )
                .into_any_element()
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("Workspace")
            .on_action(cx.listener(Self::on_toggle_palette))
            .on_action(cx.listener(Self::on_send))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_next_environment))
            .on_action(cx.listener(Self::on_new_request))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_open_collection))
            .on_action(cx.listener(Self::on_new_collection))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .text_color(c(theme::TEXT))
            .text_sm()
            .child(div().flex_1().min_h_0().flex().child(body))
            .child(self.render_status_bar(cx))
            .when_some(self.palette.as_ref(), |el, (palette, _)| {
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(72.))
                        .bg(gpui::hsla(0., 0., 0., 0.35))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.dismiss_palette(window, cx)),
                        )
                        .child(palette.clone()),
                )
            })
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

/// The app icon, decoded once and shared.
fn app_icon() -> Arc<gpui::Image> {
    static ICON: std::sync::OnceLock<Arc<gpui::Image>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            include_bytes!("../../../assets/icon.png").to_vec(),
        ))
    })
    .clone()
}

fn button(id: &'static str, label: &'static str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .when(primary, |el| {
            el.bg(c(theme::ACCENT))
                .text_color(c(theme::SURFACE_ALT))
                .hover(|s| s.opacity(0.85))
        })
        .when(!primary, |el| {
            el.border_1()
                .border_color(c(theme::HOVER))
                .text_color(c(theme::TEXT))
                .hover(|s| s.bg(c(theme::ELEVATED)))
        })
        .child(label)
}
