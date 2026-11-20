//! `shared/log/StructDecoder.ts`: the WPILib struct schema compiler and bit-level
//! decoder. `schemaStrings` and `schemas` are plain JS objects upstream, so the
//! maps here keep every stored value verbatim (a `fromSerialized` restore
//! round-trips `null`, strings and other non-schema values byte-exactly) while
//! reads go through JS semantics: `in` sees `Object.prototype` members,
//! `obj["__proto__"] = v` is swallowed, and property reads coerce via
//! `ToNumber`/`ToPropertyKey`.

use asrs_js::math;
use asrs_js::number::{self, to_integer_or_infinity};
use asrs_js::string::{JsError, JsString, TextDecoder};
use asrs_js::value::{JsObject, JsValue};

/// `VALID_TYPE_STRINGS` (`Object.values(ValueType)`).
const VALID_TYPE_STRINGS: &[&str] = &[
    "bool", "char", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64",
    "float", "float32", "double", "float64",
];

/// `BITFIELD_VALID_TYPES`.
const BITFIELD_VALID_TYPES: &[&str] = &[
    "bool", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64",
];

/// `VALUE_TYPE_MAX_BITS`.
fn type_max_bits(ty: &JsString) -> f64 {
    if js_eq(ty, "int16") || js_eq(ty, "uint16") {
        16.0
    } else if js_eq(ty, "int32")
        || js_eq(ty, "uint32")
        || js_eq(ty, "float")
        || js_eq(ty, "float32")
    {
        32.0
    } else if js_eq(ty, "int64")
        || js_eq(ty, "uint64")
        || js_eq(ty, "double")
        || js_eq(ty, "float64")
    {
        64.0
    } else {
        8.0
    }
}

fn js_eq(s: &JsString, lit: &str) -> bool {
    s.units().iter().copied().eq(lit.encode_utf16())
}

fn is_valid_type(ty: &JsString) -> bool {
    VALID_TYPE_STRINGS.iter().any(|t| js_eq(ty, t))
}

fn is_bitfield_type(ty: &JsString) -> bool {
    BITFIELD_VALID_TYPES.iter().any(|t| js_eq(ty, t))
}

fn is_bool(ty: &JsString) -> bool {
    js_eq(ty, "bool")
}

fn is_char(ty: &JsString) -> bool {
    js_eq(ty, "char")
}

fn is_proto(name: &JsString) -> bool {
    js_eq(name, "__proto__")
}

fn text_decoder() -> &'static TextDecoder {
    static DECODER: std::sync::OnceLock<TextDecoder> = std::sync::OnceLock::new();
    DECODER.get_or_init(|| TextDecoder::new(None, false, false).expect("utf-8 is supported"))
}

fn js_string(s: &str) -> JsString {
    JsString::from_units(s.encode_utf16().collect())
}

/// `ToNumber` for the values that can appear in a serialized decoder.
fn to_number(v: &JsValue) -> f64 {
    match v {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(b) => f64::from(*b),
        JsValue::Number(x) => *x,
        JsValue::String(s) => number::to_number(s.units()),
        JsValue::Array(items) => number::to_number(&join_units(items)),
        JsValue::Bytes(bytes) => {
            if bytes.is_empty() {
                0.0
            } else if bytes.len() == 1 {
                f64::from(bytes[0])
            } else {
                f64::NAN
            }
        }
        _ => f64::NAN,
    }
}

/// `ToString`/`ToPropertyKey` for stored field values.
fn to_property_key(v: &JsValue) -> JsString {
    match v {
        JsValue::String(s) => s.clone(),
        JsValue::Number(x) => JsString::from(number::to_string(*x)),
        JsValue::Bool(b) => js_string(if *b { "true" } else { "false" }),
        JsValue::Null => js_string("null"),
        JsValue::Array(items) => JsString::from_units(join_units(items)),
        JsValue::Bytes(bytes) => JsString::from(
            bytes
                .iter()
                .map(|b| number::to_string(f64::from(*b)))
                .collect::<Vec<_>>()
                .join(","),
        ),
        JsValue::Object(_) | JsValue::ExtData(..) => js_string("[object Object]"),
        _ => js_string("undefined"),
    }
}

