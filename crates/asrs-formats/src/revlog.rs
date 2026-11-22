//! `@rev-robotics/revlog-converter` 2.0.0 (`dist/lib/revlogParser.js`): converts a REVLib
//! StatusLogger `.revlog` file (format v1 or v2) into WPILOG bytes. Upstream's main process
//! calls `parseREVLOG(logPath, writable)` on `historical-start` and hands the collected chunks
//! to the WPILOG worker (`main/electron/main.ts`).

pub mod dbc;

use std::collections::HashMap;
use std::sync::OnceLock;

use asrs_js::math;
use asrs_js::string::JsError;

use dbc::{CanDecoder, DataType, Dbc, Message, Signal, SignalValue};

/// The DBC files the package ships in `dist/resources`, read with `encoding: "ascii"`.
pub const SPARK_DBC: &str = include_str!("revlog/spark.public.dbc");
pub const SERVO_HUB_DBC: &str = include_str!("revlog/servo_hub.public.dbc");
pub const ENCODER_DBC: &str = include_str!("revlog/encoder.public.dbc");
pub const A301_DBC: &str = include_str!("revlog/a301.public.dbc");

const SUPPORTED_MAJOR: u8 = 2;
const RECORD_PERSISTENT: u64 = 1;
const RECORD_PERIODIC: u64 = 2;
const RECORD_HEADER: u64 = 3;
const RECORD_BUS_TABLE: u64 = 4;
const RECORD_FIRMWARE: u64 = 5;
const RECORD_FRAMES: u64 = 6;
const RECORD_ROBOT_STATE: u64 = 7;
const RECORD_MATCH_INFO: u64 = 8;
const RECORD_LOGGER_EVENT: u64 = 9;
const RECORD_DIAGNOSTICS: u64 = 10;

const MANUFACTURER_REV: u32 = 5;
const MANUFACTURER_A301: u32 = 3;
const MOTOR_CONTROLLER_ID: u32 = 2;
const SERVO_CONTROLLER_ID: u32 = 12;
const ENCODER_ID: u32 = 7;

fn parsed_dbc(cell: &'static OnceLock<Dbc>, text: &'static str) -> &'static Dbc {
    cell.get_or_init(|| Dbc::load(text))
}

/// `(id >> 6) & 0xf` on a JS number: `>>` truncates to int32 first.
fn api_index(id: f64) -> u32 {
    ((id.rem_euclid(4_294_967_296.0) as u32) >> 6) & 0xf
}

/// `periodicFramesOf`: messages in `messages` order whose name starts with the prefix, keyed
/// by api index; later messages win.
fn periodic_frames(dbc: &'static Dbc, prefix: &str) -> HashMap<u32, &'static Message> {
    let mut out = HashMap::new();
    for (_, message) in dbc.messages() {
        if message.name.starts_with(prefix) {
            out.insert(api_index(message.id), message);
        }
    }
    out
}

struct Device {
    name: &'static str,
    v1_prefix: &'static str,
    dbc: &'static Dbc,
    periodic: HashMap<u32, &'static Message>,
    firmware: fn(&[u8]) -> String,
}

/// major (u8), minor (u8), build (u16 big-endian).
fn major_minor_build(data: &[u8]) -> String {
    if data.len() < 4 {
        return "0.0.0".into();
    }
    format!(
        "{}.{}.{}",
        data[0],
        data[1],
        u16::from_be_bytes([data[2], data[3]])
    )
}

fn servo_hub_firmware(data: &[u8]) -> String {
    if data.len() < 3 {
        return "0.0.0".into();
    }
    format!("{}.{}.{}", data[2], data[1], data[0])
}

fn encoder_firmware(data: &[u8]) -> String {
    if data.len() < 6 {
        return "0.0.0".into();
    }
    format!("{}.{}.{}", data[5], data[4], data[3])
}

fn devices() -> &'static [Device] {
    static DEVICES: OnceLock<Vec<Device>> = OnceLock::new();
    static SPARK: OnceLock<Dbc> = OnceLock::new();
    static SERVO_HUB: OnceLock<Dbc> = OnceLock::new();
    static ENCODER: OnceLock<Dbc> = OnceLock::new();
    static A301: OnceLock<Dbc> = OnceLock::new();
    DEVICES.get_or_init(|| {
        let spark = parsed_dbc(&SPARK, SPARK_DBC);
        let servo_hub = parsed_dbc(&SERVO_HUB, SERVO_HUB_DBC);
        let encoder = parsed_dbc(&ENCODER, ENCODER_DBC);
        let a301 = parsed_dbc(&A301, A301_DBC);
        vec![
            Device {
                name: "Spark",
                v1_prefix: "REV/Spark-",
                dbc: spark,
                periodic: periodic_frames(spark, "STATUS_"),
                firmware: major_minor_build,
            },
            Device {
                name: "ServoHub",
                v1_prefix: "REV/ServoHub-",
                dbc: servo_hub,
                periodic: periodic_frames(servo_hub, "STATUS_"),
                firmware: servo_hub_firmware,
            },
            Device {
                name: "Encoder",
                v1_prefix: "REV/Encoder-",
                dbc: encoder,
                periodic: periodic_frames(encoder, "PERIODIC_FRAME_"),
                firmware: encoder_firmware,
            },
            Device {
                name: "A301",
                v1_prefix: "REV/A301-",
                dbc: a301,
                periodic: periodic_frames(a301, "STATUS_"),
                firmware: major_minor_build,
            },
        ]
    })
}

