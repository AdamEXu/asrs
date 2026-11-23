//! Static UI text, laid out and painted the way Blink does through `asrs_canvas::text::dom`.
//!
//! Each painted fragment becomes a cached glyph-run image keyed by font, string, colour, the
//! origin's sub-pixel position and the scale, so a frame that repaints known text only blits.
//! Each window keeps its own cache, evicted least recently used first past a byte or entry
//! budget, and evicted images leave that window's sprite atlas on its next text paint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::text::dom::{self, Layout, Lu};
use asrs_canvas::text::font::FontKey;
use asrs_canvas::text::lut::MaskGammaLut;
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use gpui_kit::{
    App, AvailableSpace, Bounds, ContentMask, Corners, Element, ElementId, Global, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Refineable, RenderImage, Rgba, SharedString,
    Style, StyleRefinement, Styled, Window, WindowId, fill, point, px, size,
};
use image::{Frame, RgbaImage};

pub use asrs_canvas::text::FontSpec;
pub use asrs_canvas::text::dom::{
    Align, Direction, ELLIPSIS, LineHeight, Overflow, Style as TextStyle, WhiteSpace,
};

/// The default budget of one window's glyph-run cache.
pub const CACHE_BYTES: usize = 32 << 20;
pub const CACHE_ENTRIES: usize = 4096;

#[derive(Clone, PartialEq, Eq, Hash)]
struct RunKey {
    font: FontKey,
    text: String,
    color: [u8; 3],
    fraction: Lu,
    scale: u32,
}

struct Run {
    image: Option<(i32, i32, Arc<RenderImage>)>,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct RunCache {
    runs: HashMap<RunKey, Run>,
    bytes: usize,
    evicted: Vec<Arc<RenderImage>>,
}

/// Counts of one window's glyph-run cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub rasterized: u64,
    pub evicted: u64,
}

pub struct TextSystem {
    backend: Box<dyn TextBackend>,
    lut: MaskGammaLut,
    windows: HashMap<WindowId, (RunCache, CacheStats)>,
    clock: u64,
    budget: (usize, usize),
}

impl Default for TextSystem {
    fn default() -> Self {
        TextSystem {
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
            lut: MaskGammaLut::embedded().unwrap_or_else(MaskGammaLut::identity),
            windows: HashMap::new(),
            clock: 0,
            budget: (CACHE_BYTES, CACHE_ENTRIES),
        }
    }
}

impl Global for TextSystem {}

impl TextSystem {
    /// Changes the per-window cache budget, for tests that exercise eviction.
    pub fn set_budget(cx: &mut App, bytes: usize, entries: usize) {
        cx.default_global::<TextSystem>().budget = (bytes, entries);
    }

    pub fn stats(cx: &mut App, window: WindowId) -> CacheStats {
        let system = cx.default_global::<TextSystem>();
        system
            .windows
            .get(&window)
            .map_or_else(CacheStats::default, |(cache, stats)| CacheStats {
                entries: cache.runs.len(),
                bytes: cache.bytes,
                ..*stats
            })
    }

    fn evict(&mut self, window: WindowId) {
        let (bytes, entries) = self.budget;
        let Some((cache, stats)) = self.windows.get_mut(&window) else {
            return;
        };
        if cache.bytes <= bytes && cache.runs.len() <= entries {
            return;
        }
        let mut order: Vec<(u64, RunKey)> = cache
            .runs
            .iter()
            .map(|(k, r)| (r.used, k.clone()))
            .collect();
        order.sort_unstable_by_key(|(used, _)| *used);
        let (keep_bytes, keep_entries) = (bytes / 4 * 3, entries / 4 * 3);
        for (_, key) in order {
            if cache.bytes <= keep_bytes && cache.runs.len() <= keep_entries {
                break;
            }
            if let Some(run) = cache.runs.remove(&key) {
                cache.bytes -= run.bytes;
                stats.evicted += 1;
                cache.evicted.extend(run.image.map(|(_, _, image)| image));
            }
        }
    }

