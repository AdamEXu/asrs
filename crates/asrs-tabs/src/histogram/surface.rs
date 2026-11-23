use asrs_canvas::text::{FontSpec, TextBackend, measure_text};
use asrs_canvas::{Call, Canvas2d, Property};
use asrs_js::string::JsString;

#[derive(Clone)]
struct TextState {
    font: FontSpec,
    align: String,
    baseline: String,
}

impl Default for TextState {
    fn default() -> TextState {
        TextState {
            font: FontSpec::default(),
            align: "start".to_owned(),
            baseline: "alphabetic".to_owned(),
        }
    }
}

/// The chart's canvas. Chart.js never resets it between frames, so its display list would grow
/// with every draw; [`Surface::clear`] drops what a full clear makes invisible.
#[derive(Default)]
pub struct Surface {
    canvas: Canvas2d,
    text: TextState,
    stack: Vec<TextState>,
    recorder: Option<Recorder>,
}

#[derive(Default)]
struct Recorder {
    reset: bool,
    calls: Vec<Call>,
}

impl Surface {
    pub fn canvas(&self) -> &Canvas2d {
        &self.canvas
    }

    pub fn record_calls(&mut self) {
        self.recorder = Some(Recorder::default());
    }

    /// Whether the canvas was reset since the last take, and every call since the reset.
    pub fn take_calls(&mut self) -> (bool, Vec<Call>) {
        let recorder = self.recorder.as_mut().map(std::mem::take);
        recorder.map_or((false, Vec::new()), |r| (r.reset, r.calls))
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        self.canvas = Canvas2d::new(width, height);
        self.text = TextState::default();
        self.stack.clear();
        if let Some(recorder) = &mut self.recorder {
            *recorder = Recorder {
                reset: true,
                calls: Vec::new(),
            };
        }
    }

    fn push(&mut self, call: Call) {
        match &call {
            Call::Save => self.stack.push(self.text.clone()),
            Call::Restore => {
                if let Some(saved) = self.stack.pop() {
                    self.text = saved;
                }
            }
            Call::Set(Property::Font(css)) => {
                if let Some(font) = FontSpec::parse(css) {
                    self.text.font = font;
                }
            }
            Call::Set(Property::TextAlign(align)) => self.text.align.clone_from(align),
            Call::Set(Property::TextBaseline(baseline)) => self.text.baseline.clone_from(baseline),
            _ => {}
        }
        if let Some(recorder) = &mut self.recorder {
            recorder.calls.push(call.clone());
        }
        self.canvas.push(call);
    }

    pub fn clear(&mut self) {
        let mut kept: Vec<Call> = Vec::new();
        let mut depth = 0u32;
        for call in self.canvas.calls.drain(..) {
            match &call {
                Call::Save => depth += 1,
                Call::Restore => depth -= 1,
                Call::SetTransform(_) if depth == 0 => kept = vec![call],
                Call::Set(property) if depth == 0 => {
                    let name = property.name();
                    kept.retain(|k| !matches!(k, Call::Set(p) if p.name() == name));
                    kept.push(call);
                }
                _ => {}
            }
        }
        self.canvas.calls = kept;
        let (width, height) = (f64::from(self.canvas.width), f64::from(self.canvas.height));
        self.save();
        self.push(Call::ResetTransform);
        self.push(Call::ClearRect(0.0, 0.0, width, height));
        self.restore();
    }

    pub fn measure(&self, text: &mut dyn TextBackend, string: &JsString) -> f64 {
        let TextState {
            font,
            align,
            baseline,
        } = &self.text;
        measure_text(text, font, align, baseline, &string.to_string_lossy()).width
    }

    pub fn set_transform(&mut self, matrix: [f64; 6]) {
        self.push(Call::SetTransform(matrix));
    }

    pub fn save(&mut self) {
        self.push(Call::Save);
    }

    pub fn restore(&mut self) {
        self.push(Call::Restore);
    }

    pub fn set(&mut self, property: Property) {
        self.push(Call::Set(property));
    }

    pub fn font(&mut self, css: &str) {
        self.set(Property::Font(css.to_owned()));
    }

    pub fn fill_style(&mut self, css: &str) {
        self.set(Property::FillStyle(css.into()));
    }

    pub fn stroke_style(&mut self, css: &str) {
        self.set(Property::StrokeStyle(css.into()));
    }

    pub fn line_width(&mut self, width: f64) {
        self.set(Property::LineWidth(width));
    }

    pub fn text_align(&mut self, align: &str) {
        self.set(Property::TextAlign(align.to_owned()));
    }

    pub fn text_baseline(&mut self, baseline: &str) {
        self.set(Property::TextBaseline(baseline.to_owned()));
    }

    pub fn line_dash(&mut self) {
        self.push(Call::SetLineDash(Vec::new()));
    }

    pub fn translate(&mut self, x: f64, y: f64) {
        self.push(Call::Translate(x, y));
    }

    pub fn rotate(&mut self, angle: f64) {
        self.push(Call::Rotate(angle));
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

    pub fn quadratic_curve_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        self.push(Call::QuadraticCurveTo(cx, cy, x, y));
    }

    pub fn rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::Rect(x, y, w, h));
    }

    pub fn fill(&mut self) {
        self.push(Call::Fill);
    }

    pub fn fill_even_odd(&mut self) {
        self.push(Call::FillEvenOdd);
    }

    pub fn clip(&mut self) {
        self.push(Call::Clip);
    }

    pub fn stroke(&mut self) {
        self.push(Call::Stroke);
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::FillRect(x, y, w, h));
    }

    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(Call::StrokeRect(x, y, w, h));
    }

    pub fn fill_text(&mut self, text: JsString, x: f64, y: f64) {
        self.push(Call::FillText {
            text,
            x,
            y,
            max_width: None,
        });
    }

    pub fn clip_area(&mut self, left: f64, top: f64, right: f64, bottom: f64) {
        self.save();
        self.begin_path();
        self.rect(left, top, right - left, bottom - top);
        self.push(Call::Clip);
    }
}
