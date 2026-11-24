// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006 and 2020 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! The f32 matrices Skia draws with: `SkMatrix` without perspective, and the steps `SkCanvas`
//! takes to build one out of a canvas context's transform calls (`SkM44`).

use crate::hairline::P;
use crate::path::{Affine, clamp32};
use crate::skpath::Rect;

const SIN_COS_NEARLY_ZERO: f32 = 1.0 / 65536.0;
const NEARLY_ZERO: f32 = 1.0 / 4096.0;

/// `x' = sx x + kx y + tx`, `y' = ky x + sy y + ty`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub sx: f32,
    pub kx: f32,
    pub tx: f32,
    pub ky: f32,
    pub sy: f32,
    pub ty: f32,
}

impl From<&Affine> for Matrix {
    /// `AffineTransform::ToSkM44`.
    fn from(m: &Affine) -> Matrix {
        Matrix {
            sx: clamp32(m.a),
            kx: clamp32(m.c),
            tx: clamp32(m.e),
            ky: clamp32(m.b),
            sy: clamp32(m.d),
            ty: clamp32(m.f),
        }
    }
}

/// The double-precision sum `SkMatrix` concatenates with.
fn muladdmul(a: f32, b: f32, c: f32, d: f32) -> f32 {
    (f64::from(a) * f64::from(b) + f64::from(c) * f64::from(d)) as f32
}

