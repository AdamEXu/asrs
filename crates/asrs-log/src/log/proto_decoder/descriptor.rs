// Ported from protobufjs 8.3.0 (BSD-3-Clause, Copyright (c) 2016 Daniel Wirtz). See THIRD_PARTY_NOTICES.md.

//! `protobufjs/ext/descriptor`: decoding a `FileDescriptorProto` and the `fromDescriptor`
//! builders `ProtoDecoder.addDescriptor` calls.

use asrs_js::number::{self, parse_int};
use asrs_js::string::JsString;
use asrs_js::value::JsObject;

use super::ProtoRealm;
use super::descriptor_schema::{FILE_DESCRIPTOR_PROTO, TYPES};
use super::error::ProtoError;
use super::reflect::{
    EnumData, Field, FieldId, Kind, Member, NodeId, OneofId, ROOT, Reflection, Reserved, TypeData,
    is_object_prototype_name, js, js_eq,
};
use super::value::{JsDefault, Long, Message, Val};
use super::wire::Reader;

pub(super) enum SchemaKind {
    Basic(&'static str),
    Message(usize),
    Enum,
}

pub(super) enum SchemaDefault {
    None,
    Number(f64),
    Bool(bool),
    Str(&'static str),
    Long,
    Bytes,
}

pub(super) struct SchemaField {
    pub name: &'static str,
    pub id: u32,
    pub kind: SchemaKind,
    pub repeated: bool,
    pub required: bool,
    pub default: SchemaDefault,
}

/// The descriptor.proto types as a resolved, set-up reflection tree.
#[derive(Clone, Debug)]
pub(super) struct DescriptorSchema {
    refl: Reflection,
    types: Vec<NodeId>,
}

impl DescriptorSchema {
    pub fn new() -> Self {
        let mut refl = Reflection::new();
        let types: Vec<NodeId> = (0..TYPES.len())
            .map(|i| refl.new_node(js(&format!("T{i}")), Kind::Type(TypeData::default())))
            .collect();
        let enum_node = refl.new_node(js("E"), Kind::Enum(EnumData::default()));
        for (i, fields) in TYPES.iter().enumerate() {
            let ty = types[i];
            refl.nodes[ty].parent = Some(ROOT);
            let mut ids = Vec::new();
            for spec in fields.iter() {
                let basic = match spec.kind {
                    SchemaKind::Basic(b) => b,
                    _ => "message",
                };
                let mut field = Field::new(js(spec.name), f64::from(spec.id), js(basic));
                field.repeated = spec.repeated;
                field.required = spec.required;
                field.parent = Some(ty);
                field.resolved = true;
                field.resolved_type = match spec.kind {
                    SchemaKind::Message(t) => Some(types[t]),
                    SchemaKind::Enum => Some(enum_node),
                    SchemaKind::Basic(_) => None,
                };
                field.type_default = match spec.default {
                    SchemaDefault::None => JsDefault::Null,
                    SchemaDefault::Number(x) => JsDefault::Number(x),
                    SchemaDefault::Bool(b) => JsDefault::Bool(b),
                    SchemaDefault::Str(s) => JsDefault::String(js(s)),
                    SchemaDefault::Long => {
                        JsDefault::Long(Long::from_bits(0, 0, js_eq(&field.ty, "uint64")))
                    }
                    SchemaDefault::Bytes => JsDefault::EmptyArray,
                };
                field.default_value = if spec.repeated {
                    JsDefault::EmptyArray
                } else {
                    field.type_default.clone()
                };
                ids.push((js(spec.name), refl.new_field(field)));
            }
            let ctor: Vec<FieldId> = ids
                .iter()
                .map(|(_, f)| *f)
                .filter(|&f| refl.fields[f].repeated)
                .collect();
            let data = refl.type_data_mut(ty);
            data.fields = ids.into_iter().collect();
            data.ctor = Some(ctor);
            data.decode_ready = true;
        }
        DescriptorSchema { refl, types }
    }

    /// `descriptor.FileDescriptorProto.decode(data)`.
    pub fn decode_file(&mut self, data: &[u8]) -> Result<Message, ProtoError> {
        let mut reader = Reader::new(data);
        self.refl.decode(
            self.types[FILE_DESCRIPTOR_PROTO],
            &mut reader,
            None,
            0,
            None,
        )
    }

    fn field_default(&self, m: &Message, name: &str) -> JsDefault {
        self.refl
            .type_data(m.ty)
            .fields
            .get(name)
            .map_or(JsDefault::Undefined, |&f| {
                self.refl.fields[f].default_value.clone()
            })
    }
}

/// Reads decoded descriptor messages the way the builders do: own properties first, then the
/// generated prototype's defaults.
struct Desc<'a> {
    schema: &'a DescriptorSchema,
}

impl Desc<'_> {
    fn str(&self, m: &Message, name: &str) -> JsString {
        match m.own.get(name) {
            Some(Val::Str(s)) => s.clone(),
            _ => match self.schema.field_default(m, name) {
                JsDefault::String(s) => s,
                _ => JsString::new(),
            },
        }
    }

