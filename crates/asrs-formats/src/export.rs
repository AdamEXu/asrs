//! `src/hub/exportWorker.ts` (F10): field selection, timestamp sampling and the CSV
//! writers; `generateWPILOG` (F11) below; `generateMCAP` (F12) is [`crate::mcap_export`].

use asrs_js::collation::locale_compare_numeric;
use asrs_js::json;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_key_utils::filter_field_by_prefixes;
use asrs_log::log::log_util::get_log_value_text_at;
use asrs_log::log::robot_state::AKIT_TIMESTAMP_KEYS;
use asrs_log::log::{Log, LogFieldTree, LogValueSetAny, LoggableType};
use asrs_log::realm::ObjectPrototype;
use asrs_log::util::{clean_float, upper_bound};

use crate::wpilog_encoder::{Record, Sample, WpilogEncoder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    CsvTable,
    CsvList,
    Wpilog,
    Mcap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    Changes,
    Fixed,
    Akit,
}

/// `ExportOptions`.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub format: Format,
    pub sampling: Sampling,
    /// Milliseconds.
    pub sampling_period: f64,
    pub prefixes: JsString,
    pub include_generated: bool,
}

/// What main writes: a string result as UTF-8, a byte result as is.
pub enum Output {
    Text(JsString),
    Bytes(Vec<u8>),
}

impl Output {
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Output::Text(text) => asrs_js::string::text_encode(&text),
            Output::Bytes(bytes) => bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// The worker posted `reject()`: an exception or a missing AdvantageKit timestamp.
    Rejected,
    /// A branch or input the port does not model yet.
    Unported(&'static str),
}

/// `onmessage`: select fields, compute timestamps, dispatch on the format. `progress` gets every
/// value the worker posts, in order.
pub fn export(
    log: &Log,
    options: &ExportOptions,
    progress: &mut dyn FnMut(f64),
) -> Result<Output, ExportError> {
    let fields = select_fields(log, options)?;
    let timestamps = sample_timestamps(log, options)?;
    match options.format {
        Format::CsvTable => Ok(Output::Text(csv_table(
            log,
            &fields,
            &timestamps,
            progress,
        )?)),
        Format::CsvList => Ok(Output::Text(csv_list(log, &fields, &timestamps, progress)?)),
        Format::Wpilog => Ok(Output::Bytes(wpilog(log, &fields, &timestamps, progress)?)),
        Format::Mcap => Ok(Output::Bytes(crate::mcap_export::generate_mcap(
            log,
            &fields,
            &timestamps,
            progress,
        )?)),
    }
}

/// `(fieldIndex + index / length) / fields.length`.
pub(crate) fn fraction(field: usize, index: usize, length: usize, fields: usize) -> f64 {
    (field as f64 + index as f64 / length as f64) / fields as f64
}

/// The worker's field list: the tree flattened parent-first with numeric `localeCompare`
/// at each level, `Empty` fields removed, then `filterFieldByPrefixes`.
fn select_fields(log: &Log, options: &ExportOptions) -> Result<Vec<JsString>, ExportError> {
    let mut fields = Vec::new();
    // Every export runs in a fresh worker realm.
    let root = log
        .get_field_tree(
            options.include_generated,
            &JsString::new(),
            &mut ObjectPrototype::default(),
        )
        .map_err(|_| ExportError::Rejected)?;
    flatten_tree(&root, &mut fields);
    fields.retain(|field| log.get_type(field) != Some(LoggableType::Empty));
    Ok(filter_field_by_prefixes(
        &fields,
        &options.prefixes,
        options.format == Format::Wpilog,
        false,
    ))
}

fn flatten_tree(tree: &JsObject<LogFieldTree>, fields: &mut Vec<JsString>) {
    let mut keys: Vec<JsString> = tree.keys().cloned().collect();
    array_sort(&mut keys, locale_compare_numeric);
    for key in keys {
        let node = tree.get_key(&key).expect("key from keys()");
        if let Some(full_key) = &node.full_key {
            fields.push(full_key.clone());
        }
        if !node.children.is_empty() {
            flatten_tree(&node.children, fields);
        }
    }
}

/// The worker's timestamp list for fixed or AdvantageKit sampling, or `None` for the
/// per-field sampling of `changes`. `akit` rejects when no key matches or it is not a
/// number field.
pub fn sample_timestamps(
    log: &Log,
    options: &ExportOptions,
) -> Result<Option<Vec<f64>>, ExportError> {
    match options.sampling {
        Sampling::Fixed => {
            let step = options.sampling_period / 1000.0;
            let [start, end] = log.get_timestamp_range();
            let mut timestamps = Vec::new();
            let mut timestamp = start;
            while timestamp <= end {
                timestamps.push(clean_float(timestamp));
                timestamp += step;
            }
            Ok(Some(timestamps))
        }
        Sampling::Akit => {
            let key = log
                .get_field_keys()
                .into_iter()
                .find(is_akit_timestamp_key)
                .ok_or(ExportError::Rejected)?;
            log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)
                .map(|set| Some(set.timestamps))
                .ok_or(ExportError::Rejected)
        }
        Sampling::Changes => Ok(None),
    }
}

