//! `div.setting-blocks.fix-second` in the 200 px panels of `hub.css` (Swerve and Points), and the
//! form controls the four tabs' controllers own.

use std::collections::{HashMap, HashSet};

use asrs_hub::hub::HubModel;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_tabs::form::{self, NumberInput};
use asrs_tabs::joysticks::JoysticksController;
use asrs_tabs::points::{Dimension, PointsController, PointsSelect};
use asrs_tabs::swerve::{SwerveController, SwerveInput};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Subscription, Window, div, px, rgb,
};
use asrs_ui::icon::IconSource;
use asrs_ui::input::style::CONTROL_FONT;
use asrs_ui::input::{
    FieldEvent, FieldKind, FieldStyle, Segment, SegmentedControl, Select, SelectEvent, TextField,
};
use asrs_ui::parity::probed;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, LineHeight, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY};

use super::super::frame::{FrameHost, HubFrame};
use super::super::select::FormSelect;
use super::super::sidebar::NumberValue;
use super::super::source_list::{centred_offset, place_in};
use super::{local, lu, tab_index, with_controller};

const MAX_WIDTH: f64 = 200.0;
const TOP_MARGIN: f64 = 5.0;
const TITLE_HEIGHT: f64 = 20.0;
const INPUT_MIN_HEIGHT: f64 = 40.0;
const TALL_INSET: f64 = 145.0;
const TALL_MAX_HEIGHT: f64 = 120.0;
/// `div.setting-blocks input` and `select`: `margin: 5px; width: 70%`.
const MARGIN: f64 = 5.0;
const WIDTH_FRACTION: f64 = 0.7;
const SELECT_HEIGHT: f64 = 25.0;
/// The UA's 2 px border and 1 px (block) or 2 px (inline) padding on both sides of an input.
const INPUT_CHROME: [f64; 2] = [8.0, 6.0];
const SPLIT_TOP: f64 = 8.0;
const ORIENTATION_ICONS: [&str; 2] = ["rotateleft.svg", "rotateright.svg"];

/// A form control by the controller member that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    MaxSpeed,
    SizeX,
    SizeY,
    Width,
    Height,
    Orientation,
    Origin,
    Joystick(usize),
}

impl Control {
    fn probe(self) -> &'static str {
        match self {
            Control::MaxSpeed => "input.max-speed",
            Control::SizeX => "input.size-x",
            Control::SizeY => "input.size-y",
            Control::Width => "input.dimensions-width",
            Control::Height => "input.dimensions-height",
            Control::Orientation => "select.orientation",
            Control::Origin => "select.origin",
            Control::Joystick(_) => "table.joystick-selector select",
        }
    }

    /// Runs `f` on the tab's number input, then, when `commit`, its `change` listener.
    fn number<R>(
        self,
        hub: &mut HubModel,
        index: usize,
        commit: bool,
        f: impl FnOnce(&mut NumberInput) -> R,
    ) -> Option<R> {
        let swerve = match self {
            Control::MaxSpeed => Some(SwerveInput::MaxSpeed),
            Control::SizeX => Some(SwerveInput::SizeX),
            Control::SizeY => Some(SwerveInput::SizeY),
            _ => None,
        };
        let points = match self {
            Control::Width => Some(Dimension::Width),
            Control::Height => Some(Dimension::Height),
            _ => None,
        };
        if let Some(input) = swerve {
            with_controller(hub, index, |controller: &mut SwerveController| {
                let result = f(controller.input_mut(input));
                if commit {
                    controller.input_changed(input);
                }
                result
            })
        } else {
            let dimension = points?;
            with_controller(hub, index, |controller: &mut PointsController| {
                let result = f(controller.dimension_mut(dimension));
                if commit {
                    controller.dimension_changed(dimension);
                }
                result
            })
        }
    }

    pub(super) fn select<R>(
        self,
        hub: &mut HubModel,
        index: usize,
        f: impl FnOnce(&mut form::Select) -> R,
    ) -> Option<R> {
        match self {
            Control::Orientation | Control::Origin => {
                let select = if self == Control::Orientation {
                    PointsSelect::Orientation
                } else {
                    PointsSelect::Origin
                };
                with_controller(hub, index, |controller: &mut PointsController| {
                    f(controller.select_mut(select))
                })
            }
            Control::Joystick(i) => {
                with_controller(hub, index, |controller: &mut JoysticksController| {
                    controller.select_mut(i).map(f)
                })
                .flatten()
            }
            _ => None,
        }
    }
}

struct SelectView {
    select: Entity<Select<FormSelect>>,
    synced: Option<form::Select>,
    _subscription: Subscription,
}

