#![allow(clippy::disallowed_methods)]

//! Replays `files/asset-loader` (upstream's real assetLoader against the oracle's fake fs)
//! through `asrs_services::asset_loader`, and `node/path-join` (node's own `path.join`)
//! through `asrs_services::path`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use asrs_js::string::{JsError, JsString};
use asrs_parity::vectors::{check_records, workspace_root};
use asrs_parity::{Json, Record, Utf16String, Value, diff};
use asrs_services::asset_loader::{
    AssetFolders, AssetService, create_asset_folders, get_user_assets_path,
};
use asrs_services::fs::{Failure, Fs, MemFs};
use asrs_services::path::PathStyle;
use sha2::{Digest, Sha256};

const USER_DATA: &str = "/fake/userData";
const BUNDLED: &str = "/fake/bundledAssets";
const INLINE_LIMIT: usize = 4096;

fn bundled_source() -> PathBuf {
    workspace_root().join("reference/AdvantageScope/bundledAssets")
}

fn field<'a>(json: &'a Json, key: &str) -> Result<&'a Json, String> {
    json.get(key).ok_or_else(|| format!("no field {key:?}"))
}

fn text(json: &Json) -> Result<String, String> {
    json.as_str()
        .map(Utf16String::to_string_lossy)
        .ok_or_else(|| format!("{json:?} is not a string"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Memoized by content: the same multi-megabyte bundled images are dumped in many cases, and
/// std's hashing of the key is far cheaper than an unoptimized SHA-256.
fn sha256(bytes: &[u8]) -> String {
    thread_local! {
        static MEMO: RefCell<HashMap<Vec<u8>, String>> = RefCell::new(HashMap::new());
    }
    MEMO.with_borrow_mut(|memo| {
        if let Some(digest) = memo.get(bytes) {
            return digest.clone();
        }
        let digest = hex(&Sha256::digest(bytes));
        memo.insert(bytes.to_vec(), digest.clone());
        digest
    })
}

fn read_bundled(rel: &str) -> Result<Vec<u8>, String> {
    thread_local! {
        static FILES: RefCell<HashMap<String, Vec<u8>>> = RefCell::new(HashMap::new());
    }
    if let Some(bytes) = FILES.with_borrow(|files| files.get(rel).cloned()) {
        return Ok(bytes);
    }
    let bytes = std::fs::read(bundled_source().join(rel))
        .map_err(|e| format!("bundled asset {rel}: {e}"))?;
    FILES.with_borrow_mut(|files| files.insert(rel.to_owned(), bytes.clone()));
    Ok(bytes)
}

/// The record's `{"hex"}` or `{"len","sha256"}` form of `bytes`.
fn file_json(bytes: &[u8]) -> Vec<(&'static str, String)> {
    if bytes.len() <= INLINE_LIMIT {
        vec![("hex", hex(bytes))]
    } else {
        vec![("len", bytes.len().to_string()), ("sha256", sha256(bytes))]
    }
}

fn data_bytes(data: &Json) -> Result<Vec<u8>, String> {
    if let Some(rel) = data.get("bundled") {
        let bytes = read_bundled(&text(rel)?)?;
        let len = field(data, "len").ok().and_then(Json::as_f64);
        let expected = match len {
            Some(len) => vec![
                ("len", (len as usize).to_string()),
                ("sha256", text(field(data, "sha256")?)?),
            ],
            None => vec![("hex", text(field(data, "hex")?)?)],
        };
        if expected != file_json(&bytes) {
            return Err(format!(
                "bundled asset {} differs from the oracle's",
                text(rel)?
            ));
        }
        return Ok(bytes);
    }
    unhex(&text(field(data, "hex")?)?)
}

fn error_value(e: &JsError) -> Value {
    Value::Error(e.name.into(), Utf16String::from(e.message.as_str()))
}

fn expect_outcome(record: &Record, actual: Result<Value, Value>) -> Result<(), String> {
    let (key, actual) = match &actual {
        Ok(v) => ("result", v),
        Err(v) => ("throw", v),
    };
    let expected = record.field(key).ok_or_else(|| {
        let other = if key == "result" { "throw" } else { "result" };
        match record.field(other) {
            Some(json) => format!(
                "oracle has {other} {}, port has {key} {}",
                json.to_compact(),
                actual.encode()
            ),
            None => "record has neither result nor throw".to_owned(),
        }
    })?;
    let expected = Value::from_json(expected).map_err(|e| e.to_string())?;
    if expected.encode() == actual.encode() {
        Ok(())
    } else {
        Err(format!(
            "{key}: {}",
            diff::report(&expected, actual).unwrap_or_else(|| "encodings differ".to_owned())
        ))
    }
}

struct Replay {
    fs: MemFs,
    service: AssetService,
    last_loaded: Value,
    counts: BTreeMap<String, usize>,
}

fn folders() -> AssetFolders {
    AssetFolders::new(PathStyle::Posix, USER_DATA, BUNDLED)
}

impl Replay {
    fn new() -> Replay {
        let service = AssetService::new(folders());
        let last_loaded = js_json::from_js(&service.set_assets_data());
        Replay {
            fs: MemFs::new(),
            service,
            last_loaded,
            counts: BTreeMap::new(),
        }
    }

    fn step(&mut self, record: &Record) -> Result<(), String> {
        let op = record.string("op").ok_or("record has no op")?;
        let args = record.field("args");
        let arg = |key: &str| -> Result<&Json, String> { field(args.ok_or("no args")?, key) };
        let outcome: Result<Value, Value> = match op.as_str() {
            "config" => {
                let result = record.value("result").map_err(|e| e.to_string())?;
                let folders = folders();
                let expected = [
                    ("userData", USER_DATA.to_owned()),
                    ("prefsFile", folders.prefs_filename.to_string_lossy()),
                ];
                for (key, value) in expected {
                    if result
                        .get(key)
                        .and_then(Value::as_str)
                        .map(Utf16String::to_string_lossy)
                        != Some(value.clone())
                    {
                        return Err(format!("oracle {key} is not {value}"));
                    }
                }
                *self.counts.entry(op).or_default() += 1;
                return Ok(());
            }
            "reset" => {
                *self = Replay {
                    counts: std::mem::take(&mut self.counts),
                    ..Replay::new()
                };
                Ok(Value::Null)
            }
            "putDir" => {
                self.fs.put_dir(&text(arg("name")?)?);
                Ok(Value::Null)
            }
            "putFile" => {
                self.fs
                    .put(&text(arg("name")?)?, &data_bytes(arg("data")?)?);
                Ok(Value::Null)
            }
            "removeFile" => {
                MemFs::remove(&self.fs, &text(arg("name")?)?);
                Ok(Value::Null)
            }
            "setFailure" => {
                let kind = match arg("kind")? {
                    Json::Null => None,
                    kind => Some(match text(kind)?.as_str() {
                        "read" => Failure::Read,
                        "write" => Failure::Write,
                        other => return Err(format!("failure kind {other} is not modeled")),
                    }),
                };
                self.fs.set_failure(&text(arg("name")?)?, kind);
                Ok(Value::Null)
            }
            "createAssetFolders" => create_asset_folders(&self.fs, &self.service.folders)
                .map(|()| Value::Undefined)
                .map_err(|e| error_value(&e)),
            "getUserAssetsPath" => get_user_assets_path(&self.fs, &self.service.folders)
                .map(|v| js_json::from_js(&v))
                .map_err(|e| error_value(&e)),
            "loadAssets" => match self.service.reload(&self.fs) {
                Ok(()) => {
                    let loaded = js_json::from_js(&self.service.set_assets_data());
                    self.last_loaded = loaded.clone();
                    Ok(loaded)
                }
                Err(e) => {
                    let kept = js_json::from_js(&self.service.set_assets_data());
                    if kept.encode() != self.last_loaded.encode() {
                        return Err("a failed load replaced the assets".to_owned());
                    }
                    Err(error_value(&e))
                }
            },
            other => return Err(format!("op {other} is not modeled")),
        };
        expect_outcome(record, outcome)?;
        self.check_writes(record)?;
        if record.field("files").is_some() || record.field("dirs").is_some() {
            self.check_dump(record)?;
        }
        *self.counts.entry(op).or_default() += 1;
        Ok(())
    }

    fn check_writes(&self, record: &Record) -> Result<(), String> {
        let expected = record
            .field("writes")
            .and_then(Json::as_array)
            .ok_or("record has no writes")?
            .iter()
            .map(text)
            .collect::<Result<Vec<_>, _>>()?;
        let actual = self.fs.take_writes();
        if expected != actual {
            return Err(format!("writes: oracle {expected:?}, port {actual:?}"));
        }
        Ok(())
    }

    fn check_dump(&self, record: &Record) -> Result<(), String> {
        let mut expected_files = Vec::new();
        for file in field(&record.json, "files")?
            .as_array()
            .ok_or("files is not an array")?
        {
            let name = text(field(file, "name")?)?;
            let value = match file.get("len") {
                Some(len) => vec![
                    ("len", (len.as_f64().ok_or("len")? as usize).to_string()),
                    ("sha256", text(field(file, "sha256")?)?),
                ],
                None => vec![("hex", text(field(file, "hex")?)?)],
            };
            expected_files.push((name, value));
        }
        expected_files.sort();
        let mut actual_files: Vec<_> = self
            .fs
            .dump()
            .into_iter()
            .map(|(name, bytes)| (name, file_json(&bytes)))
            .collect();
        actual_files.sort();
        if expected_files != actual_files {
            return Err(format!(
                "files: oracle {expected_files:?}\n port {actual_files:?}"
            ));
        }
        let mut expected_dirs = field(&record.json, "dirs")?
            .as_array()
            .ok_or("dirs is not an array")?
            .iter()
            .map(text)
            .collect::<Result<Vec<_>, _>>()?;
        expected_dirs.sort();
        if expected_dirs != self.fs.dirs() {
            return Err(format!(
                "dirs: oracle {expected_dirs:?}, port {:?}",
                self.fs.dirs()
            ));
        }
        Ok(())
    }
}

#[test]
fn asset_loader_vectors() {
    let records =
        asrs_parity::vectors::load("files/asset-loader").unwrap_or_else(|e| panic!("{e}"));
    let mut recorded: BTreeMap<String, usize> = BTreeMap::new();
    for record in &records {
        *recorded
            .entry(record.string("op").unwrap_or_default())
            .or_default() += 1;
    }
    let mut replay = Replay::new();
    check_records("files/asset-loader", |record| replay.step(record));
    assert_eq!(
        replay.counts, recorded,
        "compared ops differ from recorded ops"
    );
    for op in ["createAssetFolders", "getUserAssetsPath", "loadAssets"] {
        assert!(
            recorded.get(op).copied().unwrap_or(0) > 0,
            "no {op} records"
        );
    }
}

#[test]
fn path_join_vectors() {
    let mut compared = 0;
    check_records("node/path-join", |record| {
        let args = record
            .field("args")
            .and_then(Json::as_array)
            .ok_or("record has no args")?
            .iter()
            .map(|a| a.as_str().map(|s| JsString::from_units(s.units().to_vec())))
            .collect::<Option<Vec<_>>>()
            .ok_or("args are not strings")?;
        let refs: Vec<&JsString> = args.iter().collect();
        for (key, style) in [("posix", PathStyle::Posix), ("win32", PathStyle::Win32)] {
            let expected = record
                .field(key)
                .and_then(Json::as_str)
                .ok_or("no expected join")?;
            let actual = style.join(&refs);
            if actual.units() != expected.units() {
                return Err(format!(
                    "{key}.join({:?}): node {:?}, port {:?}",
                    args.iter()
                        .map(JsString::to_string_lossy)
                        .collect::<Vec<_>>(),
                    expected.to_string_lossy(),
                    actual.to_string_lossy()
                ));
            }
        }
        compared += 1;
        Ok(())
    });
    assert!(compared > 0);
}

#[test]
fn readme_bytes() {
    let fs = MemFs::new();
    fs.put_dir(USER_DATA);
    create_asset_folders(&fs, &folders()).expect("folders are created");
    let records =
        asrs_parity::vectors::load("files/asset-loader").unwrap_or_else(|e| panic!("{e}"));
    let first = records
        .iter()
        .find(|r| {
            r.string("op").as_deref() == Some("createAssetFolders") && r.field("throw").is_none()
        })
        .expect("a successful createAssetFolders record");
    let readme = format!("{USER_DATA}/userAssets/README.txt");
    let oracle = field(&first.json, "files")
        .ok()
        .and_then(Json::as_array)
        .and_then(|files| {
            files.iter().find(|f| {
                f.get("name")
                    .and_then(Json::as_str)
                    .map(Utf16String::to_string_lossy)
                    == Some(readme.clone())
            })
        })
        .expect("the oracle wrote the README");
    let port = fs.read(&readme).expect("the port wrote the README");
    assert_eq!(
        text(field(oracle, "hex").unwrap()).unwrap(),
        hex(&port),
        "README bytes differ"
    );
}
