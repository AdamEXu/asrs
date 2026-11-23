use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use crate::ci::{skip_in_ci, skip_notice};
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

/// x86_64 makes NaNs with other sign and payload bits than the aarch64 hosts every vector
/// comes from (its default NaN is `0xfff8000000000000`), so x86_64 builds compare NaN bits
/// loosely until there are x86_64 vectors. Every other bit is still compared exactly.
const LOOSE_NAN_BITS: bool = cfg!(target_arch = "x86_64");
static LOOSE_NAN_NOTICE: Once = Once::new();

/// Whether CVE-1 texts `expected` and `actual` match: equal, or on x86_64 equal once every
/// NaN is the canonical `0x7ff8000000000000`.
pub fn encoded_match(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    if !LOOSE_NAN_BITS || canonical_nans(expected) != canonical_nans(actual) {
        return false;
    }
    LOOSE_NAN_NOTICE.call_once(|| {
        skip_notice(
            "exact NaN bits",
            "x86_64 NaN sign and payload bits differ from the aarch64 vectors",
        );
    });
    true
}

/// [`encoded_match`] for decoded values.
pub fn values_match(expected: &Value, actual: &Value) -> bool {
    expected == actual || encoded_match(&expected.encode(), &actual.encode())
}

/// Every vector is generated on macOS arm64. V8 takes `Math.pow` from the C library, and so
/// does `asrs_js::math::pow`, but glibc and the UCRT round some powers one ulp away from
/// macOS's libm, so V8 itself gives other bits there. Off macOS, tests of values derived from
/// `pow` therefore accept a one-ulp difference until those platforms have their own vectors.
const LOOSE_POW_BITS: bool = !cfg!(all(target_os = "macos", target_arch = "aarch64"));
static LOOSE_POW_NOTICE: Once = Once::new();

/// Whether the bits of number `actual` match `expected`, allowing off macOS the one-ulp
/// difference of a platform `pow`.
pub fn pow_bits_match(expected: u64, actual: u64) -> bool {
    expected == actual || (LOOSE_POW_BITS && one_ulp_apart(expected, actual) && pow_notice())
}

/// [`values_match`], allowing off macOS a one-ulp difference in any number, as a value derived
/// from a platform `pow` may have.
pub fn pow_values_match(expected: &Value, actual: &Value) -> bool {
    values_match(expected, actual)
        || (LOOSE_POW_BITS && values_near(expected, actual) && pow_notice())
}

fn pow_notice() -> bool {
    LOOSE_POW_NOTICE.call_once(|| {
        skip_notice(
            "the last bit of pow results",
            "V8 and asrs use this platform's C library pow, and the vectors come from macOS",
        );
    });
    true
}

fn one_ulp_apart(a: u64, b: u64) -> bool {
    let finite = |bits: u64| f64::from_bits(bits).is_finite();
    finite(a) && finite(b) && a >> 63 == b >> 63 && a.abs_diff(b) == 1
}

fn values_near(expected: &Value, actual: &Value) -> bool {
    let all_near = |a: &[Value], b: &[Value]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_near(x, y))
    };
    let fields_near = |a: &[(_, Value)], b: &[(_, Value)]| {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|((ka, va), (kb, vb))| ka == kb && values_near(va, vb))
    };
    match (expected, actual) {
        (Value::Number(a), Value::Number(b)) => {
            one_ulp_apart(*a, *b) || values_match(expected, actual)
        }
        (Value::Array(a), Value::Array(b)) | (Value::Set(a), Value::Set(b)) => all_near(a, b),
        (Value::TypedArray(ta, a), Value::TypedArray(tb, b)) => ta == tb && all_near(a, b),
        (Value::Object(a), Value::Object(b)) => fields_near(a, b),
        (Value::Instance(ca, a), Value::Instance(cb, b)) => ca == cb && fields_near(a, b),
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ka, va), (kb, vb))| values_near(ka, kb) && values_near(va, vb))
        }
        _ => values_match(expected, actual),
    }
}

fn canonical_nans(text: &str) -> String {
    const NUMBER: &str = "\"n\":\"";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(NUMBER) {
        let (head, tail) = rest.split_at(i + NUMBER.len());
        out.push_str(head);
        rest = tail;
        if let Some(bits) = tail.get(..16).and_then(|h| u64::from_str_radix(h, 16).ok())
            && f64::from_bits(bits).is_nan()
        {
            out.push_str("7ff8000000000000");
            rest = &tail[16..];
        }
    }
    out.push_str(rest);
    out
}

