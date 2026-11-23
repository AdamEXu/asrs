//! The frames ffmpeg wrote, decoded and resampled as Chromium paints an `<img>`.

use std::path::PathBuf;

use asrs_canvas::Pixmap;
use asrs_canvas::mip::{Mip, downsample};
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_ui::svg::Backend;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// The file `src` names as a URL relative to the page's `file:` URL: without its query and
/// fragment, and percent-decoded.
pub fn file_path(src: &JsString) -> PathBuf {
    let text = src.to_string_lossy();
    let path = text.split(['?', '#']).next().unwrap_or_default().as_bytes();
    let hex = |at: usize| path.get(at).and_then(|b| (*b as char).to_digit(16));
    let mut out = Vec::with_capacity(path.len());
    let mut i = 0;
    while i < path.len() {
        match (path[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

/// `None` where the image fails to load.
pub fn load(src: &JsString) -> Option<Pixmap> {
    decode(std::fs::read(file_path(src)).ok()?)
}

pub fn decode(bytes: Vec<u8>) -> Option<Pixmap> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgba = decoder.decode().ok()?;
    let (width, height) = decoder.dimensions()?;
    let size = asrs_canvas::IntSize::from_wh(width as u32, height as u32)?;
    Pixmap::from_vec(rgba, size)
}

/// `object-fit: contain` in `rect` (CSS px) at `scale`: the painted `[x, y, width, height]` in
/// device px, snapped as `ToPixelSnappedRect` snaps it.
pub fn contained(rect: [f64; 4], natural: [u32; 2], scale: f64) -> [i32; 4] {
    let lu = |v: f64| (v * 64.0).floor() / 64.0;
    let [x, y, w, h] = rect.map(|v| lu(v * scale));
    let [nw, nh] = natural.map(f64::from);
    let (width, height) = if w * nh > h * nw {
        (lu(h * nw / nh), h)
    } else {
        (w, lu(w * nh / nw))
    };
    let left = x + lu((w - width) / 2.0);
    let top = y + lu((h - height) / 2.0);
    let round = |v: f64| (v + 0.5).floor();
    [
        round(left) as i32,
        round(top) as i32,
        (round(left + width) - round(left)) as i32,
        (round(top + height) - round(top)) as i32,
    ]
}

/// One axis of a linear filter: for each target pixel the two source pixels it reads and the
/// second one's weight, clamped to the image's edge.
fn taps(source: u32, target: u32) -> Vec<(usize, usize, f32)> {
    let step = f64::from(source) / f64::from(target);
    let last = source as usize - 1;
    (0..target)
        .map(|i| {
            let at = (f64::from(i) + 0.5) * step - 0.5;
            let floor = at.floor();
            let first = floor.max(0.0) as usize;
            let second = (floor + 1.0).max(0.0) as usize;
            (first.min(last), second.min(last), (at - floor) as f32)
        })
        .collect()
}

/// A part of `image` averaged into each of `width` x `height` pixels.
fn area(image: &Pixmap, width: u32, height: u32) -> Option<Pixmap> {
    let spans = |source: u32, target: u32| -> Vec<(usize, Vec<f32>)> {
        let step = f64::from(source) / f64::from(target);
        (0..target)
            .map(|i| {
                let (start, end) = (f64::from(i) * step, f64::from(i + 1) * step);
                let first = start.floor() as u32;
                let last = (end.ceil() as u32).min(source);
                let weights = (first..last).map(|j| {
                    let covered = end.min(f64::from(j + 1)) - start.max(f64::from(j));
                    (covered / step) as f32
                });
                (first as usize, weights.collect())
            })
            .collect()
    };
    let columns = spans(image.width(), width);
    let rows = spans(image.height(), height);
    let stride = image.width() as usize * 4;
    let mut out = Pixmap::new(width, height)?;
    let line_bytes = width as usize * 4;
    let data = image.data();
    in_bands(out.data_mut(), line_bytes, &rows, |line, (y0, weights)| {
        let mut sums = vec![[0.0f32; 3]; width as usize];
        for (j, &wy) in weights.iter().enumerate() {
            let row = &data[(y0 + j) * stride..][..stride];
            for (sum, (x0, weights)) in sums.iter_mut().zip(&columns) {
                for (i, &wx) in weights.iter().enumerate() {
                    let pixel = &row[(x0 + i) * 4..][..3];
                    for (s, &v) in sum.iter_mut().zip(pixel) {
                        *s += f32::from(v) * wx * wy;
                    }
                }
            }
        }
        for (pixel, sum) in line.as_chunks_mut::<4>().0.iter_mut().zip(&sums) {
            let [r, g, b] = sum.map(|v| (v + 0.5) as u8);
            *pixel = [r, g, b, 255];
        }
    });
    Some(out)
}

/// Runs `paint` over every line of `out` with its row, on a few threads.
fn in_bands<R: Sync>(
    out: &mut [u8],
    line_bytes: usize,
    rows: &[R],
    paint: impl Fn(&mut [u8], &R) + Sync,
) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
    let band = rows.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (lines, rows) in out.chunks_mut(band * line_bytes).zip(rows.chunks(band)) {
            let paint = &paint;
            scope.spawn(move || {
                for (line, row) in lines.chunks_exact_mut(line_bytes).zip(rows) {
                    paint(line, row);
                }
            });
        }
    });
}

