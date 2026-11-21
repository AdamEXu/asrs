//! An axis's saved settings. Upstream stores whatever `restoreState` or an `edit-axis` message
//! hands it and saves it back untouched, so each setting keeps its raw value next to the
//! reading the command math uses.

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_log::line_graph_filter::LineGraphFilter;
use asrs_log::units::{self, TempUnit};

pub(crate) fn unmodeled(what: &str) -> JsError {
    JsError::new("Unmodeled", format!("line graph: {what}"))
}

/// `leftLockedRange` / `rightLockedRange`.
#[derive(Clone, Debug, PartialEq)]
pub enum LockedRange {
    Unlocked,
    Locked([f64; 2]),
    /// Anything else a layout can hold (`[null, 5]`, a string, ...). Saved back as is; the
    /// command math for it is not ported.
    Unmodeled(JsValue),
}

impl LockedRange {
    pub fn from_js(value: &JsValue) -> LockedRange {
        match value {
            JsValue::Null => LockedRange::Unlocked,
            JsValue::Array(items) => match items.as_slice() {
                [JsValue::Number(min), JsValue::Number(max)] => LockedRange::Locked([*min, *max]),
                _ => LockedRange::Unmodeled(value.clone()),
            },
            _ => LockedRange::Unmodeled(value.clone()),
        }
    }

    pub fn to_js(&self) -> JsValue {
        match self {
            LockedRange::Unlocked => JsValue::Null,
            LockedRange::Locked([min, max]) => {
                JsValue::Array(vec![JsValue::Number(*min), JsValue::Number(*max)])
            }
            LockedRange::Unmodeled(value) => value.clone(),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, LockedRange::Unlocked)
    }
}

/// `leftFilter` / `rightFilter`: a `LineGraphFilter`, or any value a layout restored.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter(pub JsValue);

impl Filter {
    pub const NONE: Filter = Filter(JsValue::Number(0.0));

    fn is(&self, n: f64) -> bool {
        matches!(self.0, JsValue::Number(v) if v == n)
    }

    /// `filter === LineGraphFilter.None`.
    pub fn is_none(&self) -> bool {
        self.is(0.0)
    }

    /// The `switch (filter)` branch taken; values matching no case behave as `None`.
    pub fn kind(&self) -> LineGraphFilter {
        if self.is(1.0) {
            LineGraphFilter::Differentiate
        } else if self.is(2.0) {
            LineGraphFilter::Integrate
        } else {
            LineGraphFilter::None
        }
    }
}

/// `Units.UnitConversionPreset` as read by the command: `type === null`, `from`, `to` and
/// `factor`.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub type_is_null: bool,
    pub from: Option<String>,
    pub to: Option<String>,
    pub factor: f64,
}

impl Preset {
    /// `Units.convertWithPreset`; an unknown unit throws the string upstream throws.
    pub fn convert(&self, value: f64) -> Result<f64, JsError> {
        fn arg(name: &Option<String>, to: bool) -> Option<(&str, TempUnit)> {
            name.as_deref().map(|key| {
                (
                    key,
                    if to {
                        TempUnit::to_arg(key, true)
                    } else {
                        TempUnit::from_arg(key, true)
                    },
                )
            })
        }
        units::convert_with_preset(
            value,
            self.type_is_null,
            arg(&self.from, false),
            arg(&self.to, true),
            self.factor,
        )
        .map_err(|_| JsError::new("", "Invalid unit provided"))
    }

    /// `preset.type !== null || preset.factor !== 1`.
    pub fn converts(&self) -> bool {
        !self.type_is_null || self.factor != 1.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PresetValue {
    Null,
    /// The property is missing (`undefined`).
    Missing,
    Preset(Preset),
}

/// `Units.UIUnitOptions` as read by the controller.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitOptions {
    /// `autoTarget` when it is a string. Any other value fails `autoTarget in GROUP_BY_UNIT`
    /// exactly as `null` skips it.
    pub auto_target: Option<String>,
    pub preset: PresetValue,
}

/// `leftUnitConversion` / `rightUnitConversion`.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitConversion {
    pub raw: JsValue,
    pub options: Result<UnitOptions, &'static str>,
}

