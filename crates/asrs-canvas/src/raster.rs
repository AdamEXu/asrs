// Ported from Skia and Blink, Chromium m142 (BSD-3-Clause, Copyright Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

use std::collections::HashMap;
use std::sync::Arc;

use asrs_js::string::JsString;
use tiny_skia::{LineCap, LineJoin, Pixmap};

use crate::aaa;
use crate::blit::Spans;
use crate::color::{self, Rgba};
use crate::dash;
use crate::display::{Call, Gradient, GradientShape, ImageData, ImageDataSource, Property, Style};
use crate::gradient;
use crate::hairline;
use crate::image;
use crate::matrix::Matrix;
use crate::mip::{Level, Mip};
use crate::path::{Affine, Path, Pt, clamp32, sat32};
use crate::pixels;
use crate::raster_clip::RasterClip;
use crate::scan::{self, Blend, Coverage};
use crate::skpath::SkPath;
use crate::stroke;
use crate::text::layout::{Placed, place_text};
use crate::text::lut::MaskGammaLut;
use crate::text::{FontSpec, TextBackend};

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
        Style::Undefined => None,
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
            let widen = |v: f64| f64::from(clamp32(v));
            Some(match shape {
                GradientShape::Linear(p) => Fill::Linear(p.map(widen), resolved),
                GradientShape::Radial(p) => Fill::Radial(p.map(widen), resolved),
            })
        }
    }
}

#[derive(Clone, Debug)]
struct State {
    /// Blink's matrix, which the current path and the invertibility checks follow.
    transform: Affine,
    /// The matrix of the `SkCanvas` the calls are played back on.
    ctm: Matrix,
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
    clip: Option<RasterClip>,
}

impl Default for State {
    fn default() -> State {
        State {
            transform: Affine::IDENTITY,
            ctm: Matrix::IDENTITY,
            fill: Fill::Color(Rgba::BLACK),
            stroke: Fill::Color(Rgba::BLACK),
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            alpha: 1.0,
            font: FontSpec::default(),
            align: "start".to_owned(),
            baseline: "alphabetic".to_owned(),
            smoothing: true,
            clip: None,
        }
    }
}

/// What one canvas carries from frame to frame: the lines-as-paths counters of its
/// `cc::RecordPaintCanvas`. Blink keeps that recorder while the canvas keeps its size and makes a
/// new one when the size changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterState {
    lines_as_paths: bool,
    draw_path_count: u32,
    draw_line_count: u32,
    size: (u32, u32),
}

impl Default for RasterState {
    /// Chromium on macOS records with Graphite, which turns the lines-as-paths heuristic off
    /// (`DisableLineDrawingAsPaths`) even for CPU canvases: measured against the Linux and
    /// macOS references of the `multiframe` scenes.
    fn default() -> RasterState {
        RasterState::new(!cfg!(target_os = "macos"))
    }
}

impl RasterState {
    pub fn new(lines_as_paths: bool) -> RasterState {
        RasterState {
            lines_as_paths,
            draw_path_count: 0,
            draw_line_count: 0,
            size: (0, 0),
        }
    }

    /// The state for a frame of `width` x `height`: kept when the size is unchanged, fresh
    /// otherwise.
    pub fn for_size(self, width: u32, height: u32) -> RasterState {
        if self.size == (width, height) {
            self
        } else {
            RasterState {
                size: (width, height),
                ..RasterState::new(self.lines_as_paths)
            }
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
    lines: RasterState,
    images: HashMap<String, ImageSource>,
    registers: HashMap<String, ImageData>,
    text: Option<TextRaster<'a>>,
    errors: Vec<(usize, CanvasError)>,
    calls: usize,
    unflushed: bool,
    shades: ShadeCache,
}

/// What `drawImage` draws: an `<img>` is decoded by cc at the mip level the draw needs, while a
/// canvas or a bitmap is sampled as it is.
#[derive(Clone)]
struct ImageSource {
    pixmap: Arc<Pixmap>,
    element: bool,
}

/// The resampled images the last frame of a canvas drew, so a frame that draws the same image
/// through the same transform does not sample it again.
#[derive(Clone, Default)]
pub struct ShadeCache {
    entries: Vec<Shade>,
    levels: Vec<ImageLevel>,
}

/// A mip level cc's decode cache holds.
#[derive(Clone)]
struct ImageLevel {
    image: Arc<Pixmap>,
    mip: Mip,
    pixels: Arc<Pixmap>,
    used: bool,
}

#[derive(Clone)]
struct Shade {
    image: Arc<Pixmap>,
    key: ShadeKey,
    pixels: Arc<Vec<[u8; 4]>>,
    used: bool,
}

#[derive(Clone, PartialEq)]
struct ShadeKey {
    shader: image::Shader,
    mip: Option<Mip>,
    runs: image::Runs,
}

impl ShadeCache {
    fn get(&mut self, image: &Arc<Pixmap>, key: &ShadeKey) -> Option<Arc<Vec<[u8; 4]>>> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| Arc::ptr_eq(&e.image, image) && e.key == *key)?;
        entry.used = true;
        Some(entry.pixels.clone())
    }

    fn insert(&mut self, image: &Arc<Pixmap>, key: ShadeKey, pixels: Arc<Vec<[u8; 4]>>) {
        self.entries.push(Shade {
            image: image.clone(),
            key,
            pixels,
            used: true,
        });
    }

    /// `GetDecodedImageForDraw`: a level is made out of the smallest one of the same part that
    /// is already there and large enough (`FindCachedCandidate`), so it depends on the sizes the
    /// image was drawn at before.
    fn level(&mut self, image: &Arc<Pixmap>, mip: Mip) -> Option<Arc<Pixmap>> {
        let candidate = self
            .levels
            .iter_mut()
            .filter(|l| {
                Arc::ptr_eq(&l.image, image)
                    && l.mip.src == mip.src
                    && l.mip.size[0] >= mip.size[0]
                    && l.mip.size[1] >= mip.size[1]
            })
            .min_by_key(|l| l.mip.size);
        let pixels = match candidate {
            Some(level) if level.mip == mip => {
                level.used = true;
                return Some(level.pixels.clone());
            }
            Some(level) => mip.build(&level.pixels)?,
            None => mip.build(mip.part(image)?.as_ref())?,
        };
        let pixels = Arc::new(pixels);
        self.levels.push(ImageLevel {
            image: image.clone(),
            mip,
            pixels: pixels.clone(),
            used: true,
        });
        Some(pixels)
    }

