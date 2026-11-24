//! `generateMCAP` from `src/hub/exportWorker.ts` (F12), with its bugs kept:
//!
//! - the `boolean[]` schema id is overwritten by a schema named `string[]` (items
//!   `string`), so boolean array channels point at schema 6 while schema 4 is only in the
//!   summary;
//! - `StringArray` has no schema, so string array fields are dropped (as are raw fields);
//! - the loop runs to `fields.length` but indexes the filtered list, so each extra
//!   iteration looks up `log.getRange(undefined)`, which is the field keyed `"undefined"`
//!   when the log has one.
//!
//! The sampled branch calls `addValue` without awaiting it. Its writes still happen in order
//! until a chunk fills mid-loop: that `addMessage` parks inside `finalizeChunk`'s awaited
//! write, and the next one re-enters the writer before the chunk builder is reset, so those
//! option sets never finish upstream (the harness skips them before export) and this reports
//! `NeverCompletes`. A sample whose `addValue` throws is an unhandled rejection, which
//! Chromium only logs: the message is lost and the export goes on.

use asrs_js::json;
use asrs_js::math;
use asrs_js::string::{JsString, text_encode};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_value_sets::LogValueSetAny;
use asrs_log::log::{Log, LoggableType};
use asrs_log::util::upper_bound;

use crate::export::{ExportError, fraction};
use crate::mcap::{McapError, McapWriter, Message, Time};

