//! `src/shared/AdvantageScopeAssets.ts` (asset types, field constants,
//! `BuiltIn3dFields`) and the `src/main/assetUtil.ts` config parsers.

use std::sync::OnceLock;

use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, array_index};
use asrs_log::units;
use asrs_log::util::check_array_type;

fn inches(x: f64) -> f64 {
    units::convert(x, "inches", "meters").expect("inches and meters are units")
}

fn feet(x: f64) -> f64 {
    units::convert(x, "feet", "meters").expect("feet and meters are units")
}

/// `FRC_STANDARD_FIELD_LENGTH`.
pub fn frc_standard_field_length() -> f64 {
    feet(54.0)
}

/// `FRC_STANDARD_FIELD_WIDTH`.
pub fn frc_standard_field_width() -> f64 {
    feet(27.0)
}

/// `FTC_STANDARD_FIELD_LENGTH`.
pub fn ftc_standard_field_length() -> f64 {
    feet(12.0)
}

/// `FTC_STANDARD_FIELD_WIDTH`.
pub fn ftc_standard_field_width() -> f64 {
    feet(12.0)
}

/// `ALLIANCE_STATION_WIDTH`.
pub fn alliance_station_width() -> f64 {
    inches(69.0)
}

/// `DEFAULT_DRIVER_STATIONS_FRC`. Upstream shares this array by reference into every
/// parsed FRC field config; it is never mutated.
pub fn default_driver_stations_frc() -> &'static [[f64; 2]] {
    static DS: OnceLock<Vec<[f64; 2]>> = OnceLock::new();
    DS.get_or_init(|| {
        let l = frc_standard_field_length() / 2.0;
        let w = alliance_station_width();
        vec![[l, -w], [l, 0.0], [l, w], [-l, w], [-l, 0.0], [-l, -w]]
    })
}

/// `DEFAULT_DRIVER_STATIONS_FTC`.
pub fn default_driver_stations_ftc() -> &'static [[f64; 2]] {
    static DS: OnceLock<Vec<[f64; 2]>> = OnceLock::new();
    DS.get_or_init(|| {
        let x = ftc_standard_field_length() / 2.0 + inches(39.0);
        let y = ftc_standard_field_width() / 6.0;
        vec![[x, -y], [x, y], [-x, y], [-x, -y]]
    })
}

/// `CoordinateSystem`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinateSystem {
    WallAlliance,
    WallBlue,
    CenterRotated,
    CenterRed,
}

impl CoordinateSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WallAlliance => "wall-alliance",
            Self::WallBlue => "wall-blue",
            Self::CenterRotated => "center-rotated",
            Self::CenterRed => "center-red",
        }
    }

    fn parse(s: &JsString) -> Option<Self> {
        for (variant, name) in [
            (Self::WallAlliance, "wall-alliance"),
            (Self::WallBlue, "wall-blue"),
            (Self::CenterRotated, "center-rotated"),
            (Self::CenterRed, "center-red"),
        ] {
            if js_eq(s, name) {
                return Some(variant);
            }
        }
        None
    }
}

/// `Config2d`.
#[derive(Clone, Debug, PartialEq)]
pub struct Config2d {
    pub name: JsString,
    pub path: JsString,
    pub id: JsString,
    pub is_ftc: bool,
    pub coordinate_system: CoordinateSystem,
    pub use_grid: bool,
    pub source_url: Option<JsString>,
    pub top_left: [f64; 2],
    pub bottom_right: [f64; 2],
    pub width_inches: f64,
    pub height_inches: f64,
}

/// The parser's initial `config` literal.
impl Default for Config2d {
    fn default() -> Self {
        Config2d {
            name: JsString::new(),
            path: JsString::new(),
            id: JsString::new(),
            is_ftc: false,
            coordinate_system: CoordinateSystem::CenterRed,
            use_grid: true,
            source_url: None,
            top_left: [-1.0, -1.0],
            bottom_right: [-1.0, -1.0],
            width_inches: 0.0,
            height_inches: 0.0,
        }
    }
}

/// `Config3dField`.
#[derive(Clone, Debug, PartialEq)]
pub struct Config3dField {
    pub name: JsString,
    pub path: JsString,
    pub id: JsString,
    pub is_ftc: bool,
    pub coordinate_system: CoordinateSystem,
    pub use_grid: bool,
    pub rotations: Vec<Config3dRotation>,
    pub position: [f64; 3],
    pub width_inches: f64,
    pub height_inches: f64,
    pub driver_stations: Vec<[f64; 2]>,
    pub game_pieces: Vec<Config3dFieldGamePiece>,
    pub april_tags: Vec<Config3dFieldAprilTag>,
}

impl Default for Config3dField {
    fn default() -> Self {
        Config3dField {
            name: JsString::new(),
            path: JsString::new(),
            id: JsString::new(),
            is_ftc: false,
            coordinate_system: CoordinateSystem::CenterRed,
            use_grid: true,
            rotations: Vec::new(),
            position: [0.0, 0.0, 0.0],
            width_inches: 0.0,
            height_inches: 0.0,
            driver_stations: default_driver_stations_frc().to_vec(),
            game_pieces: Vec::new(),
            april_tags: Vec::new(),
        }
    }
}

/// `Config3dField_GamePiece`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config3dFieldGamePiece {
    pub name: JsString,
    pub rotations: Vec<Config3dRotation>,
    pub position: [f64; 3],
    pub staged_objects: Vec<JsString>,
}

/// `Config3dField_AprilTag`.
#[derive(Clone, Debug, PartialEq)]
pub struct Config3dFieldAprilTag {
    pub variant: JsString,
    pub id: f64,
    pub rotations: Vec<Config3dRotation>,
    pub position: [f64; 3],
}