    fn run(
        &mut self,
        window: WindowId,
        font: &FontSpec,
        fragment: &dom::Fragment,
        fraction: Lu,
        scale: f32,
        color: [u8; 3],
    ) -> Option<(i32, i32, Arc<RenderImage>)> {
        self.clock += 1;
        let clock = self.clock;
        let key = RunKey {
            font: font.key(),
            text: fragment.text.clone(),
            color,
            fraction,
            scale: scale.to_bits(),
        };
        let (cache, stats) = self.windows.entry(window).or_default();
        if let Some(run) = cache.runs.get_mut(&key) {
            run.used = clock;
            return run.image.clone();
        }
        let image = dom::rasterize(
            self.backend.as_mut(),
            &self.lut,
            &fragment.glyphs,
            fraction,
            scale,
            color,
        )
        .and_then(|run| {
            let mut bgra = Vec::with_capacity(run.alpha.len() * 4);
            match &run.rgba {
                Some(rgba) => {
                    for p in rgba.as_chunks::<4>().0 {
                        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                    }
                }
                None => {
                    for &a in &run.alpha {
                        bgra.extend_from_slice(&[color[2], color[1], color[0], a]);
                    }
                }
            }
            let buffer = RgbaImage::from_raw(run.width, run.height, bgra)?;
            Some((
                run.left,
                run.top,
                Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
            ))
        });
        let bytes = image.as_ref().map_or(0, |(_, _, i)| {
            let s = i.size(0);
            s.width.0 as usize * s.height.0 as usize * 4
        }) + fragment.text.len()
            + 64;
        stats.rasterized += 1;
        cache.bytes += bytes;
        cache.runs.insert(
            key,
            Run {
                image: image.clone(),
                bytes,
                used: clock,
            },
        );
        self.evict(window);
        image
    }
}

/// Lays `text` out at device scale `scale`, in a content box `available` CSS px wide or at its
/// max-content width.
pub fn layout(
    cx: &mut App,
    style: &TextStyle,
    text: &str,
    scale: f32,
    available: Option<f32>,
) -> Layout {
    layout_lu(
        cx,
        style,
        text,
        scale,
        available.map(|w| dom::lu_round(w * scale)),
    )
}

fn layout_lu(
    cx: &mut App,
    style: &TextStyle,
    text: &str,
    scale: f32,
    available: Option<Lu>,
) -> Layout {
    let system = cx.default_global::<TextSystem>();
    dom::layout(system.backend.as_mut(), style, text, available, scale)
}

fn snap(lu: Lu, scale: f32) -> Pixels {
    px(((f64::from(lu) / 64.0 + 0.5).floor() / f64::from(scale)) as f32)
}

/// The exact width behind `width` if it is one the measure function returned, else `width`.
fn exact_width(measured: &RefCell<Vec<(Pixels, Lu)>>, width: Pixels, scale: f32) -> Lu {
    measured
        .borrow()
        .iter()
        .rev()
        .find(|(snapped, _)| *snapped == width)
        .map_or_else(
            || dom::lu_round(f32::from(width) * scale),
            |&(_, exact)| exact,
        )
}

/// `(ascent, descent)` of `style`'s font at device scale `scale`, in whole device px: what
/// Blink's `FontMetrics::Height()` adds up.
pub fn font_metrics(cx: &mut App, style: &TextStyle, scale: f32) -> (f32, f32) {
    let system = cx.default_global::<TextSystem>();
    let (ascent, descent, _) = dom::line_metrics(system.backend.as_mut(), style, scale);
    (ascent as f32 / 64.0, descent as f32 / 64.0)
}

/// One glyph run's coverage, after mask gamma, at a whole device pixel position.
pub struct GlyphMask {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

/// The coverage [`Text`] would paint for `text` in a box `width` CSS px wide whose content
/// box starts at `origin` device px, for callers that composite text into their own pixmap.
pub fn glyph_masks(
    cx: &mut App,
    style: &TextStyle,
    text: &str,
    scale: f32,
    width: f32,
    origin: (f32, f32),
    color: u32,
) -> Vec<GlyphMask> {
    let color = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
    let layout = layout(cx, style, text, scale, Some(width));
    let origin = (dom::lu_round(origin.0), dom::lu_round(origin.1));
    let system = cx.default_global::<TextSystem>();
    let mut out = Vec::new();
    for line in &layout.lines {
        let baseline = (origin.1 + line.baseline + 32).div_euclid(64);
        for fragment in &line.fragments {
            let x = origin.0 + fragment.x;
            if let Some(run) = dom::rasterize(
                system.backend.as_mut(),
                &system.lut,
                &fragment.glyphs,
                x.rem_euclid(64),
                scale,
                color,
            ) {
                out.push(GlyphMask {
                    x: x.div_euclid(64) + run.left,
                    y: baseline + run.top,
                    width: run.width,
                    height: run.height,
                    alpha: run.alpha,
                });
            }
        }
    }
    out
}

/// The border-box size Chromium gives a shrink-to-fit block holding `text`, in CSS px.
pub fn measure(cx: &mut App, style: &TextStyle, text: &str, scale: f32) -> (f64, f64) {
    let l = layout(cx, style, text, scale, None);
    (l.width_px(), l.height_px())
}

/// Measures text outside a render, with its own backend: for layout reads a model makes
/// while no `App` is at hand.
pub struct Measurer(Box<dyn TextBackend>);

impl Default for Measurer {
    fn default() -> Self {
        Measurer(platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)))
    }
}

