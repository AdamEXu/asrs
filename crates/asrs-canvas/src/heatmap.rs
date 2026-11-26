// Ported from heatmap.js 2.0.5, jwbonner fork (MIT, Copyright (c) 2015 Patrick Wied). See THIRD_PARTY_NOTICES.md.

//! Port of upstream `shared/renderers/Heatmap.ts` and the h337 fork pinned at
//! `jwbonner/heatmap.js#24b4455`: grid binning, the sparse store, radial-gradient
//! shadow stamps and the palette colorize pass all run through the crate's canvas
//! rasterizer.

#![deny(clippy::disallowed_methods)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::display::ImageDataSource;
use crate::raster::CanvasError;
use crate::{Call, Canvas2d, Gradient, ImageData, Pixmap, Property, Raster};
use asrs_js::math;
use asrs_js::math::int::{to_uint8_clamp, to_uint32};
use asrs_js::number::{self, to_int32};
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

const GRID_SIZE: f64 = 0.01;
const RADIUS: f64 = 0.1;

const DEFAULT_RADIUS: f64 = 40.0;
const DEFAULT_MAX_OPACITY: f64 = 1.0;
const DEFAULT_MIN_OPACITY: f64 = 0.0;
const DEFAULT_BLUR: f64 = 0.85;

const GET_IMAGE_REGISTER: &str = "heatmap-img";

fn truthy(value: Option<f64>) -> Option<f64> {
    value.filter(|v| *v != 0.0 && !v.is_nan())
}

fn falsy(value: Option<f64>) -> bool {
    match value {
        None => true,
        Some(v) => v == 0.0 || v.is_nan(),
    }
}

fn scale_value(value: f64, old0: f64, old1: f64, new0: f64, new1: f64) -> f64 {
    ((value - old0) / (old1 - old0)) * (new1 - new0) + new0
}

/// Blink's `canvas.width` setter (`unsigned long` + reflected content attribute): the
/// WebIDL `ToUint32` result is used unless its sign bit is set, which falls back to the
/// default 300x150 bitmap. `0.5`, NaN and infinities land on 0, `-1` and huge values on 300.
fn canvas_attr(v: f64) -> u32 {
    let u = to_uint32(v);
    if u > i32::MAX as u32 { 300 } else { u }
}

/// `container.style.width = v + "px"` on a block box in the oracle's document: the
/// declaration sticks only for finite non-negative lengths (Blink drops "NaNpx",
/// "Infinitypx" and negatives, keeping the previous value), and an auto width resolves
/// to the containing block: the 400 px, never-scrolling page of `oracle/src/heatmap/page.ts`.
/// The empty container's auto height is 0.
struct Container {
    style_width: Option<f64>,
    style_height: Option<f64>,
}

const CONTAINING_BLOCK_WIDTH: f64 = 400.0;

impl Container {
    fn new() -> Container {
        Container {
            style_width: None,
            style_height: None,
        }
    }

    fn set_size(&mut self, width: f64, height: f64) {
        if width.is_finite() && width >= 0.0 {
            self.style_width = Some(number::to_number(
                JsString::from(number::to_string(width)).units(),
            ));
        }
        if height.is_finite() && height >= 0.0 {
            self.style_height = Some(number::to_number(
                JsString::from(number::to_string(height)).units(),
            ));
        }
    }

    /// `+(getComputedStyle(container).width.replace(/px/,''))`.
    fn computed_width(&self) -> f64 {
        self.style_width.unwrap_or(CONTAINING_BLOCK_WIDTH)
    }

    fn computed_height(&self) -> f64 {
        self.style_height.unwrap_or(0.0)
    }
}

/// The canonical array index a property key names, matching V8's `Array#length` rules.
fn array_index(key: &JsString) -> Option<u64> {
    let n = number::to_number(key.units());
    if (0.0..4294967295.0).contains(&n)
        && n.trunc() == n
        && number::to_string(n) == key.as_string()?
    {
        Some(n as u64)
    } else {
        None
    }
}

fn js_key(key: &JsString) -> String {
    key.as_string().unwrap_or_default()
}

/// An upstream grid loop whose step never reaches the bound (negative or zero grid size) or
/// that needs past `2^32` iterations pushes until V8 throws `RangeError: Invalid array
/// length`, or, in a renderer with a smaller heap, until it crashes out of memory. The port
/// reports the RangeError without running the doomed loop.
fn grid_len_would_throw(bound: f64, step: f64) -> bool {
    const ARRAY_LENGTH_MAX: f64 = 4294967295.0;
    if bound.is_nan() || bound <= 0.0 {
        return false;
    }
    if step.is_nan() {
        return false;
    }
    if step <= 0.0 {
        return true;
    }
    let approx = bound / step;
    !approx.is_finite() || approx > ARRAY_LENGTH_MAX - 1.0
}

