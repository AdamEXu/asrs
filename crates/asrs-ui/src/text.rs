//! Static UI text, laid out and painted the way Blink does through `asrs_canvas::text::dom`.
//!
//! Each painted fragment becomes a cached glyph-run image keyed by font, string, colour, the
//! origin's sub-pixel position and the scale, so a frame that repaints known text only blits.
//! Each window keeps its own cache, evicted least recently used first past a byte or entry
//! budget, and evicted images leave that window's sprite atlas on its next text paint.
//!
//! Where Chromium draws LCD text (Windows, and Linux under fontconfig rules that ask for it), a run
//! is blended per channel onto the colours the frame painted behind it, which its key then holds
//! too. It stays grayscale in a window that is not opaque, under CSS opacity, behind an image or a
//! gradient, where [`Text::grayscale`] says so, and where the backend has no LCD masks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::text::caret;
use asrs_canvas::text::dom::{self, Layout, LcdExtent, Lu};
use asrs_canvas::text::font::FontKey;
use asrs_canvas::text::lut::{LcdGammaLut, MaskGammaLut};
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use gpui_kit::{
    App, AvailableSpace, Bounds, ContentMask, Corners, Element, ElementId, Global, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Refineable, RenderImage, Rgba, Scene,
    SharedString, Style, StyleRefinement, Styled, Window, WindowId, fill, point, px, size,
};
use image::{Frame, RgbaImage};

use crate::backdrop::{self, Backdrop, Rect};
use crate::selection::colors::SelectionColors;

pub use asrs_canvas::text::FontSpec;
pub use asrs_canvas::text::dom::{
    Align, Direction, ELLIPSIS, LineHeight, Overflow, Style as TextStyle, WhiteSpace,
};

/// The default budget of one window's glyph-run cache.
pub const CACHE_BYTES: usize = 32 << 20;
pub const CACHE_ENTRIES: usize = 4096;

/// How many backgrounds one run is kept blended onto: a background that animates makes a new image
/// every frame.
const LCD_IMAGES: usize = 4;

#[derive(Clone, PartialEq, Eq, Hash)]
struct RunKey {
    font: FontKey,
    text: String,
    color: [u8; 3],
    fraction: Lu,
    scale: u32,
}

/// What an LCD image holds besides its glyphs.
#[derive(Clone, Copy, PartialEq, Eq)]
struct LcdKey {
    background: [u8; 3],
    alpha: u8,
}

struct LcdImage {
    key: LcdKey,
    image: Arc<RenderImage>,
    used: u64,
}

type Placed = (i32, i32, Arc<RenderImage>);

struct Run {
    /// `Some(None)` for a run with no ink.
    image: Option<Option<Placed>>,
    lcd_extent: Option<LcdExtent>,
    lcd: Vec<LcdImage>,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct RunCache {
    runs: HashMap<RunKey, Run>,
    bytes: usize,
    evicted: Vec<Arc<RenderImage>>,
}

fn image_bytes(image: &RenderImage) -> usize {
    let size = image.size(0);
    size.width.0 as usize * size.height.0 as usize * 4
}

/// Counts of one window's glyph-run cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub rasterized: u64,
    pub evicted: u64,
}

#[derive(Default)]
struct WindowText {
    cache: RunCache,
    stats: CacheStats,
    backdrop: Backdrop,
    grayscale: bool,
}

pub struct TextSystem {
    backend: Box<dyn TextBackend>,
    lut: MaskGammaLut,
    lcd_lut: Option<LcdGammaLut>,
    windows: HashMap<WindowId, WindowText>,
    clock: u64,
    budget: (usize, usize),
}

impl Default for TextSystem {
    fn default() -> Self {
        TextSystem {
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
            lut: MaskGammaLut::embedded().unwrap_or_else(MaskGammaLut::identity),
            lcd_lut: LcdGammaLut::embedded(),
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
            .map_or_else(CacheStats::default, |window| CacheStats {
                entries: window.cache.runs.len(),
                bytes: window.cache.bytes,
                ..window.stats
            })
    }

    /// Draws all text of `window` in grayscale: Chromium decides per layer, and a page that is not
    /// opaque is one layer that is not.
    pub fn set_grayscale(cx: &mut App, window: WindowId) {
        let system = cx.default_global::<TextSystem>();
        system.windows.entry(window).or_default().grayscale = true;
    }

    fn evict(&mut self, window: WindowId) {
        let (bytes, entries) = self.budget;
        let Some(WindowText { cache, stats, .. }) = self.windows.get_mut(&window) else {
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
                cache
                    .evicted
                    .extend(run.image.flatten().map(|(_, _, image)| image));
                cache
                    .evicted
                    .extend(run.lcd.into_iter().map(|lcd| lcd.image));
            }
        }
    }

