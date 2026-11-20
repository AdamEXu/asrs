use std::collections::HashMap;

use asrs_js::string::JsString;
use tiny_skia::{
    FilterQuality, GradientStop, IntSize, LineCap, LineJoin, LinearGradient, Paint, PathBuilder,
    Pattern, Pixmap, Point, RadialGradient, Rect, Shader, SpreadMode, Stroke, StrokeDash,
    Transform,
};

use crate::color::{self, Rgba};
use crate::display::{Call, Gradient, GradientShape, ImageData, ImageDataSource, Property, Style};
use crate::hairline;
use crate::path::{Affine, Path, Pt};
use crate::pixels;
use crate::aaa;
use crate::blit::Spans;
use crate::scan::{self, Blend, Coverage, div255};
use crate::skpath::SkPath;
use crate::stroke;
use crate::text::lut::MaskGammaLut;
use crate::text::{FontSpec, TextBackend, baseline_offset, glyph_origin};

const CONIC_TOLERANCE: f32 = 0.25;

#[derive(Clone, Debug, PartialEq)]
enum Fill {
    Color(Rgba),
    Linear([f64; 4], Vec<(f32, Rgba)>),
    Radial([f64; 6], Vec<(f32, Rgba)>),
}

fn resolve(style: &Style) -> Option<Fill> {
    match style {
        Style::Color(css) => color::parse(css).map(Fill::Color),
        Style::Gradient(Gradient { shape, stops }) => {
            let mut resolved = Vec::new();
            for (offset, css) in stops {
                if !(0.0..=1.0).contains(offset) {
                    continue;
                }
                if let Some(c) = color::parse(css) {
                    resolved.push((*offset as f32, c));
                }
            }
            // Blink keeps stops sorted by offset, equal offsets in insertion order.
            let mut order: Vec<usize> = (0..resolved.len()).collect();
            order.sort_by(|&a, &b| resolved[a].0.total_cmp(&resolved[b].0).then(a.cmp(&b)));
            let resolved = order.into_iter().map(|i| resolved[i]).collect();
            Some(match shape {
                GradientShape::Linear(p) => Fill::Linear(*p, resolved),
                GradientShape::Radial(p) => Fill::Radial(*p, resolved),
            })
        }
    }
}

#[derive(Clone, Debug)]
struct State {
    transform: Affine,
    fill: Fill,
    stroke: Fill,
    line_width: f64,
    line_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f64,
    dash: Vec<f64>,
    dash_offset: f64,
    alpha: f64,
    font: FontSpec,
    align: String,
    baseline: String,
    smoothing: bool,
}

impl Default for State {
    fn default() -> State {
        State {
            transform: Affine::IDENTITY,
            fill: Fill::Color(Rgba::BLACK),
            stroke: Fill::Color(Rgba::BLACK),
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            alpha: 1.0,
            font: FontSpec::parse("10px sans-serif"),
            align: "start".to_owned(),
            baseline: "alphabetic".to_owned(),
            smoothing: true,
        }
    }
}

/// The text half of the raster: a platform glyph backend plus its mask-gamma table.
pub struct TextRaster<'a> {
    pub backend: &'a mut dyn TextBackend,
    pub lut: &'a MaskGammaLut,
}

/// Replays a display list onto a premultiplied tiny-skia pixmap the way Chromium's CPU canvas
/// (Skia) draws it.
pub struct Raster<'a> {
    pixmap: Pixmap,
    state: State,
    stack: Vec<State>,
    path: Path,
    fast: Fast,
    draw_path_count: u32,
    draw_line_count: u32,
    images: HashMap<String, Pixmap>,
    registers: HashMap<String, ImageData>,
    text: Option<TextRaster<'a>>,
}

