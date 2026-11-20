//! The draw-call trace: one CVE-1 line per call, encoding the JS array
//! `[method, ...arguments]` (`["=", property, value]` for assignments) exactly as the oracle's
//! recording canvas writes it. Numbers are IEEE-754 bit patterns, so traces compare byte for
//! byte. Gradients are `{linearGradient | radialGradient: [...], stops: [[offset, color]...]}`,
//! images are `"@<id>"`, and inline image data is `{width, height, data: Uint8Array}`.

use std::fmt::Write;

use asrs_js::json;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use crate::display::{Call, Gradient, GradientShape, ImageData, ImageDataSource, Property, Style};

pub fn encode(calls: &[Call]) -> String {
    let mut out = String::new();
    for call in calls {
        encode_value(&call_to_value(call), &mut out);
        out.push('\n');
    }
    out
}

pub fn decode(text: &str) -> Result<Vec<Call>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .map(|(i, line)| {
            let value = json::parse_str(line).map_err(|e| format!("line {}: {e:?}", i + 1))?;
            let value = from_cve1(&value).map_err(|e| format!("line {}: {e}", i + 1))?;
            call_from_value(&value).map_err(|e| format!("line {}: {e}", i + 1))
        })
        .collect()
}

fn num(x: f64) -> JsValue {
    JsValue::Number(x)
}

fn string(s: &str) -> JsValue {
    JsValue::string(s)
}

fn array(items: impl IntoIterator<Item = JsValue>) -> JsValue {
    JsValue::Array(items.into_iter().collect())
}

fn style_value(style: &Style) -> JsValue {
    match style {
        Style::Color(css) => string(css),
        Style::Gradient(g) => {
            let (key, params): (&str, &[f64]) = match &g.shape {
                GradientShape::Linear(p) => ("linearGradient", p),
                GradientShape::Radial(p) => ("radialGradient", p),
            };
            let stops = array(
                g.stops
                    .iter()
                    .map(|(offset, css)| array([num(*offset), string(css)])),
            );
            let object: JsObject = [
                (key, array(params.iter().copied().map(num))),
                ("stops", stops),
            ]
            .into_iter()
            .collect();
            JsValue::Object(object)
        }
    }
}

fn property_value(p: &Property) -> JsValue {
    match p {
        Property::FillStyle(s) | Property::StrokeStyle(s) => style_value(s),
        Property::LineWidth(v)
        | Property::MiterLimit(v)
        | Property::LineDashOffset(v)
        | Property::GlobalAlpha(v) => num(*v),
        Property::LineCap(s)
        | Property::LineJoin(s)
        | Property::Font(s)
        | Property::TextAlign(s)
        | Property::TextBaseline(s) => string(s),
        Property::ImageSmoothingEnabled(b) => JsValue::Bool(*b),
    }
}

