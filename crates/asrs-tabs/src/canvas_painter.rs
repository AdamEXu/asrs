//! What the Swerve and Joysticks painters share: reads of command values with upstream's
//! property-access exceptions, the `lastRenderState` gate and canvas colours.

use asrs_canvas::color;
use asrs_canvas::path::sat32;
use asrs_canvas::{Canvas2d, Style};
use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_js::{json, number, ops};

use crate::css_value;

pub(crate) fn unmodeled(what: impl Into<String>) -> JsError {
    JsError::new("Unmodeled", what)
}

/// `value[key]`.
pub(crate) fn member<'a>(value: &'a JsValue, key: &str) -> Result<&'a JsValue, JsError> {
    static UNDEFINED: JsValue = JsValue::Undefined;
    match value {
        JsValue::Null | JsValue::Undefined => {
            Err(ops::get(value, key).expect_err("reading a property of null or undefined throws"))
        }
        JsValue::Object(object) => Ok(object.get(key).unwrap_or(&UNDEFINED)),
        _ => Err(unmodeled(format!("{key} of {value:?}"))),
    }
}

/// The array behind `value`, whose `property` upstream reads first.
pub(crate) fn array<'a>(value: &'a JsValue, property: &str) -> Result<&'a [JsValue], JsError> {
    match value {
        JsValue::Array(items) => Ok(items),
        JsValue::Null | JsValue::Undefined => {
            Err(ops::get(value, property)
                .expect_err("reading a property of null or undefined throws"))
        }
        _ => Err(unmodeled(format!("{property} of {value:?}"))),
    }
}

/// `value` as an operand of upstream's arithmetic. A string would concatenate under `+`.
pub(crate) fn operand(value: &JsValue) -> Result<f64, JsError> {
    match value {
        JsValue::Undefined | JsValue::Null | JsValue::Bool(_) | JsValue::Number(_) => {
            ops::value_to_number(value)
        }
        _ => Err(unmodeled(format!("arithmetic on {value:?}"))),
    }
}

/// `JSON.stringify(state)`, and whether it differs from `last`, which it replaces.
pub(crate) fn state_changed(last: &mut String, state: &JsValue) -> Result<bool, JsError> {
    let text = json::stringify(state)
        .map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))?
        .unwrap_or_default();
    if text == *last {
        return Ok(false);
    }
    *last = text;
    Ok(true)
}

/// A colour assigned to `fillStyle` or `strokeStyle`. Blink ignores what it cannot parse; a
/// colour Blink accepts and the rasterizer does not know is an error.
pub(crate) fn canvas_color(value: &JsValue) -> Result<Style, JsError> {
    let css = match value {
        JsValue::Undefined => return Ok(Style::Undefined),
        JsValue::String(s) => s.to_string_lossy(),
        _ => return Err(unmodeled(format!("canvas colour {value:?}"))),
    };
    if !is_modeled_color(&css)? {
        return Err(unmodeled(format!("canvas colour {css:?}")));
    }
    Ok(Style::Color(css))
}

/// What Blink's style parser accepts for a colour property and its canvas ignores.
const STYLE_ONLY_KEYWORDS: [&str; 7] = [
    "inherit",
    "initial",
    "unset",
    "revert",
    "revert-layer",
    "-webkit-link",
    "-webkit-activelink",
];

fn is_modeled_color(css: &str) -> Result<bool, JsError> {
    if css.is_empty() || color::parse_color(css).is_some() {
        return Ok(true);
    }
    Ok(
        match css_value::serialize("background-color", css, false)? {
            None => true,
            Some(accepted) => STYLE_ONLY_KEYWORDS.contains(&accepted.as_str()),
        },
    )
}

/// `context.arc`, which throws on a negative radius unless an argument leaves the float range.
pub(crate) fn arc(
    context: &mut Canvas2d,
    [x, y]: [f64; 2],
    radius: f64,
    start: f64,
    end: f64,
    anticlockwise: Option<bool>,
) -> Result<(), JsError> {
    let finite = [x, y, radius, start, end]
        .iter()
        .all(|v| sat32(*v).is_finite());
    if finite && sat32(radius) < 0.0 {
        return Err(JsError::new(
            "IndexSizeError",
            format!(
                "Failed to execute 'arc' on 'CanvasRenderingContext2D': \
                 The radius provided ({}) is negative.",
                blink_number(f64::from(sat32(radius)))
            ),
        ));
    }
    match anticlockwise {
        Some(direction) => context.arc_with_direction(x, y, radius, start, end, direction),
        None => context.arc(x, y, radius, start, end),
    }
    Ok(())
}

/// Blink's `String::Number(double)`: six significant digits, with trailing zeros dropped only
/// when there is no exponent.
fn blink_number(x: f64) -> String {
    let text = number::to_precision(x, Some(6.0)).expect("6 is a valid precision");
    if text.contains('e') || !text.contains('.') {
        return text;
    }
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use asrs_parity::vectors::load;
    use asrs_parity::{Utf16String, Value};

    use super::*;

    fn text(value: &Value) -> String {
        value
            .as_str()
            .and_then(Utf16String::as_string)
            .expect("a string")
    }

    /// `fillStyle` reads back the previous `#010203` where Blink ignored the assignment.
    #[test]
    fn canvas_colors_follow_blink_or_fail_loudly() {
        let records = load("dom/css-style/color-syntax").unwrap();
        let value = records
            .iter()
            .find(|r| r.field("eval").is_some())
            .expect("an eval record")
            .value("value")
            .unwrap();
        let (mut accepted, mut ignored, mut unmodeled) = (0, 0, 0);
        for row in value.as_array().expect("rows") {
            let Some([kind, css, _, _, _, fill_style, _]) = row.as_array() else {
                panic!("row is not [kind, value, stroke, fill, background, fillStyle, pixel]");
            };
            let (kind, css) = (text(kind), text(css));
            let blink_accepts = text(fill_style) != "#010203";
            match canvas_color(&JsValue::from(css.as_str())) {
                Ok(style) => {
                    assert_eq!(style, Style::Color(css.clone()));
                    let known = color::parse_color(&css).is_some();
                    assert_eq!(known, blink_accepts, "{css:?}");
                    *if known { &mut accepted } else { &mut ignored } += 1;
                }
                Err(e) => {
                    assert_eq!(e.name, "Unmodeled");
                    assert!(kind == "open" || kind == "keyword", "{css:?}");
                    assert!(kind == "open" || blink_accepts, "{css:?}");
                    unmodeled += 1;
                }
            }
        }
        assert!(accepted > 4000 && ignored > 100 && unmodeled > 20);
    }

    #[test]
    fn values_that_are_not_strings() {
        assert_eq!(canvas_color(&JsValue::Undefined).unwrap(), Style::Undefined);
        for value in [JsValue::Null, JsValue::Number(0.0)] {
            assert_eq!(canvas_color(&value).unwrap_err().name, "Unmodeled");
        }
    }
}
