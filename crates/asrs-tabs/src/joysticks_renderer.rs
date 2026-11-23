//! `JoysticksRenderer.ts`: up to six joysticks, each drawn on its layout's image or as a grid
//! of generic controls.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use asrs_canvas::Canvas2d;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_js::{math, number, ops};
use asrs_log::util::scale_value;
use asrs_model::assets::{
    AdvantageScopeAssets, ConfigJoystick, ConfigJoystickAxis, ConfigJoystickButton,
    ConfigJoystickComponent, ConfigJoystickStick, ConfigJoystickTouchpad, SourcePov,
};
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use crate::canvas_painter::{arc, array, member, operand, state_changed, unmodeled};
use crate::list_cx::SourceListCx;
use crate::mechanism_renderer::for_each_items;

const YELLOW_COLOR: &str = "#ffff00";
const BLACK_COLOR: &str = "#222222";
const WHITE_COLOR: &str = "#eeeeee";
const IMAGE_COUNT: usize = 6;

/// The DOM the renderer reads and the frame it painted, shared with the host.
#[derive(Debug, Default)]
pub struct JoysticksDom {
    /// `CANVAS.clientWidth` and `clientHeight`.
    pub client_size: [f64; 2],
    /// `window.devicePixelRatio`.
    pub device_pixel_ratio: f64,
    /// Natural `[width, height]` per layout `path`; a missing entry is a broken image.
    pub images: HashMap<JsString, [f64; 2]>,
    /// The last painted frame, until the host takes it. It draws each image under its path.
    pub painted: Option<Canvas2d>,
}

pub struct JoysticksRenderer {
    dom: Rc<RefCell<JoysticksDom>>,
    last_render_state: String,
    image_load_count: u64,
    pending_image_loads: VecDeque<usize>,
}

impl JoysticksRenderer {
    pub fn new(dom: Rc<RefCell<JoysticksDom>>) -> JoysticksRenderer {
        JoysticksRenderer {
            dom,
            last_render_state: String::new(),
            image_load_count: 0,
            pending_image_loads: VecDeque::new(),
        }
    }

    pub fn dom(&self) -> &Rc<RefCell<JoysticksDom>> {
        &self.dom
    }

    pub fn image_load_count(&self) -> u64 {
        self.image_load_count
    }

    /// Delivers the oldest queued `load` event of `IMAGES` and returns its index. The host
    /// delivers them all between two animation frames.
    pub fn deliver_image_load(&mut self) -> Option<usize> {
        let index = self.pending_image_loads.pop_front()?;
        self.image_load_count += 1;
        Some(index)
    }

    /// `render(command)` in the colour scheme `dark`: when the render state changed, paints a
    /// new frame into `dom.painted`. A throw leaves the frame as far as it was painted.
    pub fn render_command(
        &mut self,
        dark: bool,
        assets: Option<&AdvantageScopeAssets>,
        command: &JsValue,
    ) -> Result<(), JsError> {
        let dom = self.dom.clone();
        let mut dom = dom.borrow_mut();
        let JoysticksDom {
            client_size: [width, height],
            device_pixel_ratio: dpr,
            images,
            painted,
        } = &mut *dom;
        let (width, height, dpr) = (*width, *height, *dpr);
        let is_light = !dark;
        let render_state = JsValue::Array(vec![
            JsValue::Number(width),
            JsValue::Number(height),
            JsValue::Bool(is_light),
            JsValue::Number(dpr),
            command.clone(),
            JsValue::Number(self.image_load_count as f64),
        ]);
        if !state_changed(&mut self.last_render_state, &render_state)? {
            return Ok(());
        }

        let context = painted.insert(Canvas2d::new(
            math::to_uint32(width * dpr),
            math::to_uint32(height * dpr),
        ));
        context.scale(dpr, dpr);
        context.clear_rect(0.0, 0.0, width, height);

        let joysticks = for_each_items(command, "command")?;
        for (index, joystick) in joysticks.iter().enumerate() {
            let config = match assets {
                Some(assets) => find_config(assets, joystick)?,
                None => None,
            };
            let mut image_size = [f64::NAN; 2];
            if let Some(config) = config {
                if index >= IMAGE_COUNT {
                    return Err(ops::get(&JsValue::Undefined, "src")
                        .expect_err("reading a property of undefined throws"));
                }
                image_size = images.get(&config.path).copied().unwrap_or_default();
                // `src` reads back as a URL, which never equals the file path the asset loader
                // stamps, so upstream assigns it on every painted frame and loads it again.
                if images.contains_key(&config.path) {
                    self.pending_image_loads.push_back(index);
                }
                if !(image_size[0] > 0.0 && image_size[1] > 0.0) {
                    continue;
                }
            }
            let Some(region) = region(joysticks.len(), index, width, height) else {
                continue;
            };
            let state = member(joystick, "state");
            let background = if config.is_some() {
                image_size
            } else {
                generic_background(state.clone()?)?
            };
            let mut painter = Painter {
                context,
                is_light,
                background,
                area: fit(background, region),
            };
            match config {
                Some(config) => painter.layout(config, state)?,
                None => painter.generic(state?)?,
            }
        }

        if joysticks.is_empty() {
            message(
                context,
                is_light,
                "Select a joystick layout to view data.",
                [width / 2.0, height / 2.0],
            );
        }
        Ok(())
    }
}