enum Source {
    Solid([u8; 4]),
    Shader(Shader<'static>),
    Device(Pixmap),
    Image {
        pixmap: Pixmap,
        transform: Affine,
        quality: FilterQuality,
        opacity: f32,
    },
}

/// The only way any stroke reaches the pixmap: dashed, outlined and filled. tiny-skia's
/// hairline stroker (`Pixmap::stroke_path` for device widths up to 1) does not match Skia.
pub fn stroke_outline(
    path: &tiny_skia::Path,
    stroke: &Stroke,
    res_scale: f32,
) -> Option<tiny_skia::Path> {
    let dashed;
    let path = match &stroke.dash {
        Some(dash) => {
            dashed = path.dash(dash, res_scale)?;
            &dashed
        }
        None => path,
    };
    let solid = Stroke {
        dash: None,
        ..stroke.clone()
    };
    path.stroke(&solid, res_scale)
}

fn pt32(x: f64, y: f64) -> Pt {
    Pt {
        x: x as f32,
        y: y as f32,
    }
}

/// What Blink's `CanvasPath` line and arc builders hold besides the general path.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fast {
    None,
    Start(Pt),
    Line(Pt, Pt),
    Arc(FastArc),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FastArc {
    x: f32,
    y: f32,
    r: f32,
    start: f32,
    sweep: f32,
    closed: bool,
}

impl FastArc {
    /// The path `cc::DrawArcImpl` and `SkDevice::drawArc` draw for this arc.
    fn path(&self, fill: bool) -> Path {
        let oval = [
            self.x - self.r,
            self.y - self.r,
            self.x + self.r,
            self.y + self.r,
        ];
        let degrees = |v: f32| {
            let d = v * 180.0 / std::f32::consts::PI;
            if d.is_finite() { d } else { 0.0 }
        };
        let (mut start, mut sweep) = (degrees(self.start), degrees(self.sweep));
        let mut path = Path::new();
        if self.closed {
            if (sweep.abs() - 360.0).abs() <= 1.0 / 4096.0 {
                path.add_oval(oval);
            } else {
                path.arc_to_oval(oval, start, sweep, false);
                path.close();
            }
            return path;
        }
        if oval[2] - oval[0] <= 0.0 || oval[3] - oval[1] <= 0.0 || sweep == 0.0 {
            return path;
        }
        if fill && sweep.abs() >= 360.0 {
            path.add_oval(oval);
            return path;
        }
        if sweep.abs() > 3600.0 {
            sweep = 3600.0f32.copysign(sweep) + sweep % 360.0;
        }
        let mut force = true;
        while sweep <= -360.0 {
            path.arc_to_oval(oval, start, -180.0, force);
            start -= 180.0;
            path.arc_to_oval(oval, start, -180.0, false);
            start -= 180.0;
            force = false;
            sweep += 360.0;
        }
        while sweep >= 360.0 {
            path.arc_to_oval(oval, start, 180.0, force);
            start += 180.0;
            path.arc_to_oval(oval, start, 180.0, false);
            start += 180.0;
            force = false;
            sweep -= 360.0;
        }
        path.arc_to_oval(oval, start, sweep, force);
        path
    }
}

struct DashPoints {
    first: Option<[f32; 4]>,
    last: Option<[f32; 4]>,
    points: Vec<Pt>,
    size: (f32, f32),
}

/// `SkDashPath::CalcDashParameters`: the length left in the dash the phase lands in, and its
/// index.
fn dash_parameters(intervals: &[f32], phase: f32, len: f32) -> (f32, usize) {
    let mut phase = phase;
    if phase < 0.0 {
        phase = -phase;
        if phase > len {
            phase %= len;
        }
        phase = len - phase;
        if phase == len {
            phase = 0.0;
        }
    } else if phase >= len {
        phase %= len;
    }
    for (i, &gap) in intervals.iter().enumerate() {
        if phase > gap || (phase == gap && gap != 0.0) {
            phase -= gap;
        } else {
            return (gap - phase, i);
        }
    }
    (intervals[0], 0)
}

fn finite(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// Blink's `ValidateRectForCanvas`: non-finite or 0x0 rects are dropped, negative sizes flip.
fn validate_rect(x: f64, y: f64, w: f64, h: f64) -> Option<[f64; 4]> {
    if !finite(&[x, y, w, h]) || (w == 0.0 && h == 0.0) {
        return None;
    }
    let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
    let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
    Some([x, y, w, h])
}

fn to_int32(v: f64) -> i64 {
    if v.is_finite() { v.trunc() as i64 } else { 0 }
}

impl<'a> Raster<'a> {
    pub fn new(width: u32, height: u32) -> Option<Raster<'a>> {
        Some(Raster {
            pixmap: Pixmap::new(width, height)?,
            state: State::default(),
            stack: Vec::new(),
            path: Path::new(),
            fast: Fast::None,
            draw_path_count: 0,
            draw_line_count: 0,
            images: HashMap::new(),
            registers: HashMap::new(),
            text: None,
        })
    }

    pub fn with_text(mut self, text: TextRaster<'a>) -> Raster<'a> {
        self.text = Some(text);
        self
    }

    /// Makes `pixmap` (premultiplied) available to `drawImage` as `id`.
    pub fn add_image(&mut self, id: &str, pixmap: Pixmap) {
        self.images.insert(id.to_owned(), pixmap);
    }

    pub fn register(&self, name: &str) -> Option<&ImageData> {
        self.registers.get(name)
    }

    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }

    pub fn into_pixmap(self) -> Pixmap {
        self.pixmap
    }

    pub fn run(&mut self, calls: &[Call]) {
        for call in calls {
            self.apply(call);
        }
    }

    pub fn apply(&mut self, call: &Call) {
        match call {
            Call::Set(p) => self.set(p),
            Call::Save => self.stack.push(self.state.clone()),
            Call::Restore => self.restore(),
            &Call::Scale(x, y) => {
                if finite(&[x, y]) {
                    self.set_transform(self.state.transform.then(&Affine::scaling(x, y)));
                }
            }
            &Call::Rotate(angle) => {
                if angle.is_finite() {
                    self.set_transform(self.state.transform.then(&Affine::rotation(angle)));
                }
            }
            &Call::Translate(x, y) => {
                if finite(&[x, y]) {
                    self.set_transform(self.state.transform.then(&Affine::translation(x, y)));
                }
            }
            Call::BeginPath => {
                self.path = Path::new();
                self.fast = Fast::None;
            }
            Call::ClosePath => self.close_path(),
            &Call::MoveTo(x, y) => {
                if self.invertible() {
                    if finite(&[x, y]) {
                        self.fast = if self.path.is_empty() {
                            Fast::Start(pt32(x, y))
                        } else {
                            Fast::None
                        };
                    }
                    self.path.move_to(x, y);
                }
            }
            &Call::LineTo(x, y) => {
                if self.invertible() {
                    self.line_to(x, y);
                }
            }
            &Call::Rect(x, y, w, h) => {
                if self.invertible() {
                    if finite(&[x, y, w, h]) {
                        self.fast = Fast::None;
                    }
                    self.path.rect(x, y, w, h);
                }
            }
            &Call::Arc {
                x,
                y,
                radius,
                start,
                end,
                anticlockwise,
            } => {
                if self.invertible() {
                    self.arc(x, y, radius, start, end, anticlockwise.unwrap_or(false));
                }
            }
            &Call::Ellipse {
                x,
                y,
                radius_x,
                radius_y,
                rotation,
                start,
                end,
                anticlockwise,
            } => {
                if self.invertible() {
                    if finite(&[x, y, radius_x, radius_y, rotation, start, end])
                        && radius_x >= 0.0
                        && radius_y >= 0.0
                    {
                        self.fast = Fast::None;
                    }
                    self.path.ellipse(
                        x,
                        y,
                        radius_x,
                        radius_y,
                        rotation,
                        start,
                        end,
                        anticlockwise.unwrap_or(false),
                    );
                }
            }
            Call::Fill => self.fill_current_path(),
            Call::Stroke => self.stroke_current_path(),
            &Call::FillRect(x, y, w, h) => self.fill_rect(x, y, w, h),
            &Call::StrokeRect(x, y, w, h) => self.stroke_rect(x, y, w, h),
            &Call::ClearRect(x, y, w, h) => self.clear_rect(x, y, w, h),
            Call::FillText {
                text,
                x,
                y,
                max_width,
            } => self.fill_text(text, *x, *y, *max_width),
            Call::SetLineDash(segments) => {
                if segments.iter().all(|v| v.is_finite() && *v >= 0.0) {
                    let mut dash = segments.clone();
                    if dash.len() % 2 == 1 {
                        dash.extend_from_slice(segments);
                    }
                    self.state.dash = dash;
                }
            }
            Call::DrawImage { image, args } => self.draw_image(image, args),
            &Call::GetImageData {
                sx,
                sy,
                sw,
                sh,
                ref register,
            } => {
                let (mut x, mut y, mut w, mut h) =
                    (to_int32(sx), to_int32(sy), to_int32(sw), to_int32(sh));
                if w == 0 || h == 0 {
                    return;
                }
                if w < 0 {
                    x += w;
                    w = -w;
                }
                if h < 0 {
                    y += h;
                    h = -h;
                }
                let data = pixels::read_rect(&self.pixmap, x, y, w as u32, h as u32);
                self.registers.insert(register.clone(), data);
            }
            Call::PutImageData { data, dx, dy } => {
                let image = match data {
                    ImageDataSource::Register(name) => self.registers.get(name).cloned(),
                    ImageDataSource::Inline(image) => Some(image.clone()),
                };
                if let Some(image) = image {
                    pixels::write_rect(&mut self.pixmap, &image, to_int32(*dx), to_int32(*dy));
                }
            }
        }
    }

    fn set(&mut self, property: &Property) {
        let s = &mut self.state;
        match property {
            Property::FillStyle(style) => {
                if let Some(fill) = resolve(style) {
                    s.fill = fill;
                }
            }
            Property::StrokeStyle(style) => {
                if let Some(fill) = resolve(style) {
                    s.stroke = fill;
                }
            }
            &Property::LineWidth(w) => {
                if w.is_finite() && w > 0.0 {
                    s.line_width = w;
                }
            }
            Property::LineCap(cap) => match cap.as_str() {
                "butt" => s.line_cap = LineCap::Butt,
                "round" => s.line_cap = LineCap::Round,
                "square" => s.line_cap = LineCap::Square,
                _ => {}
            },
            Property::LineJoin(join) => match join.as_str() {
                "miter" => s.line_join = LineJoin::Miter,
                "round" => s.line_join = LineJoin::Round,
                "bevel" => s.line_join = LineJoin::Bevel,
                _ => {}
            },
            &Property::MiterLimit(limit) => {
                if limit.is_finite() && limit > 0.0 {
                    s.miter_limit = limit;
                }
            }
            &Property::LineDashOffset(offset) => {
                if offset.is_finite() {
                    s.dash_offset = offset;
                }
            }
            &Property::GlobalAlpha(alpha) => {
                if (0.0..=1.0).contains(&alpha) {
                    s.alpha = alpha;
                }
            }
            Property::Font(css) => s.font = FontSpec::parse(css),
            Property::TextAlign(align) => {
                if ["start", "end", "left", "right", "center"].contains(&align.as_str()) {
                    s.align.clone_from(align);
                }
            }
            Property::TextBaseline(baseline) => {
                let known = [
                    "top",
                    "hanging",
                    "middle",
                    "alphabetic",
                    "ideographic",
                    "bottom",
                ];
                if known.contains(&baseline.as_str()) {
                    s.baseline.clone_from(baseline);
                }
            }
            &Property::ImageSmoothingEnabled(on) => s.smoothing = on,
        }
    }

    fn invertible(&self) -> bool {
        self.state.transform.is_invertible()
    }

    /// Blink keeps the current path in the current user space: a transform change maps the
    /// path by the inverse of the change.
    fn set_transform(&mut self, new: Affine) {
        let old = self.state.transform;
        if old == new {
            return;
        }
        self.state.transform = new;
        if new.is_invertible() {
            self.fast = Fast::None;
        }
        if let Some(inverse) = new.inverse()
            && old.is_invertible()
        {
            self.path.transform(&inverse.then(&old));
        }
    }

    fn restore(&mut self) {
        let Some(previous) = self.stack.pop() else {
            return;
        };
        if self.invertible() {
            if self.state.transform != Affine::IDENTITY {
                self.fast = Fast::None;
            }
            self.path.transform(&self.state.transform);
        }
        self.state = previous;
        if let Some(inverse) = self.state.transform.inverse() {
            if self.state.transform != Affine::IDENTITY {
                self.fast = Fast::None;
            }
            self.path.transform(&inverse);
        }
    }

    /// Blink's `CanvasPath::lineTo`: a lone `moveTo` (or nothing) followed by one `lineTo` is
    /// remembered as a line, which `stroke()` draws with `drawLine`.
    fn line_to(&mut self, x: f64, y: f64) {
        if !finite(&[x, y]) {
            return;
        }
        let p = pt32(x, y);
        self.fast = match self.fast {
            Fast::None if self.path.is_empty() => Fast::Line(p, p),
            Fast::Start(s) => Fast::Line(s, p),
            _ => Fast::None,
        };
        self.path.line_to(x, y);
    }

    fn close_path(&mut self) {
        match self.fast {
            Fast::Line(a, b) if a == b => {
                self.path = Path::new();
                self.fast = Fast::None;
                return;
            }
            Fast::Arc(mut arc) => {
                arc.closed = true;
                self.fast = Fast::Arc(arc);
            }
            _ => self.fast = Fast::None,
        }
        self.path.close();
    }

    /// Blink's `CanvasPath::arc`: an arc of radius at least 1 on an empty path is remembered as
    /// an arc, which `fill()` and `stroke()` draw with `drawArc`.
    fn arc(&mut self, x: f64, y: f64, radius: f64, start: f64, end: f64, ccw: bool) {
        let v = [x, y, radius, start, end].map(|v| v as f32);
        if !v.iter().all(|v| v.is_finite()) || v[2] < 0.0 {
            return;
        }
        let [xf, yf, r, s, e] = v;
        let was_empty = self.path.is_empty();
        self.fast = Fast::None;
        if r == 0.0 || s == e {
            if was_empty {
                let p = Pt {
                    x: xf + r * s.cos(),
                    y: yf + r * s.sin(),
                };
                self.fast = Fast::Line(p, p);
            }
            self.path.arc(x, y, radius, start, end, ccw);
            return;
        }
        let (s, e) = crate::path::arc_angles(s, e, ccw);
        if was_empty && r >= 1.0 {
            self.fast = Fast::Arc(FastArc {
                x: xf,
                y: yf,
                r,
                start: s,
                sweep: e - s,
                closed: false,
            });
        }
        self.path.arc(x, y, radius, start, end, ccw);
    }

    /// `cc::RecordPaintCanvas::drawPath` bookkeeping for its lines-as-paths heuristic.
    fn count_draw_path(&mut self) {
        if self.draw_path_count != u32::MAX {
            self.draw_path_count += 1;
            if self.draw_path_count > 4 {
                self.draw_line_count = 0;
            }
        }
    }

    /// The paint source for `fill` with the paint alpha scaled by `coverage_scale / 256`, the
    /// way `SkDraw::drawPath` modulates a thin stroke drawn as a hairline.
    fn source(&self, fill: &Fill, coverage_scale: u32) -> Option<Source> {
        let alpha = self.state.alpha as f32;
        let ctm = self.state.transform;
        let modulate = |a: f32| {
            if coverage_scale >= 256 {
                a
            } else {
                let a8 = (a * 255.0 + 0.5) as u32;
                ((a8 * coverage_scale) >> 8) as f32 / 255.0
            }
        };
        let stops = |stops: &[(f32, Rgba)]| -> Vec<GradientStop> {
            let scale = modulate(1.0);
            stops
                .iter()
                .map(|&(offset, c)| {
                    let c = c.with_alpha_scaled(alpha * scale);
                    Some(GradientStop::new(
                        offset,
                        tiny_skia::Color::from_rgba(c.r, c.g, c.b, c.a)?,
                    ))
                })
                .collect::<Option<_>>()
                .unwrap_or_default()
        };
        let point = |x: f64, y: f64| Point::from_xy(x as f32, y as f32);
        match fill {
            Fill::Color(c) => {
                let a = modulate(c.a * alpha);
                Some(Source::Solid(
                    [c.r * a, c.g * a, c.b * a, a].map(|v| (v * 255.0 + 0.5) as u8),
                ))
            }
            Fill::Linear(_, s) | Fill::Radial(_, s) if s.is_empty() => None,
            Fill::Linear([x0, y0, x1, y1], s) => {
                if x0 == x1 && y0 == y1 {
                    return None;
                }
                LinearGradient::new(
                    point(*x0, *y0),
                    point(*x1, *y1),
                    stops(s),
                    SpreadMode::Pad,
                    ctm.to_skia(),
                )
                .map(Source::Shader)
            }
            &Fill::Radial([x0, y0, r0, x1, y1, r1], ref s) => {
                if r0 < 0.0 || r1 < 0.0 || (x0 == x1 && y0 == y1 && r0 == r1) {
                    return None;
                }
                if r0 == 0.0 {
                    return RadialGradient::new(
                        point(x0, y0),
                        point(x1, y1),
                        r1 as f32,
                        stops(s),
                        SpreadMode::Pad,
                        ctm.to_skia(),
                    )
                    .map(Source::Shader);
                }
                if x0 == x1 && y0 == y1 && r1 > r0 {
                    let k = r0 / r1;
                    let mut remapped: Vec<(f32, Rgba)> = vec![(0.0, s[0].1)];
                    remapped.extend(
                        s.iter()
                            .map(|&(o, c)| ((k + f64::from(o) * (1.0 - k)) as f32, c)),
                    );
                    return RadialGradient::new(
                        point(x0, y0),
                        point(x1, y1),
                        r1 as f32,
                        stops(&remapped),
                        SpreadMode::Pad,
                        ctm.to_skia(),
                    )
                    .map(Source::Shader);
                }
                self.conical_pixmap([x0, y0, r0, x1, y1, r1], s, modulate(1.0))
                    .map(Source::Device)
            }
        }
    }

    /// The canvas-spec two-point conical gradient evaluated at every device pixel centre, for
    /// the start-radius cases tiny-skia's focal gradient cannot express.
    fn conical_pixmap(&self, g: [f64; 6], stops: &[(f32, Rgba)], scale: f32) -> Option<Pixmap> {
        let inverse = self.state.transform.inverse()?;
        let [x0, y0, r0, x1, y1, r1] = g;
        let (cdx, cdy, dr) = (x1 - x0, y1 - y0, r1 - r0);
        let a = cdx * cdx + cdy * cdy - dr * dr;
        let alpha = self.state.alpha as f32 * scale;
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        let mut out = Pixmap::new(w, h)?;
        let data = out.data_mut();
        for py in 0..h {
            for px in 0..w {
                let (dx, dy) = (f64::from(px) + 0.5, f64::from(py) + 0.5);
                let ux = inverse.a * dx + inverse.c * dy + inverse.e - x0;
                let uy = inverse.b * dx + inverse.d * dy + inverse.f - y0;
                let b = ux * cdx + uy * cdy + r0 * dr;
                let c = ux * ux + uy * uy - r0 * r0;
                let t = if a.abs() < 1e-12 {
                    if b == 0.0 {
                        continue;
                    }
                    c / (2.0 * b)
                } else {
                    let disc = b * b - a * c;
                    if disc < 0.0 {
                        continue;
                    }
                    let root = disc.sqrt();
                    let (t1, t2) = ((b + root) / a, (b - root) / a);
                    let (hi, lo) = if t1 > t2 { (t1, t2) } else { (t2, t1) };
                    if r0 + hi * dr >= 0.0 {
                        hi
                    } else if r0 + lo * dr >= 0.0 {
                        lo
                    } else {
                        continue;
                    }
                };
                let c = sample_stops(stops, t as f32).with_alpha_scaled(alpha);
                let i = ((py * w + px) * 4) as usize;
                data[i..i + 4].copy_from_slice(&pixels::premultiply([
                    (c.r * 255.0 + 0.5) as u8,
                    (c.g * 255.0 + 0.5) as u8,
                    (c.b * 255.0 + 0.5) as u8,
                    (c.a * 255.0 + 0.5) as u8,
                ]));
            }
        }
        Some(out)
    }

    fn paint(&mut self, coverage: &Coverage, source: &Source) {
        let spans = coverage.to_spans(self.pixmap.width(), self.pixmap.height());
        self.paint_spans(&spans, source);
    }

    /// Blends `source` through `spans` in blit order. Shaders are evaluated over the bounds of
    /// the spans into a scratch pixmap first, then blended with the same 8-bit arithmetic as
    /// solid colours.
    fn paint_spans(&mut self, spans: &Spans, source: &Source) {
        let shader = match source {
            Source::Solid(color) => {
                scan::blit_spans(&mut self.pixmap, spans, Blend::SourceOver, |_, _| *color);
                return;
            }
            Source::Device(pixmap) => {
                let stride = pixmap.width() as usize;
                let data = pixmap.data();
                scan::blit_spans(&mut self.pixmap, spans, Blend::SourceOver, |x, y| {
                    let i = (y * stride + x) * 4;
                    [data[i], data[i + 1], data[i + 2], data[i + 3]]
                });
                return;
            }
            Source::Shader(shader) => shader.clone(),
            Source::Image {
                pixmap,
                transform,
                quality,
                opacity,
            } => Pattern::new(
                pixmap.as_ref(),
                SpreadMode::Pad,
                *quality,
                *opacity,
                transform.to_skia(),
            ),
        };
        let Some(bounds) = spans.bounds() else {
            return;
        };
        let Some(scratch) = shade(bounds, shader) else {
            return;
        };
        let stride = scratch.width() as usize;
        let data = scratch.data();
        let (ox, oy) = (bounds[0] as usize, bounds[1] as usize);
        scan::blit_spans(&mut self.pixmap, spans, Blend::SourceOver, |x, y| {
            let i = ((y - oy) * stride + (x - ox)) * 4;
            [data[i], data[i + 1], data[i + 2], data[i + 3]]
        });
    }

    /// Fills a device path with Skia's analytic AA.
    fn fill_sk(&mut self, path: &SkPath, fill: &Fill) {
        let Some(source) = self.source(fill, 256) else {
            return;
        };
        let mut spans = Spans::new(self.pixmap.width(), self.pixmap.height());
        aaa::fill_path(path, self.pixmap.width(), self.pixmap.height(), false, &mut spans);
        self.paint_spans(&spans, &source);
    }

    /// Fills a device-space path: Skia's analytic AA, or with `snap` off the exact area that
    /// its rect scanners (`AntiFillRect`, `AntiFrameRect`) produce.
    fn fill_device(&mut self, path: &tiny_skia::Path, fill: &Fill, snap: bool) {
        let Some(source) = self.source(fill, 256) else {
            return;
        };
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        if snap {
            let spans = {
                let mut spans = Spans::new(w, h);
                aaa::fill_path(&SkPath::from_tiny(path), w, h, false, &mut spans);
                spans
            };
            self.paint_spans(&spans, &source);
        } else if let Some(coverage) = scan::rasterize(path, w, h) {
            self.paint(&coverage, &source);
        }
    }

    fn fill_current_path(&mut self) {
        if !self.invertible() {
            return;
        }
        match self.fast {
            Fast::Line(..) => return,
            Fast::Arc(arc) => {
                self.fill_arc(arc);
                return;
            }
            _ => self.count_draw_path(),
        }
        let ctm = self.state.transform;
        let device = self.path.to_sk(Some(&ctm));
        let fill = self.state.fill.clone();
        self.fill_sk(&device, &fill);
    }

    fn dash(&self) -> Option<StrokeDash> {
        let s = &self.state;
        if s.dash.is_empty() {
            return None;
        }
        StrokeDash::new(
            s.dash.iter().map(|&v| v as f32).collect(),
            s.dash_offset as f32,
        )
    }

    fn stroke_style(&self) -> Stroke {
        let s = &self.state;
        Stroke {
            width: s.line_width as f32,
            miter_limit: s.miter_limit as f32,
            line_cap: s.line_cap,
            line_join: s.line_join,
            dash: self.dash(),
        }
    }

    /// `SkDrawTreatAsHairline`: a stroke whose width maps to at most one device pixel on both
    /// axes is drawn one pixel wide with its alpha scaled by the mean mapped width.
    fn hairline_coverage(&self) -> Option<f32> {
        let m = self.state.transform.to_skia();
        let w = self.state.line_width as f32;
        let fast_len = |x: f32, y: f32| {
            let (x, y) = (x.abs(), y.abs());
            let (big, small) = if x < y { (y, x) } else { (x, y) };
            big + small * 0.5
        };
        let len0 = fast_len(m.sx * w, m.ky * w);
        let len1 = fast_len(m.kx * w, m.sy * w);
        (len0 <= 1.0 && len1 <= 1.0).then_some((len0 + len1) * 0.5)
    }

    fn stroke_user_path(&mut self, path: &Path) {
        if !self.invertible() {
            return;
        }
        let ctm = self.state.transform.to_skia();
        let res_scale = tiny_skia::PathStroker::compute_resolution_scale(&ctm);
        let Some(user) = path.to_skia(None, CONIC_TOLERANCE / res_scale) else {
            return;
        };
        let stroke = self.state.stroke.clone();
        if let Some(coverage) = self.hairline_coverage() {
            let dashed = match self.dash() {
                Some(dash) => user.dash(&dash, res_scale),
                None => Some(user),
            };
            let Some(device) = dashed.and_then(|p| p.transform(ctm)) else {
                return;
            };
            let Some(source) = self.source(&stroke, (coverage * 256.0) as u32) else {
                return;
            };
            let (w, h) = (self.pixmap.width(), self.pixmap.height());
            for (a, b) in hairline::segments(&device, self.state.line_cap) {
                if let Some(coverage) = hairline::coverage(a, b, w, h) {
                    self.paint(&coverage, &source);
                }
            }
            return;
        }
        let source = match self.dash() {
            Some(dash) => match user.dash(&dash, res_scale) {
                Some(dashed) => SkPath::from_tiny(&dashed),
                None => return,
            },
            None => path.to_sk(None),
        };
        let s = &self.state;
        let style = stroke::Style {
            width: s.line_width as f32,
            miter_limit: s.miter_limit as f32,
            cap: s.line_cap,
            join: s.line_join,
            res_scale,
        };
        let outline = stroke::stroke_path(&source, &style);
        let device = outline.transformed(&self.state.transform);
        self.fill_sk(&device, &stroke);
    }

    fn stroke_current_path(&mut self) {
        if self.invertible() {
            match self.fast {
                Fast::Line(a, b) => {
                    if self.stroke_line(a, b) {
                        return;
                    }
                }
                Fast::Arc(arc) => {
                    self.stroke_user_path(&arc.path(false));
                    return;
                }
                _ => self.count_draw_path(),
            }
        }
        let path = std::mem::take(&mut self.path);
        self.stroke_user_path(&path);
        self.path = path;
    }

    /// `cc::DrawArcOp` filling: `drawOval` for closed or full circles (`addOval`), otherwise
    /// `SkPathPriv::CreateDrawArcPath`.
    fn fill_arc(&mut self, arc: FastArc) {
        let path = arc.path(true);
        let device = path.to_sk(Some(&self.state.transform));
        let fill = self.state.fill.clone();
        self.fill_sk(&device, &fill);
    }

    /// `cc::RecordPaintCanvas::drawLine` then `SkDraw::drawPoints`: once more than four paths
    /// were drawn since the last run of five lines, cc replays the line as a path; otherwise a
    /// dashed line that `SkDashPathEffect::asPoints` accepts becomes rects. Returns false when
    /// the line is to be stroked as an ordinary path.
    fn stroke_line(&mut self, a: Pt, b: Pt) -> bool {
        if self.draw_line_count != u32::MAX {
            self.draw_line_count += 1;
            if self.draw_line_count > 4 {
                self.draw_path_count = 0;
            }
        }
        if self.draw_path_count > 4 {
            return false;
        }
        let Some(points) = self.dash_as_points(a, b) else {
            return false;
        };
        let stroke = self.state.stroke.clone();
        let ctm = self.state.transform;
        for rect in [points.first, points.last].into_iter().flatten() {
            let mut path = Path::new();
            path.rect(
                f64::from(rect[0]),
                f64::from(rect[1]),
                f64::from(rect[2] - rect[0]),
                f64::from(rect[3] - rect[1]),
            );
            let device = path.to_sk(Some(&ctm));
            self.fill_sk(&device, &stroke);
        }
        let Some(source) = self.source(&stroke, 256) else {
            return true;
        };
        let width = self.state.line_width as f32;
        for p in points.points {
            let (l, t) = (p.x - points.size.0, p.y - points.size.1);
            let (r, b) = if points.size.0 == points.size.1 {
                (l + width, t + width)
            } else {
                (p.x + points.size.0, p.y + points.size.1)
            };
            let rect = [
                f64::from(l),
                f64::from(t),
                f64::from(r) - f64::from(l),
                f64::from(b) - f64::from(t),
            ];
            if let Some(coverage) = self.rect_coverage(rect, true) {
                self.paint(&coverage, &source);
            }
        }
        true
    }

    /// `SkDashImpl::onAsPoints` with `cull_line`: an axis-aligned butt-capped line dashed with
    /// equal integer on and off lengths, as rects centred on `points` plus partial first and
    /// last rects.
    fn dash_as_points(&self, a: Pt, b: Pt) -> Option<DashPoints> {
        let s = &self.state;
        let intervals: Vec<f32> = s.dash.iter().map(|&v| v as f32).collect();
        if intervals.len() != 2 || s.line_cap != LineCap::Butt {
            return None;
        }
        let interval_length = intervals[0] + intervals[1];
        if !(interval_length > 0.0 && interval_length.is_finite()) {
            return None;
        }
        if (intervals[0] - intervals[1]).abs() > 1.0 / 4096.0
            || intervals[0] != intervals[0].floor()
            || intervals[1] != intervals[1].floor()
        {
            return None;
        }
        let m = s.transform.to_skia();
        let rect_stays = (m.kx == 0.0 && m.ky == 0.0 && m.sx != 0.0 && m.sy != 0.0)
            || (m.sx == 0.0 && m.sy == 0.0 && m.kx != 0.0 && m.ky != 0.0);
        if !rect_stays {
            return None;
        }
        let (initial_dash_length, initial_dash_index) =
            dash_parameters(&intervals, s.dash_offset as f32, interval_length);
        let width = s.line_width as f32;
        let mut pts = [a, b];
        let (dx, dy) = (pts[1].x - pts[0].x, pts[1].y - pts[0].y);
        if (dx != 0.0 && dy != 0.0) || (dx == 0.0 && dy == 0.0) {
            return None;
        }
        let mut radius = width * 0.5;
        if s.line_join == LineJoin::Miter {
            radius *= s.miter_limit as f32;
        }
        let device = Rect::from_ltrb(
            -radius,
            -radius,
            self.pixmap.width() as f32 + radius,
            self.pixmap.height() as f32 + radius,
        )?;
        let bounds = device.transform(s.transform.inverse()?.to_skia())?;
        let chop = |lo: f32, hi: f32, min: f32, max: f32| -> Option<(f32, f32)> {
            if hi <= min || lo >= max {
                return None;
            }
            let lo = if lo < min {
                min - (min - lo) % interval_length
            } else {
                lo
            };
            let hi = if hi > max {
                max + (hi - max) % interval_length
            } else {
                hi
            };
            Some((lo, hi))
        };
        if dx != 0.0 {
            let (lo, hi) = if dx < 0.0 {
                (pts[1].x, pts[0].x)
            } else {
                (pts[0].x, pts[1].x)
            };
            let (lo, hi) = chop(lo, hi, bounds.left(), bounds.right())?;
            (pts[0].x, pts[1].x) = if dx < 0.0 { (hi, lo) } else { (lo, hi) };
        } else {
            let (lo, hi) = if dy < 0.0 {
                (pts[1].y, pts[0].y)
            } else {
                (pts[0].y, pts[1].y)
            };
            let (lo, hi) = chop(lo, hi, bounds.top(), bounds.bottom())?;
            (pts[0].y, pts[1].y) = if dy < 0.0 { (hi, lo) } else { (lo, hi) };
        }
        let (tx, ty) = (pts[1].x - pts[0].x, pts[1].y - pts[0].y);
        if tx == 0.0 && ty == 0.0 {
            return None;
        }
        let length = (tx * tx + ty * ty).sqrt();
        let inv = 1.0 / length;
        let tangent = (tx * inv, ty * inv);
        let near = |a: f32, b: f32| (a - b).abs() <= 1.0 / 4096.0;
        let (size, x_axis) = if near(1.0, tangent.0) || near(-1.0, tangent.0) {
            ((intervals[0] * 0.5, width * 0.5), true)
        } else if near(1.0, tangent.1) || near(-1.0, tangent.1) {
            ((width * 0.5, intervals[0] * 0.5), false)
        } else {
            return None;
        };
        let clamped_initial = length.min(initial_dash_length);
        let mut len2 = length;
        if clamped_initial > 0.0 || initial_dash_index == 0 {
            if initial_dash_index == 0 {
                if clamped_initial > 0.0 {
                    len2 -= clamped_initial;
                }
                len2 -= intervals[1];
                if len2 < 0.0 {
                    len2 = 0.0;
                }
            } else {
                len2 -= clamped_initial;
            }
        }
        let num_intervals = len2 / interval_length;
        if !num_intervals.is_finite() || num_intervals > 1_000_000.0 {
            return None;
        }
        let mut mid = num_intervals.floor() as usize;
        len2 -= mid as f32 * interval_length;
        let mut partial_last = false;
        if len2 > 0.0 {
            if len2 < intervals[0] {
                partial_last = true;
            } else {
                mid += 1;
            }
        }
        let mut out = DashPoints {
            first: None,
            last: None,
            points: Vec::new(),
            size,
        };
        let rect_around = |x: f32, y: f32, along: f32| -> [f32; 4] {
            let (hw, hh) = if x_axis {
                (along * 0.5, width * 0.5)
            } else {
                (width * 0.5, along * 0.5)
            };
            [x - hw, y - hh, x + hw, y + hh]
        };
        let mut distance = 0.0f32;
        if clamped_initial > 0.0 || initial_dash_index == 0 {
            if initial_dash_index == 0 {
                if clamped_initial > 0.0 {
                    let x = pts[0].x + tangent.0 * (clamped_initial * 0.5);
                    let y = pts[0].y + tangent.1 * (clamped_initial * 0.5);
                    if clamped_initial < intervals[0] {
                        out.first = Some(rect_around(x, y, clamped_initial));
                    } else {
                        out.points.push(Pt { x, y });
                    }
                    distance += clamped_initial;
                }
                distance += intervals[1];
            } else {
                distance += clamped_initial;
            }
        }
        if mid != 0 {
            distance += intervals[0] * 0.5;
            for _ in 0..mid {
                out.points.push(Pt {
                    x: pts[0].x + tangent.0 * distance,
                    y: pts[0].y + tangent.1 * distance,
                });
                distance += interval_length;
            }
            distance -= intervals[0] * 0.5;
        }
        if partial_last {
            let temp = length - distance;
            let x = pts[0].x + tangent.0 * (distance + temp * 0.5);
            let y = pts[0].y + tangent.1 * (distance + temp * 0.5);
            out.last = Some(rect_around(x, y, temp));
        }
        Some(out)
    }

    /// The device rect of a user rect under a transform that keeps rects axis-aligned.
    fn device_rect(&self, [x, y, w, h]: [f64; 4]) -> Option<Rect> {
        let m = self.state.transform.to_skia();
        if m.kx != 0.0 || m.ky != 0.0 {
            return None;
        }
        let (x0, y0) = (x as f32 * m.sx + m.tx, y as f32 * m.sy + m.ty);
        let (x1, y1) = ((x + w) as f32 * m.sx + m.tx, (y + h) as f32 * m.sy + m.ty);
        Rect::from_ltrb(x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
    }

    fn rect_path(&self, [x, y, w, h]: [f64; 4]) -> Path {
        let mut path = Path::new();
        path.rect(x, y, w, h);
        path
    }

    /// Coverage of a user rect: exact area when antialiased, Skia's rounded edges otherwise.
    fn rect_coverage(&self, rect: [f64; 4], anti_alias: bool) -> Option<Coverage> {
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        if !anti_alias && let Some(device) = self.device_rect(rect) {
            let round = |v: f32| (v + 0.5).floor().clamp(0.0, 1e9) as i64;
            let (l, t) = (round(device.left()), round(device.top()));
            let r = round(device.right()).min(i64::from(w));
            let b = round(device.bottom()).min(i64::from(h));
            if l >= r || t >= b {
                return None;
            }
            let (cw, ch) = ((r - l) as usize, (b - t) as usize);
            return Some(Coverage {
                x: l as i32,
                y: t as i32,
                width: cw,
                height: ch,
                alpha: vec![255; cw * ch],
            });
        }
        let ctm = self.state.transform;
        let path = self.rect_path(rect).to_skia(Some(&ctm), CONIC_TOLERANCE)?;
        let coverage = scan::rasterize(&path, w, h)?;
        if anti_alias {
            return Some(coverage);
        }
        Some(Coverage {
            alpha: coverage
                .alpha
                .iter()
                .map(|&a| if a >= 128 { 255 } else { 0 })
                .collect(),
            ..coverage
        })
    }

    fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let Some(rect) = validate_rect(x, y, w, h) else {
            return;
        };
        if !self.invertible() {
            return;
        }
        let fill = self.state.fill.clone();
        if let (Some(source), Some(coverage)) =
            (self.source(&fill, 256), self.rect_coverage(rect, true))
        {
            self.paint(&coverage, &source);
        }
    }

    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let Some([x, y, w, h]) = validate_rect(x, y, w, h) else {
            return;
        };
        let path = if (w > 0.0) != (h > 0.0) {
            let mut path = Path::new();
            path.move_to(x, y);
            path.line_to(x + w, y + h);
            path.close();
            path
        } else {
            if self.frame_rect([x, y, w, h]) {
                return;
            }
            self.rect_path([x, y, w, h])
        };
        self.stroke_user_path(&path);
    }

