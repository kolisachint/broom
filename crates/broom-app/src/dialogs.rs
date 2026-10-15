//! Modal dialogs for file safety: external change, crash recovery, and
//! closing a tab with unsaved changes.
//!
//! A dialog only reports the user's choice as a [`DialogEvent`]. The
//! workspace does the file work.
//!
//! Keyboard: the dialog takes focus when it opens. Tab and Shift+Tab move
//! between buttons, Enter activates the highlighted one, and Escape picks
//! the safest choice (it never throws away text).

use std::path::PathBuf;

use broom_fs::{ConflictKind, RecoveryEntry};
use gpui_kit::*;

use crate::theme;

/// What the dialog is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// The open file changed (or vanished) on disk.
    Conflict { path: PathBuf, kind: ConflictKind },
    /// A draft from an earlier session can be restored.
    Recovery { entry: RecoveryEntry },
    /// A tab with unsaved changes is being closed. `key` is its draft key.
    Unsaved { key: String, name: String },
}

/// The user's choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogEvent {
    /// Load the file from disk, dropping buffer changes.
    Reload,
    /// Keep the buffer and make it win on the next save.
    KeepMine,
    /// Write the buffer to disk now, over the external change.
    Overwrite,
    /// Restore a recovery draft by key.
    Restore(String),
    /// Delete a recovery draft by key.
    Discard(String),
    /// Leave a recovery draft on disk and stop offering it now.
    Later,
    /// Save a tab (by draft key), then close it.
    SaveTab(String),
    /// Close a tab (by draft key) and drop its draft.
    DiscardTab(String),
    /// Do nothing.
    Cancel,
}

impl EventEmitter<DialogEvent> for Dialog {}

pub struct Dialog {
    pub kind: DialogKind,
    /// Index of the highlighted button. Enter activates it.
    selected: usize,
    focus: FocusHandle,
    /// Set until the first render moves keyboard focus here.
    wants_focus: bool,
}

impl Dialog {
    pub fn new(kind: DialogKind, cx: &mut Context<Dialog>) -> Dialog {
        Dialog {
            kind,
            selected: 0,
            focus: cx.focus_handle(),
            wants_focus: true,
        }
    }

    /// Title line, in words.
    fn title(&self) -> String {
        match &self.kind {
            DialogKind::Conflict { kind, .. } => match kind {
                ConflictKind::Modified => "File changed on disk".into(),
                ConflictKind::Deleted => "File was deleted on disk".into(),
            },
            DialogKind::Recovery { .. } => "Recover unsaved changes?".into(),
            DialogKind::Unsaved { name, .. } => format!("Unsaved changes in {name}"),
        }
    }

    /// Body text under the title.
    fn body(&self) -> String {
        match &self.kind {
            DialogKind::Conflict { path, .. } => format!(
                "{} was changed outside broom. Your edits are still in this window.",
                path.display()
            ),
            DialogKind::Recovery { entry } => {
                let from = entry
                    .original_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "an untitled note".into());
                format!("A draft of {from} was left by an earlier session.")
            }
            DialogKind::Unsaved { .. } => "This tab has edits that are not saved yet.".into(),
        }
    }

    /// The buttons, as (label, event). The first is the default for Enter.
    fn actions(&self) -> Vec<(&'static str, DialogEvent)> {
        match &self.kind {
            DialogKind::Conflict { .. } => vec![
                ("Reload from disk", DialogEvent::Reload),
                ("Keep mine", DialogEvent::KeepMine),
                ("Overwrite disk", DialogEvent::Overwrite),
            ],
            DialogKind::Recovery { entry } => vec![
                ("Restore", DialogEvent::Restore(entry.key.clone())),
                ("Discard", DialogEvent::Discard(entry.key.clone())),
                ("Later", DialogEvent::Later),
            ],
            DialogKind::Unsaved { key, .. } => vec![
                ("Save", DialogEvent::SaveTab(key.clone())),
                ("Discard", DialogEvent::DiscardTab(key.clone())),
                ("Cancel", DialogEvent::Cancel),
            ],
        }
    }

    /// The choice Escape makes. Never throws text away.
    fn safest(&self) -> DialogEvent {
        match &self.kind {
            DialogKind::Conflict { .. } => DialogEvent::KeepMine,
            DialogKind::Recovery { .. } => DialogEvent::Later,
            DialogKind::Unsaved { .. } => DialogEvent::Cancel,
        }
    }

    /// Tab (or Shift+Tab) moves the highlight. Wraps around.
    fn step(&mut self, back: bool, cx: &mut Context<Self>) {
        let n = self.actions().len();
        if n == 0 {
            return;
        }
        self.selected = if back {
            (self.selected + n - 1) % n
        } else {
            (self.selected + 1) % n
        };
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let shift = ev.keystroke.modifiers.shift;
        let event = match key {
            "tab" => {
                self.step(shift, cx);
                cx.stop_propagation();
                return;
            }
            "enter" => self
                .actions()
                .into_iter()
                .nth(self.selected)
                .map(|(_, event)| event),
            "escape" => Some(self.safest()),
            _ => None,
        };
        if let Some(event) = event {
            cx.stop_propagation();
            cx.emit(event);
        }
    }
}

impl Render for Dialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.wants_focus {
            self.wants_focus = false;
            let focus = self.focus.clone();
            // Focus after this frame, when the dialog is in the tree.
            window.on_next_frame(move |window, cx| window.focus(&focus, cx));
        }
        let selected = self.selected;
        let buttons = self
            .actions()
            .into_iter()
            .enumerate()
            .map(|(i, (label, event))| {
                // Brackets and a marker keep the state readable without colour.
                let text = if i == selected {
                    format!("> [{label}] <")
                } else {
                    format!("[{label}]")
                };
                div()
                    .id(("dialog-btn", i))
                    .px_4()
                    .py_2()
                    .border_1()
                    .border_color(theme::color(theme::INK))
                    .rounded_sm()
                    .text_size(px(18.))
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::color(theme::ACTIVE)))
                    .font_weight(if i == selected {
                        FontWeight::BOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(text)
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(event.clone());
                    }))
            })
            .collect::<Vec<_>>();

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .flex()
            .flex_col()
            .gap_4()
            .w(px(560.))
            .p_6()
            .rounded_md()
            .bg(theme::color(theme::BG))
            .border_1()
            .border_color(theme::color(theme::RULE))
            .text_color(theme::color(theme::INK))
            .text_size(px(18.))
            .child(
                div()
                    .text_size(px(24.))
                    .font_weight(FontWeight::BOLD)
                    .child(self.title()),
            )
            .child(div().child(self.body()))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_3()
                    .children(buttons),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(theme::color(theme::MUTED))
                    .child("Tab: next button. Enter: choose. Escape: keep everything."),
            )
    }
}