impl Measurer {
    /// [`measure`]'s border-box size.
    pub fn measure(&mut self, style: &TextStyle, text: &str, scale: f32) -> (f64, f64) {
        let l = dom::layout(self.0.as_mut(), style, text, None, scale);
        (l.width_px(), l.height_px())
    }
}

/// `v` less its value snapped to the device pixel grid the way GPUI snaps layout: what a
/// fractional CSS position loses to snapping, for [`Text::subpixel`].
pub fn snap_remainder(v: f32, scale: f32) -> f32 {
    v - (v * scale + 0.5).floor() / scale
}

/// A block box of text, sized like Chromium's shrink-to-fit box unless styled with a width.
pub struct Text {
    text: SharedString,
    style: TextStyle,
    color: [u8; 3],
    subpixel: (f32, f32),
    probe: Option<SharedString>,
    form_control: bool,
    content_width: Option<f64>,
    underline: bool,
    gpui: StyleRefinement,
    /// Each width the measure function returned, snapped as GPUI will snap the box, with the
    /// exact width it stands for: lines wrap and align at the exact width, as in Chromium.
    measured: Rc<RefCell<Vec<(Pixels, Lu)>>>,
}

impl Text {
    pub fn new(text: impl Into<SharedString>, style: TextStyle) -> Self {
        Text {
            text: text.into(),
            style,
            color: [0x22; 3],
            subpixel: (0.0, 0.0),
            probe: None,
            form_control: false,
            content_width: None,
            underline: false,
            gpui: StyleRefinement::default(),
            measured: Rc::default(),
        }
    }

    pub fn color(mut self, rgb: u32) -> Self {
        self.color = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
        self
    }

    /// The part of the box's CSS position, in CSS px, that GPUI's pixel snapping dropped; text
    /// is painted from the unsnapped position, as Chromium paints it.
    pub fn subpixel(mut self, dx: f32, dy: f32) -> Self {
        self.subpixel = (dx, dy);
        self
    }

    /// The content box's exact CSS width, for a box sized by its container: without it a width
    /// that snaps to the text's own counts as the text's, and an ellipsis is missed.
    pub fn content_width(mut self, width: f64) -> Self {
        self.content_width = Some(width);
        self.w(px(width as f32))
    }

    /// Text a form control draws (a value or placeholder): Chromium keeps it in the control's
    /// shadow tree, so the DOM dump has no text node for it and parity captures leave it out.
    pub fn form_control(mut self) -> Self {
        self.form_control = true;
        self
    }

    /// `text-decoration: underline`, as on links.
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Names the box in parity captures.
    pub fn probe(mut self, id: impl Into<SharedString>) -> Self {
        self.probe = Some(id.into());
        self
    }
}

impl Styled for Text {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.gpui
    }
}