    /// `SkDraw::drawRect` strokes an axis-aligned rect with mitred corners as a frame: the
    /// exact area between the rect grown and shrunk by half the device stroke size.
    fn frame_rect(&mut self, rect: [f64; 4]) -> bool {
        let s = &self.state;
        if !s.dash.is_empty()
            || s.line_join != LineJoin::Miter
            || s.miter_limit < std::f64::consts::SQRT_2
            || !self.invertible()
        {
            return false;
        }
        let Some(device) = self.device_rect(rect) else {
            return false;
        };
        let m = s.transform.to_skia();
        let lw = s.line_width as f32;
        let (hx, hy) = ((lw * m.sx).abs() * 0.5, (lw * m.sy).abs() * 0.5);
        let outer = Rect::from_ltrb(
            device.left() - hx,
            device.top() - hy,
            device.right() + hx,
            device.bottom() + hy,
        );
        let mut builder = PathBuilder::new();
        if let Some(outer) = outer {
            builder.push_rect(outer);
        }
        let (il, it, ir, ib) = (
            device.left() + hx,
            device.top() + hy,
            device.right() - hx,
            device.bottom() - hy,
        );
        if il < ir && it < ib {
            builder.move_to(il, it);
            builder.line_to(il, ib);
            builder.line_to(ir, ib);
            builder.line_to(ir, it);
            builder.close();
        }
        if let Some(frame) = builder.finish() {
            let stroke = s.stroke.clone();
            self.fill_device(&frame, &stroke, false);
        }
        true
    }

