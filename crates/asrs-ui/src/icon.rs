use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use asrs_canvas::path::Affine;
use gpui_kit::{
    App, Bounds, Corners, Global, IntoElement, ParentElement, Pixels, RenderImage, RenderOnce,
    SharedString, Styled, Window, canvas, div, point, px, size,
};
use image::{Frame, RgbaImage};

use crate::filter::FilterChain;
use crate::svg::Document;
use crate::symbols::{self, DeviceImage, Intrinsic, Symbol};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconSource {
    Symbol(Symbol),
    Svg(SharedString),
}

impl IconSource {
    fn bytes(&self) -> &[u8] {
        match self {
            IconSource::Symbol(s) => s.svg(),
            IconSource::Svg(text) => text.as_bytes(),
        }
    }
}

impl From<Symbol> for IconSource {
    fn from(symbol: Symbol) -> Self {
        IconSource::Symbol(symbol)
    }
}

/// How the `<img>` sits in its box. `Centered` is `global.css`'s `button > img`: absolutely
/// positioned at `left: 50%; top: 50%`, `max-width` and `max-height` a fraction of the box, then
/// `transform: translate(-50%, -50%) scale(s)`. `Natural` is an unstyled image at the box's
/// top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IconFit {
    Centered { max: f64, scale: f64 },
    Natural,
}

impl IconFit {
    pub const BUTTON: IconFit = IconFit::Centered {
        max: 1.0,
        scale: 1.0,
    };
}

/// Where Chromium paints an `<img>`: the layout position `anchor` (logical px relative to its
/// box), the content box `size` in zoomed layout px (exact LayoutUnits), the transform's
/// `scale`, whether `translate(-50%, -50%)` centres it on the anchor, and the `zoom` it was laid
/// out at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub anchor: [f64; 2],
    pub size: [f64; 2],
    pub scale: f64,
    pub centered: bool,
    pub zoom: f64,
}

/// A CSS length in LayoutUnits (1/64 px), truncated as `LayoutUnit(float)` truncates.
fn lu(v: f64) -> i64 {
    (v * 64.0) as i64
}

fn from_lu(raw: i64) -> f64 {
    raw as f64 / 64.0
}

/// `LayoutUnit::MulDiv`.
fn mul_div(v: i64, m: i64, d: i64) -> i64 {
    v * m / d
}

/// `LayoutUnit::Round` of a size at a whole-pixel offset, the size `ToPixelSnappedRect` gives.
fn snap(raw: i64) -> f64 {
    ((raw + 32) >> 6) as f64
}

impl Placement {
    /// The image as Chromium draws it for a box at `origin` (logical px). The layout position
    /// is snapped to a whole device pixel (half up) and the transform applied after, so only
    /// the translate and scale leave fractions: at 1x a box centre at x.5 lands on x + 1, while
    /// a 12.5 px wide image keeps its -6.25 px shift.
    pub fn device_image(&self, origin: [f64; 2], dpr: f64) -> DeviceImage {
        let anchor = [0, 1].map(|i| ((origin[i] + self.anchor[i]) * dpr + 0.5).floor());
        let s = self.scale;
        let (e, f) = if self.centered {
            (
                anchor[0] - s * self.size[0] * 0.5,
                anchor[1] - s * self.size[1] * 0.5,
            )
        } else {
            (anchor[0], anchor[1])
        };
        DeviceImage {
            matrix: Affine {
                a: s,
                b: 0.0,
                c: 0.0,
                d: s,
                e,
                f,
            },
            dst: self.size.map(|v| snap(lu(v))),
            target: self.size.map(|v| v as f32),
            zoom: self.zoom as f32,
        }
    }
}

/// The CSS 2.1 section 10.4 resolution of `max-width` and `max-height` for a replaced element
/// with a natural size, in LayoutUnits.
fn constrain(w: i64, h: i64, max_w: i64, max_h: i64) -> (i64, i64) {
    let (over_w, over_h) = (w > max_w, h > max_h);
    match (over_w, over_h) {
        (false, false) => (w, h),
        (true, false) => (max_w, mul_div(max_w, h, w)),
        (false, true) => (mul_div(max_h, w, h), max_h),
        (true, true) if (max_w as f64 / w as f64) <= (max_h as f64 / h as f64) => {
            (max_w, mul_div(max_w, h, w))
        }
        (true, true) => (mul_div(max_h, w, h), max_h),
    }
}