/// What `Heatmap.update` observably did, for parity recording.
pub struct UpdateOutcome {
    pub created: bool,
    /// The argument `h337.setData` was called with, when it was called.
    pub set_data: Option<SetData>,
    /// The exception the call threw, when it did (upstream's `update` propagates them).
    pub error: Option<CanvasError>,
}

/// Upstream `Heatmap`: bins translations into a field grid and feeds the h337 instance,
/// recreating it whenever the pixel or field dimensions change.
pub struct Heatmap {
    container: Container,
    heatmap: Option<H337>,
    last_pixel_dimensions: [f64; 2],
    last_field_dimensions: [f64; 2],
    last_translations_str: String,
}

impl Heatmap {
    pub fn new() -> Heatmap {
        Heatmap {
            container: Container::new(),
            heatmap: None,
            last_pixel_dimensions: [0.0, 0.0],
            last_field_dimensions: [0.0, 0.0],
            last_translations_str: String::new(),
        }
    }

    pub fn update(
        &mut self,
        translations: &[[f64; 2]],
        pixel_dimensions: [f64; 2],
        field_dimensions: [f64; 2],
    ) -> UpdateOutcome {
        let created = pixel_dimensions != self.last_pixel_dimensions
            || field_dimensions != self.last_field_dimensions;
        if created {
            self.last_pixel_dimensions = pixel_dimensions;
            self.last_field_dimensions = field_dimensions;
            self.container
                .set_size(pixel_dimensions[0], pixel_dimensions[1]);
            self.heatmap = Some(H337::create(Config {
                width: self.container.computed_width(),
                height: self.container.computed_height(),
                radius: Some(pixel_dimensions[1] * RADIUS),
                max_opacity: Some(0.75),
                ..Config::default()
            }));
        }

        let mut set_data = None;
        let mut error = None;
        let grid_size_meters = field_dimensions[1] * GRID_SIZE;
        let translations_str = translations_str(translations);
        if translations_str != self.last_translations_str || created {
            self.last_translations_str = translations_str;

            if grid_len_would_throw(field_dimensions[1] + grid_size_meters, grid_size_meters)
                || grid_len_would_throw(field_dimensions[0] + grid_size_meters, grid_size_meters)
            {
                return UpdateOutcome {
                    created,
                    set_data,
                    error: Some(CanvasError::Range("Invalid array length")),
                };
            }

            let mut grid: Vec<Vec<f64>> = Vec::new();
            let mut x = 0.0;
            while x < field_dimensions[0] + grid_size_meters {
                let mut column = Vec::new();
                let mut y = 0.0;
                while y < field_dimensions[1] + grid_size_meters {
                    column.push(0.0);
                    y += grid_size_meters;
                }
                grid.push(column);
                x += grid_size_meters;
            }

            let rows = grid.first().map_or(0, Vec::len) as f64;
            for translation in translations {
                let grid_x =
                    ((translation[0] + field_dimensions[0] / 2.0) / grid_size_meters).floor();
                let grid_y =
                    ((translation[1] + field_dimensions[1] / 2.0) / grid_size_meters).floor();
                if grid_x >= 0.0 && grid_y >= 0.0 && grid_x < grid.len() as f64 && grid_y < rows {
                    grid[grid_x as usize][grid_y as usize] += 1.0;
                }
            }

            let mut heatmap_data = Vec::new();
            let mut x = grid_size_meters / 2.0;
            let mut max_value = 0.0;
            for column in &grid {
                x += grid_size_meters;
                let mut y = grid_size_meters / 2.0;
                for &grid_value in column {
                    y += grid_size_meters;
                    let coordinates = [
                        math::round(scale_value(
                            x,
                            0.0,
                            field_dimensions[0],
                            0.0,
                            pixel_dimensions[0],
                        )),
                        math::round(scale_value(
                            y,
                            0.0,
                            field_dimensions[1],
                            pixel_dimensions[1],
                            0.0,
                        )),
                    ];
                    max_value = math::max2(max_value, grid_value);
                    if grid_value > 0.0 {
                        heatmap_data.push(Point {
                            x: coordinates[0],
                            y: coordinates[1],
                            value: grid_value,
                        });
                    }
                }
            }
            if let Some(heatmap) = self.heatmap.as_mut() {
                let data = SetData {
                    min: 0.0,
                    max: max_value,
                    data: heatmap_data,
                };
                let result = heatmap.set_data(&data);
                set_data = Some(data);
                if let Err(e) = result {
                    error = Some(e);
                }
            }
        }
        UpdateOutcome {
            created,
            set_data,
            error,
        }
    }

