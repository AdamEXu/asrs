//! What a frame has painted so far, for text that is blended onto it before the GPU sees either.

use std::ops::Range;

use gpui_kit::{Bounds, Hsla, Primitive, Quad, Rgba, ScaledPixels, Scene};

/// Left, top, right and bottom in device px.
pub type Rect = [i32; 4];

const BAND: i32 = 64;
/// A run over more primitives than this is taken as one over an image.
const MAX_OVERLAPS: usize = 64;

pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

fn contains(outer: Rect, inner: Rect) -> bool {
    outer[0] <= inner[0] && outer[1] <= inner[1] && outer[2] >= inner[2] && outer[3] >= inner[3]
}

fn inner(b: &Bounds<ScaledPixels>) -> Rect {
    [
        b.left().0.ceil() as i32,
        b.top().0.ceil() as i32,
        b.right().0.floor() as i32,
        b.bottom().0.floor() as i32,
    ]
}

fn outer(b: &Bounds<ScaledPixels>) -> Rect {
    [
        b.left().0.floor() as i32,
        b.top().0.floor() as i32,
        b.right().0.ceil() as i32,
        b.bottom().0.ceil() as i32,
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fill {
    Color(Rgba),
    /// Anything whose pixels are not one colour over the rectangle.
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct Item {
    rect: Rect,
    fill: Fill,
}

impl Item {
    fn opaque(&self) -> bool {
        matches!(self.fill, Fill::Color(c) if c.a >= 1.0)
    }
}

/// Paint operations that are not read: all they leave, if anything, is `color` in `rect`.
struct Known {
    range: Range<usize>,
    fill: Option<(Rect, Rgba)>,
}

/// The primitives of one window's frame, indexed by the rows they touch.
#[derive(Default)]
pub struct Backdrop {
    seen: usize,
    known: Vec<Known>,
    items: Vec<Item>,
    bands: Vec<Vec<u32>>,
}

impl Backdrop {
    pub fn clear(&mut self) {
        self.seen = 0;
        self.known.clear();
        self.items.clear();
        self.bands.iter_mut().for_each(Vec::clear);
    }

    /// Takes in what `scene` painted since the last call.
    pub fn read(&mut self, scene: &Scene) {
        for Known { range, fill } in std::mem::take(&mut self.known) {
            self.take(scene, self.seen..range.start);
            self.seen = range.end;
            if let Some((rect, color)) = fill {
                self.add(rect, Fill::Color(color));
            }
        }
        self.take(scene, self.seen..scene.len());
        self.seen = scene.len();
    }

    /// Leaves out what the scene painted in `range`.
    pub fn skip(&mut self, range: Range<usize>) {
        self.known.push(Known { range, fill: None });
    }

    /// Takes what the scene painted in `range` as `color` over `rect`.
    pub fn fill(&mut self, range: Range<usize>, rect: Rect, color: Rgba) {
        let fill = Some((rect, color));
        self.known.push(Known { range, fill });
    }

    fn take(&mut self, scene: &Scene, range: Range<usize>) {
        for primitive in scene.primitives(range) {
            match primitive {
                Primitive::Quad(quad) => self.quad(quad),
                Primitive::PolychromeSprite(sprite) if sprite.opacity <= 0.0 => {}
                Primitive::Shadow(shadow) => {
                    let reach = ScaledPixels(shadow.blur_radius.0 * 3.0);
                    let bounds = shadow.bounds.dilate(reach);
                    self.push(outer(&bounds), &shadow.content_mask.bounds, Fill::Unknown);
                }
                other => {
                    let clip = other.content_mask().bounds;
                    self.push(outer(other.bounds()), &clip, Fill::Unknown);
                }
            }
        }
    }

    fn quad(&mut self, quad: &Quad) {
        let clip = &quad.content_mask.bounds;
        let (bounds, whole) = (inner(&quad.bounds), outer(&quad.bounds));
        if bounds != whole {
            self.push(whole, clip, Fill::Unknown);
        }
        match quad.background.as_solid() {
            Some(color) => self.push(bounds, clip, color_fill(color)),
            None if quad.background.is_transparent() => {}
            None => self.push(bounds, clip, Fill::Unknown),
        }
        let [l, t, r, b] = bounds;
        let edge = |v: ScaledPixels| v.0.ceil() as i32;
        let widths = quad.border_widths.map(|w| edge(*w));
        if !quad.border_color.is_transparent() {
            let fill = color_fill(quad.border_color);
            let (top, bottom) = (t + widths.top, b - widths.bottom);
            for strip in [
                [l, t, r, top],
                [l, bottom, r, b],
                [l, top, l + widths.left, bottom],
                [r - widths.right, top, r, bottom],
            ] {
                self.push(strip, clip, fill);
            }
        }
        let radii = quad.corner_radii.map(|radius| edge(*radius));
        for (n, corner) in [
            (
                radii.top_left,
                [l, t, l + radii.top_left, t + radii.top_left],
            ),
            (
                radii.top_right,
                [r - radii.top_right, t, r, t + radii.top_right],
            ),
            (
                radii.bottom_left,
                [l, b - radii.bottom_left, l + radii.bottom_left, b],
            ),
            (
                radii.bottom_right,
                [r - radii.bottom_right, b - radii.bottom_right, r, b],
            ),
        ] {
            if n > 0 {
                self.push(corner, clip, Fill::Unknown);
            }
        }
    }

    fn push(&mut self, rect: Rect, clip: &Bounds<ScaledPixels>, fill: Fill) {
        if let Some(rect) = intersect(rect, outer(clip)) {
            self.add(rect, fill);
        }
    }

    fn add(&mut self, rect: Rect, fill: Fill) {
        if matches!(fill, Fill::Color(c) if c.a <= 0.0) || rect[3] <= 0 {
            return;
        }
        let index = self.items.len() as u32;
        self.items.push(Item { rect, fill });
        let (first, last) = (rect[1].max(0) / BAND, (rect[3] - 1) / BAND);
        if self.bands.len() <= last as usize {
            self.bands.resize_with(last as usize + 1, Vec::new);
        }
        for band in &mut self.bands[first as usize..=last as usize] {
            band.push(index);
        }
    }

    /// What is behind `rect`, topmost first, down to what hides the rest.
    fn behind(&self, rect: Rect) -> Vec<Item> {
        let (first, last) = (rect[1].max(0) / BAND, (rect[3] - 1).max(0) / BAND);
        let bands = || (first..=last).filter_map(|band| self.bands.get(band as usize));
        let mut found = Vec::new();
        let mut take = |index: u32| {
            let item = self.items[index as usize];
            if let Some(rect) = intersect(item.rect, rect) {
                found.push(Item { rect, ..item });
            }
            item.opaque() && contains(item.rect, rect)
        };
        if first == last {
            for &index in bands().flat_map(|band| band.iter().rev()) {
                if take(index) {
                    break;
                }
            }
        } else {
            let mut cursors: Vec<_> = bands().map(|band| band.iter().rev().peekable()).collect();
            while let Some(index) = cursors.iter_mut().filter_map(|c| c.peek().copied()).max() {
                for cursor in &mut cursors {
                    cursor.next_if(|next| *next == index);
                }
                if take(*index) {
                    break;
                }
            }
        }
        found
    }

    /// The parts of `rect` by the opaque colour behind them, or `None` where part of it is not
    /// behind one colour.
    pub fn regions(&self, rect: Rect) -> Option<Vec<(Rect, [u8; 3])>> {
        let items = self.behind(rect);
        match items.as_slice() {
            [] => return None,
            [only] if only.opaque() && only.rect == rect => {
                return Some(vec![(rect, blend([0; 3], only.fill)?)]);
            }
            items if items.len() > MAX_OVERLAPS => return None,
            _ => {}
        }
        let cuts = |low: usize, high: usize| {
            let mut v: Vec<i32> = items
                .iter()
                .flat_map(|item| [item.rect[low], item.rect[high]])
                .chain([rect[low], rect[high]])
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let (xs, ys) = (cuts(0, 2), cuts(1, 3));
        let mut regions: Vec<(Rect, [u8; 3])> = Vec::new();
        for y in ys.windows(2) {
            let row = regions.len();
            for x in xs.windows(2) {
                let cell = [x[0], y[0], x[1], y[1]];
                let color = color_of(&items, cell)?;
                match regions[row..].last_mut() {
                    Some((r, c)) if *c == color => r[2] = cell[2],
                    _ => regions.push((cell, color)),
                }
            }
            merge_rows(&mut regions, row);
        }
        Some(regions)
    }
}

/// Grows the row of regions before `row` by the one from `row` on where the two are cut alike.
fn merge_rows(regions: &mut Vec<(Rect, [u8; 3])>, row: usize) {
    let len = regions.len() - row;
    let Some(above) = row.checked_sub(len) else {
        return;
    };
    let alike = (0..len).all(|i| {
        let ((a, ac), (b, bc)) = (regions[above + i], regions[row + i]);
        ac == bc && a[0] == b[0] && a[2] == b[2] && a[3] == b[1]
    });
    if alike {
        for i in 0..len {
            regions[above + i].0[3] = regions[row + i].0[3];
        }
        regions.truncate(row);
    }
}

fn color_of(items: &[Item], cell: Rect) -> Option<[u8; 3]> {
    let over = || items.iter().filter(|item| contains(item.rect, cell));
    let base = over().position(|item| item.opaque())?;
    let layers: Vec<&Item> = over().take(base + 1).collect();
    layers
        .into_iter()
        .rev()
        .try_fold([0; 3], |under, item| blend(under, item.fill))
}

fn color_fill(color: Hsla) -> Fill {
    Fill::Color(Rgba::from(color))
}

fn blend(under: [u8; 3], fill: Fill) -> Option<[u8; 3]> {
    let Fill::Color(c) = fill else {
        return None;
    };
    let over = [c.r, c.g, c.b];
    Some([0, 1, 2].map(|i| {
        let under = f32::from(under[i]) / 255.0;
        ((over[i] * c.a + under * (1.0 - c.a)) * 255.0).round() as u8
    }))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        BorderStyle, ContentMask, Corners, Edges, Shadow, linear_color_stop, linear_gradient,
        point, rgb, rgba, size,
    };

    use super::*;

    const WHITE: [u8; 3] = [0xff; 3];
    const RUN: Rect = [100, 110, 160, 124];

    fn bounds([l, t, r, b]: Rect) -> Bounds<ScaledPixels> {
        let px = |v: i32| ScaledPixels(v as f32);
        Bounds {
            origin: point(px(l), px(t)),
            size: size(px(r - l), px(b - t)),
        }
    }

    fn quad(rect: Rect, color: Rgba) -> Quad {
        Quad {
            order: 0,
            border_style: BorderStyle::Solid,
            bounds: bounds(rect),
            content_mask: ContentMask {
                bounds: bounds([0, 0, 2000, 2000]),
            },
            background: color.into(),
            border_color: gpui_kit::transparent_black(),
            corner_radii: Corners::default(),
            border_widths: Edges::default(),
        }
    }

    fn read(quads: impl IntoIterator<Item = Quad>) -> Backdrop {
        let mut scene = Scene::default();
        for quad in quads {
            scene.insert_primitive(quad);
        }
        let mut backdrop = Backdrop::default();
        backdrop.read(&scene);
        backdrop
    }

    fn page() -> Quad {
        quad([0, 0, 1000, 800], rgb(0xffffff))
    }

    #[test]
    fn the_topmost_opaque_quad_is_the_background() {
        let backdrop = read([page(), quad([90, 100, 400, 130], rgb(0x2b66a2))]);
        assert_eq!(backdrop.regions(RUN), Some(vec![(RUN, [0x2b, 0x66, 0xa2])]));
        let below = [100, 500, 160, 520];
        assert_eq!(backdrop.regions(below), Some(vec![(below, WHITE)]));
    }

    #[test]
    fn nothing_opaque_behind_a_part_is_no_background() {
        assert_eq!(read([]).regions(RUN), None);
        let half = quad([0, 0, 130, 800], rgb(0xffffff));
        assert_eq!(read([half]).regions(RUN), None);
        let faint = quad([0, 0, 1000, 800], rgba(0xffffff80));
        assert_eq!(read([faint]).regions(RUN), None);
    }

    #[test]
    fn translucent_quads_blend_onto_what_they_cover() {
        let backdrop = read([page(), quad([0, 100, 1000, 130], rgba(0x00000080))]);
        assert_eq!(backdrop.regions(RUN), Some(vec![(RUN, [127; 3])]));
    }

    #[test]
    fn a_run_over_several_colours_has_a_region_of_each() {
        let backdrop = read([page(), quad([120, 100, 140, 130], rgb(0x3399ff))]);
        assert_eq!(
            backdrop.regions(RUN),
            Some(vec![
                ([100, 110, 120, 124], WHITE),
                ([120, 110, 140, 124], [0x33, 0x99, 0xff]),
                ([140, 110, 160, 124], WHITE),
            ])
        );
        let backdrop = read([page(), quad([0, 120, 1000, 130], rgb(0))]);
        assert_eq!(
            backdrop.regions(RUN),
            Some(vec![
                ([100, 110, 160, 120], WHITE),
                ([100, 120, 160, 124], [0; 3]),
            ])
        );
    }

    #[test]
    fn rows_cut_alike_are_one_region() {
        let backdrop = read([
            page(),
            quad([0, 0, 1000, 118], rgb(0xeeeeee)),
            quad([0, 118, 1000, 800], rgb(0xeeeeee)),
        ]);
        assert_eq!(backdrop.regions(RUN), Some(vec![(RUN, [0xee; 3])]));
    }

    #[test]
    fn gradients_shadows_and_rounded_corners_are_unknown() {
        let mut gradient = quad([90, 100, 400, 130], rgb(0));
        let stop = |c, at| linear_color_stop(rgb(c), at);
        gradient.background = linear_gradient(90.0, stop(0, 0.0), stop(0xffffff, 1.0));
        assert_eq!(read([page(), gradient]).regions(RUN), None);

        let mut rounded = quad([155, 100, 400, 130], rgb(0));
        rounded.corner_radii.bottom_left = ScaledPixels(10.0);
        assert_eq!(read([page(), rounded]).regions(RUN), None);
        rounded.bounds = bounds([90, 100, 400, 140]);
        assert_eq!(
            read([page(), rounded]).regions(RUN),
            Some(vec![(RUN, [0; 3])])
        );

        let mut scene = Scene::default();
        scene.insert_primitive(page());
        scene.insert_primitive(Shadow {
            order: 0,
            blur_radius: ScaledPixels(4.0),
            bounds: bounds([170, 100, 300, 200]),
            corner_radii: Corners::default(),
            content_mask: ContentMask {
                bounds: bounds([0, 0, 2000, 2000]),
            },
            color: gpui_kit::black(),
            element_bounds: bounds([170, 100, 300, 200]),
            element_corner_radii: Corners::default(),
            inset: 0,
            pad: 0,
        });
        let mut backdrop = Backdrop::default();
        backdrop.read(&scene);
        assert_eq!(backdrop.regions(RUN), None);
    }

    #[test]
    fn borders_are_their_own_colour() {
        let mut bordered = quad([90, 100, 400, 122], rgb(0xdddddd));
        bordered.border_color = rgb(0x888888).into();
        bordered.border_widths.bottom = ScaledPixels(2.0);
        assert_eq!(
            read([page(), bordered]).regions(RUN),
            Some(vec![
                ([100, 110, 160, 120], [0xdd; 3]),
                ([100, 120, 160, 122], [0x88; 3]),
                ([100, 122, 160, 124], WHITE),
            ])
        );
    }

    #[test]
    fn a_clipped_quad_ends_at_its_clip() {
        let mut clipped = quad([0, 0, 1000, 800], rgb(0));
        clipped.content_mask.bounds = bounds([0, 0, 1000, 112]);
        assert_eq!(
            read([page(), clipped]).regions(RUN),
            Some(vec![
                ([100, 110, 160, 112], [0; 3]),
                ([100, 112, 160, 124], WHITE),
            ])
        );
    }

    #[test]
    fn a_run_across_bands_sees_each_quad_once_in_order() {
        let run = [10, 50, 60, 140];
        let backdrop = read([
            page(),
            quad([0, 0, 1000, 800], rgba(0x00000080)),
            quad([0, 0, 1000, 800], rgba(0xffffff80)),
        ]);
        assert_eq!(backdrop.regions(run), Some(vec![(run, [191; 3])]));
        assert_eq!(backdrop.behind(run).len(), 3);
    }

    #[test]
    fn skipped_primitives_are_not_behind_anything() {
        let mut scene = Scene::default();
        let mut backdrop = Backdrop::default();
        scene.insert_primitive(page());
        let start = scene.len();
        scene.insert_primitive(quad([0, 0, 1000, 800], rgb(0)));
        backdrop.skip(start..scene.len());
        scene.insert_primitive(quad([0, 0, 110, 800], rgb(0xff0000)));
        backdrop.read(&scene);
        let regions = backdrop.regions(RUN);
        assert_eq!(
            regions,
            Some(vec![
                ([100, 110, 110, 124], [0xff, 0, 0]),
                ([110, 110, 160, 124], WHITE),
            ])
        );
        backdrop.clear();
        backdrop.read(&Scene::default());
        assert_eq!(backdrop.regions(RUN), None);
    }

    #[test]
    fn an_image_known_to_be_flat_is_its_colour() {
        let mut scene = Scene::default();
        let mut backdrop = Backdrop::default();
        scene.insert_primitive(quad([0, 0, 1000, 800], rgb(0)));
        backdrop.fill(0..scene.len(), [90, 100, 400, 130], rgb(0xffffff));
        scene.insert_primitive(quad([100, 110, 110, 124], rgb(0xff0000)));
        backdrop.read(&scene);
        assert_eq!(
            backdrop.regions(RUN),
            Some(vec![
                ([100, 110, 110, 124], [0xff, 0, 0]),
                ([110, 110, 160, 124], WHITE),
            ])
        );
    }
}
