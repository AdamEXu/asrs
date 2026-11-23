use asrs_js::string::JsString;

#[derive(Clone, Debug, PartialEq)]
pub enum GradientShape {
    Linear([f64; 4]),
    Radial([f64; 6]),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub shape: GradientShape,
    pub stops: Vec<(f64, String)>,
}

impl Gradient {
    pub fn linear(x0: f64, y0: f64, x1: f64, y1: f64) -> Gradient {
        Gradient {
            shape: GradientShape::Linear([x0, y0, x1, y1]),
            stops: Vec::new(),
        }
    }

    pub fn radial(x0: f64, y0: f64, r0: f64, x1: f64, y1: f64, r1: f64) -> Gradient {
        Gradient {
            shape: GradientShape::Radial([x0, y0, r0, x1, y1, r1]),
            stops: Vec::new(),
        }
    }

    pub fn add_color_stop(&mut self, offset: f64, color: &str) {
        self.stops.push((offset, color.to_owned()));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Style {
    Color(String),
    Gradient(Gradient),
    /// `undefined` assigned to the property: recorded, and ignored like any invalid colour.
    Undefined,
}

impl From<&str> for Style {
    fn from(css: &str) -> Style {
        Style::Color(css.to_owned())
    }
}

impl From<Gradient> for Style {
    fn from(g: Gradient) -> Style {
        Style::Gradient(g)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    FillStyle(Style),
    StrokeStyle(Style),
    LineWidth(f64),
    LineCap(String),
    LineJoin(String),
    MiterLimit(f64),
    /// `None` is `undefined` assigned to the property: recorded, and ignored like any NaN.
    LineDashOffset(Option<f64>),
    GlobalAlpha(f64),
    Font(String),
    TextAlign(String),
    TextBaseline(String),
    ImageSmoothingEnabled(bool),
}

impl Property {
    pub fn name(&self) -> &'static str {
        match self {
            Property::FillStyle(_) => "fillStyle",
            Property::StrokeStyle(_) => "strokeStyle",
            Property::LineWidth(_) => "lineWidth",
            Property::LineCap(_) => "lineCap",
            Property::LineJoin(_) => "lineJoin",
            Property::MiterLimit(_) => "miterLimit",
            Property::LineDashOffset(_) => "lineDashOffset",
            Property::GlobalAlpha(_) => "globalAlpha",
            Property::Font(_) => "font",
            Property::TextAlign(_) => "textAlign",
            Property::TextBaseline(_) => "textBaseline",
            Property::ImageSmoothingEnabled(_) => "imageSmoothingEnabled",
        }
    }
}

/// Straight-alpha RGBA, as `getImageData` returns and `putImageData` takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImageDataSource {
    Register(String),
    Inline(ImageData),
}

/// One `CanvasRenderingContext2D` call, with its arguments exactly as the painter passed them.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Set(Property),
    Save,
    Restore,
    Scale(f64, f64),
    Rotate(f64),
    Translate(f64, f64),
    SetTransform([f64; 6]),
    ResetTransform,
    BeginPath,
    ClosePath,
    MoveTo(f64, f64),
    LineTo(f64, f64),
    QuadraticCurveTo(f64, f64, f64, f64),
    BezierCurveTo(f64, f64, f64, f64, f64, f64),
    Rect(f64, f64, f64, f64),
    Arc {
        x: f64,
        y: f64,
        radius: f64,
        start: f64,
        end: f64,
        anticlockwise: Option<bool>,
    },
    Ellipse {
        x: f64,
        y: f64,
        radius_x: f64,
        radius_y: f64,
        rotation: f64,
        start: f64,
        end: f64,
        anticlockwise: Option<bool>,
    },
    Fill,
    FillEvenOdd,
    Stroke,
    Clip,
    FillRect(f64, f64, f64, f64),
    StrokeRect(f64, f64, f64, f64),
    ClearRect(f64, f64, f64, f64),
    FillText {
        text: JsString,
        x: f64,
        y: f64,
        max_width: Option<f64>,
    },
    SetLineDash(Vec<f64>),
    /// `args` is `[dx, dy]`, `[dx, dy, dw, dh]` or `[sx, sy, sw, sh, dx, dy, dw, dh]`.
    DrawImage {
        image: String,
        args: Vec<f64>,
    },
    GetImageData {
        sx: f64,
        sy: f64,
        sw: f64,
        sh: f64,
        register: String,
    },
    PutImageData {
        data: ImageDataSource,
        dx: f64,
        dy: f64,
    },
}

/// The display list of one canvas frame. Painters call it like a `CanvasRenderingContext2D`;
/// [`crate::raster::Raster`] replays it and [`crate::trace`] serializes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Canvas2d {
    pub width: u32,
    pub height: u32,
    pub calls: Vec<Call>,
}

impl Canvas2d {
    pub fn new(width: u32, height: u32) -> Canvas2d {
        Canvas2d {
            width,
            height,
            calls: Vec::new(),
        }
    }

    /// `measureText` under the `font`, `textAlign` and `textBaseline` the list has set so far.
    pub fn measure_text(
        &self,
        backend: &mut dyn crate::text::TextBackend,
        text: &str,
    ) -> crate::text::TextMetrics {
        let (font, align, baseline) = crate::text::text_state(&self.calls);
        crate::text::measure_text(backend, &font, &align, &baseline, text)
    }

    pub fn push(&mut self, call: Call) {
        self.calls.push(call);
    }

    fn set(&mut self, property: Property) {
        self.calls.push(Call::Set(property));
    }

    pub fn set_fill_style(&mut self, style: impl Into<Style>) {
        self.set(Property::FillStyle(style.into()));
    }

    pub fn set_stroke_style(&mut self, style: impl Into<Style>) {
        self.set(Property::StrokeStyle(style.into()));
    }

