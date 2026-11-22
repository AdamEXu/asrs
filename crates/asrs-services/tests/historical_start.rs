#![allow(clippy::disallowed_methods)]

//! Replays `files/historical-start` (main.ts's real handlers against the oracle's fake fs and the
//! real lzma-native) through `asrs_services::historical_start`, comparing the reply, the
//! recent document, the thrown-after-reply flag, the write log and the files left behind.

use std::collections::HashMap;

use asrs_formats::historical::MainRequest;
use asrs_js::string::JsString;
use asrs_js::time::Rng;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_services::fs::{DirEntry, Failure, Fs, MemFs, ReadError};
use asrs_services::historical_start::FileOpen;
use sha2::{Digest, Sha256};

const ID: &str = "files/historical-start";
const TEMP: &str = "/fake/temp";
const INLINE_LIMIT: usize = 4096;

/// The fake fs keys paths with `/`; the port joins with the platform separator.
struct Slashed<'a>(&'a MemFs);

fn slashed(path: &str) -> String {
    path.replace(std::path::MAIN_SEPARATOR, "/")
}

impl Fs for Slashed<'_> {
    fn exists(&self, path: &str) -> bool {
        self.0.exists(&slashed(path))
    }
    fn read(&self, path: &str) -> Result<Vec<u8>, asrs_js::string::JsError> {
        self.0.read(&slashed(path))
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<(), asrs_js::string::JsError> {
        self.0.write(&slashed(path), data)
    }
    fn copy(&self, from: &str, to: &str) -> Result<(), asrs_js::string::JsError> {
        self.0.copy(&slashed(from), &slashed(to))
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadError> {
        self.0.read_file(&slashed(path))
    }
    fn remove(&self, path: &str) {
        Fs::remove(self.0, &slashed(path))
    }
    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, asrs_js::string::JsError> {
        self.0.read_dir(&slashed(path))
    }
    fn mkdir(&self, path: &str) -> Result<(), asrs_js::string::JsError> {
        self.0.mkdir(&slashed(path))
    }
}

struct Scripted {
    values: Vec<f64>,
    next: usize,
}

impl Rng for Scripted {
    fn next_f64(&mut self) -> f64 {
        let value = self.values.get(self.next).copied();
        self.next += 1;
        value.unwrap_or_else(|| {
            panic!(
                "the port drew more than the {} recorded randoms",
                self.values.len()
            )
        })
    }
}

fn field<'a>(json: &'a Json, key: &str) -> Result<&'a Json, String> {
    json.get(key).ok_or_else(|| format!("no field {key:?}"))
}

fn array<'a>(json: &'a Json, key: &str) -> Result<&'a [Json], String> {
    field(json, key)?
        .as_array()
        .ok_or_else(|| format!("{key:?} is not an array"))
}

fn text(json: &Json) -> Result<JsString, String> {
    json.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| format!("{} is not a string", json.to_compact()))
}

fn bytes(json: &Json) -> Result<Vec<u8>, String> {
    match Value::from_json(json).map_err(|e| e.to_string())? {
        Value::Bytes(b) => Ok(b),
        other => Err(format!("expected bytes, got {}", other.kind())),
    }
}

fn input(json: &Json, blobs: &HashMap<u64, Vec<u8>>) -> Result<Vec<u8>, String> {
    if let Some(b) = json.get("bytes") {
        return bytes(b);
    }
    let id = field(json, "blob")?.as_f64().ok_or("blob id")? as u64;
    let blob = blobs.get(&id).ok_or_else(|| format!("no blob {id}"))?;
    let len = match json.get("len") {
        Some(len) => len.as_f64().ok_or("len")? as usize,
        None => blob.len(),
    };
    let mut out = blob[..len].to_vec();
    if let Some(flips) = json.get("flips") {
        for flip in flips.as_array().ok_or("flips")? {
            let pair = flip.as_array().ok_or("flip")?;
            let offset = pair[0].as_f64().ok_or("flip offset")? as usize;
            out[offset] ^= pair[1].as_f64().ok_or("flip xor")? as u8;
        }
    }
    Ok(out)
}