impl IntoElement for Text {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

pub struct Prepaint {
    layout: Layout,
    origin: (Lu, Lu),
}

impl Element for Text {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.gpui);
        let text = self.text.clone();
        let text_style = self.style;
        let wraps = text_style.white_space == WhiteSpace::Normal;
        let measured = self.measured.clone();
        measured.borrow_mut().clear();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let scale = window.scale_factor();
            let wrap_at = match (known.width, available.width) {
                (Some(w), _) => Some(exact_width(&measured, w, scale)),
                (None, AvailableSpace::Definite(w)) if wraps => {
                    Some(exact_width(&measured, w, scale))
                }
                (None, AvailableSpace::MinContent) if wraps => Some(0),
                _ => None,
            };
            let l = layout_lu(cx, &text_style, &text, scale, wrap_at);
            let width = match (known.width, wrap_at) {
                (Some(_), Some(at)) => at,
                (None, Some(at)) if l.lines.len() > 1 => at.max(l.width),
                _ => l.width,
            };
            let snapped = snap(width, scale);
            measured.borrow_mut().push((snapped, width));
            size(snapped, snap(l.height, scale))
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let scale = window.scale_factor();
        let available = match self.content_width {
            Some(width) => dom::lu_round((width * f64::from(scale)) as f32),
            None => exact_width(&self.measured, bounds.size.width, scale),
        };
        let layout = layout_lu(cx, &self.style, &self.text, scale, Some(available));
        let origin = (
            dom::lu_round((f32::from(bounds.origin.x) + self.subpixel.0) * scale),
            dom::lu_round((f32::from(bounds.origin.y) + self.subpixel.1) * scale),
        );
        Prepaint { layout, origin }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale = window.scale_factor();
        let window_id = window.window_handle().window_id();
        let system = cx.default_global::<TextSystem>();
        if let Some((cache, _)) = system.windows.get_mut(&window_id) {
            for image in cache.evicted.drain(..) {
                let _ = window.drop_image(image);
            }
        }
        let Prepaint { layout, origin } = prepaint;
        let mut images = Vec::new();
        let mut underlines = Vec::new();
        for line in &layout.lines {
            let baseline = (origin.1 + line.baseline + 32).div_euclid(64);
            let first = images.len();
            for fragment in &line.fragments {
                let x = origin.0 + fragment.x;
                if let Some((left, top, image)) = system.run(
                    window_id,
                    &self.style.font,
                    fragment,
                    x.rem_euclid(64),
                    scale,
                    self.color,
                ) {
                    images.push((x.div_euclid(64) + left, baseline + top, image));
                }
            }
            if self.underline
                && let (Some(start), Some(end)) = (line.fragments.first(), line.fragments.last())
            {
                let left = (origin.0 + start.x) as f32 / 64.0;
                let right = (origin.0 + end.x + end.width) as f32 / 64.0;
                let size = self.style.font.size * scale;
                let inked = &images[first..];
                underlines.extend(underline::segments(left, right, baseline, size, inked));
            }
        }
        let clip = (self.style.overflow != Overflow::Visible).then_some(ContentMask { bounds });
        window.with_content_mask(clip, |window| {
            let to_px = |v: i32| px(v as f32 / scale);
            let [r, g, b] = self.color.map(|c| f32::from(c) / 255.0);
            for s in underlines {
                let rect = Bounds {
                    origin: point(to_px(s.x), to_px(s.y)),
                    size: size(to_px(s.width), to_px(s.height)),
                };
                let color = Rgba {
                    r,
                    g,
                    b,
                    a: s.coverage,
                };
                window.paint_quad(fill(rect, color));
            }
            for (x, y, image) in images {
                let s = image.size(0);
                let rect = Bounds {
                    origin: point(to_px(x), to_px(y)),
                    size: size(to_px(s.width.0), to_px(s.height.0)),
                };
                let _ = window.paint_image(rect, rect, Corners::default(), image, 0, false);
            }
        });
        #[cfg(feature = "parity")]
        {
            if let Some(id) = &self.probe {
                probe::record(cx, id, bounds, layout, *origin);
            }
            if !self.form_control {
                crate::parity::record_text(cx, probe::text_run(layout, *origin));
            }
        }
    }
}

/// Blink's underline (`TextDecorationInfo`, `TextPainter::ClipDecorationsStripe`) with the auto
/// offset, thickness and `text-decoration-skip-ink`, in device px.
mod underline {
    use std::sync::Arc;

    use gpui_kit::RenderImage;

    #[derive(Debug, PartialEq)]
    pub struct Segment {
        pub x: i32,
        pub y: i32,
        pub width: i32,
        pub height: i32,
        pub coverage: f32,
    }

