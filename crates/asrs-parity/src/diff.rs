use std::fmt::Write;

use crate::cve1::Value;
use crate::json;
use crate::protocol::Response;
use crate::utf16::Utf16String;

const SHOWN: usize = 5;
const COLLECTED: usize = 100;
const SUMMARY_CHARS: usize = 96;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub path: String,
    pub detail: String,
}

/// Up to `limit` differences between two values, in document order.
pub fn differences(expected: &Value, actual: &Value, limit: usize) -> Vec<Difference> {
    let mut walker = Walker {
        path: String::from("$"),
        out: Vec::new(),
        limit,
    };
    walker.walk(expected, actual);
    walker.out
}

/// A readable report of how `actual` differs from `expected`, or `None` when they are equal.
pub fn report(expected: &Value, actual: &Value) -> Option<String> {
    let found = differences(expected, actual, COLLECTED);
    (!found.is_empty()).then(|| format_differences(&found))
}

fn format_differences(found: &[Difference]) -> String {
    let mut out = String::new();
    let total = if found.len() >= COLLECTED {
        format!("{COLLECTED}+")
    } else {
        found.len().to_string()
    };
    let _ = write!(
        out,
        "{total} difference{}",
        if found.len() == 1 { "" } else { "s" }
    );
    if found.len() > SHOWN {
        let _ = write!(out, ", first {SHOWN} shown");
    }
    for d in found.iter().take(SHOWN) {
        let _ = write!(out, "\n  at {}: {}", d.path, d.detail);
    }
    out
}

/// Compares two `fn` responses. Harness errors compare equal whatever their message, since
/// the message text is not upstream behaviour.
pub fn report_responses(expected: &str, actual: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (Response::parse(expected), Response::parse(actual)) {
        (Ok(Response::Error(_)), Ok(Response::Error(_))) => None,
        (Ok(Response::Ok(e)), Ok(Response::Ok(a)))
        | (Ok(Response::Throw(e)), Ok(Response::Throw(a))) => {
            Some(report(&e, &a).unwrap_or_else(|| "encodings differ but decode equal".to_owned()))
        }
        (Ok(e), Ok(a)) => Some(format!(
            "expected {}, actual {}",
            describe_response(&e),
            describe_response(&a)
        )),
        _ => report_lines(expected, actual),
    }
}

fn describe_response(response: &Response) -> String {
    match response {
        Response::Ok(v) => format!("ok {}", summary(v)),
        Response::Throw(v) => format!("throw {}", summary(v)),
        Response::Error(message) => {
            format!("error {}", Utf16String::from(message.as_str()).quoted())
        }
    }
}

/// Compares two JSON lines structurally, reading CVE-1 tags where they are valid.
pub fn report_lines(expected: &str, actual: &str) -> Option<String> {
    if expected == actual {
        return None;
    }
    match (json::parse(expected), json::parse(actual)) {
        (Ok(e), Ok(a)) => {
            let e = Value::from_json_lenient(&e);
            let a = Value::from_json_lenient(&a);
            Some(report(&e, &a).unwrap_or_else(|| {
                format!(
                    "same value, different text\n  expected {}\n  actual   {}",
                    truncate(expected),
                    truncate(actual)
                )
            }))
        }
        _ => Some(format!(
            "lines differ\n  expected {}\n  actual   {}",
            truncate(expected),
            truncate(actual)
        )),
    }
}

/// Compares two JSONL streams line by line and reports the first `shown` differing lines.
/// `compare` decides whether two lines match and how to describe a mismatch.
pub fn report_jsonl(
    expected: &str,
    actual: &str,
    shown: usize,
    compare: impl Fn(&str, &str) -> Option<String>,
) -> Option<String> {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let mut differing = Vec::new();
    for i in 0..expected.len().max(actual.len()) {
        let detail = match (expected.get(i), actual.get(i)) {
            (Some(e), Some(a)) => compare(e, a),
            (Some(_), None) => Some("missing in actual".to_owned()),
            (None, Some(a)) => Some(format!("extra line {}", truncate(a))),
            (None, None) => None,
        };
        if let Some(detail) = detail {
            differing.push((i + 1, detail));
        }
    }
    if differing.is_empty() {
        return None;
    }
    let mut out = format!(
        "{} of {} lines differ (expected {} lines, actual {})",
        differing.len(),
        expected.len().max(actual.len()),
        expected.len(),
        actual.len()
    );
    for (line, detail) in differing.iter().take(shown) {
        let _ = write!(out, "\nline {line}: {}", indent(detail));
    }
    Some(out)
}

