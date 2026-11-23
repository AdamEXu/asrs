//! The frames ffmpeg wrote, decoded and resampled as Chromium paints an `<img>`.

use std::path::PathBuf;

use asrs_canvas::Pixmap;
use asrs_canvas::mip::Mip;
use asrs_js::string::JsString;
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

/// `image` drawn `width` x `height`, opaque BGRA: the mip level cc picks for the size, then a
/// linear filter.
pub fn resample(image: &Pixmap, width: u32, height: u32) -> Option<Vec<u8>> {
    let natural = [image.width(), image.height()];
    let scale = [
        f64::from(width) / f64::from(natural[0]),
        f64::from(height) / f64::from(natural[1]),
    ];
    let whole = [0.0, 0.0, f64::from(natural[0]), f64::from(natural[1])];
    let level = Mip::of(whole, scale, natural).and_then(|mip| mip.build(image));
    let source = level.as_ref().unwrap_or(image);
    let columns = taps(source.width(), width);
    let rows = taps(source.height(), height);
    let stride = source.width() as usize * 4;
    let data = source.data();
    let mut out = vec![0; width as usize * height as usize * 4];
    let line_bytes = width as usize * 4;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
    let band = rows.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (lines, rows) in out.chunks_mut(band * line_bytes).zip(rows.chunks(band)) {
            let columns = &columns;
            scope.spawn(move || {
                for (line, &(y0, y1, ty)) in lines.chunks_exact_mut(line_bytes).zip(rows) {
                    let top = &data[y0 * stride..][..stride];
                    let bottom = &data[y1 * stride..][..stride];
                    for (pixel, &(x0, x1, tx)) in
                        line.as_chunks_mut::<4>().0.iter_mut().zip(columns)
                    {
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
                }
            });
        }
    });
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
