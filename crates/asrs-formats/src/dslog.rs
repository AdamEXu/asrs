//! src/hub/dataSources/dslog/*: the .dslog v4 and .dsevents v4 readers plus the
//! dsLogWorker logic that writes their records into a Log.

use std::collections::BTreeSet;

use asrs_js::math;
use asrs_js::string::{JsString, TextDecoder};
use asrs_log::log::Log;

use crate::worker::{Abort, Response};

const HEADER_SIZE: i64 = 4 + 8 + 8;
const PERIOD_SECS: f64 = 0.02;

fn u8_at(data: &[u8], position: i64) -> Result<u8, Abort> {
    data.get(usize::try_from(position).map_err(|_| Abort::Threw)?)
        .copied()
        .ok_or(Abort::Threw)
}

fn i8_at(data: &[u8], position: i64) -> Result<i8, Abort> {
    Ok(u8_at(data, position)? as i8)
}

fn u16_be_at(data: &[u8], position: i64) -> Result<u16, Abort> {
    Ok(u16::from_be_bytes([
        u8_at(data, position)?,
        u8_at(data, position + 1)?,
    ]))
}

fn i32_be_at(data: &[u8], position: i64) -> Result<i32, Abort> {
    let mut bytes = [0u8; 4];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8_at(data, position + i as i64)?;
    }
    Ok(i32::from_be_bytes(bytes))
}

fn i64_be_at(data: &[u8], position: i64) -> Result<i64, Abort> {
    let mut bytes = [0u8; 8];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8_at(data, position + i as i64)?;
    }
    Ok(i64::from_be_bytes(bytes))
}

fn u64_be_at(data: &[u8], position: i64) -> Result<u64, Abort> {
    Ok(i64_be_at(data, position)? as u64)
}

/// `Uint8Array.prototype.subarray(begin, end)`: negative indexes count from the end and both
/// ends clamp to the length; an empty range stays empty.
fn subarray(data: &[u8], begin: i64, end: i64) -> &[u8] {
    let len = data.len() as i64;
    let clamp = |x: i64| -> usize {
        if x < 0 {
            (len + x).max(0) as usize
        } else {
            x.min(len) as usize
        }
    };
    let (b, e) = (clamp(begin), clamp(end));
    if b > e { &[] } else { &data[b..e] }
}

/// DSUtil.convertLVTime.
pub fn convert_lv_time(seconds: i64, fractional: u64) -> f64 {
    let mut time = -2082826800.0;
    time += seconds as f64;
    time += fractional as f64 / math::pow(2.0, 64.0);
    time
}

/// DSUtil.getPDType.
fn pd_type(id: u8) -> PowerDistributionType {
    match id {
        33 => PowerDistributionType::Rev,
        25 => PowerDistributionType::Ctre,
        _ => PowerDistributionType::None,
    }
}

enum PowerDistributionType {
    Rev,
    Ctre,
    None,
}

/// DSLogReader.DSLogEntry.
pub struct DsLogEntry {
    pub timestamp: f64,
    pub trip_time_ms: f64,
    pub packet_loss: f64,
    pub battery_volts: f64,
    pub cpu_utilization: f64,
    pub brownout: bool,
    pub watchdog: bool,
    pub ds_teleop: bool,
    pub ds_disabled: bool,
    pub robot_teleop: bool,
    pub robot_auto: bool,
    pub robot_disabled: bool,
    pub can_utilization: f64,
    pub wifi_db: f64,
    pub wifi_mb: f64,
    pub power_distribution_currents: Vec<f64>,
}

/// DSLogReader.
pub struct DsLogReader<'a> {
    data: &'a [u8],
}

