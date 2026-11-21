//! Port of `configReducer.ts`/`configTypes.ts`: the config tree the FTC Dashboard server
//! broadcasts, kept as `JsValue`s because every accessor follows JS property semantics.

use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use crate::thrown::Thrown;

fn str_eq(value: &JsValue, text: &str) -> bool {
    matches!(value, JsValue::String(s) if s == &JsString::from(text))
}

fn custom(value: JsValue) -> JsValue {
    JsValue::Object(JsObject::from_iter([
        (JsString::from("__type"), JsValue::string("custom")),
        (JsString::from("__value"), value),
    ]))
}

fn inflate(var: &JsValue) -> Result<JsValue, Thrown> {
    if str_eq(&ops::get(var, "__type")?, "custom") {
        let value = ops::get(var, "__value")?;
        if value.is_null() {
            Ok(custom(JsValue::Null))
        } else {
            let mut children = JsObject::new();
            for key in ops::object_keys(&value)? {
                children.insert(key.clone(), inflate(&ops::get_key(&value, &key)?)?);
            }
            Ok(custom(JsValue::Object(children)))
        }
    } else {
        let new_value = ops::get(var, "__value")?;
        let mut object = ops::spread(var);
        object.insert(JsString::from("__newValue"), new_value);
        object.insert(JsString::from("__valid"), JsValue::Bool(true));
        Ok(JsValue::Object(object))
    }
}

fn merge_modified(base: &JsValue, latest: &JsValue) -> Result<JsValue, Thrown> {
    let base_type = ops::get(base, "__type")?;
    let latest_type = ops::get(latest, "__type")?;
    if str_eq(&base_type, "custom") && str_eq(&latest_type, "custom") {
        let latest_value = ops::get(latest, "__value")?;
        if latest_value.is_null() {
            return Ok(custom(JsValue::Null));
        }
        let base_value = ops::get(base, "__value")?;
        let mut out = JsObject::new();
        for key in ops::object_keys(&latest_value)? {
            // `in` also sees inherited `Object.prototype` members upstream; merging one of those
            // reaches `inflate(latest)` on every path, as a missing key does.
            let child = if !base_value.is_null() && ops::has_key(&base_value, &key)? {
                merge_modified(
                    &ops::get_key(&base_value, &key)?,
                    &ops::get_key(&latest_value, &key)?,
                )?
            } else {
                inflate(&ops::get_key(&latest_value, &key)?)?
            };
            out.insert(key, child);
        }
        Ok(custom(JsValue::Object(out)))
    } else {
        // The enum and basic preserve cases build the same object upstream; they differ only in
        // when they apply.
        let modified =
            !ops::strict_equals(&ops::get(base, "__value")?, &ops::get(base, "__newValue")?);
        let preserve = modified
            && ((str_eq(&base_type, "enum")
                && str_eq(&latest_type, "enum")
                && ops::strict_equals(
                    &ops::get(base, "__enumClass")?,
                    &ops::get(latest, "__enumClass")?,
                ))
                || (ops::strict_equals(&base_type, &latest_type)
                    && !str_eq(&base_type, "custom")
                    && !str_eq(&latest_type, "custom")));
        if preserve {
            let mut var = ops::spread(base);
            var.insert(JsString::from("__value"), ops::get(latest, "__value")?);
            Ok(JsValue::Object(var))
        } else {
            inflate(latest)
        }
    }
}

fn revert_modified(state: &JsValue) -> Result<JsValue, Thrown> {
    if str_eq(&ops::get(state, "__type")?, "custom") {
        let value = ops::get(state, "__value")?;
        if value.is_null() {
            Ok(custom(JsValue::Null))
        } else {
            let mut out = JsObject::new();
            for key in ops::object_keys(&value)? {
                out.insert(key.clone(), inflate(&ops::get_key(&value, &key)?)?);
            }
            Ok(custom(JsValue::Object(out)))
        }
    } else {
        let mut var = ops::spread(state);
        var.insert(JsString::from("__newValue"), ops::get(state, "__value")?);
        Ok(JsValue::Object(var))
    }
}

pub fn initial_state() -> JsValue {
    JsValue::Object(JsObject::from_iter([(
        JsString::from("configRoot"),
        custom(JsValue::Object(JsObject::new())),
    )]))
}

/// `configReducer(state, action)`, returning `(new state, changed)`. The upstream `!==` check
/// on `configState` reduces to whether a real case ran: every real case returns a new object.
pub fn config_reducer(state: &JsValue, action: &JsValue) -> Result<(JsValue, bool), Thrown> {
    let with_root = |root: JsValue| {
        let mut object = ops::spread(state);
        object.insert(JsString::from("configRoot"), root);
        JsValue::Object(object)
    };
    let action_type = ops::get(action, "type")?;
    if str_eq(&action_type, "RECEIVE_CONFIG") {
        Ok((
            with_root(merge_modified(
                &ops::get(state, "configRoot")?,
                &ops::get(action, "configRoot")?,
            )?),
            true,
        ))
    } else if str_eq(&action_type, "UPDATE_CONFIG") {
        Ok((with_root(ops::get(action, "configRoot")?), true))
    } else if str_eq(&action_type, "REFRESH_CONFIG") {
        Ok((
            with_root(revert_modified(&ops::get(state, "configRoot")?)?),
            true,
        ))
    } else {
        Ok((state.clone(), false))
    }
}
