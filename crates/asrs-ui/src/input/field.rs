use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::input::{
    Cut, Delete, Input, InputEvent, InputState, MoveDown, MoveUp, Paste, Position,
};
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, ClipboardItem, Context, DispatchPhase, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    WeakEntity, Window, canvas, div, px, rgba,
};

use crate::focus;
use crate::icon::{IconSource, Placement, paint_icon};
use crate::input::DeleteSelection;
use crate::input::model::{FieldModel, FieldValue};
use crate::input::native::{PointerState, paint_text_field};
use crate::input::spin::{self, paint_spin_button};
use crate::input::style::{DISABLED_TEXT, FieldStyle, Frame, device_border};
use crate::platform;
use crate::selection;
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
    /// The spin button of a number field, which takes its flex space even while hidden.
    pub spin: Option<[f32; 4]>,
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
    kind: FieldKind,
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
    let spins = kind == FieldKind::Number && style.spin_button;
    let spin_w = if spins { lu(spin::WIDTH * scale) } else { 0 };
    let spin = spins.then(|| [x + cw - spin_w, y, spin_w, ch].map(px_of));
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
        ((cw - spin_w).max(0), None)
    };
    let line = lu(line);
    FieldGeometry {
        border: px_of(border),
        editor: [x, y + (ch - line) / 2, editor_w, line].map(px_of),
        cancel,
        spin,
    }
}

/// A device-px offset as a GPUI position on the device grid plus the remainder [`Text`]
/// paints from.
fn split(v: f32, scale: f32) -> (f32, f32) {
    let snapped = v.round();
    (snapped / scale, (v - snapped) / scale)
}

/// What an edit from `old` to `new` replaced: the range of `old`, and what it is now.
fn edited<'a>(old: &str, new: &'a str) -> (Range<usize>, &'a str) {
    let start: usize = (old.chars().zip(new.chars()))
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let tail: usize = (old[start..].chars().rev().zip(new[start..].chars().rev()))
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (start..old.len() - tail, &new[start..new.len() - tail])
}

/// The editor refuses an edit that inserts more than [`FieldModel::insertable`] and, once the
/// refused edit has ended, inserts only that. `text` is what the editor shows.
fn filter_insertions<M: FieldModel>(
    text: Rc<RefCell<String>>,
    editor: WeakEntity<InputState>,
    window: AnyWindowHandle,
) -> impl Fn(&str, &mut App) -> bool + 'static {
    move |new, cx| {
        let old = text.borrow().clone();
        let (range, inserted) = edited(&old, new);
        if M::insertable(&old[..range.start], inserted, &old[range.end..]) == inserted {
            return true;
        }
        let (text, editor, new) = (text.clone(), editor.clone(), new.to_owned());
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    insert_filtered::<M>(&text, &new, editor, window, cx);
                })
            });
        });
        false
    }
}

fn insert_filtered<M: FieldModel>(
    text: &RefCell<String>,
    new: &str,
    editor: &mut InputState,
    window: &mut Window,
    cx: &mut Context<'_, InputState>,
) {
    let old = editor.value();
    let selected = editor.selected_range();
    let (left, right) = (&old[..selected.start], &old[selected.end..]);
    let (range, inserted) = match new.strip_prefix(left).and_then(|s| s.strip_suffix(right)) {
        Some(inserted) => (selected, inserted),
        None => edited(&old, new),
    };
    insert::<M>(text, range, inserted, editor, window, cx);
}

fn insert<M: FieldModel>(
    text: &RefCell<String>,
    range: Range<usize>,
    inserted: &str,
    editor: &mut InputState,
    window: &mut Window,
    cx: &mut Context<'_, InputState>,
) {
    let old = editor.value();
    let (left, right) = (&old[..range.start], &old[range.end..]);
    let kept = M::insertable(left, inserted, right);
    if kept.is_empty() && range.is_empty() {
        return;
    }
    *text.borrow_mut() = format!("{left}{kept}{right}");
    let utf16 = |s: &str| s.encode_utf16().count();
    let start = utf16(left);
    let end = start + utf16(&old[range]);
    editor.replace_text_in_range(Some(start..end), &kept, window, cx);
}

/// `SpinButtonElement`'s state.
#[derive(Default)]
struct Spin {
    /// Which half the pointer was last over.
    up: bool,
    hovered: bool,
    pressed: bool,
    repeat: Option<Task<()>>,
}

