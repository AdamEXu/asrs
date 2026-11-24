// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2008 and 2016 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! How Chromium's CPU canvas samples and blends images: sprites, `SkBitmapProcState` (the legacy
//! fixed-point shader) and, where Skia has no legacy shader for the matrix, the raster pipeline's
//! 8-bit stages.

use tiny_skia::Pixmap;

use crate::blit::{Kind, Spans};
use crate::hairline::P;
use crate::matrix::Matrix;
use crate::skpath::Rect;

type Pixel = [u8; 4];

/// Skia samples rotated images with the legacy shader only where it has NEON.
const LEGACY_AFFINE: bool = cfg!(target_arch = "aarch64");

/// `[x, y, width, height]` of an image.
pub type Part = [u32; 4];

#[derive(Clone, Copy)]
struct Bitmap<'a> {
    data: &'a [Pixel],
    stride: usize,
    part: Part,
}

impl<'a> Bitmap<'a> {
    fn new(image: &'a Pixmap, part: Part) -> Bitmap<'a> {
        Bitmap {
            data: image.data().as_chunks::<4>().0,
            stride: image.width() as usize,
            part,
        }
    }

    fn max_x(&self) -> i32 {
        self.part[2] as i32 - 1
    }

    fn max_y(&self) -> i32 {
        self.part[3] as i32 - 1
    }

    fn at(&self, x: i32, y: i32) -> Pixel {
        let x = self.part[0] as usize + x.clamp(0, self.max_x()) as usize;
        let y = self.part[1] as usize + y.clamp(0, self.max_y()) as usize;
        self.data[y * self.stride + x]
    }
}

/// `SkScalarRoundToInt`.
fn round_to_int(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

fn nearly(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

/// `SkTreatAsSprite`: where a part drawn through `matrix` covers exactly its own pixels, the
/// device position of its corner.
pub fn sprite(matrix: &Matrix, part: Part, linear: bool, anti_alias: bool) -> Option<(i32, i32)> {
    let m = matrix;
    if !m.is_scale_translate() {
        return None;
    }
    if linear && (m.tx != m.tx.trunc() || m.ty != m.ty.trunc()) {
        return None;
    }
    let (x, y) = (round_to_int(m.tx), round_to_int(m.ty));
    if !anti_alias && m.is_translate() {
        return Some((x, y));
    }
    if m.sx < 0.0 || m.sy < 0.0 {
        return None;
    }
    let (w, h) = (part[2] as i32, part[3] as i32);
    let scale = if anti_alias { 16 } else { 1 };
    let dst = m.map_rect(&Rect {
        left: 0.0,
        top: 0.0,
        right: w as f32,
        bottom: h as f32,
    });
    let drawn = [dst.left, dst.top, dst.right, dst.bottom].map(|v| round_to_int(v * scale as f32));
    let whole = [x, y, x + w, y + h].map(|v| v * scale);
    (drawn == whole).then_some((x, y))
}

/// An image shader under a paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shader {
    inverse: Matrix,
    part: Part,
    linear: bool,
    alpha: f32,
}

impl Shader {
    /// The shader of `part` of an image drawn through `matrix` (part pixels to device), `None`
    /// when the matrix is singular.
    pub fn new(matrix: &Matrix, part: Part, linear: bool, alpha: f32) -> Option<Shader> {
        Some(Shader {
            inverse: matrix.invert()?,
            part,
            linear,
            alpha,
        })
    }

    pub fn is_translate(&self) -> bool {
        self.inverse.is_translate()
    }

    fn mode(&self) -> Mode {
        if legacy_shader_can_handle(&self.inverse) {
            Mode::Legacy(Legacy::new(self))
        } else {
            Mode::Pipeline(Stages::new(self))
        }
    }

    /// The shader's pixels for every run in `runs`.
    pub fn shade(&self, image: &Pixmap, runs: &Runs) -> Vec<Pixel> {
        let bitmap = Bitmap::new(image, self.part);
        let mode = self.mode();
        let mut out = Vec::with_capacity(runs.runs.iter().map(|r| r.len as usize).sum());
        for run in &runs.runs {
            match &mode {
                Mode::Legacy(legacy) => legacy.span(&bitmap, run, &mut out),
                Mode::Pipeline(stages) => stages.span(&bitmap, run, &mut out),
            }
        }
        out
    }

    /// Blends shaded `runs` in order.
    pub fn blend(&self, pixmap: &mut Pixmap, runs: &Runs, pixels: &[Pixel]) {
        let legacy = legacy_shader_can_handle(&self.inverse);
        let (mut pixels, mut masks) = (pixels.iter(), runs.masks.iter());
        for run in &runs.runs {
            let row = row_mut(pixmap, run.x, run.y, run.len);
            for (d, &s) in row.iter_mut().zip(pixels.by_ref()) {
                let coverage = match run.coverage {
                    Some(c) => c,
                    None => masks.next().copied().unwrap_or(0),
                };
                *d = match (legacy, run.coverage) {
                    (false, _) => lowp_src_over(s, *d, coverage),
                    (true, None) => blend_mask(s, *d, coverage),
                    (true, Some(255)) => src_over(s, *d),
                    (true, Some(_)) => blend_argb32(s, *d, coverage),
                };
            }
        }
    }
}

/// Pixels side by side that one shader call covers: at one coverage, or at a mask's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub x: i32,
    pub y: i32,
    pub len: u32,
    pub coverage: Option<u8>,
}

/// The shader calls a blitter makes, and the coverage of those for masks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Runs {
    runs: Vec<Run>,
    masks: Vec<u8>,
}

