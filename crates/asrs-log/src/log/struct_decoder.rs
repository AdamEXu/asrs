//! `shared/log/StructDecoder.ts`: the WPILib struct schema compiler and bit-level
//! decoder.
//!
//! Upstream keeps `schemaStrings` and `schemas` in plain objects, so lookups use the
//! `in` operator and see `Object.prototype` members. Schema names come from log
//! data, which makes that reachable: a field typed `toString` compiles against the
//! method's arity, and decoding such a name throws.

use asrs_js::math;
use asrs_js::number::{self, to_integer_or_infinity};
use asrs_js::string::{JsString, TextDecoder};
use asrs_js::value::{JsObject, JsValue, ToJs};

const VALID_TYPE_STRINGS: &[&str] = &[
    "bool", "char", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64",
    "float", "float32", "double", "float64",
];

const BITFIELD_VALID_TYPES: &[&str] = &[
    "bool", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64",
];

/// The most elements one JS array holds in the oracle's Electron (measured with
/// `push` of small integers, doubles, booleans and one-character strings): the
/// push past it throws `RangeError: Invalid array length`. Upstream takes minutes
/// to get there, so no oracle vector covers it.
pub const MAX_ARRAY_LENGTH: f64 = 134_217_728.0;

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

/// What `obj[name]` resolves to on a plain `{}` that has no own `name`.
#[derive(Clone, Copy)]
enum Inherited {
    /// `__proto__`: `Object.prototype` itself.
    Prototype,
    /// A method, with its `Function.length`.
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

fn js_in<V>(obj: &JsObject<V>, key: &JsString) -> bool {
    obj.get_key(key).is_some() || object_proto_member(key).is_some()
}

/// `obj[key] = value` on a plain object. Assigning to `__proto__` runs the
/// prototype setter, which never creates an own property, and `Object.entries`
/// (all `Log.putUnknownStruct` reads) only sees own properties.
fn set_property<V>(object: &mut JsObject<V>, key: &JsString, value: V) {
    if is_proto(key) {
        return;
    }
    object.insert(key.clone(), value);
}

/// A compiled schema, as upstream stores it in `schemas[name]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    /// Total bit length; fractional, negative or NaN for odd array lengths.
    pub length: f64,
    pub value_schemas: Vec<ValueSchema>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValueSchema {
    pub name: JsString,
    pub ty: JsString,
    /// Numeric value (as a property key) to enum name.
    pub enum_data: Option<JsObject<JsString>>,
    pub bitfield_width: Option<f64>,
    pub array_length: Option<f64>,
    pub bit_range: [f64; 2],
}

/// `toSerialized()` / `fromSerialized()` data. `Log.mergeWith` merges two of these
/// with object spread: [`JsObject::extend`] keeps an existing key's position and
/// takes the later value, as spread does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedStructDecoder {
    pub schema_strings: JsObject<JsString>,
    pub schemas: JsObject<Schema>,
}

impl ToJs for ValueSchema {
    fn to_js(&self) -> JsValue {
        asrs_js::js_object! {
            "name" => self.name,
            "type" => self.ty,
            "enum" => self.enum_data.as_ref().map_or(JsValue::Null, ToJs::to_js),
            "bitfieldWidth" => self.bitfield_width.map_or(JsValue::Null, JsValue::Number),
            "arrayLength" => self.array_length.map_or(JsValue::Null, JsValue::Number),
            "bitRange" => self.bit_range,
        }
    }
}

impl ToJs for Schema {
    fn to_js(&self) -> JsValue {
        asrs_js::js_object! {
            "length" => self.length,
            "valueSchemas" => self.value_schemas,
        }
    }
}

impl ToJs for SerializedStructDecoder {
    fn to_js(&self) -> JsValue {
        asrs_js::js_object! {
            "schemaStrings" => self.schema_strings,
            "schemas" => self.schemas,
        }
    }
}

