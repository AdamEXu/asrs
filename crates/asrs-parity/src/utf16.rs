use std::fmt;

use crate::json::write_string;

/// A JS string: a sequence of UTF-16 code units, so lone surrogates survive.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Utf16String(Vec<u16>);

impl Utf16String {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn from_units(units: Vec<u16>) -> Self {
        Self(units)
    }

    pub fn units(&self) -> &[u16] {
        &self.0
    }

    pub fn into_units(self) -> Vec<u16> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push_str(&mut self, s: &str) {
        self.0.extend(s.encode_utf16());
    }

    pub fn push_unit(&mut self, unit: u16) {
        self.0.push(unit);
    }

    /// `None` when the string holds a lone surrogate.
    pub fn as_string(&self) -> Option<String> {
        String::from_utf16(&self.0).ok()
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }

    pub fn eq_str(&self, s: &str) -> bool {
        self.0.iter().copied().eq(s.encode_utf16())
    }

    /// The string as `JSON.stringify` writes it, quotes included.
    pub fn quoted(&self) -> String {
        let mut out = String::with_capacity(self.0.len() + 2);
        write_string(&self.0, &mut out);
        out
    }
}

impl From<&str> for Utf16String {
    fn from(s: &str) -> Self {
        Self(s.encode_utf16().collect())
    }
}

impl From<String> for Utf16String {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<&String> for Utf16String {
    fn from(s: &String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Vec<u16>> for Utf16String {
    fn from(units: Vec<u16>) -> Self {
        Self(units)
    }
}

impl fmt::Debug for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.quoted())
    }
}

impl fmt::Display for Utf16String {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_lossy())
    }
}
