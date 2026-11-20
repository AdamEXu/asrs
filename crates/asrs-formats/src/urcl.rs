//! Port of `src/hub/dataSources/schema/URCLSchema.ts` (revision 3) and
//! `URCLSchemaLegacy.ts` (revisions 1 and 2): the URCL custom-schema decoders the
//! wpilog worker runs through `CustomSchemas`.

use asrs_js::json;
use asrs_js::math::sign;
use asrs_js::number::to_string as number_to_string;
use asrs_js::string::{JsString, TextDecoder};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_util::{get_or_default, get_value_at};
use asrs_log::log::log_value_sets::RawValue;
use asrs_log::log::{Log, LogError, LoggableType};

use crate::spark::{self, Frame, SignalType, SignalValue, tables};

#[derive(Debug)]
pub enum SchemaError {
    /// The parser threw. The wpilog worker catches it, so only the writes made before the
    /// throw remain.
    Threw,
    /// A `Log` write ended the way `LogError` describes.
    Log(LogError),
}

impl From<LogError> for SchemaError {
    fn from(error: LogError) -> Self {
        SchemaError::Log(error)
    }
}

pub type SchemaParser = fn(&mut Log, &JsString, f64, &[u8]) -> Result<(), SchemaError>;

/// `CustomSchemas.get(type)`.
pub fn custom_schema(type_: &JsString) -> Option<SchemaParser> {
    let parsers: [(&str, SchemaParser); 3] = [
        ("URCL", parse_urcl_r1),
        ("URCLr2_periodic", parse_urcl_r2),
        ("URCLr3_periodic", parse_urcl_r3),
    ];
    parsers
        .into_iter()
        .find(|(name, _)| *type_ == js(name))
        .map(|(_, parser)| parser)
}

const PERSISTENT_SIZE: usize = 8;
const PERIODIC_SIZE: usize = 14;
const FIRMWARE_API: u32 = 0x98;
const MODEL_API: u32 = 0x300 | 155;
const LEGACY_PERIODIC_API_CLASS: u32 = 6;

const DEFAULT_ALIASES: &[u8] = &[0x7b, 0x7d];

const FAULTS: [&str; 16] = [
    "Brownout",
    "OverCurrent",
    "WatchdogReset",
    "MotorTypeFault",
    "SensorFault",
    "Stall",
    "EEPROMFault",
    "CANTXFault",
    "CANRXFault",
    "HasReset",
    "GateDriverFault",
    "OtherFault",
    "SoftLimitForward",
    "SoftLimitReverse",
    "HardLimitForward",
    "HardLimitReverse",
];

const APPLIED_OUTPUT_VOLTAGE_DESCRIPTION: &str = "Calculated by AdvantageScope. The estimated voltage output based on the applied output (duty cycle) and input voltage of the Spark.";
const CURRENT_INPUT_DESCRIPTION: &str = "Calculated by AdvantageScope. The input (supply) current based on the applied output (duty cycle) and output (stator) current of the Spark.";

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn child(key: &JsString, suffix: &str) -> JsString {
    let mut out = key.clone();
    out.push_str(&js(suffix));
    out
}

fn u8_at(v: &[u8], pos: usize) -> Result<u8, SchemaError> {
    v.get(pos).copied().ok_or(SchemaError::Threw)
}

fn bytes<const N: usize>(v: &[u8], pos: usize) -> Result<[u8; N], SchemaError> {
    v.get(pos..pos + N)
        .ok_or(SchemaError::Threw)?
        .try_into()
        .map_err(|_| SchemaError::Threw)
}

fn u16le(v: &[u8], pos: usize) -> Result<u16, SchemaError> {
    Ok(u16::from_le_bytes(bytes(v, pos)?))
}

fn u16be(v: &[u8], pos: usize) -> Result<u16, SchemaError> {
    Ok(u16::from_be_bytes(bytes(v, pos)?))
}

