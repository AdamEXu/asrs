//! `shared/log/Log.ts`: the structured writers (`putStruct`, `putPhotonStruct`, `putProto`,
//! `putMsgpack`) with their retry queues, `getUnit` and `mergeWith`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use asrs_js::json;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;

use super::LoggableType;
use super::log_core::{Log, child_key_js};
use super::log_field::LogField;
use super::log_key_utils::apply_key_prefix;
use super::log_value_sets::RawValue;
use super::merge_calculator::get_merge_offset;
use super::proto_decoder::{
    ProtoDecoder, ProtoError, SerializedProtoDecoder, friendly_schema_type,
};
use super::struct_decoder::{DecodeError, StructDecoder};
use crate::msgpack;
use crate::realm::JsRealm;
use crate::units;

const FILE_DESCRIPTOR_PROTO: &str = "FileDescriptorProto";
const UNIT_METADATA_KEYS: [&str; 4] = ["unit", "units", "Unit", "Units"];
const BIG_GEOMETRY: [&str; 6] = [
    "Translation2d",
    "Pose2d",
    "Transform2d",
    "Translation3d",
    "Pose3d",
    "Transform3d",
];
const BIG_GEOMETRY_BYTES: usize = 5000;

/// One entry of `queuedStructs`, `queuedStructArrays` or `queuedProtos`.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedStructure {
    pub key: JsString,
    pub timestamp: f64,
    pub value: RawValue,
    pub schema_type: JsString,
}

