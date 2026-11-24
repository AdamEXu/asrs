// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2010 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! `SkRasterClip` and `SkAAClip` for the canvas `clip()`: a pixel-aligned rect, or a coverage
//! mask that multiplies every blit.

use std::sync::Arc;

use crate::aaa;
use crate::blit::{Kind, Spans};
use crate::path::Path;
use crate::skpath::{Rect, SkPath};

type IRect = [i32; 4];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RasterClip {
    Rect(IRect),
    Mask(Arc<Mask>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Mask {
    bounds: IRect,
    alpha: Vec<u8>,
}

const EMPTY: IRect = [0; 4];

fn intersect(a: IRect, b: IRect) -> Option<IRect> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

fn contains(outer: IRect, inner: IRect) -> bool {
    outer[0] <= inner[0] && outer[1] <= inner[1] && outer[2] >= inner[2] && outer[3] >= inner[3]
}

/// `SkMulDiv255Round`.
fn mul_div_255_round(a: u8, b: u8) -> u8 {
    let prod = u32::from(a) * u32::from(b) + 128;
    ((prod + (prod >> 8)) >> 8) as u8
}

/// Coordinates within an eighth of a pixel of an integer need no antialiased clip edge.
fn nearly_integral(x: f32) -> bool {
    let x = x + 0.125;
    x - x.floor() < 0.25
}

impl Mask {
    fn row(&self, y: i32) -> &[u8] {
        let width = (self.bounds[2] - self.bounds[0]) as usize;
        let start = (y - self.bounds[1]) as usize * width;
        &self.alpha[start..start + width]
    }

    fn at(&self, x: i32, y: i32) -> u8 {
        let b = self.bounds;
        if x < b[0] || x >= b[2] || y < b[1] || y >= b[3] {
            return 0;
        }
        self.row(y)[(x - b[0]) as usize]
    }
}

impl RasterClip {
    pub fn bounds(&self) -> IRect {
        match self {
            RasterClip::Rect(r) => *r,
            RasterClip::Mask(m) => m.bounds,
        }
    }

    pub fn is_empty(&self) -> bool {
        let b = self.bounds();
        b[0] >= b[2] || b[1] >= b[3]
    }

    /// Skia's scan converters take their run-length blitter under an antialiased clip.
    pub fn force_rle(&self) -> bool {
        matches!(self, RasterClip::Mask(_))
    }

    /// The coverage at a canvas pixel.
    pub fn at(&self, x: i32, y: i32) -> u8 {
        match self {
            RasterClip::Rect(r) => {
                if x >= r[0] && x < r[2] && y >= r[1] && y < r[3] {
                    255
                } else {
                    0
                }
            }
            RasterClip::Mask(m) => m.at(x, y),
        }
    }

    /// `SkRasterClip::op` with an axis-aligned device rect.
    pub fn intersect_rect(&self, device: Rect) -> RasterClip {
        if self.is_empty() {
            return RasterClip::Rect(EMPTY);
        }
        let edges = [device.left, device.top, device.right, device.bottom];
        if let RasterClip::Rect(r) = self
            && edges.iter().all(|&v| nearly_integral(v))
        {
            let rounded = edges.map(|v| (v + 0.5).floor() as i32);
            return RasterClip::Rect(intersect(*r, rounded).unwrap_or(EMPTY));
        }
        let bounds = self.bounds();
        let Some(pixels) = intersect(bounds, device.round_out()) else {
            return RasterClip::Rect(EMPTY);
        };
        let contained = device.left <= bounds[0] as f32
            && device.top <= bounds[1] as f32
            && device.right >= bounds[2] as f32
            && device.bottom >= bounds[3] as f32;
        if contained {
            return self.clone();
        }
        let mut path = Path::new();
        path.add_rect_ltrb(edges);
        self.multiply(&path.to_sk(None), pixels)
    }

    /// `SkRasterClip::op` with a device path.
    pub fn intersect_path(&self, device: &SkPath) -> RasterClip {
        if self.is_empty() {
            return RasterClip::Rect(EMPTY);
        }
        self.multiply(device, self.bounds())
    }

    fn multiply(&self, device: &SkPath, bounds: IRect) -> RasterClip {
        let Some(bounds) = device
            .is_finite()
            .then(|| intersect(bounds, device.bounds().round_out()))
            .flatten()
        else {
            return RasterClip::Rect(EMPTY);
        };
        let mut spans = Spans::new(bounds[2] as u32, bounds[3] as u32);
        aaa::fill_path_clipped(device, bounds, true, false, &mut spans);
        let width = (bounds[2] - bounds[0]) as usize;
        let mut alpha = vec![0; width * (bounds[3] - bounds[1]) as usize];
        for (x, y, run) in spans.iter() {
            let start = (y - bounds[1]) as usize * width + (x - bounds[0]) as usize;
            for (i, (cell, &a)) in alpha[start..].iter_mut().zip(run).enumerate() {
                *cell = mul_div_255_round(a, self.at(x + i as i32, y));
            }
        }
        trimmed(bounds, &alpha)
    }

    /// `clipHandlesSprite`.
    pub fn handles_sprite(&self, sprite: IRect) -> bool {
        match self {
            RasterClip::Rect(_) => true,
            RasterClip::Mask(mask) => {
                contains(mask.bounds, sprite)
                    && (sprite[1]..sprite[3]).all(|y| {
                        let row = mask.row(y);
                        row[(sprite[0] - mask.bounds[0]) as usize
                            ..(sprite[2] - mask.bounds[0]) as usize]
                            .iter()
                            .all(|&a| a == 255)
                    })
            }
        }
    }

    /// `spans` through `SkRectClipBlitter` or `SkAAClipBlitter`, neither of which keeps the
    /// two-pixel blits.
    pub fn apply(&self, spans: Spans) -> Spans {
        let Some(bounds) = spans.bounds() else {
            return spans;
        };
        if let RasterClip::Rect(r) = self
            && contains(*r, bounds)
        {
            return spans;
        }
        let mut out = spans.same_size();
        let clip = self.bounds();
        for (x, y, run, kind) in spans.iter_kinds() {
            if y < clip[1] || y >= clip[3] {
                continue;
            }
            let (from, to) = (x.max(clip[0]), (x + run.len() as i32).min(clip[2]));
            if from >= to {
                continue;
            }
            let run = &run[(from - x) as usize..(to - x) as usize];
            match self {
                RasterClip::Rect(_) if kind == Kind::Mask => out.mask(from, y, run),
                RasterClip::Rect(_) => out.anti_h(from, y, run),
                RasterClip::Mask(mask) => {
                    let row = &mask.row(y)[(from - clip[0]) as usize..];
                    let merged: Vec<u8> = run
                        .iter()
                        .zip(row)
                        .map(|(&a, &c)| mul_div_255_round(a, c))
                        .collect();
                    out.anti_h(from, y, &merged);
                }
            }
        }
        out
    }
}

/// `SkAAClip::trimBounds`, back to a rect when no partial coverage is left.
fn trimmed(bounds: IRect, alpha: &[u8]) -> RasterClip {
    let width = (bounds[2] - bounds[0]) as usize;
    let mut used: Option<IRect> = None;
    for (row, cells) in alpha.chunks_exact(width).enumerate() {
        let y = bounds[1] + row as i32;
        for (i, _) in cells.iter().enumerate().filter(|(_, a)| **a != 0) {
            let x = bounds[0] + i as i32;
            let r = used.get_or_insert([x, y, x + 1, y + 1]);
            *r = [r[0].min(x), r[1].min(y), r[2].max(x + 1), r[3].max(y + 1)];
        }
    }
    let Some(used) = used else {
        return RasterClip::Rect(EMPTY);
    };
    let mut kept = Vec::with_capacity(((used[2] - used[0]) * (used[3] - used[1])) as usize);
    for y in used[1]..used[3] {
        let start = (y - bounds[1]) as usize * width;
        let columns = (used[0] - bounds[0]) as usize..(used[2] - bounds[0]) as usize;
        kept.extend_from_slice(&alpha[start..start + width][columns]);
    }
    if kept.iter().all(|&a| a == 255) {
        return RasterClip::Rect(used);
    }
    RasterClip::Mask(Arc::new(Mask {
        bounds: used,
        alpha: kept,
    }))
}
