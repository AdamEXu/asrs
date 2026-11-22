//! The headless half of `src/shared/renderers/Field2dRenderer.ts`: the camera state
//! (`Field2dCameraMode`, zoom, pan, `lastFieldCenter`) that `render`, `set2DCamera`, wheel and
//! drag input and the camera context menu mutate, and the `lastRenderState` gate. Painting is
//! R05's. Of `ScrollSensor`, only the drag pan and cursor tracking live here, as in the
//! timeline (R03); wheel deltas arrive through [`Field2dRenderer::wheel`] from the host's
//! wheel mapping (U08) instead of the DOM scroll field.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_js::{json, math, number, ops};
use asrs_log::units;
use asrs_log::util::scale_value;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::tab_controller::TabRenderer;

use super::command::{Field2dCommand, Object, Orientation};
use crate::cx::TabCx;

/// The DOM the renderer reads: `CONTAINER`/`SCROLL_CONTAINER` rects, `devicePixelRatio`,
/// the `satellite` body class and the natural sizes of the images that can load.
#[derive(Debug, Default)]
pub struct Field2dDom {
    /// `CONTAINER.getBoundingClientRect()`: `[x, y, width, height]`.
    pub container: [f64; 4],
    /// `SCROLL_CONTAINER.getBoundingClientRect()`.
    pub scroll_container: [f64; 4],
    /// `window.devicePixelRatio`.
    pub device_pixel_ratio: f64,
    /// `document.body.classList.contains("satellite")`.
    pub satellite: bool,
    /// Natural `[width, height]` per `IMAGE.src`; a missing entry is a broken image.
    pub images: HashMap<JsString, [f64; 2]>,
}

impl Field2dDom {
    /// `CONTAINER.clientWidth` (`Math.round` of the rect).
    pub fn client_width(&self) -> f64 {
        math::round(self.container[2])
    }

    /// `CONTAINER.clientHeight`.
    pub fn client_height(&self) -> f64 {
        math::round(self.container[3])
    }

    /// `[IMAGE.width, IMAGE.height]` for `src`; `[0, 0]` while unloaded or missing.
    pub fn image_size(&self, src: &JsString) -> [f64; 2] {
        self.images.get(src).copied().unwrap_or([0.0, 0.0])
    }
}

/// `Field2dCameraMode` is a plain number: `set2DCamera` accepts `0..=2` but stores any
/// value that passes, and `restoreState` stores whatever `trackingMode` was.
pub type CameraMode = f64;
pub const CAMERA_UNLOCKED: CameraMode = 0.0;
pub const CAMERA_ROBOT: CameraMode = 1.0;
pub const CAMERA_ROBOT_AND_ROTATION: CameraMode = 2.0;

/// The arguments of one `ScrollSensor` callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollArgs {
    pub dx: f64,
    pub dy: f64,
    pub is_pan: bool,
    pub cursor: [f64; 2],
}

pub struct Field2dRenderer {
    dom: Rc<RefCell<Field2dDom>>,
    last_image_source: JsString,
    aspect_ratio: f64,
    last_render_state: String,
    image_load_count: u64,
    pending_image_loads: u64,
    zoom: f64,
    /// Restored pans keep whatever two values the saved state held; `+=` and `*` then
    /// apply JS coercion to them.
    pan: [JsValue; 2],
    tracking_mode: CameraMode,
    jump_to_robot: bool,
    last_tracking_mode: CameraMode,
    last_field_center: Option<[f64; 2]>,
    is_ftc: bool,
    last_command: Option<Field2dCommand>,
    pan_active: bool,
    pan_last_cursor: [f64; 2],
    last_cursor: [f64; 2],
    start_px: Option<[f64; 2]>,
    frame_requests: u32,
}

fn to_number(value: &JsValue) -> f64 {
    ops::value_to_number(value).unwrap_or(f64::NAN)
}

/// `value += n`.
fn add_assign(value: &JsValue, n: f64) -> JsValue {
    match value {
        JsValue::String(_)
        | JsValue::Array(_)
        | JsValue::Object(_)
        | JsValue::Date(_)
        | JsValue::Bytes(_)
        | JsValue::ExtData(..) => {
            let mut s = ops::to_string(value);
            s.push_str(&JsString::from(number::to_string(n)));
            JsValue::String(s)
        }
        _ => JsValue::Number(to_number(value) + n),
    }
}

