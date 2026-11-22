//! Native-theme parts drawn the way Chromium rasterizes them: each part is a small SVG
//! document painted by [`crate::svg`], which models Graphite on macOS (analytic rects and round
//! rects, MSAA paths) and CPU Skia elsewhere, so half-pixel strokes and rounded corners keep
//! their coverage where GPUI would snap a quad to the pixel grid.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use asrs_canvas::Pixmap;
use asrs_canvas::path::Affine;
use gpui_kit::{App, Bounds, Corners, Global, Pixels, RenderImage, Window, px, size};
use image::{Frame, RgbaImage};

use crate::canvas_view::presenter::into_straight_bgra;
use crate::svg::{Backend, Document};

/// `fill` or `stroke` attributes for `0xRRGGBBAA`.
pub fn paint_attrs(property: &str, rgba: u32) -> String {
    format!(
        "{property}=\"#{:06x}\" {property}-opacity=\"{}\"",
        rgba >> 8,
        f64::from(rgba & 0xff) / 255.0
    )
}

/// A `<rect>` with equal corner radii, which Skia keeps as a round rect.
pub fn rect(x: f64, y: f64, w: f64, h: f64, r: f64, paint: &str) -> String {
    format!(
        "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" rx=\"{r}\" ry=\"{r}\" {paint}/>"
    )
}

/// Paints `body` (user units scaled by `scale` to device px) on a transparent
/// `width` x `height` pixmap.
pub fn svg(width: u32, height: u32, scale: f64, body: &str) -> Option<Pixmap> {
    let text = format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{body}</svg>");
    let doc = Document::parse(text.as_bytes())?;
    let mut pixmap = Pixmap::new(width, height)?;
    let clip = [0.0, 0.0, width as f32, height as f32];
    doc.paint(
        &Affine::scaling(scale, scale),
        clip,
        Backend::HOST,
        &mut pixmap,
    );
    Some(pixmap)
}

/// Source-over of premultiplied `src` onto `dst`, both the same size.
pub fn over(dst: &mut Pixmap, src: &Pixmap) {
    let (dst, _) = dst.data_mut().as_chunks_mut::<4>();
    let (src, _) = src.data().as_chunks::<4>();
    for (d, s) in dst.iter_mut().zip(src) {
        let inverse = 255 - u32::from(s[3]);
        for (d, &s) in d.iter_mut().zip(s) {
            *d = (u32::from(s) + (u32::from(*d) * inverse + 127) / 255) as u8;
        }
    }
}

const CACHE_ENTRIES: usize = 512;

#[derive(Default)]
struct PartCache {
    images: HashMap<u64, (Arc<RenderImage>, u64)>,
    clock: u64,
}

impl Global for PartCache {}

/// Paints the pixmap `draw` makes, one device pixel per pixel, at `bounds`' origin (on the
/// device grid). Pixmaps are cached by `key`, which must cover everything `draw` reads.
pub fn paint(
    key: impl Hash,
    bounds: Bounds<Pixels>,
    draw: impl FnOnce() -> Option<Pixmap>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    window.scale_factor().to_bits().hash(&mut hasher);
    let key = hasher.finish();
    let cache = cx.default_global::<PartCache>();
    cache.clock += 1;
    let clock = cache.clock;
    let image = match cache.images.get_mut(&key) {
        Some((image, used)) => {
            *used = clock;
            image.clone()
        }
        None => {
            let Some(pixmap) = draw() else {
                return;
            };
            let (w, h) = (pixmap.width(), pixmap.height());
            let Some(buffer) = RgbaImage::from_raw(w, h, into_straight_bgra(pixmap)) else {
                return;
            };
            let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
            if cache.images.len() >= CACHE_ENTRIES {
                let oldest = cache.images.iter().min_by_key(|(_, (_, used))| *used);
                if let Some(&k) = oldest.map(|(k, _)| k)
                    && let Some((old, _)) = cache.images.remove(&k)
                {
                    let _ = window.drop_image(old);
                }
            }
            cache.images.insert(key, (image.clone(), clock));
            image
        }
    };
    let scale = window.scale_factor();
    let device = image.size(0);
    let rect = Bounds {
        origin: bounds.origin,
        size: size(
            px(device.width.0 as f32 / scale),
            px(device.height.0 as f32 / scale),
        ),
    };
    let _ = window.paint_image(rect, rect, Corners::default(), image, 0, false);
}

/// `bounds` in device px, rounded to whole pixels: the canvas a part is drawn on.
pub fn device_size(bounds: Bounds<Pixels>, scale: f32) -> (u32, u32) {
    let s = |v: Pixels| (f32::from(v) * scale).round().max(0.0) as u32;
    (s(bounds.size.width), s(bounds.size.height))
}
