//! `src/hub/controllers/LineGraphController.ts` with `LineGraphController_Config.ts`: the three
//! source lists, the axis settings, and the per-frame command (filters, unit conversion,
//! AdvantageKit resampling, robot mode stripes, discrete fields and alerts), plus the hover
//! previews and the `ask-edit-axis` message.

mod axis;
mod command;

use std::rc::Rc;

use asrs_js::math;
use asrs_js::number::{self, IntlOptions};
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Rng, create_uuid};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::line_graph_filter::LineGraphFilter;
use asrs_log::log::log_util::get_log_value_text;
use asrs_log::log::robot_state::{AKIT_TIMESTAMP_KEYS, RobotMode, get_robot_state_ranges};
use asrs_log::log::{Log, LogValueSet, LogValueSetAny};
use asrs_log::units::{self, UnitConfig};
use asrs_log::util::{clamp_value, lower_bound, scale_value_clamped, upper_bound};
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::source_list::{Env, Rect, SourceList, configs};
use asrs_model::source_list_config::SourceListItemState;
use asrs_model::tab_controller::{Command, TabController};

pub use axis::{
    AxisSettings, Filter, LockedRange, Preset, PresetValue, UnitConversion, UnitOptions,
};
pub use command::{
    Alert, AlertType, Axis, AxisUnits, DiscreteField, LineGraphCommand, NumericField,
    RobotModeColors,
};

use crate::cx::{TabCx, display_offset};
use crate::list_cx::{ListParts, SourceListCx, js_error, with_env};
use axis::unmodeled;

const RANGE_MARGIN: f64 = 0.05;
const MIN_AXIS_RANGE: f64 = 1e-5;
const MAX_AXIS_RANGE: f64 = 1e20;
const MAX_VALUE: f64 = 1e20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListId {
    Left,
    Right,
    Discrete,
}

impl ListId {
    pub fn legend(self) -> &'static str {
        match self {
            ListId::Left => "left",
            ListId::Right => "right",
            ListId::Discrete => "discrete",
        }
    }
}

/// What the last command left for an axis's hover previews: the numeric command of every
/// field by key (`leftNumericCommandCache`) and the axis unit (`leftUnitCache`).
#[derive(Clone, Debug)]
struct PreviewCache {
    fields: JsObject<NumericField>,
    units: AxisUnits,
}

impl Default for PreviewCache {
    fn default() -> Self {
        PreviewCache {
            fields: JsObject::new(),
            units: AxisUnits::None,
        }
    }
}

