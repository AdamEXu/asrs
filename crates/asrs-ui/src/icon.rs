use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use gpui_kit::{
    App, Bounds, Corners, Global, IntoElement, ParentElement, Pixels, RenderImage, RenderOnce,
    SharedString, Styled, Window, canvas, div, point, px, size,
};
use image::{Frame, RgbaImage};
use resvg::usvg;

use crate::filter::FilterChain;
use crate::symbols::{self, Intrinsic, Symbol};

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

/// Where Chromium paints an `<img>`, in logical px relative to its box: the layout position
/// `anchor`, the layout `size` (before the transform) and the transform's `scale`. `centered`
/// images are moved by `translate(-50%, -50%)` around the anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub anchor: [f64; 2],
    pub size: [f64; 2],
    pub scale: f64,
    pub centered: bool,
}

impl Placement {
    /// The device-space image rectangle for a box at `origin`. Chromium snaps the layout
    /// position to a whole device pixel (half up) and applies the transform after, so only the
    /// translate and scale leave fractions: at 1x a box centre at x.5 lands on x + 1, while a
    /// 12.5 px wide image keeps its -6.25 px shift.
    pub fn device_rect(&self, origin: [f64; 2], dpr: f64) -> [f64; 4] {
        let anchor = [0, 1].map(|i| ((origin[i] + self.anchor[i]) * dpr + 0.5).floor());
        let [w, h] = self.size.map(|v| v * self.scale * dpr);
        if self.centered {
            [anchor[0] - w * 0.5, anchor[1] - h * 0.5, w, h]
        } else {
            [anchor[0], anchor[1], w, h]
        }
    }

    /// The viewport an SVG without intrinsic size is laid out in before being stretched onto
    /// the image box: Chromium sizes it to the box rounded to whole CSS px.
    pub fn viewport(&self, intrinsic: Intrinsic) -> Option<[f64; 2]> {
        match intrinsic {
            Intrinsic::Size(..) => None,
            Intrinsic::Ratio(_) => Some(self.size.map(|v| (v + 0.5).floor())),
        }
    }
}

/// The CSS 2.1 section 10.4 resolution of `max-width` and `max-height` for a replaced element.
fn constrain(w: f64, h: f64, max_w: f64, max_h: f64) -> (f64, f64) {
    let (over_w, over_h) = (w > max_w, h > max_h);
    match (over_w, over_h) {
        (false, false) => (w, h),
        (true, false) => (max_w, (max_w * h / w)),
        (false, true) => ((max_h * w / h), max_h),
        (true, true) if max_w / w <= max_h / h => (max_w, (max_w * h / w)),
        (true, true) => ((max_h * w / h), max_h),
    }
}

pub fn layout(intrinsic: Intrinsic, fit: IconFit, bw: f64, bh: f64) -> Placement {
    match fit {
        IconFit::Natural => {
            let (w, h) = match intrinsic {
                Intrinsic::Size(w, h) => (w, h),
                Intrinsic::Ratio(r) => (bw, bw / r),
            };
            Placement {
                anchor: [0.0, 0.0],
                size: [w, h],
                scale: 1.0,
                centered: false,
            }
        }
        IconFit::Centered { max, scale } => {
            let (w, h) = match intrinsic {
                Intrinsic::Size(w, h) => (w, h),
                // With only a ratio, an absolutely positioned image takes the width left after
                // `left: 50%`.
                Intrinsic::Ratio(r) => (bw * 0.5, bw * 0.5 / r),
            };
            let (w, h) = constrain(w, h, bw * max, bh * max);
            Placement {
                anchor: [bw * 0.5, bh * 0.5],
                size: [w, h],
                scale,
                centered: true,
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    source: u64,
    chain: &'static str,
    rect: [u64; 4],
    viewport: Option<[u64; 2]>,
}

#[derive(Default)]
struct IconCache {
    trees: HashMap<u64, Option<Arc<usvg::Tree>>>,
    images: HashMap<Key, Option<(i32, i32, Arc<RenderImage>)>>,
}

impl Global for IconCache {}

fn source_key(source: &IconSource) -> u64 {
    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    h.finish()
}

/// Rasterizes and caches one icon at a device-space rectangle. The cache never evicts: keys are
/// bounded by symbols x chains x the few sizes and scales the UI uses.
fn cached_image(
    source: &IconSource,
    chain: &'static str,
    rect: [f64; 4],
    viewport: Option<[f64; 2]>,
    cx: &mut App,
) -> Option<(i32, i32, Arc<RenderImage>)> {
    let source_hash = source_key(source);
    let key = Key {
        source: source_hash,
        chain,
        rect: rect.map(f64::to_bits),
        viewport: viewport.map(|v| v.map(f64::to_bits)),
    };
    let cache = cx.default_global::<IconCache>();
    if let Some(hit) = cache.images.get(&key) {
        return hit.clone();
    }
    let tree = cache
        .trees
        .entry(source_hash)
        .or_insert_with(|| symbols::parse(source.bytes()).map(Arc::new))
        .clone();
    let image = tree.and_then(|tree| {
        let filter = FilterChain::parse(chain).ok()?;
        let raster = symbols::rasterize(&tree, rect, viewport, &filter)?;
        let buffer = RgbaImage::from_raw(raster.width, raster.height, raster.bgra)?;
        let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
        Some((raster.x, raster.y, image))
    });
    cache.images.insert(key, image.clone());
    image
}

pub fn paint_icon(
    source: &IconSource,
    chain: &'static str,
    placement: &Placement,
    origin: [f64; 2],
    window: &mut Window,
    cx: &mut App,
) {
    let Some(intrinsic) = symbols::intrinsic(source.bytes()) else {
        return;
    };
    let dpr = f64::from(window.scale_factor());
    let rect = placement.device_rect(origin, dpr);
    let viewport = placement.viewport(intrinsic);
    let Some((x, y, image)) = cached_image(source, chain, rect, viewport, cx) else {
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

    #[test]
    fn button_placement_matches_chromium() {
        let s24 = Intrinsic::Size(24.0, 24.0);
        let at =
            |i, fit, b: f64, origin: [f64; 2], dpr| layout(i, fit, b, b).device_rect(origin, dpr);
        assert_eq!(
            at(s24, IconFit::BUTTON, 28.0, [10.0, 10.0], 1.0),
            [12.0, 12.0, 24.0, 24.0]
        );
        assert_eq!(
            at(s24, IconFit::BUTTON, 20.0, [0.0, 0.0], 2.0),
            [0.0, 0.0, 40.0, 40.0]
        );
        assert_eq!(
            at(s24, IconFit::BUTTON, 25.0, [10.5, 366.0], 1.0),
            [11.0, 367.0, 24.0, 24.0]
        );
        assert_eq!(
            at(s24, IconFit::BUTTON, 25.0, [10.5, 366.0], 2.0),
            [22.0, 733.0, 48.0, 48.0]
        );
        let lamp = layout(Intrinsic::Ratio(72.0 / 112.0), IconFit::BUTTON, 28.0, 28.0);
        assert_eq!(lamp.size[0], 14.0);
        assert!((lamp.size[1] - 14.0 * 112.0 / 72.0).abs() < 1e-9);
        assert_eq!(
            lamp.viewport(Intrinsic::Ratio(72.0 / 112.0)),
            Some([14.0, 22.0])
        );
        let feedback = IconFit::Centered {
            max: 1.0,
            scale: 0.9,
        };
        let r = at(s24, feedback, 28.0, [0.0, 0.0], 1.0);
        assert!((r[2] - 21.6).abs() < 1e-9 && (r[0] - 3.2).abs() < 1e-9);
    }
}