    fn num(&self, m: &Message, name: &str) -> f64 {
        match m.own.get(name) {
            Some(Val::Num(x)) => *x,
            _ => match self.schema.field_default(m, name) {
                JsDefault::Number(x) => x,
                _ => 0.0,
            },
        }
    }

    fn bool(&self, m: &Message, name: &str) -> bool {
        matches!(m.own.get(name), Some(Val::Bool(true)))
    }

    fn list<'m>(&self, m: &'m Message, name: &str) -> Vec<&'m Message> {
        match m.own.get(name) {
            Some(Val::Array(items)) => items
                .iter()
                .filter_map(|v| match v {
                    Val::Msg(msg) => Some(msg.as_ref()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn strings(&self, m: &Message, name: &str) -> Vec<JsString> {
        match m.own.get(name) {
            Some(Val::Array(items)) => items
                .iter()
                .filter_map(|v| match v {
                    Val::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn msg<'m>(&self, m: &'m Message, name: &str) -> Option<&'m Message> {
        match m.own.get(name) {
            Some(Val::Msg(msg)) => Some(msg),
            _ => None,
        }
    }
}

fn type_ref_ok(name: &JsString) -> bool {
    let ident_start = |c: u16| u8::try_from(c).is_ok_and(|c| c.is_ascii_alphabetic() || c == b'_');
    let ident_part = |c: u16| u8::try_from(c).is_ok_and(|c| c.is_ascii_alphanumeric() || c == b'_');
    let u = name.units();
    let body = match u.first() {
        Some(&c) if c == u16::from(b'.') => &u[1..],
        _ => u,
    };
    !body.is_empty()
        && body.split(|&c| c == u16::from(b'.')).all(|segment| {
            segment.first().is_some_and(|&c| ident_start(c))
                && segment.iter().all(|&c| ident_part(c))
        })
}

fn from_descriptor_type(ty: f64) -> Result<&'static str, ProtoError> {
    Ok(match ty as i64 {
        1 => "double",
        2 => "float",
        3 => "int64",
        4 => "uint64",
        5 => "int32",
        6 => "fixed64",
        7 => "fixed32",
        8 => "bool",
        9 => "string",
        12 => "bytes",
        13 => "uint32",
        15 => "sfixed32",
        16 => "sfixed64",
        17 => "sint32",
        18 => "sint64",
        _ => return Err(ProtoError::IllegalType(ty)),
    })
}

fn numbered(prefix: &str, n: f64) -> JsString {
    js(&format!("{prefix}{}", number::to_string(n)))
}

fn counted(prefix: &str, counter: &mut u64) -> JsString {
    let name = js(&format!("{prefix}{counter}"));
    *counter += 1;
    name
}

/// `/^(?![eE])[0-9]*(?:\.[0-9]*)?(?:[eE][+-]?[0-9]+)?$/`.
fn is_number_literal(u: &[u16]) -> bool {
    let c = |i: usize| u.get(i).copied().and_then(|c| u8::try_from(c).ok());
    if matches!(c(0), Some(b'e' | b'E')) {
        return false;
    }
    let mut i = 0;
    while c(i).is_some_and(|c| c.is_ascii_digit()) {
        i += 1;
    }
    if c(i) == Some(b'.') {
        i += 1;
        while c(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
    }
    if matches!(c(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(c(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let start = j;
        while c(j).is_some_and(|c| c.is_ascii_digit()) {
            j += 1;
        }
        if j == start {
            return false;
        }
        i = j;
    }
    i == u.len()
}

fn parse_default(text: JsString) -> JsDefault {
    for (lit, value) in [
        ("true", true),
        ("TRUE", true),
        ("false", false),
        ("FALSE", false),
    ] {
        if js_eq(&text, lit) {
            return JsDefault::Bool(value);
        }
    }
    if is_number_literal(text.units()) {
        return JsDefault::Number(parse_int(text.units(), 0.0));
    }
    JsDefault::String(text)
}

pub(super) struct Builder<'a> {
    pub refl: &'a mut Reflection,
    pub realm: &'a mut ProtoRealm,
    pub schema: &'a DescriptorSchema,
}

/// What a nested type's descriptor name maps to in `Type.fromDescriptor`'s `mapEntries`.
enum MapEntry<'m> {
    Entry(&'m Message),
    /// An `Object.prototype` member: its `field` is undefined and its `name` is this.
    Prototype(JsString),
}

impl<'a> Builder<'a> {
    fn d(&self) -> Desc<'a> {
        Desc {
            schema: self.schema,
        }
    }

    /// `Field.fromDescriptor(descriptor)`.
    pub fn field(&mut self, d: &Message) -> Result<FieldId, ProtoError> {
        let desc = self.d();
        let number = desc.num(d, "number");
        let type_name = desc.str(d, "typeName");
        let ty = if !type_name.is_empty() {
            if !type_ref_ok(&type_name) {
                return Err(ProtoError::IllegalTypeName(type_name));
            }
            type_name
        } else {
            js(from_descriptor_type(desc.num(d, "type"))?)
        };
        let label = desc.num(d, "label");
        let rule = match label as i64 {
            1 if label == 1.0 => None,
            2 if label == 2.0 => Some(js("required")),
            3 if label == 3.0 => Some(js("repeated")),
            _ => return Err(ProtoError::IllegalLabel(label)),
        };
        let extendee = desc.str(d, "extendee");
        let extend = if extendee.is_empty() {
            None
        } else if !type_ref_ok(&extendee) {
            return Err(ProtoError::IllegalTypeName(extendee));
        } else {
            Some(extendee)
        };
        let name = desc.str(d, "name");
        let name = if name.is_empty() {
            numbered("field", number)
        } else {
            name
        };
        if number < 0.0 {
            return Err(ProtoError::NegativeFieldId);
        }
        let mut field = Field::new(name, number, ty);
        field.repeated = rule.as_ref().is_some_and(|r| js_eq(r, "repeated"));
        field.extend = extend;
        field.proto3_optional = desc.bool(d, "proto3Optional");
        let default = desc.str(d, "defaultValue");
        if !default.is_empty() {
            field.default = Some(parse_default(default));
        }
        Ok(self.refl.new_field(field))
    }

    fn map_field(&mut self, d: &Message, entry: MapEntry<'_>) -> Result<FieldId, ProtoError> {
        let entry = match entry {
            MapEntry::Entry(e) => e,
            MapEntry::Prototype(name) => return Err(ProtoError::InvalidMapEntry(name)),
        };
        let desc = self.d();
        let entry_field = |n: f64| {
            desc.list(entry, "field")
                .into_iter()
                .find(|f| desc.num(f, "number") == n)
                .ok_or_else(|| ProtoError::InvalidMapEntry(desc.str(entry, "name")))
        };
        let key = entry_field(1.0)?;
        let value = entry_field(2.0)?;
        let value_name = desc.str(value, "typeName");
        let value_ty = if value_name.is_empty() {
            js(from_descriptor_type(desc.num(value, "type"))?)
        } else {
            value_name
        };
        let key_ty = js(from_descriptor_type(desc.num(key, "type"))?);
        let number = desc.num(d, "number");
        let name = desc.str(d, "name");
        let name = if name.is_empty() {
            numbered("field", number)
        } else {
            name
        };
        if number < 0.0 {
            return Err(ProtoError::NegativeFieldId);
        }
        let mut field = Field::new(name, number, value_ty);
        field.map_key = Some(key_ty);
        Ok(self.refl.new_field(field))
    }

    fn type_(&mut self, d: &Message, depth: u32) -> Result<NodeId, ProtoError> {
        if depth > 32 {
            return Err(ProtoError::MaxDepth);
        }
        let desc = self.d();
        let raw_name = desc.str(d, "name");
        let name = if raw_name.is_empty() {
            counted("Type", &mut self.realm.types)
        } else {
            raw_name
        };
        let name = JsString::from_units(
            name.units()
                .iter()
                .copied()
                .filter(|&c| u8::try_from(c).is_ok_and(|c| c.is_ascii_alphanumeric() || c == b'_'))
                .collect(),
        );
        let ty = self.refl.new_node(name, Kind::Type(TypeData::default()));
        let desc = self.d();
        let nested_types = desc.list(d, "nestedType");
        let mut map_entries: JsObject<usize> = JsObject::new();
        let mut proto_replaced = false;
        for (i, nt) in nested_types.iter().enumerate() {
            if desc
                .msg(nt, "options")
                .is_some_and(|o| desc.bool(o, "mapEntry"))
            {
                let key = desc.str(nt, "name");
                if js_eq(&key, "__proto__") {
                    proto_replaced = true;
                } else {
                    map_entries.insert(key, i);
                }
            }
        }
        for od in desc.list(d, "oneofDecl") {
            let o = self.oneof(od);
            self.refl.add(ty, Member::OneOf(o))?;
        }
        for fd in desc.list(d, "field") {
            let type_name = desc.str(fd, "typeName");
            let entry_name = {
                let u = type_name.units();
                let start = u
                    .iter()
                    .rposition(|&c| c == u16::from(b'.'))
                    .map_or(0, |p| p + 1);
                JsString::from_units(u[start..].to_vec())
            };
            let is_map_candidate = desc.num(fd, "label") == 3.0
                && desc.num(fd, "type") == 11.0
                && !entry_name.is_empty();
            let entry = if !is_map_candidate {
                None
            } else if let Some(&i) = map_entries.get_key(&entry_name) {
                Some(MapEntry::Entry(nested_types[i]))
            } else if proto_replaced {
                return Err(ProtoError::Unmodeled(
                    "a map entry type named __proto__ replaces the mapEntries prototype",
                ));
            } else if js_eq(&entry_name, "__proto__") {
                Some(MapEntry::Prototype(js("undefined")))
            } else if is_object_prototype_name(&entry_name) {
                Some(MapEntry::Prototype(if js_eq(&entry_name, "constructor") {
                    js("Object")
                } else {
                    entry_name
                }))
            } else {
                None
            };
            let f = match entry {
                Some(entry) => self.map_field(fd, entry)?,
                None => self.field(fd)?,
            };
            self.refl.add(ty, Member::Field(f))?;
            if fd.own.get("oneofIndex").is_some() {
                let index = desc.num(fd, "oneofIndex");
                let oneofs: Vec<OneofId> = self
                    .refl
                    .type_data(ty)
                    .oneofs
                    .as_ref()
                    .map(|o| o.values().copied().collect())
                    .unwrap_or_default();
                let o = (index >= 0.0)
                    .then(|| oneofs.get(index as usize).copied())
                    .flatten()
                    .ok_or(ProtoError::ReadUndefined("add"))?;
                self.refl.oneof_add(o, f)?;
            }
        }
        for ed in desc.list(d, "extension") {
            let f = self.field(ed)?;
            self.refl.add(ty, Member::Field(f))?;
        }
        for nt in &nested_types {
            if desc
                .msg(nt, "options")
                .is_some_and(|o| desc.bool(o, "mapEntry"))
            {
                continue;
            }
            let child = self.type_(nt, depth + 1)?;
            self.refl.add(ty, Member::Node(child))?;
        }
        for ed in desc.list(d, "enumType") {
            let e = self.enum_(ed);
            self.refl.add(ty, Member::Node(e))?;
        }
        let ranges = desc.list(d, "reservedRange");
        let names = desc.strings(d, "reservedName");
        if !ranges.is_empty() || !names.is_empty() {
            let mut reserved: Vec<Reserved> = ranges
                .iter()
                .map(|r| Reserved::Range(desc.num(r, "start"), desc.num(r, "end")))
                .collect();
            reserved.extend(names.into_iter().map(Reserved::Name));
            self.refl.type_data_mut(ty).reserved = Some(reserved);
        }
        Ok(ty)
    }

    pub fn message_type(&mut self, d: &Message) -> Result<NodeId, ProtoError> {
        self.type_(d, 0)
    }

    fn oneof(&mut self, d: &Message) -> OneofId {
        let name = self.d().str(d, "name");
        let name = if name.is_empty() {
            counted("oneof", &mut self.realm.oneofs)
        } else {
            name
        };
        self.refl.new_oneof(name)
    }

    pub fn enum_(&mut self, d: &Message) -> NodeId {
        let desc = self.d();
        let mut values: JsObject<f64> = JsObject::new();
        for v in desc.list(d, "value") {
            let number = desc.num(v, "number");
            let number = if number == 0.0 || number.is_nan() {
                0.0
            } else {
                number
            };
            let name = desc.str(v, "name");
            let name = if name.is_empty() {
                numbered("NAME", number)
            } else {
                name
            };
            if !js_eq(&name, "__proto__") {
                values.insert(name, number);
            }
        }
        let name = desc.str(d, "name");
        let name = if name.is_empty() {
            counted("Enum", &mut self.realm.enums)
        } else {
            name
        };
        let mut data = EnumData::default();
        for (key, &value) in values.iter() {
            data.values_by_id
                .insert(js(&number::to_string(value)), key.clone());
            data.values.insert(key.clone(), value);
        }
        self.refl.new_node(name, Kind::Enum(data))
    }

    pub fn service(&mut self, d: &Message) -> Result<NodeId, ProtoError> {
        let desc = self.d();
        let name = desc.str(d, "name");
        let name = if name.is_empty() {
            counted("Service", &mut self.realm.services)
        } else {
            name
        };
        let service = self.refl.new_node(name, Kind::Service(JsObject::new()));
        for md in desc.list(d, "method") {
            for key in ["inputType", "outputType"] {
                let t = desc.str(md, key);
                if !t.is_empty() && !type_ref_ok(&t) {
                    return Err(ProtoError::IllegalTypeName(t));
                }
            }
            let name = desc.str(md, "name");
            let name = if name.is_empty() {
                counted("Method", &mut self.realm.methods)
            } else {
                name
            };
            self.refl.service_add_method(service, name)?;
        }
        Ok(service)
    }
}
