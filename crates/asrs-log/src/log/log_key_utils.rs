use asrs_js::regex::sites;
use asrs_js::string::JsString;

const CHILD_LOG_PREFIX: [u16; 4] = [b'l' as u16, b'o' as u16, b'g' as u16, b':' as u16];

/// `isChildLog`: whether a field holds an embedded child log.
pub fn is_child_log(wpilib_type: Option<&JsString>, structured_type: Option<&JsString>) -> bool {
    let child = |t: Option<&JsString>| t.is_some_and(|t| t.units().starts_with(&CHILD_LOG_PREFIX));
    child(wpilib_type) || child(structured_type)
}

pub const PHOENIX_PREFIX: &str = "Phoenix6";

/// `removeMergePrefix`: the key without a leading `/LogN` merge prefix.
pub fn remove_merge_prefix(key: &JsString) -> JsString {
    match sites::merge_prefix(key) {
        Some(prefix) if !prefix.is_empty() => {
            JsString::from_units(key.units()[prefix.len()..].to_vec())
        }
        _ => key.clone(),
    }
}

/// `findKey`: the field key whose unmerged form sits earliest in `search`, also trying the
/// key without a leading `/`. Ties keep the earliest field key (strict `<`).
pub fn find_key(log: &super::Log, search: &[&str]) -> Option<JsString> {
    let search: Vec<JsString> = search.iter().map(|s| JsString::from(*s)).collect();
    let mut best_key = None;
    let mut best_index = usize::MAX;
    for key in log.get_field_keys() {
        let unmerged = remove_merge_prefix(&key);
        let index = search.iter().position(|s| *s == unmerged).or_else(|| {
            if unmerged.units().first() == Some(&(b'/' as u16)) {
                let without = JsString::from_units(unmerged.units()[1..].to_vec());
                search.iter().position(|s| *s == without)
            } else {
                None
            }
        });
        if let Some(index) = index
            && index < best_index
        {
            best_index = index;
            best_key = Some(key);
        }
    }
    best_key
}

/// `splitLogKey`: splits on `/` and `:`, and also on `_` when the unmerged key is a Phoenix 6
/// signal.
pub fn split_log_key(key: &JsString) -> Vec<JsString> {
    let unmerged = remove_merge_prefix(key);
    let phoenix = JsString::from(PHOENIX_PREFIX);
    let slash_phoenix = JsString::from(format!("/{PHOENIX_PREFIX}"));
    let is_phoenix = unmerged.units().starts_with(phoenix.units())
        || unmerged.units().starts_with(slash_phoenix.units());
    sites::split_key(key, is_phoenix)
}
