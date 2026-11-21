//! The `"save-type-memory"` merge from `main.ts`: a shallow per-memoryId merge into the
//! type-memory file's contents, written only when the serialized JSON changed. The file value
//! is a raw JS object, so `in` checks and `__proto__` assignments follow the live prototype
//! chain (`{"__proto__": {...}}` data really does change the object's prototype).

use asrs_js::json::{self};
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, array_index};
use asrs_model::json_file;

use crate::fs::Fs;

/// Own enumerable properties of `Object.prototype` (`key in {}` hits these plus `__proto__`).
const OBJECT_PROTO: &[&str] = &[
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
    "constructor",
];

const ARRAY_PROTO: &[&str] = &[
    "at",
    "concat",
    "copyWithin",
    "entries",
    "every",
    "fill",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flat",
    "flatMap",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "keys",
    "lastIndexOf",
    "map",
    "pop",
    "push",
    "reduce",
    "reduceRight",
    "reverse",
    "shift",
    "slice",
    "some",
    "sort",
    "splice",
    "toLocaleString",
    "toReversed",
    "toSorted",
    "toSpliced",
    "toString",
    "unshift",
    "values",
    "with",
];

/// What `obj.__proto__` reads/writes produce on an object-literal `typeMemory`.
#[derive(Clone)]
enum Proto {
    /// `Object.prototype`.
    Default,
    /// `__proto__ = null` removed the prototype.
    Null,
    /// `__proto__ = value` set a parsed object or array as the prototype.
    Value(Box<JsValue>),
}

/// `typeMemory`: a plain object or the file's raw root (an array root keeps array semantics;
/// a primitive root makes `in` throw).
pub struct TypeMemory {
    root: JsValue,
    proto: Proto,
}

impl TypeMemory {
    fn new() -> TypeMemory {
        TypeMemory {
            root: JsValue::Object(JsObject::new()),
            proto: Proto::Default,
        }
    }

    fn parsed(value: JsValue) -> TypeMemory {
        TypeMemory {
            root: value,
            proto: Proto::Default,
        }
    }

    pub fn to_js(&self) -> JsValue {
        self.root.clone()
    }

    /// `key in typeMemory`.
    fn has(&self, key: &JsString) -> Result<bool, JsError> {
        let index = array_index(key.units()).map(|i| i as usize);
        match &self.root {
            JsValue::Object(o) => Ok(o.get_key(key).is_some() || self.proto_has(key)),
            JsValue::Array(items) => Ok(match index {
                Some(i) => i < items.len(),
                None => {
                    *key == JsString::from("length")
                        || ARRAY_PROTO.iter().any(|m| m == &key.to_string_lossy())
                        || OBJECT_PROTO.iter().any(|m| m == &key.to_string_lossy())
                        || *key == JsString::from("__proto__")
                }
            }),
            JsValue::Bytes(bytes) => Ok(match index {
                Some(i) => i < bytes.len(),
                None => {
                    *key == JsString::from("length")
                        || OBJECT_PROTO.iter().any(|m| m == &key.to_string_lossy())
                        || *key == JsString::from("__proto__")
                }
            }),
            JsValue::ExtData(..) => {
                Ok(*key == JsString::from("type") || *key == JsString::from("data"))
            }
            JsValue::Date(_) => Ok(false),
            other => Err(JsError::type_error(format!(
                "Cannot use 'in' operator to search for '{}' in {}",
                key.to_string_lossy(),
                display(other)
            ))),
        }
    }

    fn proto_has(&self, key: &JsString) -> bool {
        let name = key.to_string_lossy();
        match &self.proto {
            Proto::Default => name == "__proto__" || OBJECT_PROTO.iter().any(|m| m == &name),
            Proto::Null => false,
            Proto::Value(value) => match value.as_ref() {
                JsValue::Object(o) => {
                    o.get_key(key).is_some()
                        || name == "__proto__"
                        || OBJECT_PROTO.iter().any(|m| m == &name)
                }
                JsValue::Array(items) => match array_index(key.units()) {
                    Some(i) => (i as usize) < items.len(),
                    None => {
                        name == "length"
                            || name == "__proto__"
                            || ARRAY_PROTO.iter().any(|m| m == &name)
                            || OBJECT_PROTO.iter().any(|m| m == &name)
                    }
                },
                _ => false,
            },
        }
    }

    /// `{...typeMemory[key]}` where the property was found by [`has`](Self::has). Reads
    /// through the prototype yield non-enumerable members, which spread to nothing.
    fn spread_at(&self, key: &JsString) -> JsObject<JsValue> {
        let JsValue::Object(o) = &self.root else {
            return JsObject::new();
        };
        if let Some(v) = o.get_key(key) {
            return ops::spread(v);
        }
        match &self.proto {
            Proto::Default | Proto::Null => JsObject::new(),
            Proto::Value(value) => match value.as_ref() {
                JsValue::Object(proto) => proto.get_key(key).map(ops::spread).unwrap_or_default(),
                _ => JsObject::new(),
            },
        }
    }

    /// `typeMemory[key] = value`, including `__proto__`'s inherited accessor when no own
    /// `__proto__` property exists.
    fn set(&mut self, key: &JsString, value: JsValue) -> Result<(), JsError> {
        match &mut self.root {
            JsValue::Object(o) => {
                if *key == JsString::from("__proto__") && !o.contains_key("__proto__") {
                    self.proto = match value {
                        JsValue::Object(_) | JsValue::Array(_) | JsValue::Bytes(_) => {
                            Proto::Value(Box::new(value))
                        }
                        JsValue::Null => Proto::Null,
                        _ => self.proto.clone(),
                    };
                    return Ok(());
                }
                o.insert(key.clone(), value);
                Ok(())
            }
            other => ops::set_key(other, key, value),
        }
    }
}

fn display(value: &JsValue) -> String {
    match value {
        JsValue::String(s) => s.to_string_lossy(),
        other => ops::to_string(other).to_string_lossy(),
    }
}

fn stringify(value: &JsValue) -> Result<Option<String>, JsError> {
    json::stringify(value).map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))
}

/// `ipcMain.on("save-type-memory", ...)`.
pub fn save(fs: &dyn Fs, type_memory_file: &str, data: &JsValue) -> Result<(), JsError> {
    let mut type_memory = if fs.exists(type_memory_file) {
        TypeMemory::parsed(json_file::parse(&fs.read(type_memory_file)?)?)
    } else {
        TypeMemory::new()
    };
    let before = stringify(&type_memory.to_js())?;
    for memory_id in ops::object_keys(data)? {
        let fields = ops::get_key(data, &memory_id)?;
        let merged = if type_memory.has(&memory_id)? {
            let mut merged = type_memory.spread_at(&memory_id);
            for (k, v) in ops::spread(&fields) {
                merged.insert(k.clone(), v.clone());
            }
            JsValue::Object(merged)
        } else {
            fields
        };
        type_memory.set(&memory_id, merged)?;
    }
    if stringify(&type_memory.to_js())? != before {
        fs.write(
            type_memory_file,
            &json_file::stringify(&type_memory.to_js())?,
        )?;
    }
    Ok(())
}