const fn device_key(manufacturer: u32, device_type: u32) -> u32 {
    (manufacturer << 8) | device_type
}

fn device_of(message_id: u32) -> Option<&'static Device> {
    const KEYS: [u32; 4] = [
        device_key(MANUFACTURER_REV, MOTOR_CONTROLLER_ID),
        device_key(MANUFACTURER_REV, SERVO_CONTROLLER_ID),
        device_key(MANUFACTURER_REV, ENCODER_ID),
        device_key(MANUFACTURER_A301, MOTOR_CONTROLLER_ID),
    ];
    let key = device_key((message_id >> 16) & 0xff, (message_id >> 24) & 0x1f);
    KEYS.iter().position(|k| *k == key).map(|i| &devices()[i])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty {
    Boolean,
    Int64,
    Float,
    Double,
    Str,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Boolean => "boolean",
            Ty::Int64 => "int64",
            Ty::Float => "float",
            Ty::Double => "double",
            Ty::Str => "string",
        }
    }
}

/// A JS value passed to `writeRecord`/`writeIfChanged`/`writeRecordFast`.
#[derive(Clone, PartialEq)]
enum Entry {
    Bool(bool),
    Number(f64),
    Big(i128),
    Str(String),
}

impl Entry {
    fn truthy(&self) -> bool {
        match self {
            Entry::Bool(b) => *b,
            Entry::Number(x) => *x != 0.0 && !x.is_nan(),
            Entry::Big(i) => *i != 0,
            Entry::Str(s) => !s.is_empty(),
        }
    }

    fn number(&self) -> f64 {
        match self {
            Entry::Bool(b) => f64::from(*b as u8),
            Entry::Number(x) => *x,
            Entry::Big(i) => *i as f64,
            Entry::Str(_) => 0.0,
        }
    }

    fn int64(&self) -> i64 {
        let value = match self {
            Entry::Big(i) => return i64::try_from(*i).unwrap_or(0),
            Entry::Bool(b) => return i64::from(*b as u8),
            Entry::Str(_) => return 0,
            Entry::Number(x) => math::round(*x),
        };
        // `BigInt(x)` throws for non-finite values and writeBigInt64LE rejects out of range;
        // both land in the catch that writes 0n.
        if !value.is_finite()
            || value < -9_223_372_036_854_775_808.0
            || value >= 9_223_372_036_854_775_808.0
        {
            0
        } else {
            value as i64
        }
    }