    /// `clearRect` paints with its own flags: clear blend, no antialiasing, no global alpha.
    fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let Some(rect) = validate_rect(x, y, w, h) else {
            return;
        };
        if !self.invertible() {
            return;
        }
        if let Some(coverage) = self.rect_coverage(rect, false) {
            scan::blit(&mut self.pixmap, &coverage, Blend::Clear, |_, _| [0; 4]);
        }
    }

    fn fill_text(&mut self, text: &JsString, x: f64, y: f64, max_width: Option<f64>) {
        let Fill::Color(color) = self.state.fill else {
            return;
        };
        let ctm = self.state.transform;
        if !finite(&[x, y]) || !ctm.is_invertible() {
            return;
        }
        let axis_aligned = ctm.is_scale_translate() && ctm.a > 0.0 && ctm.d > 0.0;
        let state = &self.state;
        let Some(TextRaster { backend, lut }) = self.text.as_mut() else {
            return;
        };
        let font = state.font;
        let shaped = backend.shape(&font, &text.to_string_lossy());
        let compress = match max_width {
            Some(m) if !(m.is_finite() && m > 0.0) => return,
            Some(m) if shaped.width > m => m / shaped.width,
            _ => 1.0,
        };
        let drawn = shaped.width * compress;
        let x = match state.align.as_str() {
            "center" => x - drawn / 2.0,
            "right" | "end" => x - drawn,
            _ => x,
        };
        let y = y + baseline_offset(&mut **backend, &font, &state.baseline);
        let rgb = color.to_u8();
        let a = color.a * state.alpha as f32;
        let src = [color.r, color.g, color.b, 1.0].map(|c| (c * a * 255.0 + 0.5) as u32);
        let (pw, ph) = (self.pixmap.width() as i32, self.pixmap.height() as i32);
        let data = self.pixmap.data_mut();
        for glyph in &shaped.glyphs {
            let gx = x + glyph.x * compress;
            let device_x = ctm.a * gx + ctm.c * y + ctm.e;
            let device_y = ctm.b * gx + ctm.d * y + ctm.f;
            let (ix, iy, mask) = if axis_aligned {
                let (ix, iy, phase) = glyph_origin(device_x, device_y);
                let mask =
                    backend.raw_mask(&font, glyph.id, ctm.d, ctm.a / ctm.d * compress, phase);
                (ix, iy, mask)
            } else {
                let (ix, _, phase_x) = glyph_origin(device_x, 0.0);
                let (iy, _, phase_y) = glyph_origin(device_y, 0.0);
                let matrix = [ctm.a * compress, ctm.b * compress, ctm.c, ctm.d];
                let mask =
                    backend.raw_mask_transformed(&font, glyph.id, matrix, (phase_x, phase_y));
                (ix, iy, mask)
            };
            let Some(mask) = mask else {
                continue;
            };
            for row in 0..mask.height as i32 {
                let py = iy + mask.top + row;
                if py < 0 || py >= ph {
                    continue;
                }
                for col in 0..mask.width as i32 {
                    let px = ix + mask.left + col;
                    if px < 0 || px >= pw {
                        continue;
                    }
                    let raw = mask.data[(row * mask.width as i32 + col) as usize];
                    let m = u32::from(lut.apply(rgb, raw));
                    if m == 0 {
                        continue;
                    }
                    let s = src.map(|c| div255(c * m));
                    let i = ((py * pw + px) * 4) as usize;
                    for (k, &sc) in s.iter().enumerate() {
                        let d = u32::from(data[i + k]);
                        data[i + k] = (sc + div255(d * (255 - s[3]))).min(255) as u8;
                    }
                }
            }
        }
    }

    fn draw_image(&mut self, id: &str, args: &[f64]) {
        let Some(image) = self.images.get(id) else {
            return;
        };
        let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
        let (src, dst) = match *args {
            [dx, dy] => ([0.0, 0.0, iw, ih], [dx, dy, iw, ih]),
            [dx, dy, dw, dh] => ([0.0, 0.0, iw, ih], [dx, dy, dw, dh]),
            [sx, sy, sw, sh, dx, dy, dw, dh] => ([sx, sy, sw, sh], [dx, dy, dw, dh]),
            _ => return,
        };
        if !finite(&src) || !finite(&dst) || !self.invertible() {
            return;
        }
        let normalize = |[x, y, w, h]: [f64; 4]| {
            let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
            let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
            [x, y, w, h]
        };
        let (src, mut dst) = (normalize(src), normalize(dst));
        if src[2] == 0.0 || src[3] == 0.0 || dst[2] == 0.0 || dst[3] == 0.0 {
            return;
        }
        let clipped = [
            src[0].max(0.0),
            src[1].max(0.0),
            (src[0] + src[2]).min(iw),
            (src[1] + src[3]).min(ih),
        ];
        if clipped[2] <= clipped[0] || clipped[3] <= clipped[1] {
            return;
        }
        let (kx, ky) = (dst[2] / src[2], dst[3] / src[3]);
        dst = [
            dst[0] + (clipped[0] - src[0]) * kx,
            dst[1] + (clipped[1] - src[1]) * ky,
            (clipped[2] - clipped[0]) * kx,
            (clipped[3] - clipped[1]) * ky,
        ];
        let src = [
            clipped[0],
            clipped[1],
            clipped[2] - clipped[0],
            clipped[3] - clipped[1],
        ];
        let ctm = self.state.transform;
        let image_to_device = ctm
            .then(&Affine::translation(dst[0], dst[1]))
            .then(&Affine::scaling(dst[2] / src[2], dst[3] / src[3]))
            .then(&Affine::translation(-src[0], -src[1]));
        let quality = if self.state.smoothing {
            FilterQuality::Bilinear
        } else {
            FilterQuality::Nearest
        };
        let m = image_to_device;
        let fractional_shift = m.a == 1.0
            && m.d == 1.0
            && m.b == 0.0
            && m.c == 0.0
            && (m.e.fract() != 0.0 || m.f.fract() != 0.0);
        let (pixmap, transform) = if quality == FilterQuality::Bilinear && fractional_shift {
            let (fx, fy) = (m.e - m.e.floor(), m.f - m.f.floor());
            (
                shift_bilinear(image, fx as f32, fy as f32),
                Affine::translation(m.e.floor(), m.f.floor()),
            )
        } else {
            (image.clone(), m)
        };
        let anti_alias = if ctm.is_scale_translate() {
            dst[2] * ctm.a.abs() < 1.0 || dst[3] * ctm.d.abs() < 1.0
        } else if ctm.a == 0.0 && ctm.d == 0.0 {
            dst[2] * ctm.b.abs() < 1.0 || dst[3] * ctm.c.abs() < 1.0
        } else {
            true
        };
        let source = Source::Image {
            pixmap,
            transform,
            quality,
            opacity: self.state.alpha as f32,
        };
        if let Some(coverage) = self.rect_coverage(dst, anti_alias) {
            self.paint(&coverage, &source);
        }
    }
}

