use gpui_kit::{
    App, Corners, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, px, rgb,
};

use crate::icon::{IconSource, Placement, paint_icon};
use crate::input::style::{
    SPLIT_BADGE, SPLIT_BADGE_OPACITY, SPLIT_BADGE_RADIUS, SPLIT_ICON_FRACTION, SPLIT_RADIUS,
    SplitStyle,
};
use crate::text::{Align, FontSpec, LineHeight, Text, TextStyle};
use crate::theme::{Appearance, chains};

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

impl RenderOnce for SegmentedControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scale = window.scale_factor();
        let style = SplitStyle::of(self.appearance);
        let hovered = window.use_keyed_state(self.id.clone(), cx, |_, _| None::<usize>);
        let hover = *hovered.read(cx);
        let count = self.segments.len();
        let boxes = segment_boxes(count, self.size.0, self.size.1, scale);
        let border = scale.floor().max(1.0);
        let on_select = std::rc::Rc::new(self.on_select);
        let mut root = div()
            .id(self.id)
            .relative()
            .w(px(self.size.0))
            .h(px(self.size.1));
        for (i, (segment, [x, _, w, h])) in self.segments.into_iter().zip(boxes).enumerate() {
            let left = x.round();
            let right = (x + w).round();
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
            let corners = Corners {
                top_left: if i == 0 { radius } else { px(0.0) },
                bottom_left: if i == 0 { radius } else { px(0.0) },
                top_right: if i + 1 == count { radius } else { px(0.0) },
                bottom_right: if i + 1 == count { radius } else { px(0.0) },
            };
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
                    let bx = ((cw - badge) / 2.0 * 64.0).trunc() / 64.0;
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
                        .left(px((border + bx.round()) / scale))
                        .top(px((border + by.round()) / scale))
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
                    let ix = lu((cw - iw as f32) / 2.0);
                    let top = lu(ch / 2.0);
                    let chain = if style.invert_icons { chains::INVERT } else { "" };
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, cx| {
                            let dpr = f64::from(window.scale_factor());
                            let placement = Placement {
                                anchor: [(ix + iw / 2.0) / dpr, top / dpr],
                                size: [iw, ih],
                                scale: 1.0,
                                centered: true,
                                zoom: dpr,
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
            let hovered = hovered.clone();
            let select = on_select.clone();
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
                                *h = Some(i);
                            } else if *h == Some(i) {
                                *h = None;
                            }
                            cx.notify();
                        });
                    })
                    .on_click(move |_, window, cx| {
                        if let Some(f) = select.as_ref() {
                            f(&i, window, cx);
                        }
                    })
                    .child(content),
            );
        }
        root
    }
}