/// Why `decode`/`decodeArray` did not return a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// `Error: Schema not defined`.
    SchemaNotDefined,
    /// The name resolved to an `Object.prototype` member, whose `valueSchemas` is
    /// `undefined`: `TypeError: Cannot read properties of undefined (reading 'length')`.
    NotASchema,
    /// Float drift in a field's bit range sliced more bytes than the type holds, and
    /// `paddedValue.set` threw `RangeError: offset is out of bounds`.
    OffsetOutOfBounds,
    /// A primitive array field has more items than a JS array can hold, so its
    /// `push` threw `RangeError: Invalid array length`. Items past the end of the
    /// data are cheap (small integers, booleans, one-character strings), so the
    /// heap holds out until then; the port does not model heap exhaustion.
    InvalidArrayLength,
    /// Upstream never returns. `decodeArray` runs on past [`MAX_ARRAY_LENGTH`]
    /// items (a zero-length schema decoded as an array, where `value.length / 0`
    /// is `Infinity`, or an infinite or huge nested array length), and each item
    /// is an object plus a `schemaTypes` entry, at least 36 bytes, so the 4 GiB V8
    /// heap is exhausted before the array limit is reached. That out-of-memory
    /// error is fatal to the realm and no `try`/`catch` sees it: `Log.putStruct`
    /// neither stores nor queues anything, and the worker or hub renderer running
    /// it crashes after minutes of allocating (measured: about 4 minutes and
    /// 3.9 GB for the zero-length case).
    NeverCompletes,
}

/// `decode`/`decodeArray`'s return: the decoded object (or array for
/// `decodeArray`) plus the schema-type map `Log.putStruct` turns into `Empty`
/// fields. Both iterate in JS property order.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub data: JsValue,
    pub schema_types: JsObject<JsString>,
}

#[derive(Clone, Debug, Default)]
pub struct StructDecoder {
    schema_strings: JsObject<JsString>,
    schemas: JsObject<Schema>,
}

enum SchemaRef<'a> {
    Own(&'a Schema),
    Inherited(Inherited),
}

