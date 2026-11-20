use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::time::{self, Clock, FixedZone};

use super::log_core::timestamp_union;
use super::log_key_utils::find_key;
use super::log_util::get_or_default;
use super::{Log, LogValueSetBoolean};
use crate::match_info::{MatchInfo, MatchType};

pub const ENABLED_KEYS: &[&str] = &[
    "/DriverStation/Enabled",                                   // AdvantageKit
    "NT:/AdvantageKit/DriverStation/Enabled",                   // AdvantageKit
    "DS:controlWord/enabled",                                   // DataLog/Phoenix, post-2027
    "DS:enabled",                                               // DataLog, pre-2027
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/enabled", // FIRST DS
    "/DSLog/Status/DSDisabled",                                 // NI DS
    "RobotEnable",                                              // Phoenix
    "NT:/DriverStation/ControlWord/enabled",                    // NT, post-2027
    "NT:/FMSInfo/FMSControlData",                               // NT, pre-2027
    "RUNNING",                                                  // Roadrunner
];

pub const AUTONOMOUS_KEYS: &[&str] = &[
    "/DriverStation/RobotMode",                  // AdvantageKit, post-2027
    "NT:/AdvantageKit/DriverStation/RobotMode",  // AdvantageKit, post-2027
    "/DriverStation/Autonomous",                 // AdvantageKit, pre-2027
    "NT:/AdvantageKit/DriverStation/Autonomous", // AdvantageKit, pre-2027
    "DS:controlWord/robotMode",                  // DataLog/Phoenix, post-2027
    "DS:autonomous",                             // DataLog, pre-2027
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/robotMode", // FIRST DS
    "/DSLog/Status/DSTeleop",                    // NI DS
    "RobotMode",                                 // Phoenix
    "NT:/DriverStation/ControlWord/robotMode",   // NT, post-2027
    "NT:/FMSInfo/FMSControlData",                // NT, pre-2027
];

pub const UTILITY_KEYS: &[&str] = &[
    "/DriverStation/RobotMode",                 // AdvantageKit, post-2027
    "NT:/AdvantageKit/DriverStation/RobotMode", // AdvantageKit, post-2027
    "/DriverStation/Test",                      // AdvantageKit, pre-2027
    "NT:/AdvantageKit/DriverStation/Test",      // AdvantageKit, pre-2027
    "DS:controlWord/robotMode",                 // DataLog/Phoenix, post-2027
    "DS:test",                                  // DataLog, pre-2027
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/robotMode", // FIRST DS
    "RobotMode",                                // Phoenix
    "NT:/DriverStation/ControlWord/robotMode",  // NT, post-2027
    "NT:/FMSInfo/FMSControlData",               // NT, pre-2027
];

pub const ALLIANCE_KEYS: &[&str] = &[
    "/DriverStation/AllianceStation",                 // AdvantageKit
    "NT:/AdvantageKit/DriverStation/AllianceStation", // AdvantageKit
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/allianceStation", // FIRST DS
    "NT:/DriverStation/IsRedAlliance",                // NT, post-2027
    "NT:/FMSInfo/IsRedAlliance",                      // NT, pre-2027
    "DS:allianceStation",                             // Phoenix
    "AllianceStation",                                // Phoenix
];

pub const DRIVER_STATION_KEYS: &[&str] = &[
    "/DriverStation/AllianceStation",                 // AdvantageKit
    "NT:/AdvantageKit/DriverStation/AllianceStation", // AdvantageKit
    "DS:/Dscomm/Control/Robot/ControlData/ControlWord/allianceStation", // FIRST DS
    "NT:/DriverStation/StationNumber",                // NT, post-2027
    "NT:/FMSInfo/StationNumber",                      // NT, pre-2027
    "DS:allianceStation",                             // Phoenix
    "AllianceStation",                                // Phoenix
];

pub const SYSTEM_TIME_KEYS: &[&str] = &[
    "/SystemStats/EpochTime",                       // AdvantageKit, post-2027
    "NT:/AdvantageKit/SystemStats/EpochTime",       // AdvantageKit, post-2027
    "/SystemStats/EpochTimeMicros",                 // AdvantageKit, pre-2027
    "NT:/AdvantageKit/SystemStats/EpochTimeMicros", // AdvantageKit, pre-2027
    "systemTime",                                   // DataLog
];

pub const AKIT_TIMESTAMP_KEYS: &[&str] = &["/Timestamp", "NT:/AdvantageKit/Timestamp"];

pub const METADATA_KEYS: &[&str] = &[
    "/Metadata",
    "/RealMetadata",
    "/ReplayMetadata",
    "NT:/Metadata",
    "NT:/AdvantageKit/RealMetadata",
    "NT:/AdvantageKit/ReplayMetadata",
];

