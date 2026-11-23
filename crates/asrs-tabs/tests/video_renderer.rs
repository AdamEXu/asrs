//! Replays `video/renderer`: upstream's `VideoRenderer` on a Blink `<img>` loading frame files.

use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::record_vectors;
use asrs_parity::vectors::{Record, workspace_root};
use asrs_tabs::video_renderer::VideoRenderer;

fn frames() -> Vec<(String, [f64; 2])> {
    let path = workspace_root().join("parity/specs/video/renderer.entry.json");
    let spec = asrs_parity::json::parse(&std::fs::read_to_string(path).expect("spec"));
    let spec = spec.expect("spec is JSON");
    let Some(Json::Object(frames)) = spec.get("fixture").and_then(|f| f.get("frames")) else {
        panic!("no frames");
    };
    frames
        .iter()
        .map(|(name, size)| {
            let size = size.as_array().expect("size");
            let at = |i: usize| size[i].as_f64().expect("number");
            (name.to_string_lossy(), [at(0), at(1)])
        })
        .collect()
}

fn command(json: &Json) -> JsValue {
    match json {
        Json::Null => JsValue::Null,
        Json::Bool(b) => JsValue::Bool(*b),
        Json::Number(n) => JsValue::Number(*n),
        Json::String(s) if s.eq_str("__undefined") => JsValue::Undefined,
        Json::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
        Json::Array(items) => JsValue::Array(items.iter().map(command).collect()),
        Json::Object(pairs) => {
            let mut object = JsObject::new();
            for (key, value) in pairs {
                object.insert(JsString::from_units(key.units().to_vec()), command(value));
            }
            JsValue::Object(object)
        }
    }
}

fn replay(
    loading: &mut Option<JsString>,
    renderer: &mut VideoRenderer,
    frames: &[(String, [f64; 2])],
    record: &Record,
) -> Result<(), String> {
    let step = record.field("step").ok_or("no step")?;
    if let Some(src) = renderer.image().borrow_mut().begin_load() {
        *loading = Some(src);
    }
    match (step.get("render"), step.get("load")) {
        (Some(json), None) => renderer.render_command(&command(json)),
        (None, Some(Json::Bool(true))) => {
            let src = loading
                .take()
                .ok_or("Blink loaded an image the port did not request")?;
            let name = src.to_string_lossy();
            let size = frames.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);
            renderer.image().borrow_mut().loaded(&src, size);
        }
        _ => return Err("unknown step".to_owned()),
    }
    let image = renderer.image().borrow();
    let expect = |key: &str, actual: String| {
        let expected = record.field(key).ok_or(format!("no {key}"))?.to_compact();
        if expected == actual {
            Ok(())
        } else {
            Err(format!("{key}: {actual}, Blink {expected}"))
        }
    };
    let ratio = renderer.aspect_ratio();
    expect(
        "aspectRatio",
        ratio.map_or("null".into(), |r| Value::number(r).encode()),
    )?;
    expect("hidden", image.hidden().to_string())?;
    expect(
        "src",
        image
            .src()
            .map_or("null".into(), |s| format!("{:?}", s.to_string_lossy())),
    )?;
    let [w, h] = image.natural_size();
    expect("natural", format!("[{w},{h}]"))?;
    let state = TabRenderer::<()>::save_state(renderer, &());
    expect(
        "state",
        if state.is_null() {
            "null".into()
        } else {
            format!("{state:?}")
        },
    )
}

record_vectors!(video_renderer, "video/renderer", {
    let mut renderer = VideoRenderer::new(Rc::default());
    let frames = frames();
    let mut loading = None;
    move |record: &Record| replay(&mut loading, &mut renderer, &frames, record)
});