/// `cmp`-style report for raw output bytes: first differing offset with hex context.
pub fn report_bytes(expected: &[u8], actual: &[u8]) -> Option<String> {
    if expected == actual {
        return None;
    }
    let offset = expected
        .iter()
        .zip(actual)
        .position(|(e, a)| e != a)
        .unwrap_or(expected.len().min(actual.len()));
    let start = offset.saturating_sub(8);
    let window = |bytes: &[u8]| -> String {
        let end = (offset + 9).min(bytes.len());
        bytes
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if start + i == offset {
                    format!("[{b:02x}]")
                } else {
                    format!("{b:02x}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let byte = |bytes: &[u8]| {
        bytes
            .get(offset)
            .map_or_else(|| "end of data".to_owned(), |b| format!("0x{b:02x}"))
    };
    Some(format!(
        "bytes differ at offset {offset} (0x{offset:x}): expected {}, actual {}; lengths {} and {}\n  expected {start:#x}: {}\n  actual   {start:#x}: {}",
        byte(expected),
        byte(actual),
        expected.len(),
        actual.len(),
        window(expected),
        window(actual)
    ))
}

fn indent(text: &str) -> String {
    text.replace('\n', "\n  ")
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= SUMMARY_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(SUMMARY_CHARS).collect();
    format!("{cut}...")
}

pub fn summary(value: &Value) -> String {
    match value {
        Value::Number(bits) => describe_number(*bits),
        Value::Date(bits) => format!("Date {}", describe_number(*bits)),
        _ => format!("{} {}", value.kind(), truncate(&value.encode())),
    }
}

pub fn describe_number(bits: u64) -> String {
    format!("0x{bits:016x} ({})", decimal(f64::from_bits(bits)))
}

fn decimal(x: f64) -> String {
    if x.is_nan() {
        "NaN".to_owned()
    } else if x == 0.0 && x.is_sign_negative() {
        "-0".to_owned()
    } else {
        format!("{x:?}")
    }
}

struct Walker {
    path: String,
    out: Vec<Difference>,
    limit: usize,
}

impl Walker {
    fn push(&mut self, detail: String) {
        if self.out.len() < self.limit {
            self.out.push(Difference {
                path: self.path.clone(),
                detail,
            });
        }
    }

    fn full(&self) -> bool {
        self.out.len() >= self.limit
    }

    fn at(&mut self, segment: &str, f: impl FnOnce(&mut Self)) {
        let len = self.path.len();
        self.path.push_str(segment);
        f(self);
        self.path.truncate(len);
    }

    fn walk(&mut self, expected: &Value, actual: &Value) {
        if self.full() || expected == actual {
            return;
        }
        match (expected, actual) {
            (Value::Number(e), Value::Number(a)) => self.push(number_detail("", *e, *a)),
            (Value::Date(e), Value::Date(a)) => self.push(number_detail("Date ", *e, *a)),
            (Value::String(e), Value::String(a)) => self.push(string_detail(e, a)),
            (Value::BigInt(e), Value::BigInt(a)) => {
                self.push(format!("expected {e}n, actual {a}n"))
            }
            (Value::Array(e), Value::Array(a)) | (Value::Set(e), Value::Set(a)) => self.list(e, a),
            (Value::TypedArray(en, e), Value::TypedArray(an, a)) => {
                if en == an {
                    self.list(e, a);
                } else {
                    self.push(format!("expected {en}, actual {an}"));
                }
            }
            (Value::Object(e), Value::Object(a)) => self.pairs(e, a),
            (Value::Instance(en, e), Value::Instance(an, a)) => {
                if en == an {
                    self.pairs(e, a);
                } else {
                    self.push(format!(
                        "expected instance of {en:?}, actual instance of {an:?}"
                    ));
                }
            }
            (Value::Bytes(e), Value::Bytes(a))
            | (Value::ArrayBuffer(e), Value::ArrayBuffer(a))
            | (Value::DataView(e), Value::DataView(a)) => {
                let detail = report_bytes(e, a).expect("bytes differ");
                self.push(detail.lines().next().unwrap_or_default().to_owned());
            }
            (Value::Map(e), Value::Map(a)) => {
                for (i, ((ek, ev), (ak, av))) in e.iter().zip(a).enumerate() {
                    self.at(&format!("{{{i}}}.key"), |w| w.walk(ek, ak));
                    self.at(&format!("{{{i}}}.value"), |w| w.walk(ev, av));
                }
                if e.len() != a.len() {
                    self.push(format!(
                        "Map size: expected {}, actual {}",
                        e.len(),
                        a.len()
                    ));
                }
            }
            (Value::Error(en, em), Value::Error(an, am)) => {
                self.at(".name", |w| {
                    w.walk(&Value::String(en.clone()), &Value::String(an.clone()))
                });
                self.at(".message", |w| {
                    w.walk(&Value::String(em.clone()), &Value::String(am.clone()))
                });
            }
            (Value::Boxed(e), Value::Boxed(a)) => self.at(".valueOf()", |w| w.walk(e, a)),
            _ => self.push(format!(
                "expected {}, actual {}",
                summary(expected),
                summary(actual)
            )),
        }
    }

    fn list(&mut self, expected: &[Value], actual: &[Value]) {
        for (i, (e, a)) in expected.iter().zip(actual).enumerate() {
            if self.full() {
                return;
            }
            if e != a {
                self.at(&format!("[{i}]"), |w| w.walk(e, a));
            }
        }
        if expected.len() != actual.len() {
            let (longer, which) = if expected.len() > actual.len() {
                (expected, "missing")
            } else {
                (actual, "extra")
            };
            let first = expected.len().min(actual.len());
            self.push(format!(
                "length: expected {}, actual {}; first {which} element [{first}] is {}",
                expected.len(),
                actual.len(),
                summary(&longer[first])
            ));
        }
    }

    fn pairs(&mut self, expected: &[(Utf16String, Value)], actual: &[(Utf16String, Value)]) {
        let keys = |pairs: &[(Utf16String, Value)]| {
            pairs.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
        };
        let (ekeys, akeys) = (keys(expected), keys(actual));
        if ekeys != akeys {
            let missing: Vec<String> = ekeys
                .iter()
                .filter(|k| !akeys.contains(k))
                .map(Utf16String::quoted)
                .collect();
            let extra: Vec<String> = akeys
                .iter()
                .filter(|k| !ekeys.contains(k))
                .map(Utf16String::quoted)
                .collect();
            if !missing.is_empty() {
                self.push(format!(
                    "missing key{} {}",
                    plural(missing.len()),
                    missing.join(", ")
                ));
            }
            if !extra.is_empty() {
                self.push(format!(
                    "extra key{} {}",
                    plural(extra.len()),
                    extra.join(", ")
                ));
            }
            let common_e: Vec<&Utf16String> = ekeys.iter().filter(|k| akeys.contains(k)).collect();
            let common_a: Vec<&Utf16String> = akeys.iter().filter(|k| ekeys.contains(k)).collect();
            if let Some(i) = common_e.iter().zip(&common_a).position(|(e, a)| e != a) {
                self.push(format!(
                    "key order differs: expected {} before {}",
                    common_e[i].quoted(),
                    common_a[i].quoted()
                ));
            }
        }
        for (key, e) in expected {
            if self.full() {
                return;
            }
            if let Some((_, a)) = actual.iter().find(|(k, _)| k == key)
                && e != a
            {
                self.at(&key_segment(key), |w| w.walk(e, a));
            }
        }
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn key_segment(key: &Utf16String) -> String {
    let plain = key.as_string().filter(|s| {
        let mut chars = s.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    });
    match plain {
        Some(s) => format!(".{s}"),
        None => format!("[{}]", key.quoted()),
    }
}

fn number_detail(prefix: &str, expected: u64, actual: u64) -> String {
    let mut detail = format!(
        "expected {prefix}{}, actual {prefix}{}",
        describe_number(expected),
        describe_number(actual)
    );
    let (e, a) = (f64::from_bits(expected), f64::from_bits(actual));
    if e.is_finite() && a.is_finite() && e.is_sign_negative() == a.is_sign_negative() {
        let ulps = expected.abs_diff(actual);
        let _ = write!(detail, "; {ulps} ulp{} apart", plural(ulps as usize));
    } else {
        let _ = write!(detail, "; xor 0x{:016x}", expected ^ actual);
    }
    detail
}

fn string_detail(expected: &Utf16String, actual: &Utf16String) -> String {
    let (e, a) = (expected.units(), actual.units());
    let i = e
        .iter()
        .zip(a)
        .position(|(x, y)| x != y)
        .unwrap_or(e.len().min(a.len()));
    let unit = |s: &[u16]| {
        s.get(i)
            .map_or_else(|| "end of string".to_owned(), |u| format!("U+{u:04X}"))
    };
    let excerpt = |s: &[u16]| {
        let start = i.saturating_sub(16);
        let end = (i + 16).min(s.len());
        let mut text = Utf16String::from_units(s[start.min(s.len())..end].to_vec()).quoted();
        if start > 0 {
            text.insert_str(0, "...");
        }
        if end < s.len() {
            text.push_str("...");
        }
        text
    };
    format!(
        "strings differ at code unit {i}: expected {}, actual {}; lengths {} and {}\n    expected {}\n    actual   {}",
        unit(e),
        unit(a),
        e.len(),
        a.len(),
        excerpt(e),
        excerpt(a)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(text: &str) -> Value {
        Value::decode(text).unwrap()
    }

    #[test]
    fn equal_values_have_no_report() {
        let v = decode(r#"{"o":[["a",[1,2]]]}"#);
        assert_eq!(report(&v, &v.clone()), None);
    }

    #[test]
    fn planted_bit_flip_names_field_sample_and_bits() {
        let expected = decode(
            r#"{"o":[["key","/Drive/Pose"],["values",{"ta":["Float64Array",[0.5,0.25,0.125]]}]]}"#,
        );
        let planted = decode(
            r#"{"o":[["key","/Drive/Pose"],["values",{"ta":["Float64Array",[0.5,{"n":"3fd0000000000001"},0.125]]}]]}"#,
        );
        assert_eq!(
            report(&expected, &planted).unwrap(),
            "1 difference\n  at $.values[1]: expected 0x3fd0000000000000 (0.25), actual 0x3fd0000000000001 (0.25000000000000006); 1 ulp apart"
        );
    }

    #[test]
    fn reports_structure_changes() {
        let expected = decode(r#"{"o":[["a",1],["b",[1,2,3]],["my key","x\ud800y"]]}"#);
        let actual = decode(r#"{"o":[["b",[1,2]],["a",-0],["my key","x\udc00y"],["c",null]]}"#);
        assert_eq!(
            report(&expected, &actual).unwrap(),
            "5 differences\n  at $: extra key \"c\"\n  at $: key order differs: expected \"a\" before \"b\"\n  at $.a: expected 0x3ff0000000000000 (1.0), actual 0x8000000000000000 (-0); xor 0xbff0000000000000\n  at $.b: length: expected 3, actual 2; first missing element [2] is 0x4008000000000000 (3.0)\n  at $[\"my key\"]: strings differ at code unit 1: expected U+D800, actual U+DC00; lengths 3 and 3\n    expected \"x\\ud800y\"\n    actual   \"x\\udc00y\""
        );
    }

    #[test]
    fn response_errors_compare_by_kind() {
        assert_eq!(
            report_responses(r#"{"error":"a"}"#, r#"{"error":"b"}"#),
            None
        );
        assert_eq!(
            report_responses(r#"{"ok":null}"#, r#"{"throw":{"err":["TypeError","x"]}}"#).unwrap(),
            "expected ok null null, actual throw Error {\"err\":[\"TypeError\",\"x\"]}"
        );
    }

    #[test]
    fn bytes_report_offset_and_context() {
        assert_eq!(
            report_bytes(b"\x00\x01\x02\x03", b"\x00\x01\x09\x03\x04").unwrap(),
            "bytes differ at offset 2 (0x2): expected 0x02, actual 0x09; lengths 4 and 5\n  expected 0x0: 00 01 [02] 03\n  actual   0x0: 00 01 [09] 03 04"
        );
    }

    #[test]
    fn jsonl_report_counts_lines() {
        let report = report_jsonl(
            "{\"ok\":1}\n{\"ok\":2}\n",
            "{\"ok\":1}\n{\"ok\":3}\n",
            5,
            report_responses,
        )
        .unwrap();
        assert!(report.starts_with("1 of 2 lines differ"), "{report}");
        assert!(report.contains("line 2:"), "{report}");
    }
}