/// Evaluates `shader` (device space) at the pixel centres of `bounds`.
fn shade(bounds: [i32; 4], shader: Shader<'_>) -> Option<Pixmap> {
    let (w, h) = ((bounds[2] - bounds[0]) as u32, (bounds[3] - bounds[1]) as u32);
    let mut scratch = Pixmap::new(w, h)?;
    let (x, y) = (bounds[0] as f32, bounds[1] as f32);
    let rect = Rect::from_xywh(x, y, w as f32, h as f32)?;
    let paint = Paint {
        shader,
        anti_alias: false,
        ..Paint::default()
    };
    scratch.fill_rect(rect, &paint, Transform::from_translate(-x, -y), None);
    Some(scratch)
}

fn sample_stops(stops: &[(f32, Rgba)], t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let first = stops[0];
    if t <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let ((o0, c0), (o1, c1)) = (pair[0], pair[1]);
        if t <= o1 {
            if o1 == o0 {
                return c1;
            }
            let k = (t - o0) / (o1 - o0);
            let lerp = |a: f32, b: f32| a + (b - a) * k;
            return Rgba {
                r: lerp(c0.r, c1.r),
                g: lerp(c0.g, c1.g),
                b: lerp(c0.b, c1.b),
                a: lerp(c0.a, c1.a),
            };
        }
    }
    stops[stops.len() - 1].1
}

