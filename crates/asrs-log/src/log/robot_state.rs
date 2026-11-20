use asrs_js::math;
use asrs_js::string::JsString;

use super::log_core::timestamp_union;
use super::log_key_utils::find_key;
use super::{Log, LogValueSetBoolean};

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

fn ends_with(key: &JsString, suffix: &str) -> bool {
    key.ends_with(&JsString::from(suffix), None)
}

pub fn get_enabled_key(log: &Log) -> Option<JsString> {
    find_key(log, ENABLED_KEYS)
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
    find_key(log, AUTONOMOUS_KEYS)
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
    find_key(log, UTILITY_KEYS)
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
