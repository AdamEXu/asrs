pub mod geometry;
pub mod presenter;

use std::collections::HashMap;
use std::sync::Arc;

use asrs_canvas::pixels::transform_bilinear;
use asrs_canvas::raster::CanvasError;
use asrs_canvas::text::lut::MaskGammaLut;
use asrs_canvas::text::{TextBackend, platform_backend};
use asrs_canvas::{Canvas2d, Pixmap, Raster, RasterState, ShadeCache, TextRaster};
use gpui_kit::{App, Bounds, Context, Entity, Global, Pixels, Point, Window, canvas};

use geometry::{bitmap_side, client_rect_coord, client_size, mouse_client, valid_bitmap};
pub use presenter::FramePresenter;

use crate::input::style::device_border;

/// What an upstream renderer reads from the DOM before drawing: the canvas box's `clientWidth`
/// and `clientHeight`, `window.devicePixelRatio` and `getBoundingClientRect()` as `[x, y, w, h]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasMetrics {
    pub client_width: i32,
    pub client_height: i32,
    pub device_pixel_ratio: f64,
    pub client_rect: [f64; 4],
}

impl CanvasMetrics {
    pub fn measure(bounds: Bounds<Pixels>, dpr: f32) -> CanvasMetrics {
        let [x, y, w, h] = [
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
        ]
        .map(f32::from);
        CanvasMetrics {
            client_width: client_size(w, dpr),
            client_height: client_size(h, dpr),
            device_pixel_ratio: f64::from(dpr),
            client_rect: [
                client_rect_coord(x, dpr),
                client_rect_coord(y, dpr),
                client_rect_coord(w, dpr),
                client_rect_coord(h, dpr),
            ],
        }
    }

    /// The bitmap `canvas.width = width * devicePixelRatio; canvas.height = height * dpr` makes.
    pub fn bitmap_size(&self, width: f64, height: f64) -> (u32, u32) {
        let dpr = self.device_pixel_ratio;
        (
            bitmap_side(width * dpr, geometry::DEFAULT_WIDTH),
            bitmap_side(height * dpr, geometry::DEFAULT_HEIGHT),
        )
    }

    /// The bitmap for a canvas sized to its own client box, as most renderers do.
    pub fn client_bitmap_size(&self) -> (u32, u32) {
        self.bitmap_size(f64::from(self.client_width), f64::from(self.client_height))
    }

    /// `event.clientX - rect.x` and `event.clientY - rect.y` for a mouse event at `position`.
    pub fn pointer(&self, position: Point<Pixels>) -> [f64; 2] {
        [
            mouse_client(f32::from(position.x)) - self.client_rect[0],
            mouse_client(f32::from(position.y)) - self.client_rect[1],
        ]
    }
}

/// The canvas text stack shared by every canvas in the app: the platform glyph backend (with its
/// caches) and the mask-gamma table.
struct CanvasText {
    backend: Option<Box<dyn TextBackend>>,
    lut: Option<MaskGammaLut>,
}

impl Default for CanvasText {
    fn default() -> Self {
        CanvasText {
            backend: platform_backend(),
            lut: MaskGammaLut::embedded(),
        }
    }
}

impl Global for CanvasText {}

/// Runs `f` with the app's canvas glyph backend, for painters whose upstream code measures text
/// while building its display list; `None` where the platform has no backend. The backend is
/// lent out for the call, so `f` may use `cx` but not rasterize through it.
pub fn with_text_backend<R>(
    cx: &mut App,
    f: impl FnOnce(Option<&mut dyn TextBackend>, &mut App) -> R,
) -> R {
    let mut backend = cx.default_global::<CanvasText>().backend.take();
    let result = match backend.as_deref_mut() {
        Some(b) => f(Some(b), cx),
        None => f(None, cx),
    };
    cx.default_global::<CanvasText>().backend = backend;
    result
}