/// `Array.prototype.join`/`ToString` for arrays: nullish elements are empty.
fn join_units(items: &[JsValue]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(u16::from(b','));
        }
        if matches!(item, JsValue::Null | JsValue::Undefined) {
            continue;
        }
        out.extend_from_slice(to_property_key(item).units());
    }
    out
}

/// The member `schemas[name]` resolves to for a name the map does not own, when
/// the name is an `Object.prototype` member: `__proto__` is the prototype object
/// itself, everything else is a method (with its `Function.length` arity).
#[derive(Clone, Copy)]
enum Inherited {
    Prototype,
    Function(f64),
}

fn object_proto_member(name: &JsString) -> Option<Inherited> {
    const MEMBERS: &[(&str, f64)] = &[
        ("constructor", 1.0),
        ("__defineGetter__", 2.0),
        ("__defineSetter__", 2.0),
        ("hasOwnProperty", 1.0),
        ("__lookupGetter__", 1.0),
        ("__lookupSetter__", 1.0),
        ("isPrototypeOf", 1.0),
        ("propertyIsEnumerable", 1.0),
        ("toString", 0.0),
        ("valueOf", 0.0),
        ("toLocaleString", 0.0),
    ];
    if is_proto(name) {
        return Some(Inherited::Prototype);
    }
    MEMBERS
        .iter()
        .find(|(m, _)| js_eq(name, m))
        .map(|(_, arity)| Inherited::Function(*arity))
}

/// `key in obj` for a plain `{}`: own properties and the inherited members.
fn js_in(obj: &JsObject, key: &JsString) -> bool {
    obj.get_key(key).is_some() || object_proto_member(key).is_some()
}

/// `Cannot use 'in' operator to search for 'key' in rhs` for primitive rhs.
fn in_type_error(key: &JsString, rhs: &JsValue) -> JsError {
    JsError::type_error(format!(
        "Cannot use 'in' operator to search for '{}' in {}",
        key.to_string_lossy(),
        to_property_key(rhs).to_string_lossy()
    ))
}

fn read_error(kind: &str, prop: &str) -> JsError {
    JsError::type_error(format!(
        "Cannot read properties of {kind} (reading '{prop}')"
    ))
}

/// The verbatim `schemaStrings`/`schemas` maps: a plain object when the stored
/// value is one, anything else as-is.
#[derive(Clone, Debug)]
enum MapData {
    Own(JsObject),
    Other(JsValue),
}

impl MapData {
    fn to_js(&self) -> JsValue {
        match self {
            MapData::Own(map) => JsValue::Object(map.clone()),
            MapData::Other(v) => v.clone(),
        }
    }

    /// `name in map`, erroring when the map is a primitive.
    fn contains(&self, name: &JsString) -> Result<bool, JsError> {
        match self {
            MapData::Own(map) => Ok(js_in(map, name)),
            MapData::Other(v) => Err(in_type_error(name, v)),
        }
    }
}

/// `obj[key] = value` where `key` is `"__proto__"`: the prototype setter runs
/// instead, and no own property ever appears.
fn set_property<V>(object: &mut JsObject<V>, key: &JsString, value: V) {
    if is_proto(key) {
        return;
    }
    object.insert(key.clone(), value);
}

/// `ToNumber(v.length)` for a stored schema entry.
fn length_of(v: &JsValue) -> f64 {
    match v {
        JsValue::Object(map) => map.get("length").map_or(f64::NAN, to_number),
        JsValue::String(s) => s.len() as f64,
        JsValue::Array(items) => items.len() as f64,
        JsValue::Bytes(bytes) => bytes.len() as f64,
        _ => f64::NAN,
    }
}

/// `v[i]` for a `valueSchemas` stand-in.
fn index_get(v: &JsValue, i: f64) -> JsValue {
    match v {
        JsValue::Array(items) => items.get(i as usize).cloned().unwrap_or_default(),
        JsValue::Object(map) => map.get(&number::to_string(i)).cloned().unwrap_or_default(),
        JsValue::String(s) => match s.char_code_at(i) {
            Some(unit) => JsValue::String(JsString::from_units(vec![unit])),
            None => JsValue::Undefined,
        },
        JsValue::Bytes(bytes) => bytes
            .get(i as usize)
            .map_or(JsValue::Undefined, |b| JsValue::Number(f64::from(*b))),
        _ => JsValue::Undefined,
    }
}