    fn string(&self) -> String {
        match self {
            Entry::Str(s) => s.clone(),
            _ => String::new(),
        }
    }
}

impl From<SignalValue> for Entry {
    fn from(value: SignalValue) -> Self {
        match value {
            SignalValue::Number(x) => Entry::Number(x),
            SignalValue::BigInt(i) => Entry::Big(i),
        }
    }
}

#[derive(Clone, Copy)]
struct RecordInfo {
    id: u32,
    ty: Ty,
}

fn required_bytes(value: u64) -> usize {
    if value <= 0xff {
        1
    } else if value <= 0xffff {
        2
    } else if value <= 0xffffff {
        3
    } else {
        4
    }
}

fn required_timestamp_bytes(value: u128) -> usize {
    for bytes in 1..8usize {
        if value < (1u128 << (8 * bytes)) {
            return bytes;
        }
    }
    8
}

/// Node's `RangeError` from `writeBigUInt64LE` on a value >= 2^64: the bigint is rendered in
/// decimal with `_` separators every three digits.
fn out_of_range(value: u128) -> JsError {
    let digits = format!("{value}");
    let mut rendered = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            rendered.push('_');
        }
        rendered.push(c);
    }
    JsError::range(format!(
        "The value of \"value\" is out of range. It must be >= 0n and < 2n ** 64n. Received {rendered}n"
    ))
}

struct State {
    out: Vec<u8>,
    records: Vec<(String, RecordInfo)>,
    fast_records: HashMap<u64, RecordInfo>,
    last_values: Vec<(String, Entry)>,
    bus_names: HashMap<u8, String>,
    dropped_totals: HashMap<u8, i128>,
    last_timestamp_us: u128,
    format_version: u8,
    records_processed: bool,
    next_record_id: u32,
}

impl State {
    fn new() -> State {
        State {
            out: Vec::new(),
            records: Vec::new(),
            fast_records: HashMap::new(),
            last_values: Vec::new(),
            bus_names: HashMap::new(),
            dropped_totals: HashMap::new(),
            last_timestamp_us: 0,
            format_version: 0,
            records_processed: false,
            next_record_id: 1,
        }
    }

    /// `writeVariableInt`: `writeBigUInt64LE` throws a RangeError for a value >= 2^64.
    fn write_variable_int(&mut self, value: u128, length: usize) -> Result<(), JsError> {
        if value > u64::MAX as u128 {
            return Err(out_of_range(value));
        }
        let bytes = (value as u64).to_le_bytes();
        self.out.extend_from_slice(&bytes[..length]);
        Ok(())
    }

    fn write_control_record(&mut self, entry_id: u32, name: &str, ty: Ty, metadata: &str) {
        let mut payload = vec![0u8];
        payload.extend_from_slice(&entry_id.to_le_bytes());
        payload.extend_from_slice(&(name.len() as u32).to_le_bytes());
        payload.extend_from_slice(name.as_bytes());
        let ty_name = ty.name();
        payload.extend_from_slice(&(ty_name.len() as u32).to_le_bytes());
        payload.extend_from_slice(ty_name.as_bytes());
        payload.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        payload.extend_from_slice(metadata.as_bytes());
        let size_len = required_bytes(payload.len() as u64);
        self.out.push(((size_len - 1) as u8) << 2);
        let _ = self.write_variable_int(0, 1);
        let _ = self.write_variable_int(payload.len() as u128, size_len);
        let _ = self.write_variable_int(0, 1);
        self.out.extend_from_slice(&payload);
        set(
            &mut self.records,
            name.to_owned(),
            RecordInfo { id: entry_id, ty },
        );
    }

