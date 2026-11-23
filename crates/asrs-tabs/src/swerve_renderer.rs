//! `SwerveRenderer.ts`: the robot frame, its modules and the velocity arrows.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::Canvas2d;
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_js::{math, number};
use asrs_log::util::{transform_px, wrap_radians};
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use crate::canvas_painter::{arc, array, canvas_color, member, operand, state_changed};
use crate::list_cx::SourceListCx;

const BLACK_COLOR: &str = "#222222";
const WHITE_COLOR: &str = "#eeeeee";

/// The DOM the renderer reads and the frame it painted, shared with the host.
#[derive(Debug, Default)]
pub struct SwerveDom {
    /// `CONTAINER.clientWidth` and `clientHeight`.
    pub client_size: [f64; 2],
    /// `window.devicePixelRatio`.
    pub device_pixel_ratio: f64,
    /// `CANVAS.style.width` and `height` in px, as the last painted frame set both.
    pub canvas_size: Option<f64>,
    /// The last painted frame, until the host takes it.
    pub painted: Option<Canvas2d>,
}

impl SwerveDom {
    /// `[CANVAS.style.width, CANVAS.style.height]`.
    pub fn canvas_style(&self) -> Option<[String; 2]> {
        let size = format!("{}px", number::to_string(self.canvas_size?));
        Some([size.clone(), size])
    }
}

pub struct SwerveRenderer {
    dom: Rc<RefCell<SwerveDom>>,
    last_render_state: String,
}

impl SwerveRenderer {
    pub fn new(dom: Rc<RefCell<SwerveDom>>) -> SwerveRenderer {
        SwerveRenderer {
            dom,
            last_render_state: String::new(),
        }
    }

    pub fn dom(&self) -> &Rc<RefCell<SwerveDom>> {
        &self.dom
    }

    /// `render(command)` in the colour scheme `dark`: when the render state changed, paints a
    /// new frame into `dom.painted`. A throw leaves the frame as far as it was painted.
    pub fn render_command(&mut self, dark: bool, command: &JsValue) -> Result<(), JsError> {
        let mut dom = self.dom.borrow_mut();
        let [client_width, client_height] = dom.client_size;
        let dpr = dom.device_pixel_ratio;
        let render_state = JsValue::Array(vec![
            JsValue::Number(client_width),
            JsValue::Number(client_height),
            JsValue::Bool(dark),
            JsValue::Number(dpr),
            command.clone(),
        ]);
        if !state_changed(&mut self.last_render_state, &render_state)? {
            return Ok(());
        }

        let size = math::min2(client_width, client_height);
        dom.canvas_size = Some(size);
        let pixels = math::to_uint32(size * dpr);
        let context = dom.painted.insert(Canvas2d::new(pixels, pixels));
        context.scale(dpr, dpr);
        context.clear_rect(0.0, 0.0, size, size);
        context.set_line_cap("round");
        context.set_line_join("round");
        paint(context, size, dark, command)
    }
}

fn line(context: &mut Canvas2d, points: [[f64; 2]; 2]) {
    let [[x0, y0], [x1, y1]] = points;
    context.begin_path();
    context.move_to(x0, y0);
    context.line_to(x1, y1);
    context.stroke();
}

struct Arrow {
    back: [f64; 2],
    front: [f64; 2],
    left: [f64; 2],
    right: [f64; 2],
}

impl Arrow {
    fn stroke(&self, context: &mut Canvas2d) {
        context.begin_path();
        context.move_to(self.back[0], self.back[1]);
        context.line_to(self.front[0], self.front[1]);
        self.head(context);
        context.stroke();
    }

    fn head(&self, context: &mut Canvas2d) {
        context.move_to(self.left[0], self.left[1]);
        context.line_to(self.front[0], self.front[1]);
        context.line_to(self.right[0], self.right[1]);
    }

    /// From `back` to `front` px along `rotation`, with a head of `head` px.
    fn along(origin: [f64; 2], rotation: f64, back: f64, front: f64, head: f64) -> Arrow {
        let at = |x, y| transform_px(origin, rotation, [x, y]);
        Arrow {
            back: at(back, 0.0),
            front: at(front, 0.0),
            left: at(front - head, head),
            right: at(front - head, -head),
        }
    }
}

