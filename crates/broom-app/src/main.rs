//! The `broom` binary.
//!
//! Scaffold: opens one window titled "broom" with three placeholder panes
//! (tree | editor | right panel) and a status bar. No crate logic is wired
//! yet, so nothing here calls a `todo!()` stub at runtime.
//!
//! Usage: `broom [folder]`. Defaults to the current folder (or `$HOME` when
//! the current folder is `/`, as it is when launched from Finder).

use std::path::PathBuf;

use broom_config::Settings;
use broom_vim::Mode;
use gpui_kit::*;

// Solarized Light base tones.
const BASE3: u32 = 0xfdf6e3; // background
const BASE2: u32 = 0xeee8d5; // pane / status bar
const BASE01: u32 = 0x586e75; // muted text
const BASE00: u32 = 0x657b83; // body text

/// Root view: three panes and a status bar.
struct BroomWindow {
    folder: PathBuf,
}

impl BroomWindow {
    fn pane(title: &str, width: Pixels) -> Div {
        div()
            .flex()
            .flex_col()
            .w(width)
            .h_full()
            .p_2()
            .gap_2()
            .bg(rgb(BASE2))
            .text_color(rgb(BASE01))
            .child(title.to_string())
    }
}

impl Render for BroomWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let font_size = Settings::default().font_size;
        let mode = Mode::Normal.label();
        let folder = self.folder.display().to_string();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BASE3))
            .text_color(rgb(BASE00))
            .text_size(px(font_size))
            .child(
                // Main row: tree | editor | right panel.
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(BroomWindow::pane("Files", px(240.)).child(folder))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .p_6()
                            .child("Editor (placeholder)"),
                    )
                    .child(BroomWindow::pane("Outline / Backlinks", px(260.))),
            )
            .child(
                // Status bar: mode word | file | word count | save state. Text only.
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .bg(rgb(BASE2))
                    .text_color(rgb(BASE01))
                    .child(mode)
                    .child("Untitled")
                    .child("0 words")
                    .child("Saved"),
            )
    }
}

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

            // Fits a 1280x800 logical screen with the title bar and menu bar.
            let bounds = Bounds::centered(None, size(px(1120.), px(720.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("broom".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            gpui_kit::open_window(options, cx, move |_, cx| cx.new(|_| BroomWindow { folder }))
                .expect("failed to open the broom window");

            cx.activate(true);
        });
}