    fn write_record_fast(
        &mut self,
        info: RecordInfo,
        value: &Entry,
        timestamp_us: u128,
    ) -> Result<(), JsError> {
        let payload: Vec<u8> = match info.ty {
            Ty::Boolean => vec![u8::from(value.truthy())],
            Ty::Int64 => value.int64().to_le_bytes().to_vec(),
            Ty::Float => (value.number() as f32).to_le_bytes().to_vec(),
            Ty::Double => value.number().to_le_bytes().to_vec(),
            Ty::Str => value.string().into_bytes(),
        };
        let id_len = required_bytes(u64::from(info.id));
        let size_len = required_bytes(payload.len() as u64);
        let ts_len = required_timestamp_bytes(timestamp_us);
        let bitfield =
            ((id_len - 1) as u8) | (((size_len - 1) as u8) << 2) | (((ts_len - 1) as u8) << 4);
        self.out.push(bitfield);
        self.write_variable_int(u128::from(info.id), id_len)?;
        self.write_variable_int(payload.len() as u128, size_len)?;
        self.write_variable_int(timestamp_us, ts_len)?;
        self.out.extend_from_slice(&payload);
        Ok(())
    }

    fn write_record(
        &mut self,
        name: &str,
        ty: Ty,
        value: &Entry,
        timestamp_us: u128,
        metadata: &str,
    ) -> Result<(), JsError> {
        if get(&self.records, name).is_none() {
            let id = self.next_record_id;
            self.next_record_id += 1;
            self.write_control_record(id, name, ty, metadata);
        }
        let info = *get(&self.records, name).unwrap();
        self.write_record_fast(info, value, timestamp_us)
    }

    fn write_if_changed(
        &mut self,
        name: &str,
        ty: Ty,
        value: Entry,
        timestamp_us: u128,
    ) -> Result<(), JsError> {
        if get(&self.last_values, name) == Some(&value) {
            return Ok(());
        }
        set(&mut self.last_values, name.to_owned(), value.clone());
        self.write_record(name, ty, &value, timestamp_us, "")
    }

    fn bus_name(&self, bus: u8) -> String {
        self.bus_names
            .get(&bus)
            .cloned()
            .unwrap_or_else(|| format!("CAN_{bus}"))
    }

    fn device_path(&self, device: &Device, bus: u8, number: u32) -> String {
        if self.format_version >= 2 {
            format!("REV/{}/{}-{number}", self.bus_name(bus), device.name)
        } else {
            format!("{}{number}", device.v1_prefix)
        }
    }

    fn write_firmware(&mut self, bus: u8, message_id: u32, data: &[u8]) -> Result<(), JsError> {
        let Some(device) = device_of(message_id) else {
            return Ok(());
        };
        let name = format!(
            "{}/FIRMWARE",
            self.device_path(device, bus, message_id & 0x3f)
        );
        let timestamp_us = if self.format_version >= 2 {
            self.last_timestamp_us
        } else {
            0
        };
        self.write_if_changed(
            &name,
            Ty::Str,
            Entry::Str((device.firmware)(data)),
            timestamp_us,
        )
    }

    /// The `try { decode + writes } catch {}` of `writeFrame`: decode misses and every error
    /// (including the timestamp RangeError, which leaves the partial record in the output)
    /// end the frame early.
    fn write_frame(&mut self, bus: u8, message_id: u32, data: &[u8], timestamp_us: u128) {
        let Some(device) = device_of(message_id) else {
            return;
        };
        let Some(spec) = device.periodic.get(&(message_id >> 6 & 0xf)) else {
            return;
        };
        // Upstream pre-pads to messageSpec.dlc; CanDecoder.decode pads to max(dlc, 8) itself.
        let _ = self.write_frame_inner(bus, message_id, device, spec, data, timestamp_us);
    }

