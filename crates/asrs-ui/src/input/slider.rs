use gpui_kit::{
    BorderStyle, Bounds, Context, Corners, EventEmitter, InteractiveElement, IntoElement, MouseButton,
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
/// track gives, with the thumb centred on it.
pub fn pointer_proportion(track: f32, thumb: f32, x: f32) -> f64 {
    let available = track - thumb;
    if available <= 0.0 {
        return 0.0;
    }
    f64::from(((x - thumb / 2.0) / available).clamp(0.0, 1.0))
}

/// An `<input type="range">` restyled with `appearance: none`: the box's background as the
/// track and a full-height thumb.
pub struct Slider<M: RangeModel> {
    model: M,
    style: SliderStyle,
    corners: Corners<Pixels>,
    disabled: bool,
    bounds: Bounds<Pixels>,
    pressed: Option<String>,
}

impl<M: RangeModel> EventEmitter<SliderEvent> for Slider<M> {}

impl<M: RangeModel> Slider<M> {
    pub fn new(model: M, style: SliderStyle) -> Self {
        Slider {
            model,
            style,
            corners: Corners::default(),
            disabled: false,
            bounds: Bounds::default(),
            pressed: None,
        }
    }

    /// The rounded clip of the box the slider fills (`div.video-timeline-container`'s
    /// `border-radius` with `overflow: hidden`), applied to the track and to the thumb where it
    /// meets a corner.
    pub fn set_corner_radii(&mut self, corners: Corners<Pixels>) {
        self.corners = corners;
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    pub fn value(&self) -> String {
        self.model.value()
    }

    /// `input.value = value`.
    pub fn set_value(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        self.model.set_value(value);
        cx.notify();
    }

    pub fn model_mut(&mut self, cx: &mut Context<'_, Self>) -> &mut M {
        cx.notify();
        &mut self.model
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<'_, Self>) {
        self.disabled = disabled;
        cx.notify();
    }

    fn drag_to(&mut self, x: Pixels, window: &Window, cx: &mut Context<'_, Self>) {
        let scale = window.scale_factor();
        let local = f32::from(x - self.bounds.origin.x) * scale;
        let track = f32::from(self.bounds.size.width) * scale;
        let p = pointer_proportion(track, self.style.thumb_width * scale, local);
        let next = self.model.value_at(p);
        if next != self.model.value() {
            self.model.set_value(&next);
            cx.emit(SliderEvent::Input);
            cx.notify();
        }
    }
}

impl<M: RangeModel> Render for Slider<M> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let style = self.style;
        let corners = self.corners;
        let proportion = self.model.proportion();
        let this = cx.entity();
        let mut root = div().id("slider").size_full().child(
            canvas(
                move |bounds, _, cx| this.update(cx, |slider, _| slider.bounds = bounds),
                move |bounds, _, window, _| {
                    let scale = window.scale_factor();
                    window.paint_quad(quad(
                        bounds,
                        corners,
                        rgb(style.track),
                        px(0.0),
                        transparent_black(),
                        BorderStyle::Solid,
                    ));
                    let thumb = style.thumb_width * scale;
                    let track = f32::from(bounds.size.width) * scale;
                    let x = thumb_offset(track, thumb, proportion).round() / scale;
                    let at_left = px(x) < corners.top_left.max(corners.bottom_left);
                    let at_right = px(x + style.thumb_width)
                        > bounds.size.width - corners.top_right.max(corners.bottom_right);
                    let zero = px(0.0);
                    window.paint_quad(quad(
                        Bounds {
                            origin: point(bounds.origin.x + px(x), bounds.origin.y),
                            size: size(px(style.thumb_width), bounds.size.height),
                        },
                        Corners {
                            top_left: if at_left { corners.top_left } else { zero },
                            bottom_left: if at_left { corners.bottom_left } else { zero },
                            top_right: if at_right { corners.top_right } else { zero },
                            bottom_right: if at_right { corners.bottom_right } else { zero },
                        },
                        rgb(style.thumb),
                        px(0.0),
                        transparent_black(),
                        BorderStyle::Solid,
                    ));
                },
            )
            .size_full(),
        );
        if !self.disabled {
            root = root
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, window, cx| {
                        this.pressed = Some(this.model.value());
                        this.drag_to(e.position.x, window, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, window, cx| {
                    if this.pressed.is_some() && e.pressed_button == Some(MouseButton::Left) {
                        this.drag_to(e.position.x, window, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if let Some(start) = this.pressed.take()
                            && start != this.model.value()
                        {
                            cx.emit(SliderEvent::Change);
                        }
                    }),
                );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_offset_truncates_to_layout_units() {
        assert_eq!(thumb_offset(260.0, 4.0, 0.0), 0.0);
        assert_eq!(thumb_offset(260.0, 4.0, 1.0), 256.0);
        assert_eq!(thumb_offset(260.0, 4.0, 59.0 / 299.0), 50.515625);
    }

    #[test]
    fn pointer_centres_the_thumb() {
        assert_eq!(pointer_proportion(260.0, 4.0, 2.0), 0.0);
        assert_eq!(pointer_proportion(260.0, 4.0, 258.0), 1.0);
        assert_eq!(pointer_proportion(260.0, 4.0, 130.0), 0.5);
    }
}