enum AutoTarget {
    Success(&'static str),
    None,
    Inconsistent,
    Manual,
}

fn str_is(s: &JsString, other: &str) -> bool {
    s.units().iter().copied().eq(other.encode_utf16())
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn truthy(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::String(s) => !s.is_empty(),
        JsValue::BigInt(b) => !b.is_zero(),
        _ => true,
    }
}

fn static_unit(unit: &str) -> Option<(&'static str, &'static UnitConfig)> {
    units::all_units()
        .iter()
        .find(|(key, _)| *key == unit)
        .copied()
}

fn is_available(log: &Log, item: &SourceListItemState) -> bool {
    item.visible && log.get_field(&item.log_key).is_some()
}

/// `getAxisUnitAutoTarget`.
fn auto_target(
    log: &Log,
    source: &[SourceListItemState],
    options: &UnitOptions,
    ignore_preset: bool,
) -> AutoTarget {
    if !ignore_preset && options.preset != PresetValue::Null {
        return AutoTarget::Manual;
    }
    let visible: Vec<&SourceListItemState> = source
        .iter()
        .filter(|item| is_available(log, item))
        .collect();
    if visible.is_empty() {
        return AutoTarget::None;
    }
    if let Some((target, _)) = options.auto_target.as_deref().and_then(static_unit) {
        let group = units::group_by_unit(target);
        if visible
            .iter()
            .all(|item| log.get_unit(&item.log_key).and_then(units::group_by_unit) == group)
        {
            return AutoTarget::Success(target);
        }
    }
    let mut first: Option<(&'static str, &'static str)> = None;
    for item in &visible {
        let Some((unit, group)) = log
            .get_unit(&item.log_key)
            .and_then(|unit| Some((unit, units::group_by_unit(unit)?)))
        else {
            return match first {
                None => AutoTarget::None,
                Some(_) => AutoTarget::Inconsistent,
            };
        };
        match first {
            None => first = Some((unit, group)),
            Some((_, first_group)) if group != first_group => return AutoTarget::Inconsistent,
            Some(_) => {}
        }
    }
    match first {
        Some((unit, _)) => AutoTarget::Success(unit),
        None => AutoTarget::None,
    }
}

fn theme_color(item: &SourceListItemState, dark: bool) -> Result<String, JsError> {
    match item.options.get("color") {
        Some(color) => Ok(asrs_log::colors::ensure_theme_contrast(
            &color.to_string_lossy(),
            dark,
        )),
        None => Err(JsError::type_error(
            "Cannot read properties of undefined (reading 'slice')",
        )),
    }
}

/// `limitAxisRange`.
fn limit_axis_range(range: [f64; 2]) -> [f64; 2] {
    let mut r = range;
    if r[0] > MAX_VALUE {
        r[0] = MAX_VALUE;
    }
    if r[1] > MAX_VALUE {
        r[1] = MAX_VALUE;
    }
    if r[0] < -MAX_VALUE {
        r[0] = -MAX_VALUE;
    }
    if r[1] < -MAX_VALUE {
        r[1] = -MAX_VALUE;
    }
    if r[0] == r[1] {
        if r[0].abs() >= MAX_VALUE {
            if r[0] > 0.0 {
                r[0] *= 0.8;
            } else {
                r[1] *= 0.8;
            }
        } else {
            r[0] -= 1.0;
            r[1] += 1.0;
        }
    }
    if r[1] - r[0] > MAX_AXIS_RANGE {
        if r[0] + MAX_AXIS_RANGE < MAX_VALUE {
            r[1] = r[0] + MAX_AXIS_RANGE;
        } else {
            r[0] = r[1] - MAX_AXIS_RANGE;
        }
    }
    if r[1] - r[0] < MIN_AXIS_RANGE {
        r[1] = r[0] + MIN_AXIS_RANGE;
    }
    r
}

/// `calcRange`. A locked range is used in place, so its non-finite ends are overwritten in
/// the controller's state too.
fn calc_range(data_range: [f64; 2], locked: &mut LockedRange) -> Result<[f64; 2], JsError> {
    let fix = |r: &mut [f64; 2]| {
        if !r[0].is_finite() {
            r[0] = -1.0;
        }
        if !r[1].is_finite() {
            r[1] = 1.0;
        }
    };
    let range = match locked {
        LockedRange::Unlocked => {
            let margin = (data_range[1] - data_range[0]) * RANGE_MARGIN;
            let mut r = [data_range[0] - margin, data_range[1] + margin];
            fix(&mut r);
            r
        }
        LockedRange::Locked(r) => {
            fix(r);
            *r
        }
        LockedRange::Unmodeled(_) => {
            return Err(unmodeled("a locked range that is not [number, number]"));
        }
    };
    Ok(limit_axis_range(range))
}

fn robot_mode_name(mode: RobotMode) -> &'static str {
    match mode {
        RobotMode::Disabled => "Disabled",
        RobotMode::Auto => "Auto",
        RobotMode::Teleop => "Teleop",
        RobotMode::Utility => "Utility",
    }
}

fn texts<T: asrs_log::log::log_util::LogValueText>(set: LogValueSet<T>) -> (Vec<f64>, Vec<String>) {
    let values = set.values.iter().map(get_log_value_text).collect();
    (set.timestamps, values)
}

fn preview_options() -> IntlOptions {
    IntlOptions {
        max_fraction: 6,
        grouping: false,
        ..IntlOptions::default()
    }
}

/// `getPreview`.
fn number_preview(
    cache: &PreviewCache,
    filter: &Filter,
    selection: &mut dyn Selection,
    key: &JsString,
    time: f64,
) -> Option<String> {
    let field = cache.fields.get_key(key)?;
    if time < selection.get_timeline_range()[0]
        || (time > selection.get_timeline_range()[1]
            && selection.get_mode() != SelectionMode::Locked)
    {
        return None;
    }
    let index = upper_bound(&field.timestamps, time, 0);
    if index == 0 {
        return None;
    }
    let value = field.values.get(index - 1).copied().unwrap_or(f64::NAN);
    let mut output = number::intl_format(value, preview_options());
    if field.has_unit {
        match cache.units {
            AxisUnits::None => {}
            AxisUnits::Unknown => return None,
            AxisUnits::Unit(unit) => {
                let suffix = units::get_suffix_for_filter(&js(unit.suffix), filter.kind());
                if suffix.len() > 1 {
                    output.push(' ');
                }
                output.push_str(&suffix.to_string_lossy());
                if filter.is_none() && unit.pluralize_suffix && value != 1.0 {
                    output.push('s');
                }
            }
        }
    }
    Some(output)
}

pub struct LineGraphController {
    left_list: SourceList,
    right_list: SourceList,
    discrete_list: SourceList,
    left: AxisSettings,
    right: AxisSettings,
    show_robot_mode: JsValue,
    left_cache: PreviewCache,
    right_cache: PreviewCache,
}

impl LineGraphController {
    /// `new LineGraphController(root)`: the three lists draw their UUIDs in construction order
    /// (left, right, discrete). The controller's own `UUID` belongs to `Tabs`.
    pub fn new(rng: &mut dyn Rng) -> Self {
        let left = create_uuid(rng);
        let right = create_uuid(rng);
        let discrete = create_uuid(rng);
        Self::with_uuids([left, right, discrete])
    }

