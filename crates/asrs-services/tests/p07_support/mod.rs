#![allow(dead_code)]

use std::collections::BTreeMap;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_services::dialog::MessageBox;

#[path = "../../../asrs-cli/src/js_json.rs"]
mod js_json;

pub const UPSTREAM_NAME: &str = "AdvantageScope";
pub const RELEASE_NAME: &str = "asrs";

/// A recorded JSON object whose every field must be read before [`Fields::finish`].
pub struct Fields {
    pub ctx: String,
    pairs: Vec<(Utf16String, Json)>,
    used: Vec<bool>,
}

impl Fields {
    pub fn new(json: &Json, ctx: &str) -> Fields {
        match json {
            Json::Object(pairs) => Fields {
                ctx: ctx.to_owned(),
                used: vec![false; pairs.len()],
                pairs: pairs.clone(),
            },
            other => panic!("{ctx}: expected an object, got {}", other.to_compact()),
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.pairs.iter().any(|(k, _)| k.eq_str(key))
    }

    pub fn take(&mut self, key: &str) -> Json {
        let i = self
            .pairs
            .iter()
            .position(|(k, _)| k.eq_str(key))
            .unwrap_or_else(|| panic!("{}: no field {key:?}", self.ctx));
        assert!(!self.used[i], "{}: field {key:?} read twice", self.ctx);
        self.used[i] = true;
        self.pairs[i].1.clone()
    }

    pub fn string(&mut self, key: &str) -> String {
        let ctx = self.ctx.clone();
        match self.take(key) {
            Json::String(s) => s
                .as_string()
                .unwrap_or_else(|| panic!("{ctx}: {key:?} is not valid UTF-16")),
            other => panic!("{ctx}: {key:?} is {}", other.to_compact()),
        }
    }

    pub fn tree(&mut self, key: &str) -> Value {
        let ctx = self.ctx.clone();
        Value::from_json(&self.take(key)).unwrap_or_else(|e| panic!("{ctx}: {key:?}: {e:?}"))
    }

    pub fn number(&mut self, key: &str) -> f64 {
        let ctx = self.ctx.clone();
        tree_number(&self.tree(key)).unwrap_or_else(|| panic!("{ctx}: {key:?} is not a number"))
    }

    /// A plain JSON number (not CVE-1), as `case` and `step` are written.
    pub fn raw_number(&mut self, key: &str) -> f64 {
        let ctx = self.ctx.clone();
        self.take(key)
            .as_f64()
            .unwrap_or_else(|| panic!("{ctx}: {key:?} is not a number"))
    }

    pub fn bool(&mut self, key: &str) -> bool {
        match self.take(key) {
            Json::Bool(b) => b,
            other => panic!("{}: {key:?} is {}", self.ctx, other.to_compact()),
        }
    }

    pub fn array(&mut self, key: &str) -> Vec<Json> {
        match self.take(key) {
            Json::Array(items) => items,
            other => panic!("{}: {key:?} is {}", self.ctx, other.to_compact()),
        }
    }

    pub fn finish(self) {
        for (i, (k, _)) in self.pairs.iter().enumerate() {
            assert!(
                self.used[i],
                "{}: recorded field {:?} was never compared",
                self.ctx,
                k.to_string_lossy()
            );
        }
    }
}

pub fn tree_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(bits) => Some(f64::from_bits(*bits)),
        _ => None,
    }
}

pub fn tree_bytes(value: &Value, ctx: &str) -> Vec<u8> {
    match value {
        Value::Bytes(b) => b.clone(),
        other => panic!("{ctx}: expected bytes, got {}", other.encode()),
    }
}

pub fn to_js(value: &Value) -> JsValue {
    js_json::to_js(value).expect("recorded value converts")
}

pub fn from_js(value: &JsValue) -> Value {
    js_json::from_js(value)
}

pub fn js_string_value(s: &JsString) -> Value {
    Value::String(Utf16String::from_units(s.units().to_vec()))
}

pub fn str_value(s: &str) -> Value {
    Value::String(Utf16String::from(s))
}

/// Same number, with every NaN equal (V8 and Rust NaN payloads differ).
pub fn same_number(expected: f64, actual: f64) -> bool {
    (expected.is_nan() && actual.is_nan()) || expected.to_bits() == actual.to_bits()
}

pub fn assert_tree(ctx: &str, expected: &Value, actual: &Value) {
    assert_eq!(expected.encode(), actual.encode(), "{ctx}");
}

/// Counts every recorded item read and every item compared against the port; the two tallies
/// must agree per kind at the end of a vector.
#[derive(Default)]
pub struct Tally {
    recorded: BTreeMap<String, usize>,
    compared: BTreeMap<String, usize>,
}

