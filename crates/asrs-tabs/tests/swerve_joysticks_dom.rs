#![allow(clippy::disallowed_methods)]

//! Replays the `dom/swerve/*` and `dom/joysticks/*` vectors written by
//! `oracle/tools/r07Specs.mjs` and recorded in Blink: every `render` of every upstream Swerve and
//! Joysticks renderer with the size reads it made, its throw, and, when it reset the canvas, the
//! pixel size, the style strings it assigned and its canvas calls. The records replay in order
//! through the port's renderers, image `load` events included, so a frame painted or kept, the
//! trace call for call and the throw must all match. Records the harness does not know fail, and
//! each test checks it compared every record of its kind.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use asrs_canvas::Canvas2d;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_model::assets::AdvantageScopeAssets;
use asrs_parity::vectors::{load, vectors_dir};
use asrs_parity::{Json, Record, Utf16String, Value};
use asrs_tabs::joysticks_renderer::{JoysticksDom, JoysticksRenderer};
use asrs_tabs::swerve_renderer::{SwerveDom, SwerveRenderer};

const OTHER_KINDS: &[&str] = &["read", "write", "main", "exception"];

/// `IMAGES` of `oracle/tools/r07Specs.mjs`: the fixtures' `images`.
const IMAGES: [(&str, [f64; 2]); 5] = [
    (
        "/Users/pilot/Assets/Joystick_Wide/image.png",
        [1250.0, 800.0],
    ),
    ("bundledAssets/Joystick_Tall/image.png", [333.0, 1000.0]),
    (
        "C:\\Users\\pilot\\Assets\\Joystick_Small%20Pad\\image.png",
        [101.0, 67.0],
    ),
    ("bundledAssets/Joystick_ZeroWidth/image.png", [0.0, 50.0]),
    ("bundledAssets/Joystick_ZeroHeight/image.png", [50.0, 0.0]),
];

type Compared = BTreeMap<&'static str, usize>;

fn count(compared: &mut Compared, what: &'static str) {
    *compared.entry(what).or_insert(0) += 1;
}

fn case_ids(area: &str) -> Vec<String> {
    let dir = vectors_dir().join(area);
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(format!("{area}/{}", name.split('.').next()?))
        })
        .collect();
    ids.sort();
    ids.dedup();
    assert!(!ids.is_empty(), "no {area} vectors");
    ids
}

fn to_js(value: &Value) -> Result<JsValue, String> {
    Ok(match value {
        Value::Undefined => JsValue::Undefined,
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(bits) => JsValue::Number(f64::from_bits(*bits)),
        Value::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
        Value::Array(items) => JsValue::Array(items.iter().map(to_js).collect::<Result<_, _>>()?),
        Value::Object(pairs) => {
            let mut object = JsObject::new();
            for (k, v) in pairs {
                object.insert(JsString::from_units(k.units().to_vec()), to_js(v)?);
            }
            JsValue::Object(object)
        }
        other => return Err(format!("no JsValue for CVE-1 {}", other.kind())),
    })
}

fn value(record: &Record, key: &str) -> Result<Value, String> {
    record.value(key).map_err(|e| format!("{key}: {e}"))
}

fn number(value: &Value) -> Result<f64, String> {
    value
        .as_f64()
        .ok_or_else(|| format!("expected a number, got {}", value.kind()))
}

fn number_field(record: &Record, key: &str) -> Result<f64, String> {
    number(&value(record, key)?)
}

fn string(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .and_then(Utf16String::as_string)
        .ok_or_else(|| format!("expected a string, got {}", value.kind()))
}

fn json_string(json: &Json) -> Result<String, String> {
    json.as_str()
        .and_then(Utf16String::as_string)
        .ok_or_else(|| format!("expected a JSON string, got {}", json.to_compact()))
}