/// `source` through a linear filter, opaque BGRA.
fn linear(source: &Pixmap, width: u32, height: u32) -> Vec<u8> {
    let columns = taps(source.width(), width);
    let rows = taps(source.height(), height);
    let stride = source.width() as usize * 4;
    let data = source.data();
    let line_bytes = width as usize * 4;
    let mut out = vec![0; line_bytes * height as usize];
    in_bands(&mut out, line_bytes, &rows, |line, &(y0, y1, ty)| {
        let top = &data[y0 * stride..][..stride];
        let bottom = &data[y1 * stride..][..stride];
        for (pixel, &(x0, x1, tx)) in line.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
            let mix = |row: &[u8], k: usize| {
                let a = f32::from(row[x0 * 4 + k]);
                a + (f32::from(row[x1 * 4 + k]) - a) * tx
            };
            let channel = |k: usize| {
                let a = mix(top, k);
                (a + (mix(bottom, k) - a) * ty + 0.5) as u8
            };
            *pixel = [channel(2), channel(1), channel(0), 255];
        }
    });
    out
}

/// What Skia's GPU backends subtract from the level of detail of a mipmapped texture.
const SHARPEN: f64 = 0.475;

/// `image` drawn `width` x `height`, opaque BGRA. cc hands Skia the mip level it picks for the
/// size. Where Chromium rasterizes the page with Graphite that level is a texture with mips of
/// its own, sampled trilinearly with a sharpening bias; elsewhere CPU Skia samples it linearly.
pub fn resample(image: &Pixmap, width: u32, height: u32) -> Option<Vec<u8>> {
    let natural = [image.width(), image.height()];
    let scale = [
        f64::from(width) / f64::from(natural[0]),
        f64::from(height) / f64::from(natural[1]),
    ];
    let whole = [0.0, 0.0, f64::from(natural[0]), f64::from(natural[1])];
    let Some(mip) = Mip::of(whole, scale, natural) else {
        return Some(linear(image, width, height));
    };
    if Backend::HOST == Backend::Raster {
        return Some(linear(&mip.build(image)?, width, height));
    }
    let level = area(image, mip.size[0], mip.size[1])?;
    let mut out = linear(&level, width, height);
    let detail = math::max2(
        f64::from(mip.size[0]) / f64::from(width),
        f64::from(mip.size[1]) / f64::from(height),
    );
    let fraction = (math::log2(detail) - SHARPEN).clamp(0.0, 1.0) as f32;
    if fraction > 0.0 {
        let next = linear(&downsample(&level)?, width, height);
        for (a, b) in out.iter_mut().zip(next) {
            let mixed = f32::from(*a) + (f32::from(b) - f32::from(*a)) * fraction;
            *a = (mixed + 0.5) as u8;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_name_files() {
        for (src, path) in [
            ("/tmp/a/00000001.jpg", "/tmp/a/00000001.jpg"),
            ("/tmp/a%20b/1.jpg", "/tmp/a b/1.jpg"),
            ("/tmp/100%/1.jpg", "/tmp/100%/1.jpg"),
            ("/tmp/a#b/1.jpg", "/tmp/a"),
            ("/tmp/a?b/1.jpg", "/tmp/a"),
        ] {
            assert_eq!(file_path(&JsString::from(src)), PathBuf::from(path));
        }
    }

    #[test]
    fn contained_images_are_centred_and_snapped() {
        let rect = [300.0, 130.0, 800.0, 435.0];
        assert_eq!(contained(rect, [1920, 1080], 1.0), [313, 130, 774, 435]);
        assert_eq!(contained(rect, [1920, 1080], 2.0), [627, 260, 1546, 870]);
        assert_eq!(contained(rect, [1920, 200], 1.0), [300, 306, 800, 83]);
    }

    #[test]
    fn an_unscaled_image_keeps_its_pixels() {
        let mut image = Pixmap::new(3, 2).expect("pixmap");
        for (i, v) in image.data_mut().iter_mut().enumerate() {
            *v = if i % 4 == 3 { 255 } else { i as u8 * 9 };
        }
        let out = resample(&image, 3, 2).expect("image");
        for (o, i) in out
            .as_chunks::<4>()
            .0
            .iter()
            .zip(image.data().as_chunks::<4>().0)
        {
            assert_eq!(*o, [i[2], i[1], i[0], 255]);
        }
    }
}