pub const EVENT_KEYS: &[&str] = &[
    "/DriverStation/EventName",                     // AdvantageKit
    "NT:/AdvantageKit/DriverStation/EventName",     // AdvantageKit
    "DS:/Dscomm/Control/Robot/MatchInfo/EventName", // FIRST DS
    "NT:/DriverStation/EventName",                  // NT, post-2027
    "NT:/FMSInfo/EventName",                        // NT, pre-2027
    "NT:/Netcomm/Control/MatchInfo/EventName",      // Systemcore
    "DS:matchInfo/eventName",                       // Phoenix
];

pub const MATCH_TYPE_KEYS: &[&str] = &[
    "/DriverStation/MatchType",                     // AdvantageKit
    "NT:/AdvantageKit/DriverStation/MatchType",     // AdvantageKit
    "DS:/Dscomm/Control/Robot/MatchInfo/MatchType", // FIRST DS
    "NT:/DriverStation/MatchType",                  // NT, post-2027
    "NT:/FMSInfo/MatchType",                        // NT, pre-2027
    "NT:/Netcomm/Control/MatchInfo/MatchType",      // Systemcore
    "DS:matchInfo/matchType",                       // Phoenix
];

pub const MATCH_NUMBER_KEYS: &[&str] = &[
    "/DriverStation/MatchNumber",                     // AdvantageKit
    "NT:/AdvantageKit/DriverStation/MatchNumber",     // AdvantageKit
    "DS:/Dscomm/Control/Robot/MatchInfo/MatchNumber", // FIRST DS
    "NT:/DriverStation/MatchNumber",                  // NT, post-2027
    "NT:/FMSInfo/MatchNumber",                        // NT, pre-2027
    "NT:/Netcomm/Control/MatchInfo/MatchNumber",      // Systemcore
    "DS:matchInfo/matchNumber",                       // Phoenix
];

pub const PROGRAM_START_TIME_KEYS: &[&str] = &[
    "NT:/Robot/ProgramStartTime",           // DataLog/NT
    "/Robot/ProgramStartTime",              // DataLog/NT
    "DS:/Dscomm/Control/Robot/ControlData", // FIRST DS
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobotMode {
    Disabled,
    Auto,
    Teleop,
    Utility,
}

/// `RobotStateRange`; `end` is `undefined` on the last range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RobotStateRange {
    pub start: f64,
    pub end: Option<f64>,
    pub mode: RobotMode,
}

fn js_strings(keys: &[&str]) -> Vec<JsString> {
    keys.iter().map(|&k| JsString::from(k)).collect()
}

fn ends_with(key: &JsString, suffix: &str) -> bool {
    key.ends_with(&JsString::from(suffix), None)
}

pub fn get_enabled_key(log: &Log) -> Option<JsString> {
    find_key(log, &js_strings(ENABLED_KEYS))
}