    /// `heatmap.getData()`: the unorganized store contents, `None` before any instance.
    pub fn get_data(&self) -> Option<StoreData> {
        self.heatmap.as_ref().map(H337::get_data)
    }

    /// The output canvas's straight-alpha pixels, as `getImageData` on it returns.
    pub fn image_data(&mut self) -> Result<Option<ImageData>, CanvasError> {
        match self.heatmap.as_mut() {
            None => Ok(None),
            Some(heatmap) => heatmap.canvas_image_data().map(Some),
        }
    }

    /// The output canvas's premultiplied bitmap, which `drawImage` draws from; `None` while it
    /// is blank, when drawing it changes nothing.
    pub fn pixmap(&self) -> Option<&Arc<Pixmap>> {
        match &self.heatmap.as_ref()?.renderer.canvas.bitmap {
            Bitmap::Shared(pixmap) => Some(pixmap),
            Bitmap::Blank | Bitmap::Raster(_) => None,
        }
    }

    /// The output canvas's bitmap size, as the element's `width`/`height` report.
    pub fn canvas_size(&self) -> Option<(u32, u32)> {
        self.heatmap.as_ref().map(|h| h.renderer.canvas_size())
    }

    /// The shadow (alpha stamp) canvas's pixels, straight alpha as `getImageData` returns,
    /// which the canvas parity gate compares byte for byte.
    pub fn shadow_image_data(&mut self) -> Result<Option<ImageData>, CanvasError> {
        match self.heatmap.as_mut() {
            None => Ok(None),
            Some(heatmap) => heatmap.shadow_image_data().map(Some),
        }
    }
}

impl Default for Heatmap {
    fn default() -> Self {
        Self::new()
    }
}

fn translations_str(translations: &[[f64; 2]]) -> String {
    let value = JsValue::Array(
        translations
            .iter()
            .map(|t| JsValue::Array(vec![JsValue::Number(t[0]), JsValue::Number(t[1])]))
            .collect(),
    );
    asrs_js::json::stringify(&value)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// One `data` entry of the `setData` argument.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub value: f64,
}

/// The `setData` argument.
#[derive(Clone, Debug, PartialEq)]
pub struct SetData {
    pub min: f64,
    pub max: f64,
    pub data: Vec<Point>,
}

/// One `getData().data` entry: `x`/`y` stay the store's string keys.
#[derive(Clone, Debug, PartialEq)]
pub struct StorePoint {
    pub x: String,
    pub y: String,
    pub radius: f64,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoreData {
    pub min: f64,
    pub max: f64,
    pub data: Vec<StorePoint>,
}

/// The h337 config keys `Heatmap.ts` or the merge can feed the renderer/store.
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// `config.width || +getComputedStyle(container).width`, resolved by the caller.
    pub width: f64,
    pub height: f64,
    pub radius: Option<f64>,
    pub max_opacity: Option<f64>,
    pub min_opacity: Option<f64>,
    pub opacity: Option<f64>,
    pub blur: Option<f64>,
}

struct H337 {
    store: Store,
    renderer: Renderer,
}

impl H337 {
    fn create(config: Config) -> H337 {
        H337 {
            renderer: Renderer::new(&config),
            store: Store::new(&config),
        }
    }

    /// `setData`: reset the store, organise every point, then `renderall`.
    fn set_data(&mut self, data: &SetData) -> Result<(), CanvasError> {
        self.store.set_data(data);
        self.renderer.render_all(&self.store.internal())
    }

    fn get_data(&self) -> StoreData {
        self.store.get_data()
    }

    fn canvas_image_data(&mut self) -> Result<ImageData, CanvasError> {
        let (w, h) = self.renderer.canvas_size();
        let canvas = &mut self.renderer.canvas;
        let data = canvas.get_image_data(0.0, 0.0, w as f64, h as f64);
        canvas.share();
        data
    }

    fn shadow_image_data(&mut self) -> Result<ImageData, CanvasError> {
        let (w, h) = (self.renderer.width, self.renderer.height);
        self.renderer.shadow.get_image_data(0.0, 0.0, w, h)
    }
}

