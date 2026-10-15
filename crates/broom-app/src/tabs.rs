//! Editor tabs above the centre editor. One tab per open markdown file.
//!
//! A preview tab (italic title) is replaced by the next preview. It becomes
//! pinned on its first edit, or when it is opened pinned (double-click, Enter).
//! The active tab's editor is also the workspace's `center`.

use std::path::{Path, PathBuf};

use broom_config::VimBindings;
use broom_core::Document;
use gpui_kit::*;

use crate::editor::{Editor, EditorEvent, EditorStyle, ViewMode};
use crate::theme;
use crate::workspace::{Workspace, guarded};

actions!(broom_tabs, [CloseTab]);

/// What a new editor needs from settings.
#[derive(Clone)]
pub struct OpenConfig {
    pub style: EditorStyle,
    pub vim: VimBindings,
    pub vim_enabled: bool,
    pub large_bytes: u64,
}

struct OpenTab {
    path: PathBuf,
    title: SharedString,
    editor: Entity<Editor>,
    pinned: bool,
    _sub: Subscription,
}

pub struct Tabs {
    workspace: WeakEntity<Workspace>,
    config: OpenConfig,
    tabs: Vec<OpenTab>,
    active: Option<usize>,
}

impl Tabs {
    pub fn new(workspace: WeakEntity<Workspace>, config: OpenConfig) -> Tabs {
        Tabs {
            workspace,
            config,
            tabs: Vec::new(),
            active: None,
        }
    }

    /// Opens `path`. Markdown goes to an editor tab. Anything else goes to
    /// the default app. A preview open replaces the current preview tab.
    /// Opening an already-open file activates it, and pins it if `pinned`.
    pub fn open(&mut self, path: &Path, pinned: bool, cx: &mut Context<Self>) {
        if !broom_fs::is_markdown(path) {
            if let Some(Err(e)) = guarded("open", || broom_fs::open_external(path)) {
                eprintln!("broom: cannot open {}: {e}", path.display());
            }
            return;
        }
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            if pinned {
                self.tabs[idx].pinned = true;
            }
            self.activate(idx, cx);
            return;
        }
        let Some(editor) = self.build_editor(path, cx) else {
            return;
        };
        let sub = self.subscribe(&editor, cx);
        let title: SharedString = file_name(path).into();
        let tab = OpenTab {
            path: path.to_path_buf(),
            title,
            editor,
            pinned,
            _sub: sub,
        };
        // A preview replaces the existing preview tab in place.
        let idx = match (pinned, self.tabs.iter().position(|t| !t.pinned)) {
            (false, Some(idx)) => {
                self.tabs[idx] = tab;
                idx
            }
            _ => {
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        self.activate(idx, cx);
    }

    /// Reads the file and builds its editor. Large files open in source mode.
    fn build_editor(&self, path: &Path, cx: &mut Context<Self>) -> Option<Entity<Editor>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("broom: cannot read {}: {e}", path.display());
                return None;
            }
        };
        let large = guarded("file size", || {
            broom_fs::is_large(path, self.config.large_bytes)
        })
        .and_then(Result::ok)
        .unwrap_or(false);
        let cfg = self.config.clone();
        let editor = guarded("editor", || {
            let mut ed = Editor::new(Document::parse(&text), cfg.style.clone(), large, &cfg.vim);
            ed.vim_enabled = cfg.vim_enabled;
            ed
        })?;
        Some(cx.new(|_| editor))
    }

    /// Any edit pins the tab and refreshes the status bar.
    fn subscribe(&self, editor: &Entity<Editor>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(editor, |this, ed, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::Edited) {
                let id = ed.entity_id();
                if let Some(tab) = this.tabs.iter_mut().find(|t| t.editor.entity_id() == id) {
                    tab.pinned = true;
                }
                cx.notify();
                this.update_workspace(cx, |ws, cx| ws.refresh_status(cx));
            }
        })
    }

    /// Makes tab `idx` active and shows its editor in the centre pane.
    pub fn activate(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active = Some(idx);
        self.sync_center(cx);
    }

    /// Pins tab `idx`. Used by double-click.
    pub fn pin_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.pinned = true;
            cx.notify();
        }
    }

    /// Closes the active tab. Bound to `CloseTab` (Cmd+W) by the keymap.
    pub fn close_active(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.active {
            self.close_at(idx, cx);
        }
    }

    /// Closes tab `idx`. The neighbour on the right becomes active, else the left.
    pub fn close_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a == idx => Some(idx.min(self.tabs.len() - 1)),
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        self.sync_center(cx);
    }

    /// Drops tabs for a deleted file or folder.
    pub fn path_deleted(&mut self, path: &Path, cx: &mut Context<Self>) {
        let before = self.tabs.len();
        self.tabs.retain(|t| !t.path.starts_with(path));
        if self.tabs.len() != before {
            self.active = self.active.filter(|&a| a < self.tabs.len());
            if self.active.is_none() && !self.tabs.is_empty() {
                self.active = Some(0);
            }
            self.sync_center(cx);
        }
    }

    /// Follows a renamed file or folder.
    pub fn path_renamed(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        for tab in self.tabs.iter_mut() {
            if let Ok(rest) = tab.path.strip_prefix(from) {
                tab.path = to.join(rest);
                tab.title = file_name(&tab.path).into();
            }
        }
        cx.notify();
    }

    /// Path of the active tab, if any.
    pub fn active_path(&self) -> Option<&Path> {
        self.active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.path.as_path())
    }

    /// Switches the large-file editor to formatted mode.
    fn open_formatted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.editor.clone())
        else {
            return;
        };
        editor.update(cx, |ed, cx| ed.set_mode(ViewMode::Formatted, window, cx));
        cx.notify();
    }

    /// Puts the active editor into the workspace's centre slot.
    fn sync_center(&self, cx: &mut Context<Self>) {
        let editor = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.editor.clone());
        self.update_workspace(cx, |ws, cx| {
            ws.center = editor;
            ws.refresh_status(cx);
        });
        cx.notify();
    }

    /// Runs `f` on the workspace if it is still alive.
    fn update_workspace(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
    ) {
        let _ = self.workspace.update(cx, f);
    }
}

