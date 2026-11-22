#![allow(clippy::disallowed_methods)]

//! Replays `node/download-helpers`: node's legacy `url` module, `url-parse-lax`, the WHATWG
//! pathname basename, `path` dirname/basename/extname, `filenamify`, `content-disposition`,
//! `ext-name`, file-type 11, `archive-type` and `decompress-unzip`, each run by the oracle on
//! upstream's own node_modules, against the helpers `asset_downloader` is built from.

use asrs_services::Invalid;
use std::collections::BTreeMap;

use asrs_js::string::JsString;
use asrs_parity::vectors::check_records;
use asrs_parity::{Json, Record, Utf16String};
use asrs_services::download_deps::{
    disposition_filename, ext_name_mime, filenamify, url_parse_lax, url_path_basename,
};
use asrs_services::file_type::{Guess, archive_type, file_type_11};
use asrs_services::legacy_url::{self, LegacyUrl};
use asrs_services::path::PathStyle;
use asrs_services::zip::{self, EntryKind};

const FNS: [&str; 11] = [
    "urlParse",
    "urlParseLax",
    "urlFormatDefaultProtocol",
    "urlPathBasename",
    "urlResolve",
    "pathParts",
    "filenamify",
    "contentDisposition",
    "extNameMime",
    "fileType",
    "unzip",
];

fn text(s: &str) -> Json {
    Json::string(s)
}

fn opt(s: &Option<String>) -> Json {
    s.as_deref().map_or(Json::Null, text)
}

fn object(fields: Vec<(&str, Json)>) -> Json {
    Json::Object(
        fields
            .into_iter()
            .map(|(k, v)| {
                let mut key = Utf16String::new();
                key.push_str(k);
                (key, v)
            })
            .collect(),
    )
}

fn throws() -> Json {
    object(vec![("throws", Json::Bool(true))])
}

