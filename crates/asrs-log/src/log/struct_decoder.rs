//! `shared/log/StructDecoder.ts`: the WPILib struct schema compiler and bit-level
//! decoder, including the plain-object lookups upstream performs with `in`
//! (which see `Object.prototype` members) and the `__proto__` assignment magic
//! that swallows decoded fields of that name.

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
    } else if js_eq(ty, "int32") || js_eq(ty, "uint32") || js_eq(ty, "float") || js_eq(ty, "float32")
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
fn js_in<T>(obj: &JsObject<T>, key: &JsString) -> bool {
    obj.get_key(key).is_some() || object_proto_member(key).is_some()
}

enum SchemaRef<'a> {
    Schema(&'a Schema),
    Inherited(Inherited),
}

/// `interfaces Schema` and `ValueSchema`. `bitfieldWidth`/`arrayLength` are JS
/// `number | null`, so `NaN` stays distinct from "absent".
#[derive(Clone, Debug)]
pub struct Schema {
    pub length: f64,
    pub value_schemas: Vec<ValueSchema>,
}

#[derive(Clone, Debug)]
pub struct ValueSchema {
    pub name: JsString,
    pub ty: JsString,
    pub enum_map: Option<JsObject<JsString>>,
    pub bitfield_width: Option<f64>,
    pub array_length: Option<f64>,
    pub bit_range: [f64; 2],
}

/// `decode`/`decodeArray`'s return: the decoded object (or array for
/// `decodeArray`) plus the schema-type map `Log.putStruct` turns into `Empty`
/// fields. Both iterate in JS property order.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub data: JsValue,
    pub schema_types: JsObject<JsString>,
}

#[derive(Clone, Debug, Default)]
pub struct StructDecoder {
    schema_strings: JsObject<JsString>,
    schemas: JsObject<Schema>,
}