/// Where vectors derived from public logs without a redistribution license live:
/// `$ASRS_CACHE/vectors`, next to the downloaded logs. They are never committed.
pub fn local_vectors_dir() -> PathBuf {
    crate::cache::cache_dir().0.join("vectors")
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
    load_file(&path)
}

fn load_file(path: &Path) -> Result<Vec<Record>, String> {
    parse_records(&read_text(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

/// Local vector `id` from [`local_vectors_dir`], or `None` after a skip notice when it has not
/// been generated on this machine or the run is under `ASRS_CI=1`.
pub fn load_local(id: &str) -> Option<Vec<Record>> {
    if skip_in_ci(
        id,
        "a local vector generated from the downloaded public logs",
    ) {
        return None;
    }
    let dir = local_vectors_dir();
    let Some(path) = vector_path(&dir, id) else {
        skip_notice(
            id,
            &format!(
                "local vector not generated in {}; run `cargo xtask corpus fetch` and then \
                 `cargo xtask oracle gen --local`",
                dir.display()
            ),
        );
        return None;
    };
    Some(load_file(&path).unwrap_or_else(|e| panic!("{e}")))
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
pub fn check_fn_vectors(id: &str, f: impl FnMut(&FnCase<'_>) -> Response) {
    check_fn_records(id, &load(id).unwrap_or_else(|e| panic!("{e}")), f);
}

/// [`check_fn_vectors`] for a local vector; skips when [`load_local`] finds none.
pub fn check_local_fn_vectors(id: &str, f: impl FnMut(&FnCase<'_>) -> Response) {
    if let Some(records) = load_local(id) {
        check_fn_records(id, &records, f);
    }
}

/// [`check_fn_vectors`] for this platform's vector of platform-dependent spec `spec`. x86_64
/// hosts have no generated platform vectors yet, so there a missing one is skipped.
pub fn check_platform_fn_vectors(spec: &str, f: impl FnMut(&FnCase<'_>) -> Response) {
    let id = platform_vector(spec);
    if vector_path(&vectors_dir(), &id).is_none() && skip_on_x86_64(&id) {
        return;
    }
    check_fn_vectors(&id, f);
}

/// Returns true, after a notice, on x86_64, so a test whose vectors hold platform-dependent
/// results (from the C library's `pow`, as in V8) returns early until x86_64 vectors exist.
pub fn skip_on_x86_64(what: &str) -> bool {
    let skip = std::env::consts::ARCH == "x86_64";
    if skip {
        skip_notice(
            what,
            "platform-dependent results, and x86_64 vectors have not been generated yet",
        );
    }
    skip
}

fn check_fn_records(id: &str, records: &[Record], mut f: impl FnMut(&FnCase<'_>) -> Response) {
    let mut failures = Vec::new();
    for record in records {
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
        if !encoded_match(&expected, &actual) {
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
pub fn check_records(id: &str, f: impl FnMut(&Record) -> Result<(), String>) {
    check_record_list(id, &load(id).unwrap_or_else(|e| panic!("{e}")), f);
}

/// [`check_records`] for a local vector; skips when [`load_local`] finds none.
pub fn check_local_records(id: &str, f: impl FnMut(&Record) -> Result<(), String>) {
    if let Some(records) = load_local(id) {
        check_record_list(id, &records, f);
    }
}

pub(crate) fn check_record_list(
    id: &str,
    records: &[Record],
    mut f: impl FnMut(&Record) -> Result<(), String>,
) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_near_allows_one_ulp_in_numbers_only() {
        let one = 1.0f64.to_bits();
        let arr = |bits: &[u64]| Value::Array(bits.iter().map(|&b| Value::Number(b)).collect());
        assert!(values_near(&arr(&[one, 2]), &arr(&[one + 1, 2])));
        assert!(values_near(&arr(&[one]), &arr(&[one - 1])));
        assert!(!values_near(&arr(&[one]), &arr(&[one + 2])));
        assert!(!values_near(&arr(&[one]), &arr(&[one, one])));
        assert!(!one_ulp_apart(0, 1 << 63));
        assert!(!one_ulp_apart(
            f64::INFINITY.to_bits(),
            f64::INFINITY.to_bits() - 1
        ));
    }

    #[test]
    fn canonical_nans_rewrites_only_nan_numbers() {
        let text = r#"[{"n":"fff8000000000000"},{"n":"7ff8000000000123"},{"n":"7ff0000000000000"},{"n":"3ff0000000000000"},"n"]"#;
        assert_eq!(
            canonical_nans(text),
            r#"[{"n":"7ff8000000000000"},{"n":"7ff8000000000000"},{"n":"7ff0000000000000"},{"n":"3ff0000000000000"},"n"]"#
        );
    }
}