/// `[[name, value], ...]`.
fn pairs(record: &Record, key: &str) -> Result<Vec<(String, Value)>, String> {
    record
        .field(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("render without {key}"))?
        .iter()
        .map(|pair| match pair.as_array() {
            Some([name, value]) => Ok((
                json_string(name)?,
                Value::from_json(value).map_err(|e| e.to_string())?,
            )),
            _ => Err(format!("{key} entry is not a pair")),
        })
        .collect()
}

fn error_text(e: &JsError) -> String {
    format!("{}: {}", e.name, e.message)
}

fn report(test: &str, compared: &Compared, failures: &[String]) {
    eprintln!("{test}: compared {compared:?}, {} failing", failures.len());
    let shown: Vec<&str> = failures.iter().take(40).map(String::as_str).collect();
    assert!(
        failures.is_empty(),
        "{test}: {} failures, first {}:\n{}",
        failures.len(),
        shown.len(),
        shown.join("\n")
    );
}

fn r07_records(id: &str) -> Result<Vec<Record>, String> {
    let mut out = Vec::new();
    for record in load(id)? {
        if record.field("r07").is_some() {
            out.push(record);
            continue;
        }
        let kind = match &record.json {
            Json::Object(pairs) => pairs.first().and_then(|(k, _)| k.as_string()),
            _ => None,
        };
        if !kind.is_some_and(|k| OTHER_KINDS.contains(&k.as_str())) {
            return Err(format!(
                "{id} line {}: unknown record {}",
                record.line, record.text
            ));
        }
    }
    Ok(out)
}

