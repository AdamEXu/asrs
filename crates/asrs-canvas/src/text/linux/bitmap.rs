// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2006-2023 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! Colour glyphs stored as PNG strikes (`CBDT`), drawn the way
//! `SkFontationsScalerContext::generatePngImage` draws them: a `drawImage` with linear filtering
//! and the nearest mip level onto a transparent raster canvas.

use skrifa::raw::TableProvider;
use skrifa::raw::tables::bitmap::{BitmapContent, BitmapDataFormat, BitmapMetrics};
use skrifa::{FontRef, GlyphId};
use tiny_skia::Pixmap;

use super::matrix::M2;
use crate::mip::downsample;

pub struct Strike<'a> {
    png: &'a [u8],
    bearing: (f32, f32),
    pub advance: f32,
    pub ppem: f32,
}

/// The strike `bitmap_glyph` picks for `size`: the nearest larger one, or the largest.
pub fn strike<'a>(font: &FontRef<'a>, glyph: u32, size: f32) -> Option<Strike<'a>> {
    let cblc = font.cblc().ok()?;
    let cbdt = font.cbdt().ok()?;
    let sizes = cblc.bitmap_sizes();
    let best = sizes.iter().reduce(|best, entry| {
        let (b, e) = (f32::from(best.ppem_y()), f32::from(entry.ppem_y()));
        if (e >= size && e < b) || (b < size && e > b) {
            entry
        } else {
            best
        }
    })?;
    let location = best
        .location(cblc.offset_data(), GlyphId::new(glyph))
        .ok()?;
    let data = cbdt.data(&location).ok()?;
    let (bearing, advance) = match data.metrics {
        BitmapMetrics::Small(m) => (
            (f32::from(m.bearing_x()), f32::from(m.bearing_y())),
            f32::from(m.advance()),
        ),
        BitmapMetrics::Big(m) => (
            (f32::from(m.hori_bearing_x()), f32::from(m.hori_bearing_y())),
            f32::from(m.hori_advance()),
        ),
    };
    let BitmapContent::Data(BitmapDataFormat::Png, png) = data.content else {
        return None;
    };
    Some(Strike {
        png,
        bearing,
        advance,
        ppem: f32::from(best.ppem_y()),
    })
}

impl Strike<'_> {
    /// The width and height in the PNG header.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let word = |at: usize| {
            let bytes = self.png.get(at..at + 4)?;
            Some(u32::from_be_bytes(bytes.try_into().ok()?))
        };
        Some((word(16)?, word(20)?))
    }

    /// The strike's pixels as Skia decodes them: premultiplied RGBA.
    pub fn decode(&self) -> Option<Pixmap> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(self.png));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().ok()?;
        let mut data = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut data).ok()?;
        let mut out = Pixmap::new(info.width, info.height)?;
        let channels = info.color_type.samples();
        let pixels = out.data_mut().as_chunks_mut::<4>().0;
        for (px, src) in pixels.iter_mut().zip(data.chunks_exact(channels)) {
            let (rgb, a) = match *src {
                [v] => ([v; 3], 255),
                [v, a] => ([v; 3], a),
                [r, g, b] => ([r, g, b], 255),
                [r, g, b, a] => ([r, g, b], a),
                _ => return None,
            };
            let premultiply = |v: u8| {
                let p = u32::from(v) * u32::from(a) + 128;
                ((p + (p >> 8)) >> 8) as u8
            };
            *px = [
                premultiply(rgb[0]),
                premultiply(rgb[1]),
                premultiply(rgb[2]),
                a,
            ];
        }
        Some(out)
    }

    /// The matrix from strike pixels to device pixels around the glyph origin, for text of
    /// `size` px under `remaining`, and its translation.
    pub fn placement(&self, size: f32, remaining: &M2) -> (M2, (f32, f32)) {
        let scale = size / self.ppem;
        let matrix = remaining.pre_scale(scale, scale);
        (matrix, matrix.map(self.bearing.0, -self.bearing.1))
    }
}

/// `matrix` applied to `[0, width] x [0, height]`, as `[left, top, right, bottom]`.
pub fn bounds(width: u32, height: u32, matrix: &M2, (tx, ty): (f32, f32)) -> [f32; 4] {
    let (w, h) = (width as f32, height as f32);
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| {
        let (x, y) = matrix.map(x, y);
        (x + tx, y + ty)
    });
    corners.iter().fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |b, &(x, y)| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
    )
}