    fn write_frame_inner(
        &mut self,
        bus: u8,
        message_id: u32,
        device: &'static Device,
        spec: &'static Message,
        data: &[u8],
        timestamp_us: u128,
    ) -> Result<(), JsError> {
        let Some(decoded) = CanDecoder::new(device.dbc).decode(spec.id, data) else {
            return Ok(());
        };
        for (index, (name, value)) in decoded.iter().enumerate() {
            let fast_key = ((bus as u64) * 0x20000000 + u64::from(message_id)) * 256 + index as u64;
            let info = match self.fast_records.get(&fast_key) {
                Some(info) => *info,
                None => {
                    let Some(signal_spec) = spec.signal(name) else {
                        continue;
                    };
                    let folder = if name.ends_with("FAULT") {
                        "/FAULT/"
                    } else if name.ends_with("WARNING") {
                        "/WARNING/"
                    } else {
                        "/"
                    };
                    let entry_name = format!(
                        "{}{}{name}",
                        self.device_path(device, bus, message_id & 0x3f),
                        folder,
                    );
                    let ty = wpilog_type(signal_spec);
                    let metadata = signal_spec
                        .description
                        .clone()
                        .unwrap_or_else(|| signal_spec.unit.clone());
                    let id = self.next_record_id;
                    self.next_record_id += 1;
                    self.write_control_record(id, &entry_name, ty, &metadata);
                    let info = RecordInfo { id, ty };
                    self.fast_records.insert(fast_key, info);
                    info
                }
            };
            self.write_record_fast(info, &Entry::from(*value), timestamp_us)?;
        }
        Ok(())
    }

    fn read_u32(payload: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(payload[at..at + 4].try_into().unwrap())
    }

    fn read_u64(payload: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(payload[at..at + 8].try_into().unwrap())
    }

    /// `readShortString`: one length byte, then up to that many bytes as lossy UTF-8; the
    /// cursor advances by `1 + n` regardless of where the payload ends.
    fn short_string(payload: &[u8], at: usize) -> (String, usize) {
        let n = payload[at] as usize;
        let end = (at + 1 + n).min(payload.len());
        (
            String::from_utf8_lossy(&payload[at + 1..end]).into_owned(),
            at + 1 + n,
        )
    }

    fn handle_header(&mut self, payload: &[u8]) -> Result<(), JsError> {
        let magic = payload
            .get(..6)
            .unwrap_or(&[])
            .iter()
            .map(|b| b & 0x7f)
            .eq(b"REVLOG".iter().copied());
        if payload.len() < 8 || !magic {
            return Err(JsError::new(
                "Error",
                "Corrupted file: invalid REVLOG header.",
            ));
        }
        let major = payload[6];
        let minor = payload[7];
        if major > SUPPORTED_MAJOR {
            return Err(JsError::new(
                "Error",
                format!(
                    "This REVLOG uses format version {major}.{minor}, which this converter can't read. Update @rev-robotics/revlog-converter (or AdvantageScope) to open it."
                ),
            ));
        }
        self.format_version = major;
        if payload.len() >= 16 {
            self.last_timestamp_us = u128::from(Self::read_u64(payload, 8));
        }
        Ok(())
    }

    fn handle_v1_persistent(&mut self, payload: &[u8]) -> Result<(), JsError> {
        let mut pc = 0;
        while pc + 10 <= payload.len() {
            self.write_firmware(0, Self::read_u32(payload, pc), &payload[pc + 4..pc + 10])?;
            pc += 10;
        }
        Ok(())
    }

    fn handle_v1_periodic(&mut self, payload: &[u8]) {
        let mut pc = 0;
        while pc + 16 <= payload.len() {
            let timestamp_us = u128::from(Self::read_u32(payload, pc)) * 1000;
            self.write_frame(
                0,
                Self::read_u32(payload, pc + 4),
                &payload[pc + 8..pc + 16],
                timestamp_us,
            );
            pc += 16;
        }
    }

    fn handle_bus_table(&mut self, payload: &[u8]) {
        let mut pc = 0;
        while pc + 2 <= payload.len() {
            let bus = payload[pc];
            let (name, next) = Self::short_string(payload, pc + 1);
            self.bus_names.insert(bus, name);
            pc = next;
        }
    }

    fn handle_firmware(&mut self, payload: &[u8]) -> Result<(), JsError> {
        let mut pc = 0;
        while pc + 6 <= payload.len() {
            let bus = payload[pc];
            let message_id = Self::read_u32(payload, pc + 1);
            let n = payload[pc + 5] as usize;
            if pc + 6 + n > payload.len() {
                break;
            }
            self.write_firmware(bus, message_id, &payload[pc + 6..pc + 6 + n])?;
            pc += 6 + n;
        }
        Ok(())
    }

