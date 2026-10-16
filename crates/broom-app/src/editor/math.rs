//! Paints display math from `broom-math` images. Falls back to the raw TeX.
//!
//! Two caches: [`MathCache`] holds CPU images per `(tex, display)`. The
//! painter holds GPUI images per `(tex, display, scale)`, since a GPUI image
//! is built once and reused every frame. Failures are cached too, so a
//! missing backend is not retried on every paint.

use std::collections::HashMap;
use std::sync::Arc;

use broom_math::{DEFAULT_SCALE, MathCache, MathError, MathImage};
use gpui_kit::*;
use image::{Frame, RgbaImage};

use crate::theme;
use crate::workspace::guarded;

/// Formulas kept in each cache before it is emptied.
const MAX_ENTRIES: usize = 256;

/// Key of a GPUI image: TeX, display flag, and the scale factor's bits.
type ImageKey = (String, bool, u32);

/// A cached paint result: the image, or a note for the source fallback.
type Painted = Result<Arc<RenderImage>, String>;

/// An inline formula, sized in points at the 16 pt body size.
#[derive(Clone)]
pub struct InlineMath {
    pub image: Arc<RenderImage>,
    /// Size in points.
    pub size: Size<Pixels>,
    /// Distance in points from the top edge to the text baseline.
    pub baseline: Pixels,
}

pub struct MathPainter {
    /// Window scale factor. The editor sets it at the start of each render.
    pub scale: f32,
    cache: MathCache,
    images: HashMap<ImageKey, Painted>,
    inline: HashMap<String, Option<InlineMath>>,
}

impl Default for MathPainter {
    fn default() -> Self {
        MathPainter {
            scale: 1.0,
            cache: MathCache::new(MAX_ENTRIES),
            images: HashMap::new(),
            inline: HashMap::new(),
        }
    }
}

impl MathPainter {
    /// Drops every cached image (font size or scale changed).
    pub fn clear(&mut self) {
        self.images.clear();
        self.inline.clear();
        let _ = guarded("math cache", || self.cache.clear());
    }

    /// The painted result for `tex`, cached per `(tex, display, scale)`.
    fn painted(&mut self, tex: &str, display: bool) -> Painted {
        let key = (tex.to_string(), display, self.scale.to_bits());
        if let Some(hit) = self.images.get(&key) {
            return hit.clone();
        }
        if self.images.len() >= MAX_ENTRIES {
            self.images.clear();
        }
        let result = match guarded("math render", || self.cache.get_or_render(tex, display)) {
            Some(Ok(math)) => {
                to_render_image(&math).ok_or_else(|| "math image is malformed".to_string())
            }
            Some(Err(err)) => Err(note_for(&err)),
            None => Err(note_for(&MathError::Unsupported)),
        };
        self.images.insert(key, result.clone());
        result
    }

    /// The inline formula for `tex`, or `None` when it cannot be rendered.
    pub fn inline(&mut self, tex: &str) -> Option<InlineMath> {
        if let Some(hit) = self.inline.get(tex) {
            return hit.clone();
        }
        if self.inline.len() >= MAX_ENTRIES {
            self.inline.clear();
        }
        let result = match guarded("math render", || self.cache.get_or_render(tex, false)) {
            Some(Ok(math)) => to_render_image(&math).map(|image| {
                let scale = DEFAULT_SCALE;
                InlineMath {
                    image,
                    size: size(
                        px(math.width as f32 / scale),
                        px(math.height as f32 / scale),
                    ),
                    baseline: px(math.baseline / scale),
                }
            }),
            _ => None,
        };
        self.inline.insert(tex.to_string(), result.clone());
        result
    }

    /// Paints display `tex` as a centred image. Falls back to the raw TeX
    /// with a short note when no image is available.
    pub fn display_element(&mut self, tex: &str, mono: Font, size: Pixels) -> AnyElement {
        match self.painted(tex, true) {
            Ok(image) => {
                let scale = self.scale.max(0.01);
                let (w, h) = logical_size(&image, scale);
                div()
                    .flex()
                    .justify_center()
                    .py_2()
                    .child(img(ImageSource::Render(image)).w(px(w)).h(px(h)))
                    .into_any_element()
            }
            Err(note) => fallback(tex, mono, size, &note),
        }
    }
}

/// Short text for a math error. Invalid TeX gets a marker in the text.
fn note_for(err: &MathError) -> String {
    match err {
        MathError::Unsupported => "[math not rendered yet]".into(),
        MathError::Parse(_) => "⚠ invalid TeX".into(),
        MathError::Backend(_) => "[math failed]".into(),
    }
}

/// The raw TeX with a note. Used when no image is available.
pub fn fallback(tex: &str, mono: Font, size: Pixels, note: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .font(mono)
                .text_size(size * 0.92)
                .text_color(theme::color(theme::INK))
                .child(tex.to_string()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::color(theme::MUTED))
                .child(note.to_string()),
        )
        .into_any_element()
}

/// Logical size of an image: pixel size divided by the scale factor.
fn logical_size(image: &RenderImage, scale: f32) -> (f32, f32) {
    let size = image.size(0);
    (size.width.0 as f32 / scale, size.height.0 as f32 / scale)
}

/// Converts a straight-alpha RGBA8 image to a GPUI image.
///
/// GPUI's decoded raster images are straight-alpha BGRA (see
/// `decode_static_image_from_decoder` in gpui-pre), so only R and B swap.
/// Premultiplying here would darken the edges.
fn to_render_image(math: &MathImage) -> Option<Arc<RenderImage>> {
    let expected = math.width as usize * math.height as usize * 4;
    if math.rgba.len() != expected {
        return None;
    }
    let mut bytes = math.rgba.clone();
    for px in bytes.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buf = RgbaImage::from_raw(math.width, math.height, bytes)?;
    Some(Arc::new(RenderImage::new([Frame::new(buf)])))
}
