// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006 Google Inc.). See THIRD_PARTY_NOTICES.md.

/// The scale and skew of an `SkMatrix`: `x' = sx x + kx y`, `y' = ky x + sy y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M2 {
    pub sx: f32,
    pub kx: f32,
    pub ky: f32,
    pub sy: f32,
}

fn dot(a: f32, b: f32, c: f32, d: f32) -> f32 {
    (f64::from(a) * f64::from(b) + f64::from(c) * f64::from(d)) as f32
}

impl M2 {
    pub const IDENTITY: M2 = M2 {
        sx: 1.0,
        kx: 0.0,
        ky: 0.0,
        sy: 1.0,
    };

    pub fn is_scale(&self) -> bool {
        self.kx == 0.0 && self.ky == 0.0
    }

    /// `SkMatrix::setConcat(self, o)`.
    pub fn concat(&self, o: &M2) -> M2 {
        if self.is_scale() && o.is_scale() {
            return M2 {
                sx: self.sx * o.sx,
                sy: self.sy * o.sy,
                ..M2::IDENTITY
            };
        }
        M2 {
            sx: dot(self.sx, o.sx, self.kx, o.ky),
            kx: dot(self.sx, o.kx, self.kx, o.sy),
            ky: dot(self.ky, o.sx, self.sy, o.ky),
            sy: dot(self.ky, o.kx, self.sy, o.sy),
        }
    }

    pub fn pre_scale(&self, x: f32, y: f32) -> M2 {
        M2 {
            sx: self.sx * x,
            kx: self.kx * y,
            ky: self.ky * x,
            sy: self.sy * y,
        }
    }

    pub fn map(&self, x: f32, y: f32) -> (f32, f32) {
        if self.is_scale() {
            (x * self.sx, y * self.sy)
        } else {
            (x * self.sx + y * self.kx, x * self.ky + y * self.sy)
        }
    }
}

/// `SkComputeGivensRotation`: the rotation that maps `(x, y)` to the positive x axis.
fn givens(x: f32, y: f32) -> M2 {
    let (c, s) = if y == 0.0 {
        (1.0f32.copysign(x), 0.0)
    } else if x == 0.0 {
        (0.0, -1.0f32.copysign(y))
    } else if y.abs() > x.abs() {
        let t = x / y;
        let u = (1.0 + t * t).sqrt().copysign(y);
        let s = -1.0 / u;
        (-s * t, s)
    } else {
        let t = y / x;
        let u = (1.0 + t * t).sqrt().copysign(x);
        let c = 1.0 / u;
        (c, -c * t)
    };
    M2 {
        sx: c,
        kx: -s,
        ky: s,
        sy: c,
    }
}

/// A strike's total matrix split as `SkScalerContextRec::computeMatrices` splits it with
/// `PreMatrixScale::kVertical`: the text size the font is scaled to and what remains.
pub struct Split {
    pub size: f32,
    pub remaining: M2,
}

/// `size` px text under the device matrix `post`, with the fake-italic skew applied if `skew`.
pub fn split(size: f32, skew: bool, post: &M2) -> Option<Split> {
    const NEARLY_ZERO: f32 = 1.0 / 4096.0;
    let local = M2 {
        sx: size,
        kx: if skew { -0.25 * size } else { 0.0 },
        ky: 0.0,
        sy: size,
    };
    let total = post.concat(&local);
    let skewed_or_flipped =
        total.kx != 0.0 || total.ky != 0.0 || total.sx < 0.0 || total.sy < 0.0;
    let upright = if skewed_or_flipped {
        let (x, y) = total.map(1.0, 0.0);
        givens(x, y).concat(&total)
    } else {
        total
    };
    let finite = [upright.sx, upright.kx, upright.ky, upright.sy]
        .iter()
        .all(|v| v.is_finite());
    if !finite || upright.sx.abs() <= NEARLY_ZERO || upright.sy.abs() <= NEARLY_ZERO {
        return None;
    }
    let size = upright.sy.abs();
    let remaining = if skewed_or_flipped {
        total.pre_scale(1.0 / size, 1.0 / size)
    } else if total.sx == total.sy {
        M2::IDENTITY
    } else {
        M2 {
            sx: total.sx / size,
            ..M2::IDENTITY
        }
    };
    Some(Split { size, remaining })
}

/// `isAxisAligned` of `SkTypeface_fontations`: whether hinting survives.
pub fn is_axis_aligned(skew: bool, post: &M2) -> bool {
    !skew && (post.kx == 0.0 && post.ky == 0.0 || post.sx == 0.0 && post.sy == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upright_text_keeps_its_vertical_size() {
        let post = M2 {
            sx: 1.5,
            sy: 2.0,
            ..M2::IDENTITY
        };
        let s = split(12.0, false, &post).expect("regular");
        assert_eq!(s.size, 24.0);
        assert_eq!(s.remaining.sx, 0.75);
        assert_eq!(s.remaining.sy, 1.0);
        assert!(split(12.0, false, &M2 { sx: 0.0, ..post }).is_none());
    }

    #[test]
    fn rotation_is_left_to_the_remaining_matrix() {
        let (s, c) = (0.6f32, 0.8f32);
        let post = M2 {
            sx: c,
            kx: -s,
            ky: s,
            sy: c,
        };
        let split = split(10.0, false, &post).expect("regular");
        assert!((split.size - 10.0).abs() < 1e-5);
        assert!((split.remaining.ky - s).abs() < 1e-6);
        assert!(!is_axis_aligned(false, &post));
    }
}