impl Field2dRenderer {
    pub fn new(dom: Rc<RefCell<Field2dDom>>) -> Field2dRenderer {
        Field2dRenderer {
            dom,
            last_image_source: JsString::new(),
            aspect_ratio: 1.0,
            last_render_state: String::new(),
            image_load_count: 0,
            pending_image_loads: 0,
            zoom: 1.0,
            pan: [0.0.to_js(), 0.0.to_js()],
            tracking_mode: CAMERA_UNLOCKED,
            jump_to_robot: false,
            last_tracking_mode: CAMERA_UNLOCKED,
            last_field_center: None,
            is_ftc: false,
            last_command: None,
            pan_active: false,
            pan_last_cursor: [0.0, 0.0],
            last_cursor: [0.0, 0.0],
            start_px: None,
            frame_requests: 0,
        }
    }

    /// Delivers the `IMAGE` `load` events queued since the last call: assigning a `src`
    /// that can load fires one asynchronously.
    pub fn flush_image_loads(&mut self) {
        self.image_load_count += self.pending_image_loads;
        self.pending_image_loads = 0;
    }

    pub fn image_load_count(&self) -> u64 {
        self.image_load_count
    }

    /// The animation frames the scroll callback requested since the last call; the host
    /// renders the last command once in each.
    pub fn take_frame_requests(&mut self) -> u32 {
        std::mem::take(&mut self.frame_requests)
    }

    /// The private camera fields, in the form the DOM harness records them.
    pub fn camera_state(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("zoom", self.zoom.to_js());
        o.insert("pan", JsValue::Array(self.pan.to_vec()));
        o.insert("trackingMode", self.tracking_mode.to_js());
        o.insert("lastTrackingMode", self.last_tracking_mode.to_js());
        o.insert("jumpToRobot", self.jump_to_robot.to_js());
        o.insert(
            "lastFieldCenter",
            self.last_field_center.map_or(JsValue::Undefined, |[x, y]| {
                JsValue::Array(vec![x.to_js(), y.to_js()])
            }),
        );
        o.insert("isFTC", self.is_ftc.to_js());
        o.insert("aspectRatio", self.aspect_ratio.to_js());
        o.insert("lastImageSource", self.last_image_source.to_js());
        JsValue::Object(o)
    }

    fn set_tracking_mode(&mut self, mode: CameraMode) {
        if mode != CAMERA_UNLOCKED {
            self.jump_to_robot = true;
        }
        self.tracking_mode = mode;
    }

    /// `set2DCamera`.
    pub fn set_2d_camera<C: TabCx + ?Sized>(&mut self, cx: &C, index: f64) {
        if (0.0..=2.0).contains(&index) {
            self.set_tracking_mode(index);
            if let Some(command) = self.last_command.clone() {
                self.render_command(cx, &command);
            }
        }
    }

    /// A wheel scroll: the sensor's non-pan callback at the cursor the last `mousemove`
    /// left.
    pub fn wheel<C: TabCx + ?Sized>(&mut self, cx: &C, dx: f64, dy: f64) -> ScrollArgs {
        let args = ScrollArgs {
            dx,
            dy,
            is_pan: false,
            cursor: self.last_cursor,
        };
        self.scroll(cx, args);
        args
    }