impl Default for Config3dFieldAprilTag {
    fn default() -> Self {
        Config3dFieldAprilTag {
            variant: JsString::from("36h11-6.5in"),
            id: 0.0,
            rotations: Vec::new(),
            position: [0.0, 0.0, 0.0],
        }
    }
}

/// `Config3dRobot`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config3dRobot {
    pub name: JsString,
    pub path: JsString,
    pub is_ftc: bool,
    pub disable_simplification: bool,
    pub rotations: Vec<Config3dRotation>,
    pub position: [f64; 3],
    pub cameras: Vec<Config3dRobotCamera>,
    pub components: Vec<Config3dRobotComponent>,
}

/// `Config3dRobot_Camera`.
#[derive(Clone, Debug, PartialEq)]
pub struct Config3dRobotCamera {
    pub name: JsString,
    pub rotations: Vec<Config3dRotation>,
    pub position: [f64; 3],
    pub resolution: [f64; 2],
    pub fov: f64,
}

impl Default for Config3dRobotCamera {
    fn default() -> Self {
        Config3dRobotCamera {
            name: JsString::new(),
            rotations: Vec::new(),
            position: [0.0, 0.0, 0.0],
            resolution: [200.0, 100.0],
            fov: 90.0,
        }
    }
}

/// `Config3dRobot_Component`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config3dRobotComponent {
    pub zeroed_rotations: Vec<Config3dRotation>,
    pub zeroed_position: [f64; 3],
}

/// `Config3d_Rotation`. Upstream validates the elements then stores the raw
/// objects, so extra keys and the input key order survive serialization; `raw`
/// is what `set-assets` carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Config3dRotation {
    pub axis: RotationAxis,
    pub degrees: f64,
    pub raw: JsValue,
}

/// `Config3d_Rotation["axis"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationAxis {
    X,
    Y,
    Z,
}

impl RotationAxis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }

    fn parse(s: &JsString) -> Option<Self> {
        if js_eq(s, "x") {
            Some(Self::X)
        } else if js_eq(s, "y") {
            Some(Self::Y)
        } else if js_eq(s, "z") {
            Some(Self::Z)
        } else {
            None
        }
    }
}

/// `ConfigJoystick`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigJoystick {
    pub name: JsString,
    pub path: JsString,
    pub components: Vec<ConfigJoystickComponent>,
}

/// The `ConfigJoystick_*` component union.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigJoystickComponent {
    Button(ConfigJoystickButton),
    Joystick(ConfigJoystickStick),
    Axis(ConfigJoystickAxis),
    Touchpad(ConfigJoystickTouchpad),
}

/// `"up" | "right" | "down" | "left"`, the `*SourcePov` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePov {
    Up,
    Right,
    Down,
    Left,
}

impl SourcePov {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Right => "right",
            Self::Down => "down",
            Self::Left => "left",
        }
    }

    fn parse(s: &JsString) -> Option<Self> {
        for (variant, name) in [
            (Self::Up, "up"),
            (Self::Right, "right"),
            (Self::Down, "down"),
            (Self::Left, "left"),
        ] {
            if js_eq(s, name) {
                return Some(variant);
            }
        }
        None
    }
}

/// `ConfigJoystick_Button`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigJoystickButton {
    pub is_yellow: bool,
    pub is_ellipse: bool,
    pub center_px: [f64; 2],
    pub size_px: [f64; 2],
    pub sdl_source_index: Option<f64>,
    pub sdl_source_pov: Option<SourcePov>,
    pub ni_source_index: Option<f64>,
    pub ni_source_pov: Option<SourcePov>,
}

/// `ConfigJoystick_Joystick`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigJoystickStick {
    pub is_yellow: bool,
    pub center_px: [f64; 2],
    pub radius_px: f64,
    pub sdl_x_source_index: Option<f64>,
    pub sdl_x_source_inverted: Option<bool>,
    pub sdl_y_source_index: Option<f64>,
    pub sdl_y_source_inverted: Option<bool>,
    pub sdl_button_source_index: Option<f64>,
    pub ni_x_source_index: Option<f64>,
    pub ni_x_source_inverted: Option<bool>,
    pub ni_y_source_index: Option<f64>,
    pub ni_y_source_inverted: Option<bool>,
    pub ni_button_source_index: Option<f64>,
}

/// `ConfigJoystick_Axis`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigJoystickAxis {
    pub is_yellow: bool,
    pub center_px: [f64; 2],
    pub size_px: [f64; 2],
    pub sdl_source_index: Option<f64>,
    pub sdl_source_range: Option<[f64; 2]>,
    pub ni_source_index: Option<f64>,
    pub ni_source_range: Option<[f64; 2]>,
}

/// `ConfigJoystick_Touchpad`.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigJoystickTouchpad {
    pub is_yellow: bool,
    pub center_px: [f64; 2],
    pub size_px: [f64; 2],
    pub sdl_source_index: f64,
}

impl Default for ConfigJoystickTouchpad {
    fn default() -> Self {
        ConfigJoystickTouchpad {
            is_yellow: false,
            center_px: [0.0, 0.0],
            size_px: [0.0, 0.0],
            sdl_source_index: -1.0,
        }
    }
}

/// `AdvantageScopeAssets`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdvantageScopeAssets {
    pub field2ds: Vec<Config2d>,
    pub field3ds: Vec<Config3dField>,
    pub robots: Vec<Config3dRobot>,
    pub joysticks: Vec<ConfigJoystick>,
    pub load_failures: Vec<JsString>,
}