    pub fn with_uuids([left, right, discrete]: [String; 3]) -> Self {
        let mut left_list = SourceList::new(configs::line_graph_numeric(), left, true);
        left_list.set_title("Left Axis");
        let mut right_list = SourceList::new(configs::line_graph_numeric(), right, true);
        right_list.set_title("Right Axis");
        let discrete_list = SourceList::new(configs::line_graph_discrete(), discrete, true);
        LineGraphController {
            left_list,
            right_list,
            discrete_list,
            left: AxisSettings::default(),
            right: AxisSettings::default(),
            show_robot_mode: JsValue::Bool(false),
            left_cache: PreviewCache::default(),
            right_cache: PreviewCache::default(),
        }
    }

    pub fn list(&self, list: ListId) -> &SourceList {
        match list {
            ListId::Left => &self.left_list,
            ListId::Right => &self.right_list,
            ListId::Discrete => &self.discrete_list,
        }
    }

    /// Runs `f` on one list with the states of the other two, which upstream passes to each
    /// list as its supplemental state suppliers (left: right, discrete; right: left,
    /// discrete; discrete: left, right).
    pub fn with_list<R>(
        &mut self,
        list: ListId,
        f: impl FnOnce(&mut SourceList, [&[SourceListItemState]; 2]) -> R,
    ) -> R {
        match list {
            ListId::Left => f(
                &mut self.left_list,
                [self.right_list.state(), self.discrete_list.state()],
            ),
            ListId::Right => f(
                &mut self.right_list,
                [self.left_list.state(), self.discrete_list.state()],
            ),
            ListId::Discrete => f(
                &mut self.discrete_list,
                [self.left_list.state(), self.right_list.state()],
            ),
        }
    }

    pub fn axis(&self, axis: Axis) -> &AxisSettings {
        match axis {
            Axis::Left => &self.left,
            Axis::Right => &self.right,
        }
    }

    pub fn show_robot_mode(&self) -> &JsValue {
        &self.show_robot_mode
    }

    /// One run of a list's animation-frame loop, with this controller's hover previews.
    pub fn frame_list<C: SourceListCx + ?Sized>(
        &mut self,
        cx: &mut C,
        list: ListId,
        render_time: Option<f64>,
    ) -> Result<(), JsError> {
        self.frame_list_observed(cx, list, render_time, |_, _, _| {})
    }

    /// [`Self::frame_list`], reporting each preview the list asks for.
    pub fn frame_list_observed<C: SourceListCx + ?Sized>(
        &mut self,
        cx: &mut C,
        list: ListId,
        render_time: Option<f64>,
        mut observe: impl FnMut(&JsString, f64, Option<&str>),
    ) -> Result<(), JsError> {
        let ListParts {
            log,
            type_memory,
            selection,
            dark,
        } = cx.list_parts();
        let mut env = Env {
            log: &log,
            type_memory,
            dark,
        };
        let (source_list, preview) = match list {
            ListId::Left => (
                &mut self.left_list,
                Some((&self.left_cache, &self.left.filter)),
            ),
            ListId::Right => (
                &mut self.right_list,
                Some((&self.right_cache, &self.right.filter)),
            ),
            ListId::Discrete => (&mut self.discrete_list, None),
        };
        let result = match preview {
            Some((cache, filter)) => {
                let mut preview = |key: &JsString, time: f64| {
                    let text = number_preview(cache, filter, selection, key, time);
                    observe(key, time, text.as_deref());
                    text
                };
                source_list.frame(&mut env, render_time, Some(&mut preview))
            }
            None => source_list.frame(&mut env, render_time, None),
        };
        result.map_err(js_error)
    }

    /// `updateAxisLabels`.
    fn update_axis_labels(&mut self) -> Result<(), JsError> {
        for (axis, list, name) in [
            (&self.left, &mut self.left_list, "Left Axis"),
            (&self.right, &mut self.right_list, "Right Axis"),
        ] {
            let labels = axis.labels()?;
            if labels.is_empty() {
                list.set_title(name);
            } else {
                list.set_title(&format!("{name} [{}]", labels.join(", ")));
            }
        }
        Ok(())
    }

    /// `editAxis(legend, lockedRange, unitConversion, filter)`, from an `edit-axis` message.
    pub fn edit_axis<C: SourceListCx + ?Sized>(
        &mut self,
        cx: &mut C,
        legend: &JsValue,
        locked_range: &JsValue,
        unit_conversion: &JsValue,
        filter: &JsValue,
    ) -> Result<(), JsError> {
        let axis = match legend {
            JsValue::String(s) if str_is(s, "left") => Some(Axis::Left),
            JsValue::String(s) if str_is(s, "right") => Some(Axis::Right),
            _ => None,
        };
        if let Some(axis) = axis {
            let locked = match locked_range {
                JsValue::Null => LockedRange::Unlocked,
                JsValue::Array(items)
                    if matches!(items.first(), Some(JsValue::Null))
                        && matches!(items.get(1), Some(JsValue::Null)) =>
                {
                    let command = self.get_command(cx)?;
                    LockedRange::Locked(match axis {
                        Axis::Left => command.left_range,
                        Axis::Right => command.right_range,
                    })
                }
                other => LockedRange::from_js(other),
            };
            let settings = match axis {
                Axis::Left => &mut self.left,
                Axis::Right => &mut self.right,
            };
            settings.locked_range = locked;
            settings.unit_conversion = UnitConversion::from_js(unit_conversion);
            settings.filter = Filter(filter.clone());
        }
        self.update_axis_labels()
    }

