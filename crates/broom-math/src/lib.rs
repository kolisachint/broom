//! broom-math: TeX → image for inline and display math.
//!
//! Backend: `mitex` converts TeX to Typst math syntax (pure Rust, no deps),
//! then Typst 0.15 lays it out and `typst-render` rasterizes it. Everything is
//! pure Rust and builds offline on macOS with no system libraries.
//!
//! Fonts: `typst_assets::fonts()` is all-or-nothing (its per-face constants
//! cover only PDF fonts), so every bundled face is embedded, about 17 files.
//! The math face is New Computer Modern Math.
//!
//! Image format: straight-alpha RGBA8, row-major, top row first. The ink is
//! black on a transparent background, so the editor can draw it over any
//! theme color. GPUI wants BGRA. The editor converts once when it builds the
//! render image.
//!
//! Sizes: formulas are laid out at a 16 pt font size (the editor's default
//! body size). `scale` is pixels per point, so `scale = 1.0` gives 16 px text.
//! [`render_math`] uses [`DEFAULT_SCALE`] (2.0, for HiDPI screens).
//!
//! Failures never panic. Unparseable TeX, formulas with no visible ink, and
//! formulas over [`MAX_PIXELS`] all give [`MathError::Parse`]. A panic inside
//! mitex or Typst is caught and also becomes `Parse`. The UI shows the raw
//! source in a styled block.
//!
//! Security: the TeX never becomes Typst source. It reaches Typst as a string
//! input, evaluated in math mode. `\iftypst` is the one mitex command that
//! emits raw Typst, so it is rejected before conversion. Typst source cannot
//! be injected. Images (`\includegraphics`) fail: no file resolver is set.

use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};

use typst::foundations::{Dict, IntoValue};
use typst_as_lib::{TypstAsLibError, TypstEngine, TypstTemplateMainFile};
use typst_layout::PagedDocument;

/// Pixels per point used by [`render_math`]. 2.0 suits HiDPI (Retina) screens.
pub const DEFAULT_SCALE: f32 = 2.0;

/// Typst template. The formula arrives as a string input and is evaluated in
/// math mode, so TeX text never becomes Typst source and cannot inject code.
const TEMPLATE: &str = r#"
#import sys: inputs
#set page(width: auto, height: auto, margin: 0pt, fill: none)
#set text(size: 16pt, fill: black)
#let body = eval(inputs.math, mode: "math")
#if inputs.display [$ #body $] else [$#body$]
"#;

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

/// The Typst engine holds the parsed fonts, so it is built once and shared.
fn engine() -> &'static TypstEngine<TypstTemplateMainFile> {
    static ENGINE: OnceLock<TypstEngine<TypstTemplateMainFile>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        TypstEngine::builder()
            .main_file(TEMPLATE)
            .fonts(typst_assets::fonts())
            .build()
    })
}

/// Renders `tex`. `display = true` for `$$ ... $$` (centered, larger),
/// `false` for inline `$...$`. The image is not cached here. See [`MathCache`].
///
/// Uses [`DEFAULT_SCALE`]. See [`render_math_scaled`] for other densities.
pub fn render_math(tex: &str, display: bool) -> Result<MathImage, MathError> {
    render_math_scaled(tex, display, DEFAULT_SCALE)
}

/// Largest image, in pixels (width × height), that will be rendered.
pub const MAX_PIXELS: u64 = 8_000_000;

/// TeX that mitex turns into raw Typst source. Rejected before conversion.
const RAW_TYPST_COMMAND: &str = "\\iftypst";

/// Like [`render_math`], with `scale` pixels per point (16 pt text at
/// `scale = 1.0` is 16 px tall). `scale` must be finite and positive.
pub fn render_math_scaled(tex: &str, display: bool, scale: f32) -> Result<MathImage, MathError> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(MathError::Backend(format!("invalid scale {scale}")));
    }
    if tex.trim().is_empty() {
        return Err(MathError::Parse("empty formula".to_string()));
    }
    if tex.contains(RAW_TYPST_COMMAND) {
        return Err(MathError::Parse(format!(
            "`{RAW_TYPST_COMMAND}` is not allowed"
        )));
    }
    // mitex, Typst, and typst-render can panic on odd input. Turn that into an error.
    std::panic::catch_unwind(AssertUnwindSafe(|| render_checked(tex, display, scale)))
        .unwrap_or_else(|_| {
            Err(MathError::Parse(
                "formula could not be rendered".to_string(),
            ))
        })
}