/// `BuiltIn3dFields`: appended after `assets.field3ds` at every lookup.
pub fn built_in_3d_fields() -> &'static [Config3dField] {
    static FIELDS: OnceLock<Vec<Config3dField>> = OnceLock::new();
    FIELDS.get_or_init(|| {
        let frc = |name: &str, id: &str| Config3dField {
            name: JsString::from(name),
            path: JsString::new(),
            id: JsString::from(id),
            is_ftc: false,
            coordinate_system: CoordinateSystem::CenterRed,
            use_grid: true,
            rotations: Vec::new(),
            position: [0.0, 0.0, 0.0],
            width_inches: units::convert(frc_standard_field_length(), "meters", "inches")
                .expect("meters and inches are units"),
            height_inches: units::convert(frc_standard_field_width(), "meters", "inches")
                .expect("meters and inches are units"),
            driver_stations: default_driver_stations_frc().to_vec(),
            game_pieces: Vec::new(),
            april_tags: Vec::new(),
        };
        let ftc = |name: &str, id: &str| Config3dField {
            name: JsString::from(name),
            path: JsString::new(),
            id: JsString::from(id),
            is_ftc: true,
            coordinate_system: CoordinateSystem::CenterRed,
            use_grid: true,
            rotations: Vec::new(),
            position: [0.0, 0.0, 0.0],
            width_inches: units::convert(ftc_standard_field_length(), "meters", "inches")
                .expect("meters and inches are units"),
            height_inches: units::convert(ftc_standard_field_width(), "meters", "inches")
                .expect("meters and inches are units"),
            driver_stations: default_driver_stations_ftc().to_vec(),
            game_pieces: Vec::new(),
            april_tags: Vec::new(),
        };
        vec![
            frc("Evergreen", "FRC:Evergreen"),
            ftc("Evergreen", "FTC:Evergreen"),
            frc("Axes", "FRC:Axes"),
            ftc("Axes", "FTC:Axes"),
        ]
    })
}

/// `TypeError` thrown by the parsers, holding the message V8 produces. It is a
/// JS string because the receiver text is embedded verbatim (a lone-surrogate
/// config string survives into the message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeError {
    pub message: JsString,
}

impl TypeError {
    /// `Cannot use 'in' operator to search for '<key>' in <receiver>`.
    fn in_operator(value: &JsValue, key: &str) -> Self {
        let mut message = JsString::from("Cannot use 'in' operator to search for '");
        message.push_str(&JsString::from(key));
        message.push_str(&JsString::from("' in "));
        message.push_str(&in_receiver(value));
        TypeError { message }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypeError: {}", self.message.to_string_lossy())
    }
}

impl std::error::Error for TypeError {}

/// The `toString`-flavored receiver text of the `in` TypeError.
fn in_receiver(value: &JsValue) -> JsString {
    match value {
        JsValue::Undefined => JsString::from("undefined"),
        JsValue::Null => JsString::from("null"),
        JsValue::Bool(true) => JsString::from("true"),
        JsValue::Bool(false) => JsString::from("false"),
        JsValue::Number(x) => JsString::from(number::to_string(*x)),
        JsValue::String(s) => s.clone(),
        JsValue::BigInt(b) => JsString::from(format!("{b}")),
        _ => JsString::new(),
    }
}

/// `key in value` for the parser's property checks: primitives raise V8's
/// TypeError; objects and arrays resolve own properties and `Object.prototype`
/// names (none of the queried keys are one, but the prototype lookup is what
/// `in` does).
fn js_in(value: &JsValue, key: &str) -> Result<bool, TypeError> {
    let proto = units::is_object_proto_name(key);
    let units: Vec<u16> = key.encode_utf16().collect();
    let index = array_index(&units).map(|i| i as usize);
    Ok(match value {
        JsValue::Object(o) => o.contains_key(key) || proto,
        JsValue::Array(a) => index.is_some_and(|i| i < a.len()) || key == "length" || proto,
        JsValue::Bytes(b) => index.is_some_and(|i| i < b.len()) || key == "length" || proto,
        JsValue::ExtData(..) => key == "type" || key == "data" || proto,
        JsValue::Date(_) => proto,
        JsValue::Undefined
        | JsValue::Null
        | JsValue::Bool(_)
        | JsValue::Number(_)
        | JsValue::String(_)
        | JsValue::BigInt(_) => return Err(TypeError::in_operator(value, key)),
    })
}

/// `value[key]` restricted to own enumerable properties, which covers every key
/// the parsers read: none collide with `Object.prototype` names.
fn own_get<'a>(value: &'a JsValue, key: &str) -> Option<&'a JsValue> {
    match value {
        JsValue::Object(o) => o.get(key),
        JsValue::Array(a) => {
            array_index(&key.encode_utf16().collect::<Vec<u16>>()).and_then(|i| a.get(i as usize))
        }
        _ => None,
    }
}

/// `"key" in value && value[key]`, raising the `in` TypeError on primitives.
fn prop<'a>(value: &'a JsValue, key: &str) -> Result<Option<&'a JsValue>, TypeError> {
    Ok(if js_in(value, key)? {
        own_get(value, key)
    } else {
        None
    })
}

fn js_eq(s: &JsString, lit: &str) -> bool {
    s.units().iter().copied().eq(lit.encode_utf16())
}

/// `"key" in v && checkArrayType(v.key, "number") && v.key.length === N`.
fn num_array<const N: usize>(v: &JsValue, key: &str) -> Result<Option<[f64; N]>, TypeError> {
    let Some(raw) = prop(v, key)? else {
        return Ok(None);
    };
    if !check_array_type(raw, "number") {
        return Ok(None);
    }
    let Some(items) = raw.as_array() else {
        return Ok(None);
    };
    if items.len() != N {
        return Ok(None);
    }
    let mut out = [0.0; N];
    for (i, item) in items.iter().enumerate() {
        out[i] = item.as_f64().expect("checked number");
    }
    Ok(Some(out))
}