impl Render for Tabs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div()
            .id("tab-strip")
            .flex()
            .flex_row()
            .w_full()
            .overflow_x_scroll()
            .bg(theme::color(theme::PANEL))
            .text_color(theme::color(theme::INK));

        if self.tabs.is_empty() {
            strip = strip.child(
                div()
                    .px_3()
                    .py_1()
                    .text_color(theme::color(theme::MUTED))
                    .child("No open tabs"),
            );
        }

        for (i, tab) in self.tabs.iter().enumerate() {
            let active = self.active == Some(i);
            let dirty = tab.editor.read(cx).is_dirty();
            let label = if dirty {
                format!("● {}", tab.title)
            } else {
                tab.title.to_string()
            };
            // Preview titles are italic. Pinned titles are upright.
            let title = if tab.pinned {
                div().child(label)
            } else {
                div().italic().child(label)
            };
            let mut el = div()
                .id(("tab", i))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .border_r_1()
                .border_color(theme::color(theme::RULE))
                .cursor_pointer()
                .child(title)
                .child(
                    div()
                        .id(("close", i))
                        .px_1()
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_at(i, cx);
                        })),
                )
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    this.activate(i, cx);
                    if ev.click_count() >= 2 {
                        this.pin_at(i, cx);
                    }
                }));
            if active {
                el = el.bg(theme::color(theme::BG)).font_weight(FontWeight::BOLD);
            }
            strip = strip.child(el);
        }

        let mut root = div()
            .flex()
            .flex_col()
            .w_full()
            .key_context("Tabs")
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_active(cx)))
            .child(strip);

        // Large-file warning bar. Text only, the action is a text link.
        let large = self
            .active
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|t| t.editor.read(cx).large_notice);
        if large {
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .px_3()
                    .py_1()
                    .bg(theme::color(theme::WARN_BG))
                    .text_color(theme::color(theme::INK))
                    .child("Large file — opened as plain text. ")
                    .child(
                        div()
                            .id("open-formatted")
                            .underline()
                            .cursor_pointer()
                            .child("[Open formatted anyway]")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_formatted(window, cx)),
                            ),
                    ),
            );
        }
        root
    }
}

/// Last path component, for tab and tree labels.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
