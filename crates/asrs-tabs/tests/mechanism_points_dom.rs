#![allow(clippy::disallowed_methods)]

//! Replays the R08 vectors written by `oracle/tools/r08Specs.mjs` and recorded in Blink.
//!
//! - `dom/css-style/*`: Blink's serialization of every value the two renderers assign, per
//!   property, on fresh elements (`asrs_tabs::css_value::serialize`, `InlineStyle`), and the
//!   order and timing of the `style` attribute (`dom_tree::Element`).
//! - `dom/mechanism/*`, `dom/points/*`: every `render` of every upstream Mechanism and Points
//!   renderer with the container reads it made, each style assignment as written, the element
//!   tree after it, its throw and `getAspectRatio()`. The geometry tests check each helper
//!   layer on its own against those records (render size and SVG coordinates, the points
//!   layout, positions, colours and transforms); the render tests replay the records in order
//!   through the port's renderers and compare the whole tree, the throw and the aspect ratio.
//!
//! Records the harness does not know fail, and each test checks it compared every record of
//! its kind.

use std::collections::BTreeMap;

use asrs_canvas::color;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_parity::vectors::{load, vectors_dir};
use asrs_parity::{Json, Record, Utf16String, Value};
use asrs_tabs::css_value;
use asrs_tabs::dom_tree::{Element, Namespace};
use asrs_tabs::mechanism_renderer::{self, MechanismRenderer};
use asrs_tabs::points_renderer::{self, PointsRenderer};

const OTHER_KINDS: &[&str] = &["read", "write", "main", "exception", "capture", "eval"];

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
        Value::Undefined | Value::Hole => JsValue::Undefined,
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

fn number_value(value: &Value) -> Result<f64, String> {
    value
        .as_f64()
        .ok_or_else(|| format!("expected a number, got {}", value.kind()))
}

fn string_value(value: &Value) -> Result<String, String> {
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

fn error_text(e: &JsError) -> String {
    format!("{}: {}", e.name, e.message)
}

fn same_number(expected: f64, actual: f64) -> bool {
    expected.to_bits() == actual.to_bits() || (expected.is_nan() && actual.is_nan())
}

/// `[localName, [[name, value], ...], [children...]]`, syncing each element's `style`
/// attribute as the recorder's `getAttributeNames()` does.
fn tree(el: &mut Element) -> Json {
    let attributes = el
        .attributes()
        .iter()
        .map(|(n, v)| Json::Array(vec![Json::string(n), Json::string(v)]))
        .collect();
    let tag = Json::string(el.tag());
    let children = el.children_mut().iter_mut().map(tree).collect();
    Json::Array(vec![tag, Json::Array(attributes), Json::Array(children)])
}

/// The record's element tree as `(tag, attributes, children)`.
struct Node {
    tag: String,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
}

fn node(json: &Json) -> Result<Node, String> {
    let parts = json.as_array().ok_or("dom node is not an array")?;
    let [tag, attributes, children] = parts else {
        return Err("dom node needs three parts".to_owned());
    };
    let attributes = attributes
        .as_array()
        .ok_or("dom attributes are not an array")?
        .iter()
        .map(|pair| match pair.as_array() {
            Some([n, v]) => Ok((json_string(n)?, json_string(v)?)),
            _ => Err("dom attribute is not a pair".to_owned()),
        })
        .collect::<Result<_, String>>()?;
    let children = children
        .as_array()
        .ok_or("dom children are not an array")?
        .iter()
        .map(node)
        .collect::<Result<_, _>>()?;
    Ok(Node {
        tag: json_string(tag)?,
        attributes,
        children,
    })
}

impl Node {
    fn attribute(&self, name: &str) -> Result<&str, String> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| format!("<{}> has no {name:?} attribute", self.tag))
    }
}

/// One `{"r08": "render"}` record, decoded.
struct Render<'a> {
    record: &'a Record,
    id: usize,
    command: Value,
    dark: bool,
    reads: Vec<(String, Value)>,
    assigns: Vec<(Who, String, Value)>,
    threw: Option<String>,
    created: usize,
    aspect: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Root,
    Created(usize),
}

