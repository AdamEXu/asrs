//! `LineGraphRendererCommand` and its parts, with `to_js` in upstream's key order.

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::units::UnitConfig;
use asrs_model::selection::SelectionMode;

/// `LineGraphRendererCommand_NumericField` plus the `hasUnit` flag the controller leaves on it.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericField {
    pub timestamps: Vec<f64>,
    pub values: Vec<f64>,
    pub color: String,
    /// `"stepped"`, `"smooth"` or `"points"`, or whatever type a restored item has.
    pub kind: JsString,
    pub size: JsString,
    pub has_unit: bool,
}

impl ToJs for NumericField {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("timestamps", self.timestamps.to_js());
        o.insert("values", self.values.to_js());
        o.insert("color", self.color.to_js());
        o.insert("type", self.kind.to_js());
        o.insert("size", self.size.to_js());
        o.insert("hasUnit", self.has_unit.to_js());
        JsValue::Object(o)
    }
}

/// The robot mode stripes' `colorMap`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RobotModeColors {
    pub disabled: &'static str,
    pub teleop: &'static str,
    pub auto: &'static str,
    pub utility: &'static str,
}

impl RobotModeColors {
    pub fn for_theme(dark: bool) -> RobotModeColors {
        if dark {
            RobotModeColors {
                disabled: "#444444",
                teleop: "#3957dd",
                auto: "#c38615",
                utility: "#777777",
            }
        } else {
            RobotModeColors {
                disabled: "#dddddd",
                teleop: "#5a83ff",
                auto: "#efab28",
                utility: "#888888",
            }
        }
    }
}

impl ToJs for RobotModeColors {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("Disabled", self.disabled.to_js());
        o.insert("Teleop", self.teleop.to_js());
        o.insert("Auto", self.auto.to_js());
        o.insert("Utility", self.utility.to_js());
        JsValue::Object(o)
    }
}

/// `LineGraphRendererCommand_DiscreteField`.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscreteField {
    pub timestamps: Vec<f64>,
    pub values: Vec<String>,
    pub color: String,
    /// Only on the robot mode stripes.
    pub color_map: Option<RobotModeColors>,
    /// `"stripes"` or `"graph"`, or whatever type a restored item has.
    pub kind: JsString,
    pub toggle_reference: bool,
}

impl ToJs for DiscreteField {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("timestamps", self.timestamps.to_js());
        o.insert("values", self.values.to_js());
        o.insert("color", self.color.to_js());
        if let Some(map) = &self.color_map {
            o.insert("colorMap", map.to_js());
        }
        o.insert("type", self.kind.to_js());
        o.insert("toggleReference", self.toggle_reference.to_js());
        JsValue::Object(o)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertType {
    Error,
    Warning,
    Info,
}

impl AlertType {
    pub const ALL: [AlertType; 3] = [AlertType::Error, AlertType::Warning, AlertType::Info];

    pub fn name(self) -> &'static str {
        match self {
            AlertType::Error => "error",
            AlertType::Warning => "warning",
            AlertType::Info => "info",
        }
    }
}

/// `LineGraphRendererCommand_Alert`.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub kind: AlertType,
    pub text: JsString,
    pub range: [f64; 2],
}

impl ToJs for Alert {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("type", self.kind.name().to_js());
        o.insert("text", self.text.to_js());
        o.insert("range", self.range.to_js());
        JsValue::Object(o)
    }
}

/// An axis's `Units.UnitConfig`: `null`, or `undefined` when a manual preset names a unit
/// `ALL_UNITS` lacks.
#[derive(Clone, Copy, Debug)]
pub enum AxisUnits {
    None,
    Unknown,
    Unit(&'static UnitConfig),
}

impl PartialEq for AxisUnits {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AxisUnits::Unit(a), AxisUnits::Unit(b)) => std::ptr::eq(*a, *b),
            (AxisUnits::None, AxisUnits::None) | (AxisUnits::Unknown, AxisUnits::Unknown) => true,
            _ => false,
        }
    }
}

impl ToJs for AxisUnits {
    fn to_js(&self) -> JsValue {
        match self {
            AxisUnits::None => JsValue::Null,
            AxisUnits::Unknown => JsValue::Undefined,
            AxisUnits::Unit(config) => {
                let mut o = JsObject::new();
                o.insert("value", config.value.to_js());
                o.insert("suffix", config.suffix.to_js());
                o.insert("pluralizeSuffix", config.pluralize_suffix.to_js());
                o.insert(
                    "names",
                    JsValue::Array(config.names.iter().map(|n| n.to_js()).collect()),
                );
                JsValue::Object(o)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Left,
    Right,
}

impl Axis {
    pub fn name(self) -> &'static str {
        match self {
            Axis::Left => "left",
            Axis::Right => "right",
        }
    }
}

/// `LineGraphRendererCommand`.
#[derive(Clone, Debug, PartialEq)]
pub struct LineGraphCommand {
    pub time_range: [f64; 2],
    pub display_offset: f64,
    pub selection_mode: SelectionMode,
    pub selected_time: Option<f64>,
    pub hovered_time: Option<f64>,
    pub grab_zoom_range: Option<[f64; 2]>,
    pub left_range: [f64; 2],
    pub right_range: [f64; 2],
    pub show_left_axis: bool,
    pub show_right_axis: bool,
    pub left_units: AxisUnits,
    pub right_units: AxisUnits,
    /// The axes' raw filter values (any value a layout restored).
    pub left_filter: JsValue,
    pub right_filter: JsValue,
    pub priority_axis: Axis,
    pub left_fields: Vec<NumericField>,
    pub right_fields: Vec<NumericField>,
    pub discrete_fields: Vec<DiscreteField>,
    pub alerts: Vec<Vec<Alert>>,
}

fn nullable<T: ToJs>(value: &Option<T>) -> JsValue {
    value.as_ref().map_or(JsValue::Null, ToJs::to_js)
}

impl ToJs for LineGraphCommand {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("timeRange", self.time_range.to_js());
        o.insert("displayOffset", self.display_offset.to_js());
        o.insert("selectionMode", self.selection_mode.to_js());
        o.insert("selectedTime", nullable(&self.selected_time));
        o.insert("hoveredTime", nullable(&self.hovered_time));
        o.insert("grabZoomRange", nullable(&self.grab_zoom_range));
        o.insert("leftRange", self.left_range.to_js());
        o.insert("rightRange", self.right_range.to_js());
        o.insert("showLeftAxis", self.show_left_axis.to_js());
        o.insert("showRightAxis", self.show_right_axis.to_js());
        o.insert("leftUnits", self.left_units.to_js());
        o.insert("rightUnits", self.right_units.to_js());
        o.insert("leftFilter", self.left_filter.clone());
        o.insert("rightFilter", self.right_filter.clone());
        o.insert("priorityAxis", self.priority_axis.name().to_js());
        o.insert("leftFields", self.left_fields.to_js());
        o.insert("rightFields", self.right_fields.to_js());
        o.insert("discreteFields", self.discrete_fields.to_js());
        o.insert("alerts", self.alerts.to_js());
        JsValue::Object(o)
    }
}
