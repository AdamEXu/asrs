use gpui_kit::{
    App, BorderStyle, Bounds, ContentMask, Context, Corners, DispatchPhase, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, Styled, Window,
    canvas, div, point, px, quad, rgb, size, transparent_black,
};

use crate::input::model::RangeModel;
use crate::input::style::SliderStyle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderEvent {
    /// `input`: the pointer moved the value.
    Input,
    /// `change`: the press ended on a different value than it started on.
    Change,
}

/// `LayoutSliderTrack`: the thumb's left edge in device px, the proportion of the track less
/// the thumb, truncated to a LayoutUnit.
pub fn thumb_offset(track: f32, thumb: f32, proportion: f64) -> f32 {
    let lu = |v: f32| (f64::from(v) * 64.0).round() as i64;
    let available = (lu(track) - lu(thumb)).max(0);
    (proportion * available as f64) as i64 as f32 / 64.0
}

/// `SliderThumbElement::SetPositionFromPoint`: the proportion a pointer `x` device px into the
/// track gives, with the thumb centred on it. `None` when there is no track to move along.
pub fn pointer_proportion(track: f32, thumb: f32, x: f32) -> Option<f64> {
    let lu = |v: f32| (f64::from(v) * 64.0).round();
    let available = lu(track) - lu(thumb);
    if available <= 0.0 {
        return None;
    }
    let offset = (f64::from(x) * 64.0).floor() - (lu(thumb) / 2.0).floor();
    Some(offset.clamp(0.0, available) / available)
}

/// The slider's track and thumb widths, in device px.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SliderTrack {
    pub track: f32,
    pub thumb: f32,
}

/// A range input's response to the pointer and the keyboard.
pub struct RangeControl<M: RangeModel> {
    model: M,
    disabled: bool,
    /// The value when the button went down.
    pressed: Option<String>,
}

impl<M: RangeModel> RangeControl<M> {
    pub fn new(model: M) -> Self {
        RangeControl {
            model,
            disabled: false,
            pressed: None,
        }
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    pub fn model_mut(&mut self) -> &mut M {
        &mut self.model
    }

    pub fn disabled(&self) -> bool {
        self.disabled
    }

    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
        if disabled {
            self.pressed = None;
        }
    }

    pub fn pressed(&self) -> bool {
        self.pressed.is_some()
    }

    /// A press of the main button `x` device px into the track. On the thumb it only starts the
    /// drag; beside it the thumb jumps there.
    pub fn press(&mut self, x: f32, at: SliderTrack) -> Option<SliderEvent> {
        if self.disabled {
            return None;
        }
        self.pressed = Some(self.model.value());
        let left = thumb_offset(at.track, at.thumb, self.model.proportion());
        if x >= left && x < left + at.thumb {
            return None;
        }
        self.drag(x, at)
    }

    /// The pointer moves with the button down.
    pub fn drag(&mut self, x: f32, at: SliderTrack) -> Option<SliderEvent> {
        self.pressed.as_ref()?;
        let proportion = pointer_proportion(at.track, at.thumb, x)?;
        let next = self.model.value_at(proportion);
        self.assign(&next).then_some(SliderEvent::Input)
    }

    pub fn release(&mut self) -> Option<SliderEvent> {
        let start = self.pressed.take()?;
        (start != self.model.value()).then_some(SliderEvent::Change)
    }

    /// A DOM `key` pressed with the focus on the slider; whether it changed the value.
    pub fn key_down(&mut self, key: &str) -> bool {
        if self.disabled {
            return false;
        }
        match self.model.value_for_key(key) {
            Some(next) => self.assign(&next),
            None => false,
        }
    }

    fn assign(&mut self, next: &str) -> bool {
        if next == self.model.value() {
            return false;
        }
        self.model.set_value(next);
        true
    }
}

/// An `<input type="range">` restyled with `appearance: none`: the box's background as the
/// track and a full-height thumb.
pub struct Slider<M: RangeModel> {
    control: RangeControl<M>,
    style: SliderStyle,
    clip: Option<Clip>,
    bounds: Bounds<Pixels>,
    focus: FocusHandle,
}

impl<M: RangeModel> EventEmitter<SliderEvent> for Slider<M> {}

impl<M: RangeModel> Focusable for Slider<M> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl<M: RangeModel> Slider<M> {
    pub fn new(model: M, style: SliderStyle, cx: &App) -> Self {
        Slider {
            control: RangeControl::new(model),
            style,
            clip: None,
            bounds: Bounds::default(),
            focus: cx.focus_handle().tab_stop(true),
        }
    }

    /// The rounded box that clips the slider (`div.video-timeline-container`'s `border-radius`
    /// with `overflow: hidden`): as wide as the slider and level with its top, `height` CSS px
    /// tall, so its bottom corners only reach the slider when the box ends with it.
    pub fn set_clip(&mut self, radius: f32, height: f32) {
        self.clip = (radius > 0.0).then_some(Clip { radius, height });
    }

    pub fn model(&self) -> &M {
        self.control.model()
    }

    pub fn value(&self) -> String {
        self.control.model().value()
    }