impl StructDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `schemas[name]` if `name in schemas`.
    fn schema_ref(&self, name: &JsString) -> Option<SchemaRef<'_>> {
        match self.schemas.get_key(name) {
            Some(schema) => Some(SchemaRef::Own(schema)),
            None => object_proto_member(name).map(SchemaRef::Inherited),
        }
    }

    pub fn add_schema(&mut self, name: &JsString, schema: &[u8]) {
        if js_in(&self.schema_strings, name) {
            return;
        }
        let text = text_decoder()
            .decode(schema)
            .unwrap_or_else(|_| JsString::new());
        self.schema_strings.insert(name.clone(), text);
        loop {
            let pending: Vec<(JsString, JsString)> = self
                .schema_strings
                .iter()
                .filter(|(key, _)| !js_in(&self.schemas, key))
                .map(|(key, text)| (key.clone(), text.clone()))
                .collect();
            let mut progress = false;
            for (key, text) in pending {
                if !js_in(&self.schemas, &key) {
                    progress |= self.compile_schema(&key, &text);
                }
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
            let mut enum_data = None;
            if schema_str.starts_with(&js_string("enum"), None) {
                let mut map = JsObject::new();
                let enum_str_start = schema_str.index_of(&js_string("{"), None) as f64 + 1.0;
                let enum_str_end = schema_str.index_of(&js_string("}"), None) as f64;
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
            if !is_valid_type(&ty) && !js_in(&self.schemas, &ty) {
                return false;
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

            value_schemas.push(ValueSchema {
                name: field_name,
                ty,
                enum_data,
                bitfield_width,
                array_length,
                bit_range: [0.0, 0.0],
            });
        }

        let mut bit_position = 0.0_f64;
        let mut bitfield: Option<(f64, f64)> = None;
        for value_schema in &mut value_schemas {
            let ty = &value_schema.ty;
            if !is_valid_type(ty) {
                if let Some((position, length)) = bitfield.take() {
                    bit_position += length - position;
                }
                let mut length = match self.schema_ref(ty) {
                    Some(SchemaRef::Own(schema)) => schema.length,
                    Some(SchemaRef::Inherited(Inherited::Function(arity))) => arity,
                    Some(SchemaRef::Inherited(Inherited::Prototype)) | None => f64::NAN,
                };
                if let Some(array_length) = value_schema.array_length {
                    length *= array_length;
                }
                value_schema.bit_range = [bit_position, bit_position + length];
                bit_position += length;
            } else if let Some(width) = value_schema.bitfield_width {
                let type_length = type_max_bits(ty);
                let value_bit_length = math::min(&[width, type_length]);
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
                value_schema.bit_range = [bit_position, bit_position + value_bit_length];
                *position += value_bit_length;
                bit_position += value_bit_length;
            } else {
                if let Some((position, length)) = bitfield.take() {
                    bit_position += length - position;
                }
                let mut bit_length = type_max_bits(ty);
                if let Some(array_length) = value_schema.array_length {
                    bit_length *= array_length;
                }
                value_schema.bit_range = [bit_position, bit_position + bit_length];
                bit_position += bit_length;
            }
        }
        if let Some((position, length)) = bitfield {
            bit_position += length - position;
        }

        set_property(
            &mut self.schemas,
            name,
            Schema {
                length: bit_position,
                value_schemas,
            },
        );
        true
    }

    /// `decode(name, value)`. The `bitLength` parameter is dead upstream (assigned
    /// and never read), so it is not part of this signature.
    pub fn decode(&self, name: &JsString, value: &[u8]) -> Result<Decoded, DecodeError> {
        let schema = match self.schema_ref(name) {
            Some(SchemaRef::Own(schema)) => schema,
            Some(SchemaRef::Inherited(_)) => return Err(DecodeError::NotASchema),
            None => return Err(DecodeError::SchemaNotDefined),
        };
        let mut output_data = JsObject::new();
        let mut output_schema_types = JsObject::<JsString>::new();
        for value_schema in &schema.value_schemas {
            let ty = &value_schema.ty;
            let name = &value_schema.name;
            let range = value_schema.bit_range;
            let (value_array, value_bit_length) = slice_bits(value, range);
            if is_valid_type(ty) {
                let enum_data = value_schema.enum_data.as_ref();
                let Some(array_length) = value_schema.array_length else {
                    set_property(
                        &mut output_data,
                        name,
                        decode_value(&value_array, ty, enum_data)?,
                    );
                    continue;
                };
                let item_length = (range[1] - range[0]) / array_length;
                let items =
                    decode_items(&value_array, value_bit_length, item_length, ty, enum_data)?;
                let decoded = if is_char(ty) {
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
            } else {
                let mut schema_type = ty.clone();
                if value_schema.array_length.is_some() {
                    schema_type.push_str(&js_string("[]"));
                }
                set_property(&mut output_schema_types, name, schema_type);
                let child = match value_schema.array_length {
                    Some(length) => self.decode_array(ty, &value_array, Some(length))?,
                    None => self.decode(ty, &value_array)?,
                };
                set_property(&mut output_data, name, child.data);
                for (field, field_type) in child.schema_types.iter() {
                    let mut key = name.clone();
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
    ) -> Result<Decoded, DecodeError> {
        let schema_length = match self.schema_ref(name) {
            Some(SchemaRef::Own(schema)) => schema.length / 8.0,
            Some(SchemaRef::Inherited(Inherited::Function(arity))) => arity / 8.0,
            Some(SchemaRef::Inherited(Inherited::Prototype)) => f64::NAN,
            None => return Err(DecodeError::SchemaNotDefined),
        };
        let length = array_length.unwrap_or(value.len() as f64 / schema_length);
        let mut output_data = Vec::new();
        let mut output_schema_types = JsObject::<JsString>::new();
        let mut i = 0.0_f64;
        while i < length {
            if length > MAX_ARRAY_LENGTH {
                if i >= MAX_ARRAY_LENGTH {
                    return Err(DecodeError::NeverCompletes);
                }
                if rest_is_empty(i, schema_length, value.len()) {
                    self.decode(name, &[])?;
                    return Err(DecodeError::NeverCompletes);
                }
            }
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

    /// `toSerialized()`. Upstream returns its live maps, but every caller either
    /// copies them (`mergeWith`'s spread) or posts them to another realm, so a
    /// snapshot is equivalent.
    pub fn to_serialized(&self) -> SerializedStructDecoder {
        SerializedStructDecoder {
            schema_strings: self.schema_strings.clone(),
            schemas: self.schemas.clone(),
        }
    }

    pub fn from_serialized(data: SerializedStructDecoder) -> Self {
        Self {
            schema_strings: data.schema_strings,
            schemas: data.schemas,
        }
    }
}

/// `for (position = 0; position < valueBitLength; position += itemLength)` over
/// a primitive array field. Once the item slices run past the data they are
/// empty and decode identically, so those items are counted, not sliced.
fn decode_items(
    value: &[u8],
    bit_length: f64,
    item_length: f64,
    ty: &JsString,
    enum_data: Option<&JsObject<JsString>>,
) -> Result<Vec<JsValue>, DecodeError> {
    let data_bits = (value.len() * 8) as f64;
    let mut items = Vec::new();
    let mut position = 0.0_f64;
    while position < bit_length {
        if position >= data_bits || item_length <= 0.0 {
            let filler = decode_value(&[], ty, enum_data)?;
            if (bit_length - position) / item_length >= 2.0 * MAX_ARRAY_LENGTH {
                return Err(DecodeError::InvalidArrayLength);
            }
            let mut count = items.len() as f64;
            while position < bit_length {
                if count >= MAX_ARRAY_LENGTH || position + item_length == position {
                    return Err(DecodeError::InvalidArrayLength);
                }
                count += 1.0;
                position += item_length;
            }
            items.resize(count as usize, filler);
            break;
        }
        let (item, _) = slice_bits(value, [position, position + item_length]);
        let decoded = decode_value(&item, ty, enum_data)?;
        if items.len() as f64 >= MAX_ARRAY_LENGTH {
            return Err(DecodeError::InvalidArrayLength);
        }
        items.push(decoded);
        position += item_length;
    }
    Ok(items)
}

/// Whether `value.slice(j * schemaLength, (j + 1) * schemaLength)` is empty for
/// every `j >= i`, so that every remaining `decodeArray` item decodes the same way.
fn rest_is_empty(i: f64, schema_length: f64, len: usize) -> bool {
    let len = len as f64;
    if schema_length.is_nan() || schema_length == 0.0 || schema_length == f64::NEG_INFINITY {
        true
    } else if schema_length > 0.0 {
        i * schema_length >= len
    } else {
        len + (i + 1.0) * schema_length <= 0.0
    }
}

/// `output in enumData ? enumData[output] : output`. Decoded outputs are booleans,
/// numbers and strings of at most one character, none of which name an
/// `Object.prototype` member, so only own keys can match.
fn enum_lookup(enum_data: Option<&JsObject<JsString>>, output: JsValue) -> JsValue {
    let Some(map) = enum_data else {
        return output;
    };
    let key: Vec<u16> = match &output {
        JsValue::Bool(b) => if *b { "true" } else { "false" }.encode_utf16().collect(),
        JsValue::Number(x) => number::to_string(*x).encode_utf16().collect(),
        JsValue::String(s) => s.units().to_vec(),
        _ => return output,
    };
    match map.get_units(&key) {
        Some(name) => JsValue::String(name.clone()),
        None => output,
    }
}

fn decode_value(
    value: &[u8],
    ty: &JsString,
    enum_data: Option<&JsObject<JsString>>,
) -> Result<JsValue, DecodeError> {
    let width = (type_max_bits(ty) / 8.0) as usize;
    if value.len() > width {
        return Err(DecodeError::OffsetOutOfBounds);
    }
    let mut padded = [0u8; 8];
    padded[..value.len()].copy_from_slice(value);
    let bytes = |n: usize| -> [u8; 8] {
        let mut out = [0u8; 8];
        out[..n].copy_from_slice(&padded[..n]);
        out
    };
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
        JsValue::Number(f64::from(i16::from_le_bytes([padded[0], padded[1]])))
    } else if js_eq(ty, "int32") {
        JsValue::Number(f64::from(i32::from_le_bytes(
            padded[..4].try_into().expect("4 bytes"),
        )))
    } else if js_eq(ty, "int64") {
        JsValue::Number(i64::from_le_bytes(bytes(8)) as f64)
    } else if js_eq(ty, "uint8") {
        JsValue::Number(f64::from(padded[0]))
    } else if js_eq(ty, "uint16") {
        JsValue::Number(f64::from(u16::from_le_bytes([padded[0], padded[1]])))
    } else if js_eq(ty, "uint32") {
        JsValue::Number(f64::from(u32::from_le_bytes(
            padded[..4].try_into().expect("4 bytes"),
        )))
    } else if js_eq(ty, "uint64") {
        JsValue::Number(u64::from_le_bytes(bytes(8)) as f64)
    } else if js_eq(ty, "float") || js_eq(ty, "float32") {
        JsValue::Number(f64::from(f32::from_le_bytes(
            padded[..4].try_into().expect("4 bytes"),
        )))
    } else {
        JsValue::Number(f64::from_le_bytes(bytes(8)))
    };
    Ok(enum_lookup(enum_data, output))
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
    let (s, count) = slice_bounds(start, end, input.len());
    input[s..s + count].to_vec()
}

/// `sliceBits`: LSB-first within each byte.
fn slice_bits(input: &[u8], range: [f64; 2]) -> (Vec<u8>, f64) {
    if range[0] % 8.0 == 0.0 && range[1] % 8.0 == 0.0 {
        (
            js_slice(input, range[0] / 8.0, range[1] / 8.0),
            range[1] - range[0],
        )
    } else {
        let (s, count) = slice_bounds(range[0], range[1], input.len() * 8);
        let mut out = vec![0u8; count.div_ceil(8)];
        for index in 0..count {
            let src = s + index;
            if input[src / 8] >> (src % 8) & 1 != 0 {
                out[index / 8] |= 1 << (index % 8);
            }
        }
        (out, range[1] - range[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> JsString {
        js_string(text)
    }

    fn decoder(schemas: &[(&str, &str)]) -> StructDecoder {
        let mut d = StructDecoder::new();
        for (name, text) in schemas {
            d.add_schema(&s(name), text.as_bytes());
        }
        d
    }

    #[test]
    fn zero_length_schema_array_never_completes() {
        let d = decoder(&[
            ("Empty", ""),
            ("ZeroArr", "int8 a[0]"),
            ("Proto", "toString x"),
        ]);
        for name in ["Empty", "ZeroArr"] {
            assert_eq!(
                d.decode_array(&s(name), &[1], None),
                Err(DecodeError::NeverCompletes)
            );
            assert!(d.decode_array(&s(name), &[], None).is_ok());
        }
        assert_eq!(
            d.decode_array(&s("Proto"), &[1], None),
            Err(DecodeError::NotASchema)
        );
    }

    #[test]
    fn unbounded_nested_arrays_never_complete() {
        let d = decoder(&[
            ("Leaf", "double v"),
            ("InfNested", "Leaf a[Infinity]"),
            ("HugeNested", "Leaf a[1e12]"),
            ("ProtoNested", "toString a[1e12]"),
        ]);
        for name in ["InfNested", "HugeNested"] {
            assert_eq!(
                d.decode(&s(name), &[0; 16]),
                Err(DecodeError::NeverCompletes),
                "{name}"
            );
        }
        assert_eq!(
            d.decode(&s("ProtoNested"), &[0; 16]),
            Err(DecodeError::NotASchema)
        );
    }

    #[test]
    fn oversized_primitive_arrays_throw_invalid_array_length() {
        let d = decoder(&[
            ("Huge", "int8 a[1e300]"),
            ("JustOver", "bool a[134217729]"),
            ("Inf", "int8 a[Infinity]"),
            ("Big", "uint8 a[100000]"),
        ]);
        for name in ["Huge", "JustOver"] {
            assert_eq!(
                d.decode(&s(name), &[1, 2, 3]),
                Err(DecodeError::InvalidArrayLength),
                "{name}"
            );
        }
        assert!(d.decode(&s("Inf"), &[0; 16]).is_ok());
        let big = d.decode(&s("Big"), &[7, 8]).expect("finite array decodes");
        let items = big
            .data
            .get("a")
            .and_then(JsValue::as_array)
            .expect("array");
        assert_eq!(items.len(), 100_000);
        assert_eq!(items[..3], [7.0, 8.0, 0.0].map(JsValue::Number));
    }
}