    fn handle_frames(&mut self, payload: &[u8]) {
        if payload.len() < 9 {
            return;
        }
        let base = u128::from(Self::read_u64(payload, 0));
        let bus = payload[8];
        let mut pc = 9;
        while pc + 7 <= payload.len() {
            let timestamp_us =
                base + u128::from(u16::from_le_bytes([payload[pc], payload[pc + 1]]));
            let message_id = Self::read_u32(payload, pc + 2);
            let n = payload[pc + 6] as usize;
            if pc + 7 + n > payload.len() {
                break;
            }
            self.write_frame(bus, message_id, &payload[pc + 7..pc + 7 + n], timestamp_us);
            if timestamp_us > self.last_timestamp_us {
                self.last_timestamp_us = timestamp_us;
            }
            pc += 7 + n;
        }
    }

    fn handle_robot_state(&mut self, payload: &[u8]) -> Result<(), JsError> {
        if payload.len() < 16 {
            return Ok(());
        }
        let timestamp_us = u128::from(Self::read_u64(payload, 0));
        let word = Self::read_u64(payload, 8);
        self.last_timestamp_us = timestamp_us;
        self.write_if_changed(
            "DS:controlWord/enabled",
            Ty::Boolean,
            Entry::Bool((word >> 58) & 1 == 1),
            timestamp_us,
        )?;
        self.write_if_changed(
            "DS:controlWord/robotMode",
            Ty::Str,
            Entry::Str(
                ["unknown", "autonomous", "teleoperated", "utility"][((word >> 56) & 3) as usize]
                    .to_owned(),
            ),
            timestamp_us,
        )
    }

    fn handle_match_info(&mut self, payload: &[u8]) -> Result<(), JsError> {
        if payload.len() < 13 {
            return Ok(());
        }
        let timestamp_us = u128::from(Self::read_u64(payload, 0));
        let (event_name, _) = Self::short_string(payload, 12);
        self.write_if_changed(
            "DS:matchInfo/eventName",
            Ty::Str,
            Entry::Str(event_name),
            timestamp_us,
        )?;
        self.write_if_changed(
            "DS:matchInfo/matchType",
            Ty::Int64,
            Entry::Big(i128::from(payload[8])),
            timestamp_us,
        )?;
        self.write_if_changed(
            "DS:matchInfo/matchNumber",
            Ty::Int64,
            Entry::Big(i128::from(u16::from_le_bytes([payload[9], payload[10]]))),
            timestamp_us,
        )
    }

    fn handle_logger_event(&mut self, payload: &[u8]) -> Result<(), JsError> {
        if payload.len() < 10 {
            return Ok(());
        }
        let timestamp_us = u128::from(Self::read_u64(payload, 0));
        let code = payload[8];
        let (detail, _) = Self::short_string(payload, 9);
        let name = match code {
            1 => "started".to_owned(),
            2 => "capturing".to_owned(),
            3 => "stopped".to_owned(),
            4 => "resumed".to_owned(),
            5 => "usb-removed".to_owned(),
            6 => "directory-changed".to_owned(),
            _ => format!("event-{code}"),
        };
        let value = if detail.is_empty() {
            name
        } else {
            format!("{name}: {detail}")
        };
        self.write_record(
            "REV/StatusLogger/Event",
            Ty::Str,
            &Entry::Str(value),
            timestamp_us,
            "",
        )
    }

    fn handle_diagnostics(&mut self, payload: &[u8]) -> Result<(), JsError> {
        if payload.len() < 8 {
            return Ok(());
        }
        let timestamp_us = u128::from(Self::read_u64(payload, 0));
        let mut pc = 8;
        while pc + 6 <= payload.len() {
            let bus = payload[pc];
            let kind = payload[pc + 1];
            let count = i128::from(Self::read_u32(payload, pc + 2));
            if kind == 1 {
                let total = self.dropped_totals.get(&bus).copied().unwrap_or(0) + count;
                self.dropped_totals.insert(bus, total);
                let name = format!("REV/StatusLogger/{}/DroppedFrames", self.bus_name(bus));
                self.write_record(&name, Ty::Int64, &Entry::Big(total), timestamp_us, "")?;
            }
            pc += 6;
        }
        Ok(())
    }