/// `Store`: the sparse `x -> y -> value`/`radius` grids keyed by `String(number)`.
struct Store {
    data: JsObject<JsObject<f64>>,
    radi: JsObject<JsObject<f64>>,
    min: f64,
    max: f64,
    cfg_radius: Option<f64>,
}

struct InternalData<'a> {
    max: f64,
    min: f64,
    data: &'a JsObject<JsObject<f64>>,
    radi: &'a JsObject<JsObject<f64>>,
}

impl Store {
    fn new(config: &Config) -> Store {
        Store {
            data: JsObject::default(),
            radi: JsObject::default(),
            min: 10.0,
            max: 1.0,
            cfg_radius: truthy(config.radius),
        }
    }

    fn organise_data(&mut self, point: &Point) {
        let x = point.x;
        let y = point.y;
        let max = self.max;
        let min = self.min;
        let value = truthy(Some(point.value)).unwrap_or(1.0);
        let radius = self.cfg_radius.unwrap_or(DEFAULT_RADIUS);

        let x_key = number::to_string(x);
        let y_key = number::to_string(y);
        if self.data.get(&x_key).is_none() {
            self.data.insert(x_key.clone(), JsObject::new());
            self.radi.insert(x_key.clone(), JsObject::new());
        }
        let column = self.data.get_mut(&x_key).unwrap();
        if falsy(column.get(&y_key).copied()) {
            column.insert(y_key.clone(), value);
            self.radi.get_mut(&x_key).unwrap().insert(&y_key, radius);
        } else {
            *column.get_mut(&y_key).unwrap() += value;
        }
        let stored_val = *column.get(&y_key).unwrap();

        if stored_val > max {
            self.max = stored_val;
        } else if stored_val < min {
            self.min = stored_val;
        }
    }

    fn set_data(&mut self, data: &SetData) {
        self.data = JsObject::default();
        self.radi = JsObject::default();
        for point in &data.data {
            self.organise_data(point);
        }
        self.max = data.max;
        self.min = truthy(Some(data.min)).unwrap_or(0.0);
    }

    fn internal(&self) -> InternalData<'_> {
        InternalData {
            max: self.max,
            min: self.min,
            data: &self.data,
            radi: &self.radi,
        }
    }

    fn get_data(&self) -> StoreData {
        let mut data = Vec::new();
        for (x, column) in self.data.iter() {
            for (y, &value) in column.iter() {
                let radius = self
                    .radi
                    .get(&js_key(x))
                    .and_then(|r| r.get(&js_key(y)))
                    .copied()
                    .unwrap_or(f64::NAN);
                data.push(StorePoint {
                    x: js_key(x),
                    y: js_key(y),
                    radius,
                    value,
                });
            }
        }
        StoreData {
            min: self.min,
            max: self.max,
            data,
        }
    }
}

/// The pixels of a [`Layer`]. A canvas no pixel was drawn on since its last whole clear has no
/// memory.
enum Bitmap {
    Blank,
    Raster(Box<Raster<'static>>),
    Shared(Arc<Pixmap>),
}

/// A canvas element plus its 2d context: a persistent `Raster`, or the all-no-op
/// behavior a zero-sized canvas bitmap exhibits (draws do nothing, `getImageData`
/// still validates its arguments and returns transparent pixels, as it does of a blank one).
struct Layer {
    bitmap: Bitmap,
    images: HashMap<String, Arc<Pixmap>>,
    width: u32,
    height: u32,
}

const OUT_OF_MEMORY: CanvasError = CanvasError::Range("Out of memory at ImageData creation");

impl Layer {
    fn new(width: u32, height: u32) -> Layer {
        Layer {
            bitmap: Bitmap::Blank,
            images: HashMap::new(),
            width,
            height,
        }
    }

    fn raster(&mut self) -> Option<&mut Raster<'static>> {
        if !matches!(self.bitmap, Bitmap::Raster(_)) {
            let mut raster = match std::mem::replace(&mut self.bitmap, Bitmap::Blank) {
                Bitmap::Shared(pixmap) => Raster::over(Arc::unwrap_or_clone(pixmap)),
                _ => {
                    #[cfg(test)]
                    tests::RASTERS.with(|n| n.set(n.get() + 1));
                    Raster::new(self.width, self.height)?
                }
            };
            for (id, pixmap) in &self.images {
                raster.add_image(id, pixmap.clone());
            }
            self.bitmap = Bitmap::Raster(Box::new(raster));
        }
        match &mut self.bitmap {
            Bitmap::Raster(raster) => Some(raster),
            _ => None,
        }
    }