    /// `clearAxis(legend)`.
    pub fn clear_axis(&mut self, legend: &JsValue) {
        let JsValue::String(legend) = legend else {
            return;
        };
        for id in [ListId::Left, ListId::Right, ListId::Discrete] {
            if str_is(legend, id.legend()) {
                self.with_list(id, |list, _| list.clear());
            }
        }
    }

    /// `setRobotModeVisible(showRobotMode)`. Upstream then refreshes the controller on the next
    /// animation frame; the hub schedules that `refresh`.
    pub fn set_robot_mode_visible(&mut self, show_robot_mode: JsValue) {
        self.show_robot_mode = show_robot_mode;
    }

    /// The data of the `ask-edit-axis` message the list's edit button sends, given the button's
    /// rectangle.
    pub fn ask_edit_axis<C: TabCx + ?Sized>(
        &self,
        cx: &C,
        list: ListId,
        rect: Rect,
    ) -> Result<JsValue, JsError> {
        let mut rect_object = JsObject::new();
        rect_object.insert("x", JsValue::Number(rect.x));
        rect_object.insert("y", JsValue::Number(rect.y));
        rect_object.insert("width", JsValue::Number(rect.width));
        rect_object.insert("height", JsValue::Number(rect.height));
        let mut o = JsObject::new();
        o.insert("rect", JsValue::Object(rect_object));
        o.insert("legend", list.legend().to_js());
        let (settings, source) = match list {
            ListId::Discrete => {
                o.insert("showRobotMode", self.show_robot_mode.clone());
                o.insert("config", configs::line_graph_discrete().to_js());
                return Ok(JsValue::Object(o));
            }
            ListId::Left => (&self.left, &self.left_list),
            ListId::Right => (&self.right, &self.right_list),
        };
        let log = cx.tab_log();
        let options = settings.unit_conversion.options()?;
        let (group, selected) = match auto_target(&log, source.state(), options, true) {
            AutoTarget::Success(unit) => (
                units::group_by_unit(unit).to_js(),
                if options.preset == PresetValue::Null {
                    unit.to_js()
                } else {
                    JsValue::Null
                },
            ),
            AutoTarget::Inconsistent => ("inconsistent".to_js(), JsValue::Null),
            AutoTarget::None | AutoTarget::Manual => ("none".to_js(), JsValue::Null),
        };
        let visible: Vec<&SourceListItemState> = source
            .state()
            .iter()
            .filter(|item| is_available(&log, item))
            .collect();
        let default = match visible.as_slice() {
            [item] => log
                .get_unit(&item.log_key)
                .map_or(JsValue::Null, |unit| unit.to_js()),
            _ => JsValue::Null,
        };
        let locked = settings.locked_range.to_js();
        let unit_conversion = settings.unit_conversion.raw.clone();
        if list == ListId::Left {
            o.insert("lockedRange", locked);
            o.insert("autoUnitGroup", group);
            o.insert("autoUnitSelected", selected);
            o.insert("autoUnitDefault", default);
            o.insert("unitConversion", unit_conversion);
        } else {
            o.insert("lockedRange", locked);
            o.insert("unitConversion", unit_conversion);
            o.insert("autoUnitGroup", group);
            o.insert("autoUnitSelected", selected);
            o.insert("autoUnitDefault", default);
        }
        o.insert("filter", settings.filter.0.clone());
        o.insert("config", configs::line_graph_numeric().to_js());
        Ok(JsValue::Object(o))
    }

