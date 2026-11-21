//! `src/hub/exportWorker.ts` (F10): field selection, timestamp sampling and the CSV
//! writers, plus the oracle session's pre-export skip checks. The `wpilog` and `mcap`
//! branches dispatch to [`ExportError::Unported`] until F11's `WPILOGEncoder` and F12's
//! `McapWriter` land.

use asrs_js::collation::locale_compare_numeric;
use asrs_js::number;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;
use asrs_js::{json, value::JsObject, value::JsValue};
use asrs_log::log::log_key_utils::{filter_field_by_prefixes, split_log_key};
use asrs_log::log::log_util::get_log_value_text_at;
use asrs_log::log::log_value_sets::LogValueSetAny;
use asrs_log::log::robot_state::AKIT_TIMESTAMP_KEYS;
use asrs_log::log::{Log, LogFieldTree, LoggableType};
use asrs_log::util::{clean_float, upper_bound};

pub const MCAP_CHUNK_SIZE: f64 = 1024.0 * 1024.0;
const MCAP_SCHEMA_OVERHEAD: f64 = 2048.0;
const MCAP_TYPES: &[LoggableType] = &[
    LoggableType::Boolean,
    LoggableType::Number,
    LoggableType::String,
    LoggableType::BooleanArray,
    LoggableType::NumberArray,
];

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
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Output::Text(text) => asrs_js::string::text_encode(text),
            Output::Bytes(bytes) => bytes.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// The worker posted `reject()`: an exception or a missing AdvantageKit timestamp.
    Rejected,
    /// A branch assigned to a later unit (F11's `wpilog`, F12's `mcap`).
    Unported(&'static str),
}

/// `onmessage`: select fields, compute timestamps, dispatch on the format.
pub fn export(log: &Log, options: &ExportOptions) -> Result<Output, ExportError> {
    let fields = select_fields(log, options)?;
    let timestamps = sample_timestamps(log, options)?;
    match options.format {
        Format::CsvTable => Ok(Output::Text(csv_table(log, &fields, &timestamps))),
        Format::CsvList => Ok(Output::Text(csv_list(log, &fields, &timestamps))),
        Format::Wpilog => Err(ExportError::Unported("wpilog export is unit F11")),
        Format::Mcap => Err(ExportError::Unported("mcap export is unit F12")),
    }
}

/// The oracle session's `export` pre-checks for the two option sets upstream never
/// finishes: fixed sampling over a range that needs more than `fixed_limit` samples or a
/// step that vanishes in float precision, and MCAP with fixed or AdvantageKit sampling
/// once the unawaited writes outgrow one 1 MiB chunk.
pub fn skip_reason(log: &Log, options: &ExportOptions, fixed_limit: f64) -> Option<&'static str> {
    if options.sampling == Sampling::Fixed {
        let [start, end] = log.get_timestamp_range();
        let step = options.sampling_period / 1000.0;
        let samples = (end - start) / step;
        if !(samples <= fixed_limit) || !(start + step > start) || !(end + step > end) {
            return Some("fixed sampling range too large");
        }
    }
    if options.format == Format::Mcap && options.sampling != Sampling::Changes {
        if let Some(timestamps) = sample_timestamps_unchecked(log, options)
            && mcap_chunk_bound(log, &timestamps) > MCAP_CHUNK_SIZE
        {
            return Some("unawaited MCAP writes past one chunk");
        }
    }
    None
}

/// The worker's field list: the tree flattened parent-first with numeric `localeCompare`
/// at each level, `Empty` fields removed, then `filterFieldByPrefixes`.
fn select_fields(log: &Log, options: &ExportOptions) -> Result<Vec<JsString>, ExportError> {
    let mut fields = Vec::new();
    let root = field_tree(log, options.include_generated)?;
    flatten_tree(&root, &mut fields);
    fields.retain(|field| log.get_type(field) != Some(LoggableType::Empty));
    Ok(filter_field_by_prefixes(
        &fields,
        &options.prefixes,
        options.format == Format::Wpilog,
        false,
    ))
}