pub fn call_to_value(call: &Call) -> JsValue {
    let named = |name: &str, args: &[f64]| {
        array(
            [string(name)]
                .into_iter()
                .chain(args.iter().copied().map(num)),
        )
    };
    match call {
        Call::Set(p) => array([string("="), string(p.name()), property_value(p)]),
        Call::Save => named("save", &[]),
        Call::Restore => named("restore", &[]),
        Call::Scale(x, y) => named("scale", &[*x, *y]),
        Call::Rotate(a) => named("rotate", &[*a]),
        Call::Translate(x, y) => named("translate", &[*x, *y]),
        Call::BeginPath => named("beginPath", &[]),
        Call::ClosePath => named("closePath", &[]),
        Call::MoveTo(x, y) => named("moveTo", &[*x, *y]),
        Call::LineTo(x, y) => named("lineTo", &[*x, *y]),
        Call::Rect(x, y, w, h) => named("rect", &[*x, *y, *w, *h]),
        Call::Arc {
            x,
            y,
            radius,
            start,
            end,
            anticlockwise,
        } => {
            let mut v = named("arc", &[*x, *y, *radius, *start, *end]);
            if let (Some(ccw), JsValue::Array(items)) = (anticlockwise, &mut v) {
                items.push(JsValue::Bool(*ccw));
            }
            v
        }
        Call::Ellipse {
            x,
            y,
            radius_x,
            radius_y,
            rotation,
            start,
            end,
            anticlockwise,
        } => {
            let mut v = named(
                "ellipse",
                &[*x, *y, *radius_x, *radius_y, *rotation, *start, *end],
            );
            if let (Some(ccw), JsValue::Array(items)) = (anticlockwise, &mut v) {
                items.push(JsValue::Bool(*ccw));
            }
            v
        }
        Call::Fill => named("fill", &[]),
        Call::Stroke => named("stroke", &[]),
        Call::FillRect(x, y, w, h) => named("fillRect", &[*x, *y, *w, *h]),
        Call::StrokeRect(x, y, w, h) => named("strokeRect", &[*x, *y, *w, *h]),
        Call::ClearRect(x, y, w, h) => named("clearRect", &[*x, *y, *w, *h]),
        Call::FillText {
            text,
            x,
            y,
            max_width,
        } => array(
            [
                string("fillText"),
                JsValue::String(text.clone()),
                num(*x),
                num(*y),
            ]
            .into_iter()
            .chain(max_width.map(num)),
        ),
        Call::SetLineDash(segments) => array([
            string("setLineDash"),
            array(segments.iter().copied().map(num)),
        ]),
        Call::DrawImage { image, args } => array(
            [string("drawImage"), string(&format!("@{image}"))]
                .into_iter()
                .chain(args.iter().copied().map(num)),
        ),
        Call::GetImageData {
            sx,
            sy,
            sw,
            sh,
            register,
        } => array([
            string("getImageData"),
            num(*sx),
            num(*sy),
            num(*sw),
            num(*sh),
            string(register),
        ]),
        Call::PutImageData { data, dx, dy } => {
            let data = match data {
                ImageDataSource::Register(name) => string(name),
                ImageDataSource::Inline(image) => JsValue::Object(
                    [
                        ("width", num(f64::from(image.width))),
                        ("height", num(f64::from(image.height))),
                        ("data", JsValue::Bytes(image.data.clone())),
                    ]
                    .into_iter()
                    .collect(),
                ),
            };
            array([string("putImageData"), data, num(*dx), num(*dy)])
        }
    }
}

fn hex(bytes: &[u8], out: &mut String) {
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
}