impl<'a> DsLogReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        DsLogReader { data }
    }

    pub fn version(&self) -> Result<i32, Abort> {
        i32_be_at(self.data, 0)
    }

    pub fn is_supported_version(&self) -> Result<bool, Abort> {
        Ok(self.version()? == 4)
    }

    pub fn timestamp(&self) -> Result<f64, Abort> {
        Ok(convert_lv_time(
            i64_be_at(self.data, 4)?,
            u64_be_at(self.data, 12)?,
        ))
    }

    /// `forEach`: the record loop's try/catch swallows a truncated read and keeps every entry
    /// already delivered; only the version check throws out of the function.
    pub fn for_each(&self, callback: &mut impl FnMut(DsLogEntry)) -> Result<(), Abort> {
        if !self.is_supported_version()? {
            return Err(Abort::Threw);
        }
        let _ = self.records(callback);
        Ok(())
    }

    fn records(&self, callback: &mut impl FnMut(DsLogEntry)) -> Result<(), Abort> {
        let mut position = HEADER_SIZE;
        let mut timestamp = 0.0f64;
        let mut last_battery_volts = 0.0f64;
        loop {
            let mask = u8_at(self.data, position + 5)?;
            let mut battery_volts =
                f64::from(u16_be_at(self.data, position + 2)?) / math::pow(2.0, 8.0);
            if battery_volts > 20.0 {
                battery_volts = last_battery_volts;
            } else {
                last_battery_volts = battery_volts;
            }
            let mut entry = DsLogEntry {
                timestamp,
                trip_time_ms: f64::from(u8_at(self.data, position)?) * 0.5,
                packet_loss: math::min2(
                    math::max2(f64::from(i8_at(self.data, position + 1)?) * 4.0 * 0.01, 0.0),
                    1.0,
                ),
                battery_volts,
                cpu_utilization: f64::from(u8_at(self.data, position + 4)?) * 0.5 * 0.01,
                brownout: (mask & (1 << 7)) == 0,
                watchdog: (mask & (1 << 6)) == 0,
                ds_teleop: (mask & (1 << 5)) == 0,
                ds_disabled: (mask & (1 << 3)) == 0,
                robot_teleop: (mask & (1 << 2)) == 0,
                robot_auto: (mask & (1 << 1)) == 0,
                robot_disabled: (mask & 1) == 0,
                can_utilization: f64::from(u8_at(self.data, position + 6)?) * 0.5 * 0.01,
                wifi_db: f64::from(u8_at(self.data, position + 7)?) * 0.5,
                wifi_mb: f64::from(u16_be_at(self.data, position + 8)?) / math::pow(2.0, 8.0),
                power_distribution_currents: Vec::new(),
            };
            position += 10;
            let pd = pd_type(u8_at(self.data, position + 3)?);
            position += 4;
            let mut currents = Vec::new();
            match pd {
                PowerDistributionType::Rev => {
                    position += 1;
                    let mut rev_booleans = Vec::new();
                    for &byte in subarray(self.data, position, position + 27) {
                        for i in 0..8 {
                            rev_booleans.push((byte & (1 << i)) != 0);
                        }
                    }
                    position += 27;
                    for i in 0..20usize {
                        let read_position = (i / 3) * 32 + (i % 3) * 10;
                        let mut value = 0.0f64;
                        for j in 0..10usize {
                            value += if rev_booleans
                                .get(read_position + j)
                                .copied()
                                .unwrap_or(false)
                            {
                                math::pow(2.0, j as f64)
                            } else {
                                0.0
                            };
                        }
                        currents.push(value / 8.0);
                    }
                    for i in 0..4i64 {
                        currents.push(match self.data.get((position + i) as usize) {
                            Some(&byte) => f64::from(byte) / 16.0,
                            None => f64::NAN,
                        });
                    }
                    position += 4;
                    position += 1;
                }
                PowerDistributionType::Ctre => {
                    position += 1;
                    let mut ctre_booleans = Vec::new();
                    for &byte in subarray(self.data, position, position + 21) {
                        for i in 0..8 {
                            ctre_booleans.push((byte & (1 << i)) != 0);
                        }
                    }
                    for i in 0..16usize {
                        let read_position = (i / 6) * 64 + (i % 6) * 10;
                        let mut value = 0.0f64;
                        for j in 0..8usize {
                            value += if ctre_booleans
                                .get(read_position + j)
                                .copied()
                                .unwrap_or(false)
                            {
                                math::pow(2.0, j as f64)
                            } else {
                                0.0
                            };
                        }
                        currents.push(value / 8.0);
                    }
                    position += 21 + 3;
                }
                PowerDistributionType::None => {}
            }
            entry.power_distribution_currents = currents;
            callback(entry);
            timestamp += PERIOD_SECS;
            if position >= self.data.len() as i64 {
                break;
            }
        }
        Ok(())
    }
}