fn is_akit_timestamp_key(key: &JsString) -> bool {
    AKIT_TIMESTAMP_KEYS
        .iter()
        .any(|&kit| key == &JsString::from(kit))
}

/// `getLogValueText(value, fieldType).replaceAll(",", ";")`, where `index` is the sample or `None`
/// for `null`. An `undefined` sample has no text, and reading `replaceAll` from it throws.
fn cell_text(set: &LogValueSetAny, index: Option<usize>) -> Result<JsString, ExportError> {
    if let LogValueSetAny::Foreign(_, set) = set
        && index.and_then(|index| set.values.get(index)) == Some(&JsValue::Undefined)
    {
        return Err(ExportError::Rejected);
    }
    let text = index.and_then(|index| get_log_value_text_at(set, index));
    Ok(JsString::from(
        text.unwrap_or_else(|| "null".to_owned()).replace(',', ";"),
    ))
}

/// `generateCsvTable`: the timestamp union for `changes`, the last sample at or before
/// each timestamp otherwise, `getLogValueText` with commas folded to semicolons, and no
/// quoting or trailing newline.
fn csv_table(
    log: &Log,
    fields: &[JsString],
    timestamps: &Option<Vec<f64>>,
    progress: &mut dyn FnMut(f64),
) -> Result<JsString, ExportError> {
    let timestamps = timestamps
        .clone()
        .unwrap_or_else(|| log.get_timestamps(fields, None));
    let mut data: Vec<Vec<JsString>> = Vec::with_capacity(timestamps.len() + 1);
    data.push(vec![JsString::from("Timestamp")]);
    for &timestamp in &timestamps {
        data.push(vec![JsString::from(number::to_string(timestamp))]);
    }
    for (field_index, field) in fields.iter().enumerate() {
        data[0].push(field.clone());
        let field_data = log.get_range(field, f64::NEG_INFINITY, f64::INFINITY, None);
        let mut next_index = 0usize;
        for (index, &timestamp) in timestamps.iter().enumerate() {
            let Some(set) = &field_data else { continue };
            next_index = upper_bound(set.timestamps(), timestamp, next_index);
            data[index + 1].push(cell_text(set, next_index.checked_sub(1))?);
            progress(fraction(field_index, index, timestamps.len(), fields.len()));
        }
    }
    let text = join_rows(&data);
    progress(1.0);
    Ok(text)
}

/// `generateCsvList`: one row per sample per field, a stable sort by timestamp, the
/// header spliced in front, and the same no-quoting text rules.
fn csv_list(
    log: &Log,
    fields: &[JsString],
    timestamps: &Option<Vec<f64>>,
    progress: &mut dyn FnMut(f64),
) -> Result<JsString, ExportError> {
    let mut rows: Vec<(f64, JsString, JsString)> = Vec::new();
    for (field_index, field) in fields.iter().enumerate() {
        let Some(field_data) = log.get_range(field, f64::NEG_INFINITY, f64::INFINITY, None) else {
            continue;
        };
        match timestamps {
            None => {
                for index in 0..field_data.len() {
                    rows.push((
                        field_data.timestamps()[index],
                        field.clone(),
                        cell_text(&field_data, Some(index))?,
                    ));
                    progress(fraction(field_index, index, field_data.len(), fields.len()));
                }
            }
            Some(timestamps) => {
                let mut next_index = 0usize;
                for (index, &timestamp) in timestamps.iter().enumerate() {
                    next_index = upper_bound(field_data.timestamps(), timestamp, next_index);
                    rows.push((
                        timestamp,
                        field.clone(),
                        cell_text(&field_data, next_index.checked_sub(1))?,
                    ));
                    progress(fraction(field_index, index, timestamps.len(), fields.len()));
                }
            }
        }
    }
    array_sort(&mut rows, |a, b| a.0 - b.0);
    let mut out = JsString::from("Timestamp,Key,Value");
    for (timestamp, key, value) in rows {
        out.push_unit(u16::from(b'\n'));
        out.push_str(&JsString::from(number::to_string(timestamp)));
        out.push_unit(u16::from(b','));
        out.push_str(&key);
        out.push_unit(u16::from(b','));
        out.push_str(&value);
    }
    progress(1.0);
    Ok(out)
}

