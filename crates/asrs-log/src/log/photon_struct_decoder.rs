//! `PhotonStructDecoder` from `src/shared/log/PhotonStructDecoder.ts`: the PhotonVision
//! "photonstruct" dynamic decoder. Schemas are `;`-separated `type name` declarations;
//! fields are byte-aligned, `optional ` prefixes a one-byte presence flag, `name[?]` is a
//! VLA with a signed int8 length, and fixed `name[n]` arrays fail to compile.
//!
//! Kept upstream quirks: `char` decodes from its offset to the end of the buffer while
//! advancing one byte; schema and field maps are plain objects, so `in` lookups resolve
//! `Object.prototype` names and `__proto__` assignments never create a key.

use asrs_js::string::{JsError, JsString, TextDecoder};
use asrs_js::value::{JsObject, JsValue};

/// Own-property names of `Object.prototype`, which `key in {}` still resolves.
const PROTO_KEYS: &[&str] = &[
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];

fn is_proto_key(units: &[u16]) -> bool {
    PROTO_KEYS
        .iter()
        .any(|k| k.encode_utf16().eq(units.iter().copied()))
}

/// `name in obj` for a plain object: an own key or an `Object.prototype` name.
fn has_key<V>(obj: &JsObject<V>, name: &JsString) -> bool {
    obj.get_units(name.units()).is_some() || is_proto_key(name.units())
}

fn out_of_bounds() -> JsError {
    JsError::range("Offset is outside the bounds of the DataView")
}

fn text_decoder() -> TextDecoder {
    TextDecoder::new(None, false, false).expect("utf-8 is always supported")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Primitive {
    Bool,
    Char,
    Int8,
    Int16,
    Int32,
    Int64,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Float,
    Float32,
    Double,
    Float64,
}

const PRIMITIVES: &[(&str, Primitive)] = &[
    ("bool", Primitive::Bool),
    ("char", Primitive::Char),
    ("int8", Primitive::Int8),
    ("int16", Primitive::Int16),
    ("int32", Primitive::Int32),
    ("int64", Primitive::Int64),
    ("uint8", Primitive::Uint8),
    ("uint16", Primitive::Uint16),
    ("uint32", Primitive::Uint32),
    ("uint64", Primitive::Uint64),
    ("float", Primitive::Float),
    ("float32", Primitive::Float32),
    ("double", Primitive::Double),
    ("float64", Primitive::Float64),
];

impl Primitive {
    fn from_name(units: &[u16]) -> Option<Self> {
        PRIMITIVES
            .iter()
            .find(|(name, _)| name.encode_utf16().eq(units.iter().copied()))
            .map(|(_, p)| *p)
    }

    fn name(self) -> &'static str {
        PRIMITIVES
            .iter()
            .find(|(_, p)| *p == self)
            .map(|(name, _)| *name)
            .expect("every primitive is listed")
    }

    fn bytes(self) -> usize {
        match self {
            Primitive::Bool | Primitive::Char | Primitive::Int8 | Primitive::Uint8 => 1,
            Primitive::Int16 | Primitive::Uint16 => 2,
            Primitive::Int32 | Primitive::Uint32 | Primitive::Float | Primitive::Float32 => 4,
            Primitive::Int64 | Primitive::Uint64 | Primitive::Double | Primitive::Float64 => 8,
        }
    }
}

/// The stored `type` token: a primitive, a nested schema name, or `undefined` when the
/// declaration had no type token and a schema named "undefined" happened to compile.
#[derive(Clone, PartialEq)]
enum FieldType {
    Primitive(Primitive),
    Named(JsString),
    Undefined,
}

impl FieldType {
    /// The value `outputSchemaTypes[name]` receives.
    fn schema_type(&self) -> JsValue {
        match self {
            FieldType::Primitive(p) => JsValue::string(p.name()),
            FieldType::Named(name) => JsValue::String(name.clone()),
            FieldType::Undefined => JsValue::Undefined,
        }
    }

    /// `valueSchema.type + "[]"`.
    fn array_schema_type(&self) -> JsValue {
        match self {
            FieldType::Primitive(p) => JsValue::string(&format!("{}[]", p.name())),
            FieldType::Named(name) => {
                let mut s = name.clone();
                s.push_str(&JsString::from("[]"));
                JsValue::String(s)
            }
            // `undefined + "[]"` stringifies to "undefined[]".
            FieldType::Undefined => JsValue::string("undefined[]"),
        }
    }

