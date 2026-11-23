use gpui_kit::base::input::{Input, InputEvent, InputState};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, px, rgba,
};

use crate::icon::{IconSource, Placement, paint_icon};
use crate::input::model::{FieldModel, FieldValue, number_accepts};
use crate::input::native::paint_text_field;
use crate::input::style::{FieldStyle, Frame, device_border};
use crate::symbols::Symbol;
use crate::text::{self, LineHeight, Overflow, Text, TextStyle, WhiteSpace};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Number,
    Search,
}

/// The DOM events upstream listens for on its inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    /// `input`: the user changed the text, including clearing it with the cancel button.
    Input,
    /// `change`: the value was committed by Enter or blur and differs from when focus came in.
    Change,
    /// `keydown` of Enter.
    Enter,
    Focus,
    Blur,
}

/// Where Blink lays out a single-line input's parts, in device px from the border box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldGeometry {
    pub border: f32,
    /// The inner editor: `[x, y, width, height]`.
    pub editor: [f32; 4],
    /// The cancel button of a search field, which takes its flex space even while hidden.
    pub cancel: Option<[f32; 4]>,
}

fn lu(v: f32) -> i64 {
    (f64::from(v) * 64.0).round() as i64
}

fn px_of(v: i64) -> f32 {
    v as f32 / 64.0
}

/// The parts of a `width` x `height` (CSS px) input whose inner editor is `line` device px
/// tall. The editor fills the content box less the cancel button and its UA margins (1 px
/// before, 3 px after) and is centred in it, by LayoutUnit halving, even when the content box
/// is shorter.
pub fn geometry(
    style: &FieldStyle,
    width: f32,
    height: f32,
    line: f32,
    scale: f32,
) -> FieldGeometry {
    let border = lu(match style.frame {
        Frame::Native => style.border * scale,
        Frame::Css { .. } => device_border(style.border, scale),
    });
    let [pt, pr, pb, pl] = style.padding.map(|p| lu(p * scale));
    let (w, h) = (lu(width * scale), lu(height * scale));
    let x = border + pl;
    let y = border + pt;
    let cw = w - 2 * border - pl - pr;
    let ch = h - 2 * border - pt - pb;
    let (editor_w, cancel) = if style.cancel_button {
        let em = lu(style.font.size * scale);
        let (start, end) = (lu(scale), lu(3.0 * scale));
        let editor_w = (cw - start - em - end).max(0);
        let top = y + (ch - em) / 2;
        (
            editor_w,
            Some([x + editor_w + start, top, em, em].map(px_of)),
        )
    } else {
        (cw.max(0), None)
    };
    let line = lu(line);
    FieldGeometry {
        border: px_of(border),
        editor: [x, y + (ch - line) / 2, editor_w, line].map(px_of),
        cancel,
    }
}

/// A device-px offset as a GPUI position on the device grid plus the remainder [`Text`]
/// paints from.
fn split(v: f32, scale: f32) -> (f32, f32) {
    let snapped = v.round();
    (snapped / scale, (v - snapped) / scale)
}

/// An `<input type="text|number|search">`. The editor (gpui-base's, for IME) is always
/// present and takes clicks; its glyphs show only while it has focus. Unfocused, the shown
/// text or the placeholder is painted with [`Text`], as Chromium paints it.
pub struct TextField<M: FieldModel> {
    value: FieldValue<M>,
    kind: FieldKind,
    style: FieldStyle,
    size: (f32, f32),
    subpixel: (f32, f32),
    placeholder: SharedString,
    disabled: bool,
    editor: Entity<InputState>,
    committed: String,
    hovered: bool,
    _subscription: Subscription,
}

impl<M: FieldModel> EventEmitter<FieldEvent> for TextField<M> {}

