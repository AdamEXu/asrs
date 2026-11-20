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

fn compare_blocks(actual: &Json, expected: &Json, data: &[u8], case: &str) -> Result<(), String> {
    let Some(expected_str) = expected.as_str() else {
        return Err(format!("{case}.blocks: not a string"));
    };
    let Some(actual_str) = actual.get("blocks").and_then(Json::as_str) else {
        return Err(format!("{case}.blocks: missing in port output"));
    };
    if actual_str == expected_str {
        return Ok(());
    }
    let Some(blocks) = harness::block_digests(data) else {
        return Err(format!("{case}: indexer traps but vector lists records"));
    };
    let expected_str = expected_str.to_string_lossy();
    for (i, digest) in blocks.iter().enumerate() {
        let expected = expected_str.get(16 * i..16 * i + 16).unwrap_or("<missing>");
        if digest != expected {
            let first = i * 512;
            return Err(format!(
                "{case}.blocks: block {i} differs (records {first}.., oracle {expected}, port {digest})"
            ));
        }
    }
    Err(format!(
        "{case}.blocks: block counts differ (oracle {}, port {})",
        expected_str.len() / 16,
        blocks.len()
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
        compare_blocks(&actual, expected_blocks, &data, &case)?;
    }
    Ok(())
});