/// The pixmap a display list draws through the app's canvas text stack, or `None` for a canvas
/// Chromium would leave blank (an invalid size or a failed allocation). `state` is what the
/// canvas's earlier frames left and receives what this one leaves, and so do `shades` with the
/// images they resampled.
pub fn rasterize(
    frame: &Canvas2d,
    images: &HashMap<String, Arc<Pixmap>>,
    state: &mut RasterState,
    shades: &mut ShadeCache,
    cx: &mut App,
) -> Option<(Pixmap, Vec<(usize, CanvasError)>)> {
    let raster = valid_bitmap(frame.width, frame.height)
        .then(|| Raster::new(frame.width, frame.height))
        .flatten();
    let Some(raster) = raster else {
        *state = RasterState::default();
        *shades = ShadeCache::default();
        return None;
    };
    let mut raster = raster
        .with_state(*state)
        .with_shades(std::mem::take(shades));
    for (id, pixmap) in images {
        raster.add_image_element(id, pixmap.clone());
    }
    let text = cx.default_global::<CanvasText>();
    if let (Some(backend), Some(lut)) = (text.backend.as_deref_mut(), text.lut.as_ref()) {
        raster = raster.with_text(TextRaster { backend, lut });
    }
    raster.run(&frame.calls);
    *state = raster.state();
    *shades = raster.take_shades();
    let errors = raster.errors().to_vec();
    Some((raster.into_pixmap(), errors))
}

/// A `<canvas>`'s own `border` and `border-radius` in CSS px with `box-sizing: border-box`: its
/// bitmap is stretched over the content box and clipped to that box's rounded corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasBorder {
    pub width: f32,
    pub radius: f32,
}

impl CanvasBorder {
    fn device_width(&self, dpr: f32) -> f32 {
        device_border(self.width, dpr)
    }

    /// The bitmap over the content box of a border box `metrics` lays out.
    fn stretch(&self, bitmap: &Pixmap, metrics: &CanvasMetrics) -> Option<Pixmap> {
        let dpr = metrics.device_pixel_ratio;
        let border = 2.0 * f64::from(self.device_width(dpr as f32));
        let side = |box_side: f64| (box_side * dpr).round() - border;
        let [_, _, width, height] = metrics.client_rect;
        let (width, height) = (side(width), side(height));
        if !(width >= 1.0 && height >= 1.0) {
            return None;
        }
        let scale = [
            f64::from(bitmap.width()) / width,
            f64::from(bitmap.height()) / height,
        ];
        let inverse = [scale[0], 0.0, 0.0, scale[1], 0.0, 0.0];
        transform_bilinear(bitmap, width as u32, height as u32, inverse)
    }
}

/// One `<canvas>`: the display list it last drew, the `<img>` elements `drawImage` may name, and the frame
/// it shows. A display list equal to the last one is not drawn again, which is the dirty check
/// upstream renderers do by comparing `JSON.stringify` of their render state.
pub struct CanvasState {
    presenter: FramePresenter,
    raster: RasterState,
    shades: ShadeCache,
    frame: Option<Canvas2d>,
    images: HashMap<String, Arc<Pixmap>>,
    images_changed: bool,
    errors: Vec<(usize, CanvasError)>,
    metrics: Option<CanvasMetrics>,
    border: Option<CanvasBorder>,
    /// The bitmap a border stretched and the border box it was stretched for.
    stretched: Option<(Pixmap, [f64; 4])>,
    draws: u64,
}

impl CanvasState {
    pub fn new(cx: &mut Context<'_, Self>) -> CanvasState {
        cx.on_release(|this, cx| this.presenter.release(cx))
            .detach();
        CanvasState {
            presenter: FramePresenter::default(),
            raster: RasterState::default(),
            shades: ShadeCache::default(),
            frame: None,
            images: HashMap::new(),
            images_changed: false,
            errors: Vec::new(),
            metrics: None,
            border: None,
            stretched: None,
            draws: 0,
        }
    }

