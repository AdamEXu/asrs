//! A horizontal `<progress>` with a value, as `NativeThemeBase::PaintProgressBar` draws it in the
//! light colour scheme.

use asrs_canvas::Pixmap;
use gpui_kit::{Bounds, IntoElement, Pixels, Styled, canvas};

use crate::input::skia;
use crate::svg::Backend;

const TRACK: [f64; 3] = [239.0, 239.0, 239.0];
const ACCENT: [f64; 3] = [0.0, 117.0, 255.0];
const BORDER: [f64; 3] = [118.0, 118.0, 118.0];
const BORDER_ALPHA: f64 = 128.0 / 255.0;
const RADIUS: f64 = 4.0;
/// Device px, of a value that is not empty.
const MINIMUM_VALUE: f64 = 2.0;

/// `[left, top, right, bottom]`.
fn rounded_coverage([l, t, r, b]: [f64; 4], radius: f64, x: f64, y: f64) -> f64 {
    let half = [(r - l) / 2.0, (b - t) / 2.0];
    let from_centre = [(x - (l + r) / 2.0).abs(), (y - (t + b) / 2.0).abs()];
    let dx = from_centre[0] - half[0] + radius;
    let dy = from_centre[1] - half[1] + radius;
    let outside = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
    (0.5 - outside).clamp(0.0, 1.0)
}

fn over(dst: &mut [f64; 4], color: [f64; 3], alpha: f64) {
    for (d, c) in dst.iter_mut().zip(color) {
        *d = *d * (1.0 - alpha) + c * alpha;
    }
    dst[3] = dst[3] * (1.0 - alpha) + 255.0 * alpha;
}

fn raster(width: u32, height: u32, value: f64, scale: f64) -> Option<Pixmap> {
    let (w, h) = (f64::from(width), f64::from(height));
    let track = [0.0, h / 4.0, w, h * 3.0 / 4.0];
    let value_width = match (w * value).trunc() {
        v if v > 0.0 => v.max(MINIMUM_VALUE),
        _ => 0.0,
    };
    let inner = [
        track[0] + scale,
        track[1] + scale,
        track[2] - scale,
        track[3] - scale,
    ];
    let radius = RADIUS * scale;
    let mut pixmap = Pixmap::new(width, height)?;
    let (out, _) = pixmap.data_mut().as_chunks_mut::<4>();
    for (i, out) in out.iter_mut().enumerate() {
        let x = f64::from(i as u32 % width) + 0.5;
        let y = f64::from(i as u32 / width) + 0.5;
        let clip = rounded_coverage(track, radius, x, y);
        if clip == 0.0 {
            continue;
        }
        let mut pixel = [0.0; 4];
        over(&mut pixel, TRACK, clip);
        if x < value_width {
            let alpha = match Backend::HOST {
                Backend::Graphite => clip,
                Backend::Raster => clip * clip,
            };
            over(&mut pixel, ACCENT, alpha);
        }
        let ring = clip - rounded_coverage(inner, radius - scale, x, y);
        over(&mut pixel, BORDER, ring.max(0.0) * clip * BORDER_ALPHA);
        *out = pixel.map(|v| v.round() as u8);
    }
    Some(pixmap)
}

/// The bar in the box it is laid out in; `value` is from 0 to 1.
pub fn progress(value: f64) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, (), window, cx| {
            let (width, height) = skia::device_size(bounds, window.scale_factor());
            let scale = f64::from(window.scale_factor());
            let filled = (f64::from(width) * value).trunc() as u32;
            let key = ("progress", width, height, filled);
            let draw = || raster(width, height, value, scale);
            skia::paint(key, bounds, draw, window, cx);
        },
    )
    .size_full()
}
