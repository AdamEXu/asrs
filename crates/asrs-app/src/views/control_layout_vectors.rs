//! `dom/control-layout/*`: Blink's boxes of the tabs' controls at whole and fractional scales.

use asrs_parity::{Value, vectors};

const SCALES: [(&str, f64); 5] = [
    ("1x", 1.0),
    ("1.25x", 1.25),
    ("1.5x", 1.5),
    ("2x", 2.0),
    ("3x", 3.0),
];
/// Below half a LayoutUnit at any scale here, above what single precision client rects lose.
const TOLERANCE: f64 = 2e-3;

pub type Rect = [f64; 4];

#[derive(Clone)]
pub struct Element {
    pub class: String,
    pub rect: Rect,
    /// The rects of the element's own text.
    pub lines: Vec<Rect>,
}

pub struct Capture {
    pub scale: f64,
    pub window: f64,
    pub name_width: f64,
    /// A joystick select's intrinsic width.
    pub select_width: Option<f64>,
    /// The fixed 3D canvas's `style.width` and `style.height`.
    pub canvas_style: Option<[String; 2]>,
    pub controls: Rect,
    pub renderer: Rect,
    parts: Vec<(String, Vec<Element>)>,
    failures: Vec<String>,
    compared: usize,
}

fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {}", other.kind()),
    }
}

fn rect(value: &Value) -> Rect {
    [0, 1, 2, 3].map(|i| items(value)[i].as_f64().expect("a number"))
}

fn text(value: &Value) -> String {
    value.as_str().expect("a string").to_string_lossy()
}

fn element(value: &Value) -> Element {
    let [class, bounds, lines] = items(value) else {
        panic!("not an element");
    };
    Element {
        class: text(class),
        rect: rect(bounds),
        lines: items(lines).iter().map(rect).collect(),
    }
}

impl Capture {
    fn new(value: &Value, scale: f64) -> Capture {
        let get = |key: &str| value.get(key).unwrap_or_else(|| panic!("no {key}"));
        let parts = items(get("parts")).iter().map(|part| {
            let [selector, found] = items(part) else {
                panic!("not a part");
            };
            let found = items(found).iter().map(element).collect();
            (text(selector), found)
        });
        Capture {
            scale,
            window: get("window").as_f64().expect("a width"),
            name_width: get("nameWidth").as_f64().expect("a width"),
            select_width: get("selectWidth").as_f64(),
            canvas_style: match get("canvasStyle") {
                Value::Array(style) => Some([0, 1].map(|i| text(&style[i]))),
                _ => None,
            },
            controls: rect(get("controls")),
            renderer: rect(get("renderer")),
            parts: parts.collect(),
            failures: Vec::new(),
            compared: 0,
        }
    }

    /// The displayed elements a recorded selector matches.
    pub fn all(&self, selector: &str) -> &[Element] {
        let found = self.parts.iter().find(|(s, _)| s == selector);
        found.map_or(&[], |(_, elements)| elements)
    }

    pub fn one(&self, selector: &str, class: &str) -> &Element {
        let found = self.all(selector).iter().find(|e| e.class.contains(class));
        found.unwrap_or_else(|| panic!("no {selector} {class}"))
    }

    pub fn check(&mut self, what: &str, ours: &[f64], blink: &[f64]) {
        self.compared += 1;
        let close = |(a, b): (&f64, &f64)| (a - b).abs() < TOLERANCE;
        if ours.len() != blink.len() || !ours.iter().zip(blink).all(close) {
            self.failures.push(format!(
                "{what} in {:?} at {}: ours {ours:?}, Blink {blink:?}",
                self.controls, self.scale
            ));
        }
    }
}

/// Runs `compare` on every capture of a tab type, which must make `compared` comparisons.
pub fn each(tab_type: f64, compared: usize, mut compare: impl FnMut(&mut Capture)) {
    let mut failures: Vec<(f64, Vec<String>)> = Vec::new();
    let mut total = 0;
    for (name, scale) in SCALES {
        let mut failed = Vec::new();
        let records = vectors::load(&format!("dom/control-layout/{name}")).expect("vector");
        for record in &records {
            if !record
                .string("eval")
                .is_some_and(|eval| eval.starts_with("/*control-layout*/"))
            {
                continue;
            }
            let value = record.value("value").expect("value");
            if value.get("type").and_then(Value::as_f64) != Some(tab_type) {
                continue;
            }
            let mut capture = Capture::new(&value, scale);
            compare(&mut capture);
            total += capture.compared;
            failed.append(&mut capture.failures);
        }
        failures.push((scale, failed));
    }
    let shown: Vec<&String> = failures
        .iter()
        .flat_map(|(_, f)| f.iter().take(3))
        .collect();
    let counts: Vec<(f64, usize)> = failures.iter().map(|(s, f)| (*s, f.len())).collect();
    assert!(shown.is_empty(), "{counts:?} of {total} differ: {shown:#?}");
    assert_eq!(total, compared);
}