/// The image resampled bilinearly (premultiplied, in Skia's float arithmetic, clamped at the image
/// edges) at a sub-pixel offset `(fx, fy)` in `[0, 1)`. tiny-skia drops to nearest-neighbour for
/// any pure translate, so fractional image positions are resolved here instead.
fn shift_bilinear(image: &Pixmap, fx: f32, fy: f32) -> Pixmap {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (ow, oh) = (w + i64::from(fx > 0.0), h + i64::from(fy > 0.0));
    let size = IntSize::from_wh(ow as u32, oh as u32).expect("non-empty image");
    let src = image.data();
    let at = |x: i64, y: i64, c: usize| {
        let (x, y) = (x.clamp(0, w - 1), y.clamp(0, h - 1));
        f32::from(src[((y * w + x) * 4) as usize + c]) * (1.0 / 255.0)
    };
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        let v = y as f32 - fy;
        let y0 = v.floor();
        let ty = v - y0;
        for x in 0..ow {
            let u = x as f32 - fx;
            let x0 = u.floor();
            let tx = u - x0;
            let (x0, y0) = (x0 as i64, y0 as i64);
            let taps = [
                (x0, y0, (1.0 - tx) * (1.0 - ty)),
                (x0 + 1, y0, tx * (1.0 - ty)),
                (x0, y0 + 1, (1.0 - tx) * ty),
                (x0 + 1, y0 + 1, tx * ty),
            ];
            for c in 0..4 {
                let value = taps.iter().fold(0.0f32, |sum, &(sx, sy, weight)| {
                    sum + at(sx, sy, c) * weight
                });
                out[((y * ow + x) * 4) as usize + c] = pixels::to_unorm(value);
            }
        }
    }
    for p in out.as_chunks_mut::<4>().0 {
        let a = p[3];
        for c in &mut p[..3] {
            *c = (*c).min(a);
        }
    }
    Pixmap::from_vec(out, size).expect("sized buffer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Canvas2d;

    fn render(canvas: &Canvas2d) -> Pixmap {
        let mut raster = Raster::new(canvas.width, canvas.height).unwrap();
        raster.run(&canvas.calls);
        raster.into_pixmap()
    }

    fn alpha_row(pixmap: &Pixmap, y: u32) -> Vec<u8> {
        (0..pixmap.width())
            .map(|x| pixmap.pixel(x, y).unwrap().alpha())
            .collect()
    }

    #[test]
    fn one_pixel_line_on_a_pixel_edge_matches_chromium() {
        let mut c = Canvas2d::new(8, 8);
        c.begin_path();
        c.move_to(4.0, 0.0);
        c.line_to(4.0, 8.0);
        c.stroke();
        let row = alpha_row(&render(&c), 4);
        assert_eq!(row, [0, 0, 0, 127, 128, 0, 0, 0]);
    }

    #[test]
    fn thin_strokes_are_modulated_hairlines() {
        let mut c = Canvas2d::new(4, 8);
        c.set_line_width(0.25);
        c.begin_path();
        c.move_to(2.0, 0.0);
        c.line_to(2.0, 8.0);
        c.stroke();
        let row = alpha_row(&render(&c), 4);
        assert_eq!((row[0], row[3]), (0, 0));
        for chromium in [row[1], row[2]] {
            assert!(chromium.abs_diff(31) <= 1, "{row:?}");
        }
    }

    #[test]
    fn no_hairline_stroker_in_the_source() {
        fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(files.len() > 10, "{files:?}");
        let banned = [concat!(".stroke", "_path("), concat!("stroke", "_hairline")];
        for file in files {
            let source = std::fs::read_to_string(&file).unwrap();
            for b in banned {
                assert!(
                    !source.contains(b),
                    "{}: tiny-skia's hairline stroker is banned: {b}",
                    file.display()
                );
            }
        }
    }

    #[test]
    fn geometry_far_off_the_canvas_is_clipped_before_fixed_point() {
        for far in [1e4, 1e5, 1e7, 1e9] {
            let mut c = Canvas2d::new(20, 20);
            c.begin_path();
            c.rect(-far, 0.0, 2.0 * far, 10.0);
            c.fill();
            c.set_line_width(2.0);
            c.begin_path();
            c.move_to(-far, 12.0);
            c.line_to(far, 12.0);
            c.stroke();
            c.set_line_width(1.0);
            c.begin_path();
            c.move_to(-far, 16.5);
            c.line_to(far, 16.5);
            c.stroke();
            c.begin_path();
            c.arc(10.0, 2e4 + 18.0, 2e4, 0.0, 6.0);
            c.fill();
            let pixmap = render(&c);
            let column: Vec<u8> = (0..20)
                .map(|y| pixmap.pixel(10, y).unwrap().alpha())
                .collect();
            let mut want = [0; 20];
            want[..10].fill(255);
            want[11..13].fill(255);
            want[16] = 255;
            want[18..].fill(255);
            assert_eq!(column, want, "{far}");
        }
    }

    #[test]
    fn clear_rect_ignores_global_alpha_and_is_not_antialiased() {
        let mut c = Canvas2d::new(6, 1);
        c.fill_rect(0.0, 0.0, 6.0, 1.0);
        c.set_global_alpha(0.5);
        c.clear_rect(1.4, 0.0, 2.2, 1.0);
        let row = alpha_row(&render(&c), 0);
        assert_eq!(row, [255, 0, 0, 0, 255, 255]);
    }

    #[test]
    fn invalid_assignments_are_ignored() {
        let mut c = Canvas2d::new(2, 1);
        c.set_fill_style("#00ff00");
        c.set_fill_style("not a color");
        c.set_global_alpha(2.0);
        c.set_line_width(-1.0);
        c.fill_rect(0.0, 0.0, 1.0, 1.0);
        let p = render(&c).pixel(0, 0).unwrap();
        assert_eq!((p.red(), p.green(), p.alpha()), (0, 255, 255));
    }

    #[test]
    fn restore_maps_the_open_path_back() {
        let mut c = Canvas2d::new(10, 10);
        c.save();
        c.scale(2.0, 2.0);
        c.begin_path();
        c.rect(1.0, 1.0, 2.0, 2.0);
        c.restore();
        c.fill();
        let pixmap = render(&c);
        assert_eq!(pixmap.pixel(3, 3).unwrap().alpha(), 255);
        assert_eq!(pixmap.pixel(6, 6).unwrap().alpha(), 0);
    }

    #[test]
    fn image_data_round_trips_through_registers() {
        let mut c = Canvas2d::new(4, 2);
        c.set_fill_style("rgba(255, 0, 0, 0.5)");
        c.fill_rect(0.0, 0.0, 2.0, 2.0);
        c.get_image_data(0.0, 0.0, 2.0, 2.0, "$a");
        c.put_image_data(ImageDataSource::Register("$a".to_owned()), 2.0, 0.0);
        let mut raster = Raster::new(4, 2).unwrap();
        raster.run(&c.calls);
        assert_eq!(raster.register("$a").unwrap().data[..4], [255, 0, 0, 128]);
        let pixmap = raster.into_pixmap();
        assert_eq!(pixmap.pixel(3, 1), pixmap.pixel(0, 0));
    }
}
