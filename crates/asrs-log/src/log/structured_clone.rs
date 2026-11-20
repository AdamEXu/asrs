//! What `postMessage` does to a serialized log on its way to another realm. V8's structured
//! clone stores every NaN it deserializes as the canonical quiet NaN, so NaN payloads that a
//! realm kept (msgpack and struct floats written into a field) do not survive a transfer.

use std::sync::Arc;

use super::log_core::SerializedLog;
use super::log_field::SerializedLogField;
use super::log_value_sets::LogValues;
use super::log_writers::QueuedStructure;
use super::struct_decoder::{Schema, ValueSchema};

fn canonical(x: f64) -> f64 {
    if x.is_nan() { f64::NAN } else { x }
}

fn has_payload(x: &f64) -> bool {
    x.is_nan() && x.to_bits() != f64::NAN.to_bits()
}

fn canonical_column(column: &mut Arc<Vec<f64>>) {
    if column.iter().any(has_payload) {
        Arc::make_mut(column)
            .iter_mut()
            .for_each(|x| *x = canonical(*x));
    }
}

impl SerializedLogField {
    /// The field as the receiving realm of a `postMessage` sees it.
    pub fn structured_clone(mut self) -> Self {
        self.canonicalize_nans();
        self
    }

    fn canonicalize_nans(&mut self) {
        canonical_column(&mut self.timestamps);
        match &mut self.values {
            LogValues::Number(column) => canonical_column(column),
            LogValues::NumberArray(column) if column.iter().flatten().any(has_payload) => {
                for item in Arc::make_mut(column) {
                    item.iter_mut().for_each(|x| *x = canonical(*x));
                }
            }
            _ => {}
        }
    }
}

fn canonical_schema(schema: &mut Schema) {
    schema.length = canonical(schema.length);
    for value in &mut schema.value_schemas {
        let ValueSchema {
            bitfield_width,
            array_length,
            bit_range,
            ..
        } = value;
        *bitfield_width = bitfield_width.map(canonical);
        *array_length = array_length.map(canonical);
        *bit_range = bit_range.map(canonical);
    }
}

fn canonical_queue(queue: &mut [QueuedStructure]) {
    for item in queue {
        item.timestamp = canonical(item.timestamp);
    }
}

impl SerializedLog {
    /// The log as the receiving realm of a `postMessage` sees it.
    pub fn structured_clone(mut self) -> Self {
        for (_, field) in self.fields.iter_mut() {
            field.canonicalize_nans();
        }
        self.timestamp_range = self.timestamp_range.map(|range| range.map(canonical));
        self.timestamp_zero = self.timestamp_zero.map(canonical);
        for (_, schema) in self.struct_decoder.schemas.iter_mut() {
            canonical_schema(schema);
        }
        canonical_queue(&mut self.queued_structs);
        canonical_queue(&mut self.queued_struct_arrays);
        canonical_queue(&mut self.queued_protos);
        self
    }
}