    /// Lets others draw from the bitmap. Drawing on the canvas again leaves what they hold as
    /// it was.
    fn share(&mut self) {
        self.bitmap = match std::mem::replace(&mut self.bitmap, Bitmap::Blank) {
            Bitmap::Raster(raster) => Bitmap::Shared(Arc::new(raster.into_pixmap())),
            other => other,
        };
    }

    fn apply(&mut self, call: &Call) {
        if let Some(raster) = self.raster() {
            raster.apply(call);
        }
    }

    fn covered_by(&self, width: f64, height: f64) -> bool {
        width >= f64::from(self.width) && height >= f64::from(self.height)
    }

    /// `clearRect(0, 0, width, height)`.
    fn clear(&mut self, width: f64, height: f64) {
        let drop = match &self.bitmap {
            Bitmap::Blank => return,
            Bitmap::Raster(_) => false,
            Bitmap::Shared(pixmap) => Arc::strong_count(pixmap) > 1,
        };
        if drop && self.covered_by(width, height) {
            self.bitmap = Bitmap::Blank;
        } else {
            self.apply(&Call::ClearRect(0.0, 0.0, width, height));
        }
    }

    /// [`Layer::clear`] of a canvas nothing is about to be drawn on.
    fn empty(&mut self, width: f64, height: f64) {
        if self.covered_by(width, height) {
            self.bitmap = Bitmap::Blank;
        } else {
            self.clear(width, height);
        }
    }

    fn add_image(&mut self, id: &str, pixmap: Arc<Pixmap>) {
        if let Bitmap::Raster(raster) = &mut self.bitmap {
            raster.add_image(id, pixmap.clone());
        }
        self.images.insert(id.to_owned(), pixmap);
    }

    /// `getImageData`, propagating the exception upstream throws instead of recording it.
    fn get_image_data(
        &mut self,
        sx: f64,
        sy: f64,
        sw: f64,
        sh: f64,
    ) -> Result<ImageData, CanvasError> {
        let raster = match self.bitmap {
            Bitmap::Blank => None,
            Bitmap::Raster(_) | Bitmap::Shared(_) => self.raster(),
        };
        match raster {
            Some(raster) => {
                let call = Call::GetImageData {
                    sx,
                    sy,
                    sw,
                    sh,
                    register: GET_IMAGE_REGISTER.to_owned(),
                };
                let errors = raster.errors().len();
                raster.apply(&call);
                if let Some((_, e)) = raster.errors().get(errors) {
                    return Err(e.clone());
                }
                Ok(raster.take_register(GET_IMAGE_REGISTER).unwrap())
            }
            None => {
                let (mut x, mut y, mut w, mut h) =
                    (to_int32(sx), to_int32(sy), to_int32(sw), to_int32(sh));
                if w.checked_mul(h).is_none() {
                    return Err(OUT_OF_MEMORY);
                }
                if w == 0 {
                    return Err(CanvasError::IndexSize("The source width is 0."));
                }
                if h == 0 {
                    return Err(CanvasError::IndexSize("The source height is 0."));
                }
                if w < 0 {
                    x = x.checked_add(w).ok_or(OUT_OF_MEMORY)?;
                    w = i32::try_from(w.unsigned_abs()).unwrap_or(i32::MAX);
                }
                if h < 0 {
                    y = y.checked_add(h).ok_or(OUT_OF_MEMORY)?;
                    h = i32::try_from(h.unsigned_abs()).unwrap_or(i32::MAX);
                }
                if x.checked_add(w).is_none() || y.checked_add(h).is_none() {
                    return Err(OUT_OF_MEMORY);
                }
                let elements = 4u32
                    .checked_mul(w as u32)
                    .and_then(|v| v.checked_mul(h as u32));
                let Some(elements) = elements else {
                    return Err(CanvasError::Range("Out of memory at ImageData creation."));
                };
                let mut data = Vec::new();
                data.try_reserve_exact(elements as usize)
                    .map_err(|_| CanvasError::Range("Array buffer allocation failed"))?;
                data.resize(elements as usize, 0);
                Ok(ImageData {
                    width: w as u32,
                    height: h as u32,
                    data,
                })
            }
        }
    }

    fn put_image_data(&mut self, image: ImageData, dx: f64, dy: f64) {
        self.apply(&Call::PutImageData {
            data: ImageDataSource::Inline(image),
            dx,
            dy,
        });
    }
}

