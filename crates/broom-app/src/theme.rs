//! Solarized Light palette, fonts, and sizes. Light theme only.
//!
//! Meaning is never carried by colour alone: states also have text
//! (status bar words, `•` for unsaved, italics for preview tabs, glyphs
//! for folders and tasks).

use gpui_kit::*;

pub const BASE03: u32 = 0x002b36;
pub const BASE02: u32 = 0x073642;
pub const BASE01: u32 = 0x586e75;
pub const BASE00: u32 = 0x657b83;
pub const BASE2: u32 = 0xeee8d5;
pub const BASE3: u32 = 0xfdf6e3;
pub const YELLOW: u32 = 0xb58900;
pub const ORANGE: u32 = 0xcb4b16;
pub const RED: u32 = 0xdc322f;
pub const VIOLET: u32 = 0x6c71c4;
pub const BLUE: u32 = 0x268bd2;

/// Page background.
pub const BG: u32 = BASE3;
/// Side panes, status bar, code blocks.
pub const PANEL: u32 = BASE2;
/// Hover and active rows.
pub const ACTIVE: u32 = 0xe2dac3;
/// Body text. Darker than BASE00 for contrast.
pub const INK: u32 = BASE02;
/// Secondary text.
pub const MUTED: u32 = BASE01;
/// Hairlines.
pub const RULE: u32 = 0xd0c8b0;
/// Selection highlight.
pub const SELECTION: u32 = 0xbcd6e6;
/// Caret: near black, drawn 3 px wide.
pub const CARET: u32 = BASE03;
/// Links.
pub const LINK: u32 = BLUE;
/// Wikilinks.
pub const WIKI: u32 = VIOLET;
/// Warning bar background.
pub const WARN_BG: u32 = 0xf3e3a6;
/// Danger text (conflicts, errors).
pub const DANGER: u32 = RED;

/// Colour from a `0xRRGGBB` literal.
pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Caret width in pixels.
pub const CARET_WIDTH: f32 = 3.0;

/// Heading size multiplier for level 1..=6.
pub fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 2.0,
        2 => 1.6,
        3 => 1.35,
        4 => 1.18,
        5 => 1.05,
        _ => 1.0,
    }
}

/// Clamped zoom range in points.
pub const ZOOM_MIN: f32 = 9.0;
pub const ZOOM_MAX: f32 = 40.0;

/// Body font, as configured. `.SystemUIFont` is SF Pro on macOS.
pub fn body_font(name: &str) -> Font {
    font(name.to_string())
}

/// Mono font for code, source, and math fallback.
pub fn mono_font(name: &str) -> Font {
    font(name.to_string())
}