fn encode_value(v: &JsValue, out: &mut String) {
    match v {
        JsValue::Undefined => out.push_str(r#"{"u":null}"#),
        JsValue::Null => out.push_str("null"),
        JsValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsValue::Number(x) => {
            let _ = write!(out, r#"{{"n":"{:016x}"}}"#, x.to_bits());
        }
        JsValue::String(s) => json::quote_into(s.units(), out),
        JsValue::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_value(item, out);
            }
            out.push(']');
        }
        JsValue::Object(object) => {
            out.push_str(r#"{"o":["#);
            for (i, (key, value)) in object.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('[');
                json::quote_into(key.units(), out);
                out.push(',');
                encode_value(value, out);
                out.push(']');
            }
            out.push_str("]}");
        }
        JsValue::Bytes(bytes) => {
            out.push_str(r#"{"b":""#);
            hex(bytes, out);
            out.push_str("\"}");
        }
        JsValue::BigInt(_) | JsValue::Date(_) => {
            unreachable!("canvas calls carry no BigInt or Date")
        }
    }
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("bad hex {s:?}"));
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Turns CVE-1 tagged JSON back into the plain JS value it encodes.
fn from_cve1(v: &JsValue) -> Result<JsValue, String> {
    match v {
        JsValue::Array(items) => Ok(JsValue::Array(
            items.iter().map(from_cve1).collect::<Result<_, _>>()?,
        )),
        JsValue::Object(object) => {
            let mut entries = object.iter();
            let (Some((tag, inner)), None) = (entries.next(), entries.next()) else {
                return Err("an object is not a CVE-1 tag".to_owned());
            };
            let tag = tag.to_string_lossy();
            let text = || {
                inner
                    .as_str()
                    .map(JsString::to_string_lossy)
                    .ok_or_else(|| format!("CVE-1 {tag} needs a string"))
            };
            match tag.as_str() {
                "u" => Ok(JsValue::Undefined),
                "n" => {
                    let bits = u64::from_str_radix(&text()?, 16).map_err(|e| e.to_string())?;
                    Ok(JsValue::Number(f64::from_bits(bits)))
                }
                "b" => Ok(JsValue::Bytes(unhex(&text()?)?)),
                "o" => {
                    let pairs = inner.as_array().ok_or("CVE-1 o needs pairs")?;
                    let mut object = JsObject::default();
                    for pair in pairs {
                        let [key, value] = pair.as_array().unwrap_or_default() else {
                            return Err("CVE-1 o pair is not [key, value]".to_owned());
                        };
                        let key = key.as_str().ok_or("CVE-1 o key is not a string")?;
                        object.insert(key.clone(), from_cve1(value)?);
                    }
                    Ok(JsValue::Object(object))
                }
                other => Err(format!("unsupported CVE-1 tag {other}")),
            }
        }
        other => Ok(other.clone()),
    }
}

fn base64(s: &str) -> Result<Vec<u8>, String> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Ok(u32::from(c - b'A')),
        b'a'..=b'z' => Ok(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Ok(u32::from(c - b'0') + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(format!("bad base64 byte {c}")),
    };
    let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    Ok(out)
}

fn arg_num(args: &[JsValue], i: usize) -> Result<f64, String> {
    args.get(i)
        .and_then(JsValue::as_f64)
        .ok_or_else(|| format!("argument {i} is not a number"))
}

fn arg_str(args: &[JsValue], i: usize) -> Result<String, String> {
    args.get(i)
        .and_then(JsValue::as_str)
        .map(JsString::to_string_lossy)
        .ok_or_else(|| format!("argument {i} is not a string"))
}

fn nums<const N: usize>(args: &[JsValue]) -> Result<[f64; N], String> {
    let mut out = [0.0; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = arg_num(args, i)?;
    }
    Ok(out)
}

fn optional_bool(args: &[JsValue], i: usize) -> Result<Option<bool>, String> {
    match args.get(i) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("argument {i} is not a boolean")),
    }
}

fn style_from(v: &JsValue) -> Result<Style, String> {
    if let Some(s) = v.as_str() {
        return Ok(Style::Color(s.to_string_lossy()));
    }
    let shape = if let Some(p) = v.get("linearGradient") {
        GradientShape::Linear(nums(p.as_array().ok_or("linearGradient is not an array")?)?)
    } else if let Some(p) = v.get("radialGradient") {
        GradientShape::Radial(nums(p.as_array().ok_or("radialGradient is not an array")?)?)
    } else {
        return Err("unknown style".to_owned());
    };
    let mut gradient = Gradient {
        shape,
        stops: Vec::new(),
    };
    for stop in v
        .get("stops")
        .and_then(JsValue::as_array)
        .unwrap_or_default()
    {
        let stop = stop.as_array().ok_or("a stop is not an array")?;
        gradient.add_color_stop(arg_num(stop, 0)?, &arg_str(stop, 1)?);
    }
    Ok(Style::Gradient(gradient))
}

fn property_from(name: &str, v: &JsValue) -> Result<Property, String> {
    let number = || v.as_f64().ok_or_else(|| format!("{name} is not a number"));
    let text = || {
        v.as_str()
            .map(JsString::to_string_lossy)
            .ok_or_else(|| format!("{name} is not a string"))
    };
    Ok(match name {
        "fillStyle" => Property::FillStyle(style_from(v)?),
        "strokeStyle" => Property::StrokeStyle(style_from(v)?),
        "lineWidth" => Property::LineWidth(number()?),
        "miterLimit" => Property::MiterLimit(number()?),
        "lineDashOffset" => Property::LineDashOffset(number()?),
        "globalAlpha" => Property::GlobalAlpha(number()?),
        "lineCap" => Property::LineCap(text()?),
        "lineJoin" => Property::LineJoin(text()?),
        "font" => Property::Font(text()?),
        "textAlign" => Property::TextAlign(text()?),
        "textBaseline" => Property::TextBaseline(text()?),
        "imageSmoothingEnabled" => {
            Property::ImageSmoothingEnabled(v.as_bool().ok_or("not a boolean")?)
        }
        other => return Err(format!("unsupported property {other}")),
    })
}