struct PreparedPoint<'a> {
    x: &'a JsString,
    y: &'a JsString,
    value: f64,
    radius: f64,
}
struct Renderer {
    shadow: Layer,
    canvas: Layer,
    width: f64,
    height: f64,
    palette: Vec<u8>,
    templates: HashSet<String>,
    render_boundaries: [f64; 4],
    blur: f64,
    opacity: f64,
    max_opacity: f64,
    min_opacity: f64,
    min: f64,
    max: f64,
}

impl Renderer {
    fn new(config: &Config) -> Renderer {
        let mut renderer = Renderer {
            shadow: Layer::new(canvas_attr(config.width), canvas_attr(config.height)),
            canvas: Layer::new(canvas_attr(config.width), canvas_attr(config.height)),
            width: config.width,
            height: config.height,
            palette: color_palette(),
            templates: HashSet::new(),
            render_boundaries: [10000.0, 10000.0, 0.0, 0.0],
            blur: 0.0,
            opacity: 0.0,
            max_opacity: 0.0,
            min_opacity: 0.0,
            min: 0.0,
            max: 0.0,
        };
        renderer.set_styles(config);
        renderer
    }

    fn canvas_size(&self) -> (u32, u32) {
        (self.canvas.width, self.canvas.height)
    }

    fn set_styles(&mut self, config: &Config) {
        self.blur = if config.blur == Some(0.0) {
            0.0
        } else {
            truthy(config.blur).unwrap_or(DEFAULT_BLUR)
        };
        self.width = truthy(Some(config.width)).unwrap_or(self.width);
        self.height = truthy(Some(config.height)).unwrap_or(self.height);
        self.opacity = truthy(config.opacity).unwrap_or(0.0) * 255.0;
        self.max_opacity = truthy(config.max_opacity).unwrap_or(DEFAULT_MAX_OPACITY) * 255.0;
        self.min_opacity = truthy(config.min_opacity).unwrap_or(DEFAULT_MIN_OPACITY) * 255.0;
    }

    fn render_all(&mut self, data: &InternalData<'_>) -> Result<(), CanvasError> {
        let (width, height) = (self.width, self.height);
        if array_length(data.data) == 0 {
            self.shadow.empty(width, height);
            self.canvas.empty(width, height);
            self.canvas.share();
            return Ok(());
        }
        self.shadow.clear(width, height);
        self.canvas.clear(width, height);
        let drawn = self
            .draw_alpha(&prepare_data(data))
            .and_then(|()| self.colorize());
        self.canvas.share();
        drawn
    }

    fn draw_alpha(&mut self, data: &Prepared<'_>) -> Result<(), CanvasError> {
        self.min = data.min;
        self.max = data.max;
        let (min, max) = (data.min, data.max);
        let blur = 1.0 - self.blur;
        for point in data.points.iter().rev() {
            let x = number::to_number(point.x.units());
            let y = number::to_number(point.y.units());
            let radius = point.radius;
            let value = math::min2(point.value, max);
            let rect_x = x - radius;
            let rect_y = y - radius;

            let key = number::to_string(radius);
            if !self.templates.contains(&key) {
                if let Some(pixmap) = point_template(radius, blur) {
                    self.shadow.add_image(&key, Arc::new(pixmap));
                }
                self.templates.insert(key.clone());
            }
            let alpha = (value - min) / (max - min);
            self.shadow
                .apply(&Call::Set(Property::GlobalAlpha(if alpha < 0.01 {
                    0.01
                } else {
                    alpha
                })));
            if canvas_attr(radius * 2.0) == 0 {
                return Err(CanvasError::InvalidState(
                    "The image argument is a canvas element with a width or height of 0.",
                ));
            }
            self.shadow.apply(&Call::DrawImage {
                image: key.clone(),
                args: vec![rect_x, rect_y],
            });
            if rect_x < self.render_boundaries[0] {
                self.render_boundaries[0] = rect_x;
            }
            if rect_y < self.render_boundaries[1] {
                self.render_boundaries[1] = rect_y;
            }
            if rect_x + 2.0 * radius > self.render_boundaries[2] {
                self.render_boundaries[2] = rect_x + 2.0 * radius;
            }
            if rect_y + 2.0 * radius > self.render_boundaries[3] {
                self.render_boundaries[3] = rect_y + 2.0 * radius;
            }
        }
        Ok(())
    }