    fn decode_name(&self) -> JsString {
        match self {
            FieldType::Named(name) => name.clone(),
            FieldType::Undefined => JsString::from("undefined"),
            FieldType::Primitive(_) => unreachable!("primitives decode inline"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct ValueSchema {
    name: JsString,
    ty: FieldType,
    is_vla: bool,
    is_optional: bool,
}

#[derive(Clone, PartialEq)]
pub struct Schema {
    value_schemas: Vec<ValueSchema>,
}

pub struct Decoded {
    pub data: JsValue,
    /// `JsValue::String` for declared types or `JsValue::Undefined` for the
    /// no-type-token edge.
    pub schema_types: JsObject<JsValue>,
}

pub struct DecodeResult {
    pub data: JsValue,
    pub schema_types: JsObject<JsValue>,
    pub bytes_consumed: usize,
}

#[derive(Default)]
pub struct PhotonStructDecoder {
    schema_strings: JsObject<JsString>,
    schemas: JsObject<Schema>,
}

impl PhotonStructDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_schemas(&self) -> &JsObject<Schema> {
        &self.schemas
    }

    pub fn add_schema(&mut self, name: &JsString, schema: &[u8]) {
        let schema_str = text_decoder()
            .decode(schema)
            .unwrap_or_else(|_| JsString::new());
        if has_key(&self.schema_strings, name) {
            return;
        }
        self.schema_strings.insert(name.clone(), schema_str);
        loop {
            let mut compiled = false;
            let keys: Vec<JsString> = self.schema_strings.keys().cloned().collect();
            for schema_name in keys {
                if !has_key(&self.schemas, &schema_name) {
                    compiled = self.compile_schema(&schema_name) || compiled;
                }
            }
            if !compiled {
                break;
            }
        }
    }

    fn compile_schema(&mut self, name: &JsString) -> bool {
        let schema = self
            .schema_strings
            .get_units(name.units())
            .expect("compile_schema is called for schema_strings keys")
            .clone();
        let optional = JsString::from("optional ");
        let space = JsString::from(" ");
        let bracket_open = JsString::from("[");
        let bracket_close = JsString::from("]");
        let question = JsString::from("?");
        let mut value_schemas = Vec::new();
        for decl in schema.split(Some(&JsString::from(";")), None) {
            if decl.is_empty() {
                continue;
            }
            let mut decl = decl;
            let mut is_optional = false;
            if decl.starts_with(&optional, None) {
                is_optional = true;
                decl = decl.substring(optional.len() as f64, None);
            }
            let parts: Vec<JsString> = decl
                .split(Some(&space), None)
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            let ty = match parts.first() {
                Some(t) if Primitive::from_name(t.units()).is_some() => {
                    FieldType::Primitive(Primitive::from_name(t.units()).expect("checked"))
                }
                Some(t) if has_key(&self.schemas, t) => FieldType::Named(t.clone()),
                Some(_) => return false,
                // `undefined in this.schemas`: the literal key, not a proto name.
                None if self.schemas.get("undefined").is_some() => FieldType::Undefined,
                None => return false,
            };
            let name_str = parts.iter().skip(1).fold(JsString::new(), |mut acc, s| {
                acc.push_str(s);
                acc
            });
            let (field_name, is_vla) = if name_str.includes(&bracket_open, None) {
                let split = name_str.split(Some(&bracket_open), None);
                let length_str = split
                    .get(1)
                    .map(|s| s.split(Some(&bracket_close), None).remove(0))
                    .unwrap_or_default();
                if length_str == question {
                    (split.into_iter().next().unwrap_or_default(), true)
                } else {
                    // console.warn("Fixed length arrays are unimplemented")
                    return false;
                }
            } else {
                (name_str, false)
            };
            if is_optional && is_vla {
                // console.warn("Can't be optional AND array?")
                return false;
            }
            value_schemas.push(ValueSchema {
                name: field_name,
                ty,
                is_vla,
                is_optional,
            });
        }
        self.schemas.insert(name.clone(), Schema { value_schemas });
        true
    }

    pub fn decode(&self, name: &JsString, value: &[u8]) -> Result<Decoded, JsError> {
        let result = self.decode_impl(name, value)?;
        Ok(Decoded {
            data: result.data,
            schema_types: result.schema_types,
        })
    }

    pub fn decode_impl(&self, name: &JsString, value: &[u8]) -> Result<DecodeResult, JsError> {
        self.decode_at(name, value, 0)
    }

    /// `base` is where `value` starts in the backing buffer, which upstream's nested
    /// `DataView` constructor errors report.
    fn decode_at(
        &self,
        name: &JsString,
        value: &[u8],
        base: usize,
    ) -> Result<DecodeResult, JsError> {
        if !has_key(&self.schemas, name) {
            return Err(JsError::new("Error", "Schema not defined"));
        }
        let Some(schema) = self.schemas.get_units(name.units()) else {
            // `key in {}` resolved a prototype name; `schema.valueSchemas` is then
            // `undefined` and reading `.length` throws.
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'length')",
            ));
        };
        let mut data = JsObject::new();
        let mut schema_types: JsObject<JsValue> = JsObject::new();
        let mut offset = 0usize;
        for vs in &schema.value_schemas {
            let mut is_present = true;
            let mut vla_length: Option<i8> = None;
            if vs.is_optional {
                is_present = decode_value(value, base, offset, Primitive::Bool)?
                    .as_bool()
                    .expect("bool decodes to Bool");
                offset += 1;
                assign(&mut data, vs.name.clone(), JsValue::Null);
            }
            if vs.is_vla {
                let length = decode_value(value, base, offset, Primitive::Int8)?
                    .as_f64()
                    .expect("int8 decodes to Number");
                vla_length = Some(length as i8);
                offset += 1;
            }
            if !is_present {
                continue;
            }
            match &vs.ty {
                FieldType::Primitive(p) => {
                    if let Some(length) = vla_length {
                        assign(
                            &mut schema_types,
                            vs.name.clone(),
                            vs.ty.array_schema_type(),
                        );
                        let mut inner = Vec::new();
                        for _ in 0..length {
                            inner.push(decode_value(value, base, offset, *p)?);
                            offset += p.bytes();
                        }
                        assign(&mut data, vs.name.clone(), JsValue::Array(inner));
                        let mut length_key = vs.name.clone();
                        length_key.push_str(&JsString::from("/length"));
                        assign(&mut data, length_key, JsValue::Number(f64::from(length)));
                    } else {
                        assign(&mut schema_types, vs.name.clone(), vs.ty.schema_type());
                        let item = decode_value(value, base, offset, *p)?;
                        offset += p.bytes();
                        assign(&mut data, vs.name.clone(), item);
                    }
                }
                named => {
                    let child_name = named.decode_name();
                    if let Some(length) = vla_length {
                        assign(
                            &mut schema_types,
                            vs.name.clone(),
                            vs.ty.array_schema_type(),
                        );
                        let mut inner = Vec::new();
                        for i in 0..length {
                            let child = self.decode_at(
                                &child_name,
                                tail(value, base, offset)?,
                                base + offset,
                            )?;
                            offset += child.bytes_consumed;
                            inner.push(child.data);
                            for (field, ty) in child.schema_types.iter() {
                                let mut key = vs.name.clone();
                                key.push_str(&JsString::from("/"));
                                key.push_str(&JsString::from(asrs_js::number::to_string(
                                    f64::from(i),
                                )));
                                key.push_str(&JsString::from("/"));
                                key.push_str(field);
                                assign(&mut schema_types, key, ty.clone());
                            }
                        }
                        assign(&mut data, vs.name.clone(), JsValue::Array(inner));
                        let mut length_key = vs.name.clone();
                        length_key.push_str(&JsString::from("/length"));
                        assign(&mut data, length_key, JsValue::Number(f64::from(length)));
                    } else {
                        assign(&mut schema_types, vs.name.clone(), vs.ty.schema_type());
                        let child =
                            self.decode_at(&child_name, tail(value, base, offset)?, base + offset)?;
                        offset += child.bytes_consumed;
                        assign(&mut data, vs.name.clone(), child.data);
                        for (field, ty) in child.schema_types.iter() {
                            let mut key = vs.name.clone();
                            key.push_str(&JsString::from("/"));
                            key.push_str(field);
                            assign(&mut schema_types, key, ty.clone());
                        }
                    }
                }
            }
        }
        Ok(DecodeResult {
            data: JsValue::Object(data),
            schema_types,
            bytes_consumed: offset,
        })
    }