    /// `input.value = value`.
    pub fn set_value(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        self.control.model_mut().set_value(value);
        cx.notify();
    }

    pub fn model_mut(&mut self, cx: &mut Context<'_, Self>) -> &mut M {
        cx.notify();
        self.control.model_mut()
    }

    /// A control that becomes disabled loses the focus.
    pub fn set_disabled(
        &mut self,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.control.disabled() != disabled {
            self.control.set_disabled(disabled);
            if disabled && self.focus.is_focused(window) {
                window.blur(cx);
            }
            cx.notify();
        }
    }

    fn track(&self, scale: f32) -> SliderTrack {
        SliderTrack {
            track: f32::from(self.bounds.size.width) * scale,
            thumb: self.style.thumb_width * scale,
        }
    }

    /// Runs `f` with the pointer's position in device px from the track's left edge.
    fn pointer(
        &mut self,
        x: Pixels,
        window: &Window,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut RangeControl<M>, f32, SliderTrack) -> Option<SliderEvent>,
    ) {
        let scale = window.scale_factor();
        let local = f32::from(x - self.bounds.origin.x) * scale;
        let track = self.track(scale);
        let value = self.value();
        if let Some(event) = f(&mut self.control, local, track) {
            cx.emit(event);
        }
        if self.value() != value {
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<'_, Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let key = match event.keystroke.key.as_str() {
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            "pageup" => "PageUp",
            "pagedown" => "PageDown",
            "home" => "Home",
            "end" => "End",
            _ => return,
        };
        if self.control.key_down(key) {
            cx.emit(SliderEvent::Input);
            cx.emit(SliderEvent::Change);
            cx.notify();
        }
    }
}

impl<M: RangeModel> Render for Slider<M> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let style = self.style;
        let clip = self.clip;
        let proportion = self.control.model().proportion();
        let this = cx.entity();
        let painted = canvas(
            {
                let this = this.clone();
                move |bounds, _, cx| this.update(cx, |slider, _| slider.bounds = bounds)
            },
            move |bounds, _, window, _| {
                let scale = window.scale_factor();
                let thumb = style.thumb_width * scale;
                let track = f32::from(bounds.size.width) * scale;
                let x = thumb_offset(track, thumb, proportion).round() / scale;
                let thumb_bounds = Bounds {
                    origin: point(bounds.origin.x + px(x), bounds.origin.y),
                    size: size(px(style.thumb_width), bounds.size.height),
                };
                paint_clipped(bounds, bounds, clip, style.track, window);
                paint_clipped(thumb_bounds, bounds, clip, style.thumb, window);
                // A drag keeps following the pointer off the slider.
                let slider = this.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && slider.read(cx).control.pressed() {
                        slider.update(cx, |slider, cx| {
                            if e.pressed_button == Some(MouseButton::Left) {
                                slider.pointer(e.position.x, window, cx, |c, x, at| c.drag(x, at));
                            } else {
                                slider.pointer(e.position.x, window, cx, |c, _, _| c.release());
                            }
                        });
                    }
                });
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                        this.update(cx, |slider, cx| {
                            slider.pointer(e.position.x, window, cx, |c, _, _| c.release());
                        });
                    }
                });
            },
        )
        .size_full();
        let mut root = div().id("slider").size_full().child(painted);
        if !self.control.disabled() {
            root = root
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                    this.key_down(e, window, cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, window, cx| {
                        this.pointer(e.position.x, window, cx, |c, x, at| c.press(x, at));
                    }),
                );
        }
        root
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Clip {
    radius: f32,
    height: f32,
}

/// Fills `part` of the slider at `bounds` as clipped by `clip`. The clip's round rect is painted
/// whole and masked to the part, so a 4 px thumb keeps the clip's corner where a quad of its own
/// would clamp the radius to half its width.
fn paint_clipped(
    part: Bounds<Pixels>,
    bounds: Bounds<Pixels>,
    clip: Option<Clip>,
    color: u32,
    window: &mut Window,
) {
    let (shape, radius) = match clip {
        Some(clip) => (
            Bounds {
                origin: bounds.origin,
                size: size(bounds.size.width, px(clip.height)),
            },
            px(clip.radius),
        ),
        None => (part, px(0.0)),
    };
    window.with_content_mask(Some(ContentMask { bounds: part }), |window| {
        window.paint_quad(quad(
            shape,
            Corners::all(radius),
            rgb(color),
            px(0.0),
            transparent_black(),
            BorderStyle::Solid,
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thumb left edges in the `ui-inputs-hub` capture: `min="1" max="300"` at values 1, 60
    /// and 300 on a 260 px track, at 1x and 2x.
    #[test]
    fn thumb_lands_where_blink_paints_it() {
        for (scale, painted) in [(1.0, [0.0, 51.0, 256.0]), (2.0, [0.0, 101.0, 512.0])] {
            for (value, x) in [1.0, 60.0, 300.0].into_iter().zip(painted) {
                let p = (value - 1.0) / 299.0;
                assert_eq!(thumb_offset(260.0 * scale, 4.0 * scale, p).round(), x);
            }
        }
    }
}
