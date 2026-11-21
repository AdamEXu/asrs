// Ported from protobufjs 8.3.0 (BSD-3-Clause, Copyright (c) 2016 Daniel Wirtz). See THIRD_PARTY_NOTICES.md.

//! `shared/log/ProtoDecoder.ts` together with the parts of protobufjs 8.3.0 it relies on:
//! `ext/descriptor` building a reflection tree from `FileDescriptorProto` payloads, the
//! generated decoder, and `toObject(message, {defaults: true})`.
//!
//! protobufjs never resolves editions features on this root (nothing calls `resolveAll`), so
//! every singular field has explicit presence, no field is required, and groups decode as
//! length-delimited messages. 64-bit integers stay `Long` instances, which reach the log as
//! plain `{low, high, unsigned}` objects.
//!
//! Anonymous descriptor names (`Type0`, `Enum0`, `oneof0`, `Service0`, `Method0`) come from
//! counters that are global to a JS realm, so callers keep one [`ProtoRealm`] per upstream
//! realm (hub renderer, each worker) and share it between every decoder in that realm.

mod descriptor;
mod descriptor_schema;
mod error;
mod reflect;
mod value;
mod wire;

use asrs_js::number::to_int32;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};

pub use super::struct_decoder::Decoded;
pub use error::{ProtoError, SyntaxKind};

use descriptor::{Builder, DescriptorSchema};
use reflect::{Filter, Member, ROOT, Reflection, js, js_eq};
use wire::Reader;

/// protobufjs's realm-global counters for unnamed descriptor objects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProtoRealm {
    types: u64,
    enums: u64,
    oneofs: u64,
    services: u64,
    methods: u64,
}

impl ProtoRealm {
    pub fn new() -> Self {
        Self::default()
    }
}

/// `ProtoDecoder#toSerialized()`: the raw descriptors in the order they were accepted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SerializedProtoDecoder(pub Vec<Vec<u8>>);

impl ToJs for SerializedProtoDecoder {
    fn to_js(&self) -> JsValue {
        JsValue::Array(self.0.iter().map(|d| JsValue::Bytes(d.clone())).collect())
    }
}

#[derive(Clone, Debug)]
pub struct ProtoDecoder {
    root: Reflection,
    schema: DescriptorSchema,
    raw_descriptors: Vec<Vec<u8>>,
}

impl Default for ProtoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

const CONTROL_WORD_ROBOT_MODES: [&str; 4] = ["Unknown", "Autonomous", "Teleoperated", "Utility"];
const CONTROL_WORD_ALLIANCE_STATIONS: [&str; 6] =
    ["Red 1", "Red 2", "Red 3", "Blue 1", "Blue 2", "Blue 3"];

impl ProtoDecoder {
    pub fn new() -> Self {
        ProtoDecoder {
            root: Reflection::new(),
            schema: DescriptorSchema::new(),
            raw_descriptors: Vec::new(),
        }
    }