/// The raw `valueSchemas` of a stored schema.
#[derive(Clone, Debug)]
enum SchemaList {
    Items(Vec<ValueSchema>),
    /// `undefined` or `null`: `.length` on it throws.
    Missing(&'static str),
    /// A primitive or non-array object: `.length` and `[i]` reads still apply.
    Other(JsValue),
}

/// A `valueSchemas` element: fields parsed through JS coercions, or any
/// non-object kept verbatim.
#[derive(Clone, Debug)]
enum ValueSchema {
    Fields {
        name: JsString,
        ty: JsString,
        enum_value: JsValue,
        array_length: Option<f64>,
        bit_range: BitRange,
    },
    Other(JsValue),
}

/// `valueSchema.bitRange`: elements `ToNumber`-coerced, or `undefined`/`null`
/// (whose `[0]` read throws).
#[derive(Clone, Debug)]
enum BitRange {
    Pair([f64; 2]),
    Missing(&'static str),
}

/// The typed view of a stored schema object.
#[derive(Clone, Debug)]
struct Schema {
    value_schemas: SchemaList,
}

/// `decode`/`decodeArray`'s return: the decoded object (or array for
/// `decodeArray`) plus the schema-type map `Log.putStruct` turns into `Empty`
/// fields. Both iterate in JS property order.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub data: JsValue,
    pub schema_types: JsObject<JsString>,
}

#[derive(Clone, Debug)]
pub struct StructDecoder {
    schema_strings: MapData,
    schemas: MapData,
}

impl Default for StructDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StructDecoder {
    pub fn new() -> Self {
        Self {
            schema_strings: MapData::Own(JsObject::new()),
            schemas: MapData::Own(JsObject::new()),
        }
    }

    /// `decoder.schemaStrings`, for `mergeWith`. `None` when a restore stored a
    /// non-object, matching the verbatim `undefined`.
    pub fn schema_strings(&self) -> Option<&JsObject> {
        match &self.schema_strings {
            MapData::Own(map) => Some(map),
            MapData::Other(_) => None,
        }
    }

    /// `this.schemaStrings[name]`, including string indexing on a stored
    /// string map; `undefined` elsewhere.
    fn schema_string(&self, name: &JsString) -> JsValue {
        match &self.schema_strings {
            MapData::Own(map) => map.get_key(name).cloned().unwrap_or_default(),
            MapData::Other(v) => index_get(
                v,
                asrs_js::value::array_index(name.units()).map_or(f64::NAN, f64::from),
            ),
        }
    }

    /// `Object.keys(this.schemaStrings)`.
    fn schema_string_keys(&self) -> Result<Vec<JsString>, JsError> {
        match &self.schema_strings {
            MapData::Own(map) => Ok(map.keys().cloned().collect()),
            MapData::Other(JsValue::Null) | MapData::Other(JsValue::Undefined) => Err(
                JsError::type_error("Cannot convert undefined or null to object"),
            ),
            MapData::Other(JsValue::String(s)) => Ok((0..s.len())
                .map(|i| JsString::from(number::to_string(i as f64)))
                .collect()),
            MapData::Other(_) => Ok(Vec::new()),
        }
    }

    /// `schemas[name]` with `in`-operator reachability.
    fn schema_ref(&self, name: &JsString) -> Result<Option<SchemaRef<'_>>, JsError> {
        match &self.schemas {
            MapData::Own(map) => {
                if let Some(schema) = map.get_key(name) {
                    return Ok(Some(SchemaRef::Own(schema)));
                }
                Ok(object_proto_member(name).map(SchemaRef::Inherited))
            }
            MapData::Other(v) => Err(in_type_error(name, v)),
        }
    }

    /// `schemas[name].length` for `compileSchema`.
    fn schema_bit_length(&self, name: &JsString) -> Result<Option<f64>, JsError> {
        Ok(match self.schema_ref(name)? {
            None => None,
            Some(SchemaRef::Own(schema)) => Some(length_of(schema)),
            Some(SchemaRef::Inherited(Inherited::Function(arity))) => Some(arity),
            Some(SchemaRef::Inherited(Inherited::Prototype)) => Some(f64::NAN),
        })
    }

    fn schema_in(&self, name: &JsString) -> Result<bool, JsError> {
        self.schemas.contains(name)
    }

