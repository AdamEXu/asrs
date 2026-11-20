//! A JS exception as upstream's callers observe it: `String(error)`, which is the thrown string
//! itself or `"<name>: <message>"` for an `Error`.

use std::fmt;

use asrs_js::json::JsonError;
use asrs_js::string::JsError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thrown(pub String);

impl Thrown {
    pub fn error(name: &str, message: impl fmt::Display) -> Self {
        Thrown(format!("{name}: {message}"))
    }

    pub fn type_error(message: impl fmt::Display) -> Self {
        Self::error("TypeError", message)
    }
}

impl fmt::Display for Thrown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
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
        Thrown::error(e.name, e.message.to_string_lossy())
    }
}
