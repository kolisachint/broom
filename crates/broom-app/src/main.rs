//! The `broom` binary.
//!
//! Opens one window: left panel slot | editor | right panel slot, and a
//! status bar. See [`workspace`] for the panel hooks.
//!
//! Usage: `broom [folder]`. Defaults to the current folder (or `$HOME` when
//! the current folder is `/`, as it is when launched from Finder).

// scaffold: remove once the panels and key routing use the editor API.
#![allow(dead_code)]

mod cheatsheet;
mod dialogs;
#[cfg(test)]
mod e2e;
mod editor;
mod keys;
mod palette;
mod right_panel;
mod safety;
mod search;
mod status_bar;
mod tabs;
mod theme;
mod tree_panel;
mod workspace;
mod zen;

use std::path::PathBuf;

use gpui_kit::*;

use workspace::Workspace;

/// Folder from the first CLI argument, else the current directory (or `$HOME`).
fn workspace_root() -> PathBuf {
    let raw = std::env::args().nth(1).map(PathBuf::from);
    let path = raw
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .filter(|p| p != std::path::Path::new("/"))
        })
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn main() {
    let folder = workspace_root();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            // Must run before any GPUI Component feature is used.
            gpui_kit::init(cx);
            keys::bind_keys(cx);

            // Fits a 1280x800 logical screen with the title bar and menu bar.
            let bounds = Bounds::centered(None, size(px(1120.), px(720.)), cx);
            // Transparent titlebar: the window draws its own cream title row.
            // The traffic lights sit inside that row, clear of its content.
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("broom".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(10.))),
                }),
                ..Default::default()
            };

            let (_, ws) = gpui_kit::open_window(options, cx, move |window, cx| {
                let ws = cx.new(|cx| Workspace::new(folder, cx));
                keys::focus_editor(&ws, window, cx);
                ws
            })
            .expect("failed to open the broom window");

            // The folder's start note, if it has one. Otherwise the empty state shows.
            ws.update(cx, |ws, cx| ws.open_startup(cx));

            // Quit keeps every edit: dirty named tabs are saved, all dirty tabs drafted.
            let quit_ws = ws.clone();
            cx.on_app_quit(move |cx| {
                quit_ws.update(cx, |ws, cx| ws.flush_on_quit(cx));
                async {}
            })
            .detach();

            cx.activate(true);
        });
}