    pub fn add_schema(&mut self, name: &JsString, schema: &[u8]) -> Result<(), JsError> {
        let text = JsValue::String(
            text_decoder()
                .decode(schema)
                .unwrap_or_else(|_| JsString::new()),
        );
        match &self.schema_strings {
            MapData::Own(map) => {
                if js_in(map, name) {
                    return Ok(());
                }
            }
            MapData::Other(v) => return Err(in_type_error(name, v)),
        }
        if let MapData::Own(map) = &mut self.schema_strings {
            map.insert(name.clone(), text);
        }
        loop {
            let keys = self.schema_string_keys()?;
            let mut progress = false;
            for schema_name in keys {
                if !self.schema_in(&schema_name)? {
                    progress |= self.compile_schema(&schema_name)?;
                }
            }
            if !progress {
                return Ok(());
            }
        }
    }

    /// `compileSchema(name, this.schemaStrings[name])`.
    fn compile_schema(&mut self, name: &JsString) -> Result<bool, JsError> {
        let schema = self.schema_string(name);
        let JsValue::String(schema) = schema else {
            return Err(match schema {
                JsValue::Null => read_error("null", "trim"),
                JsValue::Undefined => read_error("undefined", "trim"),
                _ => JsError::type_error("schema.trim is not a function"),
            });
        };
        let semis = js_string(";");
        let space = js_string(" ");
        let value_schema_strs: Vec<JsString> = schema
            .trim()
            .split(Some(&semis), None)
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let mut value_schemas: Vec<JsValue> = Vec::new();
        let mut metas: Vec<(JsString, Option<f64>, Option<f64>)> = Vec::new();
        for mut schema_str in value_schema_strs {
            let mut enum_data: Option<JsObject> = None;
            if schema_str.starts_with(&js_string("enum"), None) {
                let mut map = JsObject::new();
                let brace = js_string("{");
                let close = js_string("}");
                let enum_str_start = schema_str.index_of(&brace, None) as f64 + 1.0;
                let enum_str_end = schema_str.index_of(&close, None) as f64;
                let enum_str = JsString::from_units(
                    schema_str
                        .substring(enum_str_start, Some(enum_str_end))
                        .units()
                        .iter()
                        .copied()
                        .filter(|u| *u != u16::from(b' '))
                        .collect(),
                );
                for pair_str in enum_str
                    .split(Some(&js_string(",")), None)
                    .into_iter()
                    .filter(|s| !s.is_empty())
                {
                    let pair = pair_str.split(Some(&js_string("=")), None);
                    if pair.len() == 2 {
                        let key = number::to_number(pair[1].units());
                        if !key.is_nan() {
                            map.insert(number::to_string(key), JsValue::String(pair[0].clone()));
                        }
                    }
                }
                enum_data = Some(map);
                schema_str = schema_str.substring(enum_str_end + 1.0, None);
            }

            let mut tokens = schema_str
                .split(Some(&space), None)
                .into_iter()
                .filter(|s| !s.is_empty());
            let ty = tokens.next().unwrap_or_else(|| js_string("undefined"));
            if !is_valid_type(&ty) && !self.schema_in(&ty)? {
                return Ok(false);
            }
            let name_str = tokens.fold(JsString::new(), |mut acc, t| {
                acc.push_str(&t);
                acc
            });

            let field_name: JsString;
            let mut bitfield_width = None;
            let mut array_length = None;
            if name_str.includes(&js_string(":"), None) {
                let split = name_str.split(Some(&js_string(":")), None);
                field_name = split[0].clone();
                bitfield_width = Some(number::to_number(split[1].units()));
                if !is_bitfield_type(&ty) {
                    continue;
                }
                if is_bool(&ty) && bitfield_width != Some(1.0) {
                    continue;
                }
            } else if name_str.includes(&js_string("["), None) {
                let split = name_str.split(Some(&js_string("[")), None);
                field_name = split[0].clone();
                array_length = Some(number::to_number(
                    split[1].split(Some(&js_string("]")), None)[0].units(),
                ));
            } else {
                field_name = name_str;
            }

            value_schemas.push(JsValue::Object(JsObject::from_iter([
                (js_string("name"), JsValue::String(field_name.clone())),
                (js_string("type"), JsValue::String(ty.clone())),
                (
                    js_string("enum"),
                    enum_data.map_or(JsValue::Null, JsValue::Object),
                ),
                (
                    js_string("bitfieldWidth"),
                    bitfield_width.map_or(JsValue::Null, JsValue::Number),
                ),
                (
                    js_string("arrayLength"),
                    array_length.map_or(JsValue::Null, JsValue::Number),
                ),
                (
                    js_string("bitRange"),
                    JsValue::Array(vec![JsValue::Number(0.0), JsValue::Number(0.0)]),
                ),
            ])));
            metas.push((ty, bitfield_width, array_length));
        }

        // Upstream mutates `valueSchemas[i].bitRange` in place; the loop below
        // tracks the same positions and writes them back into the objects.
        let mut bit_position = 0.0_f64;
        let mut bitfield: Option<(f64, f64)> = None;
        let mut bit_ranges: Vec<[f64; 2]> = Vec::with_capacity(value_schemas.len());
        for (ty, bitfield_width, array_length) in &metas {
            let array_length = *array_length;
            if !is_valid_type(ty) {
                if let Some((position, length)) = bitfield.take() {
                    bit_position += length - position;
                }
                let mut length = self.schema_bit_length(ty)?.unwrap_or(f64::NAN);
                if let Some(array_length) = array_length {
                    length *= array_length;
                }
                bit_ranges.push([bit_position, bit_position + length]);
                bit_position += length;
            } else if bitfield_width.is_none() {
                if let Some((position, length)) = bitfield.take() {
                    bit_position += length - position;
                }
                let mut bit_length = type_max_bits(ty);
                if let Some(array_length) = array_length {
                    bit_length *= array_length;
                }
                bit_ranges.push([bit_position, bit_position + bit_length]);
                bit_position += bit_length;
            } else {
                let type_length = type_max_bits(ty);
                let value_bit_length =
                    math::min(&[bitfield_width.unwrap_or(f64::NAN), type_length]);
                let needs_new = match bitfield {
                    None => true,
                    Some((position, length)) => {
                        (!is_bool(ty) && length != type_length)
                            || position + value_bit_length > length
                    }
                };
                if needs_new {
                    if let Some((position, length)) = bitfield.take() {
                        bit_position += length - position;
                    }
                    bitfield = Some((0.0, type_length));
                }
                let (position, _) = bitfield.as_mut().expect("bitfield just opened");
                bit_ranges.push([bit_position, bit_position + value_bit_length]);
                *position += value_bit_length;
                bit_position += value_bit_length;
            }
        }
        if let Some((position, length)) = bitfield {
            bit_position += length - position;
        }

        for (value_schema, range) in value_schemas.iter_mut().zip(bit_ranges) {
            if let JsValue::Object(map) = value_schema {
                map.insert(
                    js_string("bitRange"),
                    JsValue::Array(vec![JsValue::Number(range[0]), JsValue::Number(range[1])]),
                );
            }
        }
        self.insert_schema(
            name,
            JsValue::Object(JsObject::from_iter([
                (js_string("length"), JsValue::Number(bit_position)),
                (js_string("valueSchemas"), JsValue::Array(value_schemas)),
            ])),
        );
        Ok(true)
    }