/// The recorder writes every NaN as the canonical one: V8's own NaN signs depend on its tier.
fn canonical(value: Value) -> Value {
    match value {
        Value::Number(bits) if f64::from_bits(bits).is_nan() => Value::Number(f64::NAN.to_bits()),
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

fn compare_calls(expected: &Value, canvas: &Canvas2d) -> Result<usize, String> {
    let actual = asrs_canvas::trace::encode(&canvas.calls)
        .lines()
        .map(|line| {
            asrs_parity::json::parse(line)
                .map_err(|e| format!("port trace line {line}: {e:?}"))
                .and_then(|json| Value::from_json(&json).map(canonical).map_err(|e| e.0))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let Value::Array(expected) = expected else {
        return Err("calls is not a list".to_owned());
    };
    for (i, (e, a)) in expected.iter().zip(&actual).enumerate() {
        if e != a {
            return Err(format!(
                "canvas call {i}:\n upstream {}\n port     {}",
                e.encode(),
                a.encode()
            ));
        }
    }
    if expected.len() != actual.len() {
        return Err(format!(
            "upstream made {} canvas calls, the port {}",
            expected.len(),
            actual.len()
        ));
    }
    Ok(expected.len())
}

fn single_read(reads: &[(String, Value)], name: &str) -> Result<f64, String> {
    let mut values = reads.iter().filter(|(n, _)| n == name).map(|(_, v)| v);
    let first = values.next().ok_or_else(|| format!("no {name} read"))?;
    if values.any(|v| v != first) {
        return Err(format!("{name} reads disagree"));
    }
    number(first)
}

enum Port {
    Swerve(SwerveRenderer),
    Joysticks(JoysticksRenderer),
}

struct Harness {
    kind: &'static str,
    ports: Vec<Port>,
    assets: Option<AdvantageScopeAssets>,
    seen_assets: bool,
}

impl Harness {
    fn create(&mut self, record: &Record) -> Result<(), String> {
        let id = number_field(record, "id")? as usize;
        if id != self.ports.len() {
            return Err(format!("create {id} out of order"));
        }
        if value(record, "state")? != Value::Null {
            return Err("a renderer saved a state".to_owned());
        }
        let port = match self.kind {
            "swerve" => Port::Swerve(SwerveRenderer::new(Rc::new(RefCell::new(
                SwerveDom::default(),
            )))),
            _ => {
                let mut dom = JoysticksDom::default();
                for (path, size) in IMAGES {
                    dom.images.insert(JsString::from(path), size);
                }
                Port::Joysticks(JoysticksRenderer::new(Rc::new(RefCell::new(dom))))
            }
        };
        self.ports.push(port);
        Ok(())
    }

    fn set_assets(&mut self, record: &Record) -> Result<(), String> {
        self.seen_assets = true;
        let recorded = to_js(&value(record, "assets")?)?;
        if recorded.is_null() {
            self.assets = None;
            return Ok(());
        }
        let assets = AdvantageScopeAssets::from_js(&recorded);
        if assets.to_js() != recorded {
            return Err("the recorded assets do not round-trip".to_owned());
        }
        self.assets = Some(assets);
        Ok(())
    }

    fn load(&mut self, record: &Record) -> Result<(), String> {
        let id = number_field(record, "id")? as usize;
        let index = number_field(record, "index")? as usize;
        let Some(Port::Joysticks(renderer)) = self.ports.get_mut(id) else {
            return Err("load of no joysticks renderer".to_owned());
        };
        match renderer.deliver_image_load() {
            Some(port) if port == index => Ok(()),
            port => Err(format!(
                "image {index} loaded upstream, the port loaded {port:?}"
            )),
        }
    }

    fn render(&mut self, record: &Record, compared: &mut Compared) -> Result<(), String> {
        let id = number_field(record, "id")? as usize;
        let command = to_js(&value(record, "command")?)?;
        let dark = value(record, "dark")?
            .as_bool()
            .ok_or("dark is not a boolean")?;
        let dpr = number_field(record, "dpr")?;
        let reads = pairs(record, "reads")?;
        let client_size = [
            single_read(&reads, "clientWidth")?,
            single_read(&reads, "clientHeight")?,
        ];
        let style = pairs(record, "style")?;

        let (result, painted, port_style, aspect) = match self.ports.get_mut(id) {
            Some(Port::Swerve(renderer)) => {
                {
                    let mut dom = renderer.dom().borrow_mut();
                    dom.client_size = client_size;
                    dom.device_pixel_ratio = dpr;
                    dom.canvas_size = None;
                }
                let result = renderer.render_command(dark, &command);
                let mut dom = renderer.dom().borrow_mut();
                let style = match dom.canvas_style() {
                    Some([w, h]) => vec![("width", w), ("height", h)],
                    None => Vec::new(),
                };
                (result, dom.painted.take(), style, Some(1.0))
            }
            Some(Port::Joysticks(renderer)) => {
                if !self.seen_assets {
                    return Err("render before the assets".to_owned());
                }
                let loads = number_field(record, "loads")?;
                if loads != renderer.image_load_count() as f64 {
                    return Err(format!(
                        "imageLoadCount upstream {loads}, port {}",
                        renderer.image_load_count()
                    ));
                }
                {
                    let mut dom = renderer.dom().borrow_mut();
                    dom.client_size = client_size;
                    dom.device_pixel_ratio = dpr;
                }
                let result = renderer.render_command(dark, self.assets.as_ref(), &command);
                let painted = renderer.dom().borrow_mut().painted.take();
                (result, painted, Vec::new(), None)
            }
            None => return Err("render before create".to_owned()),
        };

        let recorded_aspect = match value(record, "aspect")? {
            Value::Null => None,
            v => Some(number(&v)?),
        };
        if recorded_aspect != aspect {
            return Err(format!(
                "aspect ratio upstream {recorded_aspect:?}, port {aspect:?}"
            ));
        }

        let threw = result.err().map(|e| error_text(&e));
        let recorded_threw = match record.field("threw") {
            None => None,
            Some(_) => Some(string(&value(record, "threw")?)?),
        };
        if threw != recorded_threw {
            return Err(format!("threw upstream {recorded_threw:?}, port {threw:?}"));
        }
        if threw.is_some() {
            count(compared, "throws");
        }

        let port_style: Vec<(String, Value)> = port_style
            .into_iter()
            .map(|(name, text)| (name.to_owned(), Value::string(&text)))
            .collect();
        if style != port_style {
            return Err(format!(
                "canvas style upstream {style:?}, port {port_style:?}"
            ));
        }
        match (record.field("calls"), painted) {
            (None, None) => count(compared, "kept frames"),
            (Some(_), None) => {
                return Err("upstream painted a frame, the port kept the last".to_owned());
            }
            (None, Some(_)) => {
                return Err("the port painted a frame, upstream kept the last".to_owned());
            }
            (Some(_), Some(canvas)) => {
                let size = Value::Array(
                    [canvas.width, canvas.height]
                        .map(|n| Value::Number(f64::from(n).to_bits()))
                        .to_vec(),
                );
                let expected_size = value(record, "size")?;
                if size != expected_size {
                    return Err(format!(
                        "canvas size upstream {}, port {}",
                        expected_size.encode(),
                        size.encode()
                    ));
                }
                let calls = compare_calls(&value(record, "calls")?, &canvas)?;
                count(compared, "frames");
                *compared.entry("calls").or_insert(0) += calls;
            }
        }
        count(compared, "renders");
        Ok(())
    }
}

fn run_case(id: &str, kind: &'static str, compared: &mut Compared) -> Result<(), String> {
    let mut harness = Harness {
        kind,
        ports: Vec::new(),
        assets: None,
        seen_assets: false,
    };
    let records = r07_records(id)?;
    let mut here = Compared::new();
    let mut recorded = Compared::new();
    for record in &records {
        let at = |e: String| format!("{id} line {}: {e}", record.line);
        match record.string("r07").as_deref() {
            Some("assets") if kind == "joysticks" => harness.set_assets(record).map_err(at)?,
            Some("load") if kind == "joysticks" => {
                harness.load(record).map_err(at)?;
                count(&mut here, "loads");
            }
            Some(call @ ("create" | "render"))
                if record.string("kind").as_deref() == Some(kind) =>
            {
                if call == "create" {
                    harness.create(record).map_err(at)?;
                } else {
                    harness.render(record, &mut here).map_err(at)?;
                    count(&mut recorded, "renders");
                    if record.field("calls").is_some() {
                        count(&mut recorded, "frames");
                    }
                    if record.field("threw").is_some() {
                        count(&mut recorded, "throws");
                    }
                }
            }
            other => return Err(at(format!("unknown r07 record {other:?}"))),
        }
    }
    for key in ["renders", "frames", "throws"] {
        if recorded.get(key) != here.get(key) {
            return Err(format!(
                "{id}: compared {:?} {key}, upstream recorded {:?}",
                here.get(key),
                recorded.get(key)
            ));
        }
    }
    if !here.contains_key("frames") {
        return Err(format!("{id}: no frame was compared"));
    }
    for port in &mut harness.ports {
        if let Port::Joysticks(renderer) = port
            && let Some(index) = renderer.deliver_image_load()
        {
            return Err(format!(
                "{id}: the port loads image {index}, upstream does not"
            ));
        }
    }
    for (key, n) in here {
        *compared.entry(key).or_insert(0) += n;
    }
    Ok(())
}

fn run(test: &str, area: &str, kind: &'static str, at_least: &[(&str, usize)]) {
    let mut failures = Vec::new();
    let mut compared = Compared::new();
    for id in case_ids(area) {
        if let Err(e) = run_case(&id, kind, &mut compared) {
            failures.push(e);
        }
    }
    report(test, &compared, &failures);
    for (key, n) in at_least {
        assert!(
            compared.get(key).copied().unwrap_or(0) >= *n,
            "{test}: too few {key} compared"
        );
    }
}

#[test]
fn swerve_render() {
    run(
        "swerve_render",
        "dom/swerve",
        "swerve",
        &[("frames", 300), ("throws", 25), ("kept frames", 80)],
    );
}

#[test]
fn joysticks_render() {
    run(
        "joysticks_render",
        "dom/joysticks",
        "joysticks",
        &[
            ("frames", 250),
            ("throws", 20),
            ("kept frames", 40),
            ("loads", 100),
        ],
    );
}