    /// `addDescriptor(data)`. On an error the decoder keeps whatever upstream had built before
    /// throwing, including the raw descriptor once its file name was accepted.
    pub fn add_descriptor(
        &mut self,
        realm: &mut ProtoRealm,
        data: &[u8],
    ) -> Result<(), ProtoError> {
        let file = self.schema.decode_file(data)?;
        let name = match file.own.get("name") {
            Some(value::Val::Str(s)) => s.clone(),
            _ => JsString::new(),
        };
        if self.root.files.contains(&name) {
            return Ok(());
        }
        self.raw_descriptors.push(data.to_vec());
        let package = match file.own.get("package") {
            Some(value::Val::Str(s)) => s.clone(),
            _ => JsString::new(),
        };
        let file_package = if package.is_empty() {
            ROOT
        } else {
            self.root.define(&package)?
        };
        if !name.is_empty() {
            self.root.files.push(name);
        }
        let list = |key: &str| -> Vec<value::Message> {
            match file.own.get(key) {
                Some(value::Val::Array(items)) => items
                    .iter()
                    .filter_map(|v| match v {
                        value::Val::Msg(m) => Some((**m).clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let mut builder = Builder {
            refl: &mut self.root,
            realm,
            schema: &self.schema,
        };
        for d in list("messageType") {
            let ty = builder.message_type(&d)?;
            builder.refl.add(file_package, Member::Node(ty))?;
        }
        for d in list("enumType") {
            let e = builder.enum_(&d);
            builder.refl.add(file_package, Member::Node(e))?;
        }
        for d in list("extension") {
            let f = builder.field(&d)?;
            builder.refl.add(file_package, Member::Field(f))?;
        }
        for d in list("service") {
            let s = builder.service(&d)?;
            builder.refl.add(file_package, Member::Node(s))?;
        }
        Ok(())
    }

    /// `decode(name, data)`: the decoded object (`toObject` with defaults, after the
    /// ControlWord post-processor) and the schema types of its message-valued children.
    pub fn decode(&mut self, name: &JsString, data: &[u8]) -> Result<Decoded, ProtoError> {
        let ty = match self.root.lookup(ROOT, name, Filter::Type) {
            None => return Err(ProtoError::NoSuchType(name.clone())),
            Some(Member::Node(n)) if self.root.type_kind_is_root(n) => {
                return Err(ProtoError::DecodeNotAFunction);
            }
            Some(Member::Node(n)) => n,
            Some(_) => unreachable!("lookupType only returns types"),
        };
        let mut reader = Reader::new(data);
        let message = self.root.decode(ty, &mut reader, None, 0, None)?;
        let mut schema_types = self.root.schema_types(&message);
        let mut data = self.root.message_to_object(ty, &message)?;
        self.apply_post_processors(ty, &mut data, &mut schema_types, &JsString::new(), 0);
        Ok(Decoded { data, schema_types })
    }

    fn apply_post_processors(
        &self,
        ty: reflect::NodeId,
        data: &mut JsValue,
        schema_types: &mut JsObject<JsString>,
        prefix: &JsString,
        depth: usize,
    ) {
        // A self-referential schema lets wire input nest arbitrarily deep; upstream's
        // recursion throws `RangeError` and the decode fails, so the port stops descending.
        if depth > 2000 {
            return;
        }
        let full = self.root.full_name(Member::Node(ty));
        let full = match full.units().first() {
            Some(&c) if c == u16::from(b'.') => JsString::from_units(full.units()[1..].to_vec()),
            _ => full,
        };
        if js_eq(&full, "mrc.proto.ProtobufControlData") {
            post_process_control_data(data, schema_types, prefix);
        }
        for f in self.root.fields_array(ty) {
            let field = &self.root.fields[f];
            let Some(field_type) = field.resolved_type.filter(|&t| self.root.is_type(t)) else {
                continue;
            };
            let JsValue::Object(object) = data else {
                continue;
            };
            let Some(value) = object.get_units_mut(field.name.units()) else {
                continue;
            };
            let child = if prefix.is_empty() {
                field.name.clone()
            } else {
                child_key(prefix, &field.name)
            };
            match value {
                JsValue::Array(items) => {
                    for (i, item) in items.iter_mut().enumerate() {
                        if matches!(
                            item,
                            JsValue::Object(_) | JsValue::Array(_) | JsValue::Bytes(_)
                        ) {
                            let item_prefix = child_key(&child, &js(&format!("{i}")));
                            self.apply_post_processors(
                                field_type,
                                item,
                                schema_types,
                                &item_prefix,
                                depth + 1,
                            );
                        }
                    }
                }
                JsValue::Object(_) => {
                    self.apply_post_processors(field_type, value, schema_types, &child, depth + 1);
                }
                _ => {}
            }
        }
    }

    pub fn to_serialized(&self) -> SerializedProtoDecoder {
        SerializedProtoDecoder(self.raw_descriptors.clone())
    }

    /// `ProtoDecoder.fromSerialized(data)`, which throws on the first descriptor that throws.
    pub fn from_serialized(
        realm: &mut ProtoRealm,
        data: &SerializedProtoDecoder,
    ) -> Result<Self, ProtoError> {
        let mut decoder = ProtoDecoder::new();
        for descriptor in &data.0 {
            decoder.add_descriptor(realm, descriptor)?;
        }
        Ok(decoder)
    }
}

fn child_key(key: &JsString, child: &JsString) -> JsString {
    let mut out = key.clone();
    out.push_unit(u16::from(b'/'));
    out.push_str(child);
    out
}

/// `ProtoDecoder.getFriendlySchemaType`: the last dotted segment without a `Protobuf` prefix.
pub fn friendly_schema_type(schema_type: &JsString) -> JsString {
    let dot = js(".");
    let mut result = schema_type.clone();
    if schema_type.includes(&dot, None) {
        result = schema_type
            .split(Some(&dot), None)
            .pop()
            .unwrap_or_default();
    }
    let prefix = js("Protobuf");
    if result.starts_with(&prefix, None) {
        result = result.slice(prefix.len() as f64, None);
    }
    result
}

fn post_process_control_data(
    data: &mut JsValue,
    schema_types: &mut JsObject<JsString>,
    prefix: &JsString,
) {
    let JsValue::Object(object) = data else {
        return;
    };
    let Some(JsValue::Number(word)) = object.get("ControlWord") else {
        return;
    };
    let word = to_int32(*word);
    let bit = |shift: i32| (word >> shift) & 1 != 0;
    let station = CONTROL_WORD_ALLIANCE_STATIONS
        .get(((word >> 8) & 15) as usize)
        .copied()
        .unwrap_or("Unknown");
    let mut expanded = JsObject::new();
    expanded.insert("enabled", JsValue::Bool(word & 1 != 0));
    expanded.insert(
        "robotMode",
        JsValue::string(CONTROL_WORD_ROBOT_MODES[((word >> 1) & 3) as usize]),
    );
    expanded.insert("eStop", JsValue::Bool(bit(3)));
    expanded.insert("fmsConnected", JsValue::Bool(bit(4)));
    expanded.insert("dsConnected", JsValue::Bool(bit(5)));
    expanded.insert("allianceStation", JsValue::string(station));
    expanded.insert("timedMatch", JsValue::Bool(bit(13)));
    object.insert("ControlWord", JsValue::Object(expanded));
    let key = if prefix.is_empty() {
        js("ControlWord")
    } else {
        child_key(prefix, &js("ControlWord"))
    };
    schema_types.insert(key, js("ControlWord"));
}