/// `Object.prototype`'s own property names, which `table in children` always finds. The
/// `children` maps upstream builds are plain object literals, so a table with one of
/// these names descends into the inherited value instead of a node.
const INHERITED: &[&str] = &[
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

/// `Log.getFieldTree` as upstream writes it, plain-object prototype semantics included:
/// `table in position.children` is true for every inherited name, so a `__proto__`,
/// `constructor` or `toString` segment makes `position` the inherited value. Such a field
/// never reaches the tree; a deeper segment then evaluates `in` on `undefined` and throws,
/// which `reject()`s the worker. Ending a key at `__proto__` also writes
/// `Object.prototype.fullKey`, after which a `fullKey` segment lands on that string and a
/// strict-mode property assignment on it throws too.
fn field_tree(log: &Log, include_generated: bool) -> Result<JsObject<LogFieldTree>, ExportError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Pos {
        Node,
        Prototype,
        Function,
        Str,
    }
    let slash = u16::from(b'/');
    let mut root: JsObject<LogFieldTree> = JsObject::new();
    let mut polluted = false;
    for key in log.field_keys() {
        if !include_generated && log.is_generated(key) {
            continue;
        }
        let skip = usize::from(key.units().first() == Some(&slash));
        let tables: Vec<JsString> =
            split_log_key(&JsString::from_units(key.units()[skip..].to_vec()))
                .into_iter()
                .filter(|table| !table.is_empty())
                .collect();
        let mut pos = Pos::Node;
        let mut depth = 0usize;
        for table in &tables {
            if pos != Pos::Node {
                return Err(ExportError::Rejected);
            }
            let children = children_at(&mut root, &tables[..depth]);
            if children.get_key(table).is_some() {
                depth += 1;
            } else if polluted && table == &JsString::from("fullKey") {
                pos = Pos::Str;
            } else if INHERITED.iter().any(|name| table == &JsString::from(*name)) {
                pos = if table == &JsString::from("__proto__") {
                    Pos::Prototype
                } else {
                    Pos::Function
                };
            } else {
                children.insert(table.clone(), LogFieldTree::default());
                depth += 1;
            }
        }
        let assigns_full_key = key.units().last() != Some(&slash);
        match pos {
            Pos::Node if !tables.is_empty() => {
                let node = node_at(&mut root, &tables[..depth]);
                if node.full_key.is_none() || assigns_full_key {
                    node.full_key = Some(key.clone());
                }
            }
            Pos::Node => {}
            Pos::Prototype if assigns_full_key => polluted = true,
            Pos::Prototype | Pos::Function => {}
            Pos::Str if assigns_full_key => return Err(ExportError::Rejected),
            Pos::Str => {}
        }
    }
    Ok(root)
}

fn children_at<'a>(
    root: &'a mut JsObject<LogFieldTree>,
    path: &[JsString],
) -> &'a mut JsObject<LogFieldTree> {
    let mut children = root;
    for table in path {
        children = &mut children
            .get_units_mut(table.units())
            .expect("a node on the descent path")
            .children;
    }
    children
}

fn node_at<'a>(root: &'a mut JsObject<LogFieldTree>, path: &[JsString]) -> &'a mut LogFieldTree {
    let mut children = root;
    for (index, table) in path.iter().enumerate() {
        let node = children
            .get_units_mut(table.units())
            .expect("a node on the descent path");
        if index + 1 == path.len() {
            return node;
        }
        children = &mut node.children;
    }
    unreachable!("nonempty path")
}

fn flatten_tree(tree: &JsObject<LogFieldTree>, fields: &mut Vec<JsString>) {
    let mut keys: Vec<JsString> = tree.keys().cloned().collect();
    array_sort(&mut keys, |a, b| locale_compare_numeric(a, b));
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
fn sample_timestamps(log: &Log, options: &ExportOptions) -> Result<Option<Vec<f64>>, ExportError> {
    match sample_timestamps_unchecked(log, options) {
        Some(timestamps) => Ok(Some(timestamps)),
        None if options.sampling == Sampling::Changes => Ok(None),
        None => Err(ExportError::Rejected),
    }
}

/// The session's `sampleTimestamps`: `None` for `changes` and for an AdvantageKit list
/// that does not exist, where the worker would reject.
fn sample_timestamps_unchecked(log: &Log, options: &ExportOptions) -> Option<Vec<f64>> {
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
            Some(timestamps)
        }
        Sampling::Akit => {
            let key = log
                .get_field_keys()
                .into_iter()
                .find(|key| is_akit_timestamp_key(key))?;
            log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)
                .map(|set| set.timestamps)
        }
        Sampling::Changes => None,
    }
}

fn is_akit_timestamp_key(key: &JsString) -> bool {
    AKIT_TIMESTAMP_KEYS
        .iter()
        .any(|&kit| key == &JsString::from(kit))
}