    /// The `ScrollSensor` callback: zoom or pan, then `requestAnimationFrame(render)`.
    pub fn scroll<C: TabCx + ?Sized>(&mut self, cx: &C, args: ScrollArgs) {
        let Some(command) = &self.last_command else {
            return;
        };
        let is_vertical = command.orientation == Orientation::Deg90.number()
            || command.orientation == Orientation::Deg270.number();
        let dom = self.dom.borrow();
        let container_width = dom.client_width();
        let container_height = dom.client_height();
        let visible_y_offset = if dom.satellite { 0.0 } else { -13.0 };
        let bounding_width = if is_vertical {
            container_height - math::abs(visible_y_offset * 2.0)
        } else {
            container_width
        };
        let bounding_height = if is_vertical {
            container_width
        } else {
            container_height - math::abs(visible_y_offset * 2.0)
        };
        let Some(field_data) = cx
            .tab_assets()
            .and_then(|a| a.field2ds.iter().find(|f| f.id == command.field))
        else {
            return;
        };
        let [image_width, image_height] = dom.image_size(&self.last_image_source);
        let field_width = field_data.bottom_right[0] - field_data.top_left[0];
        let field_height = field_data.bottom_right[1] - field_data.top_left[1];
        let margin = math::min(&[
            field_data.top_left[1],
            image_height - field_data.bottom_right[1],
            field_data.top_left[0],
            image_width - field_data.bottom_right[0],
        ]);
        let extended_field_width = field_width + margin * 2.0;
        let extended_field_height = field_height + margin * 2.0;
        let base_image_scalar =
            if bounding_width / bounding_height > extended_field_width / extended_field_height {
                bounding_height / extended_field_height
            } else {
                bounding_width / extended_field_width
            };
        let canvas_center_x = args.cursor[0] - container_width / 2.0;
        let canvas_center_y = args.cursor[1] - container_height / 2.0;
        drop(dom);

        if args.is_pan {
            self.pan[0] = add_assign(&self.pan[0], args.dx / (base_image_scalar * self.zoom));
            self.pan[1] = add_assign(&self.pan[1], args.dy / (base_image_scalar * self.zoom));
        } else {
            let zoom_change = args.dy * -0.005;
            let old_zoom = self.zoom;
            self.zoom = math::min(&[10.0, math::max(&[1.0, self.zoom + zoom_change])]);
            if self.zoom != old_zoom {
                let scale = 1.0 / old_zoom - 1.0 / self.zoom;
                self.pan[0] =
                    add_assign(&self.pan[0], (canvas_center_x / base_image_scalar) * scale);
                self.pan[1] =
                    add_assign(&self.pan[1], (canvas_center_y / base_image_scalar) * scale);
            }
        }
        self.frame_requests += 1;
    }

    /// `mousedown`, `mousemove`, `mouseup` or `mouseleave` (`kind` without the `mouse`
    /// prefix) on `SCROLL_CONTAINER`, with `clientX`/`clientY`: the sensor's drag pan,
    /// then the camera menu's right-click tracking. Returns the scroll callback a drag
    /// dispatched.
    pub fn mouse<C: TabCx + ?Sized>(
        &mut self,
        cx: &mut C,
        kind: &str,
        button: f64,
        shift: bool,
        x: f64,
        y: f64,
    ) -> Option<ScrollArgs> {
        let scroll_rect = self.dom.borrow().scroll_container;
        let cursor = [x - scroll_rect[0], y - scroll_rect[1]];
        match kind {
            "down" => {
                if !shift {
                    self.pan_active = true;
                    self.pan_last_cursor = cursor;
                }
                if button == 2.0 {
                    self.start_px = Some([x, y]);
                }
            }
            "move" => {
                self.last_cursor = cursor;
                if self.pan_active {
                    let args = ScrollArgs {
                        dx: self.pan_last_cursor[0] - cursor[0],
                        dy: self.pan_last_cursor[1] - cursor[1],
                        is_pan: true,
                        cursor,
                    };
                    self.scroll(cx, args);
                    self.pan_last_cursor = cursor;
                    return Some(args);
                }
            }
            "up" => {
                self.pan_active = false;
                if self.start_px == Some([x, y]) {
                    let mut data = JsObject::new();
                    data.insert("position", JsValue::Array(vec![x.to_js(), y.to_js()]));
                    data.insert("selectedIndex", self.tracking_mode.to_js());
                    cx.send_main_message(NamedMessage::new(
                        HubToMain::Ask2dCamera,
                        JsValue::Object(data),
                    ));
                }
                self.start_px = None;
            }
            "leave" => self.pan_active = false,
            _ => {}
        }
        None
    }