impl Tally {
    pub fn record(&mut self, kind: &str) {
        *self.recorded.entry(kind.to_owned()).or_default() += 1;
    }

    pub fn compare(&mut self, kind: &str) {
        *self.compared.entry(kind.to_owned()).or_default() += 1;
    }

    pub fn finish(self, id: &str) {
        assert!(!self.recorded.is_empty(), "{id}: no records");
        assert_eq!(
            self.recorded, self.compared,
            "{id}: compared counts differ from recorded counts"
        );
    }
}

/// Compares a recorded `{parent, options}` box against the port's, every option key included.
/// `hub` is the window id a modal box must be parented to.
pub fn check_box(ctx: &str, parent: &Json, options: &Value, actual: &MessageBox, hub: Option<f64>) {
    match parent {
        Json::Null => assert!(!actual.modal, "{ctx}: port parents a box upstream shows unparented"),
        other => {
            let id = other.as_f64().unwrap_or_else(|| {
                tree_number(&Value::from_json(other).expect("parent id"))
                    .unwrap_or_else(|| panic!("{ctx}: parent {}", other.to_compact()))
            });
            assert!(actual.modal, "{ctx}: port shows unparented a box upstream parents");
            assert_eq!(Some(id), hub, "{ctx}: box parent is not the hub");
        }
    }
    let Value::Object(pairs) = options else {
        panic!("{ctx}: options are {}", options.encode())
    };
    let mut seen = Vec::new();
    for (key, value) in pairs {
        let key = key.to_string_lossy();
        let expected_key = |k: &str| key == k;
        let actual_value = if expected_key("type") {
            str_value(actual.kind)
        } else if expected_key("title") {
            str_value(actual.title)
        } else if expected_key("message") {
            js_string_value(&actual.message)
        } else if expected_key("detail") {
            js_string_value(&actual.detail)
        } else if expected_key("icon") {
            assert!(actual.icon, "{ctx}: port leaves out the icon");
            str_value("WINDOW_ICON")
        } else if expected_key("buttons") {
            let buttons = actual
                .buttons
                .as_ref()
                .unwrap_or_else(|| panic!("{ctx}: port leaves out buttons"));
            Value::Array(buttons.iter().map(|b| str_value(b)).collect())
        } else if expected_key("defaultId") {
            Value::number(
                actual
                    .default_id
                    .unwrap_or_else(|| panic!("{ctx}: port leaves out defaultId")),
            )
        } else if expected_key("cancelId") {
            Value::number(
                actual
                    .cancel_id
                    .unwrap_or_else(|| panic!("{ctx}: port leaves out cancelId")),
            )
        } else {
            panic!("{ctx}: unexpected box option {key:?}")
        };
        assert_tree(&format!("{ctx}: box {key}"), value, &actual_value);
        seen.push(key);
    }
    let has = |k: &str| seen.iter().any(|s| s == k);
    assert_eq!(actual.icon, has("icon"), "{ctx}: icon presence");
    assert_eq!(actual.buttons.is_some(), has("buttons"), "{ctx}: buttons presence");
    assert_eq!(actual.default_id.is_some(), has("defaultId"), "{ctx}: defaultId presence");
    assert_eq!(actual.cancel_id.is_some(), has("cancelId"), "{ctx}: cancelId presence");
}

/// The release identity's box must be upstream's with the app name substituted in the
/// presentation strings and nothing else (PLAN note 38).
pub fn check_release_box(ctx: &str, upstream: &MessageBox, release: &MessageBox) {
    let swap = |s: &JsString| {
        s.replace_all(&JsString::from(UPSTREAM_NAME), &JsString::from(RELEASE_NAME))
            .expect("string pattern")
    };
    let expected = MessageBox {
        message: swap(&upstream.message),
        detail: swap(&upstream.detail),
        ..upstream.clone()
    };
    assert_eq!(&expected, release, "{ctx}: release identity box");
}

/// A CVE-1 object (`{"o": [[key, value], ...]}`) as a JSON object whose values are the CVE-1
/// trees, so [`Fields`] can read it key by key.
pub fn cve_object(json: &Json, ctx: &str) -> Json {
    match Value::from_json(json).unwrap_or_else(|e| panic!("{ctx}: {e:?}")) {
        Value::Object(pairs) => Json::Object(
            pairs
                .into_iter()
                .map(|(k, v)| (k, asrs_parity::json::parse(&v.encode()).expect("CVE-1 is JSON")))
                .collect(),
        ),
        other => panic!("{ctx}: expected an object, got {}", other.encode()),
    }
}
