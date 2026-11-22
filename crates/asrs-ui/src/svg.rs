//! The subset of SVG upstream's `www/symbols` use, parsed with usvg's parser crates (roxmltree,
//! simplecss, svgtypes) and painted the way Blink records it: shapes keep their kind
//! (`DrawRect`, `DrawOval`, round rects and arcs as conics) and path data keeps its quads. On
//! macOS Chromium rasterizes the DOM with Graphite, which fills paths and strokes curves by MSAA
//! (asrs-canvas's `msaa`) and draws rects, round rects, lines and circles with analytic coverage;
//! elsewhere it is CPU Skia (asrs-canvas's analytic AA, stroker and hairlines).

use std::str::FromStr;

use asrs_canvas::blit::Spans;
use asrs_canvas::hairline::{self, P};
use asrs_canvas::path::Affine;
use asrs_canvas::skpath::{SkPath, Verb};
use asrs_canvas::{Pixmap, aaa, msaa, scan, stroke};
use roxmltree::Node;
use svgtypes::{Align, AspectRatio, PathParser, PathSegment};
use tiny_skia::{LineCap, LineJoin};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color([u8; 3], f32),
    CurrentColor,
}

#[derive(Clone, Debug)]
struct Style {
    color: [u8; 3],
    fill: Paint,
    fill_opacity: f32,
    even_odd: bool,
    stroke: Paint,
    stroke_opacity: f32,
    stroke_width: f32,
    cap: LineCap,
    join: LineJoin,
    miter: f32,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            color: [0, 0, 0],
            fill: Paint::Color([0, 0, 0], 1.0),
            fill_opacity: 1.0,
            even_odd: false,
            stroke: Paint::None,
            stroke_opacity: 1.0,
            stroke_width: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            miter: 4.0,
        }
    }
}

impl Style {
    fn rgba(&self, paint: Paint, opacity: f32) -> Option<[f32; 4]> {
        let (rgb, alpha) = match paint {
            Paint::None => return None,
            Paint::Color(rgb, a) => (rgb, a),
            Paint::CurrentColor => (self.color, 1.0),
        };
        let a = alpha * opacity.clamp(0.0, 1.0);
        (a > 0.0).then(|| {
            let [r, g, b] = rgb.map(|v| f32::from(v) / 255.0);
            [r, g, b, a]
        })
    }

    fn apply(&mut self, name: &str, value: &str) {
        let value = value.trim();
        let number = || svgtypes::Number::from_str(value).ok().map(|n| n.0 as f32);
        let opacity = || number().map(|v| v.clamp(0.0, 1.0));
        match name {
            "color" => {
                if let Ok(c) = svgtypes::Color::from_str(value) {
                    self.color = [c.red, c.green, c.blue];
                }
            }
            "fill" => {
                if let Some(p) = parse_paint(value) {
                    self.fill = p;
                }
            }
            "stroke" => {
                if let Some(p) = parse_paint(value) {
                    self.stroke = p;
                }
            }
            "fill-opacity" => self.fill_opacity = opacity().unwrap_or(self.fill_opacity),
            "stroke-opacity" => self.stroke_opacity = opacity().unwrap_or(self.stroke_opacity),
            "fill-rule" => match value {
                "evenodd" => self.even_odd = true,
                "nonzero" => self.even_odd = false,
                _ => {}
            },
            "stroke-width" => {
                if let Some(w) = length(value).filter(|w| *w >= 0.0) {
                    self.stroke_width = w;
                }
            }
            "stroke-miterlimit" => {
                if let Some(m) = number().filter(|m| *m >= 1.0) {
                    self.miter = m;
                }
            }
            "stroke-linecap" => match value {
                "butt" => self.cap = LineCap::Butt,
                "round" => self.cap = LineCap::Round,
                "square" => self.cap = LineCap::Square,
                _ => {}
            },
            "stroke-linejoin" => match value {
                "miter" => self.join = LineJoin::Miter,
                "round" => self.join = LineJoin::Round,
                "bevel" => self.join = LineJoin::Bevel,
                _ => {}
            },
            _ => {}
        }
    }
}

fn parse_paint(value: &str) -> Option<Paint> {
    match value {
        "none" => Some(Paint::None),
        "currentColor" => Some(Paint::CurrentColor),
        _ => svgtypes::Color::from_str(value)
            .ok()
            .map(|c| Paint::Color([c.red, c.green, c.blue], f32::from(c.alpha) / 255.0)),
    }
}

/// A length in user units: a number, optionally in `px`.
fn length(value: &str) -> Option<f32> {
    let l = svgtypes::Length::from_str(value.trim()).ok()?;
    matches!(
        l.unit,
        svgtypes::LengthUnit::None | svgtypes::LengthUnit::Px
    )
    .then_some(l.number as f32)
}