    fn handle_record(&mut self, entry_id: u64, payload: &[u8]) -> Result<(), JsError> {
        if self.format_version == 0 {
            self.format_version = if entry_id == RECORD_HEADER { 2 } else { 1 };
        }
        if self.format_version == 1 {
            if entry_id == RECORD_PERSISTENT {
                self.records_processed = true;
                self.handle_v1_persistent(payload)?;
            } else if entry_id == RECORD_PERIODIC {
                self.records_processed = true;
                self.handle_v1_periodic(payload);
            }
            return Ok(());
        }
        self.records_processed = true;
        match entry_id {
            RECORD_HEADER => self.handle_header(payload),
            RECORD_BUS_TABLE => {
                self.handle_bus_table(payload);
                Ok(())
            }
            RECORD_FIRMWARE => self.handle_firmware(payload),
            RECORD_FRAMES => {
                self.handle_frames(payload);
                Ok(())
            }
            RECORD_ROBOT_STATE => self.handle_robot_state(payload),
            RECORD_MATCH_INFO => self.handle_match_info(payload),
            RECORD_LOGGER_EVENT => self.handle_logger_event(payload),
            RECORD_DIAGNOSTICS => self.handle_diagnostics(payload),
            _ => Ok(()),
        }
    }
}

fn get<'a, V>(map: &'a [(String, V)], key: &str) -> Option<&'a V> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn set<V>(map: &mut Vec<(String, V)>, key: String, value: V) {
    match map.iter_mut().find(|(k, _)| *k == key) {
        Some((_, slot)) => *slot = value,
        None => map.push((key, value)),
    }
}

fn wpilog_type(signal: &Signal) -> Ty {
    if signal.length == 1.0 {
        return Ty::Boolean;
    }
    let is_float = matches!(signal.data_type, DataType::Float | DataType::Double)
        || (signal.factor != 1.0 && signal.factor != 0.0);
    if is_float {
        if signal.length == 32.0 && matches!(signal.data_type, DataType::Float) {
            return Ty::Float;
        }
        return Ty::Double;
    }
    Ty::Int64
}

/// `parseREVLOG(input, writable)` with the whole file as input: `Ok` is every byte written
/// to the writable when the promise resolves, `Err` is the rejection (`name` and `message`).
pub fn convert(input: &[u8]) -> Result<Vec<u8>, JsError> {
    let mut state = State::new();
    state.out.extend_from_slice(b"WPILOG");
    state.out.extend_from_slice(&[0x00, 0x01]);
    state.out.extend_from_slice(&0u32.to_le_bytes());
    let mut cursor = 0usize;
    while cursor < input.len() {
        let bitfield = input[cursor];
        let id_len = usize::from(bitfield & 3) + 1;
        let size_len = usize::from((bitfield >> 2) & 3) + 1;
        if cursor + 1 + id_len + size_len > input.len() {
            break;
        }
        let mut at = cursor + 1;
        let mut entry_id = 0u64;
        for i in 0..id_len {
            entry_id |= u64::from(input[at + i]) << (8 * i);
        }
        at += id_len;
        let mut size = 0u64;
        for i in 0..size_len {
            size |= u64::from(input[at + i]) << (8 * i);
        }
        at += size_len;
        if size > 5 * 1024 * 1024 {
            break;
        }
        let end = at + size as usize;
        if end > input.len() {
            break;
        }
        state.handle_record(entry_id, &input[at..end])?;
        cursor = end;
    }
    if !state.records_processed {
        return Err(JsError::new("Error", "No valid records found."));
    }
    Ok(state.out)
}