struct NumberView {
    field: Entity<TextField<NumberValue>>,
    _subscription: Subscription,
}

#[derive(Default)]
pub struct SettingsViews {
    numbers: HashMap<(usize, Control), NumberView>,
    selects: HashMap<(usize, Control), SelectView>,
}

impl SettingsViews {
    pub fn retain(&mut self, live: &HashSet<usize>) {
        self.numbers.retain(|(key, _), _| live.contains(key));
        self.selects.retain(|(key, _), _| live.contains(key));
    }
}

/// What an `div.input` holds.
#[derive(Clone, Copy, Debug)]
pub enum Block {
    Number(Control),
    Halves([Control; 2]),
    Select(Control),
    /// `div.orientation-switcher.split.split-2`.
    Rotate,
}

/// A panel's parity id and its three titled blocks.
pub struct Panel {
    pub probe: &'static str,
    pub blocks: [(&'static str, Block); 3],
}

/// Where a `fix-second` panel puts its parts inside `div.controls-content`, in client
/// coordinates of its unscrolled content.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PanelLayout {
    /// The border box; its first pixel column is the `border-left`.
    panel: [f64; 4],
    content_x: f64,
    content_width: f64,
    content_height: f64,
    titles: [f64; 3],
    inputs: [f64; 3],
    tall: f64,
}

impl PanelLayout {
    fn new([x, y, w, h]: [f64; 4]) -> PanelLayout {
        let content_width = math::min2(lu(w * 0.4), MAX_WIDTH);
        let panel = [x + w - content_width - 1.0, y, content_width + 1.0, h];
        let tall = math::max2(
            math::min2(math::max2(h - TALL_INSET, 0.0), TALL_MAX_HEIGHT),
            INPUT_MIN_HEIGHT,
        );
        let step = TITLE_HEIGHT + INPUT_MIN_HEIGHT;
        let titles = [0.0, 1.0, 2.0].map(|i| y + TOP_MARGIN + i * step);
        PanelLayout {
            panel,
            content_x: panel[0] + 1.0,
            content_width,
            content_height: TOP_MARGIN + 3.0 * TITLE_HEIGHT + 2.0 * INPUT_MIN_HEIGHT + tall,
            titles,
            inputs: titles.map(|t| t + TITLE_HEIGHT),
            tall,
        }
    }

    /// An inline control of border-box size `size` with its margins, centred in a column.
    fn control(&self, column: [f64; 2], top: f64, size: [f64; 2]) -> [f64; 4] {
        let [left, width] = column;
        let x = left + lu((width - size[0] - 2.0 * MARGIN) / 2.0) + MARGIN;
        [x, top + MARGIN, size[0], size[1]]
    }

    fn column(&self) -> [f64; 2] {
        [self.content_x, self.content_width]
    }

    fn halves(&self) -> [[f64; 2]; 2] {
        let half = lu(self.content_width * 0.5);
        [[self.content_x, half], [self.content_x + half, half]]
    }

    fn number(&self, column: [f64; 2], top: f64, line: f64) -> [f64; 4] {
        let width = lu(column[1] * WIDTH_FRACTION) + INPUT_CHROME[0];
        self.control(column, top, [width, line + INPUT_CHROME[1]])
    }

    fn select(&self, column: [f64; 2], top: f64) -> [f64; 4] {
        self.control(column, top, [lu(column[1] * WIDTH_FRACTION), SELECT_HEIGHT])
    }