/// The rendering pipeline. Called only through [`render_math_scaled`], which
/// has already validated the input and wrapped this in `catch_unwind`.
fn render_checked(tex: &str, display: bool, scale: f32) -> Result<MathImage, MathError> {
    let math = mitex::convert_math(tex, None).map_err(MathError::Parse)?;

    let mut inputs = Dict::new();
    inputs.insert("math".into(), math.into_value());
    inputs.insert("display".into(), display.into_value());

    let warned = engine().compile_with_input::<_, PagedDocument>(inputs);
    let doc = warned.output.map_err(|err| match err {
        TypstAsLibError::TypstSource(_) | TypstAsLibError::HintedString(_) => {
            MathError::Parse(err.to_string())
        }
        other => MathError::Backend(other.to_string()),
    })?;

    let page = doc
        .pages()
        .first()
        .ok_or_else(|| MathError::Backend("Typst produced no page".to_string()))?;

    // Check the size before rasterizing, so huge formulas cost nothing.
    let size = page.frame.size();
    let px_w = size.x.to_pt() * f64::from(scale);
    let px_h = size.y.to_pt() * f64::from(scale);
    if px_w * px_h > MAX_PIXELS as f64 {
        return Err(MathError::Parse(format!(
            "formula is too large ({} × {} px)",
            px_w.ceil(),
            px_h.ceil()
        )));
    }

    let options = typst_render::RenderOptions {
        pixel_per_pt: typst::utils::Scalar::new(f64::from(scale)),
        ..Default::default()
    };
    let pixmap = typst_render::render(page, &options);
    let (width, height) = (pixmap.width(), pixmap.height());
    if width == 0 || height == 0 {
        return Err(MathError::Parse("formula has no visible size".to_string()));
    }

    let rgba = pixmap.take_demultiplied();
    // Empty input such as `{}` or `\label{x}` lays out to a box but draws nothing.
    if !rgba.iter().skip(3).step_by(4).any(|&a| a > 0) {
        return Err(MathError::Parse(
            "formula has no visible content".to_string(),
        ));
    }

    let baseline_pt = formula_baseline_pt(&page.frame);
    let baseline = (baseline_pt * f64::from(scale)).clamp(0.0, f64::from(height)) as f32;

    Ok(MathImage {
        width,
        height,
        rgba,
        baseline,
    })
}

/// Baseline of the formula in points, measured from the top of `frame`.
///
/// Typst leaves the baseline unset on a page frame (it then reports the
/// bottom edge). A display equation sits in a single group, though, and that
/// group carries the real baseline (the axis of a `\sum`, for example). So
/// descend into a lone group and add its offset.
fn formula_baseline_pt(frame: &typst::layout::Frame) -> f64 {
    use typst::layout::FrameItem;

    let own = frame.baseline().to_pt();
    let height = frame.size().y.to_pt();
    if own < height - 1e-6 {
        return own;
    }

    let mut lone_group = None;
    for (pos, item) in frame.items() {
        match item {
            FrameItem::Group(group) => {
                if lone_group.is_some() {
                    return own;
                }
                lone_group = Some((pos, group));
            }
            FrameItem::Text(_) | FrameItem::Shape(..) | FrameItem::Image(..) => return own,
            _ => {}
        }
    }

    match lone_group {
        Some((pos, group)) => {
            let offset = pos.y.to_pt() + group.transform.ty.to_pt();
            (offset + formula_baseline_pt(&group.frame)).min(height)
        }
        None => own,
    }
}

/// Cache key: TeX source, display flag, and the bit pattern of the scale.
type CacheKey = (String, bool, u32);

