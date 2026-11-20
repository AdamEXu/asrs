//! `shared/log/LogKeyUtils.ts`: the key constants and helpers, plus
//! `ProtoDecoder.getFriendlySchemaType`, the one ProtoDecoder member it uses.

use asrs_js::regex::sites;
use asrs_js::string::JsString;
use indexmap::IndexSet;

use super::Log;

pub const STRUCT_PREFIX: &str = "struct:";
pub const PROTO_PREFIX: &str = "proto:";
pub const PHOTON_PREFIX: &str = "photonstruct:";
pub const MERGE_PREFIX: &str = "Log";
pub const PHOENIX_PREFIX: &str = "Phoenix6";

const CHILD_LOG_PREFIX: [u16; 4] = [b'l' as u16, b'o' as u16, b'g' as u16, b':' as u16];
const SLASH: u16 = b'/' as u16;

fn ascii(s: &str) -> JsString {
    JsString::from(s)
}

fn starts_with(s: &JsString, prefix: &str) -> bool {
    s.units().starts_with(ascii(prefix).units())
}

/// The element `s.split(separator)[1]`, for a `separator` `s` starts with.
fn second_segment(s: &JsString, separator: &str) -> JsString {
    s.split(Some(&ascii(separator)), None)
        .into_iter()
        .nth(1)
        .unwrap_or_default()
}

/// `isSchema`: whether the key defines a schema (`/(?:^|\/)\.schema\/(.*)/`).
pub fn is_schema(key: &JsString) -> bool {
    sites::schema_type(key).is_some()
}

/// `getSchemaType`: the captured schema type name, or `None` (`null`) when not a schema.
pub fn get_schema_type(key: &JsString) -> Option<JsString> {
    sites::schema_type(key)
}

/// `isChildLog`: whether a field holds an embedded child log.
pub fn is_child_log(wpilib_type: Option<&JsString>, structured_type: Option<&JsString>) -> bool {
    let child = |t: Option<&JsString>| t.is_some_and(|t| t.units().starts_with(&CHILD_LOG_PREFIX));
    child(wpilib_type) || child(structured_type)
}

/// `getChildLogExtension`: the lowercase file extension of a child log type.
pub fn get_child_log_extension(
    wpilib_type: Option<&JsString>,
    structured_type: Option<&JsString>,
) -> Option<JsString> {
    let log_type = match (wpilib_type, structured_type) {
        (Some(t), _) if t.units().starts_with(&CHILD_LOG_PREFIX) => t,
        (_, Some(t)) if t.units().starts_with(&CHILD_LOG_PREFIX) => t,
        _ => return None,
    };
    Some(log_type.slice(4.0, None).to_lower_case())
}

/// `isMergePrefix`: whether the key starts with a `/LogN` merge prefix.
pub fn is_merge_prefix(key: &JsString) -> bool {
    sites::merge_prefix(key).is_some()
}

/// `extractMergePrefix`: the matched `/LogN` prefix, or `""`.
pub fn extract_merge_prefix(key: &JsString) -> JsString {
    sites::merge_prefix(key).unwrap_or_default()
}

/// `ProtoDecoder.getFriendlySchemaType`: the last `.` segment, minus a `Protobuf` prefix.
pub fn get_friendly_schema_type(schema_type: &JsString) -> JsString {
    let mut result = if schema_type.includes(&ascii("."), None) {
        schema_type
            .split(Some(&ascii(".")), None)
            .into_iter()
            .last()
            .unwrap_or_default()
    } else {
        schema_type.clone()
    };
    if result.starts_with(&ascii("Protobuf"), None) {
        result = result.slice(8.0, None);
    }
    result
}