    pub fn to_serialized(&self) -> JsValue {
        let mut root = JsObject::new();
        let mut schema_strings = JsObject::new();
        for (k, v) in self.schema_strings.iter() {
            schema_strings.insert(k.clone(), JsValue::String(v.clone()));
        }
        let mut schemas = JsObject::new();
        for (k, v) in self.schemas.iter() {
            let mut schema = JsObject::new();
            schema.insert(
                "valueSchemas",
                JsValue::Array(
                    v.value_schemas
                        .iter()
                        .map(|vs| {
                            let mut entry = JsObject::new();
                            entry.insert("name", JsValue::String(vs.name.clone()));
                            entry.insert("type", vs.ty.schema_type());
                            entry.insert("isVLA", JsValue::Bool(vs.is_vla));
                            entry.insert("isOptional", JsValue::Bool(vs.is_optional));
                            JsValue::Object(entry)
                        })
                        .collect(),
                ),
            );
            schemas.insert(k.clone(), JsValue::Object(schema));
        }
        root.insert("schemaStrings", JsValue::Object(schema_strings));
        root.insert("schemas", JsValue::Object(schemas));
        JsValue::Object(root)
    }

    /// `fromSerialized` on well-formed `toSerialized` output.
    pub fn from_serialized(data: &JsValue) -> Self {
        let mut decoder = Self::new();
        if let Some(obj) = data.get("schemaStrings").and_then(JsValue::as_object) {
            for (k, v) in obj.iter() {
                if let JsValue::String(s) = v {
                    decoder.schema_strings.insert(k.clone(), s.clone());
                }
            }
        }
        if let Some(obj) = data.get("schemas").and_then(JsValue::as_object) {
            for (k, v) in obj.iter() {
                let Some(items) = v.get("valueSchemas").and_then(JsValue::as_array) else {
                    continue;
                };
                let mut value_schemas = Vec::new();
                for item in items {
                    let ty = match item.get("type") {
                        Some(JsValue::String(t)) => match Primitive::from_name(t.units()) {
                            Some(p) => FieldType::Primitive(p),
                            None => FieldType::Named(t.clone()),
                        },
                        Some(JsValue::Undefined) | None => FieldType::Undefined,
                        _ => continue,
                    };
                    value_schemas.push(ValueSchema {
                        name: item
                            .get("name")
                            .and_then(JsValue::as_str)
                            .cloned()
                            .unwrap_or_default(),
                        ty,
                        is_vla: item
                            .get("isVLA")
                            .and_then(JsValue::as_bool)
                            .unwrap_or(false),
                        is_optional: item
                            .get("isOptional")
                            .and_then(JsValue::as_bool)
                            .unwrap_or(false),
                    });
                }
                decoder.schemas.insert(k.clone(), Schema { value_schemas });
            }
        }
        decoder
    }
}

