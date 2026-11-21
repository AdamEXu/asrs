//! A JS exception as upstream's callers observe it: `String(error)`, which is the thrown string
//! itself or `"<name>: <message>"` for an `Error`. It is a JS string, so lone surrogates from
//! topic names survive.

use std::fmt;

use asrs_js::json::JsonError;
use asrs_js::string::{JsError, JsString};
use asrs_log::log::LogError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thrown(pub JsString);

impl Thrown {
    pub fn new(text: impl Into<JsString>) -> Self {
        Thrown(text.into())
    }

    pub fn error(name: &str, message: impl fmt::Display) -> Self {
        Thrown::new(format!("{name}: {message}"))
    }

    pub fn type_error(message: impl fmt::Display) -> Self {
        Self::error("TypeError", message)
    }
}

impl fmt::Display for Thrown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_string_lossy())
    }
}

impl std::error::Error for Thrown {}

impl From<JsError> for Thrown {
    fn from(e: JsError) -> Self {
        Thrown::error(e.name, e.message)
    }
}

impl From<JsonError> for Thrown {
    fn from(e: JsonError) -> Self {
        let mut text = JsString::from(format!("{}: ", e.name));
        text.push_str(&e.message);
        Thrown(text)
    }
}

impl From<LogError> for Thrown {
    fn from(e: LogError) -> Self {
        match e {
            LogError::Proto(e) => Thrown::error(e.js_name(), e.js_message().to_string_lossy()),
            LogError::StackOverflow => {
                Thrown::error("RangeError", "Maximum call stack size exceeded")
            }
            LogError::NeverCompletes => Thrown::new("upstream never completes"),
            LogError::Unmodeled(what) => Thrown::new(what),
        }
    }
}