impl Default for UnitConversion {
    fn default() -> Self {
        let mut o = asrs_js::value::JsObject::new();
        o.insert("autoTarget", JsValue::Null);
        o.insert("preset", JsValue::Null);
        UnitConversion::from_js(&JsValue::Object(o))
    }
}

fn optional_string(value: Option<&JsValue>) -> Result<Option<String>, &'static str> {
    match value {
        None | Some(JsValue::Undefined) => Ok(None),
        Some(JsValue::String(s)) => Ok(Some(s.to_string_lossy())),
        Some(_) => Err("a unit preset with a non-string unit"),
    }
}

impl UnitConversion {
    pub fn from_js(value: &JsValue) -> UnitConversion {
        let options = (|| {
            let o = value
                .as_object()
                .ok_or("a unit conversion that is not an object")?;
            let auto_target = match o.get("autoTarget") {
                Some(JsValue::String(s)) => Some(s.to_string_lossy()),
                _ => None,
            };
            let preset = match o.get("preset") {
                None | Some(JsValue::Undefined) => PresetValue::Missing,
                Some(JsValue::Null) => PresetValue::Null,
                Some(JsValue::Object(p)) => {
                    let type_is_null = match p.get("type") {
                        Some(JsValue::Null) => true,
                        None | Some(JsValue::Undefined | JsValue::String(_)) => false,
                        Some(_) => return Err("a unit preset with a non-string type"),
                    };
                    let factor = match p.get("factor") {
                        Some(JsValue::Number(f)) => *f,
                        _ => return Err("a unit preset without a numeric factor"),
                    };
                    PresetValue::Preset(Preset {
                        type_is_null,
                        from: optional_string(p.get("from"))?,
                        to: optional_string(p.get("to"))?,
                        factor,
                    })
                }
                Some(_) => return Err("a unit preset that is not an object"),
            };
            Ok(UnitOptions {
                auto_target,
                preset,
            })
        })();
        UnitConversion {
            raw: value.clone(),
            options,
        }
    }

    pub fn options(&self) -> Result<&UnitOptions, JsError> {
        self.options.as_ref().map_err(|what| unmodeled(what))
    }
}

/// One axis's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisSettings {
    pub locked_range: LockedRange,
    pub unit_conversion: UnitConversion,
    pub filter: Filter,
}

impl Default for AxisSettings {
    fn default() -> Self {
        AxisSettings {
            locked_range: LockedRange::Unlocked,
            unit_conversion: UnitConversion::default(),
            filter: Filter::NONE,
        }
    }
}

impl AxisSettings {
    /// The bracketed labels `updateAxisLabels` appends to the axis title.
    pub fn labels(&self) -> Result<Vec<&'static str>, JsError> {
        let mut labels = Vec::new();
        if !self.locked_range.is_null() {
            labels.push("Locked");
        }
        match &self.unit_conversion.raw {
            JsValue::Null | JsValue::Undefined => {
                return Err(JsError::type_error(format!(
                    "Cannot read properties of {} (reading 'preset')",
                    if self.unit_conversion.raw.is_null() {
                        "null"
                    } else {
                        "undefined"
                    }
                )));
            }
            _ => match &self.unit_conversion.options()?.preset {
                PresetValue::Null => {}
                PresetValue::Missing => {
                    return Err(JsError::type_error(
                        "Cannot read properties of undefined (reading 'type')",
                    ));
                }
                PresetValue::Preset(p) => labels.push(if p.converts() {
                    "Converted"
                } else {
                    "No Units"
                }),
            },
        }
        match self.filter.kind() {
            LineGraphFilter::Differentiate => labels.push("Differentiated"),
            LineGraphFilter::Integrate => labels.push("Integrated"),
            LineGraphFilter::None => {}
        }
        Ok(labels)
    }
}