    fn insert_schema(&mut self, name: &JsString, schema: JsValue) {
        if let MapData::Own(map) = &mut self.schemas {
            map.insert(name.clone(), schema);
        }
    }

    /// `decode(name, value)`: the `bitLength` parameter is dead upstream
    /// (assigned and never read), so it is not part of this signature.
    pub fn decode(&self, name: &JsString, value: &[u8]) -> Result<Decoded, JsError> {
        let schema = match self.schema_ref(name)? {
            Some(SchemaRef::Own(schema)) => schema,
            Some(SchemaRef::Inherited(_)) => {
                // `schema.valueSchemas` is `undefined` on functions and on
                // `Object.prototype`; reading `.length` then throws.
                return Err(read_error("undefined", "length"));
            }
            None => return Err(JsError::new("Error", "Schema not defined")),
        };
        let mut output_data = JsObject::new();
        let mut output_schema_types = JsObject::<JsString>::new();
        let schema_view = match schema {
            JsValue::Null => return Err(read_error("null", "valueSchemas")),
            JsValue::Undefined => return Err(read_error("undefined", "valueSchemas")),
            other => schema_view(other),
        };
        let mut emit = |value_schema: &ValueSchema| -> Result<(), JsError> {
            let ValueSchema::Fields {
                name,
                ty,
                enum_value,
                array_length,
                bit_range,
                ..
            } = value_schema
            else {
                let ValueSchema::Other(v) = value_schema else {
                    unreachable!()
                };
                return Err(match v {
                    JsValue::Null => read_error("null", "bitRange"),
                    JsValue::Undefined => read_error("undefined", "bitRange"),
                    _ => read_error("undefined", "0"),
                });
            };
            let range = match bit_range {
                BitRange::Pair(range) => *range,
                BitRange::Missing(kind) => return Err(read_error(kind, "0")),
            };
            let (value_array, value_bit_length) = slice_bits(value, range);
            if is_valid_type(ty) {
                if array_length.is_none() {
                    set_property(
                        &mut output_data,
                        name,
                        decode_value(&value_array, ty, enum_value)?,
                    );
                } else {
                    let mut items = Vec::new();
                    let item_length = (range[1] - range[0]) / array_length.unwrap_or(f64::NAN);
                    let mut position = 0.0_f64;
                    while position < value_bit_length {
                        let (item, _) =
                            slice_bits(&value_array, [position, position + item_length]);
                        items.push(decode_value(&item, ty, enum_value)?);
                        position += item_length;
                    }
                    let decoded = if is_char(ty) {
                        // `value.join("")`: each item was TextDecoder-decoded
                        // on its own one-byte slice already.
                        let mut joined = JsString::new();
                        for item in items {
                            if let JsValue::String(s) = item {
                                joined.push_str(&s);
                            }
                        }
                        JsValue::String(joined)
                    } else {
                        JsValue::Array(items)
                    };
                    set_property(&mut output_data, name, decoded);
                }
            } else {
                let is_array = array_length.is_some();
                let mut schema_type = ty.clone();
                if is_array {
                    schema_type.push_str(&js_string("[]"));
                }
                set_property(&mut output_schema_types, name, schema_type);
                let child = if is_array {
                    self.decode_array(ty, &value_array, *array_length)?
                } else {
                    self.decode(ty, &value_array)?
                };
                set_property(&mut output_data, name, child.data);
                for (field, field_type) in child.schema_types.iter() {
                    let mut key = name.clone();
                    key.push_str(&js_string("/"));
                    key.push_str(field);
                    set_property(&mut output_schema_types, &key, field_type.clone());
                }
            }
            Ok(())
        };
        match &schema_view.value_schemas {
            SchemaList::Items(items) => {
                for value_schema in items {
                    emit(value_schema)?;
                }
            }
            SchemaList::Missing(kind) => return Err(read_error(kind, "length")),
            SchemaList::Other(raw) => {
                let mut i = 0.0_f64;
                while i < length_of(raw) {
                    emit(&value_schema_view(&index_get(raw, i)))?;
                    i += 1.0;
                }
            }
        }
        Ok(Decoded {
            data: JsValue::Object(output_data),
            schema_types: output_schema_types,
        })
    }

