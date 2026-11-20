use asrs_js::string::JsString;

const CHILD_LOG_PREFIX: [u16; 4] = [b'l' as u16, b'o' as u16, b'g' as u16, b':' as u16];

/// `isChildLog`: whether a field holds an embedded child log.
pub fn is_child_log(wpilib_type: Option<&JsString>, structured_type: Option<&JsString>) -> bool {
    let child = |t: Option<&JsString>| t.is_some_and(|t| t.units().starts_with(&CHILD_LOG_PREFIX));
    child(wpilib_type) || child(structured_type)
}