/// `"key" in v && checkArrayType(v.key, "string")`.
fn string_array(v: &JsValue, key: &str) -> Result<Option<Vec<JsString>>, TypeError> {
    let Some(raw) = prop(v, key)? else {
        return Ok(None);
    };
    if !check_array_type(raw, "string") {
        return Ok(None);
    }
    let items = raw.as_array().expect("check_array_type is an array check");
    Ok(Some(
        items
            .iter()
            .map(|i| i.as_str().expect("checked string").clone())
            .collect(),
    ))
}

/// `typeof element === "object" && "axis" in element && axis in {x,y,z} &&
/// "degrees" in element && typeof degrees === "number"`, evaluated in that order
/// (a `null` element throws on `"axis" in null`).
fn is_rotation(element: &JsValue) -> Result<Option<(RotationAxis, f64)>, TypeError> {
    if element.type_of() != "object" || !js_in(element, "axis")? {
        return Ok(None);
    }
    let axis = match own_get(element, "axis") {
        Some(JsValue::String(s)) => RotationAxis::parse(s),
        _ => None,
    };
    let Some(axis) = axis else {
        return Ok(None);
    };
    if !js_in(element, "degrees")? {
        return Ok(None);
    }
    let Some(JsValue::Number(degrees)) = own_get(element, "degrees") else {
        return Ok(None);
    };
    Ok(Some((axis, *degrees)))
}

/// The all-or-nothing `rotations` / `zeroedRotations` array check.
fn rotations_prop(v: &JsValue, key: &str) -> Result<Option<Vec<Config3dRotation>>, TypeError> {
    let Some(raw) = prop(v, key)? else {
        return Ok(None);
    };
    let JsValue::Array(items) = raw else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(rotation) = rotation_of(item)? else {
            return Ok(None);
        };
        out.push(rotation);
    }
    Ok(Some(out))
}

fn rotation_of(element: &JsValue) -> Result<Option<Config3dRotation>, TypeError> {
    Ok(
        is_rotation(element)?.map(|(axis, degrees)| Config3dRotation {
            axis,
            degrees,
            raw: element.clone(),
        }),
    )
}

/// The parser union result: `Config`, `"invalid"` or `"skip"`.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed<T> {
    Config(T),
    Invalid,
    Skip,
}

impl<T> Parsed<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Parsed<U> {
        match self {
            Parsed::Config(v) => Parsed::Config(f(v)),
            Parsed::Invalid => Parsed::Invalid,
            Parsed::Skip => Parsed::Skip,
        }
    }
}

pub type ParseResult<T> = Result<Parsed<T>, TypeError>;

fn field_id(is_ftc: bool, name: &JsString) -> JsString {
    let mut id = JsString::from(if is_ftc { "FTC:" } else { "FRC:" });
    id.push_str(name);
    id
}

/// `parseField2d`.
pub fn parse_field_2d(config_raw: &JsValue) -> ParseResult<Config2d> {
    if config_raw.is_null() || config_raw.type_of() != "object" {
        return Ok(Parsed::Invalid);
    }
    let mut config = Config2d::default();
    if let Some(JsValue::String(s)) = prop(config_raw, "name")? {
        config.name = s.clone();
    }
    if let Some(JsValue::Bool(b)) = prop(config_raw, "isFTC")? {
        config.is_ftc = *b;
    } else {
        return Ok(Parsed::Skip);
    }
    if let Some(JsValue::String(s)) = prop(config_raw, "coordinateSystem")?
        && let Some(cs) = CoordinateSystem::parse(s)
    {
        config.coordinate_system = cs;
    }
    if let Some(JsValue::Bool(b)) = prop(config_raw, "useGrid")? {
        config.use_grid = *b;
    }
    if let Some(JsValue::String(s)) = prop(config_raw, "sourceUrl")? {
        config.source_url = Some(s.clone());
    }
    if let Some(v) = num_array::<2>(config_raw, "topLeft")? {
        config.top_left = v;
    }
    if let Some(v) = num_array::<2>(config_raw, "bottomRight")? {
        config.bottom_right = v;
    }
    if let Some(JsValue::Number(x)) = prop(config_raw, "widthInches")? {
        config.width_inches = *x;
    }
    if let Some(JsValue::Number(x)) = prop(config_raw, "heightInches")? {
        config.height_inches = *x;
    }
    if !config.name.is_empty()
        && config.top_left[0] >= 0.0
        && config.top_left[1] >= 0.0
        && config.bottom_right[0] >= 0.0
        && config.bottom_right[1] >= 0.0
        && config.width_inches > 0.0
        && config.height_inches > 0.0
    {
        config.id = field_id(config.is_ftc, &config.name);
        return Ok(Parsed::Config(config));
    }
    Ok(Parsed::Invalid)
}