    /// A canvas with a border, which the caller draws around it.
    pub fn bordered(border: CanvasBorder, cx: &mut Context<'_, Self>) -> CanvasState {
        CanvasState {
            border: Some(border),
            ..CanvasState::new(cx)
        }
    }

    /// The metrics from the last layout, for pointer handlers and painters that run outside paint.
    pub fn metrics(&self) -> Option<CanvasMetrics> {
        self.metrics
    }

    pub fn frame(&self) -> Option<&Canvas2d> {
        self.frame.as_ref()
    }

    pub fn presenter(&self) -> &FramePresenter {
        &self.presenter
    }

    /// Exceptions thrown by the calls of the last drawn display list.
    pub fn errors(&self) -> &[(usize, CanvasError)] {
        &self.errors
    }

    /// How many display lists have been rasterized; unchanged frames do not count.
    pub fn draw_count(&self) -> u64 {
        self.draws
    }

    pub fn set_image(&mut self, id: impl Into<String>, pixmap: impl Into<Arc<Pixmap>>) {
        self.images.insert(id.into(), pixmap.into());
        self.images_changed = true;
    }

    pub fn remove_image(&mut self, id: &str) {
        self.images_changed |= self.images.remove(id).is_some();
    }

    /// Rasterizes and shows `frame` unless it equals the last one. Returns whether it drew.
    pub fn draw(&mut self, frame: Canvas2d, window: &mut Window, cx: &mut App) -> bool {
        if !self.images_changed && self.frame.as_ref() == Some(&frame) {
            return false;
        }
        self.images_changed = false;
        self.draws += 1;
        let shades = &mut self.shades;
        match rasterize(&frame, &self.images, &mut self.raster, shades, cx) {
            None => {
                self.errors.clear();
                self.stretched = None;
                self.presenter.clear(window, cx);
            }
            Some((pixmap, errors)) => {
                self.errors = errors;
                match self.border {
                    None => self.presenter.present(pixmap, window, cx),
                    Some(border) => self.present_stretched(border, pixmap, window, cx),
                }
            }
        }
        self.frame = Some(frame);
        true
    }

    fn present_stretched(
        &mut self,
        border: CanvasBorder,
        bitmap: Pixmap,
        window: &mut Window,
        cx: &mut App,
    ) {
        let metrics = self.metrics;
        match metrics.and_then(|metrics| border.stretch(&bitmap, &metrics)) {
            Some(shown) => self.presenter.present(shown, window, cx),
            None => self.presenter.clear(window, cx),
        }
        self.stretched = metrics.map(|metrics| (bitmap, metrics.client_rect));
    }

    /// Stretches the bitmap again when the border box changed size under an equal frame.
    fn fit(&mut self, window: &mut Window, cx: &mut App) {
        let (Some(border), Some(metrics)) = (self.border, self.metrics) else {
            return;
        };
        let current = |(_, rect): &(Pixmap, [f64; 4])| rect[2..] == metrics.client_rect[2..];
        if let Some((bitmap, _)) = self.stretched.take_if(|s| !current(s)) {
            self.present_stretched(border, bitmap, window, cx);
        }
    }

    fn paint(&self, origin: [f32; 2], window: &mut Window) {
        match self.border {
            None => self.presenter.paint(origin, window),
            Some(border) => {
                let dpr = window.scale_factor();
                let width = border.device_width(dpr);
                let inset = origin.map(|v| v + width / dpr);
                let radius = border.radius - width / dpr;
                self.presenter.paint_rounded(inset, radius, window);
            }
        }
    }

    /// Shows a frame drawn elsewhere (a 3D readback, a video frame), bypassing the display list.
    pub fn present_pixmap(&mut self, pixmap: Pixmap, window: &mut Window, cx: &mut App) {
        self.frame = None;
        self.errors.clear();
        self.presenter.present(pixmap, window, cx);
    }
}