    /// Adds what draws `fragment` from `x` and `baseline` to `out`, and its grayscale image to
    /// `inked`.
    fn draw(
        &mut self,
        window: WindowId,
        fragment: &dom::Fragment,
        ink: &Ink<'_>,
        (x, baseline): (Lu, i32),
        out: &mut Vec<Drawn>,
        inked: Option<&mut Vec<Placed>>,
    ) {
        self.clock += 1;
        let Ink { font, scale, .. } = *ink;
        let fraction = x.rem_euclid(64);
        let x = x.div_euclid(64);
        let key = RunKey {
            font: font.key(),
            text: fragment.text.clone(),
            color: ink.color,
            fraction,
            scale: scale.to_bits(),
        };
        let backend = self.backend.as_mut();
        let WindowText {
            cache,
            stats,
            backdrop,
            ..
        } = self.windows.entry(window).or_default();
        let run = cache.runs.entry(key).or_insert_with(|| {
            let bytes = fragment.text.len() + 64;
            cache.bytes += bytes;
            Run {
                image: None,
                lcd_extent: None,
                lcd: Vec::new(),
                bytes,
                used: 0,
            }
        });
        run.used = self.clock;
        let before = run.bytes;

        let mut regions = None;
        if let (Some(scene), Some(_)) = (ink.lcd, &self.lcd_lut) {
            let extent = *run
                .lcd_extent
                .get_or_insert_with(|| dom::lcd_extent(backend, &fragment.glyphs, fraction, scale));
            match extent {
                LcdExtent::Blank => regions = Some(Vec::new()),
                LcdExtent::Grayscale => {}
                LcdExtent::Rect([left, top, width, height]) => {
                    let (left, top) = (x + left, baseline + top);
                    let extent = [left, top, left + width, top + height];
                    backdrop.read(scene);
                    match backdrop::intersect(extent, ink.clip) {
                        Some(visible) => regions = backdrop.regions(visible),
                        None => regions = Some(Vec::new()),
                    }
                }
            }
        }

        let mut run_bytes = 0;
        if regions.is_none() || inked.is_some() {
            let image = run.image.get_or_insert_with(|| {
                stats.rasterized += 1;
                let run = dom::rasterize(
                    backend,
                    &self.lut,
                    &fragment.glyphs,
                    fraction,
                    scale,
                    ink.color,
                );
                let placed = run.and_then(|run| gray_image(run, ink.color));
                run_bytes += placed
                    .as_ref()
                    .map_or(0, |(_, _, image)| image_bytes(image));
                placed
            });
            if let Some((left, top, image)) = image {
                let placed = (x + *left, baseline + *top, image.clone());
                if let Some(inked) = inked {
                    inked.push(placed.clone());
                }
                if regions.is_none() {
                    out.push(Drawn {
                        x: placed.0,
                        y: placed.1,
                        image: placed.2,
                        clip: None,
                    });
                }
            }
        }
        run.bytes += run_bytes;
        for (clip, background) in regions.into_iter().flatten() {
            let key = LcdKey {
                background,
                alpha: ink.alpha,
            };
            let (Some(LcdExtent::Rect([left, top, ..])), Some(lcd_lut)) =
                (run.lcd_extent, &self.lcd_lut)
            else {
                break;
            };
            let at = match run.lcd.iter().position(|image| image.key == key) {
                Some(at) => at,
                None => {
                    stats.rasterized += 1;
                    let lcd_ink = dom::LcdInk {
                        color: ink.color,
                        alpha: ink.alpha,
                        background,
                    };
                    let luts = (&self.lut, lcd_lut);
                    let image = dom::rasterize_lcd(
                        backend,
                        luts,
                        &fragment.glyphs,
                        fraction,
                        scale,
                        lcd_ink,
                    );
                    let Some(image) = lcd_image(image) else {
                        break;
                    };
                    if run.lcd.len() >= LCD_IMAGES {
                        let oldest = (0..run.lcd.len()).min_by_key(|&i| run.lcd[i].used);
                        let old = run.lcd.swap_remove(oldest.expect("images")).image;
                        run.bytes -= image_bytes(&old);
                        stats.evicted += 1;
                        cache.evicted.push(old);
                    }
                    run.bytes += image_bytes(&image);
                    run.lcd.push(LcdImage {
                        key,
                        image,
                        used: 0,
                    });
                    run.lcd.len() - 1
                }
            };
            run.lcd[at].used = self.clock;
            out.push(Drawn {
                x: x + left,
                y: baseline + top,
                image: run.lcd[at].image.clone(),
                clip: Some(clip),
            });
        }
        cache.bytes = cache.bytes + run.bytes - before;
        self.evict(window);
    }
}