const PROPERTIES: [&str; 10] = [
    "color",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-opacity",
    "stroke-width",
    "stroke-miterlimit",
    "stroke-linecap",
    "stroke-linejoin",
];

/// How Chromium rasterizes the DOM on this platform: Graphite on macOS, CPU Skia on the
/// GPU-less Windows and Linux render hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Graphite,
    Raster,
}

impl Backend {
    pub const HOST: Backend = if cfg!(target_os = "macos") {
        Backend::Graphite
    } else {
        Backend::Raster
    };
}

/// A shape as Blink records it: `DrawRect`, `DrawOval`, or a path, where a rounded `<rect>` is a
/// path Skia still recognizes as a round rect.
#[derive(Clone, Debug)]
enum Geometry {
    Rect([f32; 4]),
    RRect(SkPath),
    Oval([f32; 4]),
    Path(SkPath),
}

#[derive(Clone, Debug)]
struct Shape {
    transform: Affine,
    geometry: Geometry,
    style: Style,
}

#[derive(Clone, Debug)]
pub struct Document {
    view_box: Option<[f32; 4]>,
    aspect: AspectRatio,
    shapes: Vec<Shape>,
}

struct Css<'a> {
    rules: Vec<simplecss::Rule<'a>>,
}

#[derive(Clone, Copy)]
struct El<'a, 'input>(Node<'a, 'input>);

impl simplecss::Element for El<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(El)
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(El)
    }

    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }

    fn attribute_matches(&self, name: &str, op: simplecss::AttributeOperator<'_>) -> bool {
        self.0.attribute(name).is_some_and(|v| op.matches(v))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass<'_>) -> bool {
        matches!(class, simplecss::PseudoClass::FirstChild)
            && self.0.prev_sibling_element().is_none()
    }
}

impl Document {
    pub fn parse(svg: &[u8]) -> Option<Document> {
        let text = std::str::from_utf8(svg).ok()?;
        let xml = roxmltree::Document::parse(text).ok()?;
        let root = xml.root_element();
        if root.tag_name().name() != "svg" {
            return None;
        }
        let mut sheet = String::new();
        for node in root.descendants() {
            if node.tag_name().name() == "style" {
                for t in node.children().filter(|c| c.is_text()) {
                    sheet.push_str(t.text().unwrap_or_default());
                    sheet.push('\n');
                }
            }
        }
        let mut rules = simplecss::StyleSheet::parse(&sheet).rules;
        rules.sort_by_key(|r| r.selector.specificity());
        let css = Css { rules };
        let view_box = root
            .attribute("viewBox")
            .and_then(|v| svgtypes::ViewBox::from_str(v).ok())
            .filter(|v| v.w > 0.0 && v.h > 0.0)
            .map(|v| [v.x, v.y, v.w, v.h].map(|n| n as f32));
        let aspect = root
            .attribute("preserveAspectRatio")
            .and_then(|v| AspectRatio::from_str(v).ok())
            .unwrap_or_default();
        let mut shapes = Vec::new();
        let style = computed(root, &Style::default(), &css);
        for child in root.children().filter(Node::is_element) {
            walk(child, &style, Affine::IDENTITY, &css, &mut shapes);
        }
        Some(Document {
            view_box,
            aspect,
            shapes,
        })
    }

    /// `SVGPreserveAspectRatio::ComputeTransform` for an outermost `<svg>` laid out at
    /// `viewport` (CSS px).
    pub fn view_transform(&self, viewport: [f32; 2]) -> Affine {
        let Some([vx, vy, vw, vh]) = self.view_box.map(|v| v.map(f64::from)) else {
            return Affine::IDENTITY;
        };
        let [pw, ph] = viewport.map(f64::from);
        let scaled = |s: f64, tx: f64, ty: f64| Affine {
            a: s,
            b: 0.0,
            c: 0.0,
            d: s,
            e: -tx * s,
            f: -ty * s,
        };
        let align = self.aspect.align;
        if align == Align::None {
            return Affine {
                a: pw / vw,
                b: 0.0,
                c: 0.0,
                d: ph / vh,
                e: -vx * pw / vw,
                f: -vy * ph / vh,
            };
        }
        let (mid_x, max_x) = match align {
            Align::XMidYMin | Align::XMidYMid | Align::XMidYMax => (true, false),
            Align::XMaxYMin | Align::XMaxYMid | Align::XMaxYMax => (false, true),
            _ => (false, false),
        };
        let (mid_y, max_y) = match align {
            Align::XMinYMid | Align::XMidYMid | Align::XMaxYMid => (true, false),
            Align::XMinYMax | Align::XMidYMax | Align::XMaxYMax => (false, true),
            _ => (false, false),
        };
        let height_fits = vw / vh < pw / ph;
        if height_fits != self.aspect.slice {
            let extra = vw - pw * vh / ph;
            let tx = if mid_x {
                vx + extra / 2.0
            } else if max_x {
                vx + extra
            } else {
                vx
            };
            scaled(ph / vh, tx, vy)
        } else {
            let extra = vh - ph * vw / pw;
            let ty = if mid_y {
                vy + extra / 2.0
            } else if max_y {
                vy + extra
            } else {
                vy
            };
            scaled(pw / vw, vx, ty)
        }
    }