/// DSEventsReader.DSEventsEntry.
pub struct DsEventsEntry {
    pub timestamp: f64,
    pub text: JsString,
}

/// DSEventsReader.
pub struct DsEventsReader<'a> {
    data: &'a [u8],
    text_decoder: TextDecoder,
}

impl<'a> DsEventsReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        DsEventsReader {
            data,
            text_decoder: TextDecoder::new(Some(&JsString::from("UTF-8")), false, false)
                .expect("UTF-8 always resolves"),
        }
    }

    pub fn version(&self) -> Result<i32, Abort> {
        i32_be_at(self.data, 0)
    }

    pub fn is_supported_version(&self) -> Result<bool, Abort> {
        Ok(self.version()? == 4)
    }

    pub fn timestamp(&self) -> Result<f64, Abort> {
        Ok(convert_lv_time(
            i64_be_at(self.data, 4)?,
            u64_be_at(self.data, 12)?,
        ))
    }

    /// `forEach`. `position` is the loop's only state, so a negative length that brings it back
    /// to a position already read means upstream spins forever; that ends in
    /// `Abort::NeverReturns` instead.
    pub fn for_each(&self, callback: &mut impl FnMut(DsEventsEntry)) -> Result<(), Abort> {
        if !self.is_supported_version()? {
            return Err(Abort::Threw);
        }
        let mut position = HEADER_SIZE;
        let start_time = self.timestamp()?;
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(position) {
                return Err(Abort::NeverReturns);
            }
            let timestamp = convert_lv_time(
                i64_be_at(self.data, position)?,
                u64_be_at(self.data, position + 8)?,
            );
            position += 8 + 8;
            let length = i32_be_at(self.data, position)?;
            position += 4;
            let mut text = self
                .text_decoder
                .decode(subarray(self.data, position, position + i64::from(length)))
                .expect("nonfatal UTF-8 never errors");
            position += i64::from(length);
            let adjusted_timestamp = timestamp - start_time;
            let tags = [
                JsString::from("<TagVersion>"),
                JsString::from("<time>"),
                JsString::from("<count>"),
                JsString::from("<flags>"),
                JsString::from("<Code>"),
                JsString::from("<location>"),
                JsString::from("<stack>"),
            ];
            for tag in &tags {
                while text.includes(tag, None) {
                    let tag_index = text.index_of(tag, None);
                    let next_index =
                        text.index_of(&JsString::from("<"), Some((tag_index + 1) as f64));
                    let mut filtered = text.slice(0.0, Some(tag_index as f64));
                    filtered.push_str(&text.slice(next_index as f64, None));
                    text = filtered;
                }
            }
            text = text
                .replace_all(&JsString::from("<message> "), &JsString::new())
                .expect("the result only shrinks");
            text = text
                .replace_all(&JsString::from("<details> "), &JsString::new())
                .expect("the result only shrinks");
            text = text.trim();
            callback(DsEventsEntry {
                timestamp: adjusted_timestamp,
                text,
            });
            if position >= self.data.len() as i64 {
                break Ok(());
            }
        }
    }
}

