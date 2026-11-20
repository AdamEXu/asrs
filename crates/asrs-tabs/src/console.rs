//! Port of the console-line formatting and row filtering in
//! `src/shared/renderers/ConsoleRenderer.ts`.

use asrs_js::regex::sites;
use asrs_js::string::{JsError, JsString};

use crate::ansi::AnsiUp;

fn js(s: &str) -> JsString {
    JsString::from(s)
}

/// `ConsoleRenderer.formatValue`: normalizes spellings like `\x1b`, renders ANSI to HTML,
/// highlights a positive filter, then turns newlines into `<br />`.
pub fn format_value(value: &JsString, filter: &JsString) -> Result<JsString, JsError> {
    let normalized = sites::console_newlines(&sites::console_escapes(value));
    let mut ansi_up = AnsiUp::new();
    let mut html = ansi_up.ansi_to_html(&normalized);
    if !filter.is_empty() && !filter.starts_with(&js("!"), None) {
        let parts = sites::split_tags(&html);
        let filter_lower = filter.to_lower_case();
        let filter_len = filter.len() as i64;
        let mut formatted = Vec::with_capacity(html.len());
        for part in parts {
            if part.starts_with(&js("<"), None) && part.ends_with(&js(">"), None) {
                formatted.extend_from_slice(part.units());
                continue;
            }
            let part_lower = part.to_lower_case();
            let mut buf = Vec::with_capacity(part.len());
            let mut last_position: i64 = -1;
            let mut position: i64 = -1;
            while position + filter_len < part.len() as i64 && {
                position = part_lower.index_of(
                    &filter_lower,
                    Some(if position == -1 {
                        0.0
                    } else {
                        (position + filter_len) as f64
                    }),
                );
                position > -1
            } {
                let from = if last_position == -1 {
                    0.0
                } else {
                    (last_position + filter_len) as f64
                };
                buf.extend_from_slice(part.substring(from, Some(position as f64)).units());
                buf.extend("<span class=\"highlight\">".encode_utf16());
                buf.extend_from_slice(
                    part.substring(position as f64, Some((position + filter_len) as f64))
                        .units(),
                );
                buf.extend("</span>".encode_utf16());
                last_position = position;
            }
            if last_position != -1 {
                buf.extend_from_slice(
                    part.substring((last_position + filter_len) as f64, None)
                        .units(),
                );
                formatted.extend_from_slice(&buf);
            } else {
                formatted.extend_from_slice(part.units());
            }
        }
        html = JsString::from_units(formatted);
    }
    html.replace_all(&js("\n"), &js("<br />"))
}

/// The row predicate in `updateValueAvailability`: `filter` here is the raw FILTER_INPUT
/// value, exactly as upstream lowers it itself before matching.
pub fn line_visible(value: &JsString, filter_input: &JsString) -> bool {
    let filter = filter_input.to_lower_case();
    let negated = filter.starts_with(&js("!"), None);
    if !(if negated {
        filter.len() > 1
    } else {
        !filter.is_empty()
    }) {
        return true;
    }
    if negated {
        !value
            .to_lower_case()
            .includes(&filter.slice(1.0, None).to_lower_case(), None)
    } else {
        value
            .to_lower_case()
            .includes(&filter.to_lower_case(), None)
    }
}