fn hex(bytes: &[u8]) -> Json {
    text(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn url_json(url: &LegacyUrl) -> Json {
    object(vec![
        ("protocol", opt(&url.protocol)),
        (
            "slashes",
            if url.slashes {
                Json::Bool(true)
            } else {
                Json::Null
            },
        ),
        ("auth", opt(&url.auth)),
        ("host", opt(&url.host)),
        ("port", opt(&url.port)),
        ("hostname", opt(&url.hostname)),
        ("hash", opt(&url.hash)),
        ("search", opt(&url.search)),
        ("query", opt(&url.query)),
        ("pathname", opt(&url.pathname)),
        ("path", opt(&url.path)),
        ("href", text(&url.href)),
    ])
}

fn or_throws<T>(result: Result<T, Invalid>, f: impl FnOnce(T) -> Json) -> Json {
    result.map_or_else(|Invalid| throws(), f)
}

fn input_string(json: &Json) -> Result<String, String> {
    json.as_str()
        .ok_or("input is not a string")?
        .as_string()
        .ok_or_else(|| "input has a lone surrogate".to_owned())
}

fn input_bytes(json: &Json) -> Result<Vec<u8>, String> {
    let hex = json
        .get("hex")
        .and_then(Json::as_str)
        .ok_or("input is not {hex}")?
        .to_string_lossy();
    (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn path_parts(style: PathStyle, input: &str) -> Json {
    let input = JsString::from(input);
    let lossy = |s: JsString| text(&s.to_string_lossy());
    object(vec![
        ("dirname", lossy(style.dirname(&input))),
        ("basename", lossy(style.basename(&input))),
        ("extname", lossy(style.extname(&input))),
    ])
}

fn port_output(name: &str, inputs: &[Json]) -> Result<Json, String> {
    let first = || input_string(inputs.first().ok_or("no input")?);
    Ok(match name {
        "urlParse" => or_throws(legacy_url::parse(&first()?, false), |u| url_json(&u)),
        "urlParseLax" => or_throws(url_parse_lax(&JsString::from(first()?.as_str())), |u| {
            url_json(&u)
        }),
        "urlFormatDefaultProtocol" => or_throws(legacy_url::parse(&first()?, false), |mut u| {
            if u.protocol.as_deref().is_none_or(str::is_empty) {
                u.protocol = Some("http:".to_owned());
            }
            text(&legacy_url::format(&u))
        }),
        "urlPathBasename" => {
            let href = first()?;
            match (
                url_path_basename(&href, PathStyle::Posix),
                url_path_basename(&href, PathStyle::Win32),
            ) {
                (Ok(posix), Ok(win32)) => {
                    object(vec![("posix", text(&posix)), ("win32", text(&win32))])
                }
                (Err(Invalid), Err(Invalid)) => throws(),
                _ => return Err("posix and win32 disagree on whether the URL parses".to_owned()),
            }
        }
        "urlResolve" => {
            let base = first()?;
            let relative = input_string(inputs.get(1).ok_or("no relative")?)?;
            or_throws(legacy_url::resolve(&base, &relative), |r| text(&r))
        }
        "pathParts" => {
            let input = first()?;
            object(vec![
                ("posix", path_parts(PathStyle::Posix, &input)),
                ("win32", path_parts(PathStyle::Win32, &input)),
            ])
        }
        "filenamify" => text(&filenamify(&first()?)),
        "contentDisposition" => or_throws(disposition_filename(&first()?), |name| opt(&name)),
        "extNameMime" => Json::Array(ext_name_mime(&first()?).into_iter().map(text).collect()),
        "fileType" => {
            let bytes = input_bytes(inputs.first().ok_or("no input")?)?;
            let guess = match file_type_11(&bytes) {
                Guess::Ext(ext) => text(ext),
                Guess::Unknown => Json::Null,
                Guess::Stalls => return Err("file-type 11 stalls on a recorded input".to_owned()),
            };
            object(vec![
                ("fileType", guess),
                ("archiveType", archive_type(&bytes).map_or(Json::Null, text)),
            ])
        }
        "unzip" => {
            let bytes = input_bytes(inputs.first().ok_or("no input")?)?;
            or_throws(zip::decompress_unzip(&bytes), |entries| {
                Json::Array(
                    entries
                        .iter()
                        .map(|e| {
                            let kind = match e.kind {
                                EntryKind::File => "file",
                                EntryKind::Directory => "directory",
                                EntryKind::Symlink => "symlink",
                            };
                            object(vec![
                                ("path", text(&e.name)),
                                ("type", text(kind)),
                                ("mode", Json::Number(f64::from(e.mode))),
                                ("data", object(vec![("hex", hex(&e.data))])),
                            ])
                        })
                        .collect(),
                )
            })
        }
        other => return Err(format!("unknown fn {other:?}")),
    })
}

/// `filenamify` truncates to 100 UTF-16 units and may split a surrogate pair; node writes the
/// resulting lone surrogate to disk as U+FFFD, which is what the port's `String` holds.
fn expected(name: &str, out: &Json) -> Json {
    match (name, out) {
        ("filenamify", Json::String(s)) => text(&s.to_string_lossy()),
        _ => out.clone(),
    }
}

#[test]
fn download_helper_vectors() {
    let mut compared: BTreeMap<String, usize> = BTreeMap::new();
    check_records("node/download-helpers", |record: &Record| {
        let name = record.string("fn").ok_or("record has no fn")?;
        let inputs = record
            .field("in")
            .and_then(Json::as_array)
            .ok_or("record has no inputs")?;
        let out = record.field("out").ok_or("record has no out")?;
        let actual = port_output(&name, inputs)?;
        let expected = expected(&name, out);
        if actual != expected {
            return Err(format!(
                "{name}({inputs:?}):\n  node {expected:?}\n  port {actual:?}"
            ));
        }
        *compared.entry(name).or_default() += 1;
        Ok(())
    });
    for name in FNS {
        assert!(
            compared.get(name).copied().unwrap_or(0) > 0,
            "no {name} records"
        );
    }
    assert_eq!(compared.len(), FNS.len(), "unexpected fns: {compared:?}");
}
