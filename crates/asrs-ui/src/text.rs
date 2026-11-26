//! Static UI text, laid out and painted the way Blink does through `asrs_canvas::text::dom`.
//!
//! Each painted fragment becomes a cached glyph-run image keyed by font, string, colour, the
//! origin's sub-pixel position and the scale, so a frame that repaints known text only blits.
//! Each window keeps its own cache, evicted least recently used first past a byte or entry
//! budget, and evicted images leave that window's sprite atlas on its next text paint.
//!
//! Where Chromium draws LCD text (Windows, and Linux under fontconfig rules that ask for it), a run
//! is blended per channel onto the colours the frame painted behind it, which its key then holds
//! too. It stays grayscale under CSS opacity, behind an image or a gradient, where
//! [`Text::grayscale`] says so, and where the backend has no LCD masks; in a window that is not
//! opaque it is LCD only over an [`opaque_background`].

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
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Corners, Element, ElementId, Global,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Refineable, RenderImage,
    Rgba, Scene, SharedString, Style, StyleRefinement, Styled, Window, WindowId, fill, point, px,
    size,
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

/// How many backgrounds a run is kept blended onto besides those of the frame being painted: one
/// that animates makes a new image every frame.
const LCD_IMAGES: usize = 4;

#[derive(Clone, PartialEq, Eq, Hash)]
struct RunKey {
    font: FontKey,
    text: String,
    color: [u8; 3],
    fraction: Lu,
    scale: u32,
    /// [`Text::fade`]'s ends, from the run's origin.
    fade: Option<(Lu, Lu)>,
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
    /// Those of `rasterized` that were blended onto a background in LCD.
    pub lcd: u64,
    pub evicted: u64,
}

#[derive(Default)]
struct WindowText {
    cache: RunCache,
    stats: CacheStats,
    backdrop: Backdrop,
    grayscale: bool,
    /// The clock when the frame's first text was painted, once it has been.
    frame: Option<u64>,
}

pub struct TextSystem {
    backend: Box<dyn TextBackend>,
    lut: MaskGammaLut,
    lcd_lut: Option<LcdGammaLut>,
    windows: HashMap<WindowId, WindowText>,
    layers: Vec<Layer>,
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
            layers: Vec::new(),
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

