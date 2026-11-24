use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, ClickEvent, Corners, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled,
    Window, canvas, div, px, rgb,
};

use crate::focus;
use crate::icon::{IconSource, Placement, paint_icon};
use crate::input::style::{
    SPLIT_BADGE, SPLIT_BADGE_OPACITY, SPLIT_BADGE_RADIUS, SPLIT_ICON_FRACTION, SPLIT_RADIUS,
    SplitStyle,
};
use crate::text::{Align, FontSpec, LineHeight, Text, TextStyle};
use crate::theme::{Appearance, FOCUS_RING_RADIUS, FOCUS_RING_WIDTH, chains};

#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    /// `<img class="invertable">`, inverted in dark mode.
    Icon(IconSource),
    /// `<span>`: a bold number on a dark rounded badge.
    Badge(SharedString),
}

type Select = Box<dyn Fn(&usize, &mut Window, &mut App) + 'static>;

/// `div.setting-blocks div.split`: equal `inline-block` segments with 1 px borders, the outer
/// corners rounded 10 px, `selected` and `:hover` backgrounds.
#[derive(IntoElement)]
pub struct SegmentedControl {
    id: ElementId,
    segments: Vec<Segment>,
    selected: Option<usize>,
    size: (f32, f32),
    subpixel: f32,
    appearance: Appearance,
    on_select: Option<Select>,
}

/// One segment's border box in device px: `[x, y, width, height]`.
pub fn segment_boxes(count: usize, width: f32, height: f32, scale: f32) -> Vec<[f32; 4]> {
    let lu = |v: f32| (f64::from(v) * 64.0) as i64;
    let w = lu(width * scale);
    let each = if count == 1 {
        w * 3 / 5
    } else {
        w / count as i64
    };
    (0..count)
        .map(|i| {
            [
                (each * i as i64) as f32 / 64.0,
                0.0,
                each as f32 / 64.0,
                height * scale,
            ]
        })
        .collect()
}

impl SegmentedControl {
    pub fn new(id: impl Into<ElementId>, segments: Vec<Segment>) -> Self {
        SegmentedControl {
            id: id.into(),
            segments,
            selected: None,
            size: (0.0, 0.0),
            subpixel: 0.0,
            appearance: Appearance::Light,
            on_select: None,
        }
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// The `div.split` box in CSS px.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = (width, height);
        self
    }

    /// What GPUI's pixel snapping dropped from the control's x position, in CSS px: Chromium
    /// snaps each segment from its exact position.
    pub fn subpixel(mut self, dx: f32) -> Self {
        self.subpixel = dx;
        self
    }

    pub fn appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn on_select(mut self, f: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }
}

fn blend(top: u32, under: u32, alpha: f32) -> u32 {
    let mix = |shift: u32| {
        let t = ((top >> shift) & 0xff) as f32;
        let u = ((under >> shift) & 0xff) as f32;
        ((t * alpha + u * (1.0 - alpha)).round() as u32) << shift
    };
    mix(16) | mix(8) | mix(0)
}