/// `window.assets.joysticks.find((config) => config.name === joystick.layout)`.
fn find_config<'a>(
    assets: &'a AdvantageScopeAssets,
    joystick: &JsValue,
) -> Result<Option<&'a ConfigJoystick>, JsError> {
    for config in &assets.joysticks {
        if matches!(member(joystick, "layout")?, JsValue::String(name) if *name == config.name) {
            return Ok(Some(config));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

/// The region of joystick `index` of `count`, or `None` for a count upstream has no case for.
fn region(count: usize, index: usize, canvas_width: f64, canvas_height: f64) -> Option<Rect> {
    let (columns, column, rows, row) = match count {
        1 => (1, 0, 1, 0),
        2 => (2, index, 1, 0),
        3 if index < 2 => (2, index, 2, 0),
        3 => (1, 0, 2, 1),
        4 if index < 2 => (2, index, 2, 0),
        4 => (2, index - 2, 2, 1),
        5 if index < 2 => (2, index, 2, 0),
        5 => (3, index - 2, 2, 1),
        6 if index < 3 => (3, index, 2, 0),
        6 => (3, index - 3, 2, 1),
        _ => return None,
    };
    let width = canvas_width / f64::from(columns);
    let height = canvas_height / f64::from(rows);
    Some(Rect {
        left: width * column as f64,
        top: height * f64::from(row),
        width,
        height,
    })
}

/// The largest box with the aspect ratio of `background` centered in `region`.
fn fit(background: [f64; 2], region: Rect) -> Rect {
    let source_aspect_ratio = background[0] / background[1];
    let target_aspect_ratio = region.width / region.height;
    if source_aspect_ratio > target_aspect_ratio {
        Rect {
            left: region.left,
            top: region.top + region.height / 2.0 - region.width / source_aspect_ratio / 2.0,
            width: region.width,
            height: region.width / source_aspect_ratio,
        }
    } else {
        Rect {
            left: region.left + region.width / 2.0 - (region.height * source_aspect_ratio) / 2.0,
            top: region.top,
            width: region.height * source_aspect_ratio,
            height: region.height,
        }
    }
}

fn rows_of(count: usize) -> f64 {
    math::ceil(count as f64 / 6.0)
}

fn generic_background(state: &JsValue) -> Result<[f64; 2], JsError> {
    let mut lengths = [0; 4];
    for (length, key) in lengths
        .iter_mut()
        .zip(["buttons", "axes", "povs", "touchpads"])
    {
        *length = array(member(state, key)?, "length")?.len();
    }
    let row_count: f64 = lengths.iter().map(|n| rows_of(*n)).sum();
    let [buttons, axes, povs, touchpads] = lengths;
    let column_count = [
        buttons.min(6),
        axes.min(6),
        povs.min(6),
        (touchpads * 2).min(6),
        1,
    ]
    .into_iter()
    .max()
    .unwrap_or(1);
    Ok([40.0 + column_count as f64 * 100.0, 40.0 + row_count * 100.0])
}

fn message(context: &mut Canvas2d, is_light: bool, text: &str, [x, y]: [f64; 2]) {
    context.set_font("italic 16px sans-serif");
    context.set_text_align("center");
    context.set_text_baseline("middle");
    context.set_fill_style(if is_light { BLACK_COLOR } else { WHITE_COLOR });
    context.fill_text(text, x, y);
}

/// `items.find((item) => item.id === id)`.
fn find_id(items: &JsValue, id: f64) -> Result<Option<&JsValue>, JsError> {
    for item in array(items, "find")? {
        if ops::strict_equals(member(item, "id")?, &JsValue::Number(id)) {
            return Ok(Some(item));
        }
    }
    Ok(None)
}

/// `id.toString()`.
fn id_text(item: &JsValue) -> Result<JsString, JsError> {
    match member(item, "id")? {
        JsValue::Number(id) => Ok(JsString::from(number::to_string(*id))),
        other => match ops::get(other, "toString") {
            Err(e) => Err(e),
            Ok(_) => Err(unmodeled(format!("id {other:?}"))),
        },
    }
}

fn pov_active(angle: &JsValue, direction: SourcePov) -> bool {
    let angles = match direction {
        SourcePov::Up => [315.0, 0.0, 45.0],
        SourcePov::Right => [45.0, 90.0, 135.0],
        SourcePov::Down => [135.0, 180.0, 225.0],
        SourcePov::Left => [225.0, 270.0, 315.0],
    };
    angles
        .into_iter()
        .any(|a| ops::strict_equals(angle, &JsValue::Number(a)))
}

struct Painter<'a> {
    context: &'a mut Canvas2d,
    is_light: bool,
    /// `[backgroundWidth, backgroundHeight]`, the space component positions are in.
    background: [f64; 2],
    /// Where the background lands on the canvas.
    area: Rect,
}

impl Painter<'_> {
    fn color(&self, is_yellow: bool) -> &'static str {
        if is_yellow {
            YELLOW_COLOR
        } else if self.is_light {
            BLACK_COLOR
        } else {
            WHITE_COLOR
        }
    }

    fn x(&self, x: f64) -> f64 {
        let Rect { left, width, .. } = self.area;
        scale_value(x, [0.0, self.background[0]], [left, left + width])
    }

    fn y(&self, y: f64) -> f64 {
        let Rect { top, height, .. } = self.area;
        scale_value(y, [0.0, self.background[1]], [top, top + height])
    }

    fn width(&self, width: f64) -> f64 {
        (width / self.background[0]) * self.area.width
    }

    fn height(&self, height: f64) -> f64 {
        (height / self.background[1]) * self.area.height
    }

    fn set_component_style(&mut self, is_yellow: bool) {
        let color = self.color(is_yellow);
        self.context.set_fill_style(color);
        self.context.set_stroke_style(color);
        self.context.set_line_width(5.0);
    }

    /// `drawButton`, which returns the scaled center and size.
    fn button(
        &mut self,
        is_yellow: bool,
        is_ellipse: bool,
        center: [f64; 2],
        size: [f64; 2],
        active: bool,
    ) -> [f64; 4] {
        self.set_component_style(is_yellow);
        self.context.begin_path();
        let [x, y] = [self.x(center[0]), self.y(center[1])];
        let [width, height] = [self.width(size[0]), self.height(size[1])];
        if is_ellipse {
            self.context.ellipse(
                x,
                y,
                width / 2.0,
                height / 2.0,
                0.0,
                0.0,
                math::PI * 2.0,
                None,
            );
        } else {
            self.context
                .rect(x - width / 2.0, y - height / 2.0, width, height);
        }
        self.context.stroke();
        if active {
            self.context.fill();
        }
        [x, y, width, height]
    }

    /// `drawJoystick`.
    fn stick(&mut self, stick: &ConfigJoystickStick, x_value: f64, y_value: f64, pressed: bool) {
        let ConfigJoystickStick {
            center_px: [center_x, center_y],
            radius_px: radius,
            ..
        } = *stick;
        self.set_component_style(stick.is_yellow);

        self.context.begin_path();
        let [x, y] = [self.x(center_x), self.y(center_y)];
        let [radius_x, radius_y] = [self.width(radius), self.height(radius)];
        self.context
            .ellipse(x, y, radius_x, radius_y, 0.0, 0.0, math::PI * 2.0, None);
        self.context.stroke();

        self.context.begin_path();
        let stick_x = self.x(center_x + radius * x_value);
        let stick_y = self.y(center_y - radius * y_value);
        let stick_radius = self.width(radius * 0.75);
        self.context.ellipse(
            stick_x,
            stick_y,
            stick_radius,
            stick_radius,
            0.0,
            0.0,
            math::PI * 2.0,
            None,
        );
        self.context.fill();

        if pressed {
            self.set_font(stick_radius * 1.25, "center");
            self.context.set_fill_style("#000000");
            self.context.fill_text("X", stick_x, stick_y);
        }
    }

    /// `drawAxis`, which returns the scaled rectangle.
    fn axis(
        &mut self,
        is_yellow: bool,
        center: [f64; 2],
        size: [f64; 2],
        scaled_value: f64,
    ) -> [f64; 4] {
        self.set_component_style(is_yellow);
        let x = self.x(center[0] - size[0] / 2.0);
        let y = self.y(center[1] - size[1] / 2.0);
        let width = self.width(size[0]);
        let height = self.height(size[1]);
        let value = math::max2(math::min2(scaled_value, 1.0), 0.0);
        self.context.stroke_rect(x, y, width, height);
        self.context
            .fill_rect(x, y + height - height * value, width, height * value);
        [x, y, width, height]
    }

    fn fingers(
        &mut self,
        touchpad: &JsValue,
        is_yellow: bool,
        [center_x, center_y, width, height]: [f64; 4],
    ) -> Result<(), JsError> {
        for finger in array(touchpad, "forEach")? {
            if !ops::to_boolean(member(finger, "down")?) {
                continue;
            }
            let x = operand(member(finger, "x")?)?;
            if !(0.0..=1.0).contains(&x) {
                continue;
            }
            let y = operand(member(finger, "y")?)?;
            if !(0.0..=1.0).contains(&y) {
                continue;
            }
            let canvas_x = center_x - width / 2.0 + x * width;
            let canvas_y = center_y - height / 2.0 + y * height;
            let color = self.color(is_yellow);
            self.context.set_fill_style(color);
            self.context.begin_path();
            arc(
                self.context,
                [canvas_x, canvas_y],
                8.0,
                0.0,
                math::PI * 2.0,
                None,
            )?;
            self.context.fill();
        }
        Ok(())
    }

    fn set_font(&mut self, size: f64, align: &str) {
        self.context
            .set_font(&format!("{}px sans-serif", number::to_string(size)));
        self.context.set_text_align(align);
        self.context.set_text_baseline("middle");
    }

    fn layout(
        &mut self,
        config: &ConfigJoystick,
        state: Result<&JsValue, JsError>,
    ) -> Result<(), JsError> {
        let Rect {
            left,
            top,
            width,
            height,
        } = self.area;
        self.context
            .draw_image_scaled(&config.path.to_string_lossy(), left, top, width, height);
        let state = state?;
        let is_sdl = ops::strict_equals(member(state, "mapping")?, &JsValue::from("sdl"));
        for component in &config.components {
            match component {
                ConfigJoystickComponent::Button(button) => {
                    let active = button_active(button, is_sdl, state)?;
                    self.button(
                        button.is_yellow,
                        button.is_ellipse,
                        button.center_px,
                        button.size_px,
                        active,
                    );
                }
                ConfigJoystickComponent::Joystick(stick) => {
                    let ([x, y], button) = stick_sources(stick, is_sdl);
                    let x_value = axis_value(state, x)?;
                    let y_value = axis_value(state, y)?;
                    let pressed = match button {
                        Some(id) => button_state(state, id)?,
                        None => false,
                    };
                    self.stick(stick, x_value, y_value, pressed);
                }
                ConfigJoystickComponent::Axis(axis) => {
                    let value = axis_scaled_value(axis, is_sdl, state)?;
                    self.axis(axis.is_yellow, axis.center_px, axis.size_px, value);
                }
                ConfigJoystickComponent::Touchpad(touchpad) if is_sdl => {
                    self.touchpad(touchpad, state)?;
                }
                ConfigJoystickComponent::Touchpad(_) => {}
            }
        }
        Ok(())
    }

    fn touchpad(
        &mut self,
        touchpad: &ConfigJoystickTouchpad,
        state: &JsValue,
    ) -> Result<(), JsError> {
        let layout = self.button(
            touchpad.is_yellow,
            false,
            touchpad.center_px,
            touchpad.size_px,
            false,
        );
        let index = touchpad.sdl_source_index;
        let touchpads = array(member(state, "touchpads")?, &number::to_string(index))?;
        let pad = (math::floor(index) == index)
            .then(|| touchpads.get(index as usize))
            .flatten();
        match pad {
            Some(pad) if ops::to_boolean(pad) => self.fingers(pad, touchpad.is_yellow, layout),
            _ => Ok(()),
        }
    }

    fn generic(&mut self, state: &JsValue) -> Result<(), JsError> {
        let text_color = self.color(false);
        let buttons = array(member(state, "buttons")?, "length")?;
        let button_rows = rows_of(buttons.len());
        for (index, button) in buttons.iter().enumerate() {
            let (column, row) = cell(index);
            let button_state = member(button, "state")?;
            let [x, y, _, height] = self.button(
                false,
                false,
                [70.0 + column * 100.0, 70.0 + row * 100.0],
                [60.0, 60.0],
                ops::to_boolean(button_state),
            );
            self.set_font(height * 0.5, "center");
            let inverted = ops::strict_equals(&JsValue::Bool(self.is_light), button_state);
            self.context
                .set_fill_style(if inverted { WHITE_COLOR } else { BLACK_COLOR });
            self.context.fill_text(id_text(button)?, x, y);
        }

        let axes = array(member(state, "axes")?, "length")?;
        let axis_rows = rows_of(axes.len());
        for (index, axis) in axes.iter().enumerate() {
            let (column, row) = cell(index);
            let value = operand(member(axis, "value")?)?;
            let [x, y, width, height] = self.axis(
                false,
                [95.0 + column * 100.0, 70.0 + (row + button_rows) * 100.0],
                [30.0, 80.0],
                scale_value(value, [-1.0, 1.0], [0.0, 1.0]),
            );
            self.set_font(height * 0.6, "center");
            self.context.set_fill_style(text_color);
            self.context
                .fill_text(id_text(axis)?, x - width, y + height / 2.0);
        }

        let povs = array(member(state, "povs")?, "length")?;
        let pov_rows = rows_of(povs.len());
        for (index, pov) in povs.iter().enumerate() {
            let (column, row) = cell(index);
            let [x, y] = [
                70.0 + column * 100.0,
                70.0 + (row + button_rows + axis_rows) * 100.0,
            ];
            let angle = member(pov, "angle")?;
            for (direction, center, size) in [
                (SourcePov::Up, [x, y - 30.0], [40.0, 20.0]),
                (SourcePov::Right, [x + 30.0, y], [20.0, 40.0]),
                (SourcePov::Down, [x, y + 30.0], [40.0, 20.0]),
                (SourcePov::Left, [x - 30.0, y], [20.0, 40.0]),
            ] {
                self.button(false, false, center, size, pov_active(angle, direction));
            }
            let font_size = self.width(40.0) * 0.6;
            self.set_font(font_size, "center");
            self.context.set_fill_style(text_color);
            let [text_x, text_y] = [self.x(x), self.y(y)];
            self.context.fill_text(id_text(pov)?, text_x, text_y);
        }

        let touchpads = array(member(state, "touchpads")?, "length")?;
        for (index, touchpad) in touchpads.iter().enumerate() {
            let (_, row) = cell(index);
            let layout = self.button(
                false,
                false,
                [
                    95.0 + index as f64 * 140.0,
                    70.0 + (row + button_rows + axis_rows + pov_rows) * 100.0,
                ],
                [80.0, 80.0],
                false,
            );
            self.fingers(touchpad, false, layout)?;
            let [x, y, width, height] = layout;
            self.set_font(height * 0.4, "right");
            self.context.set_fill_style(text_color);
            self.context.fill_text(
                number::to_string(index as f64),
                x - width / 2.0 - width * 0.2,
                y,
            );
        }

        if buttons.is_empty() && axes.is_empty() && povs.is_empty() && touchpads.is_empty() {
            let Rect {
                left,
                top,
                width,
                height,
            } = self.area;
            message(
                self.context,
                self.is_light,
                "No data for joystick.",
                [left + width / 2.0, top + height / 2.0],
            );
        }
        Ok(())
    }
}

