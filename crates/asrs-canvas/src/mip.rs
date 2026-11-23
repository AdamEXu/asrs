// Ported from Skia and Chromium, m142 (BSD-3-Clause, Copyright 2013 Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! The mip level cc's software image decode cache draws a scaled-down image from, in place of
//! the image: the smallest level still at least as large as the image is drawn, at any filter
//! quality but nearest.

use crate::path::Affine;
use tiny_skia::{BlendMode, FilterQuality, Paint, Pattern, Pixmap, Rect, SpreadMode, Transform};

/// A part of an image at a mip level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mip {
    /// `[x, y, width, height]` of the part, in whole image pixels.
    pub src: [u32; 4],
    /// The part's size at the level.
    pub size: [u32; 2],
}

/// `ScaleAxisToMipLevel`.
fn level_size(base: u32, level: u32) -> u32 {
    let rounded = u64::from(base) + (1u64 << level) - 1;
    ((rounded >> level) as u32).max(1)
}

/// `MipMapUtil::GetLevelForSize`.
fn level_for(src: [u32; 2], target: [u32; 2]) -> u32 {
    let mut level = 0;
    loop {
        let next = src.map(|v| level_size(v, level + 1));
        if next[0] < target[0] || next[1] < target[1] {
            return level;
        }
        if src.map(|v| level_size(v, level)) == [1, 1] {
            return level;
        }
        level += 1;
    }
}

impl Mip {
    /// `CacheKey::FromDrawImage` for `src` (`[x, y, width, height]`, clipped to the image) drawn at
    /// `scale`: `None` where the image itself is drawn.
    pub fn of(src: [f64; 4], scale: [f64; 2], image: [u32; 2]) -> Option<Mip> {
        let x0 = src[0].floor().max(0.0);
        let y0 = src[1].floor().max(0.0);
        let x1 = (src[0] + src[2]).ceil().min(f64::from(image[0]));
        let y1 = (src[1] + src[3]).ceil().min(f64::from(image[1]));
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let part = [x1 - x0, y1 - y0];
        let target = [0, 1].map(|i| (part[i] * scale[i].abs() + 0.5).floor());
        if !(target[0] >= 1.0 && target[1] >= 1.0) {
            return None;
        }
        let part = part.map(|v| v as u32);
        let level = level_for(part, target.map(|v| v.min(f64::from(u32::MAX)) as u32));
        (level > 0).then(|| Mip {
            src: [x0 as u32, y0 as u32, part[0], part[1]],
            size: part.map(|v| level_size(v, level)),
        })
    }

    /// `image_to_device` for the level's pixels.
    pub fn placed(&self, image_to_device: &Affine) -> Affine {
        let [x, y, width, height] = self.src.map(f64::from);
        let [level_width, level_height] = self.size.map(f64::from);
        image_to_device
            .then(&Affine::translation(x, y))
            .then(&Affine::scaling(width / level_width, height / level_height))
    }

    /// The part's pixels at the level: `SkPixmap::scalePixels` with linear filtering and the
    /// nearest mip.
    pub fn build(&self, image: &Pixmap) -> Option<Pixmap> {
        let [x, y, width, height] = self.src;
        let mut pixels = if [x, y, width, height] == [0, 0, image.width(), image.height()] {
            image.clone()
        } else {
            let mut part = Pixmap::new(width, height)?;
            let row = width as usize * 4;
            for line in 0..height as usize {
                let from = ((y as usize + line) * image.width() as usize + x as usize) * 4;
                part.data_mut()[line * row..(line + 1) * row]
                    .copy_from_slice(&image.data()[from..from + row]);
            }
            part
        };
        let scale = |i: usize| f64::from(self.size[i]) / f64::from([width, height][i]);
        let level = -(scale(0) * scale(1)).sqrt().log2();
        for _ in 0..(level + 0.5).floor().max(0.0) as u32 {
            if pixels.width() == 1 && pixels.height() == 1 {
                break;
            }
            pixels = downsample(&pixels)?;
        }
        if [pixels.width(), pixels.height()] == self.size {
            return Some(pixels);
        }
        let mut out = Pixmap::new(self.size[0], self.size[1])?;
        let fit = Transform::from_scale(
            self.size[0] as f32 / pixels.width() as f32,
            self.size[1] as f32 / pixels.height() as f32,
        );
        let paint = Paint {
            shader: Pattern::new(
                pixels.as_ref(),
                SpreadMode::Pad,
                FilterQuality::Bilinear,
                1.0,
                fit,
            ),
            blend_mode: BlendMode::Source,
            anti_alias: false,
            ..Paint::default()
        };
        let rect = Rect::from_xywh(0.0, 0.0, self.size[0] as f32, self.size[1] as f32)?;
        out.fill_rect(rect, &paint, Transform::identity(), None);
        Some(out)
    }
}

/// One `SkMipmap` level: a box over two pixels of an even axis and a 1-2-1 filter over three of
/// an odd one, truncated.
pub fn downsample(src: &Pixmap) -> Option<Pixmap> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (w, h) = ((sw / 2).max(1), (sh / 2).max(1));
    let taps = |size: usize| -> &'static [u32] {
        match size {
            1 => &[1],
            s if s % 2 == 1 => &[1, 2, 1],
            _ => &[1, 1],
        }
    };
    let (columns, rows) = (taps(sw), taps(sh));
    let shift = (columns.iter().sum::<u32>() * rows.iter().sum::<u32>()).trailing_zeros();
    let mut out = Pixmap::new(w as u32, h as u32)?;
    let data = src.data();
    for (y, line) in out.data_mut().chunks_exact_mut(w * 4).enumerate() {
        for (x, pixel) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut sum = [0u32; 4];
            for (dy, &row) in rows.iter().enumerate() {
                for (dx, &column) in columns.iter().enumerate() {
                    let at = ((y * 2 + dy) * sw + x * 2 + dx) * 4;
                    for (s, &v) in sum.iter_mut().zip(&data[at..at + 4]) {
                        *s += u32::from(v) * row * column;
                    }
                }
            }
            *pixel = sum.map(|s| (s >> shift) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_is_the_smallest_not_below_the_drawn_size() {
        let mip = |w: f64, h: f64, scale: f64| Mip::of([0.0, 0.0, w, h], [scale; 2], [760, 460]);
        assert_eq!(mip(760.0, 460.0, 0.51), None);
        let half = Some(Mip {
            src: [0, 0, 760, 460],
            size: [380, 230],
        });
        assert_eq!(mip(760.0, 460.0, 0.5), half);
        assert_eq!(mip(760.0, 460.0, 0.26), half);
        assert_eq!(mip(760.0, 460.0, 0.25).map(|m| m.size), Some([190, 115]));
        assert_eq!(level_size(115, 1), 58);
    }

    #[test]
    fn levels_truncate_their_box() {
        let mut src = Pixmap::new(3, 2).expect("pixmap");
        for (p, v) in src
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip([1u8, 2, 3, 4, 5, 7])
        {
            *p = [v, v, v, 255];
        }
        let level = downsample(&src).expect("level");
        let sum = 1 + 2 * 2 + 3 + 4 + 2 * 5 + 7;
        assert_eq!(level.data(), [sum >> 3, sum >> 3, sum >> 3, 255]);
    }
}