fn describe(data: Option<&[u8]>) -> String {
    match data {
        None => "null".to_owned(),
        Some(d) if d.len() <= INLINE_LIMIT => {
            format!("{{\"bytes\":{}}}", Value::Bytes(d.to_vec()).encode())
        }
        Some(d) => {
            let digest: String = Sha256::digest(d)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            format!("{{\"len\":{},\"sha256\":\"{digest}\"}}", d.len())
        }
    }
}

fn recorded_description(json: &Json) -> Result<String, String> {
    if let Json::Null = json {
        return Ok("null".to_owned());
    }
    if let Some(b) = json.get("bytes") {
        return Ok(describe(Some(&bytes(b)?)));
    }
    let len = field(json, "len")?.as_f64().ok_or("len")?;
    let digest = text(field(json, "sha256")?)?.to_string_lossy();
    Ok(format!("{{\"len\":{len},\"sha256\":\"{digest}\"}}"))
}

fn check(expected: &str, actual: &str, what: &str) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!("{what}: expected {expected}, port {actual}"))
    }
}

fn run_case(record: &Json, blobs: &HashMap<u64, Vec<u8>>) -> Result<(), String> {
    let mem = MemFs::new();
    if let Some(files) = record.get("files") {
        for entry in files.as_array().ok_or("files")? {
            let pair = entry.as_array().ok_or("file entry")?;
            mem.put(&text(&pair[0])?.to_string_lossy(), &input(&pair[1], blobs)?);
        }
    }
    if let Some(failures) = record.get("failures") {
        for entry in failures.as_array().ok_or("failures")? {
            let pair = entry.as_array().ok_or("failure entry")?;
            let kind = match text(&pair[1])?.to_string_lossy().as_str() {
                "read" => Failure::Read,
                "write" => Failure::Write,
                "readOpened" => Failure::ReadOpened,
                other => return Err(format!("unknown failure kind {other:?}")),
            };
            mem.set_failure(&text(&pair[0])?.to_string_lossy(), Some(kind));
        }
    }
    if let Some(dirs) = record.get("missingDirs") {
        for dir in dirs.as_array().ok_or("missingDirs")? {
            mem.set_missing_dir(&text(dir)?.to_string_lossy(), true);
        }
    }
    let request = field(record, "request")?;
    let uuid = text(field(request, "uuid")?)?.to_string_lossy();
    let request = match text(field(request, "name")?)?.to_string_lossy().as_str() {
        "historical-start" => MainRequest::HistoricalStart {
            uuid,
            path: text(field(request, "path")?)?,
        },
        "historical-start-raw" => MainRequest::HistoricalStartRaw {
            uuid,
            data: input(field(request, "data")?, blobs)?.into(),
            extension: text(field(request, "extension")?)?,
        },
        other => return Err(format!("unknown request {other:?}")),
    };
    let mut rng = Scripted {
        values: array(record, "random")?
            .iter()
            .map(|v| v.as_f64().ok_or("random value"))
            .collect::<Result<_, _>>()?,
        next: 0,
    };

    let fs = Slashed(&mem);
    let open = FileOpen {
        fs: &fs,
        temp_dir: TEMP,
        lzma: true,
    };
    let outcome = open.handle(request, &mut rng);

    check(
        &rng.values.len().to_string(),
        &rng.next.to_string(),
        "Math.random draws",
    )?;

    let replies = array(record, "replies")?;
    let expected_replies: Vec<String> = replies
        .iter()
        .map(|r| -> Result<String, String> {
            let files = array(r, "files")?
                .iter()
                .map(recorded_description)
                .collect::<Result<Vec<_>, _>>()?
                .join(",");
            Ok(format!(
                "uuid={} error={} files=[{files}]",
                field(r, "uuid")?.to_compact(),
                field(r, "error")?.to_compact()
            ))
        })
        .collect::<Result<_, _>>()?;
    let actual_replies: Vec<String> = outcome
        .reply
        .iter()
        .map(|r| {
            let files = r
                .files
                .iter()
                .map(|f| describe(f.as_deref()))
                .collect::<Vec<_>>()
                .join(",");
            let error = match &r.error {
                None => "null".to_owned(),
                Some(e) => Json::String(asrs_parity::utf16::Utf16String::from_units(
                    e.units().to_vec(),
                ))
                .to_compact(),
            };
            format!(
                "uuid={} error={error} files=[{files}]",
                Json::string(&r.uuid).to_compact()
            )
        })
        .collect();
    check(
        &format!("{expected_replies:?}"),
        &format!("{actual_replies:?}"),
        "replies",
    )?;

    let recent: Vec<String> = array(record, "recent")?
        .iter()
        .map(|p| text(p).map(|s| s.to_string_lossy()))
        .collect::<Result<_, _>>()?;
    let port_recent: Vec<String> = outcome
        .recent_document
        .iter()
        .map(|p| p.to_string_lossy())
        .collect();
    check(
        &format!("{recent:?}"),
        &format!("{port_recent:?}"),
        "recent documents",
    )?;

    let uncaught = field(record, "uncaught")?.as_f64().ok_or("uncaught")?;
    check(
        &(uncaught > 0.0).to_string(),
        &outcome.uncaught_error.to_string(),
        "threw after replying",
    )?;

    let writes: Vec<String> = array(record, "writes")?
        .iter()
        .map(|p| text(p).map(|s| s.to_string_lossy()))
        .collect::<Result<_, _>>()?;
    check(
        &format!("{writes:?}"),
        &format!("{:?}", mem.writes()),
        "writes",
    )?;

    let mut after: Vec<(String, String)> = array(record, "after")?
        .iter()
        .map(|entry| -> Result<(String, String), String> {
            let pair = entry.as_array().ok_or("after entry")?;
            Ok((
                text(&pair[0])?.to_string_lossy(),
                recorded_description(&pair[1])?,
            ))
        })
        .collect::<Result<_, _>>()?;
    after.sort();
    let mut port_after: Vec<(String, String)> = mem
        .dump()
        .into_iter()
        .map(|(name, data)| (name, describe(Some(&data))))
        .collect();
    port_after.sort();
    check(
        &format!("{after:?}"),
        &format!("{port_after:?}"),
        "files after",
    )
}