    /// `getCommand()`.
    pub fn get_command<C: SourceListCx + ?Sized>(
        &mut self,
        cx: &mut C,
    ) -> Result<LineGraphCommand, JsError> {
        let dark = cx.tab_dark();
        let time_range = cx.tab_selection().get_timeline_range();
        self.left_cache = PreviewCache::default();
        self.right_cache = PreviewCache::default();

        let (left_fields, left_data, right_fields, right_data, discrete_fields, alerts) = {
            let log = cx.tab_log();
            let akit_timestamps = log
                .field_keys()
                .find(|key| AKIT_TIMESTAMP_KEYS.iter().any(|k| str_is(key, k)))
                .and_then(|key| log.get_number(key, f64::NEG_INFINITY, f64::INFINITY, None, None))
                .map(|data| data.timestamps);
            let (left_fields, left_data) = add_numeric(
                &log,
                self.left_list.state(),
                &self.left,
                &mut self.left_cache,
                dark,
                time_range,
                akit_timestamps.as_deref(),
            )?;
            let (right_fields, right_data) = add_numeric(
                &log,
                self.right_list.state(),
                &self.right,
                &mut self.right_cache,
                dark,
                time_range,
                akit_timestamps.as_deref(),
            )?;
            let mut discrete_fields = Vec::new();
            if truthy(&self.show_robot_mode) {
                discrete_fields.extend(robot_mode_field(&log, time_range, dark));
            }
            add_discrete(
                &log,
                self.discrete_list.state(),
                time_range,
                dark,
                &mut discrete_fields,
            )?;
            let alerts = add_alerts(&log, self.discrete_list.state(), time_range);
            (
                left_fields,
                left_data,
                right_fields,
                right_data,
                discrete_fields,
                alerts,
            )
        };

        let left_range = calc_range(left_data, &mut self.left.locked_range)?;
        let right_range = calc_range(right_data, &mut self.right.locked_range)?;
        let mut show_left_axis = !self.left.locked_range.is_null() || !left_fields.is_empty();
        let show_right_axis = !self.right.locked_range.is_null() || !right_fields.is_empty();
        if !show_left_axis && !show_right_axis {
            show_left_axis = true;
        }
        let priority_axis = if (self.left.locked_range.is_null()
            && !self.right.locked_range.is_null())
            || (left_fields.is_empty() && !right_fields.is_empty())
        {
            Axis::Right
        } else {
            Axis::Left
        };
        let mut left_fields = left_fields;
        let mut right_fields = right_fields;
        left_fields.reverse();
        right_fields.reverse();

        let display_offset = display_offset(cx);
        let selection = cx.tab_selection();
        let selection_mode = selection.get_mode();
        let selected_time = selection.get_selected_time();
        let hovered_time = selection.get_hovered_time();
        let grab_zoom_range = selection.get_grab_zoom_range();
        Ok(LineGraphCommand {
            time_range,
            display_offset,
            selection_mode,
            selected_time,
            hovered_time,
            grab_zoom_range,
            left_range,
            right_range,
            show_left_axis,
            show_right_axis,
            left_units: self.left_cache.units,
            right_units: self.right_cache.units,
            left_filter: self.left.filter.0.clone(),
            right_filter: self.right.filter.0.clone(),
            priority_axis,
            left_fields,
            right_fields,
            discrete_fields,
            alerts,
        })
    }
}