fn decode_render(record: &Record) -> Result<Render<'_>, String> {
    let id = number_value(&record.value("id").map_err(|e| e.to_string())?)? as usize;
    let command = record.value("command").map_err(|e| e.to_string())?;
    let dark = record
        .value("dark")
        .map_err(|e| e.to_string())?
        .as_bool()
        .ok_or("dark is not a boolean")?;
    let reads = record
        .field("reads")
        .and_then(Json::as_array)
        .ok_or("render without reads")?
        .iter()
        .map(|r| match r.as_array() {
            Some([name, value]) => Ok((
                json_string(name)?,
                Value::from_json(value).map_err(|e| e.to_string())?,
            )),
            _ => Err("read is not a pair".to_owned()),
        })
        .collect::<Result<_, String>>()?;
    let assigns = record
        .field("assigns")
        .and_then(Json::as_array)
        .ok_or("render without assigns")?
        .iter()
        .map(|a| match a.as_array() {
            Some([who, prop, value]) => {
                let who = match who {
                    Json::Number(n) => Who::Created(*n as usize),
                    w if json_string(w).as_deref() == Ok("root") => Who::Root,
                    w => return Err(format!("unknown assignment target {}", w.to_compact())),
                };
                Ok((
                    who,
                    json_string(prop)?,
                    Value::from_json(value).map_err(|e| e.to_string())?,
                ))
            }
            _ => Err("assignment is not a triple".to_owned()),
        })
        .collect::<Result<_, String>>()?;
    let threw = match record.field("threw") {
        None => None,
        Some(_) => Some(string_value(
            &record.value("threw").map_err(|e| e.to_string())?,
        )?),
    };
    let created = record
        .field("created")
        .and_then(Json::as_f64)
        .ok_or("render without created")? as usize;
    let aspect = number_value(&record.value("aspect").map_err(|e| e.to_string())?)?;
    Ok(Render {
        record,
        id,
        command,
        dark,
        reads,
        assigns,
        threw,
        created,
        aspect,
    })
}

impl Render<'_> {
    fn dom(&self) -> Result<Node, String> {
        node(self.record.field("dom").ok_or("render without dom")?)
    }

    /// The single value of a container read, checking every read of it agreed.
    fn read(&self, name: &str) -> Result<Value, String> {
        let mut values = self.reads.iter().filter(|(n, _)| n == name).map(|(_, v)| v);
        let first = values
            .next()
            .ok_or_else(|| format!("no {name} read"))?
            .clone();
        if values.any(|v| v != &first) {
            return Err(format!("{name} reads disagree"));
        }
        Ok(first)
    }

    fn client_size(&self) -> Result<[f64; 2], String> {
        Ok([
            number_value(&self.read("clientWidth")?)?,
            number_value(&self.read("clientHeight")?)?,
        ])
    }

    fn rect_size(&self) -> Result<[f64; 2], String> {
        let rect = self.read("rect")?;
        match rect.as_array() {
            Some([_, _, w, h]) => Ok([number_value(w)?, number_value(h)?]),
            _ => Err("rect read is not [x, y, w, h]".to_owned()),
        }
    }

    fn assigned(&self, who: Who) -> Vec<(String, Value)> {
        self.assigns
            .iter()
            .filter(|(w, _, _)| *w == who)
            .map(|(_, p, v)| (p.clone(), v.clone()))
            .collect()
    }
}

/// Every record of a vector, split into `r08` records and checked others.
fn r08_records(id: &str) -> Result<Vec<Record>, String> {
    let records = load(id)?;
    let mut out = Vec::new();
    for record in records {
        if record.field("r08").is_some() {
            out.push(record);
            continue;
        }
        let kind = match &record.json {
            Json::Object(pairs) => pairs
                .first()
                .and_then(|(k, _)| k.as_string())
                .unwrap_or_default(),
            _ => String::new(),
        };
        if !OTHER_KINDS.contains(&kind.as_str()) {
            return Err(format!(
                "{id} line {}: unknown record {}",
                record.line, record.text
            ));
        }
    }
    Ok(out)
}