/// The size Chromium gives an absolutely positioned image with only a natural ratio `rw:rh`:
/// the block size follows from the stretched inline size and is clamped by `max-height`, then
/// the inline size is derived back from it, each step truncated to a LayoutUnit (a 14 px wide
/// box gives 13.984375 x 21.765625 for a 72:112 ratio).
fn ratio_size(rw: i64, rh: i64, available: i64, max_w: i64, max_h: i64) -> (i64, i64) {
    let block = mul_div(available.min(max_w).max(0), rh, rw).min(max_h);
    let inline = mul_div(block, rw, rh).min(max_w);
    (inline, block)
}

pub fn layout(intrinsic: Intrinsic, fit: IconFit, bw: f64, bh: f64, zoom: f64) -> Placement {
    let bw_lu = lu(bw * zoom);
    let ratio = |w: f64, h: f64| (lu(w), lu(h));
    match fit {
        IconFit::Natural => {
            let (w, h) = match intrinsic {
                Intrinsic::Size(w, h) => (lu(w * zoom), lu(h * zoom)),
                Intrinsic::Ratio(rw, rh) => {
                    let (rw, rh) = ratio(rw, rh);
                    (bw_lu, mul_div(bw_lu, rh, rw))
                }
            };
            Placement {
                anchor: [0.0, 0.0],
                size: [from_lu(w), from_lu(h)],
                scale: 1.0,
                centered: false,
                zoom,
            }
        }
        IconFit::Centered { max, scale } => {
            let left = lu(bw * zoom * 0.5);
            let top = lu(bh * zoom * 0.5);
            let (max_w, max_h) = (lu(bw * zoom * max), lu(bh * zoom * max));
            let (w, h) = match intrinsic {
                Intrinsic::Size(w, h) => constrain(lu(w * zoom), lu(h * zoom), max_w, max_h),
                Intrinsic::Ratio(rw, rh) => {
                    let (rw, rh) = ratio(rw, rh);
                    ratio_size(rw, rh, bw_lu - left, max_w, max_h)
                }
            };
            Placement {
                anchor: [from_lu(left) / zoom, from_lu(top) / zoom],
                size: [from_lu(w), from_lu(h)],
                scale,
                centered: true,
                zoom,
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    source: u64,
    chain: &'static str,
    matrix: [u64; 4],
    size: [u64; 2],
    zoom: u32,
}

#[derive(Default)]
struct IconCache {
    documents: HashMap<u64, Option<Arc<Document>>>,
    images: HashMap<Key, Option<(i32, i32, Arc<RenderImage>)>>,
}

impl Global for IconCache {}

fn source_key(source: &IconSource) -> u64 {
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    h.finish()
}

/// Rasterizes and caches one icon. Only the sub-pixel part of the device position is keyed, so
/// the cache never evicts: keys are bounded by symbols x chains x the few sizes and scales the
/// UI uses, however far icons move or scroll.
fn cached_image(
    source: &IconSource,
    chain: &'static str,
    place: &Placement,
    image: DeviceImage,
    cx: &mut App,
) -> Option<(i32, i32, Arc<RenderImage>)> {
    let (dx, dy) = (image.matrix.e.floor(), image.matrix.f.floor());
    let mut local = image;
    local.matrix.e -= dx;
    local.matrix.f -= dy;
    let shift =
        |(rx, ry, image): (i32, i32, Arc<RenderImage>)| (rx + dx as i32, ry + dy as i32, image);
    let source_hash = source_key(source);
    let m = local.matrix;
    let key = Key {
        source: source_hash,
        chain,
        matrix: [m.a, m.d, m.e, m.f].map(f64::to_bits),
        size: place.size.map(f64::to_bits),
        zoom: local.zoom.to_bits(),
    };
    let cache = cx.default_global::<IconCache>();
    if let Some(hit) = cache.images.get(&key) {
        return hit.clone().map(shift);
    }
    let doc = cache
        .documents
        .entry(source_hash)
        .or_insert_with(|| symbols::parse(source.bytes()).map(Arc::new))
        .clone();
    let image = doc.and_then(|doc| {
        let filter = FilterChain::parse(chain).ok()?;
        let raster = symbols::rasterize(&doc, &local, &filter)?;
        let buffer = RgbaImage::from_raw(raster.width, raster.height, raster.bgra)?;
        let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
        Some((raster.x, raster.y, image))
    });
    cache.images.insert(key, image.clone());
    image.map(shift)
}

pub fn paint_icon(
    source: &IconSource,
    chain: &'static str,
    placement: &Placement,
    origin: [f64; 2],
    window: &mut Window,
    cx: &mut App,
) {
    let dpr = f64::from(window.scale_factor());
    let device = placement.device_image(origin, dpr);
    let Some((x, y, image)) = cached_image(source, chain, placement, device, cx) else {
        return;
    };
    let device_size = image.size(0);
    let to_px = |v: f64| px((v / dpr) as f32);
    let bounds = Bounds {
        origin: point(to_px(f64::from(x)), to_px(f64::from(y))),
        size: size(
            to_px(f64::from(device_size.width.0)),
            to_px(f64::from(device_size.height.0)),
        ),
    };
    let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
}

#[derive(IntoElement)]
pub struct Icon {
    source: IconSource,
    chain: &'static str,
    fit: IconFit,
}

impl Icon {
    pub fn new(source: impl Into<IconSource>, chain: &'static str) -> Self {
        Icon {
            source: source.into(),
            chain,
            fit: IconFit::BUTTON,
        }
    }

    pub fn fit(mut self, fit: IconFit) -> Self {
        self.fit = fit;
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Icon { source, chain, fit } = self;
        div().absolute().inset_0().child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window, cx| {
                    let Some(intrinsic) = symbols::intrinsic(source.bytes()) else {
                        return;
                    };
                    let placement = layout(
                        intrinsic,
                        fit,
                        f64::from(bounds.size.width),
                        f64::from(bounds.size.height),
                        f64::from(window.scale_factor()),
                    );
                    let origin = [f64::from(bounds.origin.x), f64::from(bounds.origin.y)];
                    paint_icon(&source, chain, &placement, origin, window, cx);
                },
            )
            .size_full(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(i: Intrinsic, fit: IconFit, b: f64, origin: [f64; 2], dpr: f64) -> [f64; 4] {
        let d = layout(i, fit, b, b, dpr).device_image(origin, dpr);
        let s = d.matrix.a;
        [d.matrix.e, d.matrix.f, d.dst[0] * s, d.dst[1] * s]
    }

    #[test]
    fn button_placement_matches_chromium() {
        let s24 = Intrinsic::Size(24.0, 24.0);
        assert_eq!(
            rect(s24, IconFit::BUTTON, 28.0, [10.0, 10.0], 1.0),
            [12.0, 12.0, 24.0, 24.0]
        );
        assert_eq!(
            rect(s24, IconFit::BUTTON, 20.0, [0.0, 0.0], 2.0),
            [0.0, 0.0, 40.0, 40.0]
        );
        assert_eq!(
            rect(s24, IconFit::BUTTON, 25.0, [10.5, 366.0], 1.0),
            [11.0, 367.0, 24.0, 24.0]
        );
        assert_eq!(
            rect(s24, IconFit::BUTTON, 25.0, [10.5, 366.0], 2.0),
            [22.0, 733.0, 48.0, 48.0]
        );
        let feedback = IconFit::Centered {
            max: 1.0,
            scale: 0.9,
        };
        let r = rect(s24, feedback, 28.0, [0.0, 0.0], 1.0);
        assert!((r[2] - 21.6).abs() < 1e-9 && (r[0] - 3.2).abs() < 1e-9);
    }

    #[test]
    fn ratio_only_images_size_in_layout_units() {
        let lamp = Intrinsic::Ratio(72.0, 112.0);
        let at1 = layout(lamp, IconFit::BUTTON, 28.0, 28.0, 1.0);
        assert_eq!(at1.size, [13.984375, 21.765625]);
        let at2 = layout(lamp, IconFit::BUTTON, 28.0, 28.0, 2.0);
        assert_eq!(at2.size, [27.984375, 43.546875]);
        assert_eq!(at2.device_image([0.0, 0.0], 2.0).dst, [28.0, 44.0]);
        let half = IconFit::Centered {
            max: 0.5,
            scale: 1.2,
        };
        assert_eq!(layout(lamp, half, 40.0, 40.0, 1.0).size, [12.84375, 20.0]);
    }
}
