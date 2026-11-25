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

/// `PaintTextField`'s background, which it draws without antialiasing: a pixel is in when its
/// centre is, with the straight edges rounded as `SkScan::FillRect` rounds them.
pub fn fill_aliased_rrect(pixmap: &mut Pixmap, rect: [f64; 4], radius: f64, rgba: u32) {
    blend_where(pixmap, rgba, |px, py| {
        in_aliased_rrect(rect, radius, px, py)
    });
}

/// Its border, which only a GPU antialiases all the same: a stroke along `rect`.
pub fn stroke_aliased_rrect(
    pixmap: &mut Pixmap,
    [x, y, w, h]: [f64; 4],
    radius: f64,
    stroke: f64,
    rgba: u32,
) {
    let grown = |by: f64| [x - by, y - by, w + 2.0 * by, h + 2.0 * by];
    let half = stroke / 2.0;
    blend_where(pixmap, rgba, |px, py| {
        in_aliased_rrect(grown(half), radius + half, px, py)
            && !in_aliased_rrect(grown(-half), (radius - half).max(0.0), px, py)
    });
}

fn in_aliased_rrect([x, y, w, h]: [f64; 4], radius: f64, px: i64, py: i64) -> bool {
    let round = |v: f64| (v + 0.5).floor() as i64;
    if !(round(x)..round(x + w)).contains(&px) || !(round(y)..round(y + h)).contains(&py) {
        return false;
    }
    let (cx, cy) = (px as f64 + 0.5, py as f64 + 0.5);
    let dx = (x + radius - cx).max(cx - (x + w - radius)).max(0.0);
    let dy = (y + radius - cy).max(cy - (y + h - radius)).max(0.0);
    dx == 0.0 || dy == 0.0 || dx * dx + dy * dy <= radius * radius
}

fn blend_where(pixmap: &mut Pixmap, rgba: u32, inside: impl Fn(i64, i64) -> bool) {
    let alpha = rgba & 0xff;
    let rgb = [(rgba >> 24) & 0xff, (rgba >> 16) & 0xff, (rgba >> 8) & 0xff];
    let width = i64::from(pixmap.width());
    let height = i64::from(pixmap.height());
    let data = pixmap.data_mut();
    for py in 0..height {
        for px in 0..width {
            if !inside(px, py) {
                continue;
            }
            let p = ((py * width + px) * 4) as usize;
            let inverse = 255 - alpha;
            for (c, &v) in rgb.iter().enumerate() {
                let src = (v * alpha + 127) / 255;
                data[p + c] = (src + (u32::from(data[p + c]) * inverse + 127) / 255) as u8;
            }
            data[p + 3] = (alpha + (u32::from(data[p + 3]) * inverse + 127) / 255) as u8;
        }
    }
}

/// The path of a round rect with corner radii from the top left one clockwise.
pub fn rrect_path([x, y, w, h]: [f64; 4], [tl, tr, br, bl]: [f64; 4]) -> String {
    let (r, b) = (x + w, y + h);
    format!(
        "M{} {y} L{} {y} A{tr} {tr} 0 0 1 {r} {} L{r} {} A{br} {br} 0 0 1 {} {b} L{} {b} \
         A{bl} {bl} 0 0 1 {x} {} L{x} {} A{tl} {tl} 0 0 1 {} {y} Z",
        x + tl,
        r - tr,
        y + tr,
        b - br,
        r - br,
        x + bl,
        b - bl,
        y + tl,
        x + tl,
    )
}

/// A box with a border of one colour and width, as Blink paints it on the CPU under
/// `kBackgroundBleedClipLayer`: the border and, inside its inner edge, the background in a layer
/// that the outer edge clips.
pub fn bordered_box(
    width: u32,
    height: u32,
    radii: [f64; 4],
    border: f64,
    border_color: u32,
    background: u32,
) -> Option<Pixmap> {
    let (w, h) = (f64::from(width), f64::from(height));
    let path = |rect: [f64; 4], radii: [f64; 4], rgba: u32| {
        let d = rrect_path(rect, radii);
        svg(
            width,
            height,
            1.0,
            &format!("<path d=\"{d}\" {}/>", paint_attrs("fill", rgba)),
        )
    };
    let mut pixmap = svg(
        width,
        height,
        1.0,
        &rect(0.0, 0.0, w, h, 0.0, &paint_attrs("fill", border_color)),
    )?;
    let inner = [border, border, w - 2.0 * border, h - 2.0 * border];
    let inner_radii = radii.map(|r| (r - border).max(0.0));
    over(&mut pixmap, &path(inner, inner_radii, background)?);
    clipped(&mut pixmap, &path([0.0, 0.0, w, h], radii, 0xffff_ffff)?);
    Some(pixmap)
}

/// `pixmap` through an antialiased clip that covers what `clip` does.
pub fn clipped(pixmap: &mut Pixmap, clip: &Pixmap) {
    let (data, _) = pixmap.data_mut().as_chunks_mut::<4>();
    let (clip, _) = clip.data().as_chunks::<4>();
    for (p, c) in data.iter_mut().zip(clip) {
        for v in p {
            *v = ((u32::from(*v) * u32::from(c[3]) + 127) / 255) as u8;
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
    paint_with_app(key, bounds, |_| draw(), window, cx);
}

/// [`paint`] with a `draw` that reads the app, such as the text system.
pub fn paint_with_app(
    key: impl Hash,
    bounds: Bounds<Pixels>,
    draw: impl FnOnce(&mut App) -> Option<Pixmap>,
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
    let cached = cache.images.get_mut(&key).map(|(image, used)| {
        *used = clock;
        image.clone()
    });
    let image = match cached {
        Some(image) => image,
        None => {
            let Some(pixmap) = draw(cx) else {
                return;
            };
            let cache = cx.default_global::<PartCache>();
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