/// `generateCsvTable`: the timestamp union for `changes`, the last sample at or before
/// each timestamp otherwise, `getLogValueText` with commas folded to semicolons, and no
/// quoting or trailing newline.
fn csv_table(log: &Log, fields: &[JsString], timestamps: &Option<Vec<f64>>) -> JsString {
    let timestamps = timestamps
        .clone()
        .unwrap_or_else(|| log.get_timestamps(fields, None));
    let mut data: Vec<Vec<JsString>> = Vec::with_capacity(timestamps.len() + 1);
    data.push(vec![JsString::from("Timestamp")]);
    for &timestamp in &timestamps {
        data.push(vec![JsString::from(number::to_string(timestamp))]);
    }
    for field in fields {
        data[0].push(field.clone());
        let field_data = log.get_range(field, f64::NEG_INFINITY, f64::INFINITY, None);
        let mut next_index = 0usize;
        for (index, &timestamp) in timestamps.iter().enumerate() {
            let Some(set) = &field_data else { continue };
            next_index = upper_bound(set.timestamps(), timestamp, next_index);
            let text = if next_index != 0 {
                get_log_value_text_at(set, next_index - 1).unwrap_or_else(|| "null".to_owned())
            } else {
                "null".to_owned()
            };
            data[index + 1].push(JsString::from(text.replace(',', ";")));
        }
    }
    join_rows(&data)
}

/// `generateCsvList`: one row per sample per field, a stable sort by timestamp, the
/// header spliced in front, and the same no-quoting text rules.
fn csv_list(log: &Log, fields: &[JsString], timestamps: &Option<Vec<f64>>) -> JsString {
    let mut rows: Vec<(f64, JsString, JsString)> = Vec::new();
    for field in fields {
        let Some(field_data) = log.get_range(field, f64::NEG_INFINITY, f64::INFINITY, None) else {
            continue;
        };
        match timestamps {
            None => {
                for index in 0..field_data.len() {
                    let text = get_log_value_text_at(&field_data, index)
                        .unwrap_or_else(|| "null".to_owned());
                    rows.push((
                        field_data.timestamps()[index],
                        field.clone(),
                        JsString::from(text.replace(',', ";")),
                    ));
                }
            }
            Some(timestamps) => {
                let mut next_index = 0usize;
                for &timestamp in timestamps {
                    next_index = upper_bound(field_data.timestamps(), timestamp, next_index);
                    let text = if next_index != 0 {
                        get_log_value_text_at(&field_data, next_index - 1)
                            .unwrap_or_else(|| "null".to_owned())
                    } else {
                        "null".to_owned()
                    };
                    rows.push((
                        timestamp,
                        field.clone(),
                        JsString::from(text.replace(',', ";")),
                    ));
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
    out
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

/// `mcapChunkBound`: a byte bound on the first chunk under sampled MCAP export, over
/// every log field of an MCAP schema type, regardless of the export's field selection.
fn mcap_chunk_bound(log: &Log, timestamps: &[f64]) -> f64 {
    let mut bound = MCAP_SCHEMA_OVERHEAD;
    for key in log.get_field_keys() {
        let Some(field_type) = log.get_type(&key) else {
            continue;
        };
        if !MCAP_TYPES.contains(&field_type) {
            continue;
        }
        bound += 64.0 + asrs_js::string::text_encode(&key).len() as f64;
        let Some(set) = log.get_range(&key, f64::NEG_INFINITY, f64::INFINITY, None) else {
            continue;
        };
        let mut next = 0usize;
        for &timestamp in timestamps {
            next = upper_bound(set.timestamps(), timestamp, next);
            if next != 0 {
                bound += 31.0 + mcap_value_json_len(&set, next - 1);
            }
        }
    }
    bound
}

/// `encoder.encode(JSON.stringify({ value })).length` for the sample at `index`.
fn mcap_value_json_len(set: &LogValueSetAny, index: usize) -> f64 {
    let value = sample_value(set, index);
    let object: JsObject<JsValue> = [(JsString::from("value"), value)].into_iter().collect();
    json::stringify(&JsValue::Object(object))
        .map(|json| json.map_or(0.0, |json| json.len() as f64))
        .unwrap_or(0.0)
}

/// The JS value upstream sees at `fieldData.values[index]` for an MCAP-typed field.
fn sample_value(set: &LogValueSetAny, index: usize) -> JsValue {
    match set {
        LogValueSetAny::Boolean(set) => JsValue::Bool(set.values[index]),
        LogValueSetAny::Number(set) => JsValue::Number(set.values[index]),
        LogValueSetAny::String(set) => JsValue::String(set.values[index].clone()),
        LogValueSetAny::BooleanArray(set) => JsValue::Array(
            set.values[index]
                .iter()
                .map(|&v| JsValue::Bool(v))
                .collect(),
        ),
        LogValueSetAny::NumberArray(set) => JsValue::Array(
            set.values[index]
                .iter()
                .map(|&v| JsValue::Number(v))
                .collect(),
        ),
        _ => JsValue::Undefined,
    }
}