fn paint(context: &mut Canvas2d, size: f64, dark: bool, command: &JsValue) -> Result<(), JsError> {
    let center = [size / 2.0, size / 2.0];
    let stroke_color = if dark { WHITE_COLOR } else { BLACK_COLOR };

    let frame_aspect_ratio = operand(member(command, "frameAspectRatio")?)?;
    let frame_width = size * 0.3 * math::min2(frame_aspect_ratio, 1.0);
    let frame_height = (size * 0.3) / math::max2(frame_aspect_ratio, 1.0);
    let module_radius = size * 0.05;
    let full_vector = size * 0.25;
    let rotation = operand(member(command, "rotation")?)?;
    let at = |x: f64, y: f64| transform_px(center, rotation, [x, y]);

    context.set_stroke_style(stroke_color);
    context.set_line_width(4.0);
    let (half_height, half_width) = (frame_height / 2.0, frame_width / 2.0);
    for [from, to] in [
        [
            [half_height, half_width - module_radius],
            [half_height, -half_width + module_radius],
        ],
        [
            [half_height - module_radius, -half_width],
            [-half_height + module_radius, -half_width],
        ],
        [
            [half_height - module_radius, half_width],
            [-half_height + module_radius, half_width],
        ],
        [
            [-half_height, half_width - module_radius],
            [-half_height, -half_width + module_radius],
        ],
    ] {
        line(context, [at(from[0], from[1]), at(to[0], to[1])]);
    }

    context.set_stroke_style(stroke_color);
    context.set_line_width(4.0);
    let robot_velocities = member(command, "robotVelocities")?;
    let has_speeds = !array(robot_velocities, "length")?.is_empty();
    let [back, front, sides] = if has_speeds {
        [0.5, 0.8, 0.65]
    } else {
        [-0.3, 0.3, 0.15]
    };
    Arrow {
        back: at(frame_height * back, 0.0),
        front: at(frame_height * front, 0.0),
        left: at(frame_height * sides, frame_width * 0.15),
        right: at(frame_height * sides, frame_width * -0.15),
    }
    .stroke(context);

    let module_velocities = member(command, "moduleVelocities")?;
    for (index, corner) in [[1.0, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]]
        .into_iter()
        .enumerate()
    {
        let module_center = at(half_height * corner[0], half_width * corner[1]);
        let sets = array(module_velocities, "forEach")?;
        for set in sets {
            let values = array(member(set, "values")?, "length")?;
            if let Some(state) = values.get(index) {
                draw_module(
                    context,
                    state,
                    member(set, "color")?,
                    &Module {
                        center: module_center,
                        radius: module_radius,
                        full_vector,
                        rotation,
                        full_circle: sets.len() >= 2,
                    },
                )?;
            }
        }

        context.set_stroke_style(stroke_color);
        context.set_line_width(4.0);
        context.begin_path();
        arc(
            context,
            module_center,
            module_radius,
            0.0,
            math::PI * 2.0,
            None,
        )?;
        context.stroke();
    }

    for speed in array(robot_velocities, "forEach")? {
        context.set_stroke_style(canvas_color(member(speed, "color")?)?);
        context.set_line_width(4.0);

        let value = member(speed, "value")?;
        let vy = operand(member(value, "vy")?)?;
        let vx = operand(member(value, "vx")?)?;
        let angle = math::atan2(vy, vx);
        let mut length = math::hypot(&[vx, vy]);
        if length >= 0.05 {
            length *= full_vector;
            Arrow::along(center, rotation + angle, 0.0, length, module_radius * 0.4)
                .stroke(context);
        }

        let omega = operand(member(value, "omega")?)?;
        if math::abs(omega) > 0.1 {
            let radius = frame_width * 0.25;
            context.begin_path();
            arc(
                context,
                center,
                radius,
                -rotation,
                -(rotation + omega),
                Some(omega > 0.0),
            )?;
            let tip = rotation + omega;
            let head = rotation + omega - 0.3 * math::sign(omega);
            let arrow = Arrow {
                back: center,
                front: transform_px(center, tip, [radius, 0.0]),
                left: transform_px(center, head, [radius - module_radius * 0.4, 0.0]),
                right: transform_px(center, head, [radius + module_radius * 0.4, 0.0]),
            };
            context.line_to(arrow.front[0], arrow.front[1]);
            arrow.head(context);
            context.stroke();
        }
    }
    Ok(())
}

struct Module {
    center: [f64; 2],
    radius: f64,
    full_vector: f64,
    rotation: f64,
    full_circle: bool,
}

/// `drawModuleData`.
fn draw_module(
    context: &mut Canvas2d,
    state: &JsValue,
    color: &JsValue,
    module: &Module,
) -> Result<(), JsError> {
    let full_rotation = module.rotation + operand(member(state, "angle")?)?;
    let color = canvas_color(color)?;
    context.set_fill_style(color.clone());
    context.set_stroke_style(color);
    context.set_line_width(4.0);

    context.begin_path();
    let [x, y] = if module.full_circle {
        module.center
    } else {
        transform_px(module.center, full_rotation, [module.radius, 0.0])
    };
    context.move_to(x, y);
    arc(
        context,
        module.center,
        module.radius,
        -wrap_radians(full_rotation - (5.0 * math::PI) / 6.0),
        -wrap_radians(full_rotation + (5.0 * math::PI) / 6.0),
        None,
    )?;
    context.close_path();
    context.fill();

    let velocity = operand(member(state, "velocity")?)?;
    if math::abs(velocity) <= 0.001 {
        return Ok(());
    }
    let mut speed = velocity;
    let mut vector_rotation = full_rotation;
    if velocity < 0.0 {
        speed *= -1.0;
        vector_rotation += math::PI;
    }
    if speed < 0.05 {
        return Ok(());
    }
    let length = module.full_vector * speed;
    Arrow::along(
        module.center,
        vector_rotation,
        module.radius,
        module.radius + length,
        module.radius * 0.4,
    )
    .stroke(context);
    Ok(())
}

impl<Cx: SourceListCx + ?Sized> TabRenderer<Cx> for SwerveRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        Some(1.0)
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        self.render_command(cx.tab_dark(), &command.to_js())
    }
}