/// `generateWPILOG`: one start record per filtered field at its first sample
/// timestamp, then a data record per sample (or per sampled timestamp), all
/// sorted by the f64 microsecond timestamp inside `getEncoded(true)`.
fn wpilog(
    log: &Log,
    fields: &[JsString],
    timestamps: &Option<Vec<f64>>,
    progress: &mut dyn FnMut(f64),
) -> Result<Vec<u8>, ExportError> {
    let mut encoder = WpilogEncoder::new(&JsString::from("AdvantageScope"));
    for (field_index, field) in fields.iter().enumerate() {
        let Some(field_data) = log.get_range(field, f64::NEG_INFINITY, f64::INFINITY, None) else {
            continue;
        };
        // Upstream skips on `fieldType === undefined`, which `null` never is, so a
        // missing type keeps going with an empty base type string.
        let field_type = log.get_type(field);
        let entry_id = (field_index + 1) as f64;
        let mut type_str = JsString::from(match field_type {
            Some(LoggableType::Raw) => "raw",
            Some(LoggableType::Boolean) => "boolean",
            Some(LoggableType::Number) => "double",
            Some(LoggableType::String) => "string",
            Some(LoggableType::BooleanArray) => "boolean[]",
            Some(LoggableType::NumberArray) => "double[]",
            Some(LoggableType::StringArray) => "string[]",
            Some(LoggableType::Empty) | None => "",
        });
        if let Some(wpilib_type) = log.get_wpilib_type(field) {
            type_str = wpilib_type.clone();
            // NT4 uses "int" but wpilog uses "int64".
            if type_str == JsString::from("int") {
                type_str = JsString::from("int64");
            }
            if type_str == JsString::from("int[]") {
                type_str = JsString::from("int64[]");
            }
        }
        let metadata = wpilog_metadata(&log.get_metadata_string(field));
        let start_timestamp = if field_data.timestamps().is_empty() {
            0.0
        } else {
            field_data.timestamps()[0]
        };
        encoder.add(Record::control_start(
            start_timestamp * 1000000.0,
            entry_id,
            field,
            &type_str,
            &metadata,
        ));
        let type_name = type_str.as_string();
        let mut add_value = |timestamp: f64, index: usize| {
            let value = Sample::at(&field_data, index);
            let timestamp_micros = timestamp * 1000000.0;
            let record = match type_name.as_deref() {
                Some("boolean") => Record::boolean(entry_id, timestamp_micros, &value),
                Some("int64") => Record::integer(entry_id, timestamp_micros, &value)?,
                Some("float") => Record::float(entry_id, timestamp_micros, &value),
                Some("double") => Record::double(entry_id, timestamp_micros, &value),
                Some("string" | "json") => Record::string(entry_id, timestamp_micros, &value),
                Some("boolean[]") => Record::boolean_array(entry_id, timestamp_micros, &value)?,
                Some("int64[]") => Record::integer_array(entry_id, timestamp_micros, &value)?,
                Some("float[]") => Record::float_array(entry_id, timestamp_micros, &value)?,
                Some("double[]") => Record::double_array(entry_id, timestamp_micros, &value)?,
                Some("string[]") => Record::string_array(entry_id, timestamp_micros, &value)?,
                _ => Record::raw(entry_id, timestamp_micros, &value),
            };
            encoder.add(record);
            Ok(())
        };
        match timestamps {
            // Upstream's per-sample guard drops every value when the resolved type
            // string is empty; the sampled path has no such guard and emits raw
            // records.
            None if type_str.is_empty() => {}
            None => {
                for index in 0..field_data.len() {
                    add_value(field_data.timestamps()[index], index)?;
                    progress(fraction(field_index, index, field_data.len(), fields.len()));
                }
            }
            Some(timestamps) => {
                let mut next_index = 0usize;
                for (index, &timestamp) in timestamps.iter().enumerate() {
                    next_index = upper_bound(field_data.timestamps(), timestamp, next_index);
                    if next_index != 0 {
                        add_value(timestamp, next_index - 1)?;
                    }
                    progress(fraction(field_index, index, timestamps.len(), fields.len()));
                }
            }
        }
    }
    progress(1.0);
    encoder.get_encoded(true)
}

/// The `exporter` merge upstream does on the metadata string: `""` becomes
/// `{"exporter":"AdvantageScope"}`; anything else is `JSON.parse`d, has its
/// `exporter` set (in place if present, appended if not) and is `JSON.stringify`d
/// back. A parse failure or a non-object result throws and leaves the string
/// unchanged (a parsed array keeps the property, which `JSON.stringify` does not
/// see, so it still comes out normalized).
fn wpilog_metadata(metadata: &JsString) -> JsString {
    if metadata.is_empty() {
        return JsString::from("{\"exporter\":\"AdvantageScope\"}");
    }
    let Ok(mut parsed) = json::parse(metadata) else {
        return metadata.clone();
    };
    if ops::set_key(
        &mut parsed,
        &JsString::from("exporter"),
        JsValue::string("AdvantageScope"),
    )
    .is_err()
    {
        return metadata.clone();
    }
    match json::stringify(&parsed) {
        Ok(Some(text)) => JsString::from(text),
        _ => metadata.clone(),
    }
}

