// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! Glyph masks Skia fills from outlines itself.

use super::{LcdMask, Mask};
use crate::aaa;
use crate::blit::Spans;
use crate::skpath::SkPath;

fn div255(product: u32) -> u8 {
    ((product + 128) * 257 >> 16) as u8
}

/// Blits source over: two spans of one row can share a pixel, as where a counter's edges meet.
fn coverage(path: &SkPath, width: u32, height: u32) -> Vec<u8> {
    let mut spans = Spans::new(width, height);
    aaa::fill_path(path, width, height, false, &mut spans);
    let mut data = vec![0u8; (width * height) as usize];
    for (x, y, alpha) in spans.iter() {
        let start = y as usize * width as usize + x as usize;
        for (out, &a) in data[start..start + alpha.len()].iter_mut().zip(alpha) {
            *out = a + div255(u32::from(255 - a) * u32::from(*out));
        }
    }
    data
}

/// A device path's mask bounds, `[left, top, width, height]`.
pub fn bounds(path: &SkPath) -> Option<[i32; 4]> {
    if path.is_empty() || !path.is_finite() {
        return None;
    }
    let [left, top, right, bottom] = path.bounds().round_out();
    (left < right && top < bottom).then_some([left, top, right - left, bottom - top])
}

/// `SkScalerContext::GenerateImageFromPath` for an A8 mask, before the preblend.
pub fn gray(path: &SkPath) -> Option<Mask> {
    let [left, top, width, height] = bounds(path)?;
    let (w, h) = (width as u32, height as u32);
    let local = path.transformed(&crate::path::Affine::translation(
        f64::from(-left),
        f64::from(-top),
    ));
    Some(Mask {
        left,
        top,
        width: w,
        height: h,
        data: coverage(&local, w, h),
        color: false,
    })
}

/// `SkScalerContext::GenerateImageFromPath` for a BW mask.
#[cfg(windows)]
pub fn aliased(path: &SkPath) -> Option<Mask> {
    use tiny_skia::{FillRule, Transform};
    let [left, top, width, height] = bounds(path)?;
    let (width, height) = (width as u32, height as u32);
    let shift = Transform::from_translate(-left as f32, -top as f32);
    let mut mask = tiny_skia::Mask::new(width, height)?;
    mask.fill_path(&path.to_tiny()?, FillRule::Winding, false, shift);
    Some(Mask {
        left,
        top,
        width,
        height,
        data: mask.data().to_vec(),
        color: false,
    })
}

const LCD_FILTER: [[u32; 12]; 3] = [
    [
        0x03, 0x0b, 0x1c, 0x33, 0x40, 0x39, 0x24, 0x10, 0x05, 0x01, 0x00, 0x00,
    ],
    [
        0x00, 0x02, 0x08, 0x16, 0x2b, 0x3d, 0x3d, 0x2b, 0x16, 0x08, 0x02, 0x00,
    ],
    [
        0x00, 0x00, 0x01, 0x05, 0x10, 0x24, 0x39, 0x40, 0x33, 0x1c, 0x0b, 0x03,
    ],
];

/// `GenerateImageFromPath` and `pack4xHToMask` for a horizontal RGB LCD mask, before the preblend
/// and the RGB565 packing.
pub fn lcd(path: &SkPath) -> Option<LcdMask> {
    let [left, top, inner, height] = bounds(path)?;
    let (left, width) = (left - 1, inner + 2);
    let samples = inner as u32 * 4;
    let scaled = path.transformed(&crate::path::Affine {
        a: 4.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: f64::from(-(left + 1) * 4),
        f: f64::from(-top),
    });
    let source = coverage(&scaled, samples, height as u32);
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for row in source.chunks_exact(samples as usize) {
        for first in (-4..samples as i32 + 4).step_by(4) {
            let mut fir = [0u32; 3];
            for (k, at) in (first - 4..first + 8).enumerate() {
                let Some(&value) = usize::try_from(at).ok().and_then(|at| row.get(at)) else {
                    continue;
                };
                for (sum, filter) in fir.iter_mut().zip(&LCD_FILTER) {
                    *sum += filter[k] * u32::from(value);
                }
            }
            data.extend(fir.map(|v| (v / 256).min(255) as u8));
        }
    }
    Some(LcdMask {
        left,
        top,
        width: width as u32,
        height: height as u32,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hairline::P;

    #[test]
    fn lcd_filter_spreads_one_pixel_to_each_side() {
        let mut path = SkPath::new();
        path.move_to(P { x: 2.0, y: 3.0 });
        for (x, y) in [(5.0, 3.0), (5.0, 5.0), (2.0, 5.0)] {
            path.line_to(P { x, y });
        }
        path.close();
        let gray = gray(&path).expect("gray");
        assert_eq!((gray.left, gray.top, gray.width, gray.height), (2, 3, 3, 2));
        assert!(gray.data.iter().all(|&v| v == 255));
        let lcd = lcd(&path).expect("lcd");
        assert_eq!((lcd.left, lcd.top, lcd.width, lcd.height), (1, 3, 5, 2));
        let middle = &lcd.data[6..9];
        assert_eq!(middle, [255; 3]);
        let sum = |f: &[u32]| f.iter().sum::<u32>() * 255 / 256;
        assert_eq!(u32::from(lcd.data[0]), sum(&LCD_FILTER[0][8..]));
        assert_eq!(u32::from(lcd.data[2]), sum(&LCD_FILTER[2][8..]));
    }
}
