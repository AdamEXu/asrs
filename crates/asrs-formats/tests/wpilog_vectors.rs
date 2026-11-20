use std::fs;
use std::path::PathBuf;

use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_parity::{diff, record_vectors};

#[path = "../../asrs-cli/src/wpilog.rs"]
#[allow(dead_code)]
mod harness;

fn hex_bytes(json: &Json) -> Result<Vec<u8>, String> {
    let text = json
        .get("b")
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| "expected {\"b\": hex}".to_owned())?;
    if text.len() % 2 != 0 {
        return Err("odd hex length".to_owned());
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| format!("hex: {e}")))
        .collect()
}

fn compare(actual: &Json, expected: &Json, at: &str) -> Result<(), String> {
    if actual.to_compact() == expected.to_compact() {
        return Ok(());
    }
    Err(format!(
        "{at}: {}",
        diff::report(
            &Value::from_json_lenient(expected),
            &Value::from_json_lenient(actual)
        )
        .unwrap_or_else(|| "values differ".to_owned())
    ))
}

fn compare_fields(actual: &Json, record: &Record, keys: &[&str], case: &str) -> Result<(), String> {
    for &key in keys {
        let Some(expected) = record.field(key) else {
            continue;
        };
        let Some(actual_value) = actual.get(key) else {
            return Err(format!("{case}.{key}: missing in port output"));
        };
        compare(actual_value, expected, &format!("{case}.{key}"))?;
    }
    Ok(())
}

record_vectors!(corpus, "wpilog/corpus", |record: &Record| {
    let case = record.string("case").unwrap_or_default();
    let bytes = hex_bytes(
        record
            .field("bytes")
            .ok_or_else(|| format!("{case}: no bytes"))?,
    )?;
    compare_fields(
        &harness::summary(&bytes),
        record,
        &["header", "iter", "index", "records"],
        &case,
    )
});

fn public_logs_dir() -> PathBuf {
    let root = vectors::workspace_root();
    if let Some(dir) = std::env::var_os("ASRS_CACHE") {
        return PathBuf::from(dir).join("public-logs");
    }
    let target = root.join("target");
    if target.is_symlink()
        && let Ok(real) = fs::canonicalize(&target)
        && let Some(parent) = real.parent()
    {
        return parent.join("cache").join("public-logs");
    }
    target.join("cache").join("public-logs")
}

fn compare_blocks(actual: &Json, expected: &Json, case: &str) -> Result<(), String> {
    let digests = |json: Option<&Json>| -> Vec<Vec<u8>> {
        json.and_then(Json::as_str)
            .map(|s| {
                s.to_string_lossy()
                    .as_bytes()
                    .chunks(16)
                    .map(<[u8]>::to_vec)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (expected, actual) = (digests(Some(expected)), digests(actual.get("blocks")));
    let Some(i) = (0..expected.len().max(actual.len())).find(|&i| expected.get(i) != actual.get(i))
    else {
        return Ok(());
    };
    let show = |d: Option<&Vec<u8>>| {
        d.map_or("<none>".into(), |d| String::from_utf8_lossy(d).into_owned())
    };
    Err(format!(
        "{case}.blocks: block {i} (records {}..) differs: oracle {}, port {}",
        i * 512,
        show(expected.get(i)),
        show(actual.get(i))
    ))
}

record_vectors!(public, "wpilog/public", |record: &Record| {
    let case = record.string("case").unwrap_or_default();
    let info = record
        .field("in")
        .ok_or_else(|| format!("{case}: no input info"))?;
    let file = info
        .get("file")
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("{case}: no file"))?;
    let path = public_logs_dir().join(&file);
    let data = fs::read(&path).map_err(|e| {
        format!(
            "{case}: {}: {e} (fetch with `cargo xtask corpus fetch`)",
            path.display()
        )
    })?;
    let size = info.get("size").and_then(Json::as_f64).unwrap_or(-1.0);
    if data.len() as f64 != size {
        return Err(format!(
            "{case}: {}: size {} differs from manifest {size}",
            path.display(),
            data.len()
        ));
    }
    let actual = harness::summary_public(&data);
    compare_fields(&actual, record, &["header", "index", "sample"], &case)?;
    if let Some(expected_blocks) = record.field("blocks") {
        compare_blocks(&actual, expected_blocks, &case)?;
    }
    Ok(())
});

record_vectors!(search_key_sets, "log/search", |record: &Record| {
    let case = record.string("case").unwrap_or_default();
    let Some(file) = record
        .field("source")
        .and_then(|s| s.get("file"))
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
    else {
        return Ok(());
    };
    let path = public_logs_dir().join(&file);
    let data = fs::read(&path).map_err(|e| {
        format!(
            "{case}: {}: {e} (fetch with `cargo xtask corpus fetch`)",
            path.display()
        )
    })?;
    let mut names = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut error = None;
    asrs_formats::wpilog::WpilogDecoder::new(&data)
        .for_each(|r, _| {
            if !r.is_start() || error.is_some() {
                return;
            }
            match r.start_data() {
                Ok(start) => {
                    if seen.insert(start.name.clone()) {
                        names.push(start.name);
                    }
                }
                Err(e) => error = Some(e),
            }
        })
        .map_err(|e| format!("{case}: {e:?}"))?;
    if let Some(e) = error {
        return Err(format!("{case}: start record: {e:?}"));
    }
    let expected: Vec<Vec<u16>> = record
        .field("in")
        .and_then(Json::as_array)
        .ok_or_else(|| format!("{case}: no key list"))?
        .iter()
        .map(|k| k.as_str().map(|s| s.units().to_vec()).unwrap_or_default())
        .collect();
    let actual: Vec<Vec<u16>> = names.iter().map(|n| n.units().to_vec()).collect();
    if actual != expected {
        return Err(format!(
            "{case}: entry names differ ({} port, {} oracle)",
            actual.len(),
            expected.len()
        ));
    }
    Ok(())
});