impl<M: FieldModel> Focusable for TextField<M> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl<M: FieldModel> TextField<M> {
    pub fn new(
        model: M,
        kind: FieldKind,
        style: FieldStyle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let shown = model.value();
        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx).default_value(shown.clone());
            if kind == FieldKind::Number {
                state = state.validate(|text, _| text.chars().all(number_accepts));
            }
            state
        });
        let subscription = cx.subscribe_in(&editor, window, Self::on_editor);
        TextField {
            value: FieldValue::new(model),
            kind,
            style,
            size: (0.0, 0.0),
            subpixel: (0.0, 0.0),
            placeholder: SharedString::default(),
            disabled: false,
            editor,
            committed: shown,
            hovered: false,
            _subscription: subscription,
        }
    }

    fn on_editor(
        &mut self,
        editor: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = editor.read(cx).value();
                if !self.value.edit(&text) {
                    return;
                }
                cx.emit(FieldEvent::Input);
            }
            InputEvent::PressEnter { .. } => {
                cx.emit(FieldEvent::Enter);
                self.commit(cx);
            }
            InputEvent::Focus => {
                self.committed = self.value.value();
                cx.emit(FieldEvent::Focus);
            }
            InputEvent::Blur => {
                self.commit(cx);
                cx.emit(FieldEvent::Blur);
            }
        }
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<'_, Self>) {
        if self.take_change() {
            cx.emit(FieldEvent::Change);
        }
    }

    /// Commits the value as blur does, before the blur reaches the field: returns whether the
    /// field owes its `change` event, which the caller then handles in its place.
    pub fn take_change(&mut self) -> bool {
        let value = self.value.value();
        let changed = value != self.committed;
        self.committed = value;
        changed
    }

    pub fn model(&self) -> &M {
        self.value.model()
    }

    /// `input.value`.
    pub fn value(&self) -> String {
        self.value.value()
    }

    /// `input.value = value`: the model sanitizes it and the editor shows the result.
    pub fn set_value(&mut self, value: &str, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.value.assign(value);
        let shown = self.value.shown().to_owned();
        self.committed = self.value.value();
        self.editor
            .update(cx, |editor, cx| editor.set_value(shown, window, cx));
        cx.notify();
    }

    /// The text the editor shows, which differs from `value` while it does not sanitize.
    pub fn shown(&self) -> &str {
        self.value.shown()
    }

    pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>) {
        self.placeholder = placeholder.into();
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<'_, Self>) {
        self.disabled = disabled;
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(disabled, cx));
    }

    /// The border box in CSS px, which the view lays out.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.size = (width, height);
    }

    /// What GPUI's pixel snapping dropped from the box's position; see [`Text::subpixel`].
    pub fn set_subpixel(&mut self, dx: f32, dy: f32) {
        self.subpixel = (dx, dy);
    }

    pub fn set_style(&mut self, style: FieldStyle) {
        self.style = style;
    }

    pub fn kind(&self) -> FieldKind {
        self.kind
    }

    /// `input.select()`: focus with the whole text selected.
    pub fn select(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.focus(window, cx);
            editor.select_all(window, cx);
        });
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    /// `input.blur()`.
    pub fn blur(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.focus_handle(cx).is_focused(window) {
            window.blur(cx);
            self.commit(cx);
            cx.emit(FieldEvent::Blur);
            cx.notify();
        }
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.value.edit("");
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.emit(FieldEvent::Input);
        cx.notify();
    }

    fn text_style(&self) -> TextStyle {
        let mut style = TextStyle::new(self.style.font);
        style.align = self.style.align;
        style.line_height = LineHeight::Normal;
        style.white_space = WhiteSpace::Pre;
        style.overflow = Overflow::Clip;
        style
    }
}