/// `drawImage` of `image` onto a transparent `width` x `height` canvas under `matrix` and
/// `translate`: mip selection as `SkMipmapAccessor` makes it, then `SkBitmapProcState`'s bilinear
/// sampler, whose weights are the top four bits of a 16.16 coordinate.
pub fn draw(
    image: Pixmap,
    matrix: &M2,
    (tx, ty): (f32, f32),
    width: u32,
    height: u32,
) -> Option<Vec<u8>> {
    let (iw, ih) = (image.width(), image.height());
    let det =
        f64::from(matrix.sx) * f64::from(matrix.sy) - f64::from(matrix.kx) * f64::from(matrix.ky);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let inverse = M2 {
        sx: (f64::from(matrix.sy) / det) as f32,
        kx: (-f64::from(matrix.kx) / det) as f32,
        ky: (-f64::from(matrix.ky) / det) as f32,
        sy: (f64::from(matrix.sx) / det) as f32,
    };
    let offset = {
        let (x, y) = inverse.map(tx, ty);
        (-x, -y)
    };
    let scale = (inverse.sx.hypot(inverse.ky), inverse.kx.hypot(inverse.sy));
    let mean = (1.0 / scale.0 * (1.0 / scale.1)).sqrt();
    let level = (-mean.log2() - 0.5).max(0.0);
    let mut level = (level + 0.5).floor() as u32;
    let mut pixels = image;
    while level > 0 && (pixels.width() > 1 || pixels.height() > 1) {
        pixels = downsample(&pixels)?;
        level -= 1;
    }
    let to_level = (
        pixels.width() as f32 / iw as f32,
        pixels.height() as f32 / ih as f32,
    );
    let inverse = M2 {
        sx: inverse.sx * to_level.0,
        kx: inverse.kx * to_level.0,
        ky: inverse.ky * to_level.1,
        sy: inverse.sy * to_level.1,
    };
    let offset = (offset.0 * to_level.0, offset.1 * to_level.1);
    let fixed = |v: f32| (f64::from(v) * 4_294_967_296.0) as i64;
    let (dx, dy) = (fixed(inverse.sx), fixed(inverse.ky));
    let mut out = vec![0u8; (width * height * 4) as usize];
    let data = pixels.data();
    let (pw, ph) = (pixels.width() as i32, pixels.height() as i32);
    let tap = |x: i32, y: i32| {
        let i = (y * pw + x) as usize * 4;
        &data[i..i + 4]
    };
    for y in 0..height as i32 {
        let Some((x0, x1)) = covered(matrix, (tx, ty), (iw, ih), y, width as i32) else {
            continue;
        };
        let (cx, cy) = (x0 as f32 + 0.5, y as f32 + 0.5);
        let (sx, sy) = inverse.map(cx, cy);
        let half = 1i64 << 31;
        let mut fx = fixed(sx + offset.0) - half;
        let mut fy = fixed(sy + offset.1) - half;
        for x in x0..x1 {
            let pack = |f: i64, size: i32| {
                let f = (f >> 16) as i32;
                let low = (f >> 16).clamp(0, size - 1);
                let high = ((f + 65536) >> 16).clamp(0, size - 1);
                (low, high, (f >> 12 & 15) as u32)
            };
            let (u0, u1, wx) = pack(fx, pw);
            let (v0, v1, wy) = pack(fy, ph);
            let i = ((y as u32 * width) as usize + x as usize) * 4;
            for c in 0..4 {
                let row =
                    |v: i32| u32::from(tap(u0, v)[c]) * (16 - wx) + u32::from(tap(u1, v)[c]) * wx;
                out[i + c] = ((row(v0) * (16 - wy) + row(v1) * wy) >> 8) as u8;
            }
            fx += dx;
            fy += dy;
        }
    }
    Some(out)
}

/// The pixels of row `y` whose centres the drawn image covers, as Skia's non-antialiased fills
/// pick them.
fn covered(
    matrix: &M2,
    (tx, ty): (f32, f32),
    (iw, ih): (u32, u32),
    y: i32,
    width: i32,
) -> Option<(i32, i32)> {
    let (w, h) = (iw as f32, ih as f32);
    let round = |v: f32| (v + 0.5).floor() as i32;
    let corner = |x: f32, y: f32| {
        let (x, y) = matrix.map(x, y);
        (x + tx, y + ty)
    };
    let (left, right) = if matrix.is_scale() {
        let (l, t) = corner(0.0, 0.0);
        let (r, b) = corner(w, h);
        if y < round(t.min(b)) || y >= round(t.max(b)) {
            return None;
        }
        (round(l.min(r)), round(l.max(r)))
    } else {
        let quad = [
            corner(0.0, 0.0),
            corner(w, 0.0),
            corner(w, h),
            corner(0.0, h),
        ];
        let cy = y as f32 + 0.5;
        let mut crossings: Vec<f32> = (0..4)
            .filter_map(|i| {
                let ((ax, ay), (bx, by)) = (quad[i], quad[(i + 1) % 4]);
                ((ay <= cy) != (by <= cy)).then(|| ax + (cy - ay) * (bx - ax) / (by - ay))
            })
            .collect();
        crossings.sort_by(f32::total_cmp);
        (round(*crossings.first()?), round(*crossings.last()?))
    };
    let (left, right) = (left.max(0), right.min(width));
    (left < right).then_some((left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unscaled_image_is_copied() {
        let mut image = Pixmap::new(2, 2).expect("pixmap");
        image
            .data_mut()
            .copy_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 9]);
        let copy = draw(image.clone(), &M2::IDENTITY, (1.0, 0.0), 4, 2).expect("image");
        assert_eq!(&copy[4..12], &image.data()[..8]);
        assert_eq!(&copy[..4], &[0; 4]);
        assert_eq!(&copy[12..16], &[0; 4]);
    }
}
