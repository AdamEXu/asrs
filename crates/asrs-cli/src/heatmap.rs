use std::fs;
use std::path::Path;

use asrs_canvas::raster::CanvasError;
use asrs_parity::cve1::Value;
use asrs_parity::json::{self, Json};
use asrs_parity::pixel;
use asrs_tabs::heatmap::{Heatmap, SetData, StoreData};

// The oracle-side conversion lives in oracle/src/heatmap: `runUpdates` records
// {created, setData, store, error} per update and canvasMain adds canvas/readError.
// Here we only decode the fixture inputs, drive asrs_tabs::heatmap::Heatmap and
// encode the same records.

fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn set_data_value(set: &SetData) -> Value {
    object(vec![
        ("min", Value::number(set.min)),
        ("max", Value::number(set.max)),
        (
            "data",
            Value::Array(
                set.data
                    .iter()
                    .map(|p| {
                        object(vec![
                            ("x", Value::number(p.x)),
                            ("y", Value::number(p.y)),
                            ("value", Value::number(p.value)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn store_value(store: &StoreData) -> Value {
    object(vec![
        ("min", Value::number(store.min)),
        ("max", Value::number(store.max)),
        (
            "data",
            Value::Array(
                store
                    .data
                    .iter()
                    .map(|p| {
                        object(vec![
                            ("x", p.x.as_str().into()),
                            ("y", p.y.as_str().into()),
                            ("radius", Value::number(p.radius)),
                            ("value", Value::number(p.value)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn error_value(error: &CanvasError) -> Value {
    match error {
        CanvasError::IndexSize(message) => Value::Error(
            "IndexSizeError".into(),
            format!("Failed to execute 'getImageData' on 'CanvasRenderingContext2D': {message}")
                .into(),
        ),
        CanvasError::Range(message) => Value::Error("RangeError".into(), (*message).into()),
    }
}

fn pair(value: &Value, key: &str) -> Result<[f64; 2], String> {
    let pair = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{key} is not a pair"))?;
    let mut out = [0.0; 2];
    for (i, v) in pair.iter().enumerate().take(2) {
        out[i] = v
            .as_f64()
            .ok_or_else(|| format!("{key}[{i}] is not a number"))?;
    }
    Ok(out)
}

fn translations(value: &Value) -> Result<Vec<[f64; 2]>, String> {
    value
        .get("translations")
        .and_then(Value::as_array)
        .ok_or_else(|| "translations is not an array".to_owned())?
        .iter()
        .map(|pair| {
            let xy = pair
                .as_array()
                .ok_or_else(|| "translation is not a pair".to_owned())?;
            let mut out = [0.0; 2];
            for (i, v) in xy.iter().enumerate().take(2) {
                out[i] = v
                    .as_f64()
                    .ok_or_else(|| "translation is not a number".to_owned())?;
            }
            Ok(out)
        })
        .collect()
}

/// Runs one fixture update through the real port and returns the `{created, setData,
/// store, throw}` record fields as a CVE-1 value pair. `args` is the CVE-1 object the
/// vector records under `args`; `json_args` accepts the same fields as plain-JSON keys
/// holding CVE-1 trees, which is what canvasMain writes into `*.fixture.json`.
pub fn update_record(heatmap: &mut Heatmap, args: &Value) -> Result<(Value, Value), String> {
    let t = translations(args)?;
    let pixel = pair(args, "pixelDimensions")?;
    let field = pair(args, "fieldDimensions")?;
    let outcome = heatmap.update(&t, pixel, field);
    let ok = object(vec![
        ("created", Value::Bool(outcome.created)),
        (
            "setData",
            outcome
                .set_data
                .as_ref()
                .map_or(Value::Null, set_data_value),
        ),
        (
            "store",
            heatmap.get_data().as_ref().map_or(Value::Null, store_value),
        ),
    ]);
    let throw = outcome.error.as_ref().map_or(Value::Null, error_value);
    Ok((ok, throw))
}

pub fn json_args(update: &Json) -> Result<Value, String> {
    let mut fields = Vec::new();
    for key in ["translations", "pixelDimensions", "fieldDimensions"] {
        let json = update
            .get(key)
            .ok_or_else(|| format!("update has no {key}"))?;
        fields.push((
            key.into(),
            Value::from_json(json).map_err(|e| format!("{key}: {e}"))?,
        ));
    }
    Ok(Value::Object(fields))
}

/// Replays a `<name>.fixture.json` through the port: writes `<name>.data.json` plus a
/// `<name>.<i>.png` per update whose canvas could be read, mirroring canvasMain's
/// "heatmap" mode output for `cargo xtask canvas heatmap`.
fn run_fixture(fixture: &Path, out: &Path) -> Result<(), String> {
    let text = fs::read_to_string(fixture).map_err(|e| format!("{}: {e}", fixture.display()))?;
    let document = json::parse(&text).map_err(|e| format!("{}: {e}", fixture.display()))?;
    let name = document
        .get("name")
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("{}: no name", fixture.display()))?;
    let updates = document
        .get("updates")
        .and_then(Json::as_array)
        .ok_or_else(|| format!("{}: no updates", fixture.display()))?;

    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut heatmap = Heatmap::new();
    let mut records = Vec::new();
    for (i, update) in updates.iter().enumerate() {
        let args =
            json_args(update).map_err(|e| format!("{}: update {i}: {e}", fixture.display()))?;
        let (ok, throw) = update_record(&mut heatmap, &args)
            .map_err(|e| format!("{}: update {i}: {e}", fixture.display()))?;
        let (size, mut read_error) = (heatmap.canvas_size(), Value::Null);
        if std::env::var("ASRS_SHADOW_DUMP").is_ok()
            && let Ok(Some(image)) = heatmap.shadow_image_data()
        {
            fs::write(out.join(format!("{name}.{i}.shadow")), image.data)
                .map_err(|e| format!("{e}"))?;
        }
        match heatmap.image_data() {
            Ok(Some(image)) => pixel::write_png(
                &out.join(format!("{name}.{i}.png")),
                &pixel::Rgba {
                    width: image.width,
                    height: image.height,
                    data: image.data,
                },
            )
            .map_err(|e| format!("{}: {e}", out.display()))?,
            Ok(None) => {}
            Err(e) => read_error = error_value(&e),
        }
        records.push(format!(
            "{{\"created\":{},\"setData\":{},\"store\":{},\"throw\":{},\"canvas\":{},\"readError\":{}}}",
            ok.get("created").unwrap().encode(),
            ok.get("setData").unwrap().encode(),
            ok.get("store").unwrap().encode(),
            throw.encode(),
            size.map_or_else(
                || "null".to_owned(),
                |(w, h)| format!("[{w},{h}]")
            ),
            read_error.encode(),
        ));
    }
    let document = format!(
        "{{\"name\":{:?},\"updates\":[\n{}\n]}}\n",
        name,
        records.join(",\n")
    );
    fs::write(out.join(format!("{name}.data.json")), document)
        .map_err(|e| format!("{}: {e}", out.display()))
}

/// `asrs-cli heatmap <fixtures dir> <out dir>`: replays every `<name>.fixture.json`.
pub fn run(args: &[String]) -> Result<(), String> {
    let [cmd, fixtures, out] = args else {
        return Err("usage: asrs-cli heatmap <fixtures dir> <out dir>".to_owned());
    };
    if cmd != "heatmap" {
        return Err("usage: asrs-cli heatmap <fixtures dir> <out dir>".to_owned());
    }
    let fixtures = Path::new(fixtures);
    let out = Path::new(out);
    let mut names = Vec::new();
    for entry in fs::read_dir(fixtures).map_err(|e| format!("{}: {e}", fixtures.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".fixture.json"))
        {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    if names.is_empty() {
        return Err(format!("no fixtures in {}", fixtures.display()));
    }
    for name in names {
        run_fixture(&fixtures.join(format!("{name}.fixture.json")), out)?;
    }
    Ok(())
}
