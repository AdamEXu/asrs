use serde_json::Value;
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, StrokeDash,
    Transform,
};

use crate::scenes::Scene;
use crate::text::{FontSpec, TextBackend, baseline_offset};

#[derive(Clone)]
struct State {
    transform: Transform,
    fill: [f32; 4],
    stroke: [f32; 4],
    line_width: f32,
    line_cap: LineCap,
    line_join: LineJoin,
    dash: Vec<f32>,
    alpha: f32,
    font: FontSpec,
    align: String,
    baseline: String,
}

pub fn parse_color(s: &str) -> [f32; 4] {
    let named = match s {
        "yellow" => Some("#ffff00"),
        "white" => Some("#ffffff"),
        "black" => Some("#000000"),
        "red" => Some("#ff0000"),
        _ => None,
    };
    let s = named.unwrap_or(s);
    let hex = s.trim_start_matches('#');
    let v: Vec<u8> = if hex.len() == 3 {
        hex.chars()
            .map(|c| u8::from_str_radix(&format!("{c}{c}"), 16).unwrap())
            .collect()
    } else {
        (0..3).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect()
    };
    [v[0] as f32 / 255.0, v[1] as f32 / 255.0, v[2] as f32 / 255.0, 1.0]
}

pub fn parse_font(s: &str) -> FontSpec {
    let mut weight = 400u16;
    let mut size = 10.0;
    for tok in s.split_whitespace() {
        if let Some(px) = tok.strip_suffix("px") {
            size = px.parse().unwrap();
            break;
        } else if let Ok(w) = tok.parse::<u16>() {
            weight = w;
        }
    }
    FontSpec { size, weight }
}

pub struct Canvas<'a> {
    pub pixmap: Pixmap,
    state: State,
    stack: Vec<State>,
    path: PathBuilder,
    text: &'a mut dyn TextBackend,
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

impl<'a> Canvas<'a> {
    pub fn new(width: u32, height: u32, text: &'a mut dyn TextBackend) -> Self {
        Canvas {
            pixmap: Pixmap::new(width, height).unwrap(),
            state: State {
                transform: Transform::identity(),
                fill: [0.0, 0.0, 0.0, 1.0],
                stroke: [0.0, 0.0, 0.0, 1.0],
                line_width: 1.0,
                line_cap: LineCap::Butt,
                line_join: LineJoin::Miter,
                dash: Vec::new(),
                alpha: 1.0,
                font: parse_font("10px sans-serif"),
                align: "start".into(),
                baseline: "alphabetic".into(),
            },
            stack: Vec::new(),
            path: PathBuilder::new(),
            text,
        }
    }

    pub fn run(scene: &Scene, text: &'a mut dyn TextBackend) -> Pixmap {
        let w = (scene.width as f64 * scene.dpr) as u32;
        let h = (scene.height as f64 * scene.dpr) as u32;
        let mut c = Canvas::new(w, h, text);
        for cmd in &scene.cmds {
            c.exec(cmd.as_array().unwrap());
        }
        c.pixmap
    }