/// How a `Log` operation ends without the result upstream reaches.
#[derive(Clone, Debug, PartialEq)]
pub enum LogError {
    /// A struct array decode exhausts the realm's heap, which kills the realm: upstream never
    /// returns from the call.
    NeverCompletes,
    /// The proto decoder threw out of the call: `addDescriptor` in `putProto`, or
    /// `ProtoDecoder.fromSerialized` in `fromSerialized` and `mergeWith`.
    Proto(ProtoError),
    /// Upstream continues in a way the port does not model; see the text.
    Unmodeled(&'static str),
}

#[derive(Debug, Default)]
pub(super) struct FieldUnitCache(Mutex<HashMap<JsString, Option<&'static str>>>);

impl FieldUnitCache {
    fn lock(&self) -> MutexGuard<'_, HashMap<JsString, Option<&'static str>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn remove(&mut self, key: &JsString) {
        self.0
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }
}

impl Clone for FieldUnitCache {
    fn clone(&self) -> Self {
        FieldUnitCache(Mutex::new(self.lock().clone()))
    }
}

fn js(text: &str) -> JsString {
    JsString::from(text)
}

impl Log {
    /// `attemptQueuedStructures`: empties the three queues, then retries every entry in queue
    /// order. Entries that still fail are queued again by the retry itself.
    pub(super) fn attempt_queued_structures(&mut self) -> Result<(), LogError> {
        let structs = std::mem::take(&mut self.queued_structs);
        let arrays = std::mem::take(&mut self.queued_struct_arrays);
        let protos = std::mem::take(&mut self.queued_protos);
        for item in structs {
            self.put_struct(
                &item.key,
                item.timestamp,
                item.value,
                &item.schema_type,
                false,
            )?;
        }
        for item in arrays {
            self.put_struct(
                &item.key,
                item.timestamp,
                item.value,
                &item.schema_type,
                true,
            )?;
        }
        for item in protos {
            self.put_proto_message(&item.key, item.timestamp, item.value, &item.schema_type)?;
        }
        Ok(())
    }

    fn put_decoded<'a>(
        &mut self,
        key: &JsString,
        timestamp: f64,
        data: &JsValue,
        schema_types: impl Iterator<Item = (&'a JsString, JsString)>,
    ) -> Result<(), LogError> {
        self.put_unknown_struct(key, timestamp, data, false)?;
        for (child, schema_type) in schema_types {
            let full_child_key = child_key_js(key, child);
            self.create_blank_field(&full_child_key, LoggableType::Empty);
            self.process_timestamp(&full_child_key, timestamp);
            self.set_structured_type(&full_child_key, Some(schema_type));
        }
        Ok(())
    }

    /// `putStruct(key, timestamp, value, schemaType, isArray)`; `schema_type` has no `struct:`
    /// prefix and no `[]`. Geometry payloads over 5000 bytes are stored raw and never decoded.
    pub fn put_struct(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: RawValue,
        schema_type: &JsString,
        is_array: bool,
    ) -> Result<(), LogError> {
        self.put_raw(key, timestamp, value.clone())?;
        if self.get_type(key) != Some(LoggableType::Raw) {
            return Ok(());
        }
        self.set_generated_parent(key);
        let mut structured_type = schema_type.clone();
        if is_array {
            structured_type.push_str(&js("[]"));
        }
        self.set_structured_type(key, Some(structured_type));

        if value.len() > BIG_GEOMETRY_BYTES
            && BIG_GEOMETRY.iter().any(|name| *schema_type == js(name))
        {
            return Ok(());
        }

        let decoded = if is_array {
            self.struct_decoder.decode_array(schema_type, &value, None)
        } else {
            self.struct_decoder.decode(schema_type, &value)
        };
        match decoded {
            Ok(decoded) => self.put_decoded(
                key,
                timestamp,
                &decoded.data,
                decoded.schema_types.iter().map(|(k, v)| (k, v.clone())),
            ),
            Err(DecodeError::NeverCompletes) => Err(LogError::NeverCompletes),
            Err(_) => {
                let queue = if is_array {
                    &mut self.queued_struct_arrays
                } else {
                    &mut self.queued_structs
                };
                queue.push(QueuedStructure {
                    key: key.clone(),
                    timestamp,
                    value,
                    schema_type: schema_type.clone(),
                });
                Ok(())
            }
        }
    }

    /// `putPhotonStruct`; `schema_type` has no `photonstruct:` prefix. A failed decode joins
    /// `queuedStructs`, so its retry goes through the struct decoder, as upstream's does.
    pub fn put_photon_struct(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: RawValue,
        schema_type: &JsString,
    ) -> Result<(), LogError> {
        self.put_raw(key, timestamp, value.clone())?;
        if self.get_type(key) != Some(LoggableType::Raw) {
            return Ok(());
        }
        self.set_generated_parent(key);
        self.set_structured_type(key, Some(schema_type.clone()));
        match self.photon_decoder.decode(schema_type, &value) {
            Ok(decoded) => {
                let mut schema_types = Vec::with_capacity(decoded.schema_types.len());
                for (child, ty) in decoded.schema_types.iter() {
                    match ty {
                        JsValue::String(ty) => schema_types.push((child, ty.clone())),
                        _ => {
                            return Err(LogError::Unmodeled(
                                "a photonstruct field without a type token sets an undefined structured type",
                            ));
                        }
                    }
                }
                self.put_decoded(key, timestamp, &decoded.data, schema_types.into_iter())
            }
            Err(_) => {
                self.queued_structs.push(QueuedStructure {
                    key: key.clone(),
                    timestamp,
                    value,
                    schema_type: schema_type.clone(),
                });
                Ok(())
            }
        }
    }

    /// `putProto`; `schema_type` is the full name without `proto:`, or `FileDescriptorProto`,
    /// which adds the descriptor (its errors propagate, as upstream does not catch them), stores
    /// the bytes and retries the queues.
    pub fn put_proto(
        &mut self,
        realm: &mut JsRealm,
        key: &JsString,
        timestamp: f64,
        value: RawValue,
        schema_type: &JsString,
    ) -> Result<(), LogError> {
        if *schema_type == js(FILE_DESCRIPTOR_PROTO) {
            self.proto_decoder
                .add_descriptor(&mut realm.proto, &value)
                .map_err(LogError::Proto)?;
            self.put_raw(key, timestamp, value)?;
            return self.attempt_queued_structures();
        }
        self.put_proto_message(key, timestamp, value, schema_type)
    }

    fn put_proto_message(
        &mut self,
        key: &JsString,
        timestamp: f64,
        value: RawValue,
        schema_type: &JsString,
    ) -> Result<(), LogError> {
        self.put_raw(key, timestamp, value.clone())?;
        if self.get_type(key) != Some(LoggableType::Raw) {
            return Ok(());
        }
        self.set_generated_parent(key);
        self.set_structured_type(key, Some(friendly_schema_type(schema_type)));
        match self.proto_decoder.decode(schema_type, &value) {
            Ok(decoded) => self.put_decoded(
                key,
                timestamp,
                &decoded.data,
                decoded.schema_types.iter().map(|(k, v)| (k, v.clone())),
            ),
            Err(ProtoError::Unmodeled(what)) => Err(LogError::Unmodeled(what)),
            Err(_) => {
                self.queued_protos.push(QueuedStructure {
                    key: key.clone(),
                    timestamp,
                    value,
                    schema_type: schema_type.clone(),
                });
                Ok(())
            }
        }
    }

    /// `putMsgpack`, decoding through the realm's shared key cache.
    pub fn put_msgpack(
        &mut self,
        realm: &mut JsRealm,
        key: &JsString,
        timestamp: f64,
        value: RawValue,
    ) -> Result<(), LogError> {
        self.put_raw(key, timestamp, value.clone())?;
        if self.get_type(key) != Some(LoggableType::Raw) {
            return Ok(());
        }
        self.set_generated_parent(key);
        self.set_structured_type(key, Some(js("MessagePack")));
        let decoded = msgpack::decode(&value, &mut realm.msgpack_keys, &mut *realm.random)
            .unwrap_or(JsValue::Null);
        if decoded.is_null() {
            return Ok(());
        }
        self.put_unknown_struct(key, timestamp, &decoded, false)
    }

    /// `getUnit`: the unit of a Number field, cached until the field's metadata changes or the
    /// field is deleted.
    pub fn get_unit(&self, key: &JsString) -> Option<&'static str> {
        if self.get_type(key)? != LoggableType::Number {
            return None;
        }
        if let Some(&unit) = self.field_unit_cache.lock().get(key) {
            return unit;
        }
        let unit = self.get_unit_uncached(key);
        self.field_unit_cache.lock().insert(key.clone(), unit);
        unit
    }

    fn get_unit_uncached(&self, key: &JsString) -> Option<&'static str> {
        let mut struct_type: Option<&JsString> = None;
        let mut struct_child_key = JsString::new();
        let mut parent_length = 0;
        for parent_key in self.field_keys() {
            if parent_key.len() > parent_length
                && key.starts_with(parent_key, None)
                && let Some(ty) = self.get_structured_type(parent_key)
            {
                struct_type = Some(ty);
                struct_child_key = key.slice((parent_key.len() + 1) as f64, None);
                parent_length = parent_key.len();
            }
        }
        if let Some(ty) = struct_type
            && let Some(unit) =
                units::struct_unit(&ty.to_string_lossy(), &struct_child_key.to_string_lossy())
        {
            return Some(unit);
        }

        let metadata = self.get_metadata_string(key);
        if let Some(unit) = metadata_json_unit(&metadata) {
            return Some(unit);
        }

        let unit_prefix = js("unit");
        for line in metadata.split(Some(&js("\n")), None) {
            if line.to_lower_case().starts_with(&unit_prefix, None)
                && let Some(unit) = units::get_unit_for_field(&line)
            {
                return Some(unit);
            }
        }

        if let Some(unit) = units::get_unit_for_field(key) {
            return Some(unit);
        }

        if key.ends_with(&js("/length"), None) {
            return None;
        }
        let parent_key = self.get_generated_parent(key)?;
        if self.get_type(parent_key) != Some(LoggableType::NumberArray) {
            return None;
        }
        units::get_unit_for_field(parent_key)
    }