fn schema_json(value: &str) -> Vec<u8> {
    format!(r#"{{"type":"object","properties":{{"value":{value}}}}}"#).into_bytes()
}

fn register_single(writer: &mut McapWriter, ty: &str) -> u32 {
    writer.register_schema(
        JsString::from(ty),
        JsString::from("jsonschema"),
        schema_json(&format!(r#"{{"type":"{ty}"}}"#)),
    )
}

fn register_array(writer: &mut McapWriter, ty: &str) -> u32 {
    writer.register_schema(
        JsString::from(format!("{ty}[]").as_str()),
        JsString::from("jsonschema"),
        schema_json(&format!(r#"{{"type":"array","items":{{"type":"{ty}"}}}}"#)),
    )
}

/// `textEncoder.encode(JSON.stringify({ value }))` for the sample at `index`, or `None`
/// for a set whose type has no schema.
pub fn value_json(set: &LogValueSetAny, index: usize) -> Option<Vec<u8>> {
    let value = match set {
        LogValueSetAny::Boolean(set) => JsValue::Bool(set.values[index]),
        LogValueSetAny::Number(set) => JsValue::Number(set.values[index]),
        LogValueSetAny::String(set) => JsValue::String(set.values[index].clone()),
        LogValueSetAny::BooleanArray(set) => JsValue::Array(
            set.values[index]
                .iter()
                .map(|&v| JsValue::Bool(v))
                .collect(),
        ),
        LogValueSetAny::NumberArray(set) => JsValue::Array(
            set.values[index]
                .iter()
                .map(|&v| JsValue::Number(v))
                .collect(),
        ),
        LogValueSetAny::Raw(set) => JsValue::Bytes(set.values[index].to_vec()),
        LogValueSetAny::StringArray(set) => JsValue::Array(
            set.values[index]
                .iter()
                .map(|v| JsValue::String(v.clone()))
                .collect(),
        ),
        LogValueSetAny::Empty(_) => return None,
    };
    let object: JsObject<JsValue> = [(JsString::from("value"), value)].into_iter().collect();
    json::stringify(&JsValue::Object(object))
        .ok()
        .flatten()
        .map(String::into_bytes)
}

/// `addValue`'s body up to `addMessage`: `BigInt(Math.round(timestamp * 1e9))` throws
/// `RangeError` once the product overflows, and `JSON.stringify` of a typeless value
/// produces nothing, both before `addMessage` runs.
fn message_at(
    channel_id: u32,
    set: &LogValueSetAny,
    timestamp: f64,
    index: usize,
    sequence: usize,
) -> Option<Message> {
    let time = Time::from_integral(math::round(timestamp * 1e9))?;
    let data = value_json(set, index)?;
    Some(Message {
        channel_id,
        sequence: sequence as u32,
        log_time: time,
        publish_time: time,
        data,
    })
}

fn add_value(
    writer: &mut McapWriter,
    channel_id: u32,
    set: &LogValueSetAny,
    timestamp: f64,
    index: usize,
    sequence: usize,
) -> Result<(), ()> {
    let message = message_at(channel_id, set, timestamp, index, sequence).ok_or(())?;
    writer.add_message(message).map_err(|_: McapError| ())?;
    Ok(())
}

pub fn generate_mcap(
    log: &Log,
    fields: &[JsString],
    timestamps: &Option<Vec<f64>>,
    progress: &mut dyn FnMut(f64),
) -> Result<Vec<u8>, ExportError> {
    let mut writer = McapWriter::new();
    writer.start(&JsString::new(), &JsString::from("AdvantageScope"));

    let boolean = register_single(&mut writer, "boolean");
    let number = register_single(&mut writer, "number");
    let string = register_single(&mut writer, "string");
    register_array(&mut writer, "boolean");
    let number_array = register_array(&mut writer, "number");
    let boolean_array = register_array(&mut writer, "string");
    let schema_id = |ty: LoggableType| match ty {
        LoggableType::Boolean => Some(boolean),
        LoggableType::Number => Some(number),
        LoggableType::String => Some(string),
        LoggableType::BooleanArray => Some(boolean_array),
        LoggableType::NumberArray => Some(number_array),
        _ => None,
    };

    let filtered: Vec<&JsString> = fields
        .iter()
        .filter(|field| log.get_type(field).and_then(schema_id).is_some())
        .collect();
    let undefined = JsString::from("undefined");
    for field_index in 0..fields.len() {
        let field = filtered.get(field_index).copied();
        let key = field.unwrap_or(&undefined);
        let (Some(field_data), Some(field_type)) = (
            log.get_range(key, f64::NEG_INFINITY, f64::INFINITY, None),
            log.get_type(key),
        ) else {
            continue;
        };

        let channel_id = writer.register_channel(
            schema_id(field_type),
            field.map(text_encode).unwrap_or_default(),
            JsString::from("json"),
            Vec::new(),
        );

        let field_timestamps = field_data.timestamps();
        match timestamps {
            None => {
                for (i, &timestamp) in field_timestamps.iter().enumerate() {
                    add_value(&mut writer, channel_id, &field_data, timestamp, i, i)
                        .map_err(|()| ExportError::Rejected)?;
                    progress(fraction(field_index, i, field_data.len(), filtered.len()));
                }
            }
            Some(timestamps) => {
                // A message that fills the chunk parks its unawaited `addMessage` inside
                // `finalizeChunk`; the next `addMessage` to run while one is parked re-enters
                // the writer and the export never finishes. The parked call drains when the
                // next field's `registerChannel` is awaited, so the flag is per field.
                let mut finalize_parked = false;
                let mut next_index = 0;
                for (i, &timestamp) in timestamps.iter().enumerate() {
                    next_index = upper_bound(field_timestamps, timestamp, next_index);
                    if next_index != 0 {
                        if let Some(message) =
                            message_at(channel_id, &field_data, timestamp, next_index - 1, i)
                        {
                            if finalize_parked {
                                return Err(ExportError::NeverCompletes);
                            }
                            finalize_parked = writer.add_message(message).unwrap_or(false);
                        }
                    }
                    progress(fraction(field_index, i, timestamps.len(), fields.len()));
                }
            }
        }
    }

    Ok(writer.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_records(file: &[u8]) -> Vec<(u8, &[u8])> {
        let u64_at = |at: usize| u64::from_le_bytes(file[at..at + 8].try_into().unwrap()) as usize;
        let footer = file.len() - 8 - 29;
        let (mut at, end) = (u64_at(footer + 9), u64_at(footer + 17));
        let mut records = Vec::new();
        while at < end {
            let length = u64_at(at + 1);
            records.push((file[at], &file[at + 9..at + 9 + length]));
            at += 9 + length;
        }
        records
    }

    fn summary_channels(file: &[u8]) -> Vec<(u16, u16, String)> {
        let u16_at = |body: &[u8], at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
        summary_records(file)
            .into_iter()
            .filter(|&(opcode, _)| opcode == 0x04)
            .map(|(_, body)| {
                let topic_length = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
                let topic = &body[8..8 + topic_length];
                (
                    u16_at(body, 0),
                    u16_at(body, 2),
                    String::from_utf8(topic.to_vec()).unwrap(),
                )
            })
            .collect()
    }

    /// The Statistics record's message count and its per-channel counts.
    fn summary_message_counts(file: &[u8]) -> (u64, Vec<(u16, u64)>) {
        let (_, body) = summary_records(file)
            .into_iter()
            .find(|&(opcode, _)| opcode == 0x0b)
            .unwrap();
        let u64_at = |at: usize| u64::from_le_bytes(body[at..at + 8].try_into().unwrap());
        let counts = body[46..]
            .chunks(10)
            .map(|entry| {
                let count = u64::from_le_bytes(entry[2..10].try_into().unwrap());
                (u16::from_le_bytes([entry[0], entry[1]]), count)
            })
            .collect();
        (u64_at(0), counts)
    }

    fn chunk_message_count(file: &[u8]) -> usize {
        summary_records(file)
            .into_iter()
            .filter(|&(opcode, _)| opcode == 0x08)
            .map(|(_, body)| {
                let body = u64::from_le_bytes(body[16..24].try_into().unwrap()) as usize + 9;
                let size = u64::from_le_bytes(file[body + 32..body + 40].try_into().unwrap());
                let records = &file[body + 40..body + 40 + size as usize];
                let mut at = 0;
                let mut messages = 0;
                while at < records.len() {
                    messages += usize::from(records[at] == 0x05);
                    at += 9 + u64::from_le_bytes(records[at + 1..at + 9].try_into().unwrap())
                        as usize;
                }
                messages
            })
            .sum()
    }

    #[test]
    fn keeps_upstream_schema_and_loop_bugs() {
        let key = |k: &str| JsString::from(k);
        let mut log = Log::new(true);
        log.put_boolean_array(&key("/Flags"), 1.0, vec![true, false]);
        log.put_string_array(&key("/Names"), 1.0, vec![key("a")]);
        log.put_number(&key("undefined"), 1.0, 0.5);
        log.put_number(&key("/Value"), 1.0, 2.0);
        let fields = ["/Flags", "/Names", "undefined", "/Value"].map(key);
        let file = generate_mcap(&log, &fields, &None, &mut |_| {}).unwrap();
        assert_eq!(
            summary_channels(&file),
            [
                (0, 6, "/Flags".to_owned()),
                (1, 2, "undefined".to_owned()),
                (2, 2, "/Value".to_owned()),
                (3, 2, String::new()),
            ]
        );
    }

    #[test]
    fn sampled_export_loses_messages_whose_add_value_throws() {
        let key = |k: &str| JsString::from(k);
        let mut log = Log::new(true);
        log.put_string_array(&key("undefined"), 1.0, vec![key("a")]);
        log.put_number(&key("/Value"), 1.0, 2.0);
        let fields = ["/Value", "undefined"].map(key);
        assert!(matches!(
            generate_mcap(&log, &fields, &None, &mut |_| {}),
            Err(ExportError::Rejected)
        ));

        let file =
            generate_mcap(&log, &fields, &Some(vec![1.0, 2.0, 1.8e299]), &mut |_| {}).unwrap();
        assert_eq!(
            summary_channels(&file),
            [(0, 2, "/Value".to_owned()), (1, 0, String::new())]
        );
        assert_eq!(summary_message_counts(&file), (4, vec![(0, 2), (1, 2)]));
        assert_eq!(chunk_message_count(&file), 2);
    }

    #[test]
    fn sampled_export_past_one_chunk_never_completes() {
        let key = |k: &str| JsString::from(k);
        let mut log = Log::new(true);
        log.put_string(
            &key("/Value"),
            1.0,
            JsString::from("x".repeat(1_100_000).as_str()),
        );
        let fields = [key("/Value")];
        // The call that fills the chunk is the last one: its parked `finalizeChunk`
        // drains once the loop ends, so a single crossing still completes.
        assert!(generate_mcap(&log, &fields, &Some(vec![1.0]), &mut |_| {}).is_ok());
        assert!(matches!(
            generate_mcap(&log, &fields, &Some(vec![1.0, 2.0]), &mut |_| {}),
            Err(ExportError::NeverCompletes)
        ));
    }
}