/// `parseField3d`.
pub fn parse_field_3d(config_raw: &JsValue) -> ParseResult<Config3dField> {
    if config_raw.is_null() || config_raw.type_of() != "object" {
        return Ok(Parsed::Invalid);
    }
    let mut config = Config3dField::default();
    if let Some(JsValue::String(s)) = prop(config_raw, "name")? {
        config.name = s.clone();
    }
    if let Some(JsValue::Bool(b)) = prop(config_raw, "isFTC")? {
        config.is_ftc = *b;
        if config.is_ftc {
            config.driver_stations = default_driver_stations_ftc().to_vec();
        }
    } else {
        return Ok(Parsed::Skip);
    }
    if let Some(JsValue::String(s)) = prop(config_raw, "coordinateSystem")?
        && let Some(cs) = CoordinateSystem::parse(s)
    {
        config.coordinate_system = cs;
    }
    if let Some(JsValue::Bool(b)) = prop(config_raw, "useGrid")? {
        config.use_grid = *b;
    }
    if let Some(rotations) = rotations_prop(config_raw, "rotations")? {
        config.rotations = rotations;
    }
    if let Some(v) = num_array::<3>(config_raw, "position")? {
        config.position = v;
    }
    if let Some(JsValue::Number(x)) = prop(config_raw, "widthInches")? {
        config.width_inches = *x;
    }
    if let Some(JsValue::Number(x)) = prop(config_raw, "heightInches")? {
        config.height_inches = *x;
    }
    if let Some(JsValue::Array(stations)) = prop(config_raw, "driverStations")?
        && stations.len() == if config.is_ftc { 4 } else { 6 }
        && stations.iter().all(|position| {
            check_array_type(position, "number")
                && position.as_array().is_some_and(|a| a.len() == 2)
        })
    {
        config.driver_stations = stations
            .iter()
            .map(|position| {
                let a = position.as_array().expect("checked array");
                [
                    a[0].as_f64().expect("checked number"),
                    a[1].as_f64().expect("checked number"),
                ]
            })
            .collect();
    }
    if let Some(JsValue::Array(items)) = prop(config_raw, "gamePieces")? {
        for raw in items {
            config.game_pieces.push(Config3dFieldGamePiece::default());
            let game_piece = config.game_pieces.last_mut().expect("just pushed");
            if let Some(JsValue::String(s)) = prop(raw, "name")? {
                game_piece.name = s.clone();
            }
            if let Some(rotations) = rotations_prop(raw, "rotations")? {
                game_piece.rotations = rotations;
            }
            if let Some(v) = num_array::<3>(raw, "position")? {
                game_piece.position = v;
            }
            if let Some(strings) = string_array(raw, "stagedObjects")? {
                game_piece.staged_objects = strings;
            }
        }
    }
    if let Some(JsValue::Array(items)) = prop(config_raw, "aprilTags")? {
        for raw in items {
            let mut april_tag = Config3dFieldAprilTag::default();
            if let Some(JsValue::String(v)) = prop(raw, "variant")? {
                let variant = legacy_april_tag_variant(v);
                let parts = variant.split(Some(&JsString::from("-")), None);
                if !(parts.len() == 2
                    && (js_eq(&parts[0], "36h11") || js_eq(&parts[0], "16h5"))
                    && parts[1].ends_with(&JsString::from("in"), None)
                    && !number::parse_float(&parts[1].units()[..parts[1].units().len() - 2])
                        .is_nan())
                {
                    continue;
                }
                april_tag.variant = variant;
            } else {
                continue;
            }
            if let Some(JsValue::Number(x)) = prop(raw, "id")? {
                april_tag.id = *x;
            }
            if let Some(rotations) = rotations_prop(raw, "rotations")? {
                april_tag.rotations = rotations;
            }
            if let Some(v) = num_array::<3>(raw, "position")? {
                april_tag.position = v;
            }
            config.april_tags.push(april_tag);
        }
    }
    if !config.name.is_empty() && config.width_inches > 0.0 && config.height_inches > 0.0 {
        config.id = field_id(config.is_ftc, &config.name);
        return Ok(Parsed::Config(config));
    }
    Ok(Parsed::Invalid)
}

/// The `variant` backwards-compatibility renames in `parseField3d`.
fn legacy_april_tag_variant(variant: &JsString) -> JsString {
    for (from, to) in [
        ("frc-36h11", "36h11-6.5in"),
        ("frc-16h5", "16h5-6in"),
        ("ftc-2in", "36h11-2in"),
        ("ftc-3in", "36h11-3in"),
        ("ftc-4in", "36h11-4in"),
        ("ftc-5in", "36h11-5in"),
    ] {
        if js_eq(variant, from) {
            return JsString::from(to);
        }
    }
    variant.clone()
}

/// `parseRobot`.
pub fn parse_robot(config_raw: &JsValue) -> ParseResult<Config3dRobot> {
    if config_raw.is_null() || config_raw.type_of() != "object" {
        return Ok(Parsed::Invalid);
    }
    let mut config = Config3dRobot {
        name: JsString::new(),
        path: JsString::new(),
        is_ftc: false,
        disable_simplification: false,
        rotations: Vec::new(),
        position: [0.0, 0.0, 0.0],
        cameras: Vec::new(),
        components: Vec::new(),
    };
    if let Some(JsValue::String(s)) = prop(config_raw, "name")? {
        config.name = s.clone();
    }
    if let Some(JsValue::Bool(b)) = prop(config_raw, "isFTC")? {
        config.is_ftc = *b;
    }
    if let Some(JsValue::Bool(b)) = prop(config_raw, "disableSimplification")? {
        config.disable_simplification = *b;
    }
    if let Some(rotations) = rotations_prop(config_raw, "rotations")? {
        config.rotations = rotations;
    }
    if let Some(v) = num_array::<3>(config_raw, "position")? {
        config.position = v;
    }
    if let Some(JsValue::Array(items)) = prop(config_raw, "cameras")? {
        for raw in items {
            config.cameras.push(Config3dRobotCamera::default());
            let camera = config.cameras.last_mut().expect("just pushed");
            if let Some(JsValue::String(s)) = prop(raw, "name")? {
                camera.name = s.clone();
            }
            if let Some(rotations) = rotations_prop(raw, "rotations")? {
                camera.rotations = rotations;
            }
            if let Some(v) = num_array::<3>(raw, "position")? {
                camera.position = v;
            }
            if let Some(v) = num_array::<2>(raw, "resolution")? {
                camera.resolution = v;
            }
            if let Some(JsValue::Number(x)) = prop(raw, "fov")? {
                camera.fov = *x;
            }
        }
    }
    if let Some(JsValue::Array(items)) = prop(config_raw, "components")? {
        for raw in items {
            config.components.push(Config3dRobotComponent {
                zeroed_rotations: Vec::new(),
                zeroed_position: [0.0, 0.0, 0.0],
            });
            let component = config.components.last_mut().expect("just pushed");
            if let Some(rotations) = rotations_prop(raw, "zeroedRotations")? {
                component.zeroed_rotations = rotations;
            }
            if let Some(v) = num_array::<3>(raw, "zeroedPosition")? {
                component.zeroed_position = v;
            }
        }
    }
    if !config.name.is_empty() && config.cameras.iter().all(|c| !c.name.is_empty()) {
        return Ok(Parsed::Config(config));
    }
    Ok(Parsed::Invalid)
}