/// `SkScalarSinSnapToZero` and `SkScalarCosSnapToZero`.
fn snapped(v: f32) -> f32 {
    if v.abs() <= SIN_COS_NEARLY_ZERO {
        0.0
    } else {
        v
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix::scale_translate(1.0, 1.0, 0.0, 0.0);

    pub const fn scale_translate(sx: f32, sy: f32, tx: f32, ty: f32) -> Matrix {
        Matrix {
            sx,
            kx: 0.0,
            tx,
            ky: 0.0,
            sy,
            ty,
        }
    }

    /// `SkMatrix::setRotate`.
    pub fn rotation(degrees: f32) -> Matrix {
        let radians = degrees * (std::f32::consts::PI / 180.0);
        let (sin, cos) = (snapped(radians.sin()), snapped(radians.cos()));
        Matrix {
            sx: cos,
            kx: -sin,
            tx: 0.0,
            ky: sin,
            sy: cos,
            ty: 0.0,
        }
    }

    /// `SkMatrix::RectToRectOrIdentity`.
    pub fn rect_to_rect(src: &Rect, dst: &Rect) -> Matrix {
        if src.is_empty() {
            return Matrix::IDENTITY;
        }
        let sx = (dst.right - dst.left) / (src.right - src.left);
        let sy = (dst.bottom - dst.top) / (src.bottom - src.top);
        Matrix::scale_translate(sx, sy, dst.left - src.left * sx, dst.top - src.top * sy)
    }

    pub fn is_identity(&self) -> bool {
        self.is_translate() && self.tx == 0.0 && self.ty == 0.0
    }

    pub fn is_translate(&self) -> bool {
        self.is_scale_translate() && self.sx == 1.0 && self.sy == 1.0
    }

    pub fn is_scale_translate(&self) -> bool {
        self.kx == 0.0 && self.ky == 0.0
    }

    pub fn is_finite(&self) -> bool {
        [self.sx, self.kx, self.tx, self.ky, self.sy, self.ty]
            .iter()
            .all(|v| v.is_finite())
    }

    /// `SkMatrix::rectStaysRect`: a scale, or a quarter turn with scales.
    pub fn rect_stays_rect(&self) -> bool {
        if self.is_scale_translate() {
            self.sx != 0.0 && self.sy != 0.0
        } else {
            self.sx == 0.0 && self.sy == 0.0 && self.kx != 0.0 && self.ky != 0.0
        }
    }

    /// `SkMatrix::setConcat`: `other` applies first.
    pub fn concat(&self, other: &Matrix) -> Matrix {
        let (a, b) = (self, other);
        if a.is_identity() {
            return *b;
        }
        if b.is_identity() {
            return *a;
        }
        if a.is_scale_translate() && b.is_scale_translate() {
            return Matrix::scale_translate(
                a.sx * b.sx,
                a.sy * b.sy,
                a.sx * b.tx + a.tx,
                a.sy * b.ty + a.ty,
            );
        }
        Matrix {
            sx: muladdmul(a.sx, b.sx, a.kx, b.ky),
            kx: muladdmul(a.sx, b.kx, a.kx, b.sy),
            tx: muladdmul(a.sx, b.tx, a.kx, b.ty) + a.tx,
            ky: muladdmul(a.ky, b.sx, a.sy, b.ky),
            sy: muladdmul(a.ky, b.kx, a.sy, b.sy),
            ty: muladdmul(a.ky, b.tx, a.sy, b.ty) + a.ty,
        }
    }

    pub fn post_scale(&self, sx: f32, sy: f32) -> Matrix {
        Matrix::scale_translate(sx, sy, 0.0, 0.0).concat(self)
    }

    pub fn post_translate(&self, tx: f32, ty: f32) -> Matrix {
        Matrix {
            tx: self.tx + tx,
            ty: self.ty + ty,
            ..*self
        }
    }

    pub fn pre_translate(&self, tx: f32, ty: f32) -> Matrix {
        self.concat(&Matrix::scale_translate(1.0, 1.0, tx, ty))
    }

    /// `SkMatrix::invert`.
    pub fn invert(&self) -> Option<Matrix> {
        if self.is_scale_translate() {
            let inverse = if self.sx == 1.0 && self.sy == 1.0 {
                Matrix::scale_translate(1.0, 1.0, -self.tx, -self.ty)
            } else {
                let (sx, sy) = (1.0 / self.sx, 1.0 / self.sy);
                Matrix::scale_translate(sx, sy, -self.tx * sx, -self.ty * sy)
            };
            return inverse.is_finite().then_some(inverse);
        }
        let [sx, kx, tx, ky, sy, ty] =
            [self.sx, self.kx, self.tx, self.ky, self.sy, self.ty].map(f64::from);
        let det = sx * sy - kx * ky;
        if (det as f32).abs() <= NEARLY_ZERO * NEARLY_ZERO * NEARLY_ZERO {
            return None;
        }
        let scale = 1.0 / det;
        let inverse = Matrix {
            sx: (sy * scale) as f32,
            kx: (-kx * scale) as f32,
            tx: ((kx * ty - sy * tx) * scale) as f32,
            ky: (-ky * scale) as f32,
            sy: (sx * scale) as f32,
            ty: ((ky * tx - sx * ty) * scale) as f32,
        };
        inverse.is_finite().then_some(inverse)
    }

    /// `SkMatrix::mapPoints`: the sums of `Scale_pts` and `Affine_vpts`.
    pub fn map_point(&self, p: P) -> P {
        if self.is_scale_translate() {
            P {
                x: p.x * self.sx + self.tx,
                y: p.y * self.sy + self.ty,
            }
        } else {
            self.map_point_affine(p)
        }
    }

    /// `SkMatrix::mapPoint`.
    pub fn map_point_affine(&self, p: P) -> P {
        P {
            x: p.x * self.sx + p.y * self.kx + self.tx,
            y: p.x * self.ky + p.y * self.sy + self.ty,
        }
    }

    /// `SkMatrix::mapRect`: the sorted bounds of the mapped corners.
    pub fn map_rect(&self, r: &Rect) -> Rect {
        let corners = [
            (r.left, r.top),
            (r.right, r.top),
            (r.right, r.bottom),
            (r.left, r.bottom),
        ]
        .map(|(x, y)| self.map_point(P { x, y }));
        let fold = |f: fn(f32, f32) -> f32, of: fn(&P) -> f32| {
            corners[1..]
                .iter()
                .map(of)
                .fold(of(&corners[0]), |a, b| f(a, b))
        };
        Rect {
            left: fold(f32::min, |p| p.x),
            top: fold(f32::min, |p| p.y),
            right: fold(f32::max, |p| p.x),
            bottom: fold(f32::max, |p| p.y),
        }
    }

    /// `SkM44::preTranslate`.
    pub fn m44_translate(&mut self, x: f32, y: f32) {
        self.tx = self.sx * x + (self.kx * y + self.tx);
        self.ty = self.ky * x + (self.sy * y + self.ty);
    }

    /// `SkM44::preScale`.
    pub fn m44_scale(&mut self, x: f32, y: f32) {
        self.sx *= x;
        self.ky *= x;
        self.kx *= y;
        self.sy *= y;
    }

    /// `SkM44::preConcat`. The products with zero are Skia's: an overflowed translation makes the
    /// whole matrix NaN.
    pub fn m44_concat(&mut self, m: &Matrix) {
        let a = *self;
        self.sx = a.sx * m.sx + (a.kx * m.ky + a.tx * 0.0);
        self.ky = a.ky * m.sx + (a.sy * m.ky + a.ty * 0.0);
        self.kx = a.sx * m.kx + (a.kx * m.sy + a.tx * 0.0);
        self.sy = a.ky * m.kx + (a.sy * m.sy + a.ty * 0.0);
        self.tx = a.sx * m.tx + (a.kx * m.ty + a.tx);
        self.ty = a.ky * m.tx + (a.sy * m.ty + a.ty);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_are_exact() {
        for (degrees, sin, cos) in [
            (90.0, 1.0, 0.0),
            (-90.0, -1.0, 0.0),
            (180.0, 0.0, -1.0),
            (450.0, 1.0, 0.0),
        ] {
            let m = Matrix::rotation(degrees);
            assert_eq!((m.ky, m.sx, m.kx, m.sy), (sin, cos, -sin, cos), "{degrees}");
            assert!(m.rect_stays_rect());
        }
        assert!(!Matrix::rotation(89.99).rect_stays_rect());
    }

    #[test]
    fn translations_overflow_like_floats() {
        let mut m = Matrix::IDENTITY;
        m.m44_scale(2.0, 2.0);
        m.m44_translate(2e38, 0.0);
        assert_eq!(m.tx, f32::INFINITY);
        m.m44_translate(-2e38, 0.0);
        assert!(m.tx.is_nan());
    }

    #[test]
    fn inverses_undo() {
        let m = Matrix {
            sx: 1.5,
            kx: -0.25,
            tx: 30.0,
            ky: 0.5,
            sy: 2.0,
            ty: -7.0,
        };
        let p = P { x: 3.0, y: 4.0 };
        let back = m.invert().unwrap().map_point(m.map_point(p));
        assert!((back.x - p.x).abs() < 1e-5 && (back.y - p.y).abs() < 1e-5);
        let flat = Matrix {
            kx: 3.0,
            ky: 1.0,
            ..m
        };
        assert_eq!(flat.invert(), None);
    }
}