impl StructDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `decoder.schemaStrings`, for `toSerialized` and `mergeWith`.
    pub fn schema_strings(&self) -> &JsObject<JsString> {
        &self.schema_strings
    }

    /// `decoder.schemas`, for `toSerialized` and `mergeWith`.
    pub fn schemas(&self) -> &JsObject<Schema> {
        &self.schemas
    }

    /// `schemas[name]` with `in`-operator reachability: an own entry wins, then
    /// an `Object.prototype` member.
    fn schema_ref(&self, name: &JsString) -> Option<SchemaRef<'_>> {
        if let Some(schema) = self.schemas.get_key(name) {
            return Some(SchemaRef::Schema(schema));
        }
        object_proto_member(name).map(SchemaRef::Inherited)
    }

    /// `schemas[name].length`: `NaN` for the `Object.prototype` object itself.
    fn schema_bit_length(&self, name: &JsString) -> Option<f64> {
        match self.schema_ref(name)? {
            SchemaRef::Schema(s) => Some(s.length),
            SchemaRef::Inherited(Inherited::Function(arity)) => Some(arity),
            SchemaRef::Inherited(Inherited::Prototype) => Some(f64::NAN),
        }
    }

    fn schema_in(&self, name: &JsString) -> bool {
        js_in(&self.schemas, name)
    }

    pub fn add_schema(&mut self, name: &JsString, schema: &[u8]) {
        let text = text_decoder()
            .decode(schema)
            .unwrap_or_else(|_| JsString::new());
        if js_in(&self.schema_strings, name) {
            return;
        }
        self.schema_strings.insert(name.clone(), text);
        loop {
            let pending: Vec<(JsString, JsString)> = self
                .schema_strings
                .iter()
                .filter(|(n, _)| !self.schema_in(n))
                .map(|(n, s)| (n.clone(), s.clone()))
                .collect();
            if pending.is_empty() {
                return;
            }
            let mut progress = false;
            for (schema_name, schema_text) in pending {
                progress |= self.compile_schema(&schema_name, &schema_text);
            }
            if !progress {
                return;
            }
        }
    }

    fn compile_schema(&mut self, name: &JsString, schema: &JsString) -> bool {
        let semis = js_string(";");
        let space = js_string(" ");
        let value_schema_strs: Vec<JsString> = schema
            .trim()
            .split(Some(&semis), None)
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
        let mut value_schemas: Vec<ValueSchema> = Vec::new();
        for mut schema_str in value_schema_strs {
            let mut enum_data: Option<JsObject<JsString>> = None;
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
                            map.insert(number::to_string(key), pair[0].clone());
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
            if !is_valid_type(&ty) && !self.schema_in(&ty) {
                return false;
            }
            let name_str = tokens.fold(JsString::new(), |mut acc, t| {
                acc.push_str(&t);
                acc
            });

            let name: JsString;
            let mut bitfield_width = None;
            let mut array_length = None;
            if name_str.includes(&js_string(":"), None) {
                let split = name_str.split(Some(&js_string(":")), None);
                name = split[0].clone();
                bitfield_width = Some(number::to_number(split[1].units()));
                if !is_bitfield_type(&ty) {
                    continue;
                }
                if is_bool(&ty) && bitfield_width != Some(1.0) {
                    continue;
                }
            } else if name_str.includes(&js_string("["), None) {
                let split = name_str.split(Some(&js_string("[")), None);
                name = split[0].clone();
                array_length = Some(number::to_number(
                    split[1].split(Some(&js_string("]")), None)[0].units(),
                ));
            } else {
                name = name_str;
            }

            value_schemas.push(ValueSchema {
                name,
                ty,
                enum_map: enum_data,
                bitfield_width,
                array_length,
                bit_range: [0.0, 0.0],
            });
        }

        let mut bit_position = 0.0_f64;
        let mut bitfield: Option<(f64, f64)> = None;
        for value_schema in &mut value_schemas {
            if !is_valid_type(&value_schema.ty) {
                if let Some((position, length)) = bitfield.take() {
                    bit_position += length - position;
                }
                let mut length = self
                    .schema_bit_length(&value_schema.ty)
                    .unwrap_or(f64::NAN);
                if let Some(array_length) = value_schema.array_length {
                    length *= array_length;
                }
                value_schema.bit_range = [bit_position, bit_position + length];
                bit_position += length;
            } else if value_schema.bitfield_width.is_none() {
                if let Some((position, length)) = bitfield.take() {
                    bit_position += length - position;
                }
                let mut bit_length = type_max_bits(&value_schema.ty);
                if let Some(array_length) = value_schema.array_length {
                    bit_length *= array_length;
                }
                value_schema.bit_range = [bit_position, bit_position + bit_length];
                bit_position += bit_length;
            } else {
                let type_length = type_max_bits(&value_schema.ty);
                let value_bit_length =
                    math::min(&[value_schema.bitfield_width.unwrap_or(f64::NAN), type_length]);
                let needs_new = match bitfield {
                    None => true,
                    Some((position, length)) => {
                        (!is_bool(&value_schema.ty) && length != type_length)
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
                value_schema.bit_range = [bit_position, bit_position + value_bit_length];
                *position += value_bit_length;
                bit_position += value_bit_length;
            }
        }
        if let Some((position, length)) = bitfield {
            bit_position += length - position;
        }

        self.schemas.insert(
            name.clone(),
            Schema {
                length: bit_position,
                value_schemas,
            },
        );
        true
    }

    /// `decode(name, value)`: the `bitLength` parameter is dead upstream
    /// (assigned and never read), so it is not part of this signature.
    pub fn decode(&self, name: &JsString, value: &[u8]) -> Result<Decoded, JsError> {
        let schema = match self.schema_ref(name) {
            Some(SchemaRef::Schema(schema)) => schema,
            Some(SchemaRef::Inherited(_)) => {
                // `schema.valueSchemas` is `undefined` on functions and on
                // `Object.prototype`; reading `.length` then throws.
                return Err(JsError::type_error(
                    "Cannot read properties of undefined (reading 'length')",
                ));
            }
            None => return Err(JsError::new("Error", "Schema not defined")),
        };
        let mut output_data = JsObject::<JsValue>::new();
        let mut output_schema_types = JsObject::<JsString>::new();
        for value_schema in &schema.value_schemas {
            let (value_array, value_bit_length) = slice_bits(value, value_schema.bit_range);
            if is_valid_type(&value_schema.ty) {
                if value_schema.array_length.is_none() {
                    set_property(
                        &mut output_data,
                        &value_schema.name,
                        decode_value(
                            &value_array,
                            &value_schema.ty,
                            value_schema.enum_map.as_ref(),
                        )?,
                    );
                } else {
                    let mut items = Vec::new();
                    let item_length = (value_schema.bit_range[1] - value_schema.bit_range[0])
                        / value_schema.array_length.unwrap_or(f64::NAN);
                    let mut position = 0.0_f64;
                    while position < value_bit_length {
                        let (item, _) =
                            slice_bits(&value_array, [position, position + item_length]);
                        items.push(decode_value(
                            &item,
                            &value_schema.ty,
                            value_schema.enum_map.as_ref(),
                        )?);
                        position += item_length;
                    }
                    let decoded = if is_char(&value_schema.ty) {
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
                    set_property(&mut output_data, &value_schema.name, decoded);
                }
            } else {
                let is_array = value_schema.array_length.is_some();
                let mut schema_type = value_schema.ty.clone();
                if is_array {
                    schema_type.push_str(&js_string("[]"));
                }
                set_property(&mut output_schema_types, &value_schema.name, schema_type);
                let child = if is_array {
                    self.decode_array(&value_schema.ty, &value_array, value_schema.array_length)?
                } else {
                    self.decode(&value_schema.ty, &value_array)?
                };
                set_property(&mut output_data, &value_schema.name, child.data);
                for (field, field_type) in child.schema_types.iter() {
                    let mut key = value_schema.name.clone();
                    key.push_str(&js_string("/"));
                    key.push_str(field);
                    set_property(&mut output_schema_types, &key, field_type.clone());
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
        let schema_length = match self.schema_ref(name) {
            Some(SchemaRef::Schema(schema)) => schema.length / 8.0,
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

    /// `toSerialized()`.
    pub fn to_serialized(&self) -> JsValue {
        let mut schema_strings = JsObject::<JsValue>::new();
        for (name, text) in self.schema_strings.iter() {
            schema_strings.insert(name.clone(), JsValue::String(text.clone()));
        }
        let mut schemas = JsObject::<JsValue>::new();
        for (name, schema) in self.schemas.iter() {
            schemas.insert(name.clone(), schema_to_js(schema));
        }
        JsValue::Object(JsObject::from_iter([
            (js_string("schemaStrings"), JsValue::Object(schema_strings)),
            (js_string("schemas"), JsValue::Object(schemas)),
        ]))
    }

    /// `StructDecoder.fromSerialized(serializedData)`: restores the maps as-is.
    /// Missing leaves read as `undefined`, matching the JS property reads.
    pub fn from_serialized(data: &JsValue) -> Self {
        let mut decoder = StructDecoder::new();
        if let Some(strings) = data.get("schemaStrings").and_then(JsValue::as_object) {
            for (name, text) in strings.iter() {
                if let JsValue::String(text) = text {
                    decoder.schema_strings.insert(name.clone(), text.clone());
                }
            }
        }
        if let Some(schemas) = data.get("schemas").and_then(JsValue::as_object) {
            for (name, value) in schemas.iter() {
                if let Some(schema) = schema_from_js(value) {
                    decoder.schemas.insert(name.clone(), schema);
                }
            }
        }
        decoder
    }
}

fn schema_to_js(schema: &Schema) -> JsValue {
    let mut value_schemas = Vec::new();
    for vs in &schema.value_schemas {
        value_schemas.push(JsValue::Object(JsObject::from_iter([
            (js_string("name"), JsValue::String(vs.name.clone())),
            (js_string("type"), JsValue::String(vs.ty.clone())),
            (
                js_string("enum"),
                vs.enum_map
                    .as_ref()
                    .map(|map| {
                        JsValue::Object(
                            map.iter()
                                .map(|(k, v)| (k.clone(), JsValue::String(v.clone())))
                                .collect(),
                        )
                    })
                    .unwrap_or(JsValue::Null),
            ),
            (
                js_string("bitfieldWidth"),
                vs.bitfield_width.map_or(JsValue::Null, JsValue::Number),
            ),
            (
                js_string("arrayLength"),
                vs.array_length.map_or(JsValue::Null, JsValue::Number),
            ),
            (
                js_string("bitRange"),
                JsValue::Array(vec![
                    JsValue::Number(vs.bit_range[0]),
                    JsValue::Number(vs.bit_range[1]),
                ]),
            ),
        ])));
    }
    JsValue::Object(JsObject::from_iter([
        (js_string("length"), JsValue::Number(schema.length)),
        (js_string("valueSchemas"), JsValue::Array(value_schemas)),
    ]))
}

fn js_f64(value: &JsValue) -> f64 {
    match value {
        JsValue::Number(x) => *x,
        _ => f64::NAN,
    }
}

fn js_string_prop(object: &JsObject, key: &str) -> Option<JsString> {
    match object.get(key) {
        Some(JsValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn schema_from_js(value: &JsValue) -> Option<Schema> {
    let object = value.as_object()?;
    let length = object.get("length").map(js_f64).unwrap_or(f64::NAN);
    let mut value_schemas = Vec::new();
    if let Some(items) = object.get("valueSchemas").and_then(JsValue::as_array) {
        for item in items {
            let Some(fields) = item.as_object() else {
                continue;
            };
            let enum_map = match fields.get("enum") {
                Some(JsValue::Object(map)) => Some(
                    map.iter()
                        .filter_map(|(k, v)| match v {
                            JsValue::String(s) => Some((k.clone(), s.clone())),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            };
            // `null` stays absent; `undefined` and every non-number read as a
            // present `NaN` (`!== null` in upstream's checks).
            let nullish = |key: &str| match fields.get(key) {
                Some(JsValue::Null) => None,
                Some(JsValue::Number(x)) => Some(*x),
                _ => Some(f64::NAN),
            };
            let bit_range = match fields.get("bitRange").and_then(JsValue::as_array) {
                Some(pair) => [
                    pair.first().map(js_f64).unwrap_or(f64::NAN),
                    pair.get(1).map(js_f64).unwrap_or(f64::NAN),
                ],
                None => [f64::NAN, f64::NAN],
            };
            value_schemas.push(ValueSchema {
                name: js_string_prop(fields, "name").unwrap_or_default(),
                ty: js_string_prop(fields, "type").unwrap_or_default(),
                enum_map,
                bitfield_width: nullish("bitfieldWidth"),
                array_length: nullish("arrayLength"),
                bit_range,
            });
        }
    }
    Some(Schema {
        length,
        value_schemas,
    })
}

/// `obj[key] = value` where `key` is `"__proto__"`: the prototype setter runs
/// instead, and no own property ever appears.
fn set_property<V>(object: &mut JsObject<V>, key: &JsString, value: V) {
    if is_proto(key) {
        return;
    }
    object.insert(key.clone(), value);
}

/// `output in enumData`: the property-key coercion of the decoded value against
/// the enum's own keys.
fn enum_lookup(map: &JsObject<JsString>, output: &JsValue) -> Option<JsValue> {
    let key: Vec<u16> = match output {
        JsValue::Bool(b) => if *b { "true" } else { "false" }
            .encode_utf16()
            .collect(),
        JsValue::Number(x) => number::to_string(*x).encode_utf16().collect(),
        JsValue::String(s) => s.units().to_vec(),
        _ => return None,
    };
    map.get_units(&key).map(|s| JsValue::String(s.clone()))
}

fn decode_value(
    value: &[u8],
    ty: &JsString,
    enum_map: Option<&JsObject<JsString>>,
) -> Result<JsValue, JsError> {
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
        JsValue::Number(f64::from(i16::from_le_bytes(padded[..2].try_into().unwrap())))
    } else if js_eq(ty, "int32") {
        JsValue::Number(f64::from(i32::from_le_bytes(padded[..4].try_into().unwrap())))
    } else if js_eq(ty, "int64") {
        JsValue::Number(i64::from_le_bytes(padded[..8].try_into().unwrap()) as f64)
    } else if js_eq(ty, "uint8") {
        JsValue::Number(f64::from(padded[0]))
    } else if js_eq(ty, "uint16") {
        JsValue::Number(f64::from(u16::from_le_bytes(padded[..2].try_into().unwrap())))
    } else if js_eq(ty, "uint32") {
        JsValue::Number(f64::from(u32::from_le_bytes(padded[..4].try_into().unwrap())))
    } else if js_eq(ty, "uint64") {
        JsValue::Number(u64::from_le_bytes(padded[..8].try_into().unwrap()) as f64)
    } else if js_eq(ty, "float") || js_eq(ty, "float32") {
        JsValue::Number(f64::from(f32::from_le_bytes(
            padded[..4].try_into().unwrap(),
        )))
    } else {
        JsValue::Number(f64::from_le_bytes(padded[..8].try_into().unwrap()))
    };
    match enum_map.and_then(|map| enum_lookup(map, &output)) {
        Some(replacement) => Ok(replacement),
        None => Ok(output),
    }
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
        (js_slice(input, range[0] / 8.0, range[1] / 8.0), range[1] - range[0])
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