/// `parseJoystick`.
pub fn parse_joystick(config_raw: &JsValue) -> ParseResult<ConfigJoystick> {
    if config_raw.is_null() || config_raw.type_of() != "object" {
        return Ok(Parsed::Invalid);
    }
    let mut config = ConfigJoystick {
        name: JsString::new(),
        path: JsString::new(),
        components: Vec::new(),
    };
    if let Some(JsValue::String(s)) = prop(config_raw, "name")? {
        config.name = s.clone();
    }
    if let Some(JsValue::Array(items)) = prop(config_raw, "components")? {
        for raw in items {
            let mut is_yellow = false;
            if let Some(JsValue::Bool(b)) = prop(raw, "isYellow")? {
                is_yellow = *b;
            }
            let mut center_px = [0.0, 0.0];
            if let Some(v) = num_array::<2>(raw, "centerPx")? {
                center_px = v;
            }
            let Some(JsValue::String(kind)) = prop(raw, "type")? else {
                continue;
            };
            if js_eq(kind, "button") {
                let mut button = ConfigJoystickButton {
                    is_yellow,
                    is_ellipse: false,
                    center_px,
                    size_px: [0.0, 0.0],
                    sdl_source_index: None,
                    sdl_source_pov: None,
                    ni_source_index: None,
                    ni_source_pov: None,
                };
                if let Some(JsValue::Bool(b)) = prop(raw, "isEllipse")? {
                    button.is_ellipse = *b;
                }
                if let Some(v) = num_array::<2>(raw, "sizePx")? {
                    button.size_px = v;
                }
                if let Some(JsValue::Number(x)) = prop(raw, "sdlSourceIndex")? {
                    button.sdl_source_index = Some(*x);
                }
                if let Some(JsValue::String(s)) = prop(raw, "sdlSourcePov")?
                    && let Some(pov) = SourcePov::parse(s)
                {
                    button.sdl_source_pov = Some(pov);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "niSourceIndex")? {
                    button.ni_source_index = Some(*x);
                } else if let Some(JsValue::Number(x)) = prop(raw, "sourceIndex")? {
                    button.ni_source_index = Some(*x);
                }
                if let Some(JsValue::String(s)) = prop(raw, "niSourcePov")?
                    && let Some(pov) = SourcePov::parse(s)
                {
                    button.ni_source_pov = Some(pov);
                } else if let Some(JsValue::String(s)) = prop(raw, "sourcePov")?
                    && let Some(pov) = SourcePov::parse(s)
                {
                    button.ni_source_pov = Some(pov);
                }
                config
                    .components
                    .push(ConfigJoystickComponent::Button(button));
            } else if js_eq(kind, "joystick") {
                let mut stick = ConfigJoystickStick {
                    is_yellow,
                    center_px,
                    radius_px: 0.0,
                    ..Default::default()
                };
                if let Some(JsValue::Number(x)) = prop(raw, "radiusPx")? {
                    stick.radius_px = *x;
                }
                if let Some(JsValue::Number(x)) = prop(raw, "sdlXSourceIndex")? {
                    stick.sdl_x_source_index = Some(*x);
                }
                if let Some(JsValue::Bool(b)) = prop(raw, "sdlXSourceInverted")? {
                    stick.sdl_x_source_inverted = Some(*b);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "sdlYSourceIndex")? {
                    stick.sdl_y_source_index = Some(*x);
                }
                if let Some(JsValue::Bool(b)) = prop(raw, "sdlYSourceInverted")? {
                    stick.sdl_y_source_inverted = Some(*b);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "sdlButtonSourceIndex")? {
                    stick.sdl_button_source_index = Some(*x);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "niXSourceIndex")? {
                    stick.ni_x_source_index = Some(*x);
                } else if let Some(JsValue::Number(x)) = prop(raw, "xSourceIndex")? {
                    stick.ni_x_source_index = Some(*x);
                }
                if let Some(JsValue::Bool(b)) = prop(raw, "niXSourceInverted")? {
                    stick.ni_x_source_inverted = Some(*b);
                } else if let Some(JsValue::Bool(b)) = prop(raw, "xSourceInverted")? {
                    stick.ni_x_source_inverted = Some(*b);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "niYSourceIndex")? {
                    stick.ni_y_source_index = Some(*x);
                } else if let Some(JsValue::Number(x)) = prop(raw, "ySourceIndex")? {
                    stick.ni_y_source_index = Some(*x);
                }
                if let Some(JsValue::Bool(b)) = prop(raw, "niYSourceInverted")? {
                    stick.ni_y_source_inverted = Some(*b);
                } else if let Some(JsValue::Bool(b)) = prop(raw, "ySourceInverted")? {
                    stick.ni_y_source_inverted = Some(*b);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "niButtonSourceIndex")? {
                    stick.ni_button_source_index = Some(*x);
                } else if let Some(JsValue::Number(x)) = prop(raw, "buttonSourceIndex")? {
                    stick.ni_button_source_index = Some(*x);
                }
                config
                    .components
                    .push(ConfigJoystickComponent::Joystick(stick));
            } else if js_eq(kind, "axis") {
                let mut axis = ConfigJoystickAxis {
                    is_yellow,
                    center_px,
                    size_px: [0.0, 0.0],
                    sdl_source_index: None,
                    sdl_source_range: None,
                    ni_source_index: None,
                    ni_source_range: None,
                };
                if let Some(v) = num_array::<2>(raw, "sizePx")? {
                    axis.size_px = v;
                }
                if let Some(JsValue::Number(x)) = prop(raw, "sdlSourceIndex")? {
                    axis.sdl_source_index = Some(*x);
                }
                if let Some(v) = num_array::<2>(raw, "sdlSourceRange")? {
                    axis.sdl_source_range = Some(v);
                }
                if let Some(JsValue::Number(x)) = prop(raw, "niSourceIndex")? {
                    axis.ni_source_index = Some(*x);
                } else if let Some(JsValue::Number(x)) = prop(raw, "sourceIndex")? {
                    axis.ni_source_index = Some(*x);
                }
                if let Some(v) = num_array::<2>(raw, "niSourceRange")? {
                    axis.ni_source_range = Some(v);
                } else if let Some(v) = num_array::<2>(raw, "sourceRange")? {
                    axis.ni_source_range = Some(v);
                }
                config.components.push(ConfigJoystickComponent::Axis(axis));
            } else if js_eq(kind, "touchpad") {
                let mut touchpad = ConfigJoystickTouchpad {
                    is_yellow,
                    center_px,
                    size_px: [0.0, 0.0],
                    sdl_source_index: -1.0,
                };
                if let Some(v) = num_array::<2>(raw, "sizePx")? {
                    touchpad.size_px = v;
                }
                if let Some(JsValue::Number(x)) = prop(raw, "sdlSourceIndex")? {
                    touchpad.sdl_source_index = *x;
                }
                config
                    .components
                    .push(ConfigJoystickComponent::Touchpad(touchpad));
            }
        }
    }
    if !config.name.is_empty()
        && config.components.iter().all(|component| match component {
            ConfigJoystickComponent::Button(b) => {
                b.size_px[0] > 0.0
                    && b.size_px[1] > 0.0
                    && (b.sdl_source_index.is_some() || b.ni_source_index.is_some())
            }
            ConfigJoystickComponent::Joystick(j) => {
                j.radius_px > 0.0
                    && ((j.sdl_x_source_index.is_some() && j.sdl_y_source_index.is_some())
                        || (j.ni_x_source_index.is_some() && j.ni_y_source_index.is_some()))
            }
            ConfigJoystickComponent::Axis(a) => {
                a.size_px[0] > 0.0
                    && a.size_px[1] > 0.0
                    && (a.sdl_source_index.is_some() || a.ni_source_index.is_some())
            }
            ConfigJoystickComponent::Touchpad(t) => {
                t.size_px[0] > 0.0 && t.size_px[1] > 0.0 && t.sdl_source_index >= 0.0
            }
        })
    {
        return Ok(Parsed::Config(config));
    }
    Ok(Parsed::Invalid)
}