    /// Paints every shape under `ctm` (user space of the root to device space relative to
    /// `pixmap`) as `backend` rasterizes it.
    ///
    /// The pixmap is the image's destination rect rounded to whole pixels, a non-antialiased
    /// clip; `clip` is the same rect unrounded, the antialiased clip the SVG document's viewport
    /// adds.
    pub fn paint(&self, ctm: &Affine, clip: [f32; 4], backend: Backend, pixmap: &mut Pixmap) {
        let mask = ClipMask::new(clip, backend, pixmap);
        for shape in &self.shapes {
            let m = ctm.then(&shape.transform);
            let style = &shape.style;
            if let Some(rgba) = style.rgba(style.fill, style.fill_opacity) {
                let spans = match (backend, folded_rect(&shape.geometry, &m)) {
                    (Backend::Graphite, Some(r)) => {
                        let r = [
                            r[0].max(clip[0]),
                            r[1].max(clip[1]),
                            r[2].min(clip[2]),
                            r[3].min(clip[3]),
                        ];
                        scan::anti_fill_rect(r, pixmap.width(), pixmap.height())
                    }
                    _ => mask.apply(&fill(&shape.geometry, &m, style.even_odd, backend, pixmap)),
                };
                blit(pixmap, &spans, rgba, 256);
            }
            if style.stroke_width > 0.0
                && let Some(rgba) = style.rgba(style.stroke, style.stroke_opacity)
            {
                let (spans, scale) = match backend {
                    Backend::Graphite => (stroke_graphite(&shape.geometry, &m, style, pixmap), 256),
                    Backend::Raster => stroke_raster(&shape.geometry, &m, style, pixmap),
                };
                blit(pixmap, &mask.apply(&spans), rgba, scale);
            }
        }
    }
}

fn computed(node: Node<'_, '_>, parent: &Style, css: &Css<'_>) -> Style {
    let mut style = parent.clone();
    for name in PROPERTIES {
        if let Some(v) = node.attribute(name) {
            style.apply(name, v);
        }
    }
    for rule in &css.rules {
        if rule.selector.matches(&El(node)) {
            for d in &rule.declarations {
                style.apply(d.name, d.value);
            }
        }
    }
    if let Some(inline) = node.attribute("style") {
        for d in simplecss::DeclarationTokenizer::from(inline) {
            style.apply(d.name, d.value);
        }
    }
    style
}

fn transform_of(node: Node<'_, '_>) -> Affine {
    node.attribute("transform")
        .and_then(|t| svgtypes::Transform::from_str(t).ok())
        .map_or(Affine::IDENTITY, |t| Affine {
            a: t.a,
            b: t.b,
            c: t.c,
            d: t.d,
            e: t.e,
            f: t.f,
        })
}

fn walk(node: Node<'_, '_>, parent: &Style, ctm: Affine, css: &Css<'_>, out: &mut Vec<Shape>) {
    let name = node.tag_name().name();
    if matches!(name, "defs" | "style" | "title" | "desc" | "metadata") {
        return;
    }
    if node.attribute("display") == Some("none") {
        return;
    }
    let style = computed(node, parent, css);
    let transform = ctm.then(&transform_of(node));
    if name == "g" {
        for child in node.children().filter(Node::is_element) {
            walk(child, &style, transform, css, out);
        }
        return;
    }
    if let Some(geometry) = geometry(node) {
        out.push(Shape {
            transform,
            geometry,
            style,
        });
    }
}

