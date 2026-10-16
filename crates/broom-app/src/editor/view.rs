//! WYSIWYG rendering. Each visual row is a [`StyledText`] (so markdown
//! symbols never show) with a `canvas` overlay that paints the caret,
//! selection, and inline math images from the text layout. Hit testing reads
//! the same layout.

use gpui_kit::component::input::Editor as CodeEditor;
use gpui_kit::*;

use broom_core::{Align, DocPos, DocRange};

use super::math::InlineMath;
use super::rows::{Deco, Line, Marker, Seg, Shown, block_lines, byte_index, char_index, drawn_seg};
use super::{Editor, LinkTarget, SegHandle, ViewMode};
use crate::theme::{self, color};

/// Width of the marker gutter (bullets, numbers, checkboxes). Markers hang
/// in it, left of the text column.
const MARK_W: f32 = 32.0;
/// Gap between a marker and the text it hangs from.
const MARK_GAP: f32 = 8.0;
/// Size in points that formulas are laid out at. See `broom_math`.
const FORMULA_PT: f32 = 16.0;

/// Shared text settings for one line.
#[derive(Clone)]
struct Base {
    font: Font,
    mono: Font,
    size: Pixels,
    color: Hsla,
    bold: bool,
}

/// An inline formula drawn over its placeholder spaces.
#[derive(Clone)]
struct Draw {
    /// Byte offset of the placeholder in the drawn text.
    byte: usize,
    math: InlineMath,
    /// Text size over the 16 pt formula size.
    scale: f32,
}

/// A segment whose inline math is drawn as images.
struct Inline {
    seg: Seg,
    shown: Shown,
    draws: Vec<Draw>,
}

/// What a segment's mouse handlers need. Cloned into each handler.
#[derive(Clone)]
struct SegHit {
    block: usize,
    start: usize,
    text: SharedString,
    shown: Shown,
    layout: TextLayout,
    links: Vec<(std::ops::Range<usize>, LinkTarget)>,
}

impl SegHit {
    /// Document position under `p`, and the link there if any.
    fn hit(&self, p: Point<Pixels>) -> (DocPos, Option<LinkTarget>) {
        let i = self.layout.index_for_position(p).unwrap_or_else(|e| e);
        let c = self.shown.to_own(char_index(&self.text, i));
        let link = self
            .links
            .iter()
            .find(|(r, _)| r.contains(&c))
            .map(|(_, t)| t.clone());
        (DocPos::new(self.block, self.start + c), link)
    }
}

use crate::keys::Run;

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.math.scale = window.scale_factor();
        let focus = self
            .focus_handle
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let body = match self.mode {
            ViewMode::Source => {
                self.ensure_source(window, cx);
                match self.source.clone() {
                    Some(state) => div()
                        .size_full()
                        .px_8()
                        .py_6()
                        .child(
                            CodeEditor::new(&state)
                                .appearance(false)
                                .bordered(false)
                                .h_full(),
                        )
                        .into_any_element(),
                    None => div().size_full().into_any_element(),
                }
            }
            ViewMode::Formatted => self
                .render_formatted(window, cx)
                .into_any_element(),
        };
        // Text input: registered every paint so IME sees the caret's element.
        let entity = cx.entity();
        let input_focus = focus.clone();
        let ime = canvas(
            |_, _, _| (),
            move |bounds, _, window, cx| {
                let handler = ElementInputHandler::new(bounds, entity.clone());
                window.handle_input(&input_focus, handler, cx);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let click_focus = focus.clone();
        div()
            .key_context("Editor")
            .track_focus(&focus)
            .size_full()
            .relative()
            .child(body)
            .child(ime)
            .on_mouse_down(
                MouseButton::Left,
                move |_, window, cx| window.focus(&click_focus, cx),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.handle_key_down(&ev.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, run: &Run, _window, cx| {
                // Actions stop propagation by default. Pass unhandled
                // commands on so the workspace can run them.
                if this.command(run.0, cx) {
                    cx.stop_propagation();
                } else {
                    cx.propagate();
                }
            }))
    }
}

