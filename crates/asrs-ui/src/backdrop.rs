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

/// The primitives of one window's frame, indexed by the rows they touch.
#[derive(Default)]
pub struct Backdrop {
    seen: usize,
    skipped: Vec<Range<usize>>,
    items: Vec<Item>,
    bands: Vec<Vec<u32>>,
}

impl Backdrop {
    pub fn clear(&mut self) {
        self.seen = 0;
        self.skipped.clear();
        self.items.clear();
        self.bands.iter_mut().for_each(Vec::clear);
    }

    /// Takes in what `scene` painted since the last call.
    pub fn read(&mut self, scene: &Scene) {
        for range in std::mem::take(&mut self.skipped) {
            self.take(scene, self.seen..range.start);
            self.seen = range.end;
        }
        self.take(scene, self.seen..scene.len());
        self.seen = scene.len();
    }

    /// Leaves out what the scene painted in `range`.
    pub fn skip(&mut self, range: Range<usize>) {
        self.skipped.push(range);
    }

    fn take(&mut self, scene: &Scene, range: Range<usize>) {
        for primitive in scene.primitives(range) {
            match primitive {
                Primitive::Quad(quad) => self.quad(quad),
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
        let Some(rect) = intersect(rect, outer(clip)) else {
            return;
        };
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