impl Spin {
    /// `ReleaseCapture`: whether it was pressed.
    fn release(&mut self) -> bool {
        self.repeat = None;
        std::mem::take(&mut self.pressed)
    }
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
    editor_text: Rc<RefCell<String>>,
    committed: String,
    hovered: bool,
    pressed: bool,
    spin: Spin,
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
        let editor_text = Rc::new(RefCell::new(shown.clone()));
        let editor = cx.new(|cx| {
            let filter = filter_insertions::<M>(
                editor_text.clone(),
                cx.weak_entity(),
                window.window_handle(),
            );
            InputState::new(window, cx)
                .default_value(shown.clone())
                .validate(filter)
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
            editor_text,
            committed: shown,
            hovered: false,
            pressed: false,
            spin: Spin::default(),
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
                *self.editor_text.borrow_mut() = text.to_string();
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
                self.spin.release();
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

    /// Changes attributes of the element, which fires no events.
    pub fn update_model<R>(&mut self, f: impl FnOnce(&mut M) -> R) -> R {
        f(self.value.model_mut())
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
        self.editor_text.borrow_mut().clone_from(&shown);
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

    /// The selected text of its editor.
    pub fn selected_text(&self, cx: &App) -> String {
        let editor = self.editor.read(cx);
        editor.value()[editor.selected_range()].to_owned()
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
    }

    /// `input.blur()`.
    pub fn blur(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.focus_handle(cx).is_focused(window) {
            window.blur(cx);
            self.spin.release();
            self.commit(cx);
            cx.emit(FieldEvent::Blur);
            cx.notify();
        }
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.value.edit("");
        self.editor_text.borrow_mut().clear();
        self.editor
            .update(cx, |editor, cx| editor.set_value("", window, cx));
        cx.emit(FieldEvent::Input);
        cx.notify();
    }

    /// The editor itself drops the line breaks of a paste before [`FieldModel::insertable`] can
    /// see them.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<'_, Self>) {
        let pasted = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(pasted) = pasted.filter(|text| text.contains(['\n', '\r'])) else {
            return;
        };
        cx.stop_propagation();
        if self.disabled {
            return;
        }
        let text = self.editor_text.clone();
        self.editor.update(cx, |editor, cx| {
            let selected = editor.selected_range();
            insert::<M>(&text, selected, &pasted, editor, window, cx);
        });
    }

    /// The Edit menu's Copy; the editor handles the keys.
    fn copy(&mut self, _: &selection::Copy, _: &mut Window, cx: &mut Context<'_, Self>) {
        let editor = self.editor.read(cx);
        let selected = editor.selected_range();
        let text = editor.value();
        if !selected.is_empty() {
            let copied = ClipboardItem::new_string(text[selected].to_owned());
            cx.write_to_clipboard(copied);
        }
    }

    /// The editor's own cut of a caret empties the clipboard.
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<'_, Self>) {
        if self.editor.read(cx).selected_range().is_empty() {
            cx.stop_propagation();
        }
    }

    fn delete_selection(
        &mut self,
        _: &DeleteSelection,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.editor.read(cx).selected_range().is_empty() {
            window.dispatch_action(Box::new(Delete), cx);
        }
    }

    /// `HandleKeydownEventForSpinButton`.
    fn arrow_key(&mut self, up: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.kind != FieldKind::Number {
            return;
        }
        cx.stop_propagation();
        if self.disabled || !self.value.arrow_key(up) {
            return;
        }
        let shown = self.value.shown().to_owned();
        self.committed = self.value.value();
        self.editor_text.borrow_mut().clone_from(&shown);
        self.editor
            .update(cx, |editor, cx| editor.set_value(shown, window, cx));
        cx.emit(FieldEvent::Input);
        cx.emit(FieldEvent::Change);
        cx.notify();
    }

    /// `SpinButtonStepUp` and `SpinButtonStepDown`.
    fn spin_step(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.disabled || !self.value.arrow_key(self.spin.up) {
            return;
        }
        let shown = self.value.shown().to_owned();
        self.editor_text.borrow_mut().clone_from(&shown);
        self.editor
            .update(cx, |editor, cx| editor.set_value(shown, window, cx));
        cx.emit(FieldEvent::Input);
        cx.notify();
    }

    fn spin_press(&mut self, up: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.select(window, cx);
        self.spin.up = up;
        self.spin.pressed = true;
        let (initial, period) = platform::scroll_button_delays();
        self.spin.repeat = Some(cx.spawn_in(window, async move |field, cx| {
            let mut delay = initial;
            loop {
                cx.background_executor().timer(delay).await;
                delay = period;
                let stepped = field.update_in(cx, |field, window, cx| field.spin_step(window, cx));
                if stepped.is_err() {
                    return;
                }
            }
        }));
        self.spin_step(window, cx);
        cx.notify();
    }

    fn spin_release(&mut self, cx: &mut Context<'_, Self>) {
        if self.spin.release() {
            self.commit(cx);
            cx.notify();
        }
    }

    /// The spin button in the box `[x, y, width, height]` (device px), which the pointer keeps
    /// turning after a press on it wherever it goes.
    fn spin_button(
        &self,
        [x, y, w, h]: [f32; 4],
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let (left, _) = split(x, scale);
        let (top, _) = split(y, scale);
        let (right, _) = split(x + w, scale);
        let (bottom, _) = split(y + h, scale);
        let half = px(h / scale / 2.0);
        let (up, pressed) = (self.spin.up, self.spin.pressed);
        let pointer = match (pressed, self.spin.hovered) {
            (true, _) => PointerState::Pressed,
            (false, true) => PointerState::Hovered,
            (false, false) => PointerState::Normal,
        };
        let bounds = Rc::new(std::cell::Cell::new(None::<Bounds<Pixels>>));
        let placed = bounds.clone();
        let above = move |position: Point<Pixels>| {
            bounds.get().is_some_and(|b| position.y - b.top() < half)
        };
        let (on_move, on_press) = (above.clone(), above.clone());
        let field = cx.weak_entity();
        div()
            .id("spin-button")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(right - left))
            .h(px(bottom - top))
            .cursor_default()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.spin.hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                this.spin.up = on_move(event.position);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.pressed = true;
                    this.spin_press(on_press(event.position), window, cx);
                }),
            )
            .child(
                canvas(
                    move |bounds, _, _| placed.set(Some(bounds)),
                    move |bounds, _, window, cx| {
                        paint_spin_button(bounds, up, pointer, window, cx);
                        if !pressed {
                            return;
                        }
                        let moved = field.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let up = above(event.position);
                                let _ = moved.update(cx, |this, cx| {
                                    this.spin.up = up;
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                let _ = field.update(cx, |this, cx| this.spin_release(cx));
                            }
                        });
                    },
                )
                .size_full(),
            )
    }

    /// The place of a hidden spin button, where a press reaches the input after its text.
    fn hidden_spin_button(
        &self,
        [x, y, w, h]: [f32; 4],
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let [(left, _), (top, _)] = [x, y].map(|v| split(v, scale));
        let [(right, _), (bottom, _)] = [x + w, y + h].map(|v| split(v, scale));
        div()
            .id("hidden-spin-button")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(right - left))
            .h(px(bottom - top))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.disabled {
                        return;
                    }
                    window.prevent_default();
                    let end = this.value.shown().encode_utf16().count() as u32;
                    this.editor.update(cx, |editor, cx| {
                        editor.set_cursor_position(Position::new(0, end), window, cx);
                    });
                }),
            )
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
        let g = geometry(&style, self.kind, self.size.0, self.size.1, line, scale);
        let handle = self.focus_handle(cx);
        let focused = handle.is_focused(window);
        let has_focus = focus::has_focus(&handle, window, cx);
        let (width, height) = self.size;
        let disabled = self.disabled;
        let color = match style.frame {
            Frame::Native if disabled => DISABLED_TEXT,
            _ => style.color,
        };

        let pointer = match (self.pressed, self.hovered) {
            (true, _) => PointerState::Pressed,
            (false, true) => PointerState::Hovered,
            (false, false) => PointerState::Normal,
        };
        let frame = match style.frame {
            Frame::Native => div().child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        paint_text_field(bounds, disabled, pointer, window, cx);
                    },
                )
                .size_full(),
            ),
            Frame::Css {
                border,
                background,
                radius,
            } => {
                let background = match style.focus_background {
                    Some(bg) if has_focus => bg,
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
        let text_color = rgba(color);
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
        let page_selected = selection::all::colors(window, cx);
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
            let alpha = (color & 0xff) as f32 / 255.0;
            editor_box = editor_box.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .opacity(alpha)
                    .child(paint(self.value.shown(), color).selected(page_selected)),
            );
        }

        let mut root = div()
            .id("text-field")
            .key_context(super::CONTEXT)
            .capture_action(cx.listener(Self::paste))
            .capture_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::delete_selection))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(|this, _: &selection::SelectAll, window, cx| {
                let editor = this.editor.clone();
                editor.update(cx, |editor, cx| editor.select_all(window, cx));
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                this.arrow_key(true, window, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                this.arrow_key(false, window, cx);
            }))
            .relative()
            .w(px(width))
            .h(px(height))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                this.spin.hovered &= *hovered;
                cx.notify();
            }))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left {
                    this.pressed = true;
                    cx.notify();
                }
            }))
            .child(frame.absolute().inset_0())
            .child(editor_box);
        if self.pressed {
            let field = cx.weak_entity();
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = field.update(cx, |this, cx| {
                                    this.pressed = false;
                                    cx.notify();
                                });
                            }
                        });
                    },
                )
                .absolute(),
            );
        }
        if let Some(spin) = g.spin {
            let shown = (has_focus || self.hovered || self.spin.pressed) && !disabled;
            root = root.child(if shown {
                self.spin_button(spin, scale, cx).into_any_element()
            } else {
                self.hidden_spin_button(spin, scale, cx).into_any_element()
            });
        }
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
            (has_focus || self.hovered) && !self.disabled && !self.value.shown().is_empty();
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