fn i16le(v: &[u8], pos: usize) -> Result<i16, SchemaError> {
    Ok(i16::from_le_bytes(bytes(v, pos)?))
}

fn u32le(v: &[u8], pos: usize) -> Result<u32, SchemaError> {
    Ok(u32::from_le_bytes(bytes(v, pos)?))
}

fn f32le(v: &[u8], pos: usize) -> Result<f32, SchemaError> {
    Ok(f32::from_le_bytes(bytes(v, pos)?))
}

fn slice(v: &[u8], start: usize, end: usize) -> &[u8] {
    &v[start.min(v.len())..end.min(v.len())]
}

fn id_key(device_id: u32) -> JsString {
    js(&number_to_string(f64::from(device_id)))
}

/// The `+` coercion applied to `devices[deviceId].alias` when it is not a string. JSON input
/// only yields the ordinary value kinds. An object whose own `toString` key holds data has no
/// callable `toString`, and `valueOf` returns the object itself, so the coercion throws.
fn js_to_string(value: &JsValue) -> Result<JsString, SchemaError> {
    Ok(match value {
        JsValue::Undefined => js("undefined"),
        JsValue::Null => js("null"),
        JsValue::Bool(b) => js(if *b { "true" } else { "false" }),
        JsValue::Number(n) => js(&number_to_string(*n)),
        JsValue::String(s) => s.clone(),
        JsValue::BigInt(b) => js(&format!("{b}")),
        JsValue::Bytes(items) => js(&items
            .iter()
            .map(|b| number_to_string(f64::from(*b)))
            .collect::<Vec<_>>()
            .join(",")),
        JsValue::Array(items) => {
            let mut out = JsString::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(&js(","));
                }
                if !matches!(item, JsValue::Null | JsValue::Undefined) {
                    out.push_str(&js_to_string(item)?);
                }
            }
            out
        }
        JsValue::Object(object) if object.contains_key("toString") => {
            return Err(SchemaError::Threw);
        }
        JsValue::Object(_) | JsValue::ExtData(..) => js("[object Object]"),
        JsValue::Date(_) => js("Invalid Date"),
    })
}