/// `addNumeric` for one axis: the visible fields' commands and the data range.
fn add_numeric(
    log: &Log,
    source: &[SourceListItemState],
    settings: &AxisSettings,
    cache: &mut PreviewCache,
    dark: bool,
    time_range: [f64; 2],
    akit_timestamps: Option<&[f64]>,
) -> Result<(Vec<NumericField>, [f64; 2]), JsError> {
    let options = settings.unit_conversion.options()?;
    let target = auto_target(log, source, options, false);
    match target {
        AutoTarget::Success(unit) => {
            cache.units = static_unit(unit).map_or(AxisUnits::Unknown, |(_, c)| AxisUnits::Unit(c))
        }
        _ => {
            if let PresetValue::Preset(Preset { to: Some(to), .. }) = &options.preset
                && !to.is_empty()
            {
                cache.units =
                    static_unit(to).map_or(AxisUnits::Unknown, |(_, c)| AxisUnits::Unit(c));
            }
        }
    }

    let filter = settings.filter.kind();
    let mut data_range = [f64::INFINITY, f64::NEG_INFINITY];
    let mut fields = Vec::new();
    for item in source {
        let Some(mut data) = log.get_number(
            &item.log_key,
            if filter == LineGraphFilter::Integrate {
                f64::NEG_INFINITY
            } else {
                time_range[0]
            },
            time_range[1],
            None,
            Some(if filter == LineGraphFilter::Differentiate {
                -1
            } else {
                0
            }),
        ) else {
            continue;
        };

        if let Some(akit) = akit_timestamps
            && (!str_is(&item.item_type, "stepped") || !settings.filter.is_none())
        {
            let first = data.timestamps.first().copied().unwrap_or(f64::NAN);
            let last = data.timestamps.last().copied().unwrap_or(f64::NAN);
            let mut resampled = LogValueSet::<f64>::default();
            let mut source_index = 0;
            let mut akit_index = lower_bound(akit, first, 0);
            while akit_index < akit.len() && akit[akit_index] <= last {
                while source_index + 1 < data.timestamps.len()
                    && akit[akit_index] >= data.timestamps[source_index + 1]
                {
                    source_index += 1;
                }
                resampled.timestamps.push(akit[akit_index]);
                resampled.values.push(data.values[source_index]);
                akit_index += 1;
            }
            data = resampled;
        }

        let LogValueSet {
            mut timestamps,
            mut values,
        } = data;
        match filter {
            LineGraphFilter::Differentiate => {
                let n = values.len();
                values = (1..n)
                    .map(|i| {
                        let (prev, next) = (i - 1, (i + 1).min(n - 1));
                        (values[next] - values[prev]) / (timestamps[next] - timestamps[prev])
                    })
                    .collect();
                if !timestamps.is_empty() {
                    timestamps.remove(0);
                }
            }
            LineGraphFilter::Integrate => {
                let mut start_index = None;
                let mut integral = 0.0;
                let mut integrated = Vec::with_capacity(values.len());
                for i in 0..values.len() {
                    let prev = i.saturating_sub(1);
                    if timestamps[i] > time_range[0] && start_index.is_none() {
                        start_index = Some(i.saturating_sub(1));
                    }
                    integral +=
                        (timestamps[i] - timestamps[prev]) * (values[i] + values[prev]) * 0.5;
                    integrated.push(integral);
                }
                let start = start_index.unwrap_or(0);
                values = integrated.split_off(start.min(integrated.len()));
                timestamps = timestamps.split_off(start.min(timestamps.len()));
            }
            LineGraphFilter::None => {}
        }

        let mut has_unit = false;
        match target {
            AutoTarget::Success(target_unit) => {
                if let Some(source_unit) = log.get_unit(&item.log_key)
                    && let Some(group) = units::group_by_unit(source_unit)
                    && units::group_by_unit(target_unit) == Some(group)
                {
                    for value in &mut values {
                        *value = units::convert(*value, source_unit, target_unit)
                            .map_err(|_| JsError::new("", "Invalid unit provided"))?;
                    }
                    has_unit = true;
                }
            }
            _ => {
                let preset = match &options.preset {
                    PresetValue::Missing => None,
                    PresetValue::Null => Some(Preset {
                        type_is_null: true,
                        from: None,
                        to: None,
                        factor: 1.0,
                    }),
                    PresetValue::Preset(preset) => Some(preset.clone()),
                };
                if let Some(preset) = preset {
                    for value in &mut values {
                        *value = preset.convert(*value)?;
                    }
                    has_unit = true;
                }
            }
        }

        for value in &mut values {
            *value = clamp_value(*value, -MAX_VALUE, MAX_VALUE);
        }

        let kind = &item.item_type;
        if !timestamps.is_empty() && timestamps[0] < time_range[0] {
            if str_is(kind, "stepped") {
                timestamps[0] = time_range[0];
            } else if str_is(kind, "smooth") {
                if timestamps.len() >= 2 {
                    values[0] = scale_value_clamped(
                        time_range[0],
                        [timestamps[0], timestamps[1]],
                        [values[0], values[1]],
                    );
                    timestamps[0] = time_range[0];
                }
            } else if str_is(kind, "points") {
                timestamps.remove(0);
                if !values.is_empty() {
                    values.remove(0);
                }
            }
        }

        let n = timestamps.len();
        if n > 0 && timestamps[n - 1] > time_range[1] {
            if str_is(kind, "stepped") || str_is(kind, "points") {
                timestamps.pop();
                values.pop();
            } else if str_is(kind, "smooth") && n >= 2 {
                values[n - 1] = scale_value_clamped(
                    time_range[1],
                    [timestamps[n - 2], timestamps[n - 1]],
                    [values[n - 2], values[n - 1]],
                );
                timestamps[n - 1] = time_range[1];
            }
        } else if str_is(kind, "smooth") && n >= 1 && timestamps[n - 1] < time_range[1] {
            timestamps.push(time_range[1]);
            values.push(values.last().copied().unwrap_or(f64::NAN));
        }

        if item.visible {
            for &value in &values {
                if value < data_range[0] {
                    data_range[0] = value;
                }
                if value > data_range[1] {
                    data_range[1] = value;
                }
            }
        }

        let field = NumericField {
            timestamps,
            values,
            color: theme_color(item, dark)?,
            kind: kind.clone(),
            size: item
                .options
                .get("size")
                .cloned()
                .unwrap_or_else(|| js("normal")),
            has_unit,
        };
        if item.visible {
            fields.push(field.clone());
        }
        cache.fields.insert(item.log_key.clone(), field);
    }
    Ok((fields, data_range))
}

/// The robot mode stripes, when any range touches the visible time range.
fn robot_mode_field(log: &Log, time_range: [f64; 2], dark: bool) -> Option<DiscreteField> {
    let ranges: Vec<_> = get_robot_state_ranges(log)
        .into_iter()
        .filter(|range| {
            range.start <= time_range[1] && range.end.is_none_or(|end| end > time_range[0])
        })
        .collect();
    let last = ranges.last()?;
    let mut timestamps: Vec<f64> = Vec::new();
    let mut values: Vec<String> = Vec::new();
    for range in &ranges {
        let start = math::max2(range.start, time_range[0]);
        let value = robot_mode_name(range.mode).to_owned();
        match (timestamps.last(), values.last_mut()) {
            (Some(&previous), Some(slot)) if start <= previous => *slot = value,
            _ => {
                timestamps.push(start);
                values.push(value);
            }
        }
    }
    if let Some(end) = last.end
        && end <= time_range[1]
    {
        timestamps.push(end);
        values.push(String::new());
    }
    let colors = RobotModeColors::for_theme(dark);
    Some(DiscreteField {
        timestamps,
        values,
        color: colors.disabled.to_owned(),
        color_map: Some(colors),
        kind: js("stripes"),
        toggle_reference: false,
    })
}