struct State {
    hovered: Option<usize>,
    /// The segments' `tabindex="0"`.
    focus: Vec<FocusHandle>,
}

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scale = window.scale_factor();
        let style = SplitStyle::of(self.appearance);
        let count = self.segments.len();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, cx| State {
            hovered: None,
            focus: (0..count)
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
        });
        let hover = state.read(cx).hovered;
        let focus = state.read(cx).focus.clone();
        let boxes = segment_boxes(count, self.size.0, self.size.1, scale);
        let border = scale.floor().max(1.0);
        let on_select = std::rc::Rc::new(self.on_select);
        let mut root = div()
            .id(self.id)
            .relative()
            .w(px(self.size.0))
            .h(px(self.size.1));
        let shift = self.subpixel * scale;
        let mut ring = None;
        for (i, (segment, [x, _, w, h])) in self.segments.into_iter().zip(boxes).enumerate() {
            let x = x + shift;
            let left = (x + 0.5).floor();
            let right = (x + w + 0.5).floor();
            let selected = self.selected == Some(i);
            let background = if selected {
                style.selected
            } else if hover == Some(i) {
                style.hover
            } else {
                style.background
            };
            let border_color = if selected {
                style.selected_border
            } else {
                style.border
            };
            let radius = px(SPLIT_RADIUS);
            let focused = focus
                .get(i)
                .is_some_and(|handle| focus::focus_visible(handle, window, cx));
            let inner = px(if focused { FOCUS_RING_RADIUS } else { 0.0 });
            let corners = Corners {
                top_left: if i == 0 { radius } else { inner },
                bottom_left: if i == 0 { radius } else { inner },
                top_right: if i + 1 == count { radius } else { inner },
                bottom_right: if i + 1 == count { radius } else { inner },
            };
            if focused {
                let outset = px(FOCUS_RING_WIDTH);
                ring = Some(
                    div()
                        .absolute()
                        .left(px(left / scale) - outset)
                        .top(-outset)
                        .w(px((right - left) / scale) + outset * 2.0)
                        .h(px(h / scale) + outset * 2.0)
                        .border(outset)
                        .border_color(rgb(self.appearance.theme().focus_ring))
                        .rounded_tl(corners.top_left + outset)
                        .rounded_bl(corners.bottom_left + outset)
                        .rounded_tr(corners.top_right + outset)
                        .rounded_br(corners.bottom_right + outset),
                );
            }
            let (cw, ch) = (w - 2.0 * border, h - 2.0 * border);
            let content = match segment {
                Segment::Badge(label) => {
                    let text = if selected {
                        style.badge_selected_text
                    } else if hover == Some(i) {
                        style.badge_hover_text
                    } else {
                        style.badge_text
                    };
                    let badge = SPLIT_BADGE * scale;
                    let bx = x - left + ((cw - badge) / 2.0 * 64.0).trunc() / 64.0;
                    let by = ch / 2.0 - badge / 2.0;
                    let mut font_style = TextStyle::new(FontSpec {
                        weight: 700.0,
                        ..crate::input::style::system_font(11.0)
                    });
                    font_style.align = Align::Center;
                    font_style.line_height = LineHeight::Number(1.0);
                    let text_top = by + (badge - 11.0 * scale) / 2.0;
                    let snapped = text_top.round();
                    div()
                        .absolute()
                        .left(px(bx.round() / scale))
                        .top(px(by.round() / scale))
                        .w(px(SPLIT_BADGE))
                        .h(px(SPLIT_BADGE))
                        .rounded(px(SPLIT_BADGE_RADIUS))
                        .bg(rgb(blend(style.badge, background, SPLIT_BADGE_OPACITY)))
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px((snapped - by.round()) / scale))
                                .child(
                                    Text::new(label, font_style)
                                        .color(blend(text, background, SPLIT_BADGE_OPACITY))
                                        .subpixel(
                                            (bx - bx.round()) / scale,
                                            (text_top - snapped) / scale,
                                        )
                                        .w(px(SPLIT_BADGE)),
                                ),
                        )
                        .into_any_element()
                }
                Segment::Icon(source) => {
                    let lu = |v: f32| (f64::from(v) * 64.0) as i64 as f64 / 64.0;
                    let iw = lu(cw * SPLIT_ICON_FRACTION);
                    let ih = lu(ch * SPLIT_ICON_FRACTION);
                    let snap = |v: f64| (v + 0.5).floor();
                    let snap_size = |size: f64, location: f64| {
                        let fraction = location - location.trunc();
                        let n = snap(fraction + size) - snap(fraction);
                        if n == 0.0 && size.abs() > 4.0 / 64.0 {
                            size.signum()
                        } else {
                            n
                        }
                    };
                    let ix = f64::from(x - left) + lu((cw - iw as f32) / 2.0);
                    let top = lu(ch / 2.0);
                    let side = iw.min(ih);
                    let lx = ix - snap(ix) + lu(((iw - side) / 2.0) as f32);
                    let ly = top - snap(top) + lu(((ih - side) / 2.0) as f32);
                    let (left_px, top_px) = (snap(ix) + snap(lx), snap(top) + snap(ly));
                    let snapped = [snap_size(side, lx), snap_size(side, ly)];
                    let chain = if style.invert_icons {
                        chains::INVERT
                    } else {
                        ""
                    };
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            let dpr = f64::from(window.scale_factor());
                            let placement = Placement {
                                anchor: [left_px / dpr, top_px / dpr],
                                size: [side, side],
                                scale: 1.0,
                                centered: false,
                                zoom: dpr,
                                translate: [0.0, -ih / 2.0 / dpr],
                                snapped: Some(snapped),
                            };
                            let origin = [f64::from(bounds.origin.x), f64::from(bounds.origin.y)];
                            paint_icon(&source, chain, &placement, origin, window, cx);
                        },
                    )
                    .absolute()
                    .inset_0()
                    .into_any_element()
                }
            };
            let hovered = state.clone();
            let (select, keyboard) = (on_select.clone(), on_select.clone());
            root = root.child(
                div()
                    .id(("segment", i))
                    .absolute()
                    .left(px(left / scale))
                    .top_0()
                    .w(px((right - left) / scale))
                    .h(px(h / scale))
                    .bg(rgb(background))
                    .border(px(border / scale))
                    .border_color(rgb(border_color))
                    .rounded_tl(corners.top_left)
                    .rounded_bl(corners.bottom_left)
                    .rounded_tr(corners.top_right)
                    .rounded_br(corners.bottom_right)
                    .on_hover(move |entered: &bool, _, cx| {
                        hovered.update(cx, |h, cx| {
                            if *entered {
                                h.hovered = Some(i);
                            } else if h.hovered == Some(i) {
                                h.hovered = None;
                            }
                            cx.notify();
                        });
                    })
                    .when_some(focus.get(i), |segment, focus| segment.track_focus(focus))
                    .on_click(move |event: &ClickEvent, window, cx| {
                        // GPUI clicks a focused element on key up; `setupKeyboardControls`
                        // clicks on `keydown`, below.
                        if matches!(event, ClickEvent::Keyboard(_)) {
                            return;
                        }
                        if let Some(f) = select.as_ref() {
                            f(&i, window, cx);
                        }
                    })
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && let Some(f) = keyboard.as_ref()
                        {
                            f(&i, window, cx);
                        }
                    })
                    .child(content),
            );
        }
        root.children(ring)
    }
}
