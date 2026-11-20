//! JSON conversion for the wpilog oracle vectors; all reader and indexer logic
//! lives in `asrs_formats::wpilog`.

use asrs_formats::wpilog::{
    self, CONTROL_ENTRY, WpilogDecoder, WpilogError, WpilogRecord,
};
use asrs_js::string::JsString;
use asrs_parity::{Json, Utf16String};

const SAMPLE_EDGE: usize = 64;
const SAMPLE_EVERY: usize = 8192;
const BLOCK: usize = 512;

fn num(x: f64) -> Json {
    Json::object([("n", Json::string(&format!("{:016x}", x.to_bits())))])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn text(s: JsString) -> Json {
    Json::String(Utf16String::from(s.into_units()))
}

fn throw(e: WpilogError) -> Json {
    match e {
        WpilogError::Message(message) => Json::string(message),
        WpilogError::DataViewOutOfBounds => Json::object([(
            "err",
            Json::Array(vec![
                Json::string("RangeError"),
                Json::string("Offset is outside the bounds of the DataView"),
            ]),
        )]),
    }
}

fn attempt<T>(f: impl FnOnce() -> Result<T, WpilogError>, v: impl FnOnce(T) -> Json) -> Json {
    match f() {
        Ok(x) => Json::object([("ok", v(x))]),
        Err(e) => Json::object([("throw", throw(e))]),
    }
}

fn getter_json(record: &WpilogRecord<'_>) -> Json {
    Json::object([
        ("boolean", attempt(|| record.boolean(), Json::Bool)),
        ("integer", attempt(|| record.integer(), num)),
        ("float", attempt(|| record.float(), num)),
        ("double", attempt(|| record.double(), num)),
        ("string", attempt(|| Ok(record.string()), text)),
        (
            "booleanArray",
            attempt(
                || Ok(record.boolean_array()),
                |a| Json::Array(a.into_iter().map(Json::Bool).collect()),
            ),
        ),
        (
            "integerArray",
            attempt(|| record.integer_array(), |a| {
                Json::Array(a.into_iter().map(num).collect())
            }),
        ),
        (
            "floatArray",
            attempt(|| record.float_array(), |a| {
                Json::Array(a.into_iter().map(num).collect())
            }),
        ),
        (
            "doubleArray",
            attempt(|| record.double_array(), |a| {
                Json::Array(a.into_iter().map(num).collect())
            }),
        ),
        (
            "stringArray",
            attempt(|| record.string_array(), |a| {
                Json::Array(a.into_iter().map(text).collect())
            }),
        ),
    ])
}

fn control_json(record: &WpilogRecord<'_>) -> Json {
    if !record.is_control() {
        return Json::Number(0.0);
    }
    if record.is_start() {
        let value = match record.start_data() {
            Ok(d) => Json::object([
                ("entry", Json::Number(f64::from(d.entry))),
                ("name", text(d.name)),
                ("type", text(d.type_)),
                ("metadata", text(d.metadata)),
            ]),
            Err(e) => Json::object([("throw", throw(e))]),
        };
        return Json::object([("start", value)]);
    }
    if record.is_finish() {
        return Json::object([(
            "finish",
            Json::Number(f64::from(record.finish_entry().unwrap())),
        )]);
    }
    if record.is_set_metadata() {
        let value = match record.set_metadata_data() {
            Ok(d) => Json::object([
                ("entry", Json::Number(f64::from(d.entry))),
                ("metadata", text(d.metadata)),
            ]),
            Err(e) => Json::object([("throw", throw(e))]),
        };
        return Json::object([("meta", value)]);
    }
    Json::object([(
        "type",
        record
            .raw()
            .first()
            .map_or(Json::Null, |b| Json::Number(f64::from(*b))),
    )])
}

pub fn record_json(
    record: &WpilogRecord<'_>,
    position: u32,
    size: usize,
    getters: bool,
) -> Json {
    let mut pairs = vec![
        ("p".into(), Json::Number(f64::from(position))),
        ("e".into(), Json::Number(f64::from(record.entry()))),
        ("t".into(), num(record.timestamp())),
        ("s".into(), Json::Number(size as f64)),
        ("c".into(), control_json(record)),
        ("d".into(), Json::object([("b", Json::string(&hex(&record.raw())))])),
    ];
    if getters {
        pairs.push(("g".into(), getter_json(record)));
    }
    Json::Object(pairs)
}

fn index_throw() -> Json {
    Json::object([(
        "throw",
        Json::object([(
            "err",
            Json::Array(vec![
                Json::string("RuntimeError"),
                Json::string("memory access out of bounds"),
            ]),
        )]),
    )])
}

fn header_json(decoder: &WpilogDecoder<'_>) -> Json {
    Json::object([
        ("valid", Json::Bool(decoder.is_valid())),
        ("version", Json::Number(f64::from(decoder.version()))),
        ("extra", text(decoder.extra_header())),
    ])
}

fn iter_json(decoder: &WpilogDecoder<'_>) -> Json {
    let mut count = 0f64;
    match decoder.for_each(|_, _| count += 1.0) {
        Ok(()) => Json::object([("ok", num(count))]),
        Err(e) => Json::object([("throw", throw(e))]),
    }
}

/// The corpus vector's per-file object: header, iter, index and every record
/// with getter results.
pub fn summary(data: &[u8]) -> Json {
    let decoder = WpilogDecoder::new(data);
    let mut pairs = vec![
        ("header".into(), header_json(&decoder)),
        ("iter".into(), iter_json(&decoder)),
    ];
    match wpilog::index(data) {
        Err(_) => pairs.push(("index".into(), index_throw())),
        Ok(indexed) => {
            let records: Vec<Json> = indexed
                .records
                .iter()
                .map(|&(entry, position)| {
                    record_at(&decoder, entry, position, true)
                })
                .collect();
            pairs.push((
                "index".into(),
                Json::object([
                    ("min", num(indexed.min_timestamp)),
                    ("max", num(indexed.max_timestamp)),
                    (
                        "records",
                        Json::Array(
                            indexed
                                .records
                                .iter()
                                .map(|&(e, p)| {
                                    Json::Array(vec![
                                        Json::Number(f64::from(e)),
                                        Json::Number(f64::from(p)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ]),
            ));
            pairs.push(("records".into(), Json::Array(records)));
        }
    }
    Json::Object(pairs)
}

fn record_at(
    decoder: &WpilogDecoder<'_>,
    entry: u32,
    position: u32,
    getters: bool,
) -> Json {
    match decoder.record_at(position as usize) {
        None => Json::object([
            ("p", Json::Number(f64::from(position))),
            ("e", Json::Number(f64::from(entry))),
            ("null", Json::Bool(true)),
        ]),
        Some((record, size)) => record_json(&record, position, size, getters),
    }
}

/// The public-log vector's per-file object: same header and index range, but a
/// per-block digest string and verbatim samples instead of every record.
pub fn summary_public(data: &[u8]) -> Json {
    let decoder = WpilogDecoder::new(data);
    let mut pairs = vec![("header".into(), header_json(&decoder))];
    match wpilog::index(data) {
        Err(_) => pairs.push(("index".into(), index_throw())),
        Ok(indexed) => {
            let total = indexed.records.len();
            let mut blocks = String::new();
            let mut block_text = String::new();
            let mut sample = Vec::new();
            for (i, &(entry, position)) in indexed.records.iter().enumerate() {
                let json = record_at(&decoder, entry, position, false);
                block_text.push_str(&json.to_compact());
                if i % BLOCK == BLOCK - 1 || i + 1 == total {
                    blocks.push_str(&fnv1a64(&block_text));
                    block_text.clear();
                }
                let is_edge = i < SAMPLE_EDGE || i + SAMPLE_EDGE >= total;
                if entry == CONTROL_ENTRY || is_edge || i % SAMPLE_EVERY == 0 {
                    let Json::Object(fields) = json else { continue };
                    let mut object = vec![("i".into(), Json::Number(i as f64))];
                    object.extend(fields);
                    sample.push(Json::Object(object));
                }
            }
            pairs.push((
                "index".into(),
                Json::object([
                    ("min", num(indexed.min_timestamp)),
                    ("max", num(indexed.max_timestamp)),
                    ("count", Json::Number(total as f64)),
                ]),
            ));
            pairs.push(("blocks".into(), Json::string(&blocks)));
            pairs.push(("sample".into(), Json::Array(sample)));
        }
    }
    Json::Object(pairs)
}

/// The 512-record block digest list `summary_public` packs into one string; used
/// to locate a mismatching block by index.
pub fn block_digests(data: &[u8]) -> Option<Vec<String>> {
    let indexed = wpilog::index(data).ok()?;
    let decoder = WpilogDecoder::new(data);
    let total = indexed.records.len();
    let mut blocks = Vec::new();
    let mut block_text = String::new();
    for (i, &(entry, position)) in indexed.records.iter().enumerate() {
        let json = record_at(&decoder, entry, position, false);
        block_text.push_str(&json.to_compact());
        if i % BLOCK == BLOCK - 1 || i + 1 == total {
            blocks.push(fnv1a64(&block_text));
            block_text.clear();
        }
    }
    Some(blocks)
}

fn fnv1a64(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for unit in text.encode_utf16() {
        h ^= u64::from(unit);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn run(args: &[String]) -> Result<(), String> {
    let public = args.iter().any(|a| a == "--public");
    let blocks = args.iter().any(|a| a == "--blocks");
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if files.is_empty() {
        return Err("usage: asrs-cli wpilog [--public | --blocks] <log.wpilog>".to_owned());
    }
    for file in files {
        let data = std::fs::read(file).map_err(|e| format!("{file}: {e}"))?;
        if blocks {
            match block_digests(&data) {
                Some(digests) => println!("{}", digests.join("")),
                None => println!("{}", index_throw().to_compact()),
            }
            continue;
        }
        let summary = if public {
            summary_public(&data)
        } else {
            summary(&data)
        };
        println!("{}", summary.to_compact());
    }
    Ok(())
}