    fn paint(&self, color: [f32; 4]) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color_rgba8(
            (color[0] * 255.0).round() as u8,
            (color[1] * 255.0).round() as u8,
            (color[2] * 255.0).round() as u8,
            (color[3] * self.state.alpha * 255.0).round() as u8,
        );
        p.anti_alias = true;
        p
    }

    fn stroke_style(&self) -> Stroke {
        let mut s = Stroke {
            width: self.state.line_width,
            line_cap: self.state.line_cap,
            line_join: self.state.line_join,
            miter_limit: 10.0,
            dash: None,
        };
        if !self.state.dash.is_empty() {
            let mut d = self.state.dash.clone();
            if d.len() % 2 == 1 {
                d.extend(self.state.dash.clone());
            }
            s.dash = StrokeDash::new(d, 0.0);
        }
        s
    }

    fn stroke(&mut self, path: &tiny_skia::Path, paint: &Paint, stroke: &Stroke) {
        if std::env::var_os("S2_HAIRLINE").is_some() {
            self.pixmap.stroke_path(path, paint, stroke, self.state.transform, None);
            return;
        }
        let res_scale = tiny_skia::PathStroker::compute_resolution_scale(&self.state.transform);
        let dashed;
        let path = match &stroke.dash {
            Some(d) => match path.dash(d, res_scale) {
                Some(p) => {
                    dashed = p;
                    &dashed
                }
                None => return,
            },
            None => path,
        };
        if let Some(outline) = path.stroke(stroke, res_scale) {
            self.pixmap.fill_path(&outline, paint, FillRule::Winding, self.state.transform, None);
        }
    }

    fn exec(&mut self, cmd: &[Value]) {
        let name = cmd[0].as_str().unwrap();
        let a = &cmd[1..];
        match name {
            "=" => self.set(a[0].as_str().unwrap(), &a[1]),
            "scale" => {
                self.state.transform = self.state.transform.pre_scale(num(&a[0]) as f32, num(&a[1]) as f32)
            }
            "translate" => {
                self.state.transform =
                    self.state.transform.pre_translate(num(&a[0]) as f32, num(&a[1]) as f32)
            }
            "save" => self.stack.push(self.state.clone()),
            "restore" => {
                if let Some(s) = self.stack.pop() {
                    self.state = s;
                }
            }
            "beginPath" => self.path = PathBuilder::new(),
            "moveTo" => self.path.move_to(num(&a[0]) as f32, num(&a[1]) as f32),
            "lineTo" => {
                if self.path.last_point().is_none() {
                    self.path.move_to(num(&a[0]) as f32, num(&a[1]) as f32);
                } else {
                    self.path.line_to(num(&a[0]) as f32, num(&a[1]) as f32)
                }
            }
            "closePath" => self.path.close(),
            "rect" => {
                let (x, y, w, h) = (num(&a[0]) as f32, num(&a[1]) as f32, num(&a[2]) as f32, num(&a[3]) as f32);
                self.path.move_to(x, y);
                self.path.line_to(x + w, y);
                self.path.line_to(x + w, y + h);
                self.path.line_to(x, y + h);
                self.path.close();
            }
            "arc" => self.arc(
                num(&a[0]),
                num(&a[1]),
                num(&a[2]),
                num(&a[3]),
                num(&a[4]),
                a.get(5).and_then(Value::as_bool).unwrap_or(false),
            ),
            "fill" => {
                if let Some(path) = self.path.clone().finish() {
                    let paint = self.paint(self.state.fill);
                    self.pixmap.fill_path(&path, &paint, FillRule::Winding, self.state.transform, None);
                }
            }
            "stroke" => {
                if let Some(path) = self.path.clone().finish() {
                    let paint = self.paint(self.state.stroke);
                    let stroke = self.stroke_style();
                    self.stroke(&path, &paint, &stroke);
                }
            }
            "fillRect" | "clearRect" | "strokeRect" => {
                let (x, y, w, h) = (num(&a[0]) as f32, num(&a[1]) as f32, num(&a[2]) as f32, num(&a[3]) as f32);
                let Some(rect) = Rect::from_xywh(x.min(x + w), y.min(y + h), w.abs(), h.abs()) else {
                    return;
                };
                match name {
                    "fillRect" => {
                        let paint = self.paint(self.state.fill);
                        self.pixmap.fill_rect(rect, &paint, self.state.transform, None);
                    }
                    "clearRect" => {
                        let mut paint = Paint::default();
                        paint.blend_mode = BlendMode::Clear;
                        paint.anti_alias = true;
                        self.pixmap.fill_rect(rect, &paint, self.state.transform, None);
                    }
                    _ => {
                        let path = PathBuilder::from_rect(rect);
                        let paint = self.paint(self.state.stroke);
                        let stroke = self.stroke_style();
                        self.stroke(&path, &paint, &stroke);
                    }
                }
            }
            "setLineDash" => {
                self.state.dash = a[0].as_array().unwrap().iter().map(|v| num(v) as f32).collect()
            }
            "fillText" => {
                let text = a[0].as_str().unwrap().to_owned();
                let max_width = a.get(3).map(num);
                self.fill_text(&text, num(&a[1]), num(&a[2]), max_width);
            }
            other => panic!("unsupported call {other}"),
        }
    }

    fn set(&mut self, prop: &str, v: &Value) {
        match prop {
            "fillStyle" => self.state.fill = parse_color(v.as_str().unwrap()),
            "strokeStyle" => self.state.stroke = parse_color(v.as_str().unwrap()),
            "lineWidth" => self.state.line_width = num(v) as f32,
            "globalAlpha" => self.state.alpha = num(v) as f32,
            "lineCap" => {
                self.state.line_cap = match v.as_str().unwrap() {
                    "round" => LineCap::Round,
                    "square" => LineCap::Square,
                    _ => LineCap::Butt,
                }
            }
            "lineJoin" => {
                self.state.line_join = match v.as_str().unwrap() {
                    "round" => LineJoin::Round,
                    "bevel" => LineJoin::Bevel,
                    _ => LineJoin::Miter,
                }
            }
            "font" => self.state.font = parse_font(v.as_str().unwrap()),
            "textAlign" => self.state.align = v.as_str().unwrap().into(),
            "textBaseline" => self.state.baseline = v.as_str().unwrap().into(),
            other => panic!("unsupported property {other}"),
        }
    }

    fn arc(&mut self, cx: f64, cy: f64, r: f64, start: f64, end: f64, ccw: bool) {
        use std::f64::consts::{FRAC_PI_2, TAU};
        let mut sweep = end - start;
        if !ccw {
            sweep = if sweep >= TAU { TAU } else { sweep.rem_euclid(TAU) };
        } else {
            sweep = if sweep <= -TAU { -TAU } else { -(-sweep).rem_euclid(TAU) };
        }
        let p0 = (cx + r * start.cos(), cy + r * start.sin());
        if self.path.last_point().is_none() {
            self.path.move_to(p0.0 as f32, p0.1 as f32);
        } else {
            self.path.line_to(p0.0 as f32, p0.1 as f32);
        }
        let segments = (sweep.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
        let step = sweep / segments as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let mut a0 = start;
        for _ in 0..segments {
            let a1 = a0 + step;
            let (c0, s0, c1, s1) = (a0.cos(), a0.sin(), a1.cos(), a1.sin());
            self.path.cubic_to(
                (cx + r * (c0 - k * s0)) as f32,
                (cy + r * (s0 + k * c0)) as f32,
                (cx + r * (c1 + k * s1)) as f32,
                (cy + r * (s1 - k * c1)) as f32,
                (cx + r * c1) as f32,
                (cy + r * s1) as f32,
            );
            a0 = a1;
        }
    }

    fn fill_text(&mut self, text: &str, x: f64, y: f64, max_width: Option<f64>) {
        let font = self.state.font;
        let shaped = self.text.shape(&font, text);
        let width = shaped.width;
        let compress = match max_width {
            Some(m) if !(m > 0.0) || !m.is_finite() => return,
            Some(m) if width > m => m / width,
            _ => 1.0,
        };
        let drawn_width = width * compress;
        let x = match self.state.align.as_str() {
            "center" => x - drawn_width / 2.0,
            "right" | "end" => x - drawn_width,
            _ => x,
        };
        let y = y + baseline_offset(&font, &self.state.baseline);
        let t = self.state.transform;
        let scale = t.sx as f64;
        let mut color = self.state.fill;
        color[3] *= self.state.alpha;
        for g in &shaped.glyphs {
            let ux = x + g.x * compress;
            let dx = ux * scale + t.tx as f64;
            let dy = y * scale + t.ty as f64;
            let qx = (dx * 4.0 + 0.5).floor() / 4.0;
            let ix = qx.floor();
            let sub = ((qx - ix) * 4.0) as u8;
            let iy = (dy + 0.5).floor();
            let mask = if compress == 1.0 {
                self.text.rasterize(&font, g.id, scale as f32, sub, color)
            } else {
                self.text.rasterize_scaled(&font, g.id, scale as f32, compress as f32, sub, color)
            };
            if let Some(m) = mask {
                self.blit(&m, ix as i32 + m.left, iy as i32 + m.top, color);
            }
        }
    }

    fn blit(&mut self, m: &crate::text::Mask, ox: i32, oy: i32, color: [f32; 4]) {
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data_mut();
        for row in 0..m.height as i32 {
            let py = oy + row;
            if py < 0 || py >= ph {
                continue;
            }
            for col in 0..m.width as i32 {
                let px = ox + col;
                if px < 0 || px >= pw {
                    continue;
                }
                let cov = m.data[(row * m.width as i32 + col) as usize] as f32 / 255.0;
                if cov == 0.0 {
                    continue;
                }
                let sa = color[3] * cov;
                let i = ((py * pw + px) * 4) as usize;
                for ch in 0..3 {
                    let d = data[i + ch] as f32;
                    data[i + ch] = (color[ch] * sa * 255.0 + d * (1.0 - sa)).round() as u8;
                }
                let d = data[i + 3] as f32;
                data[i + 3] = (sa * 255.0 + d * (1.0 - sa)).round() as u8;
            }
        }
    }
}