/// dsLogWorker's `onmessage` for its `start` request. `files` is `request.data`: a `null` slot
/// is skipped and an absent slot reads `undefined`, which passes the `!== null` check and
/// throws inside the reader constructor.
pub fn run_worker(files: &[Option<&[u8]>], post: &mut impl FnMut(Response)) -> Result<(), Abort> {
    post(Response::Progress(1.0));
    let mut log = Log::new(true);
    match files.first().copied() {
        Some(None) => {}
        Some(Some(data)) => {
            let ds_log = DsLogReader::new(data);
            if !ds_log.is_supported_version()? {
                post(Response::Failed);
                return Ok(());
            }
            ds_log.for_each(&mut |entry| {
                log.put_number(
                    &JsString::from("/DSLog/TripTimeMS"),
                    entry.timestamp,
                    entry.trip_time_ms,
                );
                log.put_number(
                    &JsString::from("/DSLog/PacketLoss"),
                    entry.timestamp,
                    entry.packet_loss,
                );
                log.put_number(
                    &JsString::from("/DSLog/BatteryVoltage"),
                    entry.timestamp,
                    entry.battery_volts,
                );
                log.put_number(
                    &JsString::from("/DSLog/CPUUtilization"),
                    entry.timestamp,
                    entry.cpu_utilization,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/Brownout"),
                    entry.timestamp,
                    entry.brownout,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/Watchdog"),
                    entry.timestamp,
                    entry.watchdog,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/DSTeleop"),
                    entry.timestamp,
                    entry.ds_teleop,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/DSDisabled"),
                    entry.timestamp,
                    entry.ds_disabled,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/RobotTeleop"),
                    entry.timestamp,
                    entry.robot_teleop,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/RobotAuto"),
                    entry.timestamp,
                    entry.robot_auto,
                );
                log.put_boolean(
                    &JsString::from("/DSLog/Status/RobotDisabled"),
                    entry.timestamp,
                    entry.robot_disabled,
                );
                log.put_number(
                    &JsString::from("/DSLog/CANUtilization"),
                    entry.timestamp,
                    entry.can_utilization,
                );
                log.put_number_array(
                    &JsString::from("/DSLog/PowerDistributionCurrents"),
                    entry.timestamp,
                    entry.power_distribution_currents,
                );
            })?;
        }
        None => return Err(Abort::Threw),
    }
    match files.get(1).copied() {
        Some(None) => {}
        Some(Some(data)) => {
            let ds_events = DsEventsReader::new(data);
            if !ds_events.is_supported_version()? {
                post(Response::Failed);
                return Ok(());
            }
            ds_events.for_each(&mut |entry| {
                log.put_string(&JsString::from("/DSEvents"), entry.timestamp, entry.text);
            })?;
        }
        None => return Err(Abort::Threw),
    }
    post(Response::Initial {
        log: log.to_serialized(),
        is_partial: false,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(records: &[(i32, &[u8])]) -> Vec<u8> {
        let mut bytes = 4i32.to_be_bytes().to_vec();
        bytes.extend([0; 16]);
        for &(length, text) in records {
            bytes.extend([0; 16]);
            bytes.extend(length.to_be_bytes());
            bytes.extend(text);
        }
        bytes
    }

    fn worker_end(dsevents: &[u8]) -> (Result<(), Abort>, usize) {
        let mut replies = 0;
        let end = run_worker(&[None, Some(dsevents)], &mut |_| replies += 1);
        (end, replies)
    }

    #[test]
    fn a_length_that_returns_to_a_read_position_never_returns() {
        assert_eq!(
            worker_end(&events(&[(-20, b"")])),
            (Err(Abort::NeverReturns), 1)
        );
        let two_cycle = events(&[(2, b"ab"), (-42, b"")]);
        assert_eq!(worker_end(&two_cycle), (Err(Abort::NeverReturns), 1));
    }

    #[test]
    fn a_length_that_leaves_the_file_throws() {
        assert_eq!(worker_end(&events(&[(-60, b"")])), (Err(Abort::Threw), 1));
    }

    #[test]
    fn records_that_reach_the_end_complete() {
        assert_eq!(worker_end(&events(&[(0, b""), (1, b"x")])), (Ok(()), 2));
    }
}