pub fn image_data_from(v: &JsValue) -> Result<ImageDataSource, String> {
    if let Some(name) = v.as_str() {
        return Ok(ImageDataSource::Register(name.to_string_lossy()));
    }
    let dim = |k: &str| {
        v.get(k)
            .and_then(JsValue::as_f64)
            .map(|x| x as u32)
            .ok_or_else(|| format!("image data has no {k}"))
    };
    let data = match (v.get("data"), v.get("rgba")) {
        (Some(JsValue::Bytes(bytes)), _) => bytes.clone(),
        (_, Some(JsValue::String(b64))) => base64(&b64.to_string_lossy())?,
        _ => return Err("image data has no pixels".to_owned()),
    };
    let image = ImageData {
        width: dim("width")?,
        height: dim("height")?,
        data,
    };
    if image.data.len() != image.width as usize * image.height as usize * 4 {
        return Err("image data size does not match its dimensions".to_owned());
    }
    Ok(ImageDataSource::Inline(image))
}

/// One call from its plain JS form (the oracle's scene format, or a decoded trace line).
pub fn call_from_value(v: &JsValue) -> Result<Call, String> {
    let items = v.as_array().ok_or("a call is not an array")?;
    let (name, args) = items.split_first().ok_or("an empty call")?;
    let name = name
        .as_str()
        .ok_or("a call name is not a string")?
        .to_string_lossy();
    let context = |e: String| format!("{name}: {e}");
    let call = (|| {
        Ok(match name.as_str() {
            "=" => Call::Set(property_from(
                &arg_str(args, 0)?,
                args.get(1).ok_or("no value")?,
            )?),
            "save" => Call::Save,
            "restore" => Call::Restore,
            "scale" => {
                let [x, y] = nums(args)?;
                Call::Scale(x, y)
            }
            "rotate" => Call::Rotate(arg_num(args, 0)?),
            "translate" => {
                let [x, y] = nums(args)?;
                Call::Translate(x, y)
            }
            "beginPath" => Call::BeginPath,
            "closePath" => Call::ClosePath,
            "moveTo" => {
                let [x, y] = nums(args)?;
                Call::MoveTo(x, y)
            }
            "lineTo" => {
                let [x, y] = nums(args)?;
                Call::LineTo(x, y)
            }
            "rect" => {
                let [x, y, w, h] = nums(args)?;
                Call::Rect(x, y, w, h)
            }
            "arc" => {
                let [x, y, radius, start, end] = nums(args)?;
                Call::Arc {
                    x,
                    y,
                    radius,
                    start,
                    end,
                    anticlockwise: optional_bool(args, 5)?,
                }
            }
            "ellipse" => {
                let [x, y, radius_x, radius_y, rotation, start, end] = nums(args)?;
                Call::Ellipse {
                    x,
                    y,
                    radius_x,
                    radius_y,
                    rotation,
                    start,
                    end,
                    anticlockwise: optional_bool(args, 7)?,
                }
            }
            "fill" => Call::Fill,
            "stroke" => Call::Stroke,
            "fillRect" | "strokeRect" | "clearRect" => {
                let [x, y, w, h] = nums(args)?;
                match name.as_str() {
                    "fillRect" => Call::FillRect(x, y, w, h),
                    "strokeRect" => Call::StrokeRect(x, y, w, h),
                    _ => Call::ClearRect(x, y, w, h),
                }
            }
            "fillText" => Call::FillText {
                text: args
                    .first()
                    .and_then(JsValue::as_str)
                    .cloned()
                    .ok_or("text is not a string")?,
                x: arg_num(args, 1)?,
                y: arg_num(args, 2)?,
                max_width: args.get(3).map(|_| arg_num(args, 3)).transpose()?,
            },
            "setLineDash" => Call::SetLineDash(
                args.first()
                    .and_then(JsValue::as_array)
                    .ok_or("segments are not an array")?
                    .iter()
                    .map(|v| v.as_f64().ok_or("a segment is not a number"))
                    .collect::<Result<_, _>>()?,
            ),
            "drawImage" => {
                let image = arg_str(args, 0)?;
                let image = image
                    .strip_prefix('@')
                    .ok_or("image is not @<id>")?
                    .to_owned();
                let rest = &args[1..];
                if ![2, 4, 8].contains(&rest.len()) {
                    return Err(format!("{} numeric arguments", rest.len()));
                }
                Call::DrawImage {
                    image,
                    args: (0..rest.len())
                        .map(|i| arg_num(rest, i))
                        .collect::<Result<_, _>>()?,
                }
            }
            "getImageData" => {
                let [sx, sy, sw, sh] = nums(args)?;
                Call::GetImageData {
                    sx,
                    sy,
                    sw,
                    sh,
                    register: arg_str(args, 4)?,
                }
            }
            "putImageData" => Call::PutImageData {
                data: image_data_from(args.first().ok_or("no image data")?)?,
                dx: arg_num(args, 1)?,
                dy: arg_num(args, 2)?,
            },
            other => return Err(format!("unsupported call {other}")),
        })
    })();
    call.map_err(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Canvas2d;

    fn sample() -> Canvas2d {
        let mut c = Canvas2d::new(100, 50);
        c.scale(2.0, 2.0);
        c.set_fill_style("#2b66a2");
        let mut g = Gradient::radial(1.0, 2.0, 0.5, 3.0, 4.0, 5.0);
        g.add_color_stop(0.0, "rgba(0,0,0,1)");
        g.add_color_stop(1.0, "rgba(0,0,0,0)");
        c.set_stroke_style(g);
        c.set_line_width(0.1 + 0.2);
        c.set_image_smoothing_enabled(false);
        c.begin_path();
        c.arc(1.0, 2.0, 3.0, 0.0, std::f64::consts::TAU);
        c.arc_with_direction(1.0, 2.0, 3.0, 0.0, -0.0, true);
        c.ellipse(1.0, 2.0, 3.0, 4.0, 0.5, 0.0, 1.0, None);
        c.stroke();
        c.fill_text("plain", 1.0, 2.0);
        c.fill_text_max_width(
            JsString::from_units(vec![0x41, 0xd800, 0x22]),
            1.0,
            2.0,
            7.5,
        );
        c.set_line_dash(&[5.0, f64::NAN]);
        c.draw_image_source("field", [0.0, 1.0, 2.0, 3.0], [4.0, 5.0, 6.0, 7.0]);
        c.get_image_data(0.0, 0.0, 10.0, 10.0, "$r");
        c.put_image_data(ImageDataSource::Register("$r".to_owned()), 1.0, 2.0);
        c.put_image_data(
            ImageDataSource::Inline(ImageData {
                width: 1,
                height: 1,
                data: vec![1, 2, 3, 4],
            }),
            -1.0,
            f64::INFINITY,
        );
        c
    }

    #[test]
    fn round_trips_bit_exactly() {
        let calls = sample().calls;
        let text = encode(&calls);
        let decoded = decode(&text).unwrap();
        assert_eq!(encode(&decoded), text);
        assert_eq!(format!("{decoded:?}"), format!("{calls:?}"));
    }

    #[test]
    fn numbers_are_bit_patterns() {
        let text = encode(&[Call::Scale(-0.0, 0.30000000000000004)]);
        assert_eq!(
            text,
            "[\"scale\",{\"n\":\"8000000000000000\"},{\"n\":\"3fd3333333333334\"}]\n"
        );
    }

    #[test]
    fn reads_the_scene_format() {
        let scene = r#"["putImageData",{"width":1,"height":1,"rgba":"AQIDBA=="},5,6]"#;
        let call = call_from_value(&json::parse_str(scene).unwrap()).unwrap();
        assert_eq!(
            call,
            Call::PutImageData {
                data: ImageDataSource::Inline(ImageData {
                    width: 1,
                    height: 1,
                    data: vec![1, 2, 3, 4]
                }),
                dx: 5.0,
                dy: 6.0
            }
        );
    }
}
