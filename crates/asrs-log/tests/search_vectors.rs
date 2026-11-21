use std::collections::HashMap;

use asrs_js::string::JsString;
use asrs_log::log::fuse::Fuse;
use asrs_log::log::{Log, LoggableType, search_fields};
use asrs_parity::json::Json;
use asrs_parity::vectors::{Record, check_local_records, check_records};

fn js(json: &Json) -> Result<JsString, String> {
    json.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| "expected a string".to_owned())
}

fn strings(json: Option<&Json>) -> Result<Vec<JsString>, String> {
    json.and_then(Json::as_array)
        .ok_or_else(|| "expected an array".to_owned())?
        .iter()
        .map(js)
        .collect()
}

fn indices(json: Option<&Json>) -> Result<Vec<usize>, String> {
    json.and_then(Json::as_array)
        .ok_or_else(|| "expected an array".to_owned())?
        .iter()
        .map(|v| {
            v.as_f64()
                .map(|n| n as usize)
                .ok_or_else(|| "expected an index".to_owned())
        })
        .collect()
}

fn show(key: &JsString) -> String {
    String::from_utf16_lossy(key.units())
}

fn check_scores(
    case: &str,
    keys: &[JsString],
    query: &JsString,
    record: &Record,
) -> Result<(), String> {
    let expected = strings(record.field("s"))?;
    let actual = if query.is_empty() {
        Vec::new()
    } else {
        Fuse::new(keys).search(query)
    };
    for (i, bits) in expected.iter().enumerate() {
        let bits = u64::from_str_radix(&show(bits), 16).map_err(|e| format!("{case}: {e}"))?;
        let score = actual.get(i).map(|r| r.score.to_bits());
        if score != Some(bits) {
            return Err(format!(
                "{case}: score {i} is {score:016x?}, oracle {bits:016x}"
            ));
        }
    }
    Ok(())
}

fn search_queries(
    check: fn(&str, &mut dyn FnMut(&Record) -> Result<(), String>),
    id: &str,
) -> usize {
    let mut sets: HashMap<String, (Log, Vec<JsString>)> = HashMap::new();
    let mut queries = 0usize;
    check(id, &mut |record: &Record| {
        let case = record.string("case").unwrap_or_default();
        if let Some(set) = case.strip_prefix("keys/") {
            let mut log = Log::new(false);
            for name in strings(record.field("in"))? {
                log.create_blank_field(&name, LoggableType::Raw);
            }
            let keys = strings(record.field("keys"))?;
            if log.get_field_keys() != keys {
                return Err(format!("{case}: getFieldKeys order differs"));
            }
            sets.insert(set.to_owned(), (log, keys));
            return Ok(());
        }
        let set = case
            .strip_prefix("query/")
            .and_then(|rest| rest.rsplit_once('/'))
            .map(|(set, _)| set)
            .ok_or_else(|| format!("{case}: unknown record"))?;
        let (log, keys) = sets
            .get(set)
            .ok_or_else(|| format!("{case}: key set {set} not loaded"))?;
        let query = js(record
            .field("q")
            .ok_or_else(|| format!("{case}: no query"))?)?;
        let expected: Vec<&JsString> = indices(record.field("r"))?
            .into_iter()
            .map(|i| {
                keys.get(i)
                    .ok_or_else(|| format!("{case}: index {i} out of range"))
            })
            .collect::<Result<_, _>>()?;
        let actual = search_fields(log, &query);
        queries += 1;
        if actual.iter().eq(expected.iter().copied()) {
            return check_scores(&case, keys, &query, record);
        }
        let at = (0..actual.len().max(expected.len()))
            .find(|&i| actual.get(i) != expected.get(i).copied())
            .unwrap_or(0);
        Err(format!(
            "{case} query {:?}: {} results vs {} expected; first difference at {at}: port {:?}, oracle {:?}",
            show(&query),
            actual.len(),
            expected.len(),
            actual.get(at).map(show),
            expected.get(at).map(|k| show(k)),
        ))
    });
    queries
}

#[test]
fn search_fields_vectors() {
    let queries = search_queries(|id, f| check_records(id, f), "log/search");
    assert!(queries >= 2000, "only {queries} queries checked");
}

#[test]
fn search_fields_public_vectors() {
    search_queries(|id, f| check_local_records(id, f), "log/search-public");
}