    /// The horizontal extents of the ink in rows `top..bottom` of the glyph runs.
    fn intercepts(top: i32, bottom: i32, runs: &[(i32, i32, Arc<RenderImage>)]) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        for (x, y, image) in runs {
            let size = image.size(0);
            let (w, h) = (size.width.0, size.height.0);
            let Some(bytes) = image.as_bytes(0) else {
                continue;
            };
            let rows = (top - y).max(0)..(bottom - y).min(h);
            let column = |col: i32| {
                rows.clone()
                    .map(|row| bytes[((row * w + col) * 4 + 3) as usize])
                    .max()
                    .unwrap_or(0)
            };
            let inked: Vec<u8> = (0..w).map(column).collect();
            let mut col = 0;
            while col < inked.len() {
                if inked[col] == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < inked.len() && inked[col] != 0 {
                    col += 1;
                }
                let part = |c: usize| f32::from(inked[c]) / 255.0;
                out.push((
                    (x + start as i32 + 1) as f32 - part(start),
                    (x + col as i32 - 1) as f32 + part(col - 1),
                ));
            }
        }
        out
    }

    /// The underline of a line from `left` to `right` whose font is `size` device px, with gaps
    /// as wide as the stroke is thick around the ink it would touch.
    pub fn segments(
        left: f32,
        right: f32,
        baseline: i32,
        size: f32,
        runs: &[(i32, i32, Arc<RenderImage>)],
    ) -> Vec<Segment> {
        let auto = (size / 10.0).max(1.0);
        let height = auto.floor() as i32;
        let y = baseline + (height as f32 / 2.0).ceil() as i32;
        let gaps = intercepts(y, y + height, runs);
        let skipped = |x: i32| {
            let centre = x as f32 + 0.5;
            gaps.iter()
                .any(|(a, b)| centre >= a - auto && centre < b + auto)
        };
        let mut out: Vec<Segment> = Vec::new();
        for x in left.floor() as i32..right.ceil() as i32 {
            if skipped(x) {
                continue;
            }
            let coverage = (right.min((x + 1) as f32) - left.max(x as f32)).min(1.0);
            match out.last_mut() {
                Some(last) if last.x + last.width == x && last.coverage == coverage => {
                    last.width += 1;
                }
                _ => out.push(Segment {
                    x,
                    y,
                    width: 1,
                    height,
                    coverage,
                }),
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fractional_ends_are_partly_covered() {
            let line = segments(180.25, 213.75, 172, 16.0, &[]);
            let spans: Vec<_> = line.iter().map(|s| (s.x, s.width, s.coverage)).collect();
            assert_eq!(spans, [(180, 1, 0.75), (181, 32, 1.0), (213, 1, 0.75)]);
            assert!(line.iter().all(|s| (s.y, s.height) == (173, 1)));
            let line = segments(360.0, 427.0, 344, 32.0, &[]);
            assert_eq!((line[0].y, line[0].height), (346, 3));
        }
    }
}

#[cfg(feature = "parity")]
pub mod probe {
    use super::*;

    /// What a [`Text`] painted, in CSS px: its box, its max-content size and each fragment's
    /// text box (`[x, y, width, height]`, the ellipsis last on its line when present).
    #[derive(Clone, Debug, PartialEq)]
    pub struct Record {
        pub id: SharedString,
        pub bounds: [f32; 4],
        pub width: f64,
        pub height: f64,
        pub fragments: Vec<(String, [f64; 4])>,
    }

    #[derive(Default)]
    pub struct Probe(pub Vec<Record>);

    fn fragments(layout: &Layout, origin: (Lu, Lu)) -> Vec<(String, [f64; 4])> {
        let px = |lu: Lu| f64::from(lu) / 64.0 / f64::from(layout.scale);
        let mut fragments = Vec::new();
        for line in &layout.lines {
            let top = origin.1 + line.baseline - layout.ascent;
            for f in &line.fragments {
                let rect = [
                    px(origin.0 + f.x),
                    px(top),
                    px(f.width),
                    px(layout.ascent + layout.descent),
                ];
                fragments.push((f.text.clone(), rect));
            }
        }
        fragments
    }

    /// The text and the boxes `Range.getClientRects()` gives for it: one per line, the union of
    /// the line's fragments, and for a line an ellipsis cut, the whole text's and then that of
    /// what the ellipsis left.
    pub(super) fn text_run(layout: &Layout, origin: (Lu, Lu)) -> crate::parity::TextRun {
        let px = |lu: Lu| f64::from(lu) / 64.0 / f64::from(layout.scale);
        let mut text = String::new();
        let mut rects = Vec::new();
        for line in &layout.lines {
            let top = origin.1 + line.baseline - layout.ascent;
            let rect = |from: Lu, to: Lu| {
                [
                    px(origin.0 + from),
                    px(top),
                    px(to - from),
                    px(layout.ascent + layout.descent),
                ]
            };
            if let Some(at) = line.cut_at {
                rects.push(rect(at, at + line.width));
            }
            let kept = line
                .fragments
                .iter()
                .filter(|f| line.cut_at.is_none() || f.text != ELLIPSIS);
            let mut extent: Option<(Lu, Lu)> = None;
            for f in kept {
                text.push_str(&f.text);
                let (from, to) = extent.unwrap_or((f.x, f.x + f.width));
                extent = Some((from.min(f.x), to.max(f.x + f.width)));
            }
            if let Some((from, to)) = extent {
                rects.push(rect(from, to));
            }
        }
        crate::parity::TextRun { text, rects }
    }

    impl Global for Probe {}

    pub(super) fn record(
        cx: &mut App,
        id: &SharedString,
        bounds: Bounds<Pixels>,
        layout: &Layout,
        origin: (Lu, Lu),
    ) {
        if !cx.has_global::<Probe>() {
            return;
        }
        let fragments = fragments(layout, origin);
        let record = Record {
            id: id.clone(),
            bounds: [
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ],
            width: layout.width_px(),
            height: layout.height_px(),
            fragments,
        };
        cx.global_mut::<Probe>().0.push(record);
    }
}
