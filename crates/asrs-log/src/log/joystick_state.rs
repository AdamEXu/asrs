//! `shared/log/LogJoystickState.ts`.

use asrs_js::math::bigint::{BigInt, BigIntRangeError};
use asrs_js::math::{int, min2, rem};
use asrs_js::number;
use asrs_js::string::{JsError, JsString};

use super::Log;
use super::log_key_utils::remove_merge_prefix;
use super::log_util::get_value_at;

/// `JOYSTICK_KEYS`, in upstream order.
pub const JOYSTICK_KEYS: &[&str] = &[
    "/DriverStation/Joystick",                         // AdvantageKit
    "NT:/AdvantageKit/DriverStation/Joystick",         // AdvantageKit NT
    "DS:joystick",                                     // DataLog
    "DS:/Dscomm/Control/Robot/ControlData/Joysticks/", // FIRST DS
];

const AKIT_SUFFIX: &str = "/DriverStation/Joystick";
const DATALOG_SUFFIX: &str = "DS:joystick";
const DS_SUFFIX: &str = "DS:/Dscomm/Control/Robot/ControlData/Joysticks/";

/// `JoystickState["mapping"]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JoystickMapping {
    Ni,
    #[default]
    Sdl,
}

impl JoystickMapping {
    pub fn name(self) -> &'static str {
        match self {
            JoystickMapping::Ni => "ni",
            JoystickMapping::Sdl => "sdl",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JoystickButton {
    pub id: u32,
    pub state: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JoystickAxis {
    pub id: u32,
    pub value: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JoystickPov {
    pub id: u32,
    pub angle: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JoystickFinger {
    pub x: f64,
    pub y: f64,
    pub down: bool,
}

/// `JoystickState`; the blank fallback is [`JoystickState::blank`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoystickState {
    pub mapping: JoystickMapping,
    pub buttons: Vec<JoystickButton>,
    pub axes: Vec<JoystickAxis>,
    pub povs: Vec<JoystickPov>,
    pub touchpads: Vec<Vec<JoystickFinger>>,
}

impl JoystickState {
    /// `jsonCopy(BlankJoystickState)`.
    pub fn blank() -> Self {
        JoystickState {
            mapping: JoystickMapping::Sdl,
            buttons: Vec::new(),
            axes: Vec::new(),
            povs: Vec::new(),
            touchpads: Vec::new(),
        }
    }
}

enum Source {
    Akit,
    Datalog,
    Ds,
}

fn ascii(s: &str) -> JsString {
    JsString::from(s)
}

fn key_join(prefix: &JsString, suffix: &str) -> JsString {
    let mut out = prefix.clone();
    out.push_str(&ascii(suffix));
    out
}

fn ends_with(s: &JsString, suffix: &str) -> bool {
    s.ends_with(&ascii(suffix), None)
}

/// `BigInt(x)` for a logged number: a `RangeError` for non-integers.
fn big_int(x: f64) -> Result<BigInt, JsError> {
    BigInt::from_f64(x)
        .map_err(|_| JsError::range(BigIntRangeError::message(&number::to_string(x))))
}

fn number_at(log: &Log, prefix: &JsString, field: &str, time: f64) -> Option<f64> {
    get_value_at::<f64>(log, &key_join(prefix, field), time, None)
}

fn boolean_at(log: &Log, prefix: &JsString, field: &str, time: f64) -> Option<bool> {
    get_value_at::<bool>(log, &key_join(prefix, field), time, None)
}

fn number_array_at(log: &Log, prefix: &JsString, field: &str, time: f64) -> Option<Vec<f64>> {
    get_value_at::<Vec<f64>>(log, &key_join(prefix, field), time, None)
}

fn big_int_at(log: &Log, prefix: &JsString, field: &str, time: f64) -> Result<BigInt, JsError> {
    match number_at(log, prefix, field, time) {
        Some(v) => big_int(v),
        None => Ok(BigInt::zero()),
    }
}

/// `decodePovBitfield`.
pub fn decode_pov_bitfield(value: f64) -> f64 {
    let bits = int::to_int32(value);
    let up = bits & 1 != 0;
    let right = bits & 2 != 0;
    let down = bits & 4 != 0;
    let left = bits & 8 != 0;
    if up && right {
        45.0
    } else if right && down {
        135.0
    } else if down && left {
        225.0
    } else if left && up {
        315.0
    } else if up {
        0.0
    } else if right {
        90.0
    } else if down {
        180.0
    } else if left {
        270.0
    } else {
        -1.0
    }
}

/// `getJoystickState`. Only `BigInt(x)` on a non-integer can throw, so the
/// `Err` is always a `RangeError`.
pub fn get_joystick_state(
    log: &Log,
    joystick_id: f64,
    time: f64,
) -> Result<JoystickState, JsError> {
    let mut state = JoystickState::blank();
    if joystick_id < 0.0 || joystick_id > 5.0 || rem(joystick_id, 1.0) != 0.0 {
        return Ok(state);
    }

    // Find joystick table
    let id_text = number::to_string(joystick_id);
    let mut table_prefix = JsString::new();
    let mut source = None;
    let mut mapping = None;
    'outer: for key in log.field_keys() {
        for joystick_key in JOYSTICK_KEYS {
            let jk = ascii(joystick_key);
            let mut needle = jk.clone();
            needle.push_str(&ascii(&id_text));
            if !remove_merge_prefix(key).starts_with(&needle, None) {
                continue;
            }
            let index = key.index_of(&jk, None);
            let mut prefix = key.slice(0.0, Some(index as f64));
            prefix.push_str(&jk);
            prefix.push_str(&ascii(&id_text));
            prefix.push_unit(u16::from(b'/'));
            table_prefix = prefix;
            if ends_with(&jk, AKIT_SUFFIX) {
                source = Some(Source::Akit);
                mapping = Some(
                    if log
                        .get_field(&key_join(&table_prefix, "ButtonsAvailable"))
                        .is_none()
                    {
                        JoystickMapping::Ni
                    } else {
                        JoystickMapping::Sdl
                    },
                );
            } else if ends_with(&jk, DATALOG_SUFFIX) {
                source = Some(Source::Datalog);
                let base = table_prefix.slice(0.0, Some(-10.0));
                mapping = Some(if log.get_field(&key_join(&base, "opMode")).is_none() {
                    JoystickMapping::Ni
                } else {
                    JoystickMapping::Sdl
                });
            } else if ends_with(&jk, DS_SUFFIX) {
                source = Some(Source::Ds);
                mapping = Some(JoystickMapping::Sdl);
            }
            break 'outer;
        }
    }
    let (Some(source), Some(mapping)) = (source, mapping) else {
        return Ok(state);
    };

    // Read values
    state.mapping = mapping;
    match source {
        Source::Akit => match mapping {
            JoystickMapping::Sdl => akit_sdl(log, &table_prefix, time, &mut state)?,
            JoystickMapping::Ni => akit_ni(log, &table_prefix, time, &mut state),
        },
        Source::Datalog => datalog(log, &table_prefix, time, mapping, &mut state),
        Source::Ds => ds(log, &table_prefix, time, &mut state)?,
    }
    Ok(state)
}

fn akit_sdl(
    log: &Log,
    prefix: &JsString,
    time: f64,
    state: &mut JoystickState,
) -> Result<(), JsError> {
    let buttons_available = big_int_at(log, prefix, "ButtonsAvailable", time)?;
    let button_values = big_int_at(log, prefix, "ButtonValues", time)?;
    let one = BigInt::from(1_i64);
    for i in 0..64_u64 {
        if !buttons_available.shr(i).bitand(&one).is_zero() {
            state.buttons.push(JoystickButton {
                id: i as u32,
                state: !button_values.shr(i).bitand(&one).is_zero(),
            });
        }
    }

    let axes_available = number_at(log, prefix, "AxesAvailable", time).unwrap_or(0.0);
    let axis_values = number_array_at(log, prefix, "AxisValues", time).unwrap_or_default();
    for (i, &value) in axis_values.iter().take(16).enumerate() {
        if int::shift_right(axes_available, i as f64) & 1 != 0 {
            state.axes.push(JoystickAxis {
                id: i as u32,
                value,
            });
        }
    }

    let povs_available = number_at(log, prefix, "POVsAvailable", time).unwrap_or(0.0);
    let pov_values = number_array_at(log, prefix, "POVValues", time).unwrap_or_default();
    for (i, &value) in pov_values.iter().take(8).enumerate() {
        if int::shift_right(povs_available, i as f64) & 1 != 0 {
            state.povs.push(JoystickPov {
                id: i as u32,
                angle: decode_pov_bitfield(value),
            });
        }
    }

    let touchpad_count = number_at(log, prefix, "TouchpadCount", time).unwrap_or(0.0);
    let mut t = 0.0;
    while t < min2(touchpad_count, 2.0) {
        let finger_count = number_at(
            log,
            prefix,
            &format!("Touchpad/{}/FingerCount", number::to_string(t)),
            time,
        )
        .unwrap_or(0.0);
        let mut touchpad = Vec::new();
        let mut f = 0.0;
        while f < min2(finger_count, 2.0) {
            let (ts, fs) = (number::to_string(t), number::to_string(f));
            touchpad.push(JoystickFinger {
                down: boolean_at(
                    log,
                    prefix,
                    &format!("Touchpad/{ts}/Finger/{fs}/Down"),
                    time,
                )
                .unwrap_or(false),
                x: number_at(log, prefix, &format!("Touchpad/{ts}/Finger/{fs}/X"), time)
                    .unwrap_or(0.0),
                y: number_at(log, prefix, &format!("Touchpad/{ts}/Finger/{fs}/Y"), time)
                    .unwrap_or(0.0),
            });
            f += 1.0;
        }
        state.touchpads.push(touchpad);
        t += 1.0;
    }
    Ok(())
}

fn akit_ni(log: &Log, prefix: &JsString, time: f64, state: &mut JoystickState) {
    let button_count = number_at(log, prefix, "ButtonCount", time).unwrap_or(0.0);
    state.buttons = Vec::new();
    if let Some(button_values) = number_at(log, prefix, "ButtonValues", time) {
        let bits = int::to_int32(button_values);
        let mut i = 0.0;
        while i < button_count {
            state.buttons.push(JoystickButton {
                id: (i + 1.0) as u32,
                state: int::shift_left(1.0, i) & bits != 0,
            });
            i += 1.0;
        }
    }
    if let Some(values) = number_array_at(log, prefix, "AxisValues", time) {
        state.axes = values
            .iter()
            .enumerate()
            .map(|(i, &value)| JoystickAxis {
                id: i as u32,
                value,
            })
            .collect();
    }
    if let Some(values) = number_array_at(log, prefix, "POVs", time) {
        state.povs = values
            .iter()
            .enumerate()
            .map(|(i, &angle)| JoystickPov {
                id: i as u32,
                angle,
            })
            .collect();
    }
}

fn datalog(
    log: &Log,
    prefix: &JsString,
    time: f64,
    mapping: JoystickMapping,
    state: &mut JoystickState,
) {
    if let Some(buttons) = get_value_at::<Vec<bool>>(log, &key_join(prefix, "buttons"), time, None)
    {
        let offset = u32::from(mapping == JoystickMapping::Ni);
        state.buttons = buttons
            .iter()
            .enumerate()
            .map(|(i, &pressed)| JoystickButton {
                id: i as u32 + offset,
                state: pressed,
            })
            .collect();
    }
    if let Some(values) = number_array_at(log, prefix, "axes", time) {
        state.axes = values
            .iter()
            .enumerate()
            .map(|(i, &value)| JoystickAxis {
                id: i as u32,
                value,
            })
            .collect();
    }
    if let Some(values) = number_array_at(log, prefix, "povs", time) {
        state.povs = values
            .iter()
            .enumerate()
            .map(|(i, &value)| JoystickPov {
                id: i as u32,
                angle: match mapping {
                    JoystickMapping::Sdl => decode_pov_bitfield(value),
                    JoystickMapping::Ni => value,
                },
            })
            .collect();
    }
}

fn ds(log: &Log, prefix: &JsString, time: f64, state: &mut JoystickState) -> Result<(), JsError> {
    let mask = BigInt::from(0xffffffff_i64);
    let buttons_available = match (
        number_at(log, prefix, "AvailableButtons/high", time),
        number_at(log, prefix, "AvailableButtons/low", time),
    ) {
        (Some(high), Some(low)) => big_int(high)?
            .bitand(&mask)
            .shl(32)
            .bitor(&big_int(low)?.bitand(&mask)),
        _ => BigInt::zero(),
    };
    let button_values = match (
        number_at(log, prefix, "Buttons/high", time),
        number_at(log, prefix, "Buttons/low", time),
    ) {
        (Some(high), Some(low)) => big_int(high)?
            .bitand(&mask)
            .shl(32)
            .bitor(&big_int(low)?.bitand(&mask)),
        _ => BigInt::zero(),
    };
    let one = BigInt::from(1_i64);
    for i in 0..64_u64 {
        if !buttons_available.shr(i).bitand(&one).is_zero() {
            state.buttons.push(JoystickButton {
                id: i as u32,
                state: !button_values.shr(i).bitand(&one).is_zero(),
            });
        }
    }

    let axes_available = number_at(log, prefix, "AvailableAxes", time).unwrap_or(0.0);
    let axis_values = number_array_at(log, prefix, "Axes", time).unwrap_or_default();
    for (i, &value) in axis_values.iter().take(16).enumerate() {
        if int::shift_right(axes_available, i as f64) & 1 != 0 {
            state.axes.push(JoystickAxis {
                id: i as u32,
                value: value / 32768.0,
            });
        }
    }

    let pov_count = number_at(log, prefix, "POVCount", time).unwrap_or(0.0);
    let povs = number_at(log, prefix, "POVs", time).unwrap_or(0.0);
    let mut i = 0.0;
    while i < min2(pov_count, 8.0) {
        let value = int::shift_right(povs, 4.0 * i) & 15;
        state.povs.push(JoystickPov {
            id: i as u32,
            angle: decode_pov_bitfield(f64::from(value)),
        });
        i += 1.0;
    }

    let touchpad_count = number_at(log, prefix, "Touchpads/length", time).unwrap_or(0.0);
    let mut t = 0.0;
    while t < min2(touchpad_count, 2.0) {
        let ts = number::to_string(t);
        let finger_count =
            number_at(log, prefix, &format!("Touchpads/{ts}/Fingers/length"), time).unwrap_or(0.0);
        let mut touchpad = Vec::new();
        let mut f = 0.0;
        while f < min2(finger_count, 2.0) {
            let fs = number::to_string(f);
            touchpad.push(JoystickFinger {
                down: boolean_at(
                    log,
                    prefix,
                    &format!("Touchpads/{ts}/Fingers/{fs}/Down"),
                    time,
                )
                .unwrap_or(false),
                x: number_at(log, prefix, &format!("Touchpads/{ts}/Fingers/{fs}/X"), time)
                    .unwrap_or(0.0)
                    / 65535.0,
                y: number_at(log, prefix, &format!("Touchpads/{ts}/Fingers/{fs}/Y"), time)
                    .unwrap_or(0.0)
                    / 65535.0,
            });
            f += 1.0;
        }
        state.touchpads.push(touchpad);
        t += 1.0;
    }
    Ok(())
}