fn num_js(v: f64) -> JsValue {
    JsValue::Number(v)
}

fn array_js(items: Vec<JsValue>) -> JsValue {
    JsValue::Array(items)
}

fn vec2_js(v: [f64; 2]) -> JsValue {
    array_js(v.into_iter().map(num_js).collect())
}

fn vec3_js(v: [f64; 3]) -> JsValue {
    array_js(v.into_iter().map(num_js).collect())
}

fn rotations_js(rotations: &[Config3dRotation]) -> JsValue {
    array_js(rotations.iter().map(|r| r.raw.clone()).collect())
}

fn str_js(s: &JsString) -> JsValue {
    JsValue::String(s.clone())
}

impl Config2d {
    /// The `set-assets` object: upstream's literal key order with `sourceUrl`
    /// appended when present.
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("name", str_js(&self.name));
        o.insert("path", str_js(&self.path));
        o.insert("id", str_js(&self.id));
        o.insert("isFTC", JsValue::Bool(self.is_ftc));
        o.insert("useGrid", JsValue::Bool(self.use_grid));
        o.insert(
            "coordinateSystem",
            JsValue::from(self.coordinate_system.as_str()),
        );
        o.insert("topLeft", vec2_js(self.top_left));
        o.insert("bottomRight", vec2_js(self.bottom_right));
        o.insert("widthInches", num_js(self.width_inches));
        o.insert("heightInches", num_js(self.height_inches));
        if let Some(source_url) = &self.source_url {
            o.insert("sourceUrl", str_js(source_url));
        }
        JsValue::Object(o)
    }
}

impl Config3dFieldGamePiece {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("name", str_js(&self.name));
        o.insert("rotations", rotations_js(&self.rotations));
        o.insert("position", vec3_js(self.position));
        o.insert(
            "stagedObjects",
            array_js(self.staged_objects.iter().map(str_js).collect()),
        );
        JsValue::Object(o)
    }
}

impl Config3dFieldAprilTag {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("variant", str_js(&self.variant));
        o.insert("id", num_js(self.id));
        o.insert("rotations", rotations_js(&self.rotations));
        o.insert("position", vec3_js(self.position));
        JsValue::Object(o)
    }
}

impl Config3dField {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("name", str_js(&self.name));
        o.insert("path", str_js(&self.path));
        o.insert("id", str_js(&self.id));
        o.insert("isFTC", JsValue::Bool(self.is_ftc));
        o.insert(
            "coordinateSystem",
            JsValue::from(self.coordinate_system.as_str()),
        );
        o.insert("useGrid", JsValue::Bool(self.use_grid));
        o.insert("rotations", rotations_js(&self.rotations));
        o.insert("position", vec3_js(self.position));
        o.insert("widthInches", num_js(self.width_inches));
        o.insert("heightInches", num_js(self.height_inches));
        o.insert(
            "driverStations",
            array_js(self.driver_stations.iter().map(|p| vec2_js(*p)).collect()),
        );
        o.insert(
            "gamePieces",
            array_js(self.game_pieces.iter().map(|g| g.to_js()).collect()),
        );
        o.insert(
            "aprilTags",
            array_js(self.april_tags.iter().map(|t| t.to_js()).collect()),
        );
        JsValue::Object(o)
    }
}