/// The element for a canvas. During prepaint it measures its box, hands the metrics to `painter`
/// (the upstream renderer's `render`), and draws what it returns; during paint it shows the
/// current frame. `painter` returns `None` only where upstream returns before assigning
/// `canvas.width`: an early return after that assignment (Field2d's unloaded field image) leaves
/// a cleared canvas, so it returns the display list drawn so far.
pub fn canvas_view(
    state: Entity<CanvasState>,
    painter: impl FnOnce(&CanvasMetrics, &mut App) -> Option<Canvas2d> + 'static,
) -> gpui_kit::Canvas<[f32; 2]> {
    let paint_state = state.clone();
    canvas(
        move |bounds, window, cx| {
            let metrics = CanvasMetrics::measure(bounds, window.scale_factor());
            state.update(cx, |this, _| this.metrics = Some(metrics));
            if let Some(frame) = painter(&metrics, cx) {
                state.update(cx, |this, cx| this.draw(frame, window, cx));
            }
            state.update(cx, |this, cx| this.fit(window, cx));
            [f32::from(bounds.origin.x), f32::from(bounds.origin.y)]
        },
        move |_, origin, window, cx| {
            let state = paint_state.read(cx);
            state.paint(origin, window);
            #[cfg(feature = "parity")]
            if let Some(metrics) = state.metrics {
                let stretched = state.stretched.as_ref().map(|(bitmap, _)| bitmap.clone());
                let image = state.presenter.image().cloned();
                if let Some(bitmap) = stretched {
                    crate::parity::record_canvas(cx, || crate::parity::CanvasDump {
                        rect: metrics.client_rect,
                        width: bitmap.width(),
                        height: bitmap.height(),
                        rgba: asrs_canvas::pixels::to_straight_rgba(&bitmap),
                    });
                } else if let Some(image) = image {
                    crate::parity::record_canvas(cx, || canvas_dump(metrics.client_rect, &image));
                }
            }
        },
    )
}

/// The presented bitmap as `getImageData` reads it: straight-alpha RGBA.
#[cfg(feature = "parity")]
fn canvas_dump(rect: [f64; 4], image: &gpui_kit::RenderImage) -> crate::parity::CanvasDump {
    let size = image.size(0);
    let mut rgba = image.as_bytes(0).unwrap_or_default().to_vec();
    for p in rgba.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    crate::parity::CanvasDump {
        rect,
        width: size.width.0 as u32,
        height: size.height.0 as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_border_insets_the_bitmap_by_whole_device_pixels() {
        let border = CanvasBorder {
            width: 1.0,
            radius: 10.0,
        };
        for (dpr, size, inner) in [
            (1.0, (780, 32), (778, 30)),
            (2.0, (1560, 64), (1556, 60)),
            (1.5, (1170, 48), (1168, 46)),
        ] {
            let mut bitmap = Pixmap::new(size.0, size.1).expect("bitmap");
            for p in bitmap.data_mut().as_chunks_mut::<4>().0 {
                *p = [200, 40, 10, 255];
            }
            let metrics = CanvasMetrics {
                client_width: 780,
                client_height: 32,
                device_pixel_ratio: dpr,
                client_rect: [310.0, 88.0, 780.0, 32.0],
            };
            let shown = border.stretch(&bitmap, &metrics).expect("shown");
            assert_eq!((shown.width(), shown.height()), inner, "{dpr}");
            assert!(shown.pixels().iter().all(|p| *p == bitmap.pixels()[0]));
        }
        let flat = CanvasMetrics {
            client_width: 780,
            client_height: 0,
            device_pixel_ratio: 1.0,
            client_rect: [310.0, 88.0, 780.0, 0.0],
        };
        let bitmap = Pixmap::new(780, 32).expect("bitmap");
        assert!(border.stretch(&bitmap, &flat).is_none());
    }
}
