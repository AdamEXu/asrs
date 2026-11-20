//! `SourceList.updatePreview`'s text.

use asrs_js::number;
use asrs_js::string::JsString;
use asrs_log::geometry::{
    self, GeometryError, ModuleVelocitySample, RotationUnits, rotation_3d_to_2d, rotation_3d_to_rpy,
};
use asrs_log::log::log_util::{LogValueText, get_log_value_text, get_value_at};
use asrs_log::log::mechanism_state::get_mechanism_state;
use asrs_log::log::{Log, LogValue, LoggableType, RawValue};
use asrs_log::units;

use super::{SourceListError, eq_str};
use crate::source_list_config::{PreviewSetting, PreviewType, SourceListItemState};

pub type NumberPreview<'a> = &'a mut dyn FnMut(&JsString, f64) -> Option<String>;

fn fixed(x: f64, digits: f64) -> String {
    number::to_fixed(x, digits).expect("fraction digits are in range")
}

fn degrees(radians: f64) -> f64 {
    units::convert(radians, "radians", "degrees").expect("radians and degrees are units")
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

struct Value {
    text: String,
    length: Option<usize>,
}

fn sample<T: LogValue + LogValueText>(
    log: &Log,
    key: &JsString,
    time: f64,
    length: impl Fn(&T) -> Option<usize>,
) -> Option<Value> {
    let value: T = get_value_at(log, key, time, None)?;
    Some(Value {
        text: get_log_value_text(&value),
        length: length(&value),
    })
}

/// `getOrDefault(log, key, type, time, null)` for a field of type `log_type`.
fn value_at(log: &Log, key: &JsString, log_type: LoggableType, time: f64) -> Option<Value> {
    match log_type {
        LoggableType::Raw => sample::<RawValue>(log, key, time, |_| None),
        LoggableType::Boolean => sample::<bool>(log, key, time, |_| None),
        LoggableType::Number => sample::<f64>(log, key, time, |_| None),
        LoggableType::String => sample::<JsString>(log, key, time, |_| None),
        LoggableType::BooleanArray => sample::<Vec<bool>>(log, key, time, |v| Some(v.len())),
        LoggableType::NumberArray => sample::<Vec<f64>>(log, key, time, |v| Some(v.len())),
        LoggableType::StringArray => sample::<Vec<JsString>>(log, key, time, |v| Some(v.len())),
        LoggableType::Empty => None,
    }
}

fn string_array_length(log: &Log, key: &JsString, suffix: &str, time: f64) -> usize {
    let mut child = key.clone();
    child.push_str(&JsString::from(suffix));
    get_value_at::<Vec<JsString>>(log, &child, time, None).map_or(0, |v| v.len())
}

fn geometry_error(error: GeometryError) -> SourceListError {
    match error {
        GeometryError::RangeError(message) => {
            SourceListError::Thrown(format!("RangeError: {message}"))
        }
        GeometryError::TypeError(message) => {
            SourceListError::Thrown(format!("TypeError: {message}"))
        }
    }
}

fn pose_strings(
    log: &Log,
    state: &SourceListItemState,
    preview: PreviewType,
    time: f64,
    uuid: &str,
) -> Result<Vec<String>, SourceListError> {
    match preview {
        PreviewType::ModuleVelocities => {
            geometry::grab_module_velocities(log, &state.log_key, time, None, Some(uuid))
                .into_iter()
                .map(|sample| {
                    let (velocity, angle) = match sample {
                        ModuleVelocitySample::Full(v) => (v.velocity, v.angle),
                        ModuleVelocitySample::Partial { velocity, angle } => (
                            velocity
                                .ok_or_else(|| SourceListError::type_error("null", "toFixed"))?,
                            angle.unwrap_or(0.0),
                        ),
                        ModuleVelocitySample::Undefined => {
                            return Err(SourceListError::type_error("undefined", "velocity"));
                        }
                    };
                    Ok(format!(
                        "\u{03bd}: {}m/s, \u{03b8}: {}\u{00b0}",
                        fixed(velocity, 2.0),
                        fixed(degrees(angle), 2.0)
                    ))
                })
                .collect()
        }
        PreviewType::RobotVelocities => {
            let v = geometry::grab_robot_velocities(log, &state.log_key, time, Some(uuid));
            Ok(vec![format!(
                "\u{03bd}x: {}m/s, \u{03bd}y: {}m/s, \u{03a9}: {}\u{00b0}/s",
                fixed(v.vx, 2.0),
                fixed(v.vy, 2.0),
                fixed(degrees(v.omega), 2.0)
            )])
        }
        _ => {
            let units = match state.options.get("units") {
                Some(u) if eq_str(u, "degrees") => RotationUnits::Degrees,
                _ => RotationUnits::Radians,
            };
            let poses = geometry::grab_poses_auto(
                log,
                &state.log_key,
                &state.log_type,
                time,
                Some(uuid),
                units,
            )
            .map_err(geometry_error)?;
            Ok(poses
                .iter()
                .map(|annotated| {
                    let t = annotated.pose.translation;
                    let r = annotated.pose.rotation;
                    let rpy = || {
                        let rpy = rotation_3d_to_rpy(r);
                        format!(
                            "Roll: {}\u{00b0}, Pitch: {}\u{00b0}, Yaw: {}\u{00b0}",
                            fixed(degrees(rpy[0]), 2.0),
                            fixed(degrees(rpy[1]), 2.0),
                            fixed(degrees(rpy[2]), 2.0)
                        )
                    };
                    match preview {
                        PreviewType::Rotation2d => {
                            format!("{}\u{00b0}", fixed(degrees(rotation_3d_to_2d(r)), 2.0))
                        }
                        PreviewType::Translation2d => {
                            format!("X: {}m, Y: {}m", fixed(t[0], 3.0), fixed(t[1], 3.0))
                        }
                        PreviewType::Pose2d | PreviewType::Transform2d => format!(
                            "X: {}m, Y: {}m, \u{03b8}: {}\u{00b0}",
                            fixed(t[0], 3.0),
                            fixed(t[1], 3.0),
                            fixed(degrees(rotation_3d_to_2d(r)), 2.0)
                        ),
                        PreviewType::Rotation3d => rpy(),
                        PreviewType::Translation3d => format!(
                            "X: {}m, Y: {}m, Z: {}m",
                            fixed(t[0], 3.0),
                            fixed(t[1], 3.0),
                            fixed(t[2], 3.0)
                        ),
                        PreviewType::Pose3d => format!(
                            "X: {}m, Y: {}m, Z: {}m, {}",
                            fixed(t[0], 3.0),
                            fixed(t[1], 3.0),
                            fixed(t[2], 3.0),
                            rpy()
                        ),
                        _ => String::new(),
                    }
                })
                .collect())
        }
    }
}

/// The preview line for `state`, or `None` when the item shows none. `available` is
/// `isFieldAvailable(state)`; `preview` is the item type's setting (`Unset` when the type is
/// not in the config).
pub(super) fn preview_text(
    log: &Log,
    state: &SourceListItemState,
    available: bool,
    preview: PreviewSetting,
    time: Option<f64>,
    uuid: &str,
    number_preview: &mut Option<NumberPreview<'_>>,
) -> Result<Option<String>, SourceListError> {
    let Some(time) = time.filter(|_| available) else {
        return Ok(None);
    };
    let Some(log_type) = log.get_type(&state.log_key) else {
        return Ok(None);
    };
    let structured_type = log.get_structured_type(&state.log_key);
    let mut value = None;
    if log_type == LoggableType::Number
        && let Some(number_preview) = number_preview
    {
        return Ok(number_preview(&state.log_key, time));
    } else if log_type != LoggableType::Empty {
        value = value_at(log, &state.log_key, log_type, time);
    }
    if value.is_none() && log_type != LoggableType::Empty {
        return Ok(None);
    }
    match preview {
        PreviewSetting::Disabled => Ok(None),
        PreviewSetting::Type(preview) => {
            let strings = pose_strings(log, state, preview, time, uuid)?;
            Ok(Some(match strings.len() {
                0 => "No values".to_owned(),
                1 => strings.into_iter().next().unwrap_or_default(),
                n => format!(
                    "{} \u{2014} [{}]",
                    plural(n, "value"),
                    strings
                        .iter()
                        .map(|s| format!("({s})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }))
        }
        PreviewSetting::Unset => {
            if structured_type.is_some_and(|t| eq_str(t, "Mechanism2d")) {
                return Ok(get_mechanism_state(log, &state.log_key, time)
                    .map(|mechanism| plural(mechanism.lines.len(), "segment")));
            }
            if structured_type.is_some_and(|t| eq_str(t, "Alerts")) {
                let errors = string_array_length(log, &state.log_key, "/errors", time);
                let warnings = string_array_length(log, &state.log_key, "/warnings", time);
                let infos = string_array_length(log, &state.log_key, "/infos", time);
                return Ok(Some(format!(
                    "{}, {}, {}",
                    plural(errors, "error"),
                    plural(warnings, "warning"),
                    plural(infos, "info")
                )));
            }
            Ok(Some(match value {
                Some(Value {
                    text,
                    length: Some(length),
                }) => format!("{} \u{2014} {text}", plural(length, "value")),
                Some(Value { text, length: None }) => text,
                None => "null".to_owned(),
            }))
        }
    }
}