    /// Replaces the platform's glyphs and LCD table, for tests of what the platform does not draw.
    pub fn set_backend(cx: &mut App, backend: Box<dyn TextBackend>, lcd_lut: Option<LcdGammaLut>) {
        let system = cx.default_global::<TextSystem>();
        system.backend = backend;
        system.lcd_lut = lcd_lut;
        system.windows.clear();
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

    /// Draws the text of `window` in grayscale except over an [`opaque_background`]: Chromium
    /// decides per layer, and a page that is not opaque is a layer that is not.
    pub fn set_grayscale(cx: &mut App, window: WindowId) {
        let system = cx.default_global::<TextSystem>();
        system.windows.entry(window).or_default().grayscale = true;
    }

    /// How many windows have a cache.
    pub fn windows(cx: &mut App) -> usize {
        cx.default_global::<TextSystem>().windows.len()
    }

    pub(crate) fn forget(cx: &mut App, window: WindowId) {
        if cx.has_global::<TextSystem>() {
            cx.global_mut::<TextSystem>().windows.remove(&window);
        }
    }

    fn lcd(&self, window: &Window) -> Lcd {
        let id = window.window_handle().window_id();
        let scale = window.scale_factor();
        if self.lcd_lut.is_none() || window.element_opacity() < self.faded() {
            return Lcd::Never;
        }
        if !self.windows.get(&id).is_some_and(|text| text.grayscale) {
            let grayscale = self.layers.iter().any(|layer| layer.grayscale(scale));
            return if grayscale { Lcd::Never } else { Lcd::Anywhere };
        }
        let mut own = false;
        let mut composited = scale < PREFER_COMPOSITING;
        for layer in &self.layers {
            match *layer {
                Layer::Always => return Lcd::Never,
                Layer::Scroller => composited = true,
                Layer::Own(always) => {
                    own = true;
                    composited |= always;
                }
                Layer::Page | Layer::Faded(_) | Layer::Selection => {}
            }
        }
        if own && composited {
            Lcd::OwnBackground
        } else {
            Lcd::Never
        }
    }

    /// The opacity around the text that does not make it grayscale.
    fn faded(&self) -> f32 {
        self.layers.iter().map(|layer| layer.opacity()).product()
    }

    /// The window's backdrop, where there is LCD text to read it.
    fn backdrop(&mut self, window: &Window) -> Option<&mut Backdrop> {
        self.lcd_lut.as_ref()?;
        let id = window.window_handle().window_id();
        Some(&mut self.windows.entry(id).or_default().backdrop)
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
        let run_from = x.div_euclid(64) * 64;
        let fade = ink
            .fade
            .filter(|(from, _)| x + fragment.width > *from)
            .map(|(from, to)| (from - run_from, to - run_from));
        let fraction = x.rem_euclid(64);
        let x = x.div_euclid(64);
        let key = RunKey {
            font: font.key(),
            text: fragment.text.clone(),
            color: ink.color,
            fraction,
            scale: scale.to_bits(),
            fade,
        };
        let backend = self.backend.as_mut();
        let WindowText {
            cache,
            stats,
            backdrop,
            frame,
            ..
        } = self.windows.entry(window).or_default();
        let frame = *frame.get_or_insert(self.clock);
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
                        Some(visible) => regions = backdrop.regions(visible, ink.own),
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
                let placed = run.and_then(|run| gray_image(run, ink.color, fade));
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
                    stats.lcd += 1;
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
                    let Some(image) = image.and_then(lcd_image) else {
                        break;
                    };
                    while run.lcd.len() >= LCD_IMAGES {
                        let oldest = (0..run.lcd.len())
                            .min_by_key(|&i| run.lcd[i].used)
                            .filter(|&i| run.lcd[i].used < frame);
                        let Some(oldest) = oldest else {
                            break;
                        };
                        let old = run.lcd.swap_remove(oldest).image;
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

/// Where text painted now may be LCD.
#[derive(Clone, Copy, PartialEq)]
enum Lcd {
    Never,
    Anywhere,
    /// Only over what an [`opaque_background`] painted.
    OwnBackground,
}

/// The device scale from which Chromium prefers compositing to LCD text.
const PREFER_COMPOSITING: f32 = 1.5;

/// The compositing layer Chromium draws text on: it is grayscale where the layer's own pixels are
/// not all opaque.
#[derive(Clone, Copy)]
enum Layer {
    /// The page's own, whatever the window is.
    Page,
    Always,
    /// A scroller is its own layer where Chromium prefers compositing to LCD text
    /// (`PreferCompositingToLCDText`).
    Scroller,
    /// CSS opacity that leaves LCD text alone (`save_layer_alpha_should_preserve_lcd_text`).
    Faded(f32),
    /// An [`opaque_background`], `true` where it is composited at any scale.
    Own(bool),
    /// A [`selection_background`].
    Selection,
}

impl Layer {
    fn opacity(self) -> f32 {
        match self {
            Layer::Faded(opacity) => opacity,
            Layer::Page | Layer::Always | Layer::Scroller | Layer::Own(_) | Layer::Selection => 1.0,
        }
    }

    fn grayscale(self, scale: f32) -> bool {
        match self {
            Layer::Page | Layer::Faded(_) | Layer::Own(_) | Layer::Selection => false,
            Layer::Always => true,
            Layer::Scroller => scale >= PREFER_COMPOSITING,
        }
    }
}

/// A wrapper that draws the text inside as Chromium draws it on a layer that is not opaque.
pub struct TextLayer {
    layer: Layer,
    child: AnyElement,
}

/// What CSS puts behind a mask or filter or on a layer of its own: its text is grayscale if
/// `separate`.
pub fn grayscale_layer(separate: bool, child: impl IntoElement) -> TextLayer {
    TextLayer {
        layer: if separate { Layer::Always } else { Layer::Page },
        child: child.into_any_element(),
    }
}

/// A scroller of the page that has no opaque background of its own.
pub fn scroller(overflows: bool, child: impl IntoElement) -> TextLayer {
    TextLayer {
        layer: if overflows {
            Layer::Scroller
        } else {
            Layer::Page
        },
        child: child.into_any_element(),
    }
}

/// An element that paints an opaque background under its own text. In a window that is not
/// opaque Chromium still draws that text LCD where it gives the element a layer of its own: below
/// 1.5x, in a scroller that overflows, or at any scale if `composited` (an overflowing scroller
/// itself, or `sticky` in one).
pub fn opaque_background(composited: bool, child: impl IntoElement) -> TextLayer {
    TextLayer {
        layer: Layer::Own(composited),
        child: child.into_any_element(),
    }
}

/// A text selection's highlight: Chromium paints it with the text, not as a background, so text
/// on it is not over an [`opaque_background`].
pub fn selection_background(child: impl IntoElement) -> TextLayer {
    TextLayer {
        layer: Layer::Selection,
        child: child.into_any_element(),
    }
}

/// CSS opacity on a page without masks, filters or blending, which preserves LCD text.
pub fn faded(opacity: f32, child: impl IntoElement) -> TextLayer {
    TextLayer {
        layer: Layer::Faded(opacity),
        child: child.into_any_element(),
    }
}

impl IntoElement for TextLayer {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextLayer {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let system = cx.default_global::<TextSystem>();
        system.layers.push(self.layer);
        let start = window.painted_scene().len();
        let own = match self.layer {
            Layer::Own(_) => Some(true),
            Layer::Selection => Some(false),
            _ => None,
        };
        let own = own.and_then(|own| Some(system.backdrop(window)?.own_from(start, own)));
        let opacity = matches!(self.layer, Layer::Faded(_)).then(|| self.layer.opacity());
        window.with_element_opacity(opacity, |window| self.child.paint(window, cx));
        let system = cx.default_global::<TextSystem>();
        system.layers.pop();
        let end = window.painted_scene().len();
        if let (Some(mark), Some(backdrop)) = (own, system.backdrop(window)) {
            backdrop.own_until(mark, end);
        }
    }
}

/// What all of a [`Text`]'s fragments are drawn with.
struct Ink<'a> {
    font: &'a FontSpec,
    scale: f32,
    color: [u8; 3],
    alpha: u8,
    fade: Option<(Lu, Lu)>,
    /// What the frame has painted so far, if the text may be LCD.
    lcd: Option<&'a Scene>,
    /// Whether it may be LCD only over an [`opaque_background`].
    own: bool,
    clip: Rect,
}

struct Drawn {
    x: i32,
    y: i32,
    image: Arc<RenderImage>,
    /// Set for an LCD image: where it has the background it was blended onto.
    clip: Option<Rect>,
}

fn gray_image(run: dom::RunImage, color: [u8; 3], fade: Option<(Lu, Lu)>) -> Option<Placed> {
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
    if let Some(fade) = fade {
        for row in bgra.chunks_exact_mut(run.width.max(1) as usize * 4) {
            for (col, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let kept = fade_kept(fade, run.left + col as i32);
                pixel[3] = (f32::from(pixel[3]) * kept + 0.5) as u8;
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

/// What a `mask-image` gradient keeps of a device column, from `from` on to `to`.
fn fade_kept((from, to): (Lu, Lu), column: i32) -> f32 {
    let at = column as f32 * 64.0 + 32.0;
    ((to as f32 - at) / (to - from).max(1) as f32).clamp(0.0, 1.0)
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

/// Whether `Text` painted now may be LCD.
pub fn may_be_lcd(window: &Window, cx: &mut App) -> bool {
    let system = cx.default_global::<TextSystem>();
    system.lcd(window) != Lcd::Never
}

/// Tells the text painted later that what `window` painted since its scene held `start` operations
/// left `bounds` in `color` (`0xRRGGBB`).
pub fn painted_flat(
    window: &Window,
    cx: &mut App,
    start: usize,
    bounds: Bounds<Pixels>,
    color: u32,
) {
    let system = cx.default_global::<TextSystem>();
    let Some(backdrop) = system.backdrop(window) else {
        return;
    };
    let clip = device_rect(window.content_mask().bounds, window.scale_factor());
    let rect = device_rect(bounds, window.scale_factor());
    let range = start..window.painted_scene().len();
    match backdrop::intersect(rect, clip) {
        Some(rect) => backdrop.fill(range, rect, gpui_kit::rgb(color)),
        None => backdrop.skip(range),
    }
}

/// Tells the text painted later that what `window` painted since its scene held `start` operations
/// is behind no text, though its bounds may reach there.
pub fn painted_beside(window: &Window, cx: &mut App, start: usize) {
    let system = cx.default_global::<TextSystem>();
    if let Some(backdrop) = system.backdrop(window) {
        backdrop.skip(start..window.painted_scene().len());
    }
}

/// `bounds` in device px, covering every pixel it touches.
fn device_rect(bounds: Bounds<Pixels>, scale: f32) -> Rect {
    let edge = |v: Pixels| f32::from(v) * scale;
    [
        edge(bounds.left()).floor() as i32,
        edge(bounds.top()).floor() as i32,
        edge(bounds.right()).ceil() as i32,
        edge(bounds.bottom()).ceil() as i32,
    ]
}

/// One glyph run's coverage, after mask gamma, at a whole device pixel position.
pub struct GlyphMask {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
    /// LCD text blended onto the background it was asked for on, as RGBA, with alpha 255 where
    /// it has coverage and 0 elsewhere.
    pub lcd: Option<Vec<u8>>,
}

/// The coverage [`Text`] would paint for `text` in a box `width` CSS px wide whose content
/// box starts at `origin` device px, for callers that composite text into their own pixmap, on
/// `background` if the text may be LCD.
#[expect(clippy::too_many_arguments)]
pub fn glyph_masks(
    cx: &mut App,
    style: &TextStyle,
    text: &str,
    scale: f32,
    width: f32,
    origin: (f32, f32),
    color: u32,
    background: Option<[u8; 3]>,
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
            if let (Some(background), Some(lcd_lut)) = (background, &system.lcd_lut)
                && let Some(run) = dom::rasterize_lcd(
                    system.backend.as_mut(),
                    (&system.lut, lcd_lut),
                    &fragment.glyphs,
                    x.rem_euclid(64),
                    scale,
                    dom::LcdInk {
                        color,
                        alpha: 255,
                        background,
                    },
                )
            {
                out.push(GlyphMask {
                    x: x.div_euclid(64) + run.left,
                    y: baseline + run.top,
                    width: run.width,
                    height: run.height,
                    alpha: Vec::new(),
                    lcd: Some(run.rgba),
                });
                continue;
            }
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
                    lcd: None,
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
    fade: Option<(f64, f64)>,
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
            fade: None,
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

    /// CSS opacity around this text alone, on a page with masks or filters: grayscale unless it
    /// is over an [`opaque_background`] that is a layer of its own.
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

    /// Fades out from the window's CSS px `from` to `to`, as a horizontal `mask-image` does.
    pub fn fade(mut self, from: f64, to: f64) -> Self {
        self.fade = Some((from, to));
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
        let system = cx.default_global::<TextSystem>();
        if system.lcd_lut.is_some()
            && let Some(text) = system.windows.get_mut(&window.window_handle().window_id())
        {
            text.backdrop.clear();
            text.frame = None;
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
        // Text behind a mask is grayscale.
        let lcd = match system.lcd(window) {
            _ if self.fade.is_some() => Lcd::Never,
            Lcd::Anywhere if self.grayscale => Lcd::Never,
            lcd => lcd,
        };
        let faded = system.faded();
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
        let device = |css: f64| dom::lu_round((css * f64::from(scale)) as f32);
        let fade = self.fade.map(|(from, to)| (device(from), device(to)));
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
            let ink = Ink {
                font: &self.style.font,
                scale,
                color: self.color,
                alpha: (self.alpha * faded * 255.0).round() as u8,
                fade,
                lcd: (lcd != Lcd::Never).then(|| window.painted_scene()),
                own: lcd == Lcd::OwnBackground,
                clip: device_rect(window.content_mask().bounds, scale),
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
                    let mut paint = |x: i32, width: i32, kept: f32| {
                        let a = s.coverage * kept;
                        let rect = bounds_of([x, s.y, width, s.height]);
                        window.paint_quad(fill(rect, Rgba { r, g, b, a }));
                    };
                    let end = s.x + s.width;
                    let solid = fade.map_or(end, |(from, _)| from.div_euclid(64).clamp(s.x, end));
                    if solid > s.x {
                        paint(s.x, solid - s.x, 1.0);
                    }
                    if let Some(fade) = fade {
                        (solid..end).for_each(|column| paint(column, 1, fade_kept(fade, column)));
                    }
                }
            });
            for Drawn { x, y, image, clip } in images {
                let s = image.size(0);
                let rect = bounds_of([x, y, s.width.0, s.height.0]);
                let (mask, opacity) = match clip {
                    Some([l, t, r, b]) => {
                        let bounds = bounds_of([l, t, r - l, b - t]);
                        let opacity = (faded != 1.0).then_some(1.0 / faded);
                        (Some(ContentMask { bounds }), opacity)
                    }
                    None => (None, alpha),
                };
                window.with_content_mask(mask, |window| {
                    window.with_element_opacity(opacity, |window| {
                        let _ = window.paint_image(rect, rect, Corners::default(), image, 0, false);
                    });
                });
            }
            if let Some(backdrop) = system.backdrop(window) {
                backdrop.skip(painted..window.painted_scene().len());
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
