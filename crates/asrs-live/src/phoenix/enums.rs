//! `PhoenixEnums` from the bottom of `PhoenixDiagnosticsSource.ts`, valid as of Phoenix
//! 26.0.0-beta-1. `has` models `name in PhoenixEnums`, so it also covers `Object.prototype`
//! members; `lookup` models `PhoenixEnums[name][key]`, which is `undefined` for them.

use asrs_js::string::JsString;

const TABLE: &[(&str, &[(&str, &str)])] = &[
    (
        "AppliedRotorPolarity",
        &[
            ("0", "PositiveIsCounterClockwise"),
            ("1", "PositiveIsClockwise"),
        ],
    ),
    (
        "BridgeOutput",
        &[
            ("0", "BridgeReq_Coast"),
            ("1", "BridgeReq_Brake"),
            ("6", "BridgeReq_Trapez"),
            ("7", "BridgeReq_FOCTorque"),
            ("8", "BridgeReq_MusicTone"),
            ("9", "BridgeReq_FOCEasy"),
            ("12", "BridgeReq_FaultBrake"),
            ("13", "BridgeReq_FaultCoast"),
            ("14", "BridgeReq_ActiveBrake"),
            ("15", "BridgeReq_VariableBrake"),
        ],
    ),
    (
        "ConnectedMotor",
        &[
            ("0", "Unknown"),
            ("1", "Falcon500_Integrated"),
            ("2", "KrakenX60_Integrated"),
            ("3", "KrakenX44_Integrated"),
            ("4", "Minion_JST"),
            ("5", "Brushed_AB"),
            ("6", "Brushed_AC"),
            ("7", "Brushed_BC"),
            ("8", "NEO_JST"),
            ("9", "NEO550_JST"),
            ("10", "VORTEX_JST"),
            ("11", "CustomBrushless"),
        ],
    ),
    (
        "ControlMode",
        &[
            ("0", "DisabledOutput"),
            ("1", "NeutralOut"),
            ("2", "StaticBrake"),
            ("3", "DutyCycleOut"),
            ("4", "PositionDutyCycle"),
            ("5", "VelocityDutyCycle"),
            ("6", "MotionMagicDutyCycle"),
            ("7", "DutyCycleFOC"),
            ("8", "PositionDutyCycleFOC"),
            ("9", "VelocityDutyCycleFOC"),
            ("10", "MotionMagicDutyCycleFOC"),
            ("11", "VoltageOut"),
            ("12", "PositionVoltage"),
            ("13", "VelocityVoltage"),
            ("14", "MotionMagicVoltage"),
            ("15", "VoltageFOC"),
            ("16", "PositionVoltageFOC"),
            ("17", "VelocityVoltageFOC"),
            ("18", "MotionMagicVoltageFOC"),
            ("19", "TorqueCurrentFOC"),
            ("20", "PositionTorqueCurrentFOC"),
            ("21", "VelocityTorqueCurrentFOC"),
            ("22", "MotionMagicTorqueCurrentFOC"),
            ("23", "Follower"),
            ("24", "Reserved"),
            ("25", "CoastOut"),
            ("26", "UnauthorizedDevice"),
            ("27", "MusicTone"),
            ("28", "MotionMagicVelocityDutyCycle"),
            ("29", "MotionMagicVelocityDutyCycleFOC"),
            ("30", "MotionMagicVelocityVoltage"),
            ("31", "MotionMagicVelocityVoltageFOC"),
            ("32", "MotionMagicVelocityTorqueCurrentFOC"),
            ("33", "MotionMagicExpoDutyCycle"),
            ("34", "MotionMagicExpoDutyCycleFOC"),
            ("35", "MotionMagicExpoVoltage"),
            ("36", "MotionMagicExpoVoltageFOC"),
            ("37", "MotionMagicExpoTorqueCurrentFOC"),
        ],
    ),
    ("DeviceEnable", &[("1", "Enabled"), ("0", "Disabled")]),
    (
        "DifferentialControlMode",
        &[
            ("0", "DisabledOutput"),
            ("1", "NeutralOut"),
            ("2", "StaticBrake"),
            ("3", "DutyCycleOut"),
            ("4", "PositionDutyCycle"),
            ("5", "VelocityDutyCycle"),
            ("6", "MotionMagicDutyCycle"),
            ("7", "DutyCycleFOC"),
            ("8", "PositionDutyCycleFOC"),
            ("9", "VelocityDutyCycleFOC"),
            ("10", "MotionMagicDutyCycleFOC"),
            ("11", "VoltageOut"),
            ("12", "PositionVoltage"),
            ("13", "VelocityVoltage"),
            ("14", "MotionMagicVoltage"),
            ("15", "VoltageFOC"),
            ("16", "PositionVoltageFOC"),
            ("17", "VelocityVoltageFOC"),
            ("18", "MotionMagicVoltageFOC"),
            ("19", "TorqueCurrentFOC"),
            ("20", "PositionTorqueCurrentFOC"),
            ("21", "VelocityTorqueCurrentFOC"),
            ("22", "MotionMagicTorqueCurrentFOC"),
            ("23", "Follower"),
            ("24", "Reserved"),
            ("25", "CoastOut"),
            ("26", "UnauthorizedDevice"),
            ("27", "MusicTone"),
            ("28", "MotionMagicVelocityDutyCycle"),
            ("29", "MotionMagicVelocityDutyCycleFOC"),
            ("30", "MotionMagicVelocityVoltage"),
            ("31", "MotionMagicVelocityVoltageFOC"),
            ("32", "MotionMagicVelocityTorqueCurrentFOC"),
            ("33", "MotionMagicExpoDutyCycle"),
            ("34", "MotionMagicExpoDutyCycleFOC"),
            ("35", "MotionMagicExpoVoltage"),
            ("36", "MotionMagicExpoVoltageFOC"),
            ("37", "MotionMagicExpoTorqueCurrentFOC"),
        ],
    ),
    (
        "ExternalMotorTempStatus",
        &[
            ("0", "Collecting"),
            ("1", "Disconnected"),
            ("2", "TooHot"),
            ("3", "Normal"),
            ("4", "NotUsed"),
            ("5", "WrongMotorOrShorted"),
        ],
    ),
    ("ForwardLimit", &[("0", "ClosedToGround"), ("1", "Open")]),
    ("IsProLicensed", &[("0", "NotLicensed"), ("1", "Licensed")]),
    (
        "MagnetHealth",
        &[
            ("1", "Magnet_Red"),
            ("2", "Magnet_Orange"),
            ("3", "Magnet_Green"),
            ("0", "Magnet_Invalid"),
        ],
    ),
    (
        "MeasurementHealth",
        &[("0", "Good"), ("1", "Limited"), ("2", "Bad")],
    ),
    (
        "MotionMagicIsRunning",
        &[("1", "Enabled"), ("0", "Disabled")],
    ),
    (
        "MotorOutputStatus",
        &[
            ("0", "Unknown"),
            ("1", "Off"),
            ("2", "StaticBraking"),
            ("3", "Motoring"),
            ("4", "DiscordantMotoring"),
            ("5", "RegenBraking"),
        ],
    ),
    (
        "MotorType",
        &[("0", "Unknown"), ("1", "Falcon500"), ("2", "KrakenX60")],
    ),
    ("ReverseLimit", &[("0", "ClosedToGround"), ("1", "Open")]),
    ("RobotEnable", &[("0", "Disabled"), ("1", "Enabled")]),
];

const PROTOTYPE_KEYS: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
    "__proto__",
];

fn is(name: &JsString, text: &str) -> bool {
    name.units().iter().copied().eq(text.encode_utf16())
}

/// `name in PhoenixEnums`.
pub fn has(name: &JsString) -> bool {
    TABLE.iter().any(|(n, _)| is(name, n)) || PROTOTYPE_KEYS.iter().any(|p| is(name, p))
}

/// `PhoenixEnums[name][key]`: `None` when the value upstream reads back is `undefined`.
pub fn lookup<'a>(name: &JsString, key: &JsString) -> Option<&'a str> {
    TABLE
        .iter()
        .find(|(n, _)| is(name, n))?
        .1
        .iter()
        .find(|(k, _)| is(key, k))
        .map(|(_, v)| *v)
}
