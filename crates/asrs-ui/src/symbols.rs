use asrs_canvas::Pixmap;
use asrs_canvas::hairline::P;
use asrs_canvas::path::Affine;

use crate::filter::FilterChain;
use crate::svg::{Backend, Document};

/// One of upstream's `www/symbols` SVGs, embedded at build time. Upstream refers to them by path
/// (`symbols/add.svg`, `symbols/sourceList/<symbol>.svg`), so lookup is by that path too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Symbol(usize);

include!(concat!(env!("OUT_DIR"), "/symbols.rs"));

impl Symbol {
    /// `name` is relative to `www/symbols`, e.g. `add.svg` or `sourceList/360.svg`; a leading
    /// `symbols/` is accepted.
    pub fn named(name: &str) -> Option<Symbol> {
        let name = name.strip_prefix("symbols/").unwrap_or(name);
        EMBEDDED.iter().position(|(n, _)| *n == name).map(Symbol)
    }

    pub fn source_list(symbol: &str) -> Option<Symbol> {
        Symbol::named(&format!("sourceList/{symbol}.svg"))
    }

    pub fn all() -> impl Iterator<Item = Symbol> {
        (0..EMBEDDED.len()).map(Symbol)
    }

    pub fn name(self) -> &'static str {
        EMBEDDED[self.0].0
    }

    pub fn svg(self) -> &'static [u8] {
        EMBEDDED[self.0].1
    }
}

/// The `width` and `height` attributes of the root element in CSS px. An SVG with only a
/// `viewBox` has no natural size, only the natural ratio of the box's width to its height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intrinsic {
    Size(f64, f64),
    Ratio(f64, f64),
}

pub fn intrinsic(svg: &[u8]) -> Option<Intrinsic> {
    let text = std::str::from_utf8(svg).ok()?;
    let start = text.find("<svg")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let attr = |name: &str| {
        let key = format!(" {name}=\"");
        let at = tag.find(&key)? + key.len();
        let value = &tag[at..at + tag[at..].find('"')?];
        Some(value)
    };
    let px = |v: &str| v.strip_suffix("px").unwrap_or(v).trim().parse::<f64>().ok();
    match (attr("width").and_then(px), attr("height").and_then(px)) {
        (Some(w), Some(h)) if w.is_finite() && h.is_finite() => Some(Intrinsic::Size(w, h)),
        _ => {
            let mut view_box = attr("viewBox")?
                .split([' ', ','])
                .filter(|s| !s.is_empty())
                .map(|s| s.parse::<f64>().ok());
            let (_, _, w, h) = (
                view_box.next()??,
                view_box.next()??,
                view_box.next()??,
                view_box.next()??,
            );
            let representable = |v: f64| v.is_finite() && v * 64.0 >= 1.0;
            (representable(w) && representable(h)).then_some(Intrinsic::Ratio(w, h))
        }
    }
}

pub fn parse(svg: &[u8]) -> Option<Document> {
    Document::parse(svg)
}

/// Straight-alpha BGRA pixels covering device pixels `[x, x + width) x [y, y + height)`, the
/// layout `RenderImage` expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// An SVG `<img>` as Blink paints it: `matrix` maps the image's local box (zoomed layout px) to
/// device px, `dst` is that box pixel-snapped (`ImagePainter`'s destination rect), `target` the
/// unsnapped box the `SVGImageForContainer` is sized from, and `zoom` the device scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceImage {
    pub matrix: Affine,
    pub dst: [f64; 2],
    pub target: [f32; 2],
    pub zoom: f32,
}

impl DeviceImage {
    /// `SVGImage` lays the document out in the target size without zoom (its root in that
    /// size floored to a LayoutUnit) and maps that size onto `dst`. Electron 39 does not
    /// compensate for rounding the container size, so the content always fills `dst`.
    fn frame(&self) -> (Affine, [f32; 2]) {
        let inv_zoom = 1.0 / self.zoom;
        let container = self.target.map(|v| v * inv_zoom);
        let src_to_dst = Affine::scaling(
            f64::from((self.dst[0] as f32) / container[0]),
            f64::from((self.dst[1] as f32) / container[1]),
        );
        let viewport = container.map(|v| (v * 64.0).floor() / 64.0);
        (self.matrix.then(&src_to_dst), viewport)
    }
}