#[test]
fn historical_start_vectors() {
    let records = vectors::load(ID).unwrap_or_else(|e| panic!("{e}"));
    let mut blobs = HashMap::new();
    let mut recorded = 0;
    let mut compared = 0;
    vectors::check_records(ID, |record| {
        let json = &record.json;
        if json
            .get("op")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .as_deref()
            == Some("config")
        {
            let config = record.value("result").map_err(|e| e.to_string())?;
            let temp = config
                .get("temp")
                .and_then(Value::as_str)
                .map(|s| s.to_string_lossy());
            return check(TEMP, temp.as_deref().unwrap_or("<missing>"), "temp path");
        }
        match json
            .get("kind")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .as_deref()
        {
            Some("blob") => {
                let id = field(json, "id")?.as_f64().ok_or("blob id")? as u64;
                blobs.insert(id, bytes(field(json, "bytes")?)?);
                Ok(())
            }
            Some("case") => {
                recorded += 1;
                let result = run_case(json, &blobs);
                compared += 1;
                result
            }
            other => Err(format!("unknown record kind {other:?}")),
        }
    });
    let cases = records
        .iter()
        .filter(|r| {
            r.json
                .get("kind")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .as_deref()
                == Some("case")
        })
        .count();
    assert_eq!(recorded, cases, "every case record was replayed");
    assert_eq!(compared, cases, "every case record was compared");
}