fn join_rows(rows: &[Vec<JsString>]) -> JsString {
    let mut out = JsString::new();
    for (row_index, row) in rows.iter().enumerate() {
        if row_index != 0 {
            out.push_unit(u16::from(b'\n'));
        }
        for (cell_index, cell) in row.iter().enumerate() {
            if cell_index != 0 {
                out.push_unit(u16::from(b','));
            }
            out.push_str(cell);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields_of(keys: &[&str]) -> Result<Vec<String>, ExportError> {
        let mut log = Log::new(true);
        for key in keys {
            log.put_number(&JsString::from(*key), 0.0, 1.0);
        }
        let options = ExportOptions {
            format: Format::CsvTable,
            sampling: Sampling::Changes,
            sampling_period: 20.0,
            prefixes: JsString::new(),
            include_generated: true,
        };
        select_fields(&log, &options)
            .map(|fields| fields.iter().map(JsString::to_string_lossy).collect())
    }

    // Expectations recorded from upstream's getFieldTree and processTree, one fresh realm per case.
    #[test]
    fn field_tree_inherits_object_prototype() {
        let rejected = Err(ExportError::Rejected);
        assert_eq!(fields_of(&["/A/__proto__", "/B/fullKey"]), rejected);
        assert_eq!(fields_of(&["/A/__proto__/x"]), rejected);
        assert_eq!(fields_of(&["/A/toString/x"]), rejected);
        assert_eq!(fields_of(&["/A/__proto__", "/B/fullKey/c"]), rejected);
        assert_eq!(
            fields_of(&["/A/constructor", "/A/b"]),
            Ok(vec!["/A/b".into()])
        );
        assert_eq!(
            fields_of(&["/A/__proto__/", "/B/fullKey"]),
            Ok(vec!["/B/fullKey".into()])
        );
        assert_eq!(
            fields_of(&["/fullKey", "/A/__proto__", "/fullKey/x"]),
            Ok(vec!["/fullKey".into(), "/fullKey/x".into()])
        );
        assert_eq!(fields_of(&["/A/__proto__", "/B/fullKey/"]), Ok(vec![]));
        assert_eq!(fields_of(&["/A/__proto__", "/B/constructor"]), Ok(vec![]));
    }

    #[test]
    fn field_tree_sorts_numerically() {
        assert_eq!(
            fields_of(&["/A/10", "/A/9", "/A/b", "/A/B", "/A/a", "/A/", "/A"]),
            Ok(["/A", "/A/9", "/A/10", "/A/a", "/A/b", "/A/B"]
                .map(String::from)
                .to_vec())
        );
    }

    // Recorded from upstream's parse, `exporter` assignment and stringify under the oracle's
    // Electron, in a strict-mode realm as the bundled worker is.
    #[test]
    fn wpilog_metadata_matches_upstream() {
        let cases = [
            (
                r#"{"b":1,"2":0,"1":0,"exporter":"x","z":2}"#,
                r#"{"1":0,"2":0,"b":1,"exporter":"AdvantageScope","z":2}"#,
            ),
            ("[1,2]", "[1,2]"),
            ("[]", "[]"),
            ("5", "5"),
            ("1e1", "1e1"),
            (r#""s""#, r#""s""#),
            ("true", "true"),
            ("null", "null"),
            (
                r#"{"__proto__":1}"#,
                r#"{"__proto__":1,"exporter":"AdvantageScope"}"#,
            ),
            (r#" {"a" : 1 } "#, r#"{"a":1,"exporter":"AdvantageScope"}"#),
            (r#"{"a":1,"a":2}"#, r#"{"a":2,"exporter":"AdvantageScope"}"#),
            (r#""\ud800""#, r#""\ud800""#),
            ("-0", "-0"),
            (
                r#"{"a":-0,"b":1e21,"c":1.5e-7}"#,
                r#"{"a":0,"b":1e+21,"c":1.5e-7,"exporter":"AdvantageScope"}"#,
            ),
            ("not json", "not json"),
            ("", r#"{"exporter":"AdvantageScope"}"#),
        ];
        for (input, expected) in cases {
            assert_eq!(
                wpilog_metadata(&JsString::from(input)).to_string_lossy(),
                expected,
                "{input}"
            );
        }
    }
}