    pub fn set_line_width(&mut self, width: f64) {
        self.set(Property::LineWidth(width));
    }

    pub fn set_line_cap(&mut self, cap: &str) {
        self.set(Property::LineCap(cap.to_owned()));
    }

    pub fn set_line_join(&mut self, join: &str) {
        self.set(Property::LineJoin(join.to_owned()));
    }

    pub fn set_miter_limit(&mut self, limit: f64) {
        self.set(Property::MiterLimit(limit));
    }

    pub fn set_line_dash_offset(&mut self, offset: f64) {
        self.set(Property::LineDashOffset(Some(offset)));
    }

    pub fn set_global_alpha(&mut self, alpha: f64) {
        self.set(Property::GlobalAlpha(alpha));
    }

    pub fn set_font(&mut self, font: &str) {
        self.set(Property::Font(font.to_owned()));
    }

    pub fn set_text_align(&mut self, align: &str) {
        self.set(Property::TextAlign(align.to_owned()));
    }

    pub fn set_text_baseline(&mut self, baseline: &str) {
        self.set(Property::TextBaseline(baseline.to_owned()));
    }

    pub fn set_image_smoothing_enabled(&mut self, enabled: bool) {
        self.set(Property::ImageSmoothingEnabled(enabled));
    }

    pub fn save(&mut self) {
        self.push(Call::Save);
    }

    pub fn restore(&mut self) {
        self.push(Call::Restore);
    }

    pub fn scale(&mut self, x: f64, y: f64) {
        self.push(Call::Scale(x, y));
    }

    pub fn rotate(&mut self, angle: f64) {
        self.push(Call::Rotate(angle));
    }

    pub fn translate(&mut self, x: f64, y: f64) {
        self.push(Call::Translate(x, y));
    }

    pub fn begin_path(&mut self) {
        self.push(Call::BeginPath);
    }

    pub fn close_path(&mut self) {
        self.push(Call::ClosePath);
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        self.push(Call::MoveTo(x, y));
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        self.push(Call::LineTo(x, y));
    }

    pub fn quadratic_curve_to(&mut self, cpx: f64, cpy: f64, x: f64, y: f64) {
        self.push(Call::QuadraticCurveTo(cpx, cpy, x, y));
    }

    pub fn bezier_curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        self.push(Call::BezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y));
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::Rect(x, y, w, h));
    }

    pub fn arc(&mut self, x: f64, y: f64, radius: f64, start: f64, end: f64) {
        self.push(Call::Arc {
            x,
            y,
            radius,
            start,
            end,
            anticlockwise: None,
        });
    }

    pub fn arc_with_direction(
        &mut self,
        x: f64,
        y: f64,
        radius: f64,
        start: f64,
        end: f64,
        anticlockwise: bool,
    ) {
        self.push(Call::Arc {
            x,
            y,
            radius,
            start,
            end,
            anticlockwise: Some(anticlockwise),
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        x: f64,
        y: f64,
        radius_x: f64,
        radius_y: f64,
        rotation: f64,
        start: f64,
        end: f64,
        anticlockwise: Option<bool>,
    ) {
        self.push(Call::Ellipse {
            x,
            y,
            radius_x,
            radius_y,
            rotation,
            start,
            end,
            anticlockwise,
        });
    }

    pub fn set_transform(&mut self, matrix: [f64; 6]) {
        self.push(Call::SetTransform(matrix));
    }

    pub fn reset_transform(&mut self) {
        self.push(Call::ResetTransform);
    }

    pub fn fill(&mut self) {
        self.push(Call::Fill);
    }

    pub fn stroke(&mut self) {
        self.push(Call::Stroke);
    }

    pub fn clip(&mut self) {
        self.push(Call::Clip);
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::FillRect(x, y, w, h));
    }

    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::StrokeRect(x, y, w, h));
    }

    pub fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::ClearRect(x, y, w, h));
    }

    pub fn fill_text(&mut self, text: impl Into<JsString>, x: f64, y: f64) {
        self.push(Call::FillText {
            text: text.into(),
            x,
            y,
            max_width: None,
        });
    }

    pub fn fill_text_max_width(
        &mut self,
        text: impl Into<JsString>,
        x: f64,
        y: f64,
        max_width: f64,
    ) {
        self.push(Call::FillText {
            text: text.into(),
            x,
            y,
            max_width: Some(max_width),
        });
    }

    pub fn set_line_dash(&mut self, segments: &[f64]) {
        self.push(Call::SetLineDash(segments.to_vec()));
    }

    pub fn draw_image(&mut self, image: &str, dx: f64, dy: f64) {
        self.push(Call::DrawImage {
            image: image.to_owned(),
            args: vec![dx, dy],
        });
    }

    pub fn draw_image_scaled(&mut self, image: &str, dx: f64, dy: f64, dw: f64, dh: f64) {
        self.push(Call::DrawImage {
            image: image.to_owned(),
            args: vec![dx, dy, dw, dh],
        });
    }

    pub fn draw_image_source(&mut self, image: &str, source: [f64; 4], dest: [f64; 4]) {
        let mut args = source.to_vec();
        args.extend(dest);
        self.push(Call::DrawImage {
            image: image.to_owned(),
            args,
        });
    }

    /// Records `getImageData`; the pixels land in `register` when the list is rasterized.
    pub fn get_image_data(&mut self, sx: f64, sy: f64, sw: f64, sh: f64, register: &str) {
        self.push(Call::GetImageData {
            sx,
            sy,
            sw,
            sh,
            register: register.to_owned(),
        });
    }

    pub fn put_image_data(&mut self, data: ImageDataSource, dx: f64, dy: f64) {
        self.push(Call::PutImageData { data, dx, dy });
    }
}