fn mapped_rect(m: &Affine, [w, h]: [f64; 2]) -> [f32; 4] {
    let a = asrs_canvas::skpath::map_point(m, P { x: 0.0, y: 0.0 });
    let b = asrs_canvas::skpath::map_point(
        m,
        P {
            x: w as f32,
            y: h as f32,
        },
    );
    [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)]
}

/// Paints `doc` as Chromium paints the `<img>` at `place`, then runs `chain` over every pixel
/// on unpremultiplied colour, as Chromium paints a filtered `<img>`.
pub fn rasterize(doc: &Document, place: &DeviceImage, chain: &FilterChain) -> Option<Raster> {
    if !(place.dst[0] > 0.0 && place.dst[1] > 0.0) {
        return None;
    }
    let (frame, viewport) = place.frame();
    if !(viewport[0] > 0.0 && viewport[1] > 0.0) {
        return None;
    }
    let dst = mapped_rect(&place.matrix, place.dst);
    let bw = dst.map(|v| (v + 0.5).floor() as i32);
    let (width, height) = ((bw[2] - bw[0]) as u32, (bw[3] - bw[1]) as u32);
    let mut pixmap = Pixmap::new(width, height)?;
    let shift = Affine::translation(-f64::from(bw[0]), -f64::from(bw[1]));
    let ctm = shift.then(&frame).then(&doc.view_transform(viewport));
    let clip = [0, 1, 2, 3].map(|i| dst[i] - bw[i % 2] as f32);
    doc.paint(&ctm, clip, Backend::HOST, &mut pixmap);
    let mut bgra = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let a = px.alpha();
        let straight = if a == 0 {
            [0, 0, 0, 0]
        } else {
            let c = px.demultiply();
            chain.apply([c.red(), c.green(), c.blue(), a])
        };
        bgra.extend_from_slice(&[straight[2], straight[1], straight[0], straight[3]]);
    }
    Some(Raster {
        x: bw[0],
        y: bw[1],
        width,
        height,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_parses_and_has_a_size() {
        assert!(Symbol::all().count() >= 70);
        for symbol in Symbol::all() {
            assert!(parse(symbol.svg()).is_some(), "{}", symbol.name());
            assert!(intrinsic(symbol.svg()).is_some(), "{}", symbol.name());
        }
        assert_eq!(intrinsic(ADD.svg()), Some(Intrinsic::Size(24.0, 24.0)));
        assert_eq!(
            intrinsic(TBALAMP.svg()),
            Some(Intrinsic::Ratio(72.0, 112.0))
        );
    }

    #[test]
    fn lookup_by_upstream_path() {
        assert_eq!(Symbol::named("symbols/add.svg"), Some(ADD));
        assert_eq!(Symbol::named("close.svg"), Some(CLOSE));
        assert_eq!(
            Symbol::source_list("360").map(Symbol::name),
            Some("sourceList/360.svg")
        );
        assert_eq!(Symbol::named("missing.svg"), None);
    }

    #[test]
    fn opaque_square_takes_the_filtered_colour() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#000000"/></svg>"##;
        let chain = FilterChain::parse(crate::theme::chains::BUTTON_LIGHT).unwrap();
        let place = DeviceImage {
            matrix: Affine::IDENTITY,
            dst: [4.0, 4.0],
            target: [4.0, 4.0],
            zoom: 1.0,
        };
        let r = rasterize(&parse(svg).unwrap(), &place, &chain).unwrap();
        assert_eq!((r.width, r.height), (4, 4));
        assert_eq!(&r.bgra[..4], &[84, 84, 84, 255]);
    }
}