impl Editor {
    fn render_formatted(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        self.segs.clear();
        let body = px(self.style.font_size);
        let measure = body * theme::MEASURE_EM;
        let mut column: Vec<AnyElement> = Vec::new();
        let mut caret_row: Option<usize> = None;
        for b in 0..self.doc.block_count() {
            if let Some(mini) = self.mini.as_ref().filter(|m| m.block == b) {
                let state = mini.state.clone();
                let el = div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_xs()
                            .text_color(color(theme::MUTED))
                            .pb_1()
                            .child("Locked block source. Escape saves."),
                    )
                    .child(
                        div()
                            .h(px(180.))
                            .border_1()
                            .border_color(color(theme::RULE))
                            .child(
                                CodeEditor::new(&state)
                                    .appearance(false)
                                    .bordered(false)
                                    .h_full(),
                            ),
                    );
                let top = if b > 0 {
                    body * theme::BLOCK_GAP
                } else {
                    px(0.)
                };
                column.push(column_row(el.into_any_element(), top, measure));
                continue;
            }
            let Some(block) = self.doc.block(b) else {
                continue;
            };
            let lines = block_lines(b, block);
            let n = lines.len();
            for (i, line) in lines.iter().enumerate() {
                if caret_row.is_none() && self.line_has_caret(line) {
                    caret_row = Some(column.len());
                }
                let el = self.render_line(line, i + 1 == n, window, cx);
                column.push(column_row(el, space_above(line, body), measure));
            }
        }
        if self.want_scroll {
            if let Some(ix) = caret_row {
                self.scroll.scroll_to_item(ix);
            }
            self.want_scroll = false;
        }
        div()
            .id("wysiwyg")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px_8()
            .py_6()
            .children(column)
    }

    fn line_has_caret(&self, line: &Line) -> bool {
        let c = self.cursor;
        c.block == line.block
            && line
                .segs
                .iter()
                .any(|s| c.offset >= s.start && c.offset <= s.start + s.chars())
    }

    fn render_line(
        &mut self,
        line: &Line,
        last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let size = px(self.style.font_size);
        let body = font_for(&self.style.body);
        let mono = font_for(&self.style.mono);
        let (font, text_size, text_color, bold) = match &line.deco {
            Deco::Heading(level) => (
                body.clone(),
                size * theme::heading_scale(*level),
                color(theme::INK),
                true,
            ),
            Deco::Code | Deco::Math | Deco::Front => {
                (mono.clone(), size * 0.92, color(theme::INK), false)
            }
            Deco::Opaque => (mono.clone(), size * 0.92, color(theme::MUTED), false),
            Deco::Rule | Deco::Plain => (body.clone(), size, color(theme::INK), false),
        };
        let base = Base {
            font,
            mono: mono.clone(),
            size: text_size,
            color: text_color,
            bold,
        };

        // The empty last line of an opaque block (a source that ends in a
        // newline) would add a blank row under the box. Give it no height.
        let trailing_empty = line.deco == Deco::Opaque
            && last
            && !line.first
            && line.segs.first().is_some_and(|s| s.text.is_empty());

        let content: AnyElement = if trailing_empty {
            div().h(px(0.)).into_any_element()
        } else if line.table {
            self.render_table_row(line, &base, window, cx)
                .into_any_element()
        } else if let Some(note) = &line.note {
            div()
                .text_sm()
                .text_color(color(theme::MUTED))
                .child(note.clone())
                .into_any_element()
        } else if line.deco == Deco::Rule {
            div()
                .w_full()
                .h(px(1.))
                .my_3()
                .bg(color(theme::RULE))
                .into_any_element()
        } else if let Some(seg) = line
            .segs
            .first()
            .filter(|s| line.deco == Deco::Math && self.caret_in(line.block, s).is_none())
        {
            // Math line with the caret elsewhere: paint the formula.
            self.math.display_element(&seg.text, mono.clone(), size)
        } else if let Some(seg) = line.segs.first() {
            let caret = self.caret_in(line.block, seg);
            let sel = self.sel_in(line.block, seg);
            self.seg_el(line.block, seg, &base, caret, sel, window, cx)
                .into_any_element()
        } else {
            div()
                .h(size * theme::LINE_HEIGHT)
                .into_any_element()
        };

        let mut inner = div().flex_1().min_w_0();
        if line.deco == Deco::Opaque && line.first {
            inner = inner.child(
                div()
                    .text_xs()
                    .text_color(color(theme::MUTED))
                    .pb_1()
                    .child("Locked — Enter edits the source."),
            );
        }
        let mut inner = inner.child(content);
        if matches!(
            line.deco,
            Deco::Code | Deco::Math | Deco::Front | Deco::Opaque
        ) {
            inner = inner.bg(color(theme::PANEL)).px_3();
            if line.first {
                inner = inner.pt_2();
            }
            if last {
                inner = inner.pb_2();
            }
        }
        let marker = self.marker_el(line, size, cx);
        let indent = px(line.depth as f32 * 24.0);
        let mut row = div()
            .flex()
            .flex_row()
            .w_full()
            .pl(indent)
            .child(marker)
            .child(inner);
        for _ in 0..line.quote {
            row = div()
                .flex()
                .flex_row()
                .w_full()
                .child(div().w(px(3.)).mr_3().bg(color(theme::RULE)))
                .child(row);
        }
        row.into_any_element()
    }

    fn marker_el(&mut self, line: &Line, size: Pixels, cx: &mut Context<Self>) -> AnyElement {
        // Same text size and line height as the text, so the marker sits on
        // the line's own baseline and centre.
        let gutter = div()
            .w(px(MARK_W))
            .flex_shrink_0()
            .pr(px(MARK_GAP))
            .text_right()
            .whitespace_nowrap()
            .text_size(size)
            .line_height(relative(theme::LINE_HEIGHT));
        let Some(marker) = &line.marker else {
            return gutter.into_any_element();
        };
        match marker {
            Marker::Bullet => gutter
                .text_color(color(theme::MUTED))
                .child("•")
                .into_any_element(),
            Marker::Number(n) => gutter
                .text_color(color(theme::MUTED))
                .child(format!("{n}."))
                .into_any_element(),
            Marker::Task { checked, index } => {
                let glyph = if *checked { "☑" } else { "☐" };
                let block = line.block;
                let index = *index;
                gutter
                    .cursor_pointer()
                    .child(glyph)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                            this.toggle_task(block, index, cx);
                        }),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_table_row(
        &mut self,
        line: &Line,
        base: &Base,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut row = div().flex().flex_row().w_full();
        if line.header {
            row = row.border_b_1().border_color(color(theme::RULE));
        }
        for seg in &line.segs {
            let caret = self.caret_in(line.block, seg);
            let sel = self.sel_in(line.block, seg);
            let cell_base = Base {
                bold: base.bold || line.header,
                ..base.clone()
            };
            let el = self.seg_el(line.block, seg, &cell_base, caret, sel, window, cx);
            let cell = div()
                .flex_1()
                .min_w(px(80.))
                .px_2()
                .py_1()
                .border_r_1()
                .border_color(color(theme::RULE))
                .child(el);
            let cell = match seg.align {
                Align::Center => cell.text_center(),
                Align::Right => cell.text_right(),
                _ => cell,
            };
            row = row.child(cell);
        }
        row
    }

    /// Caret offset inside `seg`, if the caret is there.
    fn caret_in(&self, block: usize, seg: &Seg) -> Option<usize> {
        let c = self.cursor;
        let end = seg.start + seg.chars();
        (c.block == block && c.offset >= seg.start && c.offset <= end).then(|| c.offset - seg.start)
    }

    /// Selection inside `seg` as char offsets, if any.
    fn sel_in(&self, block: usize, seg: &Seg) -> Option<(usize, usize)> {
        let r: DocRange = self.selection_range()?;
        let (s, e) = (seg.start, seg.start + seg.chars());
        let lo = if block == r.start.block {
            r.start.offset.max(s)
        } else if block > r.start.block {
            s
        } else {
            return None;
        };
        let hi = if block == r.end.block {
            r.end.offset.min(e)
        } else if block < r.end.block {
            e
        } else {
            return None;
        };
        (lo < hi).then_some((lo - s, hi - s))
    }

    /// The inline math of `seg` as images. `None` when there is no math, when
    /// the caret or selection is inside a formula (its source shows then), or
    /// when a formula does not render. Those cases draw the plain text.
    fn inline_math(
        &mut self,
        seg: &Seg,
        base: &Base,
        caret: Option<usize>,
        sel: Option<(usize, usize)>,
        window: &Window,
    ) -> Option<Inline> {
        // (tex, own char start, own char length) per math span.
        let mut maths: Vec<(String, usize, usize)> = Vec::new();
        let (mut at, mut bytes) = (0, 0);
        for span in &seg.spans {
            let part = &seg.text[bytes..bytes + span.len];
            let n = part.chars().count();
            if span.style.math {
                let (s, e) = (at, at + n);
                let editing = caret.is_some_and(|c| c > s && c < e)
                    || sel.is_some_and(|(a, b)| a < e && b > s);
                if editing {
                    return None;
                }
                maths.push((part.to_string(), s, n));
            }
            at += n;
            bytes += span.len;
        }
        if maths.is_empty() {
            return None;
        }
        let scale = base.size / px(FORMULA_PT);
        let fid = window.text_system().resolve_font(&base.font);
        let space = window
            .text_system()
            .advance(fid, base.size, '\u{a0}')
            .map_or(base.size * 0.25, |s| s.width);
        let mut images = Vec::new();
        let mut pads = Vec::new();
        for (tex, _, _) in &maths {
            let math = self.math.inline(tex)?;
            let width = math.size.width * scale;
            let pad = (width / space).round().max(1.0) as usize;
            pads.push(pad);
            images.push(math);
        }
        let (drawn, shown) = drawn_seg(seg, &pads);
        let draws = shown
            .maths
            .iter()
            .zip(images)
            .map(|(m, math)| Draw {
                byte: byte_index(&drawn.text, m.drawn),
                math,
                scale,
            })
            .collect();
        Some(Inline {
            seg: drawn,
            shown,
            draws,
        })
    }

    /// One editable segment: styled text, overlay, and pointer handlers.
    #[allow(clippy::too_many_arguments)]
    fn seg_el(
        &mut self,
        block: usize,
        seg: &Seg,
        base: &Base,
        caret: Option<usize>,
        sel: Option<(usize, usize)>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let inline = self.inline_math(seg, base, caret, sel, window);
        let (drawn, shown, draws) = match inline {
            Some(i) => (i.seg, i.shown, i.draws),
            None => (seg.clone(), Shown::default(), Vec::new()),
        };
        let text: SharedString = drawn.text.clone().into();
        let runs = text_runs(&drawn, base);
        let styled = StyledText::new(text.clone()).with_runs(runs);
        let layout = styled.layout().clone();
        let links = link_ranges(seg);
        let hit = SegHit {
            block,
            start: seg.start,
            text: text.clone(),
            shown: shown.clone(),
            layout: layout.clone(),
            links: links.clone(),
        };
        self.segs.push(SegHandle {
            block,
            start: seg.start,
            len: seg.chars(),
            text: text.clone(),
            shown: shown.clone(),
            layout: layout.clone(),
            links,
        });

        let caret_b = caret.map(|c| byte_index(&text, shown.to_drawn(c)));
        let sel_b = sel.map(|(a, b)| {
            (
                byte_index(&text, shown.to_drawn(a)),
                byte_index(&text, shown.to_drawn(b)),
            )
        });
        let fid = window.text_system().resolve_font(&base.font);
        let metrics = (
            window.text_system().ascent(fid, base.size),
            window.text_system().descent(fid, base.size),
        );
        // A formula taller than the text line grows the line box, so it does
        // not paint over the lines above or below.
        let lh = base.size * theme::LINE_HEIGHT;
        let (ascent, descent) = metrics;
        let inset = (lh - (ascent + descent)) / 2.0;
        let (mut above, mut below) = (px(0.), px(0.));
        for d in &draws {
            let up = d.math.baseline * d.scale;
            let down = (d.math.size.height - d.math.baseline) * d.scale;
            if up > above {
                above = up;
            }
            if down > below {
                below = down;
            }
        }
        let pad_top = if above > inset + ascent {
            above - (inset + ascent)
        } else {
            px(0.)
        };
        let pad_bottom = if below > lh - inset - ascent {
            below - (lh - inset - ascent)
        } else {
            px(0.)
        };
        let overlay = canvas(
            |_, _, _| (),
            move |_bounds, _, window, _cx| {
                paint_overlay(&layout, caret_b, sel_b, &draws, metrics, window)
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let down_hit = hit.clone();
        let move_hit = hit;
        div()
            .relative()
            .w_full()
            .min_h(lh)
            .pt(pad_top)
            .pb(pad_bottom)
            .text_size(base.size)
            .line_height(relative(theme::LINE_HEIGHT))
            .child(styled)
            .child(overlay)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                    let (pos, link) = down_hit.hit(ev.position);
                    let m = ev.modifiers;
                    this.pointer_down(pos, m.shift, m.platform, link, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _window, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    let (pos, _) = move_hit.hit(ev.position);
                    this.pointer_drag(pos, cx);
                }
            }))
    }
}