    /// `decodeArray(name, value, arrayLength?)`.
    pub fn decode_array(
        &self,
        name: &JsString,
        value: &[u8],
        array_length: Option<f64>,
    ) -> Result<Decoded, JsError> {
        let schema_length = match self.schema_ref(name)? {
            Some(SchemaRef::Own(schema)) => match schema {
                JsValue::Null => return Err(read_error("null", "length")),
                JsValue::Undefined => return Err(read_error("undefined", "length")),
                other => length_of(other) / 8.0,
            },
            Some(SchemaRef::Inherited(Inherited::Function(arity))) => arity / 8.0,
            Some(SchemaRef::Inherited(Inherited::Prototype)) => f64::NAN,
            None => return Err(JsError::new("Error", "Schema not defined")),
        };
        let length = array_length.unwrap_or(value.len() as f64 / schema_length);
        let mut output_data = Vec::new();
        let mut output_schema_types = JsObject::<JsString>::new();
        let mut i = 0.0_f64;
        while i < length {
            let slice = js_slice(value, i * schema_length, (i + 1.0) * schema_length);
            let decoded = self.decode(name, &slice)?;
            output_data.push(decoded.data);
            for (item_key, item_type) in decoded.schema_types.iter() {
                let mut key = JsString::from(number::to_string(i));
                key.push_str(&js_string("/"));
                key.push_str(item_key);
                set_property(&mut output_schema_types, &key, item_type.clone());
            }
            set_property(
                &mut output_schema_types,
                &JsString::from(number::to_string(i)),
                name.clone(),
            );
            i += 1.0;
        }
        Ok(Decoded {
            data: JsValue::Array(output_data),
            schema_types: output_schema_types,
        })
    }