    /// `render(command)`.
    pub fn render_command<C: TabCx + ?Sized>(&mut self, cx: &C, command: &Field2dCommand) {
        self.last_command = Some(command.clone());
        let Some(field_data) = cx
            .tab_assets()
            .and_then(|a| a.field2ds.iter().find(|f| f.id == command.field))
        else {
            return;
        };
        if field_data.is_ftc != self.is_ftc {
            self.is_ftc = field_data.is_ftc;
            self.zoom = 1.0;
            self.pan = [0.0.to_js(), 0.0.to_js()];
            self.tracking_mode = CAMERA_UNLOCKED;
        }

        let primary_robot_pose = command.objects.iter().find_map(|object| match object {
            Object::Robot { poses, .. } => Some(poses.first().map(|p| p.pose)),
            _ => None,
        });
        let primary_robot_pose = primary_robot_pose.flatten();

        let dom = self.dom.borrow();
        let container_width = dom.client_width();
        let container_height = dom.client_height();
        let is_vertical = command.orientation == Orientation::Deg90.number()
            || command.orientation == Orientation::Deg270.number();
        let visible_y_offset = if dom.satellite { 0.0 } else { -13.0 };

        let (mut width, mut height) = if is_vertical {
            (container_height, container_width)
        } else {
            (container_width, container_height)
        };
        let bounding_width = if is_vertical {
            container_height - math::abs(visible_y_offset * 2.0)
        } else {
            container_width
        };
        let bounding_height = if is_vertical {
            container_width
        } else {
            container_height - math::abs(visible_y_offset * 2.0)
        };
        if self.tracking_mode == CAMERA_ROBOT_AND_ROTATION {
            let max_dim = math::hypot(&[container_width, container_height]);
            width = max_dim;
            height = max_dim;
        }

        let render_state = JsValue::Array(vec![
            width.to_js(),
            height.to_js(),
            dom.device_pixel_ratio.to_js(),
            command.to_js(),
            (self.image_load_count as f64).to_js(),
            self.zoom.to_js(),
            self.pan[0].clone(),
            self.pan[1].clone(),
            self.tracking_mode.to_js(),
        ]);
        let render_state = json::stringify(&render_state)
            .ok()
            .flatten()
            .unwrap_or_default();
        if render_state == self.last_render_state {
            return;
        }
        self.last_render_state = render_state;

        let mut canvas_rotation = match Orientation::from_number(command.orientation) {
            Some(Orientation::Deg0) | None => 0.0,
            Some(Orientation::Deg90) => -math::PI / 2.0,
            Some(Orientation::Deg180) => math::PI,
            Some(Orientation::Deg270) => math::PI / 2.0,
        };
        if self.tracking_mode == CAMERA_ROBOT_AND_ROTATION
            && let Some(pose) = primary_robot_pose
        {
            canvas_rotation += pose.rotation;
        }

        if field_data.path != self.last_image_source {
            self.last_image_source = field_data.path.clone();
            let [w, h] = dom.image_size(&self.last_image_source);
            if w > 0.0 && h > 0.0 {
                self.pending_image_loads += 1;
            }
        }
        let [image_width, image_height] = dom.image_size(&self.last_image_source);
        drop(dom);
        if !(image_width > 0.0 && image_height > 0.0) {
            return;
        }

        let field_width = field_data.bottom_right[0] - field_data.top_left[0];
        let field_height = field_data.bottom_right[1] - field_data.top_left[1];
        let mut target_field_x = field_width * 0.5 + field_data.top_left[0];
        let mut target_field_y = field_height * 0.5 + field_data.top_left[1];
        if self.tracking_mode > CAMERA_UNLOCKED
            && let Some(pose) = primary_robot_pose
        {
            let inches = |m: f64| units::convert(m, "meters", "inches").unwrap_or(f64::NAN);
            target_field_x = scale_value(
                inches(pose.translation[0]),
                [
                    -field_data.width_inches / 2.0,
                    field_data.width_inches / 2.0,
                ],
                [field_data.top_left[0], field_data.top_left[0] + field_width],
            );
            target_field_y = scale_value(
                inches(pose.translation[1]),
                [
                    -field_data.height_inches / 2.0,
                    field_data.height_inches / 2.0,
                ],
                [
                    field_data.top_left[1] + field_height,
                    field_data.top_left[1],
                ],
            );
        }

        if self.jump_to_robot {
            self.pan = [0.0.to_js(), 0.0.to_js()];
            self.jump_to_robot = false;
        } else if self.last_tracking_mode != self.tracking_mode
            && self.tracking_mode == CAMERA_UNLOCKED
            && let Some(last_center) = self.last_field_center
        {
            let dx = last_center[0] - target_field_x;
            let dy = last_center[1] - target_field_y;
            self.pan = [
                (dx * math::cos(canvas_rotation) - dy * math::sin(canvas_rotation)).to_js(),
                (dx * math::sin(canvas_rotation) + dy * math::cos(canvas_rotation)).to_js(),
            ];
        }
        self.last_tracking_mode = self.tracking_mode;

        let pan_x = to_number(&self.pan[0]);
        let pan_y = to_number(&self.pan[1]);
        let mut pan_field_x =
            pan_x * math::cos(-canvas_rotation) - pan_y * math::sin(-canvas_rotation);
        let mut pan_field_y =
            pan_x * math::sin(-canvas_rotation) + pan_y * math::cos(-canvas_rotation);

        let margin = math::min(&[
            field_data.top_left[1],
            image_height - field_data.bottom_right[1],
            field_data.top_left[0],
            image_width - field_data.bottom_right[0],
        ]);
        let extended_field_width = field_width + margin * 2.0;
        let extended_field_height = field_height + margin * 2.0;
        let base_image_scalar =
            if bounding_width / bounding_height > extended_field_width / extended_field_height {
                bounding_height / extended_field_height
            } else {
                bounding_width / extended_field_width
            };
        let image_scalar = base_image_scalar * self.zoom;

        if self.tracking_mode == CAMERA_UNLOCKED {
            let field_display_width = extended_field_width * image_scalar;
            let field_display_height = extended_field_height * image_scalar;

            let pan_offset_x = (math::sin(canvas_rotation) * visible_y_offset) / image_scalar;
            let pan_offset_y = (math::cos(canvas_rotation) * visible_y_offset) / image_scalar;

            let margin_x =
                math::max(&[0.0, (field_display_width - bounding_width) * 0.5]) / image_scalar;
            let margin_y =
                math::max(&[0.0, (field_display_height - bounding_height) * 0.5]) / image_scalar;

            pan_field_x = math::max(&[
                -margin_x + pan_offset_x,
                math::min(&[margin_x + pan_offset_x, pan_field_x]),
            ]);
            pan_field_y = math::max(&[
                -margin_y + pan_offset_y,
                math::min(&[margin_y + pan_offset_y, pan_field_y]),
            ]);

            self.pan = [
                (pan_field_x * math::cos(canvas_rotation)
                    - pan_field_y * math::sin(canvas_rotation))
                .to_js(),
                (pan_field_x * math::sin(canvas_rotation)
                    + pan_field_y * math::cos(canvas_rotation))
                .to_js(),
            ];
        }

        self.last_field_center = Some([target_field_x + pan_field_x, target_field_y + pan_field_y]);
        self.aspect_ratio = if is_vertical {
            field_height / field_width
        } else {
            field_width / field_height
        };
    }
}