impl Runs {
    pub fn new(spans: &Spans) -> Runs {
        let mut out = Runs::default();
        for (x, y, coverage, kind) in spans.iter_kinds() {
            if kind == Kind::Mask {
                out.runs.push(Run {
                    x,
                    y,
                    len: coverage.len() as u32,
                    coverage: None,
                });
                out.masks.extend_from_slice(coverage);
                continue;
            }
            let mut x = x;
            for group in coverage.chunk_by(|a, b| a == b) {
                if group[0] != 0 {
                    out.runs.push(Run {
                        x,
                        y,
                        len: group.len() as u32,
                        coverage: Some(group[0]),
                    });
                }
                x += group.len() as i32;
            }
        }
        out
    }
}

fn row_mut(pixmap: &mut Pixmap, x: i32, y: i32, len: u32) -> &mut [Pixel] {
    let start = y as usize * pixmap.width() as usize + x as usize;
    &mut pixmap.data_mut().as_chunks_mut::<4>().0[start..start + len as usize]
}

/// `Sprite_D32_S32`: `part` of `image` with its corner at `at`, through runs of full coverage.
pub fn blit_sprite(
    pixmap: &mut Pixmap,
    image: &Pixmap,
    part: Part,
    at: (i32, i32),
    alpha: u8,
    runs: &Runs,
) {
    if alpha == 0 {
        return;
    }
    let bitmap = Bitmap::new(image, part);
    for run in &runs.runs {
        let (sx, sy) = (run.x - at.0, run.y - at.1);
        for (i, d) in row_mut(pixmap, run.x, run.y, run.len)
            .iter_mut()
            .enumerate()
        {
            let s = bitmap.at(sx + i as i32, sy);
            *d = if alpha == 255 {
                src_over(s, *d)
            } else {
                blend_argb32(s, *d, alpha)
            };
        }
    }
}