    /// `toSerialized()`: the stored objects verbatim.
    pub fn to_serialized(&self) -> JsValue {
        JsValue::Object(JsObject::from_iter([
            (js_string("schemaStrings"), self.schema_strings.to_js()),
            (js_string("schemas"), self.schemas.to_js()),
        ]))
    }

    /// `StructDecoder.fromSerialized(serializedData)`: stores the objects
    /// verbatim; `null`/`undefined` `serializedData` throws.
    pub fn from_serialized(data: &JsValue) -> Result<Self, JsError> {
        let field = |key: &'static str| -> Result<MapData, JsError> {
            let value = match data {
                JsValue::Object(map) => map.get(key).cloned().unwrap_or_default(),
                JsValue::Null => return Err(read_error("null", key)),
                JsValue::Undefined => return Err(read_error("undefined", key)),
                _ => JsValue::Undefined,
            };
            Ok(match value {
                JsValue::Object(map) => MapData::Own(map),
                other => MapData::Other(other),
            })
        };
        Ok(StructDecoder {
            schema_strings: field("schemaStrings")?,
            schemas: field("schemas")?,
        })
    }
}

enum SchemaRef<'a> {
    Own(&'a JsValue),
    Inherited(Inherited),
}

/// Parses a stored `valueSchemas` element through JS property reads.
fn value_schema_view(item: &JsValue) -> ValueSchema {
    let Some(fields) = item.as_object() else {
        return ValueSchema::Other(item.clone());
    };
    let get = |key: &str| fields.get(key).cloned().unwrap_or_default();
    // `!== null` upstream: `undefined` and every non-number stay present as `NaN`.
    let nullish = |key: &str| match fields.get(key) {
        Some(JsValue::Null) => None,
        Some(v) => Some(to_number(v)),
        None => Some(f64::NAN),
    };
    let bit_range = match get("bitRange") {
        JsValue::Null => BitRange::Missing("null"),
        JsValue::Undefined => BitRange::Missing("undefined"),
        raw => BitRange::Pair([
            to_number(&index_get(&raw, 0.0)),
            to_number(&index_get(&raw, 1.0)),
        ]),
    };
    ValueSchema::Fields {
        name: to_property_key(&get("name")),
        ty: to_property_key(&get("type")),
        enum_value: get("enum"),
        array_length: nullish("arrayLength"),
        bit_range,
    }
}

/// Parses a stored schema object through JS property reads.
fn schema_view(entry: &JsValue) -> Schema {
    let object = entry.as_object();
    let value_schemas = match object.and_then(|o| o.get("valueSchemas")) {
        Some(JsValue::Array(items)) => {
            SchemaList::Items(items.iter().map(value_schema_view).collect())
        }
        Some(JsValue::Null) => SchemaList::Missing("null"),
        None => SchemaList::Missing("undefined"),
        Some(other) => SchemaList::Other(other.clone()),
    };
    Schema { value_schemas }
}

/// `output in enumData ? enumData[output] : output` for a decoded primitive.
fn enum_lookup(enum_value: &JsValue, output: &JsValue) -> Result<JsValue, JsError> {
    let key: Vec<u16> = match output {
        JsValue::Bool(b) => if *b { "true" } else { "false" }.encode_utf16().collect(),
        JsValue::Number(x) => number::to_string(*x).encode_utf16().collect(),
        JsValue::String(s) => s.units().to_vec(),
        _ => return Ok(output.clone()),
    };
    let lookup = match enum_value {
        JsValue::Null => return Ok(output.clone()),
        JsValue::Object(map) => map.get_units(&key).cloned(),
        JsValue::Array(items) => asrs_js::value::array_index(&key)
            .and_then(|i| items.get(i as usize))
            .cloned(),
        JsValue::Bytes(bytes) => asrs_js::value::array_index(&key)
            .and_then(|i| bytes.get(i as usize))
            .map(|b| JsValue::Number(f64::from(*b))),
        // `output in` an object never matches the decoded primitives.
        JsValue::Date(_) | JsValue::ExtData(..) => None,
        // `output in enumData` throws for every other primitive enum value.
        other => {
            return Err(JsError::type_error(format!(
                "Cannot use 'in' operator to search for '{}' in {}",
                String::from_utf16_lossy(&key),
                to_property_key(other).to_string_lossy()
            )));
        }
    };
    Ok(lookup.unwrap_or_else(|| output.clone()))
}

