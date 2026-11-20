pub mod geometry;
pub mod presenter;

use std::collections::HashMap;

use asrs_canvas::raster::CanvasError;
use asrs_canvas::text::lut::MaskGammaLut;
use asrs_canvas::text::{TextBackend, platform_backend};
use asrs_canvas::{Canvas2d, Pixmap, Raster, RasterState, TextRaster};
use gpui_kit::{App, Bounds, Context, Entity, Global, Pixels, Point, Window, canvas};

use geometry::{bitmap_side, client_rect_coord, client_size, mouse_client, valid_bitmap};
pub use presenter::FramePresenter;

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

/// The pixmap a display list draws through the app's canvas text stack, or `None` for a canvas
/// Chromium would leave blank (an invalid size or a failed allocation). `state` is what the
/// canvas's earlier frames left and receives what this one leaves.
pub fn rasterize(
    frame: &Canvas2d,
    images: &HashMap<String, Pixmap>,
    state: &mut RasterState,
    cx: &mut App,
) -> Option<(Pixmap, Vec<(usize, CanvasError)>)> {
    let raster = valid_bitmap(frame.width, frame.height)
        .then(|| Raster::new(frame.width, frame.height))
        .flatten();
    let Some(raster) = raster else {
        *state = RasterState::default();
        return None;
    };
    let mut raster = raster.with_state(*state);
    for (id, pixmap) in images {
        raster.add_image(id, pixmap.clone());
    }
    let text = cx.default_global::<CanvasText>();
    if let (Some(backend), Some(lut)) = (text.backend.as_deref_mut(), text.lut.as_ref()) {
        raster = raster.with_text(TextRaster { backend, lut });
    }
    raster.run(&frame.calls);
    *state = raster.state();
    let errors = raster.errors().to_vec();
    Some((raster.into_pixmap(), errors))
}

/// One `<canvas>`: the display list it last drew, the images `drawImage` may name, and the frame
/// it shows. A display list equal to the last one is not drawn again, which is the dirty check
/// upstream renderers do by comparing `JSON.stringify` of their render state.
pub struct CanvasState {
    presenter: FramePresenter,
    raster: RasterState,
    frame: Option<Canvas2d>,
    images: HashMap<String, Pixmap>,
    images_changed: bool,
    errors: Vec<(usize, CanvasError)>,
    metrics: Option<CanvasMetrics>,
    draws: u64,
}

impl CanvasState {
    pub fn new(cx: &mut Context<'_, Self>) -> CanvasState {
        cx.on_release(|this, cx| this.presenter.release(cx))
            .detach();
        CanvasState {
            presenter: FramePresenter::default(),
            raster: RasterState::default(),
            frame: None,
            images: HashMap::new(),
            images_changed: false,
            errors: Vec::new(),
            metrics: None,
            draws: 0,
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

    pub fn set_image(&mut self, id: impl Into<String>, pixmap: Pixmap) {
        self.images.insert(id.into(), pixmap);
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
        match rasterize(&frame, &self.images, &mut self.raster, cx) {
            None => {
                self.errors.clear();
                self.presenter.clear(window, cx);
            }
            Some((pixmap, errors)) => {
                self.errors = errors;
                self.presenter.present(pixmap, window, cx);
            }
        }
        self.frame = Some(frame);
        true
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
            [f32::from(bounds.origin.x), f32::from(bounds.origin.y)]
        },
        move |_, origin, window, cx| paint_state.read(cx).presenter.paint(origin, window),
    )
}