    /// `mergeWith(source, prefix)`: copies the source's fields (timestamps shifted by the merge
    /// offset) under `prefix`, replacing fields with the same key, and merges the generated
    /// parents, the timestamp range and zero, and the struct and proto schemas. The offset is
    /// `getMergeOffset`, computed only when both logs have a timestamp range. Returns the
    /// offset; when rebuilding the proto decoder throws, everything before it stays merged.
    pub fn merge_with(
        &mut self,
        source: &Log,
        prefix: &JsString,
        realm: &mut JsRealm,
    ) -> Result<f64, LogError> {
        let offset = if self.timestamp_range.is_some() && source.timestamp_range.is_some() {
            get_merge_offset(self, source)
        } else {
            0.0
        };
        let serialized = source.to_serialized();

        for (key, mut field) in serialized.fields {
            field.timestamps = Arc::new(field.timestamps.iter().map(|t| t + offset).collect());
            let target_key = apply_key_prefix(prefix, &key);
            if !prefix.is_empty() && target_key == *prefix {
                continue;
            }
            self.fields
                .insert(target_key, LogField::from_serialized(field));
        }

        for key in &serialized.generated_parents {
            self.generated_parents.insert(apply_key_prefix(prefix, key));
        }

        if let Some([start, end]) = serialized.timestamp_range {
            let source_range = [start + offset, end + offset];
            self.timestamp_range = Some(match self.timestamp_range {
                None => source_range,
                Some(range) => [
                    math::min2(range[0], source_range[0]),
                    math::max2(range[1], source_range[1]),
                ],
            });
        }
        if let Some(zero) = serialized.timestamp_zero {
            let source_zero = zero + offset;
            self.timestamp_zero = Some(match self.timestamp_zero {
                None => source_zero,
                Some(own) => math::min2(own, source_zero),
            });
        }

        let mut structs = self.struct_decoder.to_serialized();
        structs
            .schema_strings
            .extend(serialized.struct_decoder.schema_strings);
        structs.schemas.extend(serialized.struct_decoder.schemas);
        self.struct_decoder = StructDecoder::from_serialized(structs);

        let mut descriptors = self.proto_decoder.to_serialized().0;
        descriptors.extend(serialized.proto_decoder.0);
        self.proto_decoder =
            ProtoDecoder::from_serialized(&mut realm.proto, &SerializedProtoDecoder(descriptors))
                .map_err(LogError::Proto)?;
        Ok(offset)
    }
}

/// The metadata-JSON step of `getUnit`: the last of `unit`, `units`, `Unit`, `Units` present,
/// lowercased and looked up in `UNIT_SUFFIXES`. A present value that is not a string throws
/// inside upstream's `try`, which abandons the step.
fn metadata_json_unit(metadata: &JsString) -> Option<&'static str> {
    let parsed = json::parse(metadata).ok()?;
    let JsValue::Object(object) = &parsed else {
        return None;
    };
    let mut unit_value = JsString::new();
    for name in UNIT_METADATA_KEYS {
        match object.get(name) {
            None => {}
            Some(JsValue::String(text)) => unit_value = text.to_lower_case(),
            Some(_) => return None,
        }
    }
    if unit_value.is_empty() {
        return None;
    }
    units::unit_suffix(&unit_value)
}
