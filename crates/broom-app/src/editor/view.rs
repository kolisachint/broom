//! WYSIWYG rendering. Each visual row is a [`StyledText`] (so markdown
//! symbols never show) with a `canvas` overlay that paints the caret and
//! selection from the text layout. Hit testing reads the same layout.

use gpui_kit::component::input::Editor as CodeEditor;
use gpui_kit::*;

use broom_core::{Align, DocPos, DocRange};

use super::rows::{Deco, Line, Marker, Seg, block_lines, byte_index, char_index};
use super::{Editor, LinkTarget, SegHandle, ViewMode};
use crate::theme::{self, color};

/// Width of the marker column (bullets, numbers, checkboxes).
const MARK_W: f32 = 26.0;

/// Shared text settings for one line.
#[derive(Clone)]
struct Base {
    font: Font,
    mono: Font,
    size: Pixels,
    color: Hsla,
    bold: bool,
}

/// What a segment's mouse handlers need. Cloned into each handler.
#[derive(Clone)]
struct SegHit {
    block: usize,
    start: usize,
    text: SharedString,
    layout: TextLayout,
    links: Vec<(std::ops::Range<usize>, LinkTarget)>,
}

impl SegHit {
    /// Document position under `p`, and the link there if any.
    fn hit(&self, p: Point<Pixels>) -> (DocPos, Option<LinkTarget>) {
        let i = self.layout.index_for_position(p).unwrap_or_else(|e| e);
        let c = char_index(&self.text, i);
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
                        .child(CodeEditor::new(&state).h_full())
                        .into_any_element(),
                    None => div().size_full().into_any_element(),
                }
            }
            ViewMode::Formatted => self.render_formatted(cx).into_any_element(),
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
                if this.command(run.0, cx) {
                    cx.stop_propagation();
                }
            }))
    }
}

impl Editor {
    fn render_formatted(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        self.segs.clear();
        let mut column: Vec<AnyElement> = Vec::new();
        let mut caret_row: Option<usize> = None;
        for b in 0..self.doc.block_count() {
            if let Some(mini) = self.mini.as_ref().filter(|m| m.block == b) {
                let state = mini.state.clone();
                column.push(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .py_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(color(theme::MUTED))
                                .child("Locked block source. Escape saves."),
                        )
                        .child(
                            div()
                                .h(px(180.))
                                .border_1()
                                .border_color(color(theme::RULE))
                                .child(CodeEditor::new(&state).h_full()),
                        )
                        .into_any_element(),
                );
                continue;
            }
            let Some(block) = self.doc.block(b) else {
                continue;
            };
            let lines = block_lines(b, block);
            for line in &lines {
                if caret_row.is_none() && self.line_has_caret(line) {
                    caret_row = Some(column.len());
                }
                let el = self.render_line(line, cx);
                column.push(el);
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

    fn render_line(&mut self, line: &Line, cx: &mut Context<Self>) -> AnyElement {
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

        let content: AnyElement = if line.table {
            self.render_table_row(line, &base, cx).into_any_element()
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
        } else if let Some(seg) = line.segs.first() {
            let caret = self.caret_in(line.block, seg);
            let sel = self.sel_in(line.block, seg);
            self.seg_el(line.block, seg, &base, caret, sel, cx)
                .into_any_element()
        } else {
            div().h(size * 1.5).into_any_element()
        };

        let mut inner = div().flex_1().min_w_0().child(content);
        if matches!(
            line.deco,
            Deco::Code | Deco::Math | Deco::Front | Deco::Opaque
        ) {
            inner = inner.bg(color(theme::PANEL)).px_3();
        }
        if matches!(line.deco, Deco::Heading(_)) && line.first {
            inner = inner.pt(size * 0.6);
        }
        let marker = self.marker_el(line, cx);
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
        if line.deco == Deco::Opaque && line.first {
            return div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_sm()
                        .text_color(color(theme::MUTED))
                        .child("Locked block. Enter edits its source."),
                )
                .child(row)
                .into_any_element();
        }
        row.into_any_element()
    }

    fn marker_el(&mut self, line: &Line, cx: &mut Context<Self>) -> AnyElement {
        let Some(marker) = &line.marker else {
            return div().w(px(MARK_W)).into_any_element();
        };
        match marker {
            Marker::Bullet => div()
                .w(px(MARK_W))
                .text_color(color(theme::MUTED))
                .child("•")
                .into_any_element(),
            Marker::Number(n) => div()
                .w(px(MARK_W))
                .text_color(color(theme::MUTED))
                .child(format!("{n}."))
                .into_any_element(),
            Marker::Task { checked, index } => {
                let glyph = if *checked { "☑" } else { "☐" };
                let block = line.block;
                let index = *index;
                div()
                    .w(px(MARK_W))
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

    fn render_table_row(&mut self, line: &Line, base: &Base, cx: &mut Context<Self>) -> Div {
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
            let el = self.seg_el(line.block, seg, &cell_base, caret, sel, cx);
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

    /// One editable segment: styled text, overlay, and pointer handlers.
    fn seg_el(
        &mut self,
        block: usize,
        seg: &Seg,
        base: &Base,
        caret: Option<usize>,
        sel: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let text: SharedString = seg.text.clone().into();
        let runs = text_runs(seg, base);
        let styled = StyledText::new(text.clone()).with_runs(runs);
        let layout = styled.layout().clone();
        let links = link_ranges(seg);
        let hit = SegHit {
            block,
            start: seg.start,
            text: text.clone(),
            layout: layout.clone(),
            links: links.clone(),
        };
        self.segs.push(SegHandle {
            block,
            start: seg.start,
            text: text.clone(),
            layout: layout.clone(),
            links,
        });

        let caret_b = caret.map(|c| byte_index(&seg.text, c));
        let sel_b = sel.map(|(a, b)| (byte_index(&seg.text, a), byte_index(&seg.text, b)));
        let overlay = canvas(
            |_, _, _| (),
            move |_bounds, _, window, _cx| paint_overlay(&layout, caret_b, sel_b, window),
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
            .min_h(base.size * 1.5)
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
            if st.italic || st.math {
                font.style = FontStyle::Italic;
            }
            let mut text_color = base.color;
            let mut background = None;
            let mut underline = None;
            if st.code {
                background = Some(color(theme::PANEL));
            }
            if st.math {
                text_color = color(theme::VIOLET);
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

/// Paints selection and caret over a laid-out segment.
fn paint_overlay(
    layout: &TextLayout,
    caret: Option<usize>,
    sel: Option<(usize, usize)>,
    window: &mut Window,
) {
    let lh = layout.line_height();
    let b = layout.bounds();
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
    if let Some(c) = caret {
        let p = layout.position_for_index(c).unwrap_or(b.origin);
        window.paint_quad(fill(
            Bounds::new(p, size(px(theme::CARET_WIDTH), lh)),
            color(theme::CARET),
        ));
    }
}