fn decode_value(value: &[u8], ty: &JsString, enum_value: &JsValue) -> Result<JsValue, JsError> {
    let width = (type_max_bits(ty) / 8.0) as usize;
    if value.len() > width {
        // `paddedValue.set(value)` upstream.
        return Err(JsError::range("offset is out of bounds"));
    }
    let mut padded = [0u8; 8];
    padded[..value.len()].copy_from_slice(value);
    let output = if is_bool(ty) {
        JsValue::Bool(padded[0] > 0)
    } else if is_char(ty) {
        JsValue::String(
            text_decoder()
                .decode(value)
                .unwrap_or_else(|_| JsString::new()),
        )
    } else if js_eq(ty, "int8") {
        JsValue::Number(f64::from(padded[0] as i8))
    } else if js_eq(ty, "int16") {
        JsValue::Number(f64::from(i16::from_le_bytes(
            padded[..2].try_into().unwrap(),
        )))
    } else if js_eq(ty, "int32") {
        JsValue::Number(f64::from(i32::from_le_bytes(
            padded[..4].try_into().unwrap(),
        )))
    } else if js_eq(ty, "int64") {
        JsValue::Number(i64::from_le_bytes(padded[..8].try_into().unwrap()) as f64)
    } else if js_eq(ty, "uint8") {
        JsValue::Number(f64::from(padded[0]))
    } else if js_eq(ty, "uint16") {
        JsValue::Number(f64::from(u16::from_le_bytes(
            padded[..2].try_into().unwrap(),
        )))
    } else if js_eq(ty, "uint32") {
        JsValue::Number(f64::from(u32::from_le_bytes(
            padded[..4].try_into().unwrap(),
        )))
    } else if js_eq(ty, "uint64") {
        JsValue::Number(u64::from_le_bytes(padded[..8].try_into().unwrap()) as f64)
    } else if js_eq(ty, "float") || js_eq(ty, "float32") {
        JsValue::Number(f64::from(f32::from_le_bytes(
            padded[..4].try_into().unwrap(),
        )))
    } else {
        JsValue::Number(f64::from_le_bytes(padded[..8].try_into().unwrap()))
    };
    enum_lookup(enum_value, &output)
}

/// `Array.prototype.slice` / `TypedArray.prototype.slice` index math:
/// `ToIntegerOrInfinity` on each argument, negatives relative to the length.
fn slice_bounds(start: f64, end: f64, len: usize) -> (usize, usize) {
    let len = len as f64;
    let bound = |x: f64| {
        let i = to_integer_or_infinity(x);
        if i < 0.0 {
            math::max(&[len + i, 0.0])
        } else {
            math::min(&[i, len])
        }
    };
    let (s, e) = (bound(start), bound(end));
    (s as usize, math::max(&[e - s, 0.0]) as usize)
}

fn js_slice(input: &[u8], start: f64, end: f64) -> Vec<u8> {
    let (s, e) = slice_bounds(start, end, input.len());
    input[s..s + e].to_vec()
}

fn slice_bits(input: &[u8], range: [f64; 2]) -> (Vec<u8>, f64) {
    if range[0] % 8.0 == 0.0 && range[1] % 8.0 == 0.0 {
        (
            js_slice(input, range[0] / 8.0, range[1] / 8.0),
            range[1] - range[0],
        )
    } else {
        let (s, count) = slice_bounds(range[0], range[1], input.len() * 8);
        let mut out = vec![0u8; count.div_ceil(8)];
        for (i, out_byte) in out.iter_mut().enumerate() {
            for bit in 0..8 {
                let index = i * 8 + bit;
                if index < count {
                    let src = s + index;
                    if input[src / 8] >> (src % 8) & 1 != 0 {
                        *out_byte |= 1 << bit;
                    }
                }
            }
        }
        (out, range[1] - range[0])
    }
}