/// `SkPaint::getAlpha`.
pub fn alpha_u8(alpha: f32) -> u8 {
    (alpha * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
}

fn zip(s: Pixel, d: Pixel, f: impl Fn(u32, u32) -> u32) -> Pixel {
    std::array::from_fn(|i| f(u32::from(s[i]), u32::from(d[i])).min(255) as u8)
}

/// `blit_row_s32a_opaque`, which is exact with NEON and a shift elsewhere.
fn src_over(s: Pixel, d: Pixel) -> Pixel {
    let a = u32::from(s[3]);
    if cfg!(target_arch = "aarch64") {
        zip(s, d, |s, d| s + crate::scan::div255(d * (255 - a)))
    } else {
        zip(s, d, |s, d| s + ((d * (256 - a)) >> 8))
    }
}

/// `SkBlendARGB32`.
fn blend_argb32(s: Pixel, d: Pixel, alpha: u8) -> Pixel {
    let scale = u32::from(alpha) + 1;
    let inverse = 0xffff - u32::from(s[3]) * scale;
    let inverse = (inverse + (inverse >> 8)) >> 8;
    zip(s, d, |s, d| (s * scale + d * inverse) >> 8)
}

/// `blend_row_A8`.
fn blend_mask(s: Pixel, d: Pixel, coverage: u8) -> Pixel {
    let s = s.map(|v| ((u32::from(v) * (u32::from(coverage) + 1)) >> 8) as u8);
    let inverse = 256 - u32::from(s[3]);
    zip(s, d, |s, d| s + ((d * inverse) >> 8))
}

/// The raster pipeline's `div255`, exact with NEON only.
fn lowp_div255(v: u32) -> u32 {
    if cfg!(target_arch = "aarch64") {
        crate::scan::div255(v)
    } else {
        (v + 255) >> 8
    }
}

fn lowp_src_over(s: Pixel, d: Pixel, coverage: u8) -> Pixel {
    let s = if coverage == 255 {
        s
    } else {
        s.map(|v| lowp_div255(u32::from(v) * u32::from(coverage)) as u8)
    };
    let inverse = 255 - u32::from(s[3]);
    zip(s, d, |s, d| s + lowp_div255(d * inverse))
}

/// `legacy_shader_can_handle`.
fn legacy_shader_can_handle(inverse: &Matrix) -> bool {
    if !LEGACY_AFFINE && !inverse.is_scale_translate() {
        return false;
    }
    let max = 32767.0;
    let src = inverse.map_rect(&Rect {
        left: 0.0,
        top: 0.0,
        right: max,
        bottom: max,
    });
    let limit = i32::MAX as f32 * 0.25;
    src.left >= -limit && src.top >= -limit && src.right <= limit && src.bottom <= limit
}

enum Mode {
    Legacy(Legacy),
    Pipeline(Stages),
}

/// `SkBitmapProcState` with both axes clamped.
struct Legacy {
    inverse: Matrix,
    bilerp: bool,
    alpha_scale: u32,
    dx: i64,
    dy: i64,
}

const FIXED_ONE: i32 = 1 << 16;

/// `SkScalarToFixed3232`.
fn fixed3232(v: f32) -> i64 {
    (v * 4_294_967_296.0) as i64
}

fn to_fixed(v: i64) -> i32 {
    (v >> 16) as i32
}

fn to_int(v: i64) -> i32 {
    (v >> 32) as i32
}

/// `sk_fixed3232_saturate2fixed`.
fn saturate_to_fixed(v: i64) -> i32 {
    let v = if v >> 32 < i64::from(i16::MAX) {
        v
    } else {
        i64::from(i16::MAX) << 32
    };
    let v = if v >> 32 > i64::from(i16::MIN) {
        v
    } else {
        i64::from(i16::MIN) << 32
    };
    to_fixed(v)
}

/// The two pixels a fixed-point sample point falls between, and the weight of the second in
/// sixteenths: `pack` with `clamp`.
fn taps(f: i32, max: i32) -> (i32, i32, u32) {
    (
        (f >> 16).clamp(0, max),
        (f.wrapping_add(FIXED_ONE) >> 16).clamp(0, max),
        (f >> 12) as u32 & 0xf,
    )
}

fn just_trans_integral(m: &Matrix) -> bool {
    let integral = |v: f32| nearly(v, (v + 0.5).floor(), 1.0 / 256.0);
    m.is_translate() && integral(m.tx) && integral(m.ty)
}

impl Legacy {
    fn new(shader: &Shader) -> Legacy {
        let mut inverse = shader.inverse;
        let mut integral = just_trans_integral(&inverse);
        if !integral {
            if inverse.is_scale_translate()
                && !inverse.is_translate()
                && let Some(forward) = inverse.invert()
                && nearly(forward.sx, 1.0, 1.0 / 32768.0)
                && nearly(forward.sy, 1.0, 1.0 / 32768.0)
            {
                inverse = Matrix::scale_translate(1.0, 1.0, -forward.tx, -forward.ty);
            }
            integral = just_trans_integral(&inverse);
        }
        let [_, _, width, height] = shader.part;
        Legacy {
            inverse,
            bilerp: shader.linear && (width | height) & !0x3fff == 0 && !integral,
            alpha_scale: u32::from(alpha_u8(shader.alpha)) + 1,
            dx: fixed3232(inverse.sx),
            dy: fixed3232(inverse.ky),
        }
    }

    /// `SkBitmapProcStateAutoMapper`.
    fn map(&self, x: i32, y: i32) -> (i64, i64) {
        let p = self.inverse.map_point_affine(P {
            x: x as f32 + 0.5,
            y: y as f32 + 0.5,
        });
        let bias = i64::from(if self.bilerp { FIXED_ONE >> 1 } else { 1 }) << 16;
        (
            fixed3232(p.x).wrapping_sub(bias),
            fixed3232(p.y).wrapping_sub(bias),
        )
    }

    fn scaled(&self, p: Pixel) -> Pixel {
        p.map(|v| ((u32::from(v) * self.alpha_scale) >> 8) as u8)
    }

    /// `S32_alpha_D32_filter_DX` and `_DXDY`.
    fn filter(&self, bitmap: &Bitmap<'_>, x: (i32, i32, u32), y: (i32, i32, u32)) -> Pixel {
        let (p00, p01) = (bitmap.at(x.0, y.0), bitmap.at(x.1, y.0));
        let (p10, p11) = (bitmap.at(x.0, y.1), bitmap.at(x.1, y.1));
        let (wx, wy) = (x.2, y.2);
        let p = std::array::from_fn(|i| {
            let row = |a: Pixel, b: Pixel| u32::from(a[i]) * (16 - wx) + u32::from(b[i]) * wx;
            ((row(p00, p01) * (16 - wy) + row(p10, p11) * wy) >> 8) as u8
        });
        self.scaled(p)
    }

    /// `shadeSpan`, whose sample points restart from the matrix in every buffer it fills.
    fn span(&self, bitmap: &Bitmap<'_>, run: &Run, out: &mut Vec<Pixel>) {
        let scale_translate = self.inverse.is_scale_translate();
        if self.alpha_scale == 256 && !self.bilerp && scale_translate {
            let (mut fx, fy) = self.map(run.x, run.y);
            let y = to_int(fy);
            for _ in 0..run.len {
                out.push(bitmap.at(to_int(fx), y));
                fx = fx.wrapping_add(self.dx);
            }
            return;
        }
        let buffer = match (scale_translate, self.bilerp) {
            (true, true) => 127,
            (true, false) => 254,
            (false, true) => 64,
            (false, false) => 128,
        };
        let (mut x, mut left) = (run.x, run.len);
        while left > 0 {
            let n = left.min(buffer);
            match (scale_translate, self.bilerp) {
                (true, true) => self.filter_scale(bitmap, x, run.y, n, out),
                (true, false) => self.nofilter_scale(bitmap, x, run.y, n, out),
                (false, _) => self.affine(bitmap, x, run.y, n, out),
            }
            x += n as i32;
            left -= n;
        }
    }

    fn filter_scale(&self, bitmap: &Bitmap<'_>, x: i32, y: i32, n: u32, out: &mut Vec<Pixel>) {
        let (mut fx, fy) = self.map(x, y);
        let y = taps(saturate_to_fixed(fy), bitmap.max_y());
        for _ in 0..n {
            let x = taps(saturate_to_fixed(fx), bitmap.max_x());
            out.push(self.filter(bitmap, x, y));
            fx = fx.wrapping_add(self.dx);
        }
    }

    fn nofilter_scale(&self, bitmap: &Bitmap<'_>, x: i32, y: i32, n: u32, out: &mut Vec<Pixel>) {
        let (fx, fy) = self.map(x, y);
        let y = to_int(fy);
        if self.inverse.is_translate() {
            let x = to_int(fx);
            out.extend((0..n as i32).map(|i| self.scaled(bitmap.at(x + i, y))));
            return;
        }
        let max = bitmap.max_x();
        let (fixed, step) = (i64::from(to_fixed(fx)), i64::from(to_fixed(self.dx)));
        let last = fixed + step * i64::from(n - 1);
        let inside = |v: i64| (0..i64::from(max)).contains(&(v >> 16));
        let truncated = step > i64::from(FIXED_ONE / 256)
            && inside(fixed)
            && i32::try_from(last).is_ok()
            && inside(last);
        for i in 0..i64::from(n) {
            let x = if truncated {
                ((fixed + step * i) >> 16) as i32
            } else {
                to_int(fx.wrapping_add(self.dx.wrapping_mul(i)))
            };
            out.push(self.scaled(bitmap.at(x, y)));
        }
    }

    fn affine(&self, bitmap: &Bitmap<'_>, x: i32, y: i32, n: u32, out: &mut Vec<Pixel>) {
        let (mut fx, mut fy) = self.map(x, y);
        for _ in 0..n {
            let (x, y) = (to_fixed(fx), to_fixed(fy));
            out.push(if self.bilerp {
                let (x, y) = (taps(x, bitmap.max_x()), taps(y, bitmap.max_y()));
                self.filter(bitmap, x, y)
            } else {
                self.scaled(bitmap.at(x >> 16, y >> 16))
            });
            fx = fx.wrapping_add(self.dx);
            fy = fy.wrapping_add(self.dy);
        }
    }
}

/// The image stages of a raster pipeline in 8 bits.
pub struct Stages {
    inverse: Matrix,
    linear: bool,
    alpha: f32,
}

impl Stages {
    fn new(shader: &Shader) -> Stages {
        let m = shader.inverse;
        let integral = m.is_translate() && m.tx == m.tx.trunc() && m.ty == m.ty.trunc();
        Stages {
            inverse: m,
            linear: shader.linear && !integral,
            alpha: shader.alpha,
        }
    }

    fn span(&self, bitmap: &Bitmap<'_>, run: &Run, out: &mut Vec<Pixel>) {
        let m = &self.inverse;
        let c = (self.alpha * 255.0 + 0.5) as u32;
        for i in 0..run.len as i32 {
            let (px, py) = ((run.x + i) as f32 + 0.5, run.y as f32 + 0.5);
            let (x, y) = if m.is_translate() {
                (px + m.tx, py + m.ty)
            } else if m.is_scale_translate() {
                (px.mul_add(m.sx, m.tx), py.mul_add(m.sy, m.ty))
            } else {
                (
                    px.mul_add(m.sx, py.mul_add(m.kx, m.tx)),
                    px.mul_add(m.ky, py.mul_add(m.sy, m.ty)),
                )
            };
            let p = if self.linear {
                bilerp_clamp(bitmap, x, y)
            } else {
                gather(bitmap, x, y)
            };
            out.push(if self.alpha == 1.0 {
                p
            } else {
                p.map(|v| lowp_div255(u32::from(v) * c) as u8)
            });
        }
    }
}

/// `gather_8888` with `roundDownAtInteger`.
fn gather(bitmap: &Bitmap<'_>, x: f32, y: f32) -> Pixel {
    let index = |v: f32, size: u32| {
        let v = v.max(f32::MIN_POSITIVE).min((size as f32).next_down());
        v.next_down() as i32
    };
    bitmap.at(index(x, bitmap.part[2]), index(y, bitmap.part[3]))
}

/// `bilerp_clamp_8888`, the 8-bit stage.
fn bilerp_clamp(bitmap: &Bitmap<'_>, x: f32, y: f32) -> Pixel {
    let fixed = |v: f32| ((65536.0 * v + 0.5).floor() as i32).wrapping_sub(32768);
    let (qx, qy) = (fixed(x), fixed(y));
    let (sx, sy) = (qx >> 16, qy >> 16);
    let (tx, ty) = ((qx ^ 0x8000) as i16, (qy ^ 0x8000) as i16);
    let scaled = |a: i16, b: i16| {
        ((2 * i64::from(a) * i64::from(b) + (1 << 15)) >> 16)
            .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
    };
    let add = |a: i16, b: u16| b.wrapping_add(a as u16);
    let lerp_x = |left: u8, right: u8| -> u16 {
        let (left, right) = (u16::from(left), u16::from(right));
        let width = (right.wrapping_sub(left) as i16) << 7;
        let middle = (right + left) << 7;
        add(scaled(tx, width), middle).wrapping_add(1) >> 1
    };
    let lerp_y = |top: u16, bottom: u16| -> u8 {
        let width = (bottom as i16).wrapping_sub(top as i16);
        let middle = bottom.wrapping_add(top);
        (add(scaled(ty, width), middle).wrapping_add(0x80) >> 8) as u8
    };
    let (p00, p01) = (bitmap.at(sx, sy), bitmap.at(sx + 1, sy));
    let (p10, p11) = (bitmap.at(sx, sy + 1), bitmap.at(sx + 1, sy + 1));
    std::array::from_fn(|i| lerp_y(lerp_x(p00[i], p01[i]), lerp_x(p10[i], p11[i])))
}

/// `SkPixmap::scalePixels` with linear filtering of `level`, one of the mips of a `base` sized
/// image.
pub fn scale_pixels(level: &Pixmap, base: [u32; 2], size: [u32; 2]) -> Option<Pixmap> {
    let ratio = |a: u32, b: u32| a as f32 / b as f32;
    let fit = Matrix::scale_translate(ratio(size[0], base[0]), ratio(size[1], base[1]), 0.0, 0.0);
    let to_level = Matrix::scale_translate(
        ratio(level.width(), base[0]),
        ratio(level.height(), base[1]),
        0.0,
        0.0,
    );
    let stages = Stages::new(&Shader {
        inverse: to_level.concat(&fit.invert()?),
        part: [0, 0, level.width(), level.height()],
        linear: true,
        alpha: 1.0,
    });
    let bitmap = Bitmap::new(level, [0, 0, level.width(), level.height()]);
    let mut pixels = Vec::with_capacity((size[0] * size[1]) as usize);
    for y in 0..size[1] as i32 {
        let run = Run {
            x: 0,
            y,
            len: size[0],
            coverage: Some(255),
        };
        stages.span(&bitmap, &run, &mut pixels);
    }
    let mut out = Pixmap::new(size[0], size[1])?;
    out.data_mut().copy_from_slice(pixels.as_flattened());
    Some(out)
}