fn geometry(node: Node<'_, '_>) -> Option<Geometry> {
    let num = |name: &str| node.attribute(name).and_then(length);
    let pt = |x: f32, y: f32| P { x, y };
    match node.tag_name().name() {
        "path" => Some(Geometry::Path(path_data(node.attribute("d")?))),
        "rect" => {
            let (x, y) = (num("x").unwrap_or(0.0), num("y").unwrap_or(0.0));
            let (w, h) = (num("width")?, num("height")?);
            if !(w > 0.0 && h > 0.0) {
                return None;
            }
            let mut rx = num("rx").filter(|v| *v >= 0.0);
            let mut ry = num("ry").filter(|v| *v >= 0.0);
            if rx.is_none() {
                rx = ry;
            }
            if ry.is_none() {
                ry = rx;
            }
            let (rx, ry) = (rx.unwrap_or(0.0), ry.unwrap_or(0.0));
            let rect = [x, y, x + w, y + h];
            if rx > 0.0 || ry > 0.0 {
                Some(Geometry::RRect(rounded_rect(
                    rect,
                    rx.min(w * 0.5),
                    ry.min(h * 0.5),
                )))
            } else {
                Some(Geometry::Rect(rect))
            }
        }
        "circle" => {
            let r = num("r").filter(|r| *r > 0.0)?;
            let (cx, cy) = (num("cx").unwrap_or(0.0), num("cy").unwrap_or(0.0));
            Some(Geometry::Oval([cx - r, cy - r, cx + r, cy + r]))
        }
        "ellipse" => {
            let (rx, ry) = (num("rx")?, num("ry")?);
            if !(rx > 0.0 && ry > 0.0) {
                return None;
            }
            let (cx, cy) = (num("cx").unwrap_or(0.0), num("cy").unwrap_or(0.0));
            Some(Geometry::Oval([cx - rx, cy - ry, cx + rx, cy + ry]))
        }
        "line" => {
            let mut path = SkPath::new();
            path.move_to(pt(num("x1").unwrap_or(0.0), num("y1").unwrap_or(0.0)));
            path.line_to(pt(num("x2").unwrap_or(0.0), num("y2").unwrap_or(0.0)));
            Some(Geometry::Path(path))
        }
        name @ ("polyline" | "polygon") => {
            let mut path = SkPath::new();
            for (i, (x, y)) in svgtypes::PointsParser::from(node.attribute("points")?).enumerate() {
                let p = pt(x as f32, y as f32);
                if i == 0 {
                    path.move_to(p);
                } else {
                    path.line_to(p);
                }
            }
            if name == "polygon" {
                path.close();
            }
            Some(Geometry::Path(path))
        }
        _ => None,
    }
}

/// `SVGPathBuilder` over Blink's path parser: relative coordinates resolved in float, smooth
/// segments reflected, quads kept, arcs through `SkPathBuilder::arcTo`. Parsing stops at the
/// first error, keeping what came before it.
fn path_data(d: &str) -> SkPath {
    let mut path = SkPath::new();
    let mut current = P { x: 0.0, y: 0.0 };
    let mut subpath = current;
    let mut last_control = current;
    let mut last: Option<PathSegment> = None;
    let add = |a: P, x: f64, y: f64| P {
        x: a.x + x as f32,
        y: a.y + y as f32,
    };
    let abs = |x: f64, y: f64| P {
        x: x as f32,
        y: y as f32,
    };
    for segment in PathParser::from(d) {
        let Ok(segment) = segment else {
            break;
        };
        let at = |is_abs: bool, x: f64, y: f64| {
            if is_abs {
                abs(x, y)
            } else {
                add(current, x, y)
            }
        };
        let smooth = |compatible: bool| {
            if compatible {
                P {
                    x: current.x + (current.x - last_control.x),
                    y: current.y + (current.y - last_control.y),
                }
            } else {
                current
            }
        };
        match segment {
            PathSegment::MoveTo { abs, x, y } => {
                let p = at(abs, x, y);
                path.move_to(p);
                subpath = p;
                current = p;
            }
            PathSegment::LineTo { abs, x, y } => {
                current = at(abs, x, y);
                path.line_to(current);
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                current = if abs {
                    P {
                        x: x as f32,
                        y: current.y,
                    }
                } else {
                    add(current, x, 0.0)
                };
                path.line_to(current);
            }
            PathSegment::VerticalLineTo { abs, y } => {
                current = if abs {
                    P {
                        x: current.x,
                        y: y as f32,
                    }
                } else {
                    add(current, 0.0, y)
                };
                path.line_to(current);
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let c = at(abs, x1, y1);
                let p = at(abs, x, y);
                path.quad_to(c, p);
                last_control = c;
                current = p;
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let compatible = matches!(
                    last,
                    Some(PathSegment::Quadratic { .. } | PathSegment::SmoothQuadratic { .. })
                );
                let c = smooth(compatible);
                let p = at(abs, x, y);
                path.quad_to(c, p);
                last_control = c;
                current = p;
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (c1, c2, p) = (at(abs, x1, y1), at(abs, x2, y2), at(abs, x, y));
                path.cubic_to(c1, c2, p);
                last_control = c2;
                current = p;
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let compatible = matches!(
                    last,
                    Some(PathSegment::CurveTo { .. } | PathSegment::SmoothCurveTo { .. })
                );
                let c1 = smooth(compatible);
                let (c2, p) = (at(abs, x2, y2), at(abs, x, y));
                path.cubic_to(c1, c2, p);
                last_control = c2;
                current = p;
            }
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let p = at(abs, x, y);
                arc_to(
                    &mut path,
                    current,
                    [rx as f32, ry as f32],
                    x_axis_rotation as f32,
                    large_arc,
                    sweep,
                    p,
                );
                current = p;
            }
            PathSegment::ClosePath { .. } => {
                path.close();
                current = subpath;
            }
        }
        last = Some(segment);
    }
    path
}

