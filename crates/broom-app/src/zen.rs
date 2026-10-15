//! Zen mode: hides the side panels and the status bar, and keeps the text
//! column at a readable width. Toggled by `Command::ZenMode` (Cmd+K Z).

use gpui_kit::*;

use crate::workspace::Workspace;

/// Widest text column in zen mode, in logical pixels.
pub const MAX_WIDTH: f32 = 720.0;

impl Workspace {
    /// Flips zen mode on or off.
    pub fn toggle_zen(&mut self, cx: &mut Context<Self>) {
        self.zen = !self.zen;
        cx.notify();
    }
}

/// Wraps the centre body. In zen mode it becomes a centred column of at most
/// [`MAX_WIDTH`]. Otherwise the body is returned unchanged.
pub fn column(zen: bool, body: AnyElement) -> AnyElement {
    if zen {
        div()
            .w_full()
            .max_w(px(MAX_WIDTH))
            .h_full()
            .mx_auto()
            .child(body)
            .into_any_element()
    } else {
        body
    }
}