impl Config3dRobotCamera {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("name", str_js(&self.name));
        o.insert("rotations", rotations_js(&self.rotations));
        o.insert("position", vec3_js(self.position));
        o.insert("resolution", vec2_js(self.resolution));
        o.insert("fov", num_js(self.fov));
        JsValue::Object(o)
    }
}

impl Config3dRobotComponent {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("zeroedRotations", rotations_js(&self.zeroed_rotations));
        o.insert("zeroedPosition", vec3_js(self.zeroed_position));
        JsValue::Object(o)
    }
}

impl Config3dRobot {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("name", str_js(&self.name));
        o.insert("path", str_js(&self.path));
        o.insert("isFTC", JsValue::Bool(self.is_ftc));
        o.insert("rotations", rotations_js(&self.rotations));
        o.insert("position", vec3_js(self.position));
        o.insert(
            "cameras",
            array_js(self.cameras.iter().map(|c| c.to_js()).collect()),
        );
        o.insert(
            "components",
            array_js(self.components.iter().map(|c| c.to_js()).collect()),
        );
        o.insert(
            "disableSimplification",
            JsValue::Bool(self.disable_simplification),
        );
        JsValue::Object(o)
    }
}

fn opt_num(o: &mut JsObject<JsValue>, key: &str, value: Option<f64>) {
    if let Some(x) = value {
        o.insert(key, num_js(x));
    }
}

fn opt_bool(o: &mut JsObject<JsValue>, key: &str, value: Option<bool>) {
    if let Some(b) = value {
        o.insert(key, JsValue::Bool(b));
    }
}

fn opt_pov(o: &mut JsObject<JsValue>, key: &str, value: Option<SourcePov>) {
    if let Some(p) = value {
        o.insert(key, JsValue::from(p.as_str()));
    }
}

impl ConfigJoystickComponent {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        match self {
            Self::Button(b) => {
                o.insert("type", JsValue::from("button"));
                o.insert("isYellow", JsValue::Bool(b.is_yellow));
                o.insert("isEllipse", JsValue::Bool(b.is_ellipse));
                o.insert("centerPx", vec2_js(b.center_px));
                o.insert("sizePx", vec2_js(b.size_px));
                opt_num(&mut o, "sdlSourceIndex", b.sdl_source_index);
                opt_pov(&mut o, "sdlSourcePov", b.sdl_source_pov);
                opt_num(&mut o, "niSourceIndex", b.ni_source_index);
                opt_pov(&mut o, "niSourcePov", b.ni_source_pov);
            }
            Self::Joystick(j) => {
                o.insert("type", JsValue::from("joystick"));
                o.insert("isYellow", JsValue::Bool(j.is_yellow));
                o.insert("centerPx", vec2_js(j.center_px));
                o.insert("radiusPx", num_js(j.radius_px));
                opt_num(&mut o, "sdlXSourceIndex", j.sdl_x_source_index);
                opt_bool(&mut o, "sdlXSourceInverted", j.sdl_x_source_inverted);
                opt_num(&mut o, "sdlYSourceIndex", j.sdl_y_source_index);
                opt_bool(&mut o, "sdlYSourceInverted", j.sdl_y_source_inverted);
                opt_num(&mut o, "sdlButtonSourceIndex", j.sdl_button_source_index);
                opt_num(&mut o, "niXSourceIndex", j.ni_x_source_index);
                opt_bool(&mut o, "niXSourceInverted", j.ni_x_source_inverted);
                opt_num(&mut o, "niYSourceIndex", j.ni_y_source_index);
                opt_bool(&mut o, "niYSourceInverted", j.ni_y_source_inverted);
                opt_num(&mut o, "niButtonSourceIndex", j.ni_button_source_index);
            }
            Self::Axis(a) => {
                o.insert("type", JsValue::from("axis"));
                o.insert("isYellow", JsValue::Bool(a.is_yellow));
                o.insert("centerPx", vec2_js(a.center_px));
                o.insert("sizePx", vec2_js(a.size_px));
                opt_num(&mut o, "sdlSourceIndex", a.sdl_source_index);
                if let Some(range) = a.sdl_source_range {
                    o.insert("sdlSourceRange", vec2_js(range));
                }
                opt_num(&mut o, "niSourceIndex", a.ni_source_index);
                if let Some(range) = a.ni_source_range {
                    o.insert("niSourceRange", vec2_js(range));
                }
            }
            Self::Touchpad(t) => {
                o.insert("type", JsValue::from("touchpad"));
                o.insert("isYellow", JsValue::Bool(t.is_yellow));
                o.insert("centerPx", vec2_js(t.center_px));
                o.insert("sizePx", vec2_js(t.size_px));
                o.insert("sdlSourceIndex", num_js(t.sdl_source_index));
            }
        }
        JsValue::Object(o)
    }
}

impl ConfigJoystick {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("name", str_js(&self.name));
        o.insert("path", str_js(&self.path));
        o.insert(
            "components",
            array_js(self.components.iter().map(|c| c.to_js()).collect()),
        );
        JsValue::Object(o)
    }
}

impl AdvantageScopeAssets {
    pub fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert(
            "field2ds",
            array_js(self.field2ds.iter().map(|f| f.to_js()).collect()),
        );
        o.insert(
            "field3ds",
            array_js(self.field3ds.iter().map(|f| f.to_js()).collect()),
        );
        o.insert(
            "robots",
            array_js(self.robots.iter().map(|r| r.to_js()).collect()),
        );
        o.insert(
            "joysticks",
            array_js(self.joysticks.iter().map(|j| j.to_js()).collect()),
        );
        o.insert(
            "loadFailures",
            array_js(self.load_failures.iter().map(str_js).collect()),
        );
        JsValue::Object(o)
    }
}
