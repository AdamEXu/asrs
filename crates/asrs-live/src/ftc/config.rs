//! Port of `configReducer.ts`/`configTypes.ts`: the config tree the FTC Dashboard server
//! broadcasts, kept as `JsValue`s because every accessor follows JS property semantics.

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use crate::js;
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
    if str_eq(&js::get(var, "__type")?, "custom") {
        let value = js::get(var, "__value")?;
        if value.is_null() {
            Ok(custom(JsValue::Null))
        } else {
            let mut children = JsObject::new();
            for key in js::object_keys(&value)? {
                children.insert(key.clone(), inflate(&js::get_key(&value, &key)?)?);
            }
            Ok(custom(JsValue::Object(children)))
        }
    } else {
        let new_value = js::get(var, "__value")?;
        let mut object = js::spread(var);
        object.insert(JsString::from("__newValue"), new_value);
        object.insert(JsString::from("__valid"), JsValue::Bool(true));
        Ok(JsValue::Object(object))
    }
}

fn merge_modified(base: &JsValue, latest: &JsValue) -> Result<JsValue, Thrown> {
    let base_type = js::get(base, "__type")?;
    let latest_type = js::get(latest, "__type")?;
    let custom_root = |value: JsValue| custom(value);
    if str_eq(&base_type, "custom") && str_eq(&latest_type, "custom") {
        let latest_value = js::get(latest, "__value")?;
        if latest_value.is_null() {
            return Ok(custom_root(JsValue::Null));
        }
        let base_value = js::get(base, "__value")?;
        let mut out = JsObject::new();
        for key in js::object_keys(&latest_value)? {
            let child = if !base_value.is_null() && js::has_key_in(&base_value, &key)? {
                merge_modified(
                    &js::get_key(&base_value, &key)?,
                    &js::get_key(&latest_value, &key)?,
                )?
            } else {
                inflate(&js::get_key(&latest_value, &key)?)?
            };
            out.insert(key, child);
        }
        Ok(custom(JsValue::Object(out)))
    } else {
        // The enum and basic preserve cases build the same object upstream; they differ only in
        // when they apply.
        let modified =
            !js::strict_equals(&js::get(base, "__value")?, &js::get(base, "__newValue")?);
        let preserve = modified
            && ((str_eq(&base_type, "enum")
                && str_eq(&latest_type, "enum")
                && js::strict_equals(
                    &js::get(base, "__enumClass")?,
                    &js::get(latest, "__enumClass")?,
                ))
                || (js::strict_equals(&base_type, &latest_type)
                    && !str_eq(&base_type, "custom")
                    && !str_eq(&latest_type, "custom")));
        if preserve {
            let mut var = js::spread(base);
            var.insert(JsString::from("__value"), js::get(latest, "__value")?);
            Ok(JsValue::Object(var))
        } else {
            inflate(latest)
        }
    }
}

fn revert_modified(state: &JsValue) -> Result<JsValue, Thrown> {
    if str_eq(&js::get(state, "__type")?, "custom") {
        let value = js::get(state, "__value")?;
        if value.is_null() {
            Ok(custom(JsValue::Null))
        } else {
            let mut out = JsObject::new();
            for key in js::object_keys(&value)? {
                out.insert(key.clone(), revert_modified(&js::get_key(&value, &key)?)?);
            }
            Ok(custom(JsValue::Object(out)))
        }
    } else {
        let mut var = js::spread(state);
        var.insert(JsString::from("__newValue"), js::get(state, "__value")?);
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
        let mut object = js::spread(state);
        object.insert(JsString::from("configRoot"), root);
        JsValue::Object(object)
    };
    let action_type = js::get(action, "type")?;
    if str_eq(&action_type, "RECEIVE_CONFIG") {
        Ok((
            with_root(merge_modified(
                &js::get(state, "configRoot")?,
                &js::get(action, "configRoot")?,
            )?),
            true,
        ))
    } else if str_eq(&action_type, "UPDATE_CONFIG") {
        Ok((with_root(js::get(action, "configRoot")?), true))
    } else if str_eq(&action_type, "REFRESH_CONFIG") {
        Ok((
            with_root(revert_modified(&js::get(state, "configRoot")?)?),
            true,
        ))
    } else {
        Ok((state.clone(), false))
    }
}
