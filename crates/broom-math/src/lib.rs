//! broom-math: TeX → image for inline and display math.
//!
//! The rendering backend is NOT chosen yet. [`render_math`] returns
//! [`MathError::Unsupported`] until an implementer picks one. The API below
//! is the contract the editor codes against.
//!
//! Image format: straight-alpha RGBA8, row-major, top row first.
//! GPUI wants BGRA. The editor converts once when it builds the render image.

#![allow(unused_variables, dead_code)] // scaffold: remove as stubs are implemented

use std::collections::HashMap;
use std::sync::Arc;

/// A rendered formula.
#[derive(Clone, Debug, PartialEq)]
pub struct MathImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8, straight alpha.
    pub rgba: Vec<u8>,
    /// Distance in pixels from the top edge to the text baseline.
    /// Used to align inline math with surrounding text.
    pub baseline: f32,
}

/// Errors from math rendering.
#[derive(Debug, thiserror::Error)]
pub enum MathError {
    /// No backend is available yet. The UI shows the raw TeX.
    #[error("math rendering is not available yet")]
    Unsupported,
    /// The TeX source is invalid. The UI shows the raw TeX with a marker.
    #[error("invalid TeX: {0}")]
    Parse(String),
    /// The backend failed for another reason.
    #[error("math backend failed: {0}")]
    Backend(String),
}

/// Renders `tex`. `display = true` for `$$ ... $$` (centered, larger),
/// `false` for inline `$...$`. The image is not cached here. See [`MathCache`].
///
/// Scaffold: always returns `Err(MathError::Unsupported)`.
pub fn render_math(tex: &str, display: bool) -> Result<MathImage, MathError> {
    Err(MathError::Unsupported)
}

/// Caches rendered formulas by `(tex, display)`. Evicts everything when full.
#[derive(Debug)]
pub struct MathCache {
    capacity: usize,
    entries: HashMap<(String, bool), Arc<MathImage>>,
}

impl MathCache {
    /// Empty cache that holds at most `capacity` formulas.
    pub fn new(capacity: usize) -> MathCache {
        MathCache {
            capacity,
            entries: HashMap::new(),
        }
    }

    /// Returns the cached image, or renders and caches it. Errors are not cached.
    pub fn get_or_render(&mut self, tex: &str, display: bool) -> Result<Arc<MathImage>, MathError> {
        todo!()
    }

    /// Drops every cached image (for example when the font size changes).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Number of cached images.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_unsupported() {
        assert!(matches!(
            render_math("x^2", false),
            Err(MathError::Unsupported)
        ));
    }
}