/// `outputData[key] = v`: a literal `__proto__` assignment never creates an own key.
fn assign(data: &mut JsObject<JsValue>, key: JsString, value: JsValue) {
    if key.as_string().as_deref() == Some("__proto__") {
        return;
    }
    data.insert(key, value);
}

/// The remaining view a nested `DataView(buffer, offset, length - offset)` sees.
fn tail(view: &[u8], base: usize, offset: usize) -> Result<&[u8], JsError> {
    view.get(offset..).ok_or_else(|| {
        JsError::range(format!(
            "Start offset {} is outside the bounds of the buffer",
            base + offset
        ))
    })
}

/// One `dataView.get*` read at `offset`; `char` takes the rest of the view.
fn decode_value(
    view: &[u8],
    base: usize,
    offset: usize,
    ty: Primitive,
) -> Result<JsValue, JsError> {
    let at = |width: usize| -> Result<&[u8], JsError> {
        view.get(offset..offset + width).ok_or_else(out_of_bounds)
    };
    Ok(match ty {
        Primitive::Bool => JsValue::Bool(at(1)?[0] > 0),
        Primitive::Char => JsValue::String(
            text_decoder()
                .decode(tail(view, base, offset)?)
                .unwrap_or_else(|_| JsString::new()),
        ),
        Primitive::Int8 => JsValue::Number(f64::from(at(1)?[0] as i8)),
        Primitive::Int16 => {
            JsValue::Number(f64::from(i16::from_le_bytes(at(2)?.try_into().expect("2"))))
        }
        Primitive::Int32 => {
            JsValue::Number(f64::from(i32::from_le_bytes(at(4)?.try_into().expect("4"))))
        }
        Primitive::Int64 => {
            JsValue::Number(i64::from_le_bytes(at(8)?.try_into().expect("8")) as f64)
        }
        Primitive::Uint8 => JsValue::Number(f64::from(at(1)?[0])),
        Primitive::Uint16 => {
            JsValue::Number(f64::from(u16::from_le_bytes(at(2)?.try_into().expect("2"))))
        }
        Primitive::Uint32 => {
            JsValue::Number(f64::from(u32::from_le_bytes(at(4)?.try_into().expect("4"))))
        }
        Primitive::Uint64 => {
            JsValue::Number(u64::from_le_bytes(at(8)?.try_into().expect("8")) as f64)
        }
        Primitive::Float | Primitive::Float32 => {
            JsValue::Number(f64::from(f32::from_le_bytes(at(4)?.try_into().expect("4"))))
        }
        Primitive::Double | Primitive::Float64 => {
            JsValue::Number(f64::from_le_bytes(at(8)?.try_into().expect("8")))
        }
    })
}
