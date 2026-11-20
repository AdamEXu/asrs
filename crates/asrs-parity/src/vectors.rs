use std::fs;
use std::path::{Path, PathBuf};

use crate::cve1::{DecodeError, Value};
use crate::diff;
use crate::json::{self, Json};
use crate::protocol::{Request, Response};

pub const VECTORS_ENV: &str = "ASRS_PARITY_VECTORS";
const ZSTD_LEVEL: i32 = 19;
const SHOWN_FAILURES: usize = 5;

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("asrs-parity lives in crates/ of the workspace")
        .to_path_buf()
}

/// `$ASRS_PARITY_VECTORS`, or `parity/vectors` in the workspace.
pub fn vectors_dir() -> PathBuf {
    std::env::var_os(VECTORS_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("parity/vectors"))
}

/// The vector of platform-dependent spec `spec` for the platform this test runs on, named
/// `<os>-<arch>` as the oracle's manifest names hosts (`windows-aarch64`).
pub fn platform_vector(spec: &str) -> String {
    format!("{spec}/{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The file holding vector `id`: `<id>.jsonl.zst`, or plain `<id>.jsonl`.
pub fn vector_path(dir: &Path, id: &str) -> Option<PathBuf> {
    [".jsonl.zst", ".jsonl"]
        .iter()
        .map(|ext| dir.join(format!("{id}{ext}")))
        .find(|p| p.is_file())
}

pub fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = if path.extension().is_some_and(|e| e == "zst") {
        zstd::stream::decode_all(bytes.as_slice())
            .map_err(|e| format!("{}: zstd: {e}", path.display()))?
    } else {
        bytes
    };
    String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn compress(text: &str) -> Vec<u8> {
    zstd::bulk::compress(text.as_bytes(), ZSTD_LEVEL)
        .expect("zstd compression of an in-memory buffer")
}

/// One line of a vector file.
#[derive(Clone, Debug)]
pub struct Record {
    pub line: usize,
    pub text: String,
    pub json: Json,
}

impl Record {
    pub fn field(&self, key: &str) -> Option<&Json> {
        self.json.get(key)
    }

    pub fn string(&self, key: &str) -> Option<String> {
        self.field(key)?.as_str().map(|s| s.to_string_lossy())
    }

    /// The CVE-1 value stored at `key`.
    pub fn value(&self, key: &str) -> Result<Value, DecodeError> {
        match self.field(key) {
            Some(json) => Value::from_json(json),
            None => Err(DecodeError(format!("record has no field {key:?}"))),
        }
    }
}

pub fn parse_records(text: &str) -> Result<Vec<Record>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty())
        .map(|(i, line)| {
            json::parse(line)
                .map(|json| Record {
                    line: i + 1,
                    text: line.to_owned(),
                    json,
                })
                .map_err(|e| format!("line {}: {e}", i + 1))
        })
        .collect()
}

pub fn load(id: &str) -> Result<Vec<Record>, String> {
    let dir = vectors_dir();
    let path = vector_path(&dir, id).ok_or_else(|| {
        format!(
            "no vectors for {id:?} in {} (generate them with `cargo xtask oracle gen {id}`)",
            dir.display()
        )
    })?;
    parse_records(&read_text(&path)?).map_err(|e| format!("{}: {e}", path.display()))
}

/// One `fn` vector: the recorded request and the record it came from.
pub struct FnCase<'a> {
    pub record: &'a Record,
    pub request: Request,
}

static UNDEFINED: Value = Value::Undefined;

impl FnCase<'_> {
    pub fn name(&self) -> &str {
        &self.request.name
    }

    /// Argument `i`, or `undefined` when the call passed fewer, as in JS.
    pub fn arg(&self, i: usize) -> &Value {
        self.request.args.get(i).unwrap_or(&UNDEFINED)
    }

    pub fn number(&self, i: usize) -> f64 {
        self.arg(i).as_f64().unwrap_or_else(|| {
            panic!(
                "line {}: argument {i} of {} is not a number",
                self.record.line, self.request.name
            )
        })
    }
}

/// Runs `f` on every case of `fn` vector `id` and panics with a structured report when any
/// response differs from the recorded one.
pub fn check_fn_vectors(id: &str, mut f: impl FnMut(&FnCase<'_>) -> Response) {
    let records = load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut failures = Vec::new();
    for record in &records {
        let request = Request::from_json(&record.json)
            .unwrap_or_else(|e| panic!("{id} line {}: bad request: {e}", record.line));
        let expected = Response::from_record(&record.json)
            .unwrap_or_else(|e| panic!("{id} line {}: bad response: {e}", record.line));
        if let Response::Error(message) = &expected {
            panic!(
                "{id} line {}: vector records an oracle error: {message}",
                record.line
            );
        }
        let case = FnCase { record, request };
        let actual = f(&case).encode();
        let expected = expected.encode();
        if actual != expected {
            let detail = diff::report_responses(&expected, &actual).unwrap_or_default();
            failures.push(format!(
                "line {}: {}\n  {}",
                record.line,
                case.request.encode(),
                detail.replace('\n', "\n  ")
            ));
        }
    }
    finish(id, records.len(), failures);
}

/// Runs `f` on every record of vector `id`; `Err` marks the record as failed.
pub fn check_records(id: &str, mut f: impl FnMut(&Record) -> Result<(), String>) {
    let records = load(id).unwrap_or_else(|e| panic!("{e}"));
    let failures: Vec<String> = records
        .iter()
        .filter_map(|r| {
            f(r).err()
                .map(|e| format!("line {}: {}", r.line, e.replace('\n', "\n  ")))
        })
        .collect();
    finish(id, records.len(), failures);
}

fn finish(id: &str, total: usize, failures: Vec<String>) {
    if total == 0 {
        panic!("vectors {id}: no cases");
    }
    if failures.is_empty() {
        return;
    }
    let mut message = format!("vectors {id}: {} of {total} cases differ", failures.len());
    for failure in failures.iter().take(SHOWN_FAILURES) {
        message.push_str("\n\n");
        message.push_str(failure);
    }
    panic!("{message}");
}

/// Declares a test that checks `fn` vector `$id` against a closure
/// `|case: &FnCase| -> Response`.
#[macro_export]
macro_rules! fn_vectors {
    ($(#[$meta:meta])* $name:ident, $id:expr, $f:expr $(,)?) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            $crate::vectors::check_fn_vectors($id, $f)
        }
    };
}

/// Declares a test that checks every record of vector `$id` with a closure
/// `|record: &Record| -> Result<(), String>`.
#[macro_export]
macro_rules! record_vectors {
    ($(#[$meta:meta])* $name:ident, $id:expr, $f:expr $(,)?) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            $crate::vectors::check_records($id, $f)
        }
    };
}