/// Space above a line. Each block after the first gets a gap, and a heading
/// adds room above itself, so headings sit closer to what follows them.
fn space_above(line: &Line, body: Pixels) -> Pixels {
    if !line.first {
        return px(0.);
    }
    let mut top = if line.block > 0 {
        body * theme::BLOCK_GAP
    } else {
        px(0.)
    };
    if let Deco::Heading(level) = line.deco {
        top += body * theme::heading_scale(level) * 0.6;
    }
    if line.table {
        top += body * theme::BLOCK_GAP;
    }
    top
}

/// One column child: centred in the window, at most the text measure wide.
fn column_row(el: AnyElement, top: Pixels, measure: Pixels) -> AnyElement {
    div()
        .w_full()
        .flex()
        .justify_center()
        .pt(top)
        .child(div().w(measure).max_w_full().child(el))
        .into_any_element()
}

/// The body font family, as a [`Font`].
fn font_for(name: &str) -> Font {
    theme::body_font(name)
}

/// Styled runs for a segment. Lengths are bytes and sum to the text length.
fn text_runs(seg: &Seg, base: &Base) -> Vec<TextRun> {
    seg.spans
        .iter()
        .map(|span| {
            let st = &span.style;
            let mut font = if st.code || st.math {
                base.mono.clone()
            } else {
                base.font.clone()
            };
            if base.bold || st.bold {
                font.weight = FontWeight::BOLD;
            }
            if st.italic {
                font.style = FontStyle::Italic;
            }
            let mut text_color = base.color;
            let mut background = None;
            let mut underline = None;
            if st.code {
                background = Some(color(theme::PANEL));
            }
            if st.math {
                // Fallback for a formula that did not render: plain source.
                text_color = color(theme::INK);
            }
            if st.link.is_some() {
                text_color = color(theme::LINK);
                underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color(theme::LINK)),
                    wavy: false,
                });
            }
            if st.wiki.is_some() {
                text_color = color(theme::WIKI);
                background = Some(color(theme::PANEL));
                underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color(theme::WIKI)),
                    wavy: false,
                });
            }
            let strikethrough = st.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
            TextRun {
                len: span.len,
                font,
                color: text_color,
                background_color: background,
                underline,
                strikethrough,
            }
        })
        .collect()
}

