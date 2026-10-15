//! Modal dialogs for file safety: external change and crash recovery.
//!
//! A dialog only reports the user's choice as a [`DialogEvent`]. The
//! workspace does the file work.

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
}

/// The user's choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogEvent {
    /// Load the file from disk, dropping buffer changes.
    Reload,
    /// Keep the buffer and make it win on the next save.
    KeepMine,
    /// Write the buffer to disk now, over the external change.
    Save,
    /// Restore a recovery draft by key.
    Restore(String),
    /// Delete a recovery draft by key.
    Discard(String),
}

impl EventEmitter<DialogEvent> for Dialog {}

pub struct Dialog {
    pub kind: DialogKind,
}

impl Dialog {
    pub fn new(kind: DialogKind) -> Dialog {
        Dialog { kind }
    }

    /// Title line, in words.
    fn title(&self) -> String {
        match &self.kind {
            DialogKind::Conflict { kind, .. } => match kind {
                ConflictKind::Modified => "File changed on disk".into(),
                ConflictKind::Deleted => "File was deleted on disk".into(),
            },
            DialogKind::Recovery { .. } => "Recover unsaved changes?".into(),
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
        }
    }

    /// The buttons, as (label, event).
    fn actions(&self) -> Vec<(&'static str, DialogEvent)> {
        match &self.kind {
            DialogKind::Conflict { .. } => vec![
                ("Reload", DialogEvent::Reload),
                ("Keep mine", DialogEvent::KeepMine),
                ("Save", DialogEvent::Save),
            ],
            DialogKind::Recovery { entry } => vec![
                ("Restore", DialogEvent::Restore(entry.key.clone())),
                ("Discard", DialogEvent::Discard(entry.key.clone())),
            ],
        }
    }
}

impl Render for Dialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let buttons = self
            .actions()
            .into_iter()
            .enumerate()
            .map(|(i, (label, event))| {
                // Brackets keep the buttons readable without colour.
                div()
                    .id(("dialog-btn", i))
                    .px_3()
                    .py_1()
                    .border_1()
                    .border_color(theme::color(theme::INK))
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::color(theme::ACTIVE)))
                    .child(format!("[{label}]"))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(event.clone());
                    }))
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(440.))
            .p_5()
            .rounded_md()
            .bg(theme::color(theme::BG))
            .border_1()
            .border_color(theme::color(theme::RULE))
            .text_color(theme::color(theme::INK))
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child(self.title()),
            )
            .child(div().child(self.body()))
            .child(div().flex().flex_row().gap_2().children(buttons))
    }
}