impl<C: TabCx + ?Sized> TabRenderer<C> for Field2dRenderer {
    fn save_state(&self, _cx: &C) -> JsValue {
        let mut o = JsObject::new();
        o.insert("zoom", self.zoom.to_js());
        o.insert("pan", JsValue::Array(self.pan.to_vec()));
        o.insert("trackingMode", self.tracking_mode.to_js());
        o.insert("isFTC", self.is_ftc.to_js());
        JsValue::Object(o)
    }

    fn restore_state(&mut self, _cx: &mut C, state: &JsValue) -> Result<(), JsError> {
        if !matches!(state, JsValue::Object(_) | JsValue::Array(_)) {
            return Ok(());
        }
        if let Some(JsValue::Number(zoom)) = state.get("zoom") {
            self.zoom = *zoom;
        }
        if let Some(JsValue::Array(pan)) = state.get("pan")
            && let [x, y] = pan.as_slice()
        {
            self.pan = [x.clone(), y.clone()];
        }
        if let Some(JsValue::Number(mode)) = state.get("trackingMode") {
            self.set_tracking_mode(*mode);
        }
        if let Some(JsValue::Bool(is_ftc)) = state.get("isFTC") {
            self.is_ftc = *is_ftc;
        }
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &C) -> Option<f64> {
        Some(self.aspect_ratio)
    }

    fn render(
        &mut self,
        cx: &mut C,
        command: &dyn asrs_model::tab_controller::TabCommand,
    ) -> Result<(), JsError> {
        let Some(command) = command.downcast_ref::<Field2dCommand>() else {
            self.last_command = None;
            return match cx.tab_assets() {
                Some(assets) if !assets.field2ds.is_empty() => Err(JsError::type_error(
                    "Cannot read properties of undefined (reading 'field')",
                )),
                _ => Ok(()),
            };
        };
        self.render_command(cx, command);
        Ok(())
    }
}