/// `getEnabledData`: boolean enabled series, or null when no key or no samples.
pub fn get_enabled_data(log: &Log) -> Option<LogValueSetBoolean> {
    let key = get_enabled_key(log)?;
    if ends_with(&key, "FMSControlData") {
        let data = log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(LogValueSetBoolean {
            timestamps: data.timestamps,
            values: data
                .values
                .iter()
                .map(|w| math::rem(*w, 2.0) == 1.0)
                .collect(),
        })
    } else {
        let data = log.get_boolean(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        if ends_with(&key, "DSDisabled") {
            Some(LogValueSetBoolean {
                timestamps: data.timestamps,
                values: data.values.iter().map(|v| !v).collect(),
            })
        } else {
            Some(data)
        }
    }
}

pub fn get_autonomous_key(log: &Log) -> Option<JsString> {
    find_key(log, &js_strings(AUTONOMOUS_KEYS))
}

/// `getAutonomousData`: boolean autonomous series, or null when no key or no samples.
pub fn get_autonomous_data(log: &Log) -> Option<LogValueSetBoolean> {
    let key = get_autonomous_key(log)?;
    if ends_with(&key, "FMSControlData") {
        let data = log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(LogValueSetBoolean {
            timestamps: data.timestamps,
            values: data
                .values
                .iter()
                .map(|w| (math::to_int32(*w) >> 1) & 1 != 0)
                .collect(),
        })
    } else if key
        .to_lower_case()
        .ends_with(&JsString::from("robotmode"), None)
    {
        let data = log.get_string(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(LogValueSetBoolean {
            timestamps: data.timestamps,
            values: data
                .values
                .iter()
                .map(|text| text.to_lower_case() == JsString::from("autonomous"))
                .collect(),
        })
    } else {
        let data = log.get_boolean(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        if ends_with(&key, "DSTeleop") {
            Some(LogValueSetBoolean {
                timestamps: data.timestamps,
                values: data.values.iter().map(|v| !v).collect(),
            })
        } else {
            Some(data)
        }
    }
}

pub fn get_utility_key(log: &Log) -> Option<JsString> {
    find_key(log, &js_strings(UTILITY_KEYS))
}

/// `getUtilityData`: boolean utility series, or null when no key or no samples.
pub fn get_utility_data(log: &Log) -> Option<LogValueSetBoolean> {
    let key = get_utility_key(log)?;
    if ends_with(&key, "FMSControlData") {
        let data = log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(LogValueSetBoolean {
            timestamps: data.timestamps,
            values: data
                .values
                .iter()
                .map(|w| (math::to_int32(*w) >> 2) & 1 != 0)
                .collect(),
        })
    } else if key
        .to_lower_case()
        .ends_with(&JsString::from("robotmode"), None)
    {
        let data = log.get_string(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(LogValueSetBoolean {
            timestamps: data.timestamps,
            values: data
                .values
                .iter()
                .map(|text| {
                    let lower = text.to_lower_case();
                    lower == JsString::from("utility") || lower == JsString::from("test")
                })
                .collect(),
        })
    } else {
        let data = log.get_boolean(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(data)
    }
}

/// `values.findLast((_, i) => timestamps[i] <= t)`, defaulting to `false`.
fn find_last(set: &LogValueSetBoolean, timestamp: f64) -> bool {
    set.timestamps
        .iter()
        .rposition(|t| *t <= timestamp)
        .is_some_and(|i| set.values[i])
}

/// `getRobotStateRanges`: contiguous state ranges merged across the enabled,
/// autonomous and utility series.
pub fn get_robot_state_ranges(log: &Log) -> Vec<RobotStateRange> {
    let enabled_data = get_enabled_data(log);
    let auto_data = get_autonomous_data(log);
    let utility_data = get_utility_data(log);
    let Some(enabled_data) = enabled_data else {
        return Vec::new();
    };
    if get_autonomous_key(log).is_some() && auto_data.is_none() {
        return Vec::new();
    }
    if get_utility_key(log).is_some() && utility_data.is_none() {
        return Vec::new();
    }
    let auto_data = auto_data.unwrap_or_default();
    let utility_data = utility_data.unwrap_or_default();

    let all_timestamps = timestamp_union(
        [
            enabled_data.timestamps.as_slice(),
            auto_data.timestamps.as_slice(),
            utility_data.timestamps.as_slice(),
        ]
        .into_iter(),
    );
    let combined: Vec<RobotStateRange> = all_timestamps
        .iter()
        .map(|&timestamp| RobotStateRange {
            start: timestamp,
            end: None,
            mode: if find_last(&enabled_data, timestamp) {
                if find_last(&auto_data, timestamp) {
                    RobotMode::Auto
                } else if find_last(&utility_data, timestamp) {
                    RobotMode::Utility
                } else {
                    RobotMode::Teleop
                }
            } else {
                RobotMode::Disabled
            },
        })
        .collect();

    let mut ranges: Vec<RobotStateRange> = Vec::new();
    for (index, sample) in combined.iter().enumerate() {
        let end = (index + 1 < combined.len()).then(|| combined[index + 1].start);
        if let Some(last) = ranges.last_mut()
            && last.mode == sample.mode
        {
            last.end = end;
        } else {
            ranges.push(RobotStateRange {
                start: sample.start,
                end,
                mode: sample.mode,
            });
        }
    }
    ranges
}

/// `getProgramStartTime`: the last `ProgramStartTime` sample (nanoseconds) in
/// seconds, or the FIRST DS control data's first timestamp.
pub fn get_program_start_time(log: &Log) -> Option<f64> {
    let key = find_key(log, &js_strings(PROGRAM_START_TIME_KEYS))?;
    if ends_with(&key, "ProgramStartTime") {
        let data = log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)?;
        if data.timestamps.is_empty() {
            return None;
        }
        Some(data.values[data.values.len() - 1] / 1e9)
    } else {
        let data = log.get_range(&key, f64::NEG_INFINITY, f64::INFINITY, None)?;
        if data.is_empty() {
            return None;
        }
        Some(data.timestamps()[0])
    }
}

/// `getIsRedAlliance`: whether the robot is on the red alliance at `time`.
pub fn get_is_red_alliance(log: &Log, time: f64) -> bool {
    let Some(key) = find_key(log, &js_strings(ALLIANCE_KEYS)) else {
        return false;
    };
    if ends_with(&key, "DriverStation/AllianceStation") {
        if let Some(data) = log.get_number(&key, time, time, None, None)
            && let Some(&value) = data.values.last()
        {
            return value <= 3.0 && value > 0.0;
        }
    } else if ends_with(&key, "AllianceStation") || ends_with(&key, "allianceStation") {
        if let Some(data) = log.get_string(&key, time, time, None, None)
            && let Some(value) = data.values.last()
        {
            return value.starts_with(&JsString::from("Red"), None);
        }
    } else if let Some(data) = log.get_boolean(&key, time, time, None, None)
        && let Some(&value) = data.values.last()
    {
        return value;
    }
    false
}

/// `getDriverStation`: the driver station number (0-5) at `time`, -1 when unknown.
pub fn get_driver_station(log: &Log, time: f64) -> f64 {
    let Some(key) = find_key(log, &js_strings(DRIVER_STATION_KEYS)) else {
        return -1.0;
    };
    if ends_with(&key, "DriverStation/AllianceStation") {
        if let Some(data) = log.get_number(&key, time, time, None, None)
            && let Some(&value) = data.values.last()
        {
            if value == 1.0 {
                return 3.0;
            } else if value == 2.0 {
                return 4.0;
            } else if value == 3.0 {
                return 5.0;
            } else if value == 4.0 {
                return 0.0;
            } else if value == 5.0 {
                return 1.0;
            } else if value == 6.0 {
                return 2.0;
            }
        }
    } else if ends_with(&key, "StationNumber") {
        if let Some(data) = log.get_number(&key, time, time, None, None)
            && let Some(&value) = data.values.last()
        {
            if get_is_red_alliance(log, time) {
                if value == 1.0 {
                    return 3.0;
                } else if value == 2.0 {
                    return 4.0;
                } else if value == 3.0 {
                    return 5.0;
                }
            } else if value == 1.0 {
                return 0.0;
            } else if value == 2.0 {
                return 1.0;
            } else if value == 3.0 {
                return 2.0;
            }
        }
    } else if (ends_with(&key, "AllianceStation") || ends_with(&key, "allianceStation"))
        && let Some(data) = log.get_string(&key, time, time, None, None)
        && let Some(value) = data.values.last()
    {
        if *value == JsString::from("Blue 1") {
            return 0.0;
        } else if *value == JsString::from("Blue 2") {
            return 1.0;
        } else if *value == JsString::from("Blue 3") {
            return 2.0;
        } else if *value == JsString::from("Red 1") {
            return 3.0;
        } else if *value == JsString::from("Red 2") {
            return 4.0;
        } else if *value == JsString::from("Red 3") {
            return 5.0;
        }
    }
    -1.0
}

/// `new Date(ms).getUTCFullYear()`: NaN on an invalid date.
fn utc_year(epoch_ms: f64) -> f64 {
    time::epoch_to_civil(epoch_ms, &FixedZone::UTC).map_or(f64::NAN, |c| f64::from(c.year))
}

/// `getMatchInfo`: match metadata read at the first enabled timestamp, or null
/// when the robot was never enabled or the log is not a match.
pub fn get_match_info(log: &Log, clock: &dyn Clock) -> Option<MatchInfo> {
    let enabled_data = get_enabled_data(log);
    let mut enabled_time: Option<f64> = None;
    if let Some(data) = &enabled_data {
        for (index, &enabled) in data.values.iter().enumerate() {
            if enabled_time.is_none() && enabled {
                enabled_time = Some(data.timestamps[index]);
            }
        }
    }
    let enabled_time = enabled_time?;

    let system_time_key = find_key(log, &js_strings(SYSTEM_TIME_KEYS));
    let event_key = find_key(log, &js_strings(EVENT_KEYS));
    let match_type_key = find_key(log, &js_strings(MATCH_TYPE_KEYS));
    let match_number_key = find_key(log, &js_strings(MATCH_NUMBER_KEYS));
    let (Some(event_key), Some(match_type_key), Some(match_number_key)) =
        (event_key, match_type_key, match_number_key)
    else {
        return None;
    };

    let mut info = MatchInfo {
        year: 2022.0,
        event: JsString::new(),
        match_type: MatchType::Qualification,
        match_number: 1.0,
    };
    if let Some(key) = system_time_key {
        let epoch = get_or_default(log, &key, enabled_time, info.year, None);
        let epoch_millis = if ends_with(&key, "Micros") {
            epoch / 1.0e3
        } else {
            epoch / 1.0e6
        };
        info.year = utc_year(epoch_millis);
        if info.year < 2022.0 {
            info.year = utc_year(clock.now());
        }
    }
    info.event = get_or_default(log, &event_key, enabled_time, info.event, None);
    let match_type = get_or_default(log, &match_type_key, enabled_time, 0.0, None);
    info.match_type = MatchType::from_number(match_type)?;
    info.match_number = get_or_default(
        log,
        &match_number_key,
        enabled_time,
        info.match_number,
        None,
    );
    Some(info)
}