    fn next_frame(mut self) -> ShadeCache {
        for entry in &mut self.entries {
            entry.used = false;
        }
        for level in &mut self.levels {
            level.used = false;
        }
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// An exception Blink throws from a canvas call instead of drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasError {
    /// `IndexSizeError` with its message.
    IndexSize(&'static str),
    /// `RangeError` with its message.
    Range(&'static str),
    /// `drawImage`'s `InvalidStateError` with its message.
    InvalidState(&'static str),
}

const OUT_OF_MEMORY: CanvasError = CanvasError::Range("Out of memory at ImageData creation");

enum Source {
    Solid([u8; 4]),
    Gradient(Box<gradient::Paint>),
}

/// A `CanvasPath` point argument pair, or `None` when Blink ignores the call.
fn path_pt(x: f64, y: f64) -> Option<Pt> {
    let p = Pt {
        x: sat32(x),
        y: sat32(y),
    };
    (p.x.is_finite() && p.y.is_finite()).then_some(p)
}

/// `SkPathPriv::TooBigForMath`: `SkDraw` drops device geometry beyond a quarter of `FLT_MAX`.
fn too_big_for_math(r: &crate::skpath::Rect) -> bool {
    let max = f32::MAX * 0.25;
    !(r.left >= -max && r.top >= -max && r.right <= max && r.bottom <= max)
}

fn sorted([l, t, r, b]: [f32; 4]) -> [f32; 4] {
    [l.min(r), t.min(b), l.max(r), t.max(b)]
}

/// What `SkDraw::drawDevPath` accepts: finite device geometry within `TooBigForMath`.
fn dev_path_drawable(path: &SkPath) -> bool {
    path.is_finite() && !too_big_for_math(&path.bounds())
}

fn fits_in_fixed(r: [f32; 4]) -> bool {
    r.iter().all(|v| v.abs() <= 32767.0)
}

fn ltrb([left, top, right, bottom]: [f32; 4]) -> crate::skpath::Rect {
    crate::skpath::Rect {
        left,
        top,
        right,
        bottom,
    }
}

/// The `gfx::RectF` Blink builds from a validated rect: each value through `ClampTo<float>`,
/// and sizes at most `8 * FLT_EPSILON` dropped to zero.
fn rect32([x, y, w, h]: [f64; 4]) -> [f32; 4] {
    let size = |v: f64| {
        let v = clamp32(v);
        if v > 8.0 * f32::EPSILON { v } else { 0.0 }
    };
    [clamp32(x), clamp32(y), size(w), size(h)]
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

use asrs_js::number::to_int32;

impl<'a> Raster<'a> {
    pub fn new(width: u32, height: u32) -> Option<Raster<'a>> {
        Some(Raster {
            pixmap: Pixmap::new(width, height)?,
            state: State::default(),
            stack: Vec::new(),
            path: Path::new(),
            fast: Fast::None,
            lines: RasterState::default().for_size(width, height),
            images: HashMap::new(),
            registers: HashMap::new(),
            text: None,
            errors: Vec::new(),
            calls: 0,
            unflushed: false,
            shades: ShadeCache::default(),
        })
    }

    /// Reuses the images an earlier frame of the same canvas resampled.
    pub fn with_shades(mut self, shades: ShadeCache) -> Raster<'a> {
        self.shades = shades.next_frame();
        self
    }

    /// The resampled images this frame drew, to hand to the next frame of this canvas.
    pub fn take_shades(&mut self) -> ShadeCache {
        let mut shades = std::mem::take(&mut self.shades);
        shades.entries.retain(|e| e.used);
        shades.levels.retain(|l| {
            let whole = l.mip.src[2..] == [l.image.width(), l.image.height()];
            let mut images = self.images.values();
            (l.used || whole) && images.any(|source| Arc::ptr_eq(&source.pixmap, &l.image))
        });
        shades
    }

    /// Continues from the state an earlier frame of the same canvas left; a state from a
    /// canvas of another size is dropped.
    pub fn with_state(mut self, state: RasterState) -> Raster<'a> {
        self.lines = state.for_size(self.pixmap.width(), self.pixmap.height());
        self
    }

    /// The state to hand to the next frame of this canvas.
    pub fn state(&self) -> RasterState {
        self.lines
    }

    pub fn with_text(mut self, text: TextRaster<'a>) -> Raster<'a> {
        self.text = Some(text);
        self
    }

    /// Makes `pixmap` (premultiplied), a canvas or a bitmap, available to `drawImage` as `id`.
    pub fn add_image(&mut self, id: &str, pixmap: impl Into<Arc<Pixmap>>) {
        let source = ImageSource {
            pixmap: pixmap.into(),
            element: false,
        };
        self.images.insert(id.to_owned(), source);
    }

    /// [`Raster::add_image`] for an `<img>` element.
    pub fn add_image_element(&mut self, id: &str, pixmap: impl Into<Arc<Pixmap>>) {
        let source = ImageSource {
            pixmap: pixmap.into(),
            element: true,
        };
        self.images.insert(id.to_owned(), source);
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

    /// Every exception a call threw, with the index of the call among those applied.
    pub fn errors(&self) -> &[(usize, CanvasError)] {
        &self.errors
    }

    pub fn run(&mut self, calls: &[Call]) {
        for call in calls {
            self.apply(call);
        }
    }

    pub fn apply(&mut self, call: &Call) {
        self.calls += 1;
        if let Err(e) = self.apply_call(call) {
            self.errors.push((self.calls - 1, e));
        }
    }

    fn apply_call(&mut self, call: &Call) -> Result<(), CanvasError> {
        self.unflushed |= matches!(
            call,
            Call::Fill
                | Call::FillEvenOdd
                | Call::Stroke
                | Call::FillRect(..)
                | Call::StrokeRect(..)
                | Call::ClearRect(..)
                | Call::FillText { .. }
                | Call::DrawImage { .. }
        );
        match call {
            Call::Set(p) => self.set(p),
            Call::Save => self.stack.push(self.state.clone()),
            Call::Restore => self.restore(),
            &Call::Scale(x, y) => self.scale(x, y),
            &Call::Rotate(angle) => self.rotate(angle),
            &Call::Translate(x, y) => self.translate(x, y),
            Call::ResetTransform => self.reset_transform(),
            Call::SetTransform(m) => {
                if finite(m) {
                    self.reset_transform();
                    self.apply_transform(m);
                }
            }
            Call::Clip => self.clip_current_path(),
            Call::BeginPath => {
                self.path = Path::new();
                self.fast = Fast::None;
            }
            Call::ClosePath => self.close_path(),
            &Call::MoveTo(x, y) => {
                if self.invertible() {
                    if let Some(p) = path_pt(x, y) {
                        self.fast = if self.path.is_empty() {
                            Fast::Start(p)
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
            &Call::QuadraticCurveTo(cx, cy, x, y) => {
                if self.invertible()
                    && let (Some(cp), Some(p)) = (path_pt(cx, cy), path_pt(x, y))
                {
                    self.fast = Fast::None;
                    self.path.quad_to(cp, p);
                }
            }
            &Call::BezierCurveTo(c1x, c1y, c2x, c2y, x, y) => {
                if self.invertible()
                    && let (Some(c1), Some(c2), Some(p)) =
                        (path_pt(c1x, c1y), path_pt(c2x, c2y), path_pt(x, y))
                {
                    self.fast = Fast::None;
                    self.path.cubic_to(c1, c2, p);
                }
            }
            &Call::Rect(x, y, w, h) => {
                if self.invertible() {
                    if let (Some(p), Some(size)) = (path_pt(x, y), path_pt(w, h)) {
                        self.fast = if size.x == 0.0 && size.y == 0.0 && self.path.is_empty() {
                            Fast::Start(p)
                        } else {
                            Fast::None
                        };
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
                    if [x, y, radius_x, radius_y, rotation, start, end]
                        .iter()
                        .all(|&v| sat32(v).is_finite())
                        && sat32(radius_x) >= 0.0
                        && sat32(radius_y) >= 0.0
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
            Call::Fill => self.fill_current_path(false),
            Call::FillEvenOdd => self.fill_current_path(true),
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
                let data = self.get_image_data(sx, sy, sw, sh)?;
                self.flush();
                self.registers.insert(register.clone(), data);
            }
            Call::PutImageData { data, dx, dy } => {
                let image = match data {
                    ImageDataSource::Register(name) => self.registers.get(name).cloned(),
                    ImageDataSource::Inline(image) => Some(image.clone()),
                };
                if let Some(image) = image {
                    let (x, y) = (i64::from(to_int32(*dx)), i64::from(to_int32(*dy)));
                    self.unflushed |= x <= 0
                        && y <= 0
                        && x + i64::from(image.width) >= i64::from(self.pixmap.width())
                        && y + i64::from(image.height) >= i64::from(self.pixmap.height());
                    self.flush();
                    pixels::write_rect(&mut self.pixmap, &image, x, y);
                }
            }
        }
        Ok(())
    }

    /// Blink's `getImageDataInternal`: the arguments are WebIDL `long`s, and sizes whose
    /// arithmetic overflows `int` or whose buffer cannot exist throw instead of allocating.
    fn get_image_data(&self, sx: f64, sy: f64, sw: f64, sh: f64) -> Result<ImageData, CanvasError> {
        let (mut x, mut y, mut w, mut h) = (to_int32(sx), to_int32(sy), to_int32(sw), to_int32(sh));
        if w.checked_mul(h).is_none() {
            return Err(OUT_OF_MEMORY);
        }
        if w == 0 {
            return Err(CanvasError::IndexSize("The source width is 0."));
        }
        if h == 0 {
            return Err(CanvasError::IndexSize("The source height is 0."));
        }
        if w < 0 {
            x = x.checked_add(w).ok_or(OUT_OF_MEMORY)?;
            w = i32::try_from(w.unsigned_abs()).unwrap_or(i32::MAX);
        }
        if h < 0 {
            y = y.checked_add(h).ok_or(OUT_OF_MEMORY)?;
            h = i32::try_from(h.unsigned_abs()).unwrap_or(i32::MAX);
        }
        if x.checked_add(w).is_none() || y.checked_add(h).is_none() {
            return Err(OUT_OF_MEMORY);
        }
        let elements = 4u32
            .checked_mul(w as u32)
            .and_then(|v| v.checked_mul(h as u32));
        if elements.is_none() {
            return Err(CanvasError::Range("Out of memory at ImageData creation."));
        }
        pixels::read_rect(&self.pixmap, i64::from(x), i64::from(y), w as u32, h as u32)
            .ok_or(CanvasError::Range("Array buffer allocation failed"))
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
                    s.line_width = f64::from(clamp32(w));
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
                    s.miter_limit = f64::from(clamp32(limit));
                }
            }
            &Property::LineDashOffset(offset) => {
                if let Some(offset) = offset.filter(|v| v.is_finite()) {
                    s.dash_offset = f64::from(clamp32(offset));
                }
            }
            &Property::GlobalAlpha(alpha) => {
                if (0.0..=1.0).contains(&alpha) {
                    s.alpha = alpha;
                }
            }
            Property::Font(css) => {
                if let Some(font) = FontSpec::parse(css) {
                    s.font = font;
                }
            }
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

    /// `RestoreMatrixClipStack`: once Blink has played a recording back, the next one starts from
    /// matrices made of Blink's own instead of the steps that led to them.
    fn flush(&mut self) {
        if std::mem::take(&mut self.unflushed) {
            for state in self.stack.iter_mut().chain([&mut self.state]) {
                state.ctm = Matrix::from(&state.transform);
            }
        }
    }

    /// Blink keeps the current path in the current user space: a transform change maps the
    /// path by the inverse of the change.
    fn map_path(&mut self, inverse: &Affine) {
        if self.invertible() {
            self.fast = Fast::None;
            self.path.transform(inverse);
        }
    }

    fn scale(&mut self, x: f64, y: f64) {
        if !finite(&[x, y]) {
            return;
        }
        let (sx, sy) = (clamp32(x), clamp32(y));
        let (x, y) = (f64::from(sx), f64::from(sy));
        let new = self.state.transform.then(&Affine::scaling(x, y));
        if new == self.state.transform {
            return;
        }
        self.state.transform = new;
        self.state.ctm.m44_scale(sx, sy);
        self.map_path(&Affine::scaling(1.0 / x, 1.0 / y));
    }

    fn rotate(&mut self, angle: f64) {
        if !angle.is_finite() {
            return;
        }
        let new = self.state.transform.then(&Affine::rotation(angle));
        if new == self.state.transform {
            return;
        }
        self.state.transform = new;
        let degrees = clamp32(angle * (180.0 / f64::from(std::f32::consts::PI)));
        let rotation = Matrix::rotation(degrees);
        if !rotation.is_identity() {
            self.state.ctm.m44_concat(&rotation);
        }
        self.map_path(&Affine::rotation(-angle));
    }

    fn translate(&mut self, x: f64, y: f64) {
        if !self.invertible() || !finite(&[x, y]) {
            return;
        }
        let (tx, ty) = (clamp32(x), clamp32(y));
        let (x, y) = (f64::from(tx), f64::from(ty));
        let new = self.state.transform.then(&Affine::translation(x, y));
        if new == self.state.transform {
            return;
        }
        self.state.transform = new;
        self.state.ctm.m44_translate(tx, ty);
        self.map_path(&Affine::translation(-x, -y));
    }

    fn reset_transform(&mut self) {
        let old = self.state.transform;
        if old == Affine::IDENTITY {
            return;
        }
        let invertible = old.is_invertible();
        self.state.transform = Affine::IDENTITY;
        self.state.ctm = Matrix::IDENTITY;
        if invertible {
            self.map_path(&old);
        }
    }

    /// Blink's `transform()`.
    fn apply_transform(&mut self, m: &[f64; 6]) {
        let [a, b, c, d, e, f] = m.map(|v| f64::from(clamp32(v)));
        let change = Affine { a, b, c, d, e, f };
        let new = self.state.transform.then(&change);
        if new == self.state.transform {
            return;
        }
        self.state.transform = new;
        self.state.ctm.m44_concat(&Matrix::from(&change));
        if let Some(inverse) = change.inverse() {
            self.map_path(&inverse);
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
        let Some(p) = path_pt(x, y) else {
            return;
        };
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
        let v = [x, y, radius, start, end].map(sat32);
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
        let l = &mut self.lines;
        if l.lines_as_paths && l.draw_path_count != u32::MAX {
            l.draw_path_count += 1;
            if l.draw_path_count > 4 {
                l.draw_line_count = 0;
            }
        }
    }

    /// The paint source for `fill` with the paint alpha scaled by `coverage_scale / 256`, the
    /// way `SkDraw::drawPath` modulates a thin stroke drawn as a hairline.
    fn source(&self, fill: &Fill, coverage_scale: u32) -> Option<Source> {
        let alpha = self.state.alpha as f32;
        let ctm = &self.state.ctm;
        let modulate = |a: f32| {
            if coverage_scale >= 256 {
                a
            } else {
                let a8 = (a * 255.0 + 0.5) as u32;
                ((a8 * coverage_scale) >> 8) as f32 / 255.0
            }
        };
        let clamped = |x: f64, y: f64| {
            let max = f32::MAX / 1e6;
            ((x as f32).clamp(-max, max), (y as f32).clamp(-max, max))
        };
        let (geometry, stops) = match fill {
            Fill::Color(c) => {
                let a = modulate(c.a * alpha);
                return Some(Source::Solid(
                    [c.r, c.g, c.b, a].map(|v| (v * 255.0 + 0.5) as u8),
                ));
            }
            Fill::Linear(_, s) | Fill::Radial(_, s) if s.is_empty() => return None,
            Fill::Linear([x0, y0, x1, y1], s) => {
                if x0 == x1 && y0 == y1 {
                    return None;
                }
                let (p0, p1) = (clamped(*x0, *y0), clamped(*x1, *y1));
                (gradient::Geometry::linear(ctm, p0, p1)?, s)
            }
            &Fill::Radial([x0, y0, r0, x1, y1, r1], ref s) => {
                if r0 < 0.0 || r1 < 0.0 || (x0 == x1 && y0 == y1 && r0 == r1) {
                    return None;
                }
                let g = [x0, y0, r0, x1, y1, r1].map(|v| v as f32);
                (gradient::Geometry::radial(ctm, g)?, s)
            }
        };
        let colors = gradient::Colors::new(stops);
        let paint = gradient::Paint::new(geometry, colors, modulate(alpha));
        Some(Source::Gradient(Box::new(paint)))
    }

    /// Blends `source` through `spans` in blit order.
    fn paint_spans(&mut self, spans: &Spans, source: &Source) {
        let clipped;
        let spans = match &self.state.clip {
            None => spans,
            Some(clip) => {
                clipped = clip.apply(spans.clone());
                &clipped
            }
        };
        match source {
            Source::Solid(color) => scan::blit_spans_solid(&mut self.pixmap, spans, *color),
            Source::Gradient(paint) => gradient::blit(&mut self.pixmap, spans, paint),
        }
    }

    /// Fills a device path with Skia's analytic AA.
    fn fill_sk(&mut self, path: &SkPath, fill: &Fill) {
        self.fill_sk_rule(path, fill, false);
    }

    fn fill_sk_rule(&mut self, path: &SkPath, fill: &Fill, even_odd: bool) {
        if !dev_path_drawable(path) {
            return;
        }
        let Some(source) = self.source(fill, 256) else {
            return;
        };
        let spans = self.path_spans(path, even_odd);
        self.paint_spans(&spans, &source);
    }

    /// The blits `SkDraw::drawDevPath` makes to fill a device path.
    fn path_spans(&self, path: &SkPath, even_odd: bool) -> Spans {
        let mut spans = Spans::new(self.pixmap.width(), self.pixmap.height());
        if dev_path_drawable(path) {
            let (bounds, force_rle) = match &self.state.clip {
                None => (self.canvas_rect(), false),
                Some(clip) => (clip.bounds(), clip.force_rle()),
            };
            aaa::fill_path_clipped(path, bounds, force_rle, even_odd, &mut spans);
        }
        spans
    }

    fn canvas_rect(&self) -> [i32; 4] {
        [
            0,
            0,
            self.pixmap.width() as i32,
            self.pixmap.height() as i32,
        ]
    }

    /// Blink's `clip()`: antialiased, and a rect path that the transform keeps axis-aligned
    /// clips as a rect.
    fn clip_current_path(&mut self) {
        if !self.invertible() {
            return;
        }
        let clip = self
            .state
            .clip
            .take()
            .unwrap_or_else(|| RasterClip::Rect(self.canvas_rect()));
        let user = self.path.to_sk(None);
        let device = user
            .is_rect()
            .and_then(|r| self.device_rect([r.left, r.top, r.right, r.bottom]));
        self.state.clip = Some(match device {
            Some(device) => clip.intersect_rect(ltrb(device)),
            None => clip.intersect_path(&self.path.to_sk(Some(&self.state.ctm))),
        });
    }

    fn fill_current_path(&mut self, even_odd: bool) {
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
        if self.path_rejected(&self.path.to_sk(None), false) {
            return;
        }
        let device = self.path.to_sk(Some(&self.state.ctm));
        let fill = self.state.fill.clone();
        self.fill_sk_rule(&device, &fill, even_odd);
    }

    /// `SkDrawTreatAsHairline`: a stroke whose width maps to at most one device pixel on both
    /// axes is drawn one pixel wide with its alpha scaled by the mean mapped width.
    fn hairline_coverage(&self) -> Option<f32> {
        let m = self.state.ctm;
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
        let m = self.state.ctm;
        let res_scale = tiny_skia::PathStroker::compute_resolution_scale(
            &tiny_skia::Transform::from_row(m.sx, m.ky, m.kx, m.sy, m.tx, m.ty),
        );
        let user = path.to_sk(None);
        if self.path_rejected(&user, true) {
            return;
        }
        let stroke = self.state.stroke.clone();
        let hairline = self.hairline_coverage();
        let s = &self.state;
        let rec = dash::StrokeRec {
            width: if hairline.is_some() {
                0.0
            } else {
                s.line_width as f32
            },
            hairline: hairline.is_some(),
            cap: s.line_cap,
            miter: (s.line_join == LineJoin::Miter).then_some(s.miter_limit as f32),
            res_scale,
        };
        let dash = dash::Dash {
            intervals: s.dash.iter().map(|&v| v as f32).collect(),
            phase: s.dash_offset as f32,
        };
        let dashed = if s.dash.is_empty() {
            None
        } else {
            dash::dash_path(&user, &dash, &rec, self.local_clip_bounds())
        };
        let (source, filled) = match dashed {
            Some(d) => (d.path, d.filled),
            None => (user, false),
        };
        if let Some(coverage) = hairline {
            let device = source.mapped(&self.state.ctm);
            if !dev_path_drawable(&device) {
                return;
            }
            let Some(device) = device.to_tiny() else {
                return;
            };
            let Some(paint) = self.source(&stroke, (coverage * 256.0) as u32) else {
                return;
            };
            let (w, h) = (self.pixmap.width(), self.pixmap.height());
            for seg in hairline::segments(&device, self.state.line_cap, w, h) {
                if let Some(spans) = hairline::spans(seg, w, h) {
                    self.paint_spans(&spans, &paint);
                }
            }
            return;
        }
        if filled {
            let device = source.mapped(&self.state.ctm);
            self.fill_sk(&device, &stroke);
            return;
        }
        let s = &self.state;
        let style = stroke::Style {
            width: s.line_width as f32,
            miter_limit: s.miter_limit as f32,
            cap: s.line_cap,
            join: s.line_join,
            res_scale,
        };
        let outline = stroke::stroke_path(&source, &style);
        let device = outline.mapped(&self.state.ctm);
        self.fill_sk(&device, &stroke);
    }

    /// `SkDraw::computeConservativeLocalClipBounds`: the canvas outset by a pixel, in user space.
    fn local_clip_bounds(&self) -> Option<crate::skpath::Rect> {
        let (w, h) = (self.pixmap.width() as f32, self.pixmap.height() as f32);
        Some(
            self.state
                .ctm
                .invert()?
                .map_rect(&ltrb([-1.0, -1.0, w + 1.0, h + 1.0])),
        )
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
        let device = path.to_sk(Some(&self.state.ctm));
        let fill = self.state.fill.clone();
        self.fill_sk(&device, &fill);
    }

    /// `cc::RecordPaintCanvas::drawLine` then `SkDraw::drawPoints`: once more than four paths
    /// were drawn since the last run of five lines, cc replays the line as a path; otherwise a
    /// dashed line that `SkDashPathEffect::asPoints` accepts becomes rects. Returns false when
    /// the line is to be stroked as an ordinary path.
    fn stroke_line(&mut self, a: Pt, b: Pt) -> bool {
        let l = &mut self.lines;
        if l.lines_as_paths && l.draw_line_count != u32::MAX {
            l.draw_line_count += 1;
            if l.draw_line_count > 4 {
                l.draw_path_count = 0;
            }
        }
        if l.draw_path_count > 4 {
            return false;
        }
        let Some(points) = self.dash_as_points(a, b) else {
            return false;
        };
        let stroke = self.state.stroke.clone();
        let ctm = self.state.ctm;
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
            if let Some(spans) = self.rect_spans(sorted([l, t, r, b]), true) {
                self.paint_spans(&spans, &source);
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
        if !s.ctm.rect_stays_rect() {
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
        let bounds = s.ctm.invert()?.map_rect(&ltrb([
            -radius,
            -radius,
            self.pixmap.width() as f32 + radius,
            self.pixmap.height() as f32 + radius,
        ]));
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
            let (lo, hi) = chop(lo, hi, bounds.left, bounds.right)?;
            (pts[0].x, pts[1].x) = if dx < 0.0 { (hi, lo) } else { (lo, hi) };
        } else {
            let (lo, hi) = if dy < 0.0 {
                (pts[1].y, pts[0].y)
            } else {
                (pts[0].y, pts[1].y)
            };
            let (lo, hi) = chop(lo, hi, bounds.top, bounds.bottom)?;
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

    /// The device rect `SkDraw::drawRect` maps a sorted LTRB rect to, when the transform keeps
    /// rects axis-aligned.
    fn device_rect(&self, r: [f32; 4]) -> Option<[f32; 4]> {
        let m = &self.state.ctm;
        if !m.rect_stays_rect() {
            return None;
        }
        let a = m.map_point(hairline::P { x: r[0], y: r[1] });
        let b = m.map_point(hairline::P { x: r[2], y: r[3] });
        Some(sorted([a.x, a.y, b.x, b.y]))
    }

    fn rect_path(&self, r: [f32; 4]) -> Path {
        let mut path = Path::new();
        path.add_rect_ltrb(r);
        path
    }

    /// `SkCanvas::internalQuickReject` for user bounds outset by `outset` on each side:
    /// non-finite bounds, before or after the transform, or bounds outside the clip draw nothing.
    fn quick_reject(&self, r: [f32; 4], outset: f32) -> bool {
        if !r.iter().all(|v| v.is_finite()) {
            return true;
        }
        let r = [r[0] - outset, r[1] - outset, r[2] + outset, r[3] + outset];
        let corners = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])];
        let mapped = corners.map(|(x, y)| self.state.ctm.map_point(hairline::P { x, y }));
        if !mapped.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
            return true;
        }
        let l = mapped.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let t = mapped.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let rr = mapped.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let b = mapped.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        let (w, h) = (self.pixmap.width() as f32, self.pixmap.height() as f32);
        !(l.max(-1.0) < rr.min(w + 1.0) && t.max(-1.0) < b.min(h + 1.0))
    }

    /// The blits `SkDraw::drawRect` makes to fill a sorted LTRB user rect: `AntiFillRect` when
    /// the transform keeps it axis-aligned and it fits in fixed point, an analytic AA path fill
    /// otherwise; without antialiasing, Skia's rounded edges.
    fn rect_spans(&self, rect: [f32; 4], anti_alias: bool) -> Option<Spans> {
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        if self.quick_reject(rect, 0.0) {
            return None;
        }
        if !anti_alias {
            return self.rect_coverage(rect, false).map(|c| c.to_spans(w, h));
        }
        if let Some(r) = self.device_rect(rect) {
            if too_big_for_math(&ltrb(r)) {
                return None;
            }
            if fits_in_fixed(r) {
                return Some(scan::anti_fill_rect(r, w, h));
            }
        }
        let device = self.rect_path(rect).to_sk(Some(&self.state.ctm));
        Some(self.path_spans(&device, false))
    }

    fn rect_coverage(&self, rect: [f32; 4], anti_alias: bool) -> Option<Coverage> {
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        if self.quick_reject(rect, 0.0) {
            return None;
        }
        if !anti_alias && let Some(device) = self.device_rect(rect) {
            if too_big_for_math(&ltrb(device)) {
                return None;
            }
            let round = |v: f32| (v + 0.5).floor().clamp(0.0, 1e9) as i64;
            let (l, t) = (round(device[0]), round(device[1]));
            let r = round(device[2]).min(i64::from(w));
            let b = round(device[3]).min(i64::from(h));
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
        let path = self
            .rect_path(rect)
            .to_skia(Some(&self.state.ctm), CONIC_TOLERANCE)?;
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
        let [x, y, w, h] = rect32(rect);
        let fill = self.state.fill.clone();
        if let (Some(source), Some(spans)) = (
            self.source(&fill, 256),
            self.rect_spans(sorted([x, y, x + w, y + h]), true),
        ) {
            self.paint_spans(&spans, &source);
        }
    }

    /// Blink's `strokeRect`: a rect with one empty side is a closed line path, any other goes
    /// through `SkDraw::drawRect`.
    fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let Some(rect) = validate_rect(x, y, w, h) else {
            return;
        };
        let [x, y, w, h] = rect32(rect);
        if (w > 0.0) != (h > 0.0) {
            if self.invertible() {
                self.count_draw_path();
            }
            let mut path = Path::new();
            path.move_to(f64::from(x), f64::from(y));
            path.line_to(f64::from(x + w), f64::from(y + h));
            path.close();
            self.stroke_user_path(&path);
            return;
        }
        let r = sorted([x, y, x + w, y + h]);
        if !self.invertible() || self.quick_reject(r, self.stroke_outset()) {
            return;
        }
        if self.frame_rect(r) {
            return;
        }
        let path = self.rect_path(r);
        self.stroke_user_path(&path);
    }

    /// `SkStrokeRec::GetInflationRadius` for the stroke paint, in user space.
    fn stroke_outset(&self) -> f32 {
        let s = &self.state;
        let mut multiplier = 1.0f32;
        if s.line_join == LineJoin::Miter {
            multiplier = multiplier.max(s.miter_limit as f32);
        }
        if s.line_cap == LineCap::Square {
            multiplier = multiplier.max(std::f32::consts::SQRT_2);
        }
        s.line_width as f32 / 2.0 * multiplier
    }

    /// `SkCanvas::onDrawPath`'s checks on the user path: not finite, or quick-rejected.
    fn path_rejected(&self, path: &SkPath, stroke: bool) -> bool {
        if !path.is_finite() {
            return true;
        }
        if path.pts.is_empty() {
            return false;
        }
        let b = path.bounds();
        let outset = if stroke { self.stroke_outset() } else { 0.0 };
        self.quick_reject([b.left, b.top, b.right, b.bottom], outset)
    }

    /// `SkDraw::drawRect` for a stroke with mitred corners on an axis-aligned rect:
    /// `AntiFrameRect` when it fits in fixed point. Returns false when Skia strokes the rect as a
    /// path instead.
    fn frame_rect(&mut self, rect: [f32; 4]) -> bool {
        let s = &self.state;
        if !s.dash.is_empty()
            || s.line_join != LineJoin::Miter
            || (s.miter_limit as f32) < std::f32::consts::SQRT_2
            || rect[0] >= rect[2]
            || rect[1] >= rect[3]
        {
            return false;
        }
        let Some(device) = self.device_rect(rect) else {
            return false;
        };
        let m = s.ctm;
        let lw = s.line_width as f32;
        let size = if m.is_scale_translate() {
            ((lw * m.sx).abs(), (lw * m.sy).abs())
        } else {
            ((lw * m.kx).abs(), (lw * m.ky).abs())
        };
        let bbox = [
            device[0] - size.0 / 2.0,
            device[1] - size.1 / 2.0,
            device[2] + size.0 / 2.0,
            device[3] + size.1 / 2.0,
        ];
        if too_big_for_math(&ltrb(bbox)) {
            return true;
        }
        if !fits_in_fixed(bbox) {
            return false;
        }
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        let spans = scan::anti_frame_rect(device, size, w, h);
        let stroke = s.stroke.clone();
        if let Some(source) = self.source(&stroke, 256) {
            self.paint_spans(&spans, &source);
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
        let [x, y, w, h] = rect32(rect);
        if let Some(coverage) = self.rect_coverage(sorted([x, y, x + w, y + h]), false) {
            match &self.state.clip {
                None => scan::blit(&mut self.pixmap, &coverage, Blend::Clear, |_, _| [0; 4]),
                Some(clip) => {
                    let spans = coverage.to_spans(self.pixmap.width(), self.pixmap.height());
                    let spans = clip.apply(spans);
                    scan::blit_spans(&mut self.pixmap, &spans, Blend::Clear, |_, _| [0; 4]);
                }
            }
        }
    }

    fn fill_text(&mut self, text: &JsString, x: f64, y: f64, max_width: Option<f64>) {
        let fill = self.state.fill.clone();
        let Some(source) = self.source(&fill, 256) else {
            return;
        };
        let luminance = match &fill {
            Fill::Color(color) => color.to_u8(),
            Fill::Linear(_, stops) | Fill::Radial(_, stops) => gradient_luminance(stops),
        };
        let ctm = self.state.ctm;
        if !finite(&[x, y]) || !self.invertible() || !ctm.is_finite() {
            return;
        }
        let (x, y) = (f64::from(clamp32(x)), f64::from(clamp32(y)));
        let state = &self.state;
        let Some(TextRaster { backend, lut }) = self.text.as_mut() else {
            return;
        };
        let glyphs = place_text(
            &mut **backend,
            &state.font,
            &state.align,
            &state.baseline,
            &text.to_string_lossy(),
            x,
            y,
            max_width,
            [ctm.sx, ctm.ky, ctm.kx, ctm.sy, ctm.tx, ctm.ty].map(f64::from),
        );
        let mut spans = Spans::new(self.pixmap.width(), self.pixmap.height());
        let mut color_glyphs = Vec::new();
        for Placed { x: ix, y: iy, mask } in glyphs {
            if mask.color {
                color_glyphs.push((ix, iy, mask));
                continue;
            }
            for row in 0..mask.height as i32 {
                let start = (row * mask.width as i32) as usize;
                let coverage: Vec<u8> = mask.data[start..start + mask.width as usize]
                    .iter()
                    .map(|&raw| lut.apply(luminance, raw))
                    .collect();
                spans.anti_h(ix + mask.left, iy + mask.top + row, &coverage);
            }
        }
        self.paint_spans(&spans, &source);
        let paint_alpha = match &fill {
            Fill::Color(color) => color.a * self.state.alpha as f32,
            _ => self.state.alpha as f32,
        };
        let a8 = (paint_alpha * 255.0 + 0.5) as u8;
        for (ix, iy, mask) in color_glyphs {
            self.blit_color_glyph(ix + mask.left, iy + mask.top, &mask, a8);
        }
    }

    /// An ARGB glyph image drawn as Skia draws colour glyphs: its own pixels, scaled by the
    /// paint's alpha, source-over.
    fn blit_color_glyph(&mut self, left: i32, top: i32, mask: &crate::text::Mask, alpha: u8) {
        let mut spans = Spans::new(self.pixmap.width(), self.pixmap.height());
        let row = vec![alpha; mask.width as usize];
        for y in 0..mask.height as i32 {
            spans.anti_h(left, top + y, &row);
        }
        if let Some(clip) = &self.state.clip {
            spans = clip.apply(spans);
        }
        let w = mask.width as usize;
        scan::blit_spans(&mut self.pixmap, &spans, Blend::SourceOver, |x, y| {
            let i = ((y as i32 - top) as usize * w + (x as i32 - left) as usize) * 4;
            let p = &mask.data[i..i + 4];
            [p[0], p[1], p[2], p[3]]
        });
    }

    fn draw_image(&mut self, id: &str, args: &[f64]) {
        let Some(ImageSource { pixmap, element }) = self.images.get(id) else {
            return;
        };
        let (pixmap, element) = (pixmap.clone(), *element);
        let size = [f64::from(pixmap.width()), f64::from(pixmap.height())];
        let (src, dst) = match *args {
            [dx, dy] => ([0.0, 0.0, size[0], size[1]], [dx, dy, size[0], size[1]]),
            [dx, dy, dw, dh] => ([0.0, 0.0, size[0], size[1]], [dx, dy, dw, dh]),
            [sx, sy, sw, sh, dx, dy, dw, dh] => ([sx, sy, sw, sh], [dx, dy, dw, dh]),
            _ => return,
        };
        if !finite(&src) || !finite(&dst) || !self.invertible() {
            return;
        }
        if [src[2], src[3], dst[2], dst[3]].contains(&0.0) {
            return;
        }
        let normalized = |[x, y, w, h]: [f64; 4]| {
            let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
            let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
            rect32([x, y, w, h])
        };
        let Some((src, dst)) = clip_to_image(normalized(src), normalized(dst), size) else {
            return;
        };
        let ctm = self.state.ctm;
        let anti_alias = !ctm.rect_stays_rect() || {
            let (x, y) = if ctm.is_scale_translate() {
                (ctm.sx, ctm.sy)
            } else {
                (ctm.ky, ctm.kx)
            };
            dst[2] * x.abs() < 1.0 || dst[3] * y.abs() < 1.0
        };
        let mut draw = ImageDraw {
            pixmap,
            mip: None,
            src: ltrb(xywh(src)),
            dst: ltrb(xywh(dst)),
            linear: self.state.smoothing,
            anti_alias,
        };
        if element {
            let m = ctm.concat(&Matrix::rect_to_rect(&draw.src, &draw.dst));
            let scale = [m.sx.hypot(m.ky), m.kx.hypot(m.sy)];
            let image = [draw.pixmap.width(), draw.pixmap.height()];
            match Mip::of(&draw.src, scale, image) {
                Level::Nothing => return,
                Level::Mip(mip) if draw.linear => {
                    draw.src = mip.placed(&draw.src);
                    draw.mip = Some(mip);
                }
                _ => {}
            }
        }
        self.draw_image_rect(draw);
    }

    /// `SkBitmapDevice::drawImageRect` with the fast constraint: a whole bitmap (and, without
    /// filtering, a part of one) is drawn as a sprite where it covers its own pixels.
    fn draw_image_rect(&mut self, draw: ImageDraw) {
        let ImageDraw {
            pixmap,
            mip,
            mut src,
            mut dst,
            linear,
            anti_alias,
        } = draw;
        if self.quick_reject([dst.left, dst.top, dst.right, dst.bottom], 0.0) {
            return;
        }
        let size = mip.map_or([pixmap.width(), pixmap.height()], |m| m.size);
        let bounds = ltrb([0.0, 0.0, size[0] as f32, size[1] as f32]);
        let mut matrix = Matrix::rect_to_rect(&src, &dst);
        if !bounds.contains(&src) {
            let Some(inside) = src.intersect(&bounds) else {
                return;
            };
            src = inside;
            dst = matrix.map_rect(&src);
        }
        let subset = !src.contains(&bounds);
        let mut part = [0, 0, size[0], size[1]];
        if subset && !linear {
            let [l, t, r, b] = src.round_out().map(|v| v as u32);
            part = [l, t, r.min(size[0]) - l, b.min(size[1]) - t];
            if l > 0 || t > 0 {
                matrix = matrix.pre_translate(l as f32, t as f32);
            }
        }
        let as_bitmap =
            !subset || (!linear && src == ltrb([0.0, 0.0, part[2] as f32, part[3] as f32]));
        let total = self.state.ctm.concat(&matrix);
        let alpha = self.state.alpha as f32;
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        let level = |shades: &mut ShadeCache| match mip {
            Some(mip) => shades.level(&pixmap, mip),
            None => Some(pixmap.clone()),
        };
        if as_bitmap && let Some(at) = image::sprite(&total, part, linear, anti_alias) {
            let sprite = [at.0, at.1, at.0 + part[2] as i32, at.1 + part[3] as i32];
            let clip = self.state.clip.as_ref();
            if clip.is_none_or(|c| c.handles_sprite(sprite)) {
                let mut spans = Spans::new(w, h);
                spans.rect(at.0, at.1, part[2] as i32, part[3] as i32);
                if let Some(clip) = clip {
                    spans = clip.apply(spans);
                }
                let runs = image::Runs::new(&spans);
                let alpha = image::alpha_u8(alpha);
                if let Some(image) = level(&mut self.shades) {
                    image::blit_sprite(&mut self.pixmap, &image, part, at, alpha, &runs);
                }
                return;
            }
        }
        let Some(shader) = image::Shader::new(&total, part, linear, alpha) else {
            return;
        };
        let spans = if as_bitmap && !self.state.ctm.rect_stays_rect() {
            let path = self.rect_path([0.0, 0.0, part[2] as f32, part[3] as f32]);
            Some(self.path_spans(&path.to_sk(Some(&total)), false))
        } else {
            self.rect_spans([dst.left, dst.top, dst.right, dst.bottom], anti_alias)
        };
        let Some(mut spans) = spans else {
            return;
        };
        if let Some(clip) = &self.state.clip {
            spans = clip.apply(spans);
        }
        let runs = image::Runs::new(&spans);
        let shade = |shades: &mut ShadeCache| {
            level(shades).map(|image| Arc::new(shader.shade(&image, &runs)))
        };
        let pixels = if shader.is_translate() {
            shade(&mut self.shades)
        } else {
            let key = ShadeKey {
                shader,
                mip,
                runs: runs.clone(),
            };
            self.shades.get(&pixmap, &key).or_else(|| {
                let pixels = shade(&mut self.shades)?;
                self.shades.insert(&pixmap, key, pixels.clone());
                Some(pixels)
            })
        };
        let Some(pixels) = pixels else {
            return;
        };
        shader.blend(&mut self.pixmap, &runs, &pixels);
    }
}

/// What cc hands Skia for a `drawImage`: part of an image, or of the mip level of it that the
/// decode cache keeps, and the user rect it fills.
struct ImageDraw {
    pixmap: Arc<Pixmap>,
    mip: Option<Mip>,
    src: crate::skpath::Rect,
    dst: crate::skpath::Rect,
    linear: bool,
    anti_alias: bool,
}

/// `gfx::RectFToSkRect`.
fn xywh([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    [x, y, x + w, y + h]
}

/// Sizes a `gfx::SizeF` keeps.
fn size32(v: f32) -> f32 {
    if v > 8.0 * f32::EPSILON { v } else { 0.0 }
}

/// `gfx::RectF::Intersect`.
fn intersect_xywh(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let (a, b) = (xywh(a), xywh(b));
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    let (w, h) = (size32(r[2] - r[0]), size32(r[3] - r[1]));
    (r[0] < r[2] && r[1] < r[3] && w > 0.0 && h > 0.0).then_some([r[0], r[1], w, h])
}

/// Blink's `ClipRectsToImageRect` and the clip `StaticBitmapImage::DrawHelper` repeats, in
/// `[x, y, width, height]`: `None` where nothing is left to draw.
fn clip_to_image(src: [f32; 4], dst: [f32; 4], size: [f64; 2]) -> Option<([f32; 4], [f32; 4])> {
    let image = [0.0, 0.0, size[0] as f32, size[1] as f32];
    let s = xywh(src);
    let contained = s[0] >= 0.0 && s[1] >= 0.0 && s[2] <= image[2] && s[3] <= image[3];
    let (mut src, mut dst) = (src, dst);
    if !contained {
        let (kx, ky) = (dst[2] / src[2], dst[3] / src[3]);
        let offset = (dst[0] - src[0] * kx, dst[1] - src[1] * ky);
        src = intersect_xywh(src, image)?;
        dst = [
            src[0] * kx + offset.0,
            src[1] * ky + offset.1,
            size32(src[2] * kx),
            size32(src[3] * ky),
        ];
    }
    if src[2] <= 0.0 || src[3] <= 0.0 || dst[2] <= 0.0 || dst[3] <= 0.0 {
        return None;
    }
    Some((intersect_xywh(src, image)?, dst))
}

/// `SkGradientBaseShader::onAsLuminanceColor`, which text masks are gamma-corrected for: the
/// plain average of the stop colours Skia keeps, after Blink pads stops at 0 and 1 and Skia's
/// constructor drops the inner stops of any run of three or more at one offset.
fn gradient_luminance(stops: &[(f32, Rgba)]) -> [u8; 3] {
    let mut padded: Vec<(f32, Rgba)> = Vec::with_capacity(stops.len() + 2);
    if let Some(&(offset, color)) = stops.first()
        && offset > 0.0
    {
        padded.push((0.0, color));
    }
    padded.extend_from_slice(stops);
    if let Some(&(offset, color)) = padded.last()
        && offset < 1.0
    {
        padded.push((1.0, color));
    }
    let colors: Vec<Rgba> = if skia_uniform_stops(&padded) {
        padded.iter().map(|s| s.1).collect()
    } else {
        padded
            .chunk_by(|x, y| x.0 == y.0)
            .flat_map(|run| {
                let last = (run.len() > 1).then(|| run[run.len() - 1].1);
                std::iter::once(run[0].1).chain(last)
            })
            .collect()
    };
    let scale = 1.0 / colors.len().max(1) as f32;
    let sum = |f: fn(&Rgba) -> f32| colors.iter().map(f).fold(0.0f32, |a, b| a + b) * scale;
    [sum(|c| c.r), sum(|c| c.g), sum(|c| c.b)].map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
}

/// Skia's test for evenly spaced stops, which it keeps as implicit positions without removing
/// duplicates.
fn skia_uniform_stops(stops: &[(f32, Rgba)]) -> bool {
    let Some(&(step, _)) = stops.get(1) else {
        return true;
    };
    let mut prev = 0.0f32;
    stops[1..].iter().all(|&(offset, _)| {
        let curr = offset.clamp(prev, 1.0);
        let uniform = (step - (curr - prev)).abs() <= 1.0 / 4096.0;
        prev = curr;
        uniform
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Canvas2d;

    #[test]
    fn gradient_luminance_drops_inner_stops_of_a_shared_offset() {
        let dark = Rgba::from_u8(0x22, 0x22, 0x22, 1.0);
        let white = Rgba::from_u8(255, 255, 255, 1.0);
        let stops = |middle: &[Rgba]| {
            let mut v = vec![(0.0, dark), (0.5, dark)];
            v.extend(middle.iter().map(|&c| (0.5, c)));
            v.extend([(0.5, dark), (1.0, dark)]);
            v
        };
        assert_eq!(gradient_luminance(&stops(&[white, white])), [0x22; 3]);
        assert_eq!(gradient_luminance(&[(0.5, dark), (0.5, white)]), [0x91; 3]);
        assert_eq!(
            gradient_luminance(&[(0.0, white), (0.5, dark), (1.0, dark)]),
            [0x6c; 3]
        );
    }

    #[test]
    fn set_transform_with_a_number_that_is_not_finite_does_nothing() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut c = Canvas2d::new(20, 10);
            c.translate(10.0, 0.0);
            c.set_transform([1.0, 0.0, 0.0, 1.0, bad, 0.0]);
            c.fill_rect(0.0, 0.0, 10.0, 10.0);
            let pixmap = render(&c);
            assert_eq!(pixmap.pixel(5, 5).unwrap().alpha(), 0);
            assert_eq!(pixmap.pixel(15, 5).unwrap().alpha(), 255);
        }
    }

    #[test]
    fn quarter_turns_keep_rects_rects() {
        let rects = [[2.25, 3.5, 6.75, 4.125], [-1.5, 0.5, 0.25, 9.0]];
        let mut turned = Canvas2d::new(20, 20);
        turned.translate(14.0, 2.0);
        turned.rotate(std::f64::consts::FRAC_PI_2);
        let mut upright = Canvas2d::new(20, 20);
        upright.translate(14.0, 2.0);
        for [x, y, w, h] in rects {
            turned.fill_rect(x, y, w, h);
            turned.stroke_rect(x, y, w, h);
            upright.fill_rect(-y - h, x, h, w);
            upright.stroke_rect(-y - h, x, h, w);
        }
        assert!(render(&turned) == render(&upright));
    }

    #[test]
    fn translations_past_float_range_lose_the_canvas() {
        for (far, drawn) in [(1e38, 255), (2e38, 0)] {
            let mut c = Canvas2d::new(4, 4);
            c.scale(2.0, 2.0);
            c.translate(far, 0.0);
            c.translate(-far, 0.0);
            c.fill_rect(0.0, 0.0, 2.0, 2.0);
            assert_eq!(render(&c).pixel(1, 1).unwrap().alpha(), drawn, "{far}");
        }
    }

    #[test]
    fn a_readback_rebuilds_the_matrix_from_doubles() {
        let mut c = Canvas2d::new(4, 4);
        c.fill_rect(0.0, 0.0, 1.0, 1.0);
        c.rotate(std::f64::consts::FRAC_PI_2);
        let mut raster = Raster::new(4, 4).unwrap();
        raster.run(&c.calls);
        assert!(raster.state.ctm.rect_stays_rect());
        c.get_image_data(0.0, 0.0, 1.0, 1.0, "$a");
        let mut raster = Raster::new(4, 4).unwrap();
        raster.run(&c.calls);
        assert!(!raster.state.ctm.rect_stays_rect());
        assert_eq!(raster.state.ctm, Matrix::from(&raster.state.transform));
    }

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
    fn get_image_data_throws_instead_of_allocating() {
        let mut c = Canvas2d::new(4, 4);
        c.get_image_data(0.0, 0.0, 0.0, 2.0, "$zero");
        c.get_image_data(0.0, 0.0, 1e5, 1e5, "$int");
        c.get_image_data(0.0, 0.0, 40000.0, 40000.0, "$bytes");
        c.get_image_data(2_147_483_000.0, 0.0, 1000.0, 1.0, "$sum");
        c.get_image_data(0.0, 0.0, 4_294_967_298.0, 1.0, "$wrapped");
        let mut raster = Raster::new(4, 4).unwrap();
        raster.run(&c.calls);
        assert_eq!(
            raster.errors(),
            [
                (0, CanvasError::IndexSize("The source width is 0.")),
                (1, OUT_OF_MEMORY),
                (
                    2,
                    CanvasError::Range("Out of memory at ImageData creation.")
                ),
                (3, OUT_OF_MEMORY),
            ]
        );
        assert_eq!(raster.register("$wrapped").unwrap().width, 2);
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

#[cfg(test)]
mod dash_tests {
    use super::*;
    use crate::display::Canvas2d;

    #[test]
    fn zero_length_dashes_draw_round_caps() {
        let mut c = Canvas2d::new(300, 100);
        c.set_line_dash(&[0.0, 4.0]);
        c.set_line_width(2.0);
        c.set_line_cap("round");
        c.begin_path();
        c.move_to(10.0, 50.0);
        c.line_to(290.0, 50.0);
        c.stroke();
        let mut raster = Raster::new(300, 100).unwrap();
        raster.run(&c.calls);
        let p = raster.into_pixmap();
        let row: Vec<u8> = (0..40).map(|x| p.pixel(x, 50).unwrap().alpha()).collect();
        assert!(row.iter().any(|&a| a > 0), "{row:?}");
    }

    fn dashed_line_frame() -> Canvas2d {
        let mut c = Canvas2d::new(60, 20);
        c.set_line_width(2.0);
        c.set_line_dash(&[3.0, 3.0]);
        c.begin_path();
        c.move_to(5.0, 10.6);
        c.line_to(55.0, 10.6);
        c.stroke();
        c
    }

    fn after(state: RasterState, frame: &Canvas2d) -> (Pixmap, RasterState) {
        let mut raster = Raster::new(60, 20).unwrap().with_state(state);
        raster.run(&frame.calls);
        let state = raster.state();
        (raster.into_pixmap(), state)
    }

    #[test]
    fn lines_as_paths_counters_outlive_a_frame_but_not_a_resize() {
        let mut paths = Canvas2d::new(60, 20);
        for i in 0..6 {
            paths.begin_path();
            paths.rect(f64::from(i), 0.0, 1.0, 1.0);
            paths.fill();
        }
        let line = dashed_line_frame();
        let (fresh, _) = after(RasterState::new(true), &line);
        let (_, counted) = after(RasterState::new(true), &paths);
        let (persisted, _) = after(counted, &line);
        assert_ne!(fresh, persisted);
        let (resized, _) = after(counted.for_size(61, 20), &line);
        assert_eq!(fresh, resized);
        let (_, off) = after(RasterState::new(false), &paths);
        assert_eq!(after(off, &line).0, fresh);
    }
}

#[cfg(test)]
mod shade_tests {
    use super::*;
    use crate::display::Canvas2d;

    fn image() -> Arc<Pixmap> {
        sized(90, 60)
    }

    fn sized(width: u32, height: u32) -> Arc<Pixmap> {
        let mut pixmap = Pixmap::new(width, height).unwrap();
        for (i, p) in pixmap
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let v = (i * 37 % 251) as u8;
            *p = [v / 3, v / 2, v, v];
        }
        Arc::new(pixmap)
    }

    fn frame(width: f64, x: f64) -> Canvas2d {
        let mut c = Canvas2d::new(120, 80);
        c.clear_rect(0.0, 0.0, 120.0, 80.0);
        c.draw_image_scaled("a", 3.25, 2.5, width, 50.0);
        c.set_fill_style("#ff0000");
        c.fill_rect(x, 10.0, 8.0, 8.0);
        c
    }

    fn draw(c: &Canvas2d, image: &Arc<Pixmap>, shades: ShadeCache) -> (Pixmap, ShadeCache) {
        let mut raster = Raster::new(c.width, c.height).unwrap().with_shades(shades);
        raster.add_image_element("a", image.clone());
        raster.run(&c.calls);
        let shades = raster.take_shades();
        (raster.into_pixmap(), shades)
    }

    #[test]
    fn resampled_images_carry_over_to_equal_draws_only() {
        for width in [101.0, 40.0, 20.0] {
            let image = image();
            let (_, shades) = draw(&frame(width, 5.0), &image, ShadeCache::default());
            assert_eq!(shades.len(), 1);
            let sampled = Arc::as_ptr(&shades.entries[0].pixels);
            let (cached, shades) = draw(&frame(width, 40.0), &image, shades);
            assert_eq!(Arc::as_ptr(&shades.entries[0].pixels), sampled);
            let (fresh, _) = draw(&frame(width, 40.0), &image, ShadeCache::default());
            assert!(cached.data() == fresh.data(), "{width}");

            let wider = frame(width + 1.0, 40.0);
            let (cached, shades) = draw(&wider, &image, shades);
            let (fresh, _) = draw(&wider, &image, ShadeCache::default());
            assert!(cached.data() == fresh.data(), "{width}");
            assert_eq!(shades.len(), 1);

            let other = Arc::new(Pixmap::new(90, 60).unwrap());
            let (cached, _) = draw(&wider, &other, shades);
            let (fresh, _) = draw(&wider, &other, ShadeCache::default());
            assert!(cached.data() == fresh.data(), "{width}");
        }
    }

    #[test]
    fn levels_stay_with_their_image_and_make_the_smaller_ones() {
        let image = sized(91, 61);
        let at = |width: f64, height: f64| {
            let mut c = Canvas2d::new(120, 80);
            c.draw_image_scaled("a", 3.25, 2.5, width, height);
            c
        };
        let (_, shades) = draw(&at(40.0, 28.0), &image, ShadeCache::default());
        let half = Arc::as_ptr(&shades.levels[0].pixels);
        let (_, shades) = draw(&at(41.5, 28.0), &image, shades);
        assert_eq!(shades.levels.len(), 1);
        assert_eq!(Arc::as_ptr(&shades.levels[0].pixels), half);

        let (chained, shades) = draw(&at(20.0, 14.0), &image, shades);
        let (fresh, _) = draw(&at(20.0, 14.0), &image, ShadeCache::default());
        assert_eq!(shades.levels[0].mip.size, [46, 31]);
        assert_eq!(shades.levels[1].mip.size, [23, 16]);
        assert!(chained.data() != fresh.data());

        let (_, shades) = (1..90).fold(draw(&at(2.0, 2.0), &image, shades), |(_, s), w| {
            draw(&at(f64::from(w), f64::from(w) / 1.5), &image, s)
        });
        assert!(shades.levels.len() <= 7);
        let (_, shades) = draw(&at(40.0, 28.0), &Arc::new((*image).clone()), shades);
        assert_eq!(shades.levels.len(), 1);
    }
}