    fn split(&self, top: f64) -> [f64; 4] {
        let width = lu(self.content_width * 0.8);
        [
            self.content_x + lu(self.content_width * 0.5) - width / 2.0,
            top + SPLIT_TOP,
            width,
            lu(self.tall * 0.65),
        ]
    }
}

fn title_style() -> TextStyle {
    let font = format!("bold 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "20px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
    .expect("settings title style")
}

/// The control font's line in an input's inner editor, in CSS px.
fn editor_line(scale: f32, cx: &mut App) -> f64 {
    let mut style = TextStyle::new(CONTROL_FONT);
    style.line_height = LineHeight::Normal;
    style.white_space = WhiteSpace::Pre;
    text::layout(cx, &style, "0", scale, None).height_px()
}

impl<H: FrameHost> HubFrame<H> {
    fn number_changed(
        &mut self,
        key: usize,
        control: Control,
        value: JsString,
        commit: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, key)?;
            control.number(hub, index, commit, |input| {
                if !commit {
                    input.set_value(value);
                }
            })
        });
        cx.notify();
    }

    /// The number input of `control` at `rect`, showing the controller's value.
    fn number_field(
        &mut self,
        key: usize,
        control: Control,
        rect: [f64; 4],
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Entity<TextField<NumberValue>>> {
        let wanted = self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, key)?;
            control.number(hub, index, false, |input| input.clone())
        })?;
        let view = self
            .canvas_tabs
            .settings
            .numbers
            .entry((key, control))
            .or_insert_with(|| {
                let field = cx.new(|cx| {
                    TextField::new(
                        NumberValue(wanted.clone()),
                        FieldKind::Number,
                        FieldStyle::setting_block(),
                        window,
                        cx,
                    )
                });
                let subscription = cx.subscribe(
                    &field,
                    move |frame: &mut Self, field, event: &FieldEvent, cx| {
                        let commit = match event {
                            FieldEvent::Input => false,
                            FieldEvent::Change => true,
                            FieldEvent::Enter | FieldEvent::Focus | FieldEvent::Blur => return,
                        };
                        let value = JsString::from(field.read(cx).value().as_str());
                        frame.number_changed(key, control, value, commit, cx);
                    },
                );
                NumberView {
                    field,
                    _subscription: subscription,
                }
            });
        let field = view.field.clone();
        let wanted = wanted.value().to_string_lossy();
        field.update(cx, |field, cx| {
            if field.value() != wanted {
                field.set_value(&wanted, window, cx);
            }
            field.set_size(rect[2] as f32, rect[3] as f32);
            field.set_subpixel(
                snap_remainder(rect[0] as f32, scale),
                snap_remainder(rect[1] as f32, scale),
            );
        });
        Some(field)
    }

    /// The select of `control` at `rect`, showing the controller's options and selection.
    pub(super) fn select_control(
        &mut self,
        key: usize,
        control: Control,
        rect: [f64; 4],
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Entity<Select<FormSelect>>> {
        let wanted = self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, key)?;
            control.select(hub, index, |select| select.clone())
        })?;
        let view = self
            .canvas_tabs
            .settings
            .selects
            .entry((key, control))
            .or_insert_with(|| {
                let select = cx.new(|cx| Select::new(FormSelect::new(wanted.clone()), window, cx));
                let subscription = cx.subscribe(
                    &select,
                    move |frame: &mut Self, select, event: &SelectEvent, cx| {
                        if *event != SelectEvent::Change {
                            return;
                        }
                        let selected = select.read(cx).model().select().selected_index();
                        frame.host_mut().input(cx, |hub| {
                            let index = tab_index(hub, key)?;
                            control.select(hub, index, |s| s.set_selected_index(selected))
                        });
                        cx.notify();
                    },
                );
                SelectView {
                    select,
                    synced: None,
                    _subscription: subscription,
                }
            });
        let changed = view.synced.as_ref().is_none_or(|s| {
            s.items() != wanted.items() || s.selected_index() != wanted.selected_index()
        });
        let select = view.select.clone();
        if changed {
            view.synced = Some(wanted.clone());
            select.update(cx, |s, cx| {
                s.update_model(|m| *m.select_mut() = wanted, cx);
            });
        }
        select.update(cx, |s, _| {
            s.set_size(rect[2] as f32, rect[3] as f32);
            s.set_subpixel(
                snap_remainder(rect[0] as f32, scale),
                snap_remainder(rect[1] as f32, scale),
            );
        });
        Some(select)
    }

    /// The panel of the tab `key` in `controls` (`div.controls-content`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn settings_panel(
        &mut self,
        key: usize,
        panel: &Panel,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let layout = PanelLayout::new(controls);
        let dark = appearance == Appearance::Dark;
        let theme = appearance.theme();
        let line = editor_line(scale, cx);
        let origin = [layout.content_x, layout.panel[1], 0.0, 0.0];
        let mut content = div()
            .relative()
            .w_full()
            .h(px(layout.content_height as f32));
        for (i, (title, block)) in panel.blocks.iter().enumerate() {
            let (width, _) = text::measure(cx, &title_style(), title, scale);
            let box_rect = [
                layout.content_x,
                layout.titles[i],
                layout.content_width,
                TITLE_HEIGHT,
            ];
            let offset = centred_offset(layout.content_width, width).unwrap_or(0.0);
            let (x, dx) = place_in(box_rect[0], offset, f64::from(scale));
            content = content.child(
                local(box_rect, origin).overflow_hidden().child(
                    div().absolute().left(px(x)).top_0().child(
                        Text::new(*title, title_style())
                            .color(theme.text)
                            .subpixel(dx as f32, 0.0),
                    ),
                ),
            );
            let top = layout.inputs[i];
            let numbers: &[(Control, [f64; 2])] = match block {
                Block::Number(control) => &[(*control, layout.column())],
                Block::Halves([a, b]) => {
                    let [left, right] = layout.halves();
                    &[(*a, left), (*b, right)]
                }
                Block::Select(_) | Block::Rotate => &[],
            };
            for (control, column) in numbers {
                let rect = layout.number(*column, top, line);
                if let Some(field) = self.number_field(key, *control, rect, scale, window, cx) {
                    let outside = field.clone();
                    let element = local(rect, origin)
                        .on_mouse_down_out(move |_, window, cx| {
                            outside.update(cx, |field, cx| field.blur(window, cx));
                        })
                        .child(field);
                    content = content.child(probed(control.probe(), element));
                }
            }
            if let Block::Select(control) = block {
                let rect = layout.select(layout.column(), top);
                if let Some(select) = self.select_control(key, *control, rect, scale, window, cx) {
                    content =
                        content.child(probed(control.probe(), local(rect, origin).child(select)));
                }
            }
            if let Block::Rotate = block {
                let rect = layout.split(top);
                content = content.child(probed(
                    "div.orientation-switcher",
                    self.rotate_switcher(key, rect, origin, appearance, scale, cx),
                ));
            }
        }
        let [_, _, _, height] = layout.panel;
        let element = local(layout.panel, controls)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(1.0))
                    .h(px(height as f32))
                    .bg(rgb(if dark { 0x000000 } else { 0xdddddd })),
            )
            .child(
                div()
                    .id((panel.probe, key))
                    .absolute()
                    .left(px(1.0))
                    .top_0()
                    .w(px(layout.content_width as f32))
                    .h(px(height as f32))
                    .overflow_y_scroll()
                    .child(content),
            );
        div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(probed(panel.probe, element))
            .into_any_element()
    }

    fn rotate_switcher(
        &self,
        key: usize,
        rect: [f64; 4],
        origin: [f64; 4],
        appearance: Appearance,
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<H> {
        let entity = cx.entity().downgrade();
        let segments = ORIENTATION_ICONS
            .iter()
            .filter_map(|name| Symbol::named(name).map(|s| Segment::Icon(IconSource::from(s))))
            .collect();
        let snapped = |v: f64| math::floor(v * f64::from(scale) + 0.5) / f64::from(scale);
        let (x, y) = (snapped(rect[0]), snapped(rect[1]));
        local([x, y, rect[2], rect[3]], origin).child(
            SegmentedControl::new(("swerve-orientation", key), segments)
                .subpixel((rect[0] - x) as f32)
                .size(rect[2] as f32, rect[3] as f32)
                .appearance(appearance)
                .on_select(move |segment, _, cx| {
                    let segment = *segment;
                    let _ = entity.update(cx, |frame, cx| {
                        frame.host_mut().input(cx, |hub| {
                            let index = tab_index(hub, key)?;
                            with_controller(hub, index, |controller: &mut SwerveController| {
                                match segment {
                                    0 => controller.rotate_left(),
                                    _ => controller.rotate_right(),
                                }
                            })
                        });
                        cx.notify();
                    });
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_boxes_match_blink() {
        let layout = PanelLayout::new([300.0, 450.0, 800.0, 200.0]);
        let [title, _, _] = layout.inputs;
        assert_eq!(layout.panel, [899.0, 450.0, 201.0, 200.0]);
        assert_eq!(
            layout.number(layout.column(), title, 15.0),
            [926.0, 480.0, 148.0, 21.0]
        );
        let [left, right] = layout.halves();
        assert_eq!(
            layout.number(left, layout.inputs[1], 15.5),
            [911.0, 540.0, 78.0, 21.5]
        );
        assert_eq!(
            layout.number(right, layout.inputs[1], 15.5),
            [1011.0, 540.0, 78.0, 21.5]
        );
        assert_eq!(layout.split(layout.inputs[2]), [920.0, 603.0, 160.0, 35.75]);
        assert_eq!(
            layout.select(layout.column(), layout.inputs[1]),
            [930.0, 540.0, 140.0, 25.0]
        );
        assert_eq!(
            layout.select(layout.column(), layout.inputs[2]),
            [930.0, 600.0, 140.0, 25.0]
        );

        let taller = PanelLayout::new([300.0, 400.0, 800.0, 250.0]);
        assert_eq!(taller.split(taller.inputs[2]), [920.0, 553.0, 160.0, 68.25]);
        let shorter = PanelLayout::new([300.0, 470.0, 800.0, 180.0]);
        assert_eq!(
            shorter.split(shorter.inputs[2]),
            [920.0, 623.0, 160.0, 26.0]
        );
        assert_eq!(shorter.content_height, 185.0);
    }
}