/// `Object.keys(aliases)` plus the per-key read. `Object.keys` throws on `null` and
/// `undefined`; strings and arrays yield index keys.
fn object_entries(value: &JsValue) -> Result<Vec<(JsString, JsValue)>, SchemaError> {
    match value {
        JsValue::Object(object) => Ok(object.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        JsValue::Array(items) => Ok(items
            .iter()
            .enumerate()
            .map(|(i, v)| (id_key(i as u32), v.clone()))
            .collect()),
        JsValue::String(s) => Ok(s
            .units()
            .iter()
            .enumerate()
            .map(|(i, u)| {
                (
                    id_key(i as u32),
                    JsValue::String(JsString::from_units(vec![*u])),
                )
            })
            .collect()),
        JsValue::Null | JsValue::Undefined => Err(SchemaError::Threw),
        _ => Ok(Vec::new()),
    }
}

fn utf8_decode(bytes: &[u8]) -> JsString {
    TextDecoder::new(Some(&js("UTF-8")), false, false)
        .expect("utf-8 is always supported")
        .decode(bytes)
        .expect("non-fatal decode never fails")
}

fn description_metadata(description: &str) -> JsString {
    let mut out = String::from("{\"description\":");
    let units: Vec<u16> = description.encode_utf16().collect();
    json::quote_into(&units, &mut out);
    out.push('}');
    js(&out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceModel {
    SparkMax,
    SparkFlex,
}

impl DeviceModel {
    fn name(self) -> &'static str {
        match self {
            DeviceModel::SparkMax => "SparkMax",
            DeviceModel::SparkFlex => "SparkFlex",
        }
    }
}

#[derive(Clone, Copy)]
struct FirmwareVersion {
    major: f64,
    minor: f64,
    build: f64,
}

#[derive(Clone, Default)]
struct Device {
    alias: Option<JsValue>,
    model: Option<DeviceModel>,
    firmware: Option<FirmwareVersion>,
    major_firmware: Option<f64>,
}

fn read_aliases(
    log: &Log,
    alias_key: &JsString,
    timestamp: f64,
) -> Result<Vec<(JsString, JsValue)>, SchemaError> {
    let raw = get_value_at::<RawValue>(log, alias_key, timestamp, None);
    let raw: &[u8] = raw.as_deref().unwrap_or(DEFAULT_ALIASES);
    let aliases = json::parse(&utf8_decode(raw)).map_err(|_| SchemaError::Threw)?;
    object_entries(&aliases)
}

fn periodic_root(key: &JsString) -> Option<JsString> {
    if !key.ends_with(&js("Raw/Periodic"), None) {
        return None;
    }
    Some(key.slice(0.0, Some(key.len() as f64 - "Raw/Periodic".len() as f64)))
}

/// `URCLSchema.parseURCLr3`.
pub fn parse_urcl_r3(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
) -> Result<(), SchemaError> {
    let periodic_api_class = tables::STATUS_0_2026.api_class;
    let firmware_spec: &Frame = &tables::GET_FIRMWARE_VERSION_2026;
    let firmware_api = (firmware_spec.api_class << 4) | firmware_spec.api_index;

    let Some(root_key) = periodic_root(key) else {
        return Ok(());
    };
    let mut devices: JsObject<Device> = JsObject::new();
    let alias_key = child(&root_key, "Raw/Aliases");
    let persistent_key = child(&root_key, "Raw/Persistent");
    let get_name = |devices: &JsObject<Device>, device_id: &JsString| {
        let device = devices.get_key(device_id).expect("getName key exists");
        match &device.alias {
            Some(alias) => js_to_string(alias),
            None => {
                let mut name = js("Spark-");
                name.push_str(device_id);
                Ok(name)
            }
        }
    };

    for (id_string, alias) in read_aliases(log, &alias_key, timestamp)? {
        devices.insert(
            id_string,
            Device {
                alias: Some(alias),
                ..Device::default()
            },
        );
    }

    let Some(persistent_raw) = get_value_at::<RawValue>(log, &persistent_key, timestamp, None)
    else {
        return Ok(());
    };
    let persistent: &[u8] = &persistent_raw;
    let mut position = 0;
    while position < persistent.len() {
        let message_id = u32::from(u16le(persistent, position)?);
        let message_value = slice(persistent, position + 2, position + 8);
        let device_id = message_id & 0x3f;
        let device_key = id_key(device_id);
        if devices.get_key(&device_key).is_none() {
            devices.insert(device_key.clone(), Device::default());
        }
        if (message_id >> 6) & 0x3ff == firmware_api {
            let mut full_message_value = [0u8; 8];
            full_message_value[..message_value.len()].copy_from_slice(message_value);
            let firmware_values = spark::parse_can_frame(firmware_spec, &full_message_value, None)
                .map_err(|_| SchemaError::Threw)?;
            devices
                .get_units_mut(device_key.units())
                .expect("just inserted")
                .major_firmware = firmware_values.get("MAJOR").map(signal_number);
        }
        position += PERSISTENT_SIZE;
    }

    for (device_id, device) in devices.iter() {
        let Some(major) = device.major_firmware else {
            continue;
        };
        let device_key = {
            let mut out = root_key.clone();
            out.push_str(&get_name(&devices, device_id)?);
            out
        };
        log.put_string(
            &child(&device_key, "/Firmware"),
            timestamp,
            js(&number_to_string(major)),
        );
        log.create_blank_field(&device_key, LoggableType::Empty);
        log.set_generated_parent(&device_key);
    }

    let mut position = 0;
    while position < value.len() {
        let message_timestamp = f64::from(u32le(value, position)?) / 1e3;
        let message_id = u32::from(u16le(value, position + 4)?);
        let message_value = slice(value, position + 6, position + 14);
        let device_id = message_id & 0x3f;
        let device_key = id_key(device_id);
        let device = devices.get_key(&device_key);
        match device.and_then(|d| d.major_firmware) {
            Some(major) if major >= 25.0 => {}
            _ => {
                position += PERIODIC_SIZE;
                continue;
            }
        }
        let device = device.expect("checked");

        if (message_id >> 10) & 0x3f == periodic_api_class {
            let frame_index = (message_id >> 6) & 0xf;
            let device_log_key = {
                let mut out = root_key.clone();
                out.push_str(&get_name(&devices, &device_key)?);
                out
            };
            let frame_key = child(
                &child(&device_log_key, "/PeriodicFrame/"),
                &number_to_string(frame_index as f64),
            );
            let specs: &[&Frame] = if device.major_firmware.expect("checked") >= 26.0 {
                tables::STATUS_2026
            } else {
                tables::STATUS_2025
            };
            log.put_raw(&frame_key, message_timestamp, message_value)?;
            if (frame_index as usize) < specs.len() {
                let frame_spec = specs[frame_index as usize];
                let frame_values = spark::parse_can_frame(frame_spec, message_value, None)
                    .map_err(|_| SchemaError::Threw)?;

                let mut applied_output: Option<f64> = None;
                let mut voltage: Option<f64> = None;
                let mut output_current: Option<f64> = None;

                for (signal_key, signal_value) in &frame_values.entries {
                    let Some(signal_spec) = frame_spec
                        .signals
                        .iter()
                        .find(|(k, _)| k == signal_key)
                        .map(|(_, s)| s)
                    else {
                        continue;
                    };
                    if signal_spec.name.contains("Reserved") {
                        continue;
                    }
                    let signal_group = if signal_spec.name.contains("Fault") {
                        "Fault/"
                    } else if signal_spec.name.contains("Warning") {
                        "Warning/"
                    } else {
                        ""
                    };
                    let mut signal_name = signal_spec.name.replace(' ', "");
                    if signal_name == "Current" {
                        "CurrentOutput".clone_into(&mut signal_name);
                    } else if signal_name == "Voltage" {
                        "InputVoltage".clone_into(&mut signal_name);
                    }
                    let log_key = {
                        let mut out = device_log_key.clone();
                        out.push_str(&js("/"));
                        out.push_str(&js(signal_group));
                        out.push_str(&js(&signal_name));
                        out
                    };

                    match signal_spec.signal_type {
                        SignalType::Int | SignalType::Uint | SignalType::Float => {
                            log.put_number(
                                &log_key,
                                message_timestamp,
                                signal_number(signal_value),
                            );
                        }
                        SignalType::Boolean => {
                            if let SignalValue::Bool(b) = signal_value {
                                log.put_boolean(&log_key, message_timestamp, *b);
                            }
                        }
                    }
                    if let Some(description) = signal_spec.description {
                        log.set_metadata_string(&log_key, description_metadata(description));
                    }

                    if signal_name == "AppliedOutput" {
                        applied_output = Some(signal_number(signal_value));
                    } else if signal_name == "InputVoltage" {
                        voltage = Some(signal_number(signal_value));
                    } else if signal_name == "CurrentOutput" {
                        output_current = Some(signal_number(signal_value));
                    }
                }

                // Upstream writes the derived fields with the outer record timestamp,
                // not `messageTimestamp`.
                if let (Some(applied), Some(v)) = (applied_output, voltage)
                    && v > 0.0
                {
                    let derived = child(&device_log_key, "/AppliedOutputVoltage");
                    log.put_number(&derived, timestamp, applied * v);
                    log.set_metadata_string(
                        &derived,
                        description_metadata(APPLIED_OUTPUT_VOLTAGE_DESCRIPTION),
                    );
                }
                if let (Some(current), Some(applied)) = (output_current, applied_output) {
                    let derived = child(&device_log_key, "/CurrentInput");
                    log.put_number(&derived, timestamp, (current * applied).abs());
                    log.set_metadata_string(
                        &derived,
                        description_metadata(CURRENT_INPUT_DESCRIPTION),
                    );
                }
            }
        }
        position += PERIODIC_SIZE;
    }
    Ok(())
}

/// `Number(signalValue)`: a `BigNumber` or boolean.
fn signal_number(value: &SignalValue) -> f64 {
    match value {
        SignalValue::Num(n) => n.to_number(),
        SignalValue::Bool(b) => f64::from(*b as u8),
    }
}

/// `URCLSchemaLegacy.parseURCLr2`.
pub fn parse_urcl_r2(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
) -> Result<(), SchemaError> {
    let Some(root_key) = periodic_root(key) else {
        return Ok(());
    };
    let mut devices: JsObject<Device> = JsObject::new();
    let alias_key = child(&root_key, "Raw/Aliases");
    let persistent_key = child(&root_key, "Raw/Persistent");
    let get_name = |devices: &JsObject<Device>, device_id: &JsString| {
        let device = devices.get_key(device_id).expect("getName key exists");
        match &device.alias {
            Some(alias) => js_to_string(alias),
            None => {
                let mut name = js(device.model.map_or("undefined", DeviceModel::name));
                name.push_str(&js("-"));
                name.push_str(device_id);
                Ok(name)
            }
        }
    };

    for (id_string, alias) in read_aliases(log, &alias_key, timestamp)? {
        devices.insert(
            id_string,
            Device {
                alias: Some(alias),
                ..Device::default()
            },
        );
    }

    let Some(persistent_raw) = get_value_at::<RawValue>(log, &persistent_key, timestamp, None)
    else {
        return Ok(());
    };
    read_legacy_persistent(&persistent_raw, &mut devices)?;

    for (device_id, device) in devices.iter() {
        let (Some(_), Some(firmware)) = (device.model, device.firmware) else {
            continue;
        };
        let device_key = {
            let mut out = root_key.clone();
            out.push_str(&get_name(&devices, device_id)?);
            out
        };
        log.put_string(
            &child(&device_key, "/Firmware"),
            timestamp,
            firmware_string(firmware),
        );
        log.create_blank_field(&device_key, LoggableType::Empty);
        log.set_generated_parent(&device_key);
    }

    let mut position = 0;
    while position < value.len() {
        let message_timestamp = f64::from(u32le(value, position)?) / 1e3;
        let message_id = u32::from(u16le(value, position + 4)?);
        let message_value = slice(value, position + 6, position + 14);
        let device_id = message_id & 0x3f;
        let device_key = id_key(device_id);
        let Some(device) = devices.get_key(&device_key) else {
            position += PERIODIC_SIZE;
            continue;
        };
        if device.model.is_none() || device.firmware.is_none() {
            position += PERIODIC_SIZE;
            continue;
        }

        if (message_id >> 10) & 0x3f == LEGACY_PERIODIC_API_CLASS {
            let frame_index = ((message_id >> 6) & 0xf) as usize;
            let device_log_key = {
                let mut out = root_key.clone();
                out.push_str(&get_name(&devices, &device_key)?);
                out
            };
            let frame_key = child(
                &child(&device_log_key, "/PeriodicFrame/"),
                &number_to_string(frame_index as f64),
            );
            log.put_raw(&frame_key, message_timestamp, message_value)?;
            if frame_index < PARSE_PERIODIC.len() {
                PARSE_PERIODIC[frame_index](
                    log,
                    &device_log_key,
                    message_timestamp,
                    message_value,
                    device.model.expect("checked"),
                    device.firmware.expect("checked"),
                )?;
            }
        }
        position += PERIODIC_SIZE;
    }
    Ok(())
}

/// `URCLSchemaLegacy.parseURCLr1`.
pub fn parse_urcl_r1(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
) -> Result<(), SchemaError> {
    let persistent_count = u64::from(u32le(value, 0)?);
    let periodic_count = u64::from(u32le(value, 4)?);

    let mut devices: JsObject<Device> = JsObject::new();
    let mut position = 8usize;
    while (position as u64) < 8 + persistent_count * PERSISTENT_SIZE as u64 {
        read_legacy_record(value, position, &mut devices)?;
        position += PERSISTENT_SIZE;
    }

    for (device_id, device) in devices.iter() {
        let (Some(model), Some(firmware)) = (device.model, device.firmware) else {
            continue;
        };
        let device_key = legacy_device_key(key, model, device_id);
        log.put_string(
            &child(&device_key, "/Firmware"),
            timestamp,
            firmware_string(firmware),
        );
    }

    let periodic_end =
        8 + persistent_count * PERSISTENT_SIZE as u64 + periodic_count * PERIODIC_SIZE as u64;
    while (position as u64) < periodic_end {
        let message_timestamp = f64::from(u32le(value, position)?) / 1e3;
        let message_id = u32::from(u16le(value, position + 4)?);
        let message_value = slice(value, position + 6, position + 14);
        let device_id = message_id & 0x3f;
        let device_key = id_key(device_id);
        let Some(device) = devices.get_key(&device_key) else {
            position += PERIODIC_SIZE;
            continue;
        };
        if device.model.is_none() || device.firmware.is_none() {
            position += PERIODIC_SIZE;
            continue;
        }

        if (message_id >> 10) & 0x3f == LEGACY_PERIODIC_API_CLASS {
            let frame_index = ((message_id >> 6) & 0xf) as usize;
            let device_log_key =
                legacy_device_key(key, device.model.expect("checked"), &device_key);
            let frame_key = child(
                &child(&device_log_key, "/PeriodicFrame/"),
                &number_to_string(frame_index as f64),
            );
            log.put_raw(&frame_key, message_timestamp, message_value)?;
            if frame_index < PARSE_PERIODIC.len() {
                PARSE_PERIODIC[frame_index](
                    log,
                    &device_log_key,
                    message_timestamp,
                    message_value,
                    device.model.expect("checked"),
                    device.firmware.expect("checked"),
                )?;
            }
        }
        position += PERIODIC_SIZE;
    }
    Ok(())
}

fn legacy_device_key(key: &JsString, model: DeviceModel, device_id: &JsString) -> JsString {
    let mut out = key.clone();
    out.push_str(&js("/"));
    out.push_str(&js(model.name()));
    out.push_str(&js("-"));
    out.push_str(device_id);
    out
}

fn read_legacy_record(
    persistent: &[u8],
    position: usize,
    devices: &mut JsObject<Device>,
) -> Result<(), SchemaError> {
    let message_id = u32::from(u16le(persistent, position)?);
    let message_value = slice(persistent, position + 2, position + 8);
    let device_id = message_id & 0x3f;
    let device_key = id_key(device_id);
    if devices.get_key(&device_key).is_none() {
        devices.insert(device_key.clone(), Device::default());
    }
    let api = (message_id >> 6) & 0x3ff;
    if api == FIRMWARE_API {
        devices
            .get_units_mut(device_key.units())
            .expect("just inserted")
            .firmware = Some(parse_firmware(message_value)?);
    } else if api == MODEL_API {
        devices
            .get_units_mut(device_key.units())
            .expect("just inserted")
            .model = Some(parse_device_model(message_value)?);
    }
    Ok(())
}

fn read_legacy_persistent(
    persistent: &[u8],
    devices: &mut JsObject<Device>,
) -> Result<(), SchemaError> {
    let mut position = 0;
    while position < persistent.len() {
        read_legacy_record(persistent, position, devices)?;
        position += PERSISTENT_SIZE;
    }
    Ok(())
}

fn parse_firmware(value: &[u8]) -> Result<FirmwareVersion, SchemaError> {
    Ok(FirmwareVersion {
        major: f64::from(u8_at(value, 0)?),
        minor: f64::from(u8_at(value, 1)?),
        build: f64::from(u16be(value, 2)?),
    })
}

fn parse_device_model(value: &[u8]) -> Result<DeviceModel, SchemaError> {
    let parameter_value = u32le(value, 0)?;
    let success = u8_at(value, 5)? == 0;
    Ok(match (success, parameter_value) {
        (true, 0x2158) => DeviceModel::SparkMax,
        (true, 0x2159) => DeviceModel::SparkFlex,
        _ => DeviceModel::SparkMax,
    })
}

fn firmware_string(firmware: FirmwareVersion) -> JsString {
    js(&format!(
        "{}.{}.{}",
        number_to_string(firmware.major),
        number_to_string(firmware.minor),
        number_to_string(firmware.build)
    ))
}

type PeriodicParser =
    fn(&mut Log, &JsString, f64, &[u8], DeviceModel, FirmwareVersion) -> Result<(), SchemaError>;

const PARSE_PERIODIC: [PeriodicParser; 8] = [
    parse_periodic_0,
    parse_periodic_1,
    parse_periodic_2,
    parse_periodic_3,
    parse_periodic_4,
    parse_periodic_5,
    parse_periodic_6,
    parse_periodic_7,
];

fn parse_periodic_0(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    _model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let applied_output = f64::from(i16le(value, 0)?) / 32767.0;
    let faults = u16le(value, 2)?;
    let sticky_faults = u16le(value, 4)?;
    let bits = u8_at(value, 6)?;

    log.put_number(&child(key, "/AppliedOutput"), timestamp, applied_output);
    let bus_voltage = get_or_default(log, &child(key, "/BusVoltage"), timestamp, 0.0_f64, None);
    if bus_voltage > 0.0 {
        log.put_number(
            &child(key, "/AppliedOutputVoltage"),
            timestamp,
            applied_output * bus_voltage,
        );
    }
    for (index, fault_name) in FAULTS.iter().enumerate() {
        log.put_boolean(
            &child(&child(key, "/Fault/"), fault_name),
            timestamp,
            faults & (1 << index) != 0,
        );
        log.put_boolean(
            &child(&child(key, "/StickyFault/"), fault_name),
            timestamp,
            sticky_faults & (1 << index) != 0,
        );
    }
    log.put_boolean(&child(key, "/SensorInvert"), timestamp, bits & 0x01 != 0);
    log.put_boolean(&child(key, "/MotorInvert"), timestamp, bits & 0x02 != 0);
    log.put_boolean(&child(key, "/IsBrushless"), timestamp, bits & 0x10 != 0);
    log.put_boolean(&child(key, "/IsFollower"), timestamp, bits & 0x20 != 0);
    Ok(())
}

fn parse_periodic_1(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    _model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let motor_velocity = f64::from(f32le(value, 0)?);
    let motor_temperature = f64::from(u8_at(value, 4)?);
    let motor_voltage_raw = u32::from(u8_at(value, 5)?) | (u32::from(u8_at(value, 6)? & 0x0f) << 8);
    let motor_voltage = f64::from(motor_voltage_raw) / 128.0;
    let motor_current_raw = (u32::from(u16le(value, 6)?) >> 4) & 0x0fff;
    let motor_current = f64::from(motor_current_raw) / 32.0;

    log.put_number(&child(key, "/MotorVelocityRPM"), timestamp, motor_velocity);
    if motor_temperature > 0.0 {
        log.put_number(
            &child(key, "/MotorTemperatureC"),
            timestamp,
            motor_temperature,
        );
    }
    log.put_number(&child(key, "/BusVoltage"), timestamp, motor_voltage);
    log.put_number(&child(key, "/MotorCurrentAmps"), timestamp, motor_current);
    Ok(())
}

fn parse_periodic_2(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    model: DeviceModel,
    firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let mut motor_position = f64::from(f32le(value, 0)?);
    let mut i_accum: Option<f64> = None;
    if firmware.major < 24.0 {
        i_accum = Some(f64::from(f32le(value, 4)?));
    } else if model == DeviceModel::SparkFlex {
        motor_position = motor_position.trunc();
        motor_position +=
            sign(motor_position) * (f64::from(u16le(value, 4)?) / f64::from(1u32 << 16));
    }
    log.put_number(
        &child(key, "/MotorPositionRotations"),
        timestamp,
        motor_position,
    );
    if let Some(i_accum) = i_accum {
        log.put_number(&child(key, "/IAccum"), timestamp, i_accum);
    }
    Ok(())
}

fn parse_periodic_3(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let sensor_voltage_raw =
        u32::from(u8_at(value, 0)?) | (u32::from(u8_at(value, 1)? & 0x03) << 8);
    let sensor_voltage = (f64::from(sensor_voltage_raw) / f64::from(1u32 << 8))
        * if model == DeviceModel::SparkFlex {
            5.0 / 3.3
        } else {
            1.0
        };
    let mut sensor_velocity_raw = i64::from((u32le(value, 1)? >> 2) & 0x3fffff);
    if (sensor_velocity_raw >> 21) & 0x01 != 0 {
        sensor_velocity_raw -= 1 << 22;
    }
    let sensor_velocity = sensor_velocity_raw as f64 / 128.0;
    let sensor_position = f64::from(f32le(value, 4)?);

    log.put_number(
        &child(key, "/AnalogSensor/Voltage"),
        timestamp,
        sensor_voltage,
    );
    log.put_number(
        &child(key, "/AnalogSensor/Velocity"),
        timestamp,
        sensor_velocity,
    );
    log.put_number(
        &child(key, "/AnalogSensor/Position"),
        timestamp,
        sensor_position,
    );
    Ok(())
}

fn parse_periodic_4(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let encoder_velocity = f64::from(f32le(value, 0)?);
    let encoder_position = f64::from(f32le(value, 4)?);
    let name = if model == DeviceModel::SparkFlex {
        "ExternalEncoder"
    } else {
        "AlternateEncoder"
    };
    log.put_number(
        &child(&child(&child(key, "/"), name), "/VelocityRPM"),
        timestamp,
        encoder_velocity,
    );
    log.put_number(
        &child(&child(&child(key, "/"), name), "/PositionRotations"),
        timestamp,
        encoder_position,
    );
    Ok(())
}

fn parse_periodic_5(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    _model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let encoder_position = f64::from(f32le(value, 0)?);
    let encoder_absolute_angle = f64::from(u16le(value, 4)?) / f64::from(1u32 << 16);
    log.put_number(
        &child(key, "/AbsoluteEncoder/PositionRotations"),
        timestamp,
        encoder_position,
    );
    log.put_number(
        &child(key, "/AbsoluteEncoder/PositionRotationsNoOffset"),
        timestamp,
        encoder_absolute_angle,
    );
    Ok(())
}

fn parse_periodic_6(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    _model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let encoder_velocity = f64::from(f32le(value, 0)?);
    let encoder_frequency = f64::from(u16le(value, 4)?);
    log.put_number(
        &child(key, "/AbsoluteEncoder/VelocityRPM"),
        timestamp,
        encoder_velocity,
    );
    log.put_number(
        &child(key, "/AbsoluteEncoder/Frequency"),
        timestamp,
        encoder_frequency,
    );
    Ok(())
}

fn parse_periodic_7(
    log: &mut Log,
    key: &JsString,
    timestamp: f64,
    value: &[u8],
    _model: DeviceModel,
    _firmware: FirmwareVersion,
) -> Result<(), SchemaError> {
    let i_accum = f64::from(f32le(value, 0)?);
    log.put_number(&child(key, "/IAccum"), timestamp, i_accum);
    Ok(())
}