fn render_records(id: &str, kind: &str) -> Result<Vec<Record>, String> {
    Ok(r08_records(id)?
        .into_iter()
        .filter(|r| {
            r.string("r08").as_deref() == Some("render")
                && r.string("kind").as_deref() == Some(kind)
        })
        .collect())
}

fn report(test: &str, compared: &BTreeMap<&str, usize>, failures: &[String]) {
    eprintln!("{test}: compared {compared:?}, {} failing", failures.len());
    let shown: Vec<&String> = failures.iter().take(40).collect();
    assert!(
        failures.is_empty(),
        "{test}: {} failures, first {}:\n{}",
        failures.len(),
        shown.len(),
        shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn eval_value(id: &str) -> Result<Value, String> {
    let records = load(id)?;
    let mut values = Vec::new();
    for record in &records {
        if record.field("eval").is_some() {
            values.push(record.value("value").map_err(|e| e.to_string())?);
        } else if record.field("capture").is_none() {
            return Err(format!(
                "{id} line {}: unknown record {}",
                record.line, record.text
            ));
        }
    }
    match values.as_slice() {
        [v] => Ok(v.clone()),
        _ => Err(format!(
            "{id}: expected one eval record, found {}",
            values.len()
        )),
    }
}

/// What a style assignment of `raw` hands `setProperty`: the recorded value is a string,
/// `null` (which becomes "") or a number (`ToString`).
fn assigned_text(raw: &Value) -> Result<String, String> {
    Ok(match raw {
        Value::Null => String::new(),
        Value::Number(bits) => number::to_string(f64::from_bits(*bits)),
        other => string_value(other)?,
    })
}

fn namespace(name: &str) -> Namespace {
    match name {
        "svg" => Namespace::Svg,
        "html" => Namespace::Html,
        other => panic!("unknown namespace {other}"),
    }
}

/// `[input, getPropertyValue, style attribute or null]` against a fresh `<tag>`.
fn check_value(
    property: &str,
    ns: Namespace,
    tag: &str,
    row: &Value,
    failures: &mut Vec<String>,
    id: &str,
) -> Result<(), String> {
    let Some([input, out, attr]) = row.as_array() else {
        return Err(format!("{id}: row is not a triple"));
    };
    let input_text = assigned_text(input)?;
    let out = string_value(out)?;
    let attr = match attr {
        Value::Null => None,
        a => Some(string_value(a)?),
    };
    let quirks = ns == Namespace::Svg;
    if !input_text.is_empty() {
        let expected = Ok((!out.is_empty()).then(|| out.clone()));
        let actual = css_value::serialize(property, &input_text, quirks);
        if actual != expected {
            failures.push(format!(
                "{id}: serialize({property:?}, {input_text:?}, quirks {quirks}) upstream {expected:?}, port {actual:?}"
            ));
        }
    }
    let mut el = Element::new(ns, tag);
    if let Err(e) = el.set_style(property, &input_text) {
        failures.push(format!(
            "{id}: <{tag}> style {property} = {input_text:?}: {e:?}"
        ));
    }
    let got = el.style().get(property).to_owned();
    let got_attr = el
        .attributes()
        .iter()
        .find(|(n, _)| n == "style")
        .map(|(_, v)| v.clone());
    if got != out || got_attr != attr {
        failures.push(format!(
            "{id}: <{tag}> style {property} = {input_text:?}: upstream {out:?} / {attr:?}, port {got:?} / {got_attr:?}"
        ));
    }
    Ok(())
}

#[test]
fn css_style_values() {
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for case in ["lengths", "stroke-width", "colors", "keywords"] {
        let id = format!("dom/css-style/{case}");
        let value = eval_value(&id).unwrap();
        for group in value.as_array().expect("property groups") {
            let Some([property, ns, tag, rows]) = group.as_array() else {
                panic!("{id}: group is not [property, namespace, tag, rows]");
            };
            let property = string_value(property).unwrap();
            let ns = namespace(&string_value(ns).unwrap());
            let tag = string_value(tag).unwrap();
            for row in rows.as_array().expect("rows") {
                check_value(&property, ns, &tag, row, &mut failures, &id).unwrap();
                *compared.entry(case).or_insert(0) += 1;
            }
        }
    }
    assert!(
        compared.values().sum::<usize>() > 900,
        "too few css values compared"
    );
    report("css_style_values", &compared, &failures);
}

/// Every colour string of `dom/css-style/color-syntax` on `stroke`, `fill` and an SVG's
/// `background-color`: the port equals Blink or, for the "open" rows only, reports the value as
/// unmodeled. Where Blink computes a colour, `asrs_canvas::color` equals its canvas `fillStyle`
/// and pixel too; where Blink rejects it, so does the canvas parser.
#[test]
fn css_color_syntax() {
    let id = "dom/css-style/color-syntax";
    let value = eval_value(id).unwrap();
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for row in value.as_array().expect("rows") {
        let Some([kind, input, stroke, fill, background, fill_style, pixel]) = row.as_array()
        else {
            panic!(
                "{id}: row is not [kind, value, stroke, fill, background-color, fillStyle, pixel]"
            );
        };
        let kind = string_value(kind).unwrap();
        let input = string_value(input).unwrap();
        let mut stroke_ok = None;
        for (property, recorded) in [
            ("stroke", stroke),
            ("fill", fill),
            ("background-color", background),
        ] {
            let recorded = string_value(recorded).unwrap();
            match css_value::serialize(property, &input, true) {
                Ok(port) if port.as_deref().unwrap_or("") == recorded => {
                    *compared.entry("style").or_insert(0) += 1;
                    if property == "stroke" {
                        stroke_ok = Some(recorded);
                    }
                }
                Ok(port) => failures.push(format!(
                    "{id}: {property} {input:?}: upstream {recorded:?}, port {port:?}"
                )),
                Err(_) if kind == "open" => *compared.entry("style unmodeled").or_insert(0) += 1,
                Err(e) => failures.push(format!("{id}: {property} {input:?} ({kind}): {e:?}")),
            }
        }
        let Some(stroke) = stroke_ok else {
            continue;
        };
        let fill_style = string_value(fill_style).unwrap();
        let pixel: Vec<u8> = pixel
            .as_array()
            .expect("pixel")
            .iter()
            .map(|v| v.as_f64().expect("pixel byte") as u8)
            .collect();
        let port = color::parse_color(&input);
        if stroke.is_empty() {
            *compared.entry("canvas rejected").or_insert(0) += 1;
            if port.is_some() || fill_style != "#010203" {
                failures.push(format!(
                    "{id}: canvas {input:?}: upstream {fill_style:?}, port {port:?}"
                ));
            }
        } else if stroke.starts_with("rgb") {
            *compared.entry("canvas").or_insert(0) += 1;
            let Some(port) = port else {
                failures.push(format!(
                    "{id}: canvas {input:?}: upstream {fill_style:?}, port None"
                ));
                continue;
            };
            let rgba = port.to_rgba();
            let [r, g, b] = rgba.to_u8();
            let alpha = (rgba.a * 255.0).round() as u8;
            let matches = if rgba.a >= 1.0 {
                fill_style == format!("#{r:02x}{g:02x}{b:02x}") && pixel == [r, g, b, 255]
            } else {
                pixel[3] == alpha
            };
            if !matches {
                failures.push(format!(
                    "{id}: canvas {input:?}: upstream {fill_style:?} {pixel:?}, port {:?} alpha {alpha}",
                    [r, g, b]
                ));
            }
        }
    }
    assert!(
        compared.get("canvas").copied().unwrap_or(0) > 4000,
        "too few canvas colours compared"
    );
    report("css_color_syntax", &compared, &failures);
}

#[test]
fn css_style_declarations() {
    let id = "dom/css-style/declarations";
    let value = eval_value(id).unwrap();
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for case in value.as_array().expect("scripts") {
        let Some([script, trace]) = case.as_array() else {
            panic!("{id}: case is not [script, trace]");
        };
        let steps = script.as_array().expect("script steps");
        let make = steps[0].as_array().expect("element step");
        let mut el = Element::new(
            namespace(&string_value(&make[0]).unwrap()),
            &string_value(&make[1]).unwrap(),
        );
        let mut got = Vec::new();
        for step in &steps[1..] {
            let step = step.as_array().expect("step");
            match string_value(&step[0]).unwrap().as_str() {
                "attr" => el.set_attribute(
                    &string_value(&step[1]).unwrap(),
                    &string_value(&step[2]).unwrap(),
                ),
                "style" => el
                    .set_style(
                        &string_value(&step[1]).unwrap(),
                        &string_value(&step[2]).unwrap(),
                    )
                    .unwrap(),
                "sync" => {
                    let attr = el
                        .attributes()
                        .iter()
                        .find(|(n, _)| n == "style")
                        .map(|(_, v)| v.clone());
                    got.push(attr.map_or(Value::Null, |v| Value::string(&v)));
                }
                other => panic!("{id}: unknown step {other}"),
            }
        }
        let attributes = el
            .attributes()
            .iter()
            .map(|(n, v)| Value::Array(vec![Value::string(n), Value::string(v)]))
            .collect();
        got.push(Value::Array(attributes));
        if &Value::Array(got.clone()) != trace {
            failures.push(format!(
                "{id}: {}\n  upstream {}\n  port     {}",
                script.encode(),
                trace.encode(),
                Value::Array(got).encode()
            ));
        }
        *compared.entry("scripts").or_insert(0) += 1;
    }
    report("css_style_declarations", &compared, &failures);
}

fn mechanism_geometry_case(render: &Render<'_>) -> Result<usize, String> {
    if render.command == Value::Null {
        if !render.reads.is_empty() || render.assigns.len() != 1 {
            return Err(
                "a null command read the container or assigned more than display".to_owned(),
            );
        }
        return Ok(0);
    }
    let command = to_js(&render.command)?;
    let client = render.client_size()?;
    let dimensions = mechanism_renderer::read_number_pair(
        &ops::get(&command, "dimensions").map_err(|e| error_text(&e))?,
    )
    .map_err(|e| error_text(&e))?;
    let size = mechanism_renderer::render_size(client, dimensions);
    let dom = render.dom()?;
    let mut checked = 0;
    for (attr, value) in [("width", size[0]), ("height", size[1])] {
        let expected = dom.attribute(attr)?;
        let actual = number::to_string(value);
        if expected != actual {
            return Err(format!(
                "render_size {client:?} {dimensions:?}: svg {attr} upstream {expected}, port {actual}"
            ));
        }
        checked += 1;
    }
    let background = ops::get(&command, "backgroundColor").map_err(|e| error_text(&e))?;
    let root = render.assigned(Who::Root);
    let expected_root = vec![
        ("display".to_owned(), Value::string("initial")),
        (
            "backgroundColor".to_owned(),
            background_value(&render.command)?,
        ),
    ];
    if root != expected_root {
        return Err(format!("root assignments {root:?}"));
    }
    let background_text = mechanism_renderer::style_string(&background);
    if assigned_text(&expected_root[1].1)? != background_text {
        return Err(format!(
            "style_string(backgroundColor) port {background_text:?}"
        ));
    }

    let lines = ops::get(&command, "lines").map_err(|e| error_text(&e))?;
    let lines = lines.as_array().ok_or("lines is not an array")?;
    for (i, line_el) in dom
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.tag == "line")
    {
        let line = &lines[i / 3];
        let start = mechanism_renderer::to_svg(
            mechanism_renderer::read_number_pair(
                &ops::get(line, "start").map_err(|e| error_text(&e))?,
            )
            .map_err(|e| error_text(&e))?,
            dimensions,
            size,
        );
        let end = mechanism_renderer::to_svg(
            mechanism_renderer::read_number_pair(
                &ops::get(line, "end").map_err(|e| error_text(&e))?,
            )
            .map_err(|e| error_text(&e))?,
            dimensions,
            size,
        );
        for (attr, value) in [
            ("x1", start[0]),
            ("y1", start[1]),
            ("x2", end[0]),
            ("y2", end[1]),
        ] {
            let expected = line_el.attribute(attr)?;
            let actual = number::to_string(value);
            if expected != actual {
                return Err(format!(
                    "line {} {attr}: upstream {expected}, port {actual}",
                    i / 3
                ));
            }
            checked += 1;
        }
        for (offset, point) in [(1, start), (2, end)] {
            let Some(circle) = dom.children.get(i + offset) else {
                continue;
            };
            for (attr, value) in [("cx", point[0]), ("cy", point[1])] {
                let expected = circle.attribute(attr)?;
                let actual = number::to_string(value);
                if expected != actual {
                    return Err(format!(
                        "line {} circle {attr}: upstream {expected}, port {actual}",
                        i / 3
                    ));
                }
                checked += 1;
            }
        }
    }
    for k in 0..render.created {
        let line = &lines[k / 3];
        let color =
            mechanism_renderer::style_string(&ops::get(line, "color").map_err(|e| error_text(&e))?);
        for (prop, raw) in render.assigned(Who::Created(k)) {
            let raw_text = assigned_text(&raw)?;
            let expected_text = match prop.as_str() {
                "stroke" | "fill" => color.clone(),
                "strokeWidth" => {
                    ops::to_string(&ops::get(line, "weight").map_err(|e| error_text(&e))?)
                        .to_string()
                }
                other => return Err(format!("unexpected assignment {other}")),
            };
            if raw_text != expected_text {
                return Err(format!(
                    "element {k} {prop}: upstream {raw_text:?}, port {expected_text:?}"
                ));
            }
            checked += 1;
        }
    }
    Ok(checked)
}

fn background_value(command: &Value) -> Result<Value, String> {
    match command.get("backgroundColor") {
        Some(v) => Ok(v.clone()),
        None => Err("command without backgroundColor".to_owned()),
    }
}

#[test]
fn mechanism_geometry() {
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for id in case_ids("dom/mechanism") {
        let records = render_records(&id, "mechanism").unwrap();
        assert!(!records.is_empty(), "{id}: no mechanism renders");
        for record in &records {
            let render = decode_render(record).unwrap();
            match mechanism_geometry_case(&render) {
                Ok(n) => {
                    *compared.entry("renders").or_insert(0) += 1;
                    *compared.entry("values").or_insert(0) += n;
                }
                Err(e) => failures.push(format!("{id} line {}: {e}", record.line)),
            }
        }
    }
    report("mechanism_geometry", &compared, &failures);
}

fn points_geometry_case(render: &Render<'_>) -> Result<usize, String> {
    let command = to_js(&render.command)?;
    let container = render.rect_size()?;
    let dims = ops::get(&command, "dimensions").map_err(|e| error_text(&e))?;
    let dimensions = [
        ops::value_to_number(&ops::index(&dims, 0).map_err(|e| error_text(&e))?)
            .map_err(|e| error_text(&e))?,
        ops::value_to_number(&ops::index(&dims, 1).map_err(|e| error_text(&e))?)
            .map_err(|e| error_text(&e))?,
    ];
    let layout = points_renderer::layout(container, dimensions);
    let mut checked = 0;
    if !same_number(render.aspect, layout.aspect_ratio) {
        return Err(format!(
            "layout {container:?} {dimensions:?}: aspect upstream {}, port {}",
            render.aspect, layout.aspect_ratio
        ));
    }
    let [width, height] = layout.background_size();
    let expected_root = vec![
        (
            "width".to_owned(),
            Value::string(&format!("{}px", number::to_string(width))),
        ),
        (
            "height".to_owned(),
            Value::string(&format!("{}px", number::to_string(height))),
        ),
    ];
    let root = render.assigned(Who::Root);
    if root != expected_root {
        return Err(format!(
            "layout {container:?} {dimensions:?}: background upstream {root:?}, port {expected_root:?}"
        ));
    }
    checked += 2;

    let mut k = 0;
    let mut threw = None;
    let sets = ops::get(&command, "sets").map_err(|e| error_text(&e))?;
    'sets: for set in sets.as_array().ok_or("sets is not an array")? {
        let shape =
            ops::to_string(&ops::get(set, "shape").map_err(|e| error_text(&e))?).to_string();
        let size = ops::to_string(&ops::get(set, "size").map_err(|e| error_text(&e))?).to_string();
        let group_size =
            ops::value_to_number(&ops::get(set, "groupSize").map_err(|e| error_text(&e))?)
                .map_err(|e| error_text(&e))?;
        let points = ops::get(set, "points").map_err(|e| error_text(&e))?;
        for (index, p) in points
            .as_array()
            .ok_or("points is not an array")?
            .iter()
            .enumerate()
        {
            let point = [
                ops::value_to_number(&ops::index(p, 0).map_err(|e| error_text(&e))?)
                    .map_err(|e| error_text(&e))?,
                ops::value_to_number(&ops::index(p, 1).map_err(|e| error_text(&e))?)
                    .map_err(|e| error_text(&e))?,
            ];
            let Some(position) = layout.position(point, dimensions) else {
                continue;
            };
            if points_renderer::point_template(&shape).is_none() {
                return Err(format!("no template for shape {shape:?}"));
            }
            let mut expected = Vec::new();
            if let Some(t) = points_renderer::size_transform(&size) {
                expected.push(("transform".to_owned(), Value::string(t)));
            }
            match points_renderer::point_color(index, group_size, render.dark) {
                Ok(color) => {
                    expected.push(("fill".to_owned(), Value::string(color)));
                    expected.push(("stroke".to_owned(), Value::string(color)));
                    expected.push((
                        "left".to_owned(),
                        Value::string(&format!("{}px", number::to_string(position[0]))),
                    ));
                    expected.push((
                        "top".to_owned(),
                        Value::string(&format!("{}px", number::to_string(position[1]))),
                    ));
                }
                Err(e) => threw = Some(error_text(&e)),
            }
            let actual = render.assigned(Who::Created(k));
            if actual != expected {
                return Err(format!(
                    "set point {index} at {point:?} (group {group_size}, dark {}): upstream {actual:?}, port {expected:?}",
                    render.dark
                ));
            }
            checked += expected.len();
            k += 1;
            if threw.is_some() {
                break 'sets;
            }
        }
    }
    if threw != render.threw {
        return Err(format!(
            "point_color throw upstream {:?}, port {threw:?}",
            render.threw
        ));
    }
    if k != render.created {
        return Err(format!(
            "upstream cloned {} points, port positions {k}",
            render.created
        ));
    }
    Ok(checked)
}