/// What all of a [`Text`]'s fragments are drawn with.
struct Ink<'a> {
    font: &'a FontSpec,
    scale: f32,
    color: [u8; 3],
    alpha: u8,
    /// What the frame has painted so far, if the text may be LCD.
    lcd: Option<&'a Scene>,
    clip: Rect,
}

struct Drawn {
    x: i32,
    y: i32,
    image: Arc<RenderImage>,
    /// Set for an LCD image: where it has the background it was blended onto.
    clip: Option<Rect>,
}

fn gray_image(run: dom::RunImage, color: [u8; 3]) -> Option<Placed> {
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
    let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
    Some((run.left, run.top, image))
}

fn lcd_image(mut run: dom::LcdImage) -> Option<Arc<RenderImage>> {
    for p in run.rgba.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    let buffer = RgbaImage::from_raw(run.width, run.height, run.rgba)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
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

/// The advance of a space in device px.
pub fn space_width(cx: &mut App, style: &TextStyle, scale: f32) -> f32 {
    let system = cx.default_global::<TextSystem>();
    caret::space_width(system.backend.as_mut(), &style.font, scale)
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

/// The width in CSS px of what an ellipsis leaves of `text`, one inline box of a line that has
/// `room` CSS px before the ellipsis. [`Text::cut`] draws it.
pub fn cut_width(cx: &mut App, style: &TextStyle, text: &str, scale: f32, room: f64) -> f64 {
    let mut l = layout(cx, style, text, scale, None);
    let end = dom::cut(&mut l, dom::lu_round((room * f64::from(scale)) as f32));
    f64::from(end) / 64.0 / f64::from(scale)
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
    cut: Option<f64>,
    selected: Option<Rgba>,
    alpha: f32,
    grayscale: bool,
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
            cut: None,
            selected: None,
            alpha: 1.0,
            grayscale: false,
            gpui: StyleRefinement::default(),
            measured: Rc::default(),
        }
    }

    pub fn color(mut self, rgb: u32) -> Self {
        self.color = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
        self
    }

    /// The colour's alpha, which unlike CSS opacity leaves text in LCD.
    pub fn alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }

    /// Text that Chromium puts on a layer that is not opaque, such as one behind a CSS mask.
    pub fn grayscale(mut self) -> Self {
        self.grayscale = true;
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

    /// Text whose boxes the view reports to parity captures itself, with
    /// [`record_text`](crate::parity::record_text).
    pub fn unrecorded(self) -> Self {
        self.form_control()
    }

    /// `text-decoration: underline`, as on links.
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Draws the whole text selected, the line breaks after its lines included.
    pub fn selected(mut self, colors: Option<SelectionColors>) -> Self {
        self.selected = colors.map(|colors| colors.rgba());
        match colors.and_then(|colors| colors.text) {
            Some(text) => self.color(text),
            None => self,
        }
    }

    /// Draws only what fits in `room` CSS px, for a box the line's ellipsis follows or replaces.
    pub fn cut(mut self, room: f64) -> Self {
        self.cut = Some(room);
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
        let mut layout = layout_lu(cx, &self.style, &self.text, scale, Some(available));
        if let Some(room) = self.cut {
            dom::cut(&mut layout, dom::lu_round((room * f64::from(scale)) as f32));
        }
        let window_id = window.window_handle().window_id();
        let system = cx.default_global::<TextSystem>();
        if system.lcd_lut.is_some()
            && let Some(text) = system.windows.get_mut(&window_id)
        {
            text.backdrop.clear();
        }
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
        let text = system.windows.entry(window_id).or_default();
        for image in text.cache.evicted.drain(..) {
            let _ = window.drop_image(image);
        }
        let may_be_lcd = system.lcd_lut.is_some()
            && !self.grayscale
            && !text.grayscale
            && window.element_opacity() >= 1.0;
        let Prepaint { layout, origin } = prepaint;
        let mut selection = Vec::new();
        if self.selected.is_some() {
            let space = caret::space_width(system.backend.as_mut(), &self.style.font, scale);
            let round = |lu: Lu| (lu + 32).div_euclid(64);
            for line in &layout.lines {
                if let (Some(first), Some(last)) = (line.fragments.first(), line.fragments.last()) {
                    let left = round(origin.0 + first.x);
                    let right = origin.0 + last.x + last.width + (space * 64.0) as Lu;
                    let box_top = line.baseline - layout.ascent;
                    let top = round(origin.1 + line.top.min(box_top));
                    let bottom = (line.top + line.height).max(line.baseline + layout.descent);
                    let bottom = round(origin.1 + bottom);
                    selection.push([left, top, round(right) - left, bottom - top]);
                }
            }
        }
        let to_px = |v: i32| px(v as f32 / scale);
        let bounds_of = |[x, y, w, h]: [i32; 4]| Bounds {
            origin: point(to_px(x), to_px(y)),
            size: size(to_px(w), to_px(h)),
        };
        let clip = (self.style.overflow != Overflow::Visible).then_some(ContentMask { bounds });
        let alpha = Some(self.alpha);
        window.with_content_mask(clip, |window| {
            if let Some(background) = self.selected {
                window.with_element_opacity(alpha, |window| {
                    for rect in selection {
                        window.paint_quad(fill(bounds_of(rect), background));
                    }
                });
            }
            let painted = window.painted_scene().len();
            let mask = window.content_mask().bounds;
            let edge = |v: Pixels| f32::from(v) * scale;
            let ink = Ink {
                font: &self.style.font,
                scale,
                color: self.color,
                alpha: (self.alpha * 255.0).round() as u8,
                lcd: may_be_lcd.then(|| window.painted_scene()),
                clip: [
                    edge(mask.left()).floor() as i32,
                    edge(mask.top()).floor() as i32,
                    edge(mask.right()).ceil() as i32,
                    edge(mask.bottom()).ceil() as i32,
                ],
            };
            let mut images = Vec::new();
            let mut underlines = Vec::new();
            for line in &layout.lines {
                let baseline = (origin.1 + line.baseline + 32).div_euclid(64);
                let mut inked = self.underline.then(Vec::new);
                for fragment in &line.fragments {
                    let at = (origin.0 + fragment.x, baseline);
                    system.draw(window_id, fragment, &ink, at, &mut images, inked.as_mut());
                }
                if let (Some(inked), Some(start), Some(end)) =
                    (inked, line.fragments.first(), line.fragments.last())
                {
                    let left = (origin.0 + start.x) as f32 / 64.0;
                    let right = (origin.0 + end.x + end.width) as f32 / 64.0;
                    let size = self.style.font.size * scale;
                    underlines.extend(underline::segments(left, right, baseline, size, &inked));
                }
            }
            let [r, g, b] = self.color.map(|c| f32::from(c) / 255.0);
            window.with_element_opacity(alpha, |window| {
                for s in underlines {
                    let color = Rgba {
                        r,
                        g,
                        b,
                        a: s.coverage,
                    };
                    let rect = bounds_of([s.x, s.y, s.width, s.height]);
                    window.paint_quad(fill(rect, color));
                }
            });
            for Drawn { x, y, image, clip } in images {
                let s = image.size(0);
                let rect = bounds_of([x, y, s.width.0, s.height.0]);
                let (mask, opacity) = match clip {
                    Some([l, t, r, b]) => {
                        let bounds = bounds_of([l, t, r - l, b - t]);
                        (Some(ContentMask { bounds }), None)
                    }
                    None => (None, alpha),
                };
                window.with_content_mask(mask, |window| {
                    window.with_element_opacity(opacity, |window| {
                        let _ = window.paint_image(rect, rect, Corners::default(), image, 0, false);
                    });
                });
            }
            if system.lcd_lut.is_some()
                && let Some(text) = system.windows.get_mut(&window_id)
            {
                let now = window.painted_scene().len();
                text.backdrop.skip(painted..now);
            }
        });
        #[cfg(feature = "parity")]
        {
            if let Some(id) = &self.probe {
                probe::record(cx, id, bounds, layout, *origin);
            }
            if !self.form_control {
                let mut run = probe::text_run(layout, *origin);
                if run.text.is_empty() && self.cut.is_some() {
                    run.text = self.text.to_string();
                }
                crate::parity::record_text(cx, run);
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
