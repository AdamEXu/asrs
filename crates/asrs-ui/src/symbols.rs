use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg;

use crate::filter::FilterChain;

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
/// `viewBox` has an intrinsic ratio and no intrinsic size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intrinsic {
    Size(f64, f64),
    Ratio(f64),
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
        (Some(w), Some(h)) => Some(Intrinsic::Size(w, h)),
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
            Some(Intrinsic::Ratio(w / h))
        }
    }
}

pub fn parse(svg: &[u8]) -> Option<usvg::Tree> {
    usvg::Tree::from_data(svg, &usvg::Options::default()).ok()
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

/// Draws `tree` into the device-space rectangle `(x, y, w, h)` with anti-aliasing, then runs
/// `chain` over every pixel on unpremultiplied colour, as Chromium paints a filtered `<img>`.
pub fn rasterize(tree: &usvg::Tree, rect: [f64; 4], chain: &FilterChain) -> Option<Raster> {
    let [x, y, w, h] = rect;
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let (x0, y0) = (x.floor(), y.floor());
    let width = ((x + w).ceil() - x0) as u32;
    let height = ((y + h).ceil() - y0) as u32;
    let mut pixmap = Pixmap::new(width, height)?;
    let size = tree.size();
    let transform = Transform::from_row(
        (w / f64::from(size.width())) as f32,
        0.0,
        0.0,
        (h / f64::from(size.height())) as f32,
        (x - x0) as f32,
        (y - y0) as f32,
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());
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
        x: x0 as i32,
        y: y0 as i32,
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
            Some(Intrinsic::Ratio(72.0 / 112.0))
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
        let r = rasterize(&parse(svg).unwrap(), [0.0, 0.0, 4.0, 4.0], &chain).unwrap();
        assert_eq!((r.width, r.height), (4, 4));
        assert_eq!(&r.bgra[..4], &[84, 84, 84, 255]);
    }
}