    fn colorize(&mut self) -> Result<(), CanvasError> {
        let boundaries = self.render_boundaries;
        let mut x = boundaries[0];
        let mut y = boundaries[1];
        let mut width = boundaries[2] - x;
        let mut height = boundaries[3] - y;
        let max_width = self.width;
        let max_height = self.height;
        let opacity = self.opacity;
        let max_opacity = self.max_opacity;
        let min_opacity = self.min_opacity;
        if x < 0.0 {
            x = 0.0;
        }
        if y < 0.0 {
            y = 0.0;
        }
        if x + width > max_width {
            width = max_width - x;
        }
        if y + height > max_height {
            height = max_height - y;
        }

        let mut img = self.shadow.get_image_data(x, y, width, height)?;
        let palette = &self.palette;
        let mut i = 3;
        while i < img.data.len() {
            let alpha = img.data[i] as usize;
            let offset = alpha * 4;
            if offset != 0 {
                let alpha = alpha as f64;
                let final_alpha = if opacity > 0.0 {
                    opacity
                } else if alpha < max_opacity {
                    if alpha < min_opacity {
                        min_opacity
                    } else {
                        alpha
                    }
                } else {
                    max_opacity
                };
                img.data[i - 3] = palette[offset];
                img.data[i - 2] = palette[offset + 1];
                img.data[i - 1] = palette[offset + 2];
                img.data[i] = to_uint8_clamp(final_alpha);
            }
            i += 4;
        }

        self.canvas.put_image_data(img, x, y);
        self.render_boundaries = [1000.0, 1000.0, 0.0, 0.0];
        Ok(())
    }
}

struct Prepared<'a> {
    min: f64,
    max: f64,
    points: Vec<PreparedPoint<'a>>,
}

fn prepare_data<'a>(data: &InternalData<'a>) -> Prepared<'a> {
    let mut points = Vec::new();
    for (x, column) in data.data.iter().rev() {
        for (y, &value) in column.iter().rev() {
            let radius = data
                .radi
                .get(&js_key(x))
                .and_then(|r| r.get(&js_key(y)))
                .copied()
                .unwrap_or(f64::NAN);
            points.push(PreparedPoint {
                x,
                y,
                value,
                radius,
            });
        }
    }
    Prepared {
        min: data.min,
        max: data.max,
        points,
    }
}

/// JS `Array#length` over the sparse store's keys (`data.data.length` upstream).
fn array_length(data: &JsObject<JsObject<f64>>) -> u64 {
    let mut length = 0;
    for (key, _) in data.iter() {
        if let Some(index) = array_index(key) {
            length = length.max(index + 1);
        }
    }
    length
}

/// `h337._getColorPalette`: 256x1 gradient, keyed in `for..in` order so `1` (from `1.0`)
/// sorts before the fractional stops exactly as it does upstream.
fn color_palette() -> Vec<u8> {
    let mut list = Canvas2d::new(256, 1);
    let mut gradient = Gradient::linear(0.0, 0.0, 256.0, 1.0);
    gradient.add_color_stop(1.0, "rgb(255,0,0)");
    gradient.add_color_stop(0.25, "rgb(0,0,255)");
    gradient.add_color_stop(0.55, "rgb(0,255,0)");
    gradient.add_color_stop(0.85, "yellow");
    list.set_fill_style(gradient);
    list.fill_rect(0.0, 0.0, 256.0, 1.0);
    list.get_image_data(0.0, 0.0, 256.0, 1.0, GET_IMAGE_REGISTER);
    let mut raster = Raster::new(256, 1).unwrap();
    raster.run(&list.calls);
    raster.register(GET_IMAGE_REGISTER).unwrap().data.clone()
}