impl<M: FieldModel> Render for TextField<M> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let style = self.style;
        let text_style = self.text_style();
        let line = text::layout(cx, &text_style, "0", scale, None).height as f32 / 64.0;
        let g = geometry(&style, self.size.0, self.size.1, line, scale);
        let focused = self.focus_handle(cx).is_focused(window);
        let (width, height) = self.size;

        let frame = match style.frame {
            Frame::Native => div().child(
                canvas(
                    |_, _, _| {},
                    |bounds, _, window, cx| paint_text_field(bounds, window, cx),
                )
                .size_full(),
            ),
            Frame::Css {
                border,
                background,
                radius,
            } => {
                let background = match style.focus_background {
                    Some(bg) if focused => bg,
                    _ => background,
                };
                div()
                    .bg(rgba(background))
                    .border(px(g.border / scale))
                    .border_color(rgba(border))
                    .rounded(px(radius))
            }
        };

        let [ex, ey, ew, eh] = g.editor;
        let (left, dx) = split(ex, scale);
        let (top, dy) = split(ey, scale);
        let (right, _) = split(ex + ew, scale);
        let (bottom, _) = split(ey + eh, scale);
        let text_color = rgba(style.color);
        let editor_text_color: Hsla = if focused {
            text_color.into()
        } else {
            gpui_kit::transparent_black()
        };
        let mut editor_box = div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(right - left))
            .h(px(bottom - top))
            .text_size(px(style.font.size))
            .text_color(editor_text_color)
            .child(Input::new(&self.editor));
        let subpixel = (self.subpixel.0 + dx, self.subpixel.1 + dy);
        let paint = |s: &str, rgba_hex: u32| {
            Text::new(s.to_owned(), text_style)
                .form_control()
                .color(rgba_hex >> 8)
                .subpixel(subpixel.0, subpixel.1)
                .w(px(right - left))
                .h(px(bottom - top))
        };
        if self.value.shown().is_empty() && !self.placeholder.is_empty() {
            let alpha = (style.placeholder & 0xff) as f32 / 255.0 * style.placeholder_opacity;
            editor_box = editor_box.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .opacity(alpha)
                    .child(paint(&self.placeholder, style.placeholder)),
            );
        } else if !focused && !self.value.shown().is_empty() {
            let alpha = (style.color & 0xff) as f32 / 255.0;
            editor_box = editor_box.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .opacity(alpha)
                    .child(paint(self.value.shown(), style.color)),
            );
        }

        let mut root = div()
            .id("text-field")
            .relative()
            .w(px(width))
            .h(px(height))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .child(frame.absolute().inset_0())
            .child(editor_box);
        if focused {
            root = root.child(
                canvas(
                    |bounds, _, cx| {
                        crate::parity::record_mask(cx, crate::parity::FOCUSED_INPUT, bounds)
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        }

        let cancel_visible =
            (focused || self.hovered) && !self.disabled && !self.value.shown().is_empty();
        if let (Some([cx0, cy0, cw, ch]), true) = (g.cancel, cancel_visible) {
            let (cl, _) = split(cx0, scale);
            let (ct, _) = split(cy0, scale);
            let filter = style.cancel_filter;
            root = root.child(
                div()
                    .id("cancel")
                    .absolute()
                    .left(px(cl))
                    .top(px(ct))
                    .w(px(cw / scale))
                    .h(px(ch / scale))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx)))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                let dpr = f64::from(window.scale_factor());
                                let side = f64::from(bounds.size.width) * dpr;
                                let placement = Placement {
                                    anchor: [0.0, 0.0],
                                    size: [side, side],
                                    scale: 1.0,
                                    centered: false,
                                    zoom: dpr,
                                    translate: [0.0, 0.0],
                                    snapped: None,
                                };
                                let origin =
                                    [f64::from(bounds.origin.x), f64::from(bounds.origin.y)];
                                let cancel = IconSource::Symbol(
                                    Symbol::named("cancel.svg").expect("cancel symbol"),
                                );
                                paint_icon(&cancel, filter, &placement, origin, window, cx);
                            },
                        )
                        .size_full(),
                    ),
            );
        }
        root
    }
}