fn sin_snap(r: f32) -> f32 {
    let v = r.sin();
    if v.abs() <= 1.0 / 4096.0 { 0.0 } else { v }
}

fn cos_snap(r: f32) -> f32 {
    let v = r.cos();
    if v.abs() <= 1.0 / 4096.0 { 0.0 } else { v }
}

/// `SkMatrix::setRotate(degrees)` as `[cos, sin]`.
fn rotation(degrees: f32) -> [f32; 2] {
    let r = degrees.to_radians();
    [cos_snap(r), sin_snap(r)]
}

/// `SkPathBuilder::arcTo(rad, angle, arcLarge, arcSweep, endPt)`, the SVG arc as conics.
fn arc_to(
    path: &mut SkPath,
    start: P,
    rad: [f32; 2],
    angle: f32,
    large: bool,
    sweep: bool,
    end: P,
) {
    if path.is_empty() {
        path.move_to(start);
    }
    if rad[0] == 0.0 || rad[1] == 0.0 || (start.x == end.x && start.y == end.y) {
        path.line_to(end);
        return;
    }
    let (mut rx, mut ry) = (rad[0].abs(), rad[1].abs());
    let mid = P {
        x: (start.x - end.x) * 0.5,
        y: (start.y - end.y) * 0.5,
    };
    let [c, s] = rotation(-angle);
    let tm = P {
        x: c * mid.x - s * mid.y,
        y: s * mid.x + c * mid.y,
    };
    let radii_scale = tm.x * tm.x / (rx * rx) + tm.y * tm.y / (ry * ry);
    if radii_scale > 1.0 {
        let k = radii_scale.sqrt();
        rx *= k;
        ry *= k;
    }
    let (sx, sy) = (1.0 / rx, 1.0 / ry);
    let to_unit = |p: P| P {
        x: sx * c * p.x + sx * -s * p.y,
        y: sy * s * p.x + sy * c * p.y,
    };
    let mut u0 = to_unit(start);
    let mut u1 = to_unit(end);
    let mut delta = P {
        x: u1.x - u0.x,
        y: u1.y - u0.y,
    };
    let d = delta.x * delta.x + delta.y * delta.y;
    let mut scale = (1.0 / d - 0.25).max(0.0).sqrt();
    if !sweep != large {
        scale = -scale;
    }
    delta.x *= scale;
    delta.y *= scale;
    let mut center = P {
        x: (u0.x + u1.x) * 0.5,
        y: (u0.y + u1.y) * 0.5,
    };
    center.x -= delta.y;
    center.y += delta.x;
    u0.x -= center.x;
    u0.y -= center.y;
    u1.x -= center.x;
    u1.y -= center.y;
    let theta1 = u0.y.atan2(u0.x);
    let theta2 = u1.y.atan2(u1.x);
    let mut theta_arc = theta2 - theta1;
    if theta_arc < 0.0 && sweep {
        theta_arc += std::f32::consts::PI * 2.0;
    } else if theta_arc > 0.0 && !sweep {
        theta_arc -= std::f32::consts::PI * 2.0;
    }
    if theta_arc.abs() < std::f32::consts::PI / (1000.0 * 1000.0) {
        path.line_to(end);
        return;
    }
    let [c, s] = rotation(angle);
    let [m00, m01, m10, m11] = [c * rx, -s * ry, s * rx, c * ry];
    let map = |p: P| P {
        x: m00 * p.x + m01 * p.y,
        y: m10 * p.x + m11 * p.y,
    };
    let segments = (theta_arc / (2.0 * std::f32::consts::PI / 3.0))
        .abs()
        .ceil() as i32;
    let width = theta_arc / segments as f32;
    let t = (0.5 * width).tan();
    if !t.is_finite() {
        return;
    }
    let w = (0.5 + width.cos() * 0.5).sqrt();
    let integral = |v: f32| v == v.floor();
    let expect_integers = (std::f32::consts::FRAC_PI_2 - width.abs()).abs() <= 1.0 / 4096.0
        && integral(rx)
        && integral(ry)
        && integral(end.x)
        && integral(end.y);
    let mut theta = theta1;
    for _ in 0..segments {
        let end_theta = theta + width;
        let (sin_end, cos_end) = (sin_snap(end_theta), cos_snap(end_theta));
        let p1 = P {
            x: cos_end + center.x,
            y: sin_end + center.y,
        };
        let p0 = P {
            x: p1.x + t * sin_end,
            y: p1.y - t * cos_end,
        };
        let mut mapped = [map(p0), map(p1)];
        if expect_integers {
            for p in &mut mapped {
                p.x = (p.x + 0.5).floor();
                p.y = (p.y + 0.5).floor();
            }
        }
        path.conic_to(mapped[0], mapped[1], w);
        theta = end_theta;
    }
    path.set_last_pt(end);
}

