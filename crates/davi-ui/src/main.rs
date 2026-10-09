#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

use anyhow::Context as _;
use davi_net::{EngineConfig, HttpEngine};
use davi_ui::workspace::{
    CloseTab, NewCollection, NewFolder, NewRequest, NextEnvironment, OpenCollection, Quit,
    SaveRequest, SendRequest, ToggleCommandPalette, Workspace,
};
use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, prelude::*,
    px, size,
};
use gpui_component::{Root, Theme, ThemeMode};

/// `davi [COLLECTION_DIR]`, falling back to `$DAVI_COLLECTION`.
fn collection_dir() -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("DAVI_COLLECTION").map(PathBuf::from))
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Started before the window so a failure is reported instead of panicking
    // inside GPUI's run loop. Its worker threads idle at ~0% CPU.
    let engine = HttpEngine::new(EngineConfig::default()).context("starting HTTP engine")?;
    let collection = collection_dir();

    Application::new().run(move |cx: &mut App| {
        gpui_component::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        // Registered after the component bindings so ours take precedence.
        davi_ui::palette::init(cx);
        cx.bind_keys([
            KeyBinding::new("secondary-p", ToggleCommandPalette, None),
            KeyBinding::new("secondary-enter", SendRequest, None),
            KeyBinding::new("secondary-s", SaveRequest, None),
            KeyBinding::new("secondary-w", CloseTab, None),
            KeyBinding::new("secondary-e", NextEnvironment, None),
            KeyBinding::new("secondary-n", NewRequest, None),
            KeyBinding::new("secondary-shift-n", NewFolder, None),
            KeyBinding::new("secondary-o", OpenCollection, None),
            KeyBinding::new("secondary-shift-o", NewCollection, None),
            KeyBinding::new("secondary-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let window = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Davi".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(720.), px(420.))),
                app_id: Some("dev.davi.Davi".into()),
                ..Default::default()
            },
            |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(engine, collection, window, cx));
                cx.new(|cx| Root::new(workspace, window, cx))
            },
        );
        if let Err(e) = window {
            log::error!("failed to open window: {e:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
    Ok(())
}
