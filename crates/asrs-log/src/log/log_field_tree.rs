use asrs_js::string::JsString;
use asrs_js::value::JsObject;

/// `shared/log/LogFieldTree.ts`: one layer of the recursive field tree.
/// `children` iterates in JS property order (integer-like keys ascending
/// first, then insertion order), which `JsObject` reproduces.
#[derive(Clone, Default)]
pub struct LogFieldTree {
    pub full_key: Option<JsString>,
    pub children: JsObject<LogFieldTree>,
}