/// Char ranges of link and wikilink spans in a segment.
fn link_ranges(seg: &Seg) -> Vec<(std::ops::Range<usize>, LinkTarget)> {
    let mut out = Vec::new();
    let mut bytes = 0;
    let mut chars = 0;
    for span in &seg.spans {
        let n = seg.text[bytes..bytes + span.len].chars().count();
        let target = match (&span.style.wiki, &span.style.link) {
            (Some(w), _) => Some(LinkTarget::Wiki(w.clone())),
            (None, Some(u)) => Some(LinkTarget::Url(u.clone())),
            _ => None,
        };
        if let Some(t) = target {
            out.push((chars..chars + n, t));
        }
        bytes += span.len;
        chars += n;
    }
    out
}

/// Paints selection, inline math, and caret over a laid-out segment.
/// `metrics` is the font's ascent and descent at the text size.
fn paint_overlay(
    layout: &TextLayout,
    caret: Option<usize>,
    sel: Option<(usize, usize)>,
    draws: &[Draw],
    metrics: (Pixels, Pixels),
    window: &mut Window,
) {
    let lh = layout.line_height();
    let b = layout.bounds();
    let (ascent, descent) = metrics;
    // Where the glyph box sits inside the line box: centred, as GPUI lays it out.
    let inset = (lh - (ascent + descent)) / 2.0;
    if let Some((a, z)) = sel
        && let (Some(p0), Some(p1)) = (layout.position_for_index(a), layout.position_for_index(z))
    {
        let sel_color = color(theme::SELECTION);
        if (p1.y - p0.y).abs() < lh * 0.5 {
            window.paint_quad(fill(Bounds::new(p0, size(p1.x - p0.x, lh)), sel_color));
        } else {
            window.paint_quad(fill(Bounds::new(p0, size(b.right() - p0.x, lh)), sel_color));
            let mut y = p0.y + lh;
            while y < p1.y - lh * 0.5 {
                window.paint_quad(fill(
                    Bounds::new(point(b.left(), y), size(b.size.width, lh)),
                    sel_color,
                ));
                y += lh;
            }
            window.paint_quad(fill(
                Bounds::new(point(b.left(), p1.y), size(p1.x - b.left(), lh)),
                sel_color,
            ));
        }
    }
    for d in draws {
        let Some(p) = layout.position_for_index(d.byte) else {
            continue;
        };
        let baseline = p.y + inset + ascent;
        let top = baseline - d.math.baseline * d.scale;
        let w = d.math.size.width * d.scale;
        let h = d.math.size.height * d.scale;
        let bounds = Bounds::new(point(p.x, top), size(w, h));
        let _ = window.paint_image(
            bounds,
            bounds,
            Corners::all(px(0.)),
            d.math.image.clone(),
            0,
            false,
        );
    }
    if let Some(c) = caret {
        // The caret sits in the gap before the character, so it does not
        // cover the glyph it precedes.
        let p = layout.position_for_index(c).unwrap_or(b.origin);
        let top = p.y + inset;
        window.paint_quad(fill(
            Bounds::new(
                point(p.x - px(theme::CARET_WIDTH), top),
                size(px(theme::CARET_WIDTH), ascent + descent),
            ),
            color(theme::CARET),
        ));
    }
}
