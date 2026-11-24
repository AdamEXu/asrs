// Ported from Skia and Chromium, m142 (BSD-3-Clause, Copyright 2013 Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! The mip level cc's software image decode cache draws a scaled-down image from, in place of
//! the image: the smallest level still at least as large as the image is drawn, at any filter
//! quality but nearest.

use tiny_skia::Pixmap;

use crate::image::scale_pixels;
use crate::skpath::Rect as SkRect;

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

/// What cc's decode cache gives a draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Level {
    /// The image would be less than half a pixel wide or high, so it is not decoded at all.
    Nothing,
    Image,
    Mip(Mip),
}

impl Mip {
    /// `CacheKey::FromDrawImage` for `src` (clipped to the image) drawn at `scale`.
    pub fn of(src: &SkRect, scale: [f32; 2], image: [u32; 2]) -> Level {
        let [l, t, r, b] = src.round_out();
        let (l, t) = (l.max(0) as u32, t.max(0) as u32);
        let (r, b) = (
            r.clamp(0, image[0] as i32) as u32,
            b.clamp(0, image[1] as i32) as u32,
        );
        if r <= l || b <= t {
            return Level::Nothing;
        }
        let part = [r - l, b - t];
        let target = [0, 1].map(|i| ((part[i] as f32 * scale[i]).abs() + 0.5).floor() as u32);
        if target.contains(&0) {
            return Level::Nothing;
        }
        match level_for(part, target) {
            0 => Level::Image,
            level => Level::Mip(Mip {
                src: [l, t, part[0], part[1]],
                size: part.map(|v| level_size(v, level)),
            }),
        }
    }

    /// The part of the level that `src` is of the image (`AdjustSrcRectForScale`).
    pub fn placed(&self, src: &SkRect) -> SkRect {
        let scale = [0, 1].map(|i| self.size[i] as f32 / self.src[i + 2] as f32);
        let (dx, dy) = (-(self.src[0] as f32), -(self.src[1] as f32));
        let (l, t) = (src.left + dx, src.top + dy);
        let (r, b) = (src.right + dx, src.bottom + dy);
        let (x, y) = (l * scale[0], t * scale[1]);
        SkRect {
            left: x,
            top: y,
            right: x + (r - l) * scale[0],
            bottom: y + (b - t) * scale[1],
        }
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
        for _ in 0..mip_level(self.size, [width, height]) {
            if pixels.width() == 1 && pixels.height() == 1 {
                break;
            }
            pixels = downsample(&pixels)?;
        }
        scale_pixels(&pixels, [width, height], self.size)
    }
}

/// `SkMipmap::ComputeLevel` with Chromium's anisotropic scale, rounded to the nearest level.
fn mip_level(size: [u32; 2], base: [u32; 2]) -> u32 {
    let scale = |i: usize| 1.0 / (1.0 / (size[i] as f32 / base[i] as f32));
    let scale = (scale(0) * scale(1)).sqrt();
    if scale >= 1.0 {
        return 0;
    }
    let level = (-scale.log2() - 0.5).max(0.0);
    (level + 0.5).floor() as u32
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
        let mip = |scale: f32| {
            let src = SkRect {
                left: 0.0,
                top: 0.0,
                right: 760.0,
                bottom: 460.0,
            };
            Mip::of(&src, [scale; 2], [760, 460])
        };
        let half = Level::Mip(Mip {
            src: [0, 0, 760, 460],
            size: [380, 230],
        });
        let quarter = Level::Mip(Mip {
            src: [0, 0, 760, 460],
            size: [190, 115],
        });
        assert_eq!(mip(0.51), Level::Image);
        assert_eq!(mip(0.5), half);
        assert_eq!(mip(0.26), half);
        assert_eq!(mip(0.25), quarter);
        assert_eq!(mip(0.001), Level::Nothing);
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
