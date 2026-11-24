// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2012 The Android Open Source Project). See THIRD_PARTY_NOTICES.md.

//! `SkDWriteGeometrySink`: DirectWrite glyph outlines as Skia paths.

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_BEZIER_SEGMENT, D2D1_FIGURE_BEGIN, D2D1_FIGURE_END, D2D1_FILL_MODE, D2D1_PATH_SEGMENT,
    ID2D1SimplifiedGeometrySink, ID2D1SimplifiedGeometrySink_Impl,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_GLYPH_OFFSET, IDWriteFontFace};
use windows::core::implement;
use windows_numerics::Vector2;

use crate::hairline::P;
use crate::skpath::SkPath;

#[derive(Default)]
struct Builder {
    path: SkPath,
    started: bool,
    current: P,
}

impl Builder {
    fn going_to(&mut self, p: P) {
        if !self.started {
            self.started = true;
            self.path.move_to(self.current);
        }
        self.current = p;
    }
}

#[implement(ID2D1SimplifiedGeometrySink)]
struct Sink(Rc<RefCell<Builder>>);

fn point(v: Vector2) -> P {
    P { x: v.X, y: v.Y }
}

fn same(a: P, b: P) -> bool {
    a.x == b.x && a.y == b.y
}

/// `SkFloatingPoint<float, 10>::AlmostEquals`.
fn approximately_equal(a: f32, b: f32) -> bool {
    let biased = |v: f32| {
        let bits = v.to_bits();
        if bits >> 31 != 0 {
            (!bits).wrapping_add(1)
        } else {
            bits | 0x8000_0000
        }
    };
    !a.is_nan() && !b.is_nan() && biased(a).abs_diff(biased(b)) <= 10
}

/// The control point of the quadratic a cubic was raised from, if it was.
fn quadratic(c: [P; 4]) -> Option<P> {
    let along = |p: [f32; 4]| {
        let mid = p[0] + (p[1] - p[0]) * 3.0 / 2.0;
        approximately_equal(mid, (p[2] - p[3]) * 3.0 / 2.0 + p[3]).then_some(mid)
    };
    Some(P {
        x: along(c.map(|p| p.x))?,
        y: along(c.map(|p| p.y))?,
    })
}

impl ID2D1SimplifiedGeometrySink_Impl for Sink_Impl {
    fn SetFillMode(&self, _: D2D1_FILL_MODE) {}

    fn SetSegmentFlags(&self, _: D2D1_PATH_SEGMENT) {}

    fn BeginFigure(&self, start: &Vector2, _: D2D1_FIGURE_BEGIN) {
        let mut b = self.0.borrow_mut();
        b.started = false;
        b.current = point(*start);
    }

    fn AddLines(&self, points: *const Vector2, count: u32) {
        let mut b = self.0.borrow_mut();
        // SAFETY: DirectWrite passes `count` points.
        for &p in unsafe { std::slice::from_raw_parts(points, count as usize) } {
            let p = point(p);
            if !same(b.current, p) {
                b.going_to(p);
                b.path.line_to(p);
            }
        }
    }

    fn AddBeziers(&self, beziers: *const D2D1_BEZIER_SEGMENT, count: u32) {
        let mut b = self.0.borrow_mut();
        // SAFETY: DirectWrite passes `count` segments.
        for s in unsafe { std::slice::from_raw_parts(beziers, count as usize) } {
            let [p1, p2, p3] = [s.point1, s.point2, s.point3].map(point);
            let from = b.current;
            if [p1, p2, p3].iter().all(|&p| same(from, p)) {
                continue;
            }
            b.going_to(p3);
            match quadratic([from, p1, p2, p3]) {
                Some(control) => b.path.quad_to(control, p3),
                None => b.path.cubic_to(p1, p2, p3),
            }
        }
    }

    fn EndFigure(&self, _: D2D1_FIGURE_END) {
        let mut b = self.0.borrow_mut();
        if b.started {
            b.path.close();
        }
    }

    fn Close(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The outline of glyphs at `size`, y down.
pub fn outline(
    face: &IDWriteFontFace,
    size: f32,
    glyphs: &[u16],
    advances: Option<&[f32]>,
    offsets: Option<&[DWRITE_GLYPH_OFFSET]>,
) -> Option<SkPath> {
    let builder = Rc::new(RefCell::new(Builder::default()));
    let sink: ID2D1SimplifiedGeometrySink = Sink(Rc::clone(&builder)).into();
    // SAFETY: the glyph, advance and offset arrays hold one entry per glyph.
    unsafe {
        face.GetGlyphRunOutline(
            size,
            glyphs.as_ptr(),
            advances.map(<[f32]>::as_ptr),
            offsets.map(<[DWRITE_GLYPH_OFFSET]>::as_ptr),
            glyphs.len() as u32,
            false,
            false,
            &sink,
        )
    }
    .ok()?;
    let path = std::mem::take(&mut builder.borrow_mut().path);
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raised_quadratics_come_back_as_quadratics() {
        let p = |x: f32, y: f32| P { x, y };
        let (a, control, b) = (p(1.0, 2.0), p(4.0, 8.0), p(7.0, 3.0));
        let toward = |from: P| {
            p(
                from.x + (control.x - from.x) * 2.0 / 3.0,
                from.y + (control.y - from.y) * 2.0 / 3.0,
            )
        };
        let found = quadratic([a, toward(a), toward(b), b]).expect("a quadratic");
        assert!((found.x - 4.0).abs() < 1e-5 && (found.y - 8.0).abs() < 1e-5);
        assert!(quadratic([a, p(2.0, 9.0), p(3.0, -4.0), b]).is_none());
    }
}