/// `getStructuredTypeFromRaw`: normalizes a raw wpilib type string to a structured type.
pub fn get_structured_type_from_raw(raw_type: &JsString) -> Option<JsString> {
    if starts_with(raw_type, STRUCT_PREFIX) {
        let mut schema_type = second_segment(raw_type, STRUCT_PREFIX);
        if schema_type.ends_with(&ascii("[]"), None) {
            schema_type = schema_type.slice(0.0, Some(-2.0));
        }
        Some(schema_type)
    } else if starts_with(raw_type, PHOTON_PREFIX) {
        Some(second_segment(raw_type, PHOTON_PREFIX))
    } else if starts_with(raw_type, PROTO_PREFIX) {
        Some(get_friendly_schema_type(&second_segment(
            raw_type,
            PROTO_PREFIX,
        )))
    } else if raw_type == &ascii("msgpack") || raw_type == &ascii("MessagePack") {
        Some(ascii("MessagePack"))
    } else if raw_type == &ascii("json") || raw_type == &ascii("JSON") {
        Some(ascii("JSON"))
    } else if raw_type.units().starts_with(&CHILD_LOG_PREFIX) {
        let extension = second_segment(raw_type, "log:");
        let mut out = ascii(".");
        out.push_str(&extension.to_lower_case());
        Some(out)
    } else {
        None
    }
}

/// `removeMergePrefix`: the key without a leading `/LogN` merge prefix.
pub fn remove_merge_prefix(key: &JsString) -> JsString {
    match sites::merge_prefix(key) {
        Some(prefix) if !prefix.is_empty() => {
            JsString::from_units(key.units()[prefix.len()..].to_vec())
        }
        _ => key.clone(),
    }
}

/// `findKey`: the field key with the lowest index in `search`, trying the key both without
/// its merge prefix and without a leading `/` after that.
pub fn find_key(log: &Log, search: &[JsString]) -> Option<JsString> {
    let mut best: Option<&JsString> = None;
    let mut best_index = usize::MAX;
    for key in log.field_keys() {
        let unmerged = remove_merge_prefix(key);
        let index = match search.iter().position(|s| *s == unmerged) {
            Some(i) => Some(i),
            None if unmerged.units().first() == Some(&SLASH) => search
                .iter()
                .position(|s| s.units() == &unmerged.units()[1..]),
            None => None,
        };
        if let Some(index) = index
            && index < best_index
        {
            best = Some(key);
            best_index = index;
        }
    }
    best.cloned()
}

/// `applyKeyPrefix`: joins a merge prefix onto a key.
pub fn apply_key_prefix(prefix: &JsString, key: &JsString) -> JsString {
    if prefix.is_empty() {
        return key.clone();
    }
    if key.is_empty() || key == &ascii("/") {
        return prefix.clone();
    }
    let mut out = prefix.clone();
    if key.units().first() != Some(&SLASH) {
        out.push_unit(SLASH);
    }
    out.push_str(key);
    out
}

/// `splitLogKey`: splits on `/` and `:`, and also on `_` when the unmerged key is a Phoenix 6
/// signal.
pub fn split_log_key(key: &JsString) -> Vec<JsString> {
    let unmerged = remove_merge_prefix(key);
    let phoenix = ascii(PHOENIX_PREFIX);
    let slash_phoenix = ascii(&format!("/{PHOENIX_PREFIX}"));
    let is_phoenix = unmerged.units().starts_with(phoenix.units())
        || unmerged.units().starts_with(slash_phoenix.units());
    sites::split_key(key, is_phoenix)
}

/// `filterFieldByPrefixes`: the fields matching any comma-separated prefix, in discovery
/// order. Prefix segments compare case-insensitively; schema fields can always pass.
pub fn filter_field_by_prefixes(
    fields: &[JsString],
    prefixes: &JsString,
    always_include_schemas: bool,
    nt_only: bool,
) -> Vec<JsString> {
    let mut filtered: IndexSet<JsString> = IndexSet::new();
    for prefix in prefixes.split(Some(&ascii(",")), None) {
        let mut prefix_series: Vec<JsString> = split_log_key(&prefix)
            .into_iter()
            .filter(|item| !item.is_empty())
            .collect();
        if nt_only {
            prefix_series.insert(0, ascii("NT"));
        }
        for field in fields {
            let field_series: Vec<JsString> = split_log_key(field)
                .into_iter()
                .filter(|item| !item.is_empty())
                .collect();
            if field_series.len() < prefix_series.len() {
                continue;
            }
            if prefix_series
                .iter()
                .enumerate()
                .all(|(i, p)| field_series[i].to_lower_case() == p.to_lower_case())
                || (always_include_schemas && is_schema(field))
            {
                filtered.insert(field.clone());
            }
        }
    }
    filtered.into_iter().collect()
}