/// `SkPathBuilder::addRRect(rrect, kCW, 0)` for a rect with the same radii on every corner.
fn rounded_rect([l, t, r, b]: [f32; 4], rx: f32, ry: f32) -> SkPath {
    let pt = |x: f32, y: f32| P { x, y };
    let mut path = SkPath::new();
    if rx <= 0.0 || ry <= 0.0 {
        path.move_to(pt(l, t));
        path.line_to(pt(r, t));
        path.line_to(pt(r, b));
        path.line_to(pt(l, b));
        path.close();
        path.set_convex(true);
        return path;
    }
    let w = std::f32::consts::FRAC_1_SQRT_2;
    let corners = [pt(l, t), pt(r, t), pt(r, b), pt(l, b)];
    let edge = [
        pt(l + rx, t),
        pt(r - rx, t),
        pt(r, t + ry),
        pt(r, b - ry),
        pt(r - rx, b),
        pt(l + rx, b),
        pt(l, b - ry),
        pt(l, t + ry),
    ];
    path.move_to(edge[0]);
    for i in 0..4 {
        path.line_to(edge[i * 2 + 1]);
        path.conic_to(corners[(i + 1) % 4], edge[(i * 2 + 2) % 8], w);
    }
    path.close();
    path.set_convex(true);
    path
}

/// `SkPath::Oval(oval)`: clockwise from the right-hand point.
fn oval_path([l, t, r, b]: [f32; 4]) -> SkPath {
    let pt = |x: f32, y: f32| P { x, y };
    let (cx, cy) = ((l + r) * 0.5, (t + b) * 0.5);
    let w = std::f32::consts::FRAC_1_SQRT_2;
    let mut path = SkPath::new();
    path.move_to(pt(r, cy));
    path.conic_to(pt(r, b), pt(cx, b), w);
    path.conic_to(pt(l, b), pt(l, cy), w);
    path.conic_to(pt(l, t), pt(cx, t), w);
    path.conic_to(pt(r, t), pt(r, cy), w);
    path.close();
    path.set_convex(true);
    path
}

fn rect_path([l, t, r, b]: [f32; 4]) -> SkPath {
    rounded_rect([l, t, r, b], 0.0, 0.0)
}

fn as_path(geometry: &Geometry) -> SkPath {
    match geometry {
        Geometry::Rect(r) => rect_path(*r),
        Geometry::RRect(p) | Geometry::Path(p) => p.clone(),
        Geometry::Oval(r) => oval_path(*r),
    }
}

fn rect_stays_rect(m: &Affine) -> bool {
    (m.b == 0.0 && m.c == 0.0 && m.a != 0.0 && m.d != 0.0)
        || (m.a == 0.0 && m.d == 0.0 && m.b != 0.0 && m.c != 0.0)
}

fn device_rect(m: &Affine, [l, t, r, b]: [f32; 4]) -> [f32; 4] {
    let a = asrs_canvas::skpath::map_point(m, P { x: l, y: t });
    let c = asrs_canvas::skpath::map_point(m, P { x: r, y: b });
    [a.x.min(c.x), a.y.min(c.y), a.x.max(c.x), a.y.max(c.y)]
}

fn is_line(path: &SkPath) -> bool {
    path.verbs == [Verb::Move, Verb::Line]
}

fn area_coverage(path: &SkPath, even_odd: bool, pixmap: &Pixmap) -> Spans {
    let mut spans = Spans::new(pixmap.width(), pixmap.height());
    if path.is_finite() {
        aaa::fill_path(path, pixmap.width(), pixmap.height(), even_odd, &mut spans);
    }
    spans
}