/// Default byte budget for [`MathCache::new`]: 64 MB of RGBA pixels.
pub const DEFAULT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Caches rendered formulas by `(tex, display, scale)`. Evicts the least
/// recently used entries when either the entry count or the byte budget
/// (sum of `rgba` lengths) would be exceeded. Errors are not cached, and an
/// image larger than the whole budget is not cached.
#[derive(Debug)]
pub struct MathCache {
    capacity: usize,
    max_bytes: usize,
    bytes: usize,
    entries: HashMap<CacheKey, Arc<MathImage>>,
    /// Keys from least to most recently used.
    order: VecDeque<CacheKey>,
}

impl MathCache {
    /// Empty cache that holds at most `capacity` formulas and
    /// [`DEFAULT_CACHE_BYTES`]. A capacity of 0 renders every call and caches nothing.
    pub fn new(capacity: usize) -> MathCache {
        MathCache::with_budget(capacity, DEFAULT_CACHE_BYTES)
    }

    /// Like [`MathCache::new`], with an explicit byte budget.
    pub fn with_budget(capacity: usize, max_bytes: usize) -> MathCache {
        MathCache {
            capacity,
            max_bytes,
            bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Returns the cached image, or renders and caches it. Errors are not cached.
    /// Uses [`DEFAULT_SCALE`].
    pub fn get_or_render(&mut self, tex: &str, display: bool) -> Result<Arc<MathImage>, MathError> {
        self.get_or_render_scaled(tex, display, DEFAULT_SCALE)
    }

    /// Like [`MathCache::get_or_render`], with an explicit pixels-per-point scale.
    pub fn get_or_render_scaled(
        &mut self,
        tex: &str,
        display: bool,
        scale: f32,
    ) -> Result<Arc<MathImage>, MathError> {
        let key: CacheKey = (tex.to_string(), display, scale.to_bits());

        if let Some(image) = self.entries.get(&key).cloned() {
            self.touch(&key);
            return Ok(image);
        }

        let image = Arc::new(render_math_scaled(tex, display, scale)?);
        let size = image.rgba.len();
        if self.capacity == 0 || size > self.max_bytes {
            return Ok(image);
        }
        while self.entries.len() >= self.capacity || self.bytes + size > self.max_bytes {
            match self.order.pop_front() {
                Some(oldest) => {
                    if let Some(old) = self.entries.remove(&oldest) {
                        self.bytes -= old.rgba.len();
                    }
                }
                None => break,
            }
        }
        self.bytes += size;
        self.order.push_back(key.clone());
        self.entries.insert(key, Arc::clone(&image));
        Ok(image)
    }

    /// Drops every cached image (for example when the font size changes).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    /// Number of cached images.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total RGBA bytes held by the cache. Never more than the byte budget.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Marks `key` as most recently used.
    fn touch(&mut self, key: &CacheKey) {
        let pos = self.order.iter().position(|k| k == key);
        if let Some(k) = pos.and_then(|pos| self.order.remove(pos)) {
            self.order.push_back(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_valid(img: &MathImage) {
        assert!(img.width > 0 && img.height > 0, "empty image");
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        assert!(img.baseline >= 0.0 && img.baseline <= img.height as f32);
        // Some pixel must carry ink.
        assert!(img.rgba.iter().skip(3).step_by(4).any(|&a| a > 0), "no ink");
    }

    #[test]
    fn display_sum_baseline_is_inside_the_group() {
        // The sum's axis is well above its bottom edge, so the baseline must
        // come from the inner group, not the page bottom.
        let img = render_math(r"\sum_{i=1}^{n} i^2", true).expect("sum");
        assert!(img.baseline > 0.0);
        assert!(img.baseline < img.height as f32 - 1.0);
    }

    #[test]
    fn renders_superscript_inline() {
        let img = render_math("x^2", false).expect("x^2 should render");
        assert_valid(&img);
    }

    #[test]
    fn renders_fraction_display() {
        let img = render_math(r"\frac{a}{b}", true).expect("fraction should render");
        assert_valid(&img);
        // Display fractions are taller than a single line of text.
        let inline = render_math(r"\frac{a}{b}", false).expect("inline fraction");
        assert!(img.height >= inline.height);
    }

    #[test]
    fn baseline_sits_inside_the_image() {
        let img = render_math(r"\frac{a}{b}", false).expect("fraction");
        assert!(img.baseline > 0.0);
        assert!(img.baseline < img.height as f32);
    }

    #[test]
    fn scale_changes_pixel_size() {
        let small = render_math_scaled("x^2", false, 1.0).expect("scale 1");
        let big = render_math_scaled("x^2", false, 3.0).expect("scale 3");
        assert!(big.width > small.width);
        assert!(big.height > small.height);
    }

    #[test]
    fn rejects_bad_scale_and_empty_input() {
        assert!(matches!(
            render_math_scaled("x", false, 0.0),
            Err(MathError::Backend(_))
        ));
        assert!(matches!(
            render_math_scaled("x", false, f32::NAN),
            Err(MathError::Backend(_))
        ));
        assert!(matches!(render_math("   ", true), Err(MathError::Parse(_))));
    }

    #[test]
    fn malformed_tex_is_an_error_not_a_panic() {
        // Unclosed group: must return Err, never panic.
        assert!(render_math(r"\frac{a", false).is_err());
    }

    #[test]
    fn cache_hits_return_the_same_image() {
        let mut cache = MathCache::new(8);
        let first = cache.get_or_render("x^2", false).expect("render");
        let second = cache.get_or_render("x^2", false).expect("hit");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_keys_include_display_and_scale() {
        let mut cache = MathCache::new(8);
        cache.get_or_render("x^2", false).expect("inline");
        cache.get_or_render("x^2", true).expect("display");
        cache
            .get_or_render_scaled("x^2", false, 1.0)
            .expect("scale 1");
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let mut cache = MathCache::new(2);
        let a = cache.get_or_render("a", false).expect("a");
        cache.get_or_render("b", false).expect("b");
        // Touch `a` so `b` becomes the oldest entry.
        let a_again = cache.get_or_render("a", false).expect("a hit");
        assert!(Arc::ptr_eq(&a, &a_again));
        cache.get_or_render("c", false).expect("c evicts b");
        assert_eq!(cache.len(), 2);
        let a_third = cache.get_or_render("a", false).expect("a still cached");
        assert!(Arc::ptr_eq(&a, &a_third));
    }

    #[test]
    fn cache_does_not_store_errors() {
        let mut cache = MathCache::new(4);
        assert!(cache.get_or_render("   ", false).is_err());
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_clear_empties() {
        let mut cache = MathCache::new(4);
        cache.get_or_render("x", false).expect("x");
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn zero_capacity_renders_without_caching() {
        let mut cache = MathCache::new(0);
        cache.get_or_render("x", false).expect("x");
        assert!(cache.is_empty());
    }

    #[test]
    fn empty_braces_are_an_error() {
        assert!(matches!(render_math("{}", false), Err(MathError::Parse(_))));
    }

    #[test]
    fn label_without_ink_is_an_error() {
        assert!(matches!(
            render_math(r"\label{x}", false),
            Err(MathError::Parse(_))
        ));
    }

    #[test]
    fn include_graphics_is_an_error() {
        assert!(render_math(r"\includegraphics{a}", false).is_err());
    }

    #[test]
    fn raw_typst_command_is_rejected_before_conversion() {
        assert!(matches!(
            render_math(r"\iftypst x \fi", false),
            Err(MathError::Parse(ref m)) if m.contains("iftypst")
        ));
    }

    #[test]
    fn oversized_formula_is_rejected_before_rasterizing() {
        let tex = r"\quad".repeat(20_000);
        assert!(
            matches!(render_math(&tex, false), Err(MathError::Parse(ref m)) if m.contains("too large"))
        );
    }

    #[test]
    fn cache_respects_byte_budget() {
        // Each `x` image is a few KB. A budget of one image's size keeps one entry.
        let one = render_math("x", false).expect("x").rgba.len();
        let mut cache = MathCache::with_budget(100, one);
        cache.get_or_render("x", false).expect("x");
        cache.get_or_render("y", false).expect("y");
        assert_eq!(cache.len(), 1);
        assert!(cache.bytes() <= one);
    }

    #[test]
    fn image_bigger_than_budget_is_not_cached() {
        let mut cache = MathCache::with_budget(10, 16);
        assert!(cache.get_or_render("x^2", false).is_ok());
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }
}