#[test]
fn points_geometry() {
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for id in case_ids("dom/points") {
        let records = render_records(&id, "points").unwrap();
        assert!(!records.is_empty(), "{id}: no points renders");
        for record in &records {
            let render = decode_render(record).unwrap();
            match points_geometry_case(&render) {
                Ok(n) => {
                    *compared.entry("renders").or_insert(0) += 1;
                    *compared.entry("values").or_insert(0) += n;
                }
                Err(e) => failures.push(format!("{id} line {}: {e}", record.line)),
            }
        }
    }
    report("points_geometry", &compared, &failures);
}

enum Port {
    Mechanism(MechanismRenderer),
    Points(PointsRenderer),
}

impl Port {
    fn root(&mut self) -> &mut Element {
        match self {
            Port::Mechanism(r) => r.svg_mut(),
            Port::Points(r) => r.background_mut(),
        }
    }

    fn aspect(&self) -> f64 {
        match self {
            Port::Mechanism(r) => r.aspect_ratio(),
            Port::Points(r) => r.aspect_ratio(),
        }
    }
}

fn compare_state(port: &mut Port, record: &Record, aspect: f64) -> Result<(), String> {
    let expected = record.field("dom").ok_or("record without dom")?;
    let actual = tree(port.root());
    if &actual != expected {
        return Err(format!(
            "tree\n  upstream {}\n  port     {}",
            expected.to_compact(),
            actual.to_compact()
        ));
    }
    if !same_number(aspect, port.aspect()) {
        return Err(format!("aspect upstream {aspect}, port {}", port.aspect()));
    }
    Ok(())
}