fn sample_coverage(path: &SkPath, even_odd: bool, pixmap: &Pixmap) -> Spans {
    let mut spans = Spans::new(pixmap.width(), pixmap.height());
    if path.is_finite() {
        msaa::fill_path(path, pixmap.width(), pixmap.height(), even_odd, &mut spans);
    }
    spans
}

/// The device rect of a filled rect that Graphite intersects with a rect clip instead of
/// multiplying in the clip's coverage: `DrawRect`, and paths `Device::drawPath` turns into it.
fn folded_rect(geometry: &Geometry, m: &Affine) -> Option<[f32; 4]> {
    if !rect_stays_rect(m) {
        return None;
    }
    match geometry {
        Geometry::Rect(r) => Some(device_rect(m, *r)),
        Geometry::Path(p) => p
            .is_rect()
            .map(|r| device_rect(m, [r.left, r.top, r.right, r.bottom])),
        _ => None,
    }
}

/// Coverage of the antialiased viewport clip: Graphite's analytic rect clip, or the `SkAAClip`
/// CPU Skia builds when the rect is not within 1/8 px of whole pixels.
struct ClipMask {
    width: usize,
    coverage: Option<Vec<u8>>,
}

impl ClipMask {
    fn new(clip: [f32; 4], backend: Backend, pixmap: &Pixmap) -> ClipMask {
        let (w, h) = (pixmap.width(), pixmap.height());
        let coverage = match backend {
            Backend::Graphite => {
                let mut out = Vec::with_capacity((w * h) as usize);
                for y in 0..h {
                    for x in 0..w {
                        let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                        let d = (cx - clip[0])
                            .min(clip[2] - cx)
                            .min(cy - clip[1])
                            .min(clip[3] - cy);
                        out.push(((d + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
                    }
                }
                Some(out)
            }
            Backend::Raster => {
                let nearly_integral = |v: f32| {
                    let v = v + 1.0 / 8.0;
                    v - v.floor() < 1.0 / 4.0
                };
                if clip.iter().all(|v| nearly_integral(*v)) {
                    None
                } else {
                    let mut out = vec![0u8; (w * h) as usize];
                    let mut spans = Spans::new(w, h);
                    aaa::fill_path(&rect_path(clip), w, h, false, &mut spans);
                    for (x, y, alphas) in spans.iter() {
                        let start = (y as u32 * w) as usize + x as usize;
                        out[start..start + alphas.len()].copy_from_slice(alphas);
                    }
                    Some(out)
                }
            }
        };
        ClipMask {
            width: w as usize,
            coverage,
        }
    }

    fn apply(&self, spans: &Spans) -> Spans {
        let Some(coverage) = &self.coverage else {
            return spans.clone();
        };
        let mut out = Spans::new(self.width as u32, (coverage.len() / self.width) as u32);
        for (x, y, alphas) in spans.iter() {
            let row = &coverage[y as usize * self.width..];
            let merged: Vec<u8> = alphas
                .iter()
                .enumerate()
                .map(|(i, &a)| {
                    let prod = u32::from(a) * u32::from(row[x as usize + i]) + 128;
                    ((prod + (prod >> 8)) >> 8) as u8
                })
                .collect();
            out.anti_h(x, y, &merged);
        }
        out
    }
}

fn fill(
    geometry: &Geometry,
    m: &Affine,
    even_odd: bool,
    backend: Backend,
    pixmap: &Pixmap,
) -> Spans {
    if let Geometry::Rect(r) = geometry
        && rect_stays_rect(m)
    {
        return scan::anti_fill_rect(device_rect(m, *r), pixmap.width(), pixmap.height());
    }
    let device = as_path(geometry).transformed(m);
    match (backend, geometry) {
        (Backend::Graphite, Geometry::Path(p)) if is_line(p) => {
            Spans::new(pixmap.width(), pixmap.height())
        }
        (Backend::Graphite, Geometry::Path(p)) if p.rect_contour().is_none() => {
            sample_coverage(&device, even_odd, pixmap)
        }
        _ => area_coverage(&device, even_odd, pixmap),
    }
}

fn fast_len(x: f32, y: f32) -> f32 {
    let (x, y) = (x.abs(), y.abs());
    let (big, small) = if x < y { (y, x) } else { (x, y) };
    big + small / 2.0
}

fn stroke_outline(geometry: &Geometry, m: &Affine, style: &Style) -> SkPath {
    let [sx, ky, kx, sy] = [m.a, m.b, m.c, m.d].map(|v| v as f32);
    let res_scale = {
        let a = (sx * sx + kx * kx).sqrt();
        let b = (ky * ky + sy * sy).sqrt();
        if a.is_finite() && b.is_finite() && a.max(b) > 0.0 {
            a.max(b)
        } else {
            1.0
        }
    };
    stroke::stroke_path(
        &as_path(geometry),
        &stroke::Style {
            width: style.stroke_width,
            miter_limit: style.miter,
            cap: style.cap,
            join: style.join,
            res_scale,
        },
    )
    .transformed(m)
}

/// Graphite draws lines, rects, round rects and circles with analytic coverage and every other
/// stroke as a tessellated outline under MSAA.
fn stroke_graphite(geometry: &Geometry, m: &Affine, style: &Style, pixmap: &Pixmap) -> Spans {
    let outline = stroke_outline(geometry, m, style);
    let analytic = match geometry {
        Geometry::Rect(_) | Geometry::RRect(_) | Geometry::Oval(_) => true,
        Geometry::Path(p) => is_line(p) || p.rect_contour().is_some_and(|(_, closed, _)| closed),
    };
    if analytic {
        area_coverage(&outline, false, pixmap)
    } else {
        sample_coverage(&outline, false, pixmap)
    }
}

/// CPU Skia's `SkDraw::drawRect` and `drawPath` for a stroke: the stroke's coverage and the
/// 0..=256 alpha scale `modifyPaintForHairlines` applies.
fn stroke_raster(geometry: &Geometry, m: &Affine, style: &Style, pixmap: &Pixmap) -> (Spans, u32) {
    let (w, h) = (pixmap.width(), pixmap.height());
    let width = style.stroke_width;
    let [sx, ky, kx, sy] = [m.a, m.b, m.c, m.d].map(|v| v as f32);
    if let Geometry::Rect(r) = geometry
        && rect_stays_rect(m)
        && style.join == LineJoin::Miter
        && style.miter >= std::f32::consts::SQRT_2
    {
        let size = (
            (sx * width + kx * width).abs(),
            (ky * width + sy * width).abs(),
        );
        return (scan::anti_frame_rect(device_rect(m, *r), size, w, h), 256);
    }
    let len0 = fast_len(sx * width, ky * width);
    let len1 = fast_len(kx * width, sy * width);
    if len0 <= 1.0 && len1 <= 1.0 {
        let coverage = (len0 + len1) * 0.5;
        let scale = if coverage == 1.0 {
            256
        } else {
            (coverage * 256.0) as u32
        };
        let mut spans = Spans::new(w, h);
        if let Some(device) = as_path(geometry).transformed(m).to_tiny() {
            for (a, b) in hairline::segments(&device, style.cap) {
                if let Some(c) = hairline::spans(a, b, w, h) {
                    for (x, y, alphas) in c.iter() {
                        spans.anti_h(x, y, alphas);
                    }
                }
            }
        }
        return (spans, scale);
    }
    (
        area_coverage(&stroke_outline(geometry, m, style), false, pixmap),
        256,
    )
}

fn blit(pixmap: &mut Pixmap, spans: &Spans, rgba: [f32; 4], scale: u32) {
    let mut a8 = (rgba[3] * 255.0 + 0.5) as u32;
    if scale < 256 {
        a8 = (a8 * scale) >> 8;
    }
    let color = [
        (rgba[0] * 255.0 + 0.5) as u8,
        (rgba[1] * 255.0 + 0.5) as u8,
        (rgba[2] * 255.0 + 0.5) as u8,
        a8 as u8,
    ];
    scan::blit_spans_solid(pixmap, spans, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcs_become_conics_like_skia() {
        let path = path_data("M39,92a25,25,0,0,0,25,25");
        let tiny = path.to_tiny().unwrap();
        let b = tiny.bounds();
        assert!((b.left() - 39.0).abs() < 1e-4 && (b.bottom() - 117.0).abs() < 1e-4);
    }

    #[test]
    fn styles_cascade_from_classes() {
        let doc = Document::parse(
            br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><defs><style>.a{fill:none;stroke:#4555a5;stroke-width:6px;}</style></defs><line class="a" x1="1" y1="2" x2="1" y2="8"/><rect width="10" height="2" rx="1"/></svg>"##,
        )
        .unwrap();
        assert_eq!(doc.shapes.len(), 2);
        assert_eq!(doc.shapes[0].style.fill, Paint::None);
        assert_eq!(
            doc.shapes[0].style.stroke,
            Paint::Color([0x45, 0x55, 0xa5], 1.0)
        );
        assert_eq!(doc.shapes[0].style.stroke_width, 6.0);
        assert!(matches!(doc.shapes[1].geometry, Geometry::RRect(_)));
    }
}