/// `[index % 6, Math.floor(index / 6)]`.
fn cell(index: usize) -> (f64, f64) {
    ((index % 6) as f64, (index / 6) as f64)
}

fn button_state(state: &JsValue, id: f64) -> Result<bool, JsError> {
    Ok(match find_id(member(state, "buttons")?, id)? {
        Some(button) => ops::to_boolean(member(button, "state")?),
        None => false,
    })
}

fn button_active(
    button: &ConfigJoystickButton,
    is_sdl: bool,
    state: &JsValue,
) -> Result<bool, JsError> {
    let (source_pov, source_index) = if is_sdl {
        (button.sdl_source_pov, button.sdl_source_index)
    } else {
        (button.ni_source_pov, button.ni_source_index)
    };
    match (source_pov, source_index) {
        (Some(direction), Some(id)) => Ok(match find_id(member(state, "povs")?, id)? {
            Some(pov) => pov_active(member(pov, "angle")?, direction),
            None => false,
        }),
        (None, Some(id)) => button_state(state, id),
        (_, None) => Ok(false),
    }
}

/// A source index and whether it is inverted.
type AxisSource = (Option<f64>, Option<bool>);

/// The x and y axes and the button of a stick.
fn stick_sources(stick: &ConfigJoystickStick, is_sdl: bool) -> ([AxisSource; 2], Option<f64>) {
    if is_sdl {
        (
            [
                (stick.sdl_x_source_index, stick.sdl_x_source_inverted),
                (stick.sdl_y_source_index, stick.sdl_y_source_inverted),
            ],
            stick.sdl_button_source_index,
        )
    } else {
        (
            [
                (stick.ni_x_source_index, stick.ni_x_source_inverted),
                (stick.ni_y_source_index, stick.ni_y_source_inverted),
            ],
            stick.ni_button_source_index,
        )
    }
}