fn add_discrete(
    log: &Log,
    source: &[SourceListItemState],
    time_range: [f64; 2],
    dark: bool,
    fields: &mut Vec<DiscreteField>,
) -> Result<(), JsError> {
    for item in source {
        if !item.visible || str_is(&item.item_type, "alerts") {
            continue;
        }
        let Some(data) = log.get_range(&item.log_key, time_range[0], time_range[1], None) else {
            continue;
        };
        let (mut timestamps, mut values, first_bool) = match data {
            LogValueSetAny::Raw(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
            LogValueSetAny::Boolean(s) => {
                let first = s.values.first().copied();
                let (t, v) = texts(s);
                (t, v, first)
            }
            LogValueSetAny::Number(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
            LogValueSetAny::String(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
            LogValueSetAny::BooleanArray(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
            LogValueSetAny::NumberArray(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
            LogValueSetAny::StringArray(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
            LogValueSetAny::Empty(s) => {
                let (t, v) = texts(s);
                (t, v, None)
            }
        };

        let first = timestamps.first().copied();
        let mut toggle_reference = log
            .get_timestamps(std::slice::from_ref(&item.log_key), None)
            .iter()
            .position(|&t| Some(t) == first)
            .is_some_and(|index| index % 2 == 0);
        toggle_reference = toggle_reference != log.get_striping_reference(&item.log_key);
        if let Some(value) = first_bool {
            toggle_reference = !value;
        }

        if !timestamps.is_empty() && timestamps[0] < time_range[0] {
            timestamps[0] = time_range[0];
        }
        if timestamps.last().is_some_and(|&t| t > time_range[1]) {
            timestamps.pop();
            values.pop();
        }
        if log.get_type(&item.log_key).is_none() {
            continue;
        }
        fields.push(DiscreteField {
            timestamps,
            values,
            color: theme_color(item, dark)?,
            color_map: None,
            kind: item.item_type.clone(),
            toggle_reference,
        });
    }
    Ok(())
}

fn distinct<'a>(items: impl IntoIterator<Item = &'a JsString>) -> Vec<&'a JsString> {
    let mut out: Vec<&JsString> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn open_count(alerts: &[Alert], text: &JsString) -> usize {
    alerts
        .iter()
        .filter(|a| &a.text == text && !a.range[1].is_finite())
        .count()
}

fn add_alerts(log: &Log, source: &[SourceListItemState], time_range: [f64; 2]) -> Vec<Vec<Alert>> {
    let mut rows: Vec<Vec<Alert>> = Vec::new();
    for alert_type in AlertType::ALL {
        for item in source {
            if !item.visible || !str_is(&item.item_type, "alerts") {
                continue;
            }
            let mut key = item.log_key.clone();
            key.push_str(&js(&format!("/{}s", alert_type.name())));
            let Some(set) =
                log.get_string_array(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)
            else {
                continue;
            };
            let mut all: Vec<Alert> = Vec::new();
            for (i, current) in set.values.iter().enumerate() {
                let time = set.timestamps[i];
                for text in distinct(current) {
                    let current_count = current.iter().filter(|x| *x == text).count();
                    let active_count = open_count(&all, text);
                    for _ in active_count..current_count {
                        all.push(Alert {
                            kind: alert_type,
                            text: text.clone(),
                            range: [time, f64::INFINITY],
                        });
                    }
                }
                let texts: Vec<JsString> = distinct(all.iter().map(|a| &a.text))
                    .into_iter()
                    .cloned()
                    .collect();
                for text in &texts {
                    let current_count = current.iter().filter(|x| *x == text).count();
                    let active_count = open_count(&all, text);
                    for _ in current_count..active_count {
                        if let Some(alert) = all
                            .iter_mut()
                            .find(|a| &a.text == text && !a.range[1].is_finite())
                        {
                            alert.range[1] = time;
                        }
                    }
                }
            }
            for alert in &mut all {
                if alert.range[1] == f64::INFINITY {
                    alert.range[1] = time_range[1];
                }
            }
            for alert in all {
                let mut row = 0;
                loop {
                    while rows.len() <= row {
                        rows.push(Vec::new());
                    }
                    if rows[row].iter().all(|other| {
                        other.range[1] <= alert.range[0] || other.range[0] >= alert.range[1]
                    }) {
                        break;
                    }
                    row += 1;
                }
                rows[row].push(alert);
            }
        }
    }
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .filter(|alert| {
                    !(alert.range[0] > time_range[1] && alert.range[1] > time_range[1])
                        && !(alert.range[0] < time_range[0] && alert.range[1] < time_range[0])
                })
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

fn source_state(value: &JsValue) -> Result<Vec<SourceListItemState>, JsError> {
    value
        .as_array()
        .ok_or_else(|| unmodeled("restored sources that are not an array"))?
        .iter()
        .map(|item| {
            SourceListItemState::from_js(item)
                .ok_or_else(|| unmodeled("a restored source item of another shape"))
        })
        .collect()
}

impl<Cx: SourceListCx + ?Sized> TabController<Cx> for LineGraphController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert("leftSources", self.left_list.state().to_js());
        o.insert("rightSources", self.right_list.state().to_js());
        o.insert("discreteSources", self.discrete_list.state().to_js());
        o.insert("leftLockedRange", self.left.locked_range.to_js());
        o.insert("rightLockedRange", self.right.locked_range.to_js());
        o.insert("leftUnitConversion", self.left.unit_conversion.raw.clone());
        o.insert(
            "rightUnitConversion",
            self.right.unit_conversion.raw.clone(),
        );
        o.insert("leftFilter", self.left.filter.0.clone());
        o.insert("rightFilter", self.right.filter.0.clone());
        o.insert("showRobotMode", self.show_robot_mode.clone());
        JsValue::Object(o)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        let state = match state {
            JsValue::Object(o) => o.clone(),
            JsValue::Array(_) | JsValue::Bytes(_) | JsValue::Date(_) | JsValue::ExtData(..) => {
                JsObject::new()
            }
            _ => return Ok(()),
        };
        if let Some(v) = state.get("leftLockedRange") {
            self.left.locked_range = LockedRange::from_js(v);
        }
        if let Some(v) = state.get("rightLockedRange") {
            self.right.locked_range = LockedRange::from_js(v);
        }
        if let Some(v) = state.get("leftUnitConversion") {
            self.left.unit_conversion = UnitConversion::from_js(v);
        }
        if let Some(v) = state.get("rightUnitConversion") {
            self.right.unit_conversion = UnitConversion::from_js(v);
        }
        if let Some(v) = state.get("leftFilter") {
            self.left.filter = Filter(v.clone());
        }
        if let Some(v) = state.get("rightFilter") {
            self.right.filter = Filter(v.clone());
        }
        if let Some(v) = state.get("showRobotMode") {
            self.show_robot_mode = v.clone();
        }
        self.update_axis_labels()?;
        for (key, id) in [
            ("leftSources", ListId::Left),
            ("rightSources", ListId::Right),
            ("discreteSources", ListId::Discrete),
        ] {
            if let Some(v) = state.get(key) {
                let items = source_state(v)?;
                with_env(cx, |env| {
                    self.with_list(id, |list, _| list.set_state(env, items))
                })
                .map_err(js_error)?;
            }
        }
        Ok(())
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        for id in [ListId::Left, ListId::Discrete, ListId::Right] {
            with_env(cx, |env| self.with_list(id, |list, _| list.refresh(env)))
                .map_err(js_error)?;
        }
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        let mut fields = self.left_list.active_fields();
        fields.extend(self.discrete_list.active_fields());
        fields.extend(self.right_list.active_fields());
        fields
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        false
    }

    fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        Ok(Rc::new(self.get_command(cx)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter(u32);

    impl Rng for Counter {
        fn next_f64(&mut self) -> f64 {
            self.0 += 1;
            f64::from(self.0 % 36) / 36.0
        }
    }

    #[test]
    fn lists_draw_their_uuids_left_right_discrete() {
        let controller = LineGraphController::new(&mut Counter(0));
        let mut replay = Counter(0);
        let uuids: Vec<String> = (0..3).map(|_| create_uuid(&mut replay)).collect();
        assert_eq!(controller.list(ListId::Left).uuid(), uuids[0]);
        assert_eq!(controller.list(ListId::Right).uuid(), uuids[1]);
        assert_eq!(controller.list(ListId::Discrete).uuid(), uuids[2]);
        assert_ne!(uuids[0], uuids[1]);
        assert_eq!(controller.list(ListId::Left).title(), "Left Axis");
        assert_eq!(controller.list(ListId::Right).title(), "Right Axis");
        assert_eq!(controller.list(ListId::Discrete).title(), "Discrete Fields");
    }

    #[test]
    fn axis_limits() {
        assert_eq!(limit_axis_range([2.0, 2.0]), [1.0, 3.0]);
        assert_eq!(limit_axis_range([1e25, 2e25]), [8e19, 1e20]);
        assert_eq!(limit_axis_range([-1e21, 1e21]), [-1e20, 0.0]);
        assert_eq!(
            limit_axis_range([1.0, 1.000001]),
            [1.0, 1.0 + MIN_AXIS_RANGE]
        );
        let mut locked = LockedRange::Locked([f64::NAN, f64::INFINITY]);
        assert_eq!(calc_range([0.0, 1.0], &mut locked).unwrap(), [-1.0, 1.0]);
        assert_eq!(locked, LockedRange::Locked([-1.0, 1.0]));
    }
}