/// `h337._getPointTemplate`: the `radius*2` canvas stamped at `x - radius`.
fn point_template(radius: f64, blur: f64) -> Option<Pixmap> {
    let size = canvas_attr(radius * 2.0);
    let mut list = Canvas2d::new(size, size);
    if blur == 1.0 {
        list.begin_path();
        list.arc(radius, radius, radius, 0.0, 2.0 * std::f64::consts::PI);
        list.set_fill_style("rgba(0,0,0,1)");
        list.fill();
    } else {
        let mut gradient = Gradient::radial(radius, radius, radius * blur, radius, radius, radius);
        gradient.add_color_stop(0.0, "rgba(0,0,0,1)");
        gradient.add_color_stop(1.0, "rgba(0,0,0,0)");
        list.set_fill_style(gradient);
        list.fill_rect(0.0, 0.0, 2.0 * radius, 2.0 * radius);
    }
    let mut raster = Raster::new(size, size)?;
    raster.run(&list.calls);
    Some(raster.into_pixmap())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIXELS: [f64; 2] = [120.0, 60.0];
    const FIELD: [f64; 2] = [16.0, 8.0];
    const A: &[[f64; 2]] = &[[0.0, 0.0], [1.0, 1.0], [1.0, 1.0]];
    const B: &[[f64; 2]] = &[[-6.0, -3.0], [2.0, 3.5]];

    fn update(heatmap: &mut Heatmap, translations: &[[f64; 2]]) {
        let outcome = heatmap.update(translations, PIXELS, FIELD);
        assert_eq!(outcome.error, None);
    }

    fn pixels(heatmap: &mut Heatmap) -> [Vec<u8>; 2] {
        [
            heatmap.image_data().unwrap().unwrap().data,
            heatmap.shadow_image_data().unwrap().unwrap().data,
        ]
    }

    fn fresh(translations: &[[f64; 2]]) -> [Vec<u8>; 2] {
        let mut heatmap = Heatmap::new();
        update(&mut heatmap, translations);
        pixels(&mut heatmap)
    }

    fn blank(heatmap: &Heatmap) -> bool {
        let renderer = &heatmap.heatmap.as_ref().unwrap().renderer;
        [&renderer.canvas, &renderer.shadow]
            .iter()
            .all(|layer| matches!(layer.bitmap, Bitmap::Blank))
    }

    #[test]
    fn canvases_have_memory_only_while_there_are_points() {
        let mut heatmap = Heatmap::new();
        update(&mut heatmap, &[]);
        assert!(blank(&heatmap) && heatmap.pixmap().is_none());
        assert_eq!(heatmap.canvas_size(), Some((120, 60)));
        let empty = pixels(&mut heatmap);
        assert_eq!(empty, [vec![0; 120 * 60 * 4], vec![0; 120 * 60 * 4]]);
        assert!(blank(&heatmap));

        update(&mut heatmap, A);
        assert!(!blank(&heatmap) && heatmap.pixmap().is_some());
        assert_eq!(pixels(&mut heatmap), fresh(A));
        assert!(heatmap.pixmap().is_some());

        update(&mut heatmap, &[]);
        assert!(blank(&heatmap) && heatmap.pixmap().is_none());
        assert_eq!(pixels(&mut heatmap), empty);

        update(&mut heatmap, B);
        assert_eq!(pixels(&mut heatmap), fresh(B));
        heatmap.update(B, [90.0, 45.0], FIELD);
        heatmap.update(&[], [60.0, 30.0], FIELD);
        assert!(blank(&heatmap));
    }

    thread_local! {
        pub(super) static RASTERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    #[test]
    fn a_heatmap_without_points_makes_no_canvas() {
        let mut heatmap = Heatmap::new();
        for size in [[2000.0, 1000.0], [2400.0, 1200.0]] {
            heatmap.update(&[], size, FIELD);
            heatmap.update(&[[90.0, 90.0]], size, FIELD);
            heatmap.update(&[], size, FIELD);
        }
        assert_eq!(RASTERS.get(), 0);
        heatmap.update(A, [2000.0, 1000.0], FIELD);
        assert_eq!(RASTERS.get(), 2);
    }

    #[test]
    fn the_bitmap_is_shared_and_stays_as_it_was_for_those_who_hold_it() {
        let mut heatmap = Heatmap::new();
        update(&mut heatmap, A);
        let held = heatmap.pixmap().unwrap().clone();
        update(&mut heatmap, A);
        assert!(Arc::ptr_eq(&held, heatmap.pixmap().unwrap()));
        let before = held.data().to_vec();

        update(&mut heatmap, B);
        assert!(!Arc::ptr_eq(&held, heatmap.pixmap().unwrap()));
        assert_eq!(held.data(), before);
        assert_eq!(pixels(&mut heatmap), fresh(B));
        update(&mut heatmap, A);
        assert_eq!(pixels(&mut heatmap), fresh(A));
        assert_eq!(heatmap.pixmap().unwrap().data(), before);
    }

    #[test]
    fn a_bitmap_nobody_else_holds_is_drawn_on_again() {
        let mut heatmap = Heatmap::new();
        update(&mut heatmap, A);
        let at = heatmap.pixmap().unwrap().data().as_ptr();
        update(&mut heatmap, B);
        assert_eq!(heatmap.pixmap().unwrap().data().as_ptr(), at);
        assert_eq!(pixels(&mut heatmap), fresh(B));
    }
}