fn render_case(
    id: &str,
    kind: &str,
    compared: &mut BTreeMap<&'static str, usize>,
) -> Result<(), String> {
    let mut ports: Vec<Port> = Vec::new();
    let mut renders = 0;
    for record in r08_records(id)? {
        let at = |e: String| format!("{id} line {}: {e}", record.line);
        if record.string("kind").as_deref() != Some(kind) {
            if !matches!(
                record.string("kind").as_deref(),
                Some("mechanism" | "points")
            ) {
                return Err(at("unknown r08 kind".to_owned()));
            }
            continue;
        }
        match record.string("r08").as_deref() {
            Some("create") => {
                let id_value = number_value(&record.value("id").map_err(|e| e.to_string())?)
                    .map_err(at)? as usize;
                if id_value != ports.len() {
                    return Err(at(format!("create {id_value} out of order")));
                }
                let mut port = match kind {
                    "mechanism" => Port::Mechanism(MechanismRenderer::new()),
                    _ => Port::Points(PointsRenderer::new()),
                };
                let aspect = number_value(&record.value("aspect").map_err(|e| e.to_string())?)
                    .map_err(at)?;
                compare_state(&mut port, &record, aspect).map_err(at)?;
                ports.push(port);
                *compared.entry("creates").or_insert(0) += 1;
            }
            Some("render") => {
                let render = decode_render(&record).map_err(at)?;
                let port = ports
                    .get_mut(render.id)
                    .ok_or_else(|| at("render before create".to_owned()))?;
                let command = to_js(&render.command).map_err(at)?;
                let result = match port {
                    Port::Mechanism(r) => {
                        let client = if render.reads.is_empty() {
                            [f64::NAN, f64::NAN]
                        } else {
                            render.client_size().map_err(at)?
                        };
                        r.render_command(client, &command)
                    }
                    Port::Points(r) => {
                        let container = render.rect_size().map_err(at)?;
                        r.render_command(container, render.dark, &command)
                    }
                };
                let threw = result.err().map(|e| error_text(&e));
                if threw != render.threw {
                    return Err(at(format!(
                        "threw upstream {:?}, port {threw:?}",
                        render.threw
                    )));
                }
                compare_state(port, &record, render.aspect).map_err(at)?;
                renders += 1;
                *compared.entry("renders").or_insert(0) += 1;
                if render.threw.is_some() {
                    *compared.entry("throws").or_insert(0) += 1;
                }
            }
            other => return Err(at(format!("unknown r08 record {other:?}"))),
        }
    }
    let recorded = render_records(id, kind)?.len();
    if renders != recorded || renders == 0 {
        return Err(format!(
            "{id}: compared {renders} {kind} renders, upstream recorded {recorded}"
        ));
    }
    Ok(())
}

#[test]
fn mechanism_render() {
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for id in case_ids("dom/mechanism") {
        if let Err(e) = render_case(&id, "mechanism", &mut compared) {
            failures.push(e);
        }
    }
    assert!(
        compared.get("throws").copied().unwrap_or(0) >= 5 || !failures.is_empty(),
        "too few throws compared"
    );
    report("mechanism_render", &compared, &failures);
}

#[test]
fn points_render() {
    let mut failures = Vec::new();
    let mut compared = BTreeMap::new();
    for id in case_ids("dom/points") {
        if let Err(e) = render_case(&id, "points", &mut compared) {
            failures.push(e);
        }
    }
    assert!(
        compared.get("throws").copied().unwrap_or(0) >= 3 || !failures.is_empty(),
        "too few throws compared"
    );
    report("points_render", &compared, &failures);
}