fn axis_value(state: &JsValue, (id, inverted): AxisSource) -> Result<f64, JsError> {
    let Some(id) = id else {
        return Ok(0.0);
    };
    let Some(axis) = find_id(member(state, "axes")?, id)? else {
        return Ok(0.0);
    };
    let value = operand(member(axis, "value")?)?;
    Ok(if inverted == Some(true) {
        -value
    } else {
        value
    })
}

fn axis_scaled_value(
    axis: &ConfigJoystickAxis,
    is_sdl: bool,
    state: &JsValue,
) -> Result<f64, JsError> {
    let (id, range) = if is_sdl {
        (axis.sdl_source_index, axis.sdl_source_range)
    } else {
        (axis.ni_source_index, axis.ni_source_range)
    };
    let Some(id) = id else {
        return Ok(0.0);
    };
    let Some(axis) = find_id(member(state, "axes")?, id)? else {
        return Ok(0.0);
    };
    let value = operand(member(axis, "value")?)?;
    Ok(scale_value(value, range.unwrap_or([-1.0, 1.0]), [0.0, 1.0]))
}

impl<Cx: SourceListCx + ?Sized> TabRenderer<Cx> for JoysticksRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        self.render_command(cx.tab_dark(), cx.tab_assets(), &command.to_js())
    }
}
