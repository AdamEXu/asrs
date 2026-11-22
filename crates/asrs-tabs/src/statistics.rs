//! `hub/controllers/StatisticsController.ts`. The source list and the histogram inputs are
//! modelled headless: each `input[type=number]` is a [`NumberInput`] with its `step` attribute
//! string beside it, and the time range is a [`Select`].

use std::rc::Rc;

use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::sort::{self, Slot};
use asrs_js::string::{JsError, JsString};
use asrs_js::time;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::robot_state::{
    AKIT_TIMESTAMP_KEYS, RobotMode, RobotStateRange, get_robot_state_ranges,
};
use asrs_model::selection::Selection;
use asrs_model::source_list::{ListInput, ListOutput, SourceList, configs};
use asrs_model::source_list_config::SourceListItemState;
use asrs_model::tab_controller::{Command, TabCommand, TabController};

use crate::cx::TabCx;
use crate::form::{NumberInput, Select, SelectOption};
use crate::list_cx::{self, SourceListCx, js_error, with_env};
use crate::simple_statistics as stats;

const UPDATE_PERIOD_MS: f64 = 100.0;
const DEFAULT_DT: f64 = 0.02;
const MAX_BINS: f64 = 1000.0;

const TIME_RANGE_OPTIONS: [(&str, &str); 7] = [
    ("visible", "Visible Range"),
    ("full", "Full Log"),
    ("enabled", "Enabled"),
    ("auto", "Auto"),
    ("teleop", "Teleop"),
    ("live-30", "Live: 30 Seconds"),
    ("live-10", "Live: 10 Seconds"),
];

/// What the controller reads besides [`SourceListCx`].
pub trait StatisticsCx: SourceListCx {
    /// `new Date().getTime()`.
    fn now(&self) -> f64;
}

/// The `.stats-*` control values a DOM capture reads off the elements.
#[derive(Clone, Debug, PartialEq)]
pub struct HistogramInputs {
    pub range_min: JsString,
    pub range_max: JsString,
    pub step_size: JsString,
    pub range_min_step: JsString,
    pub range_max_step: JsString,
    pub step_size_step: JsString,
    pub time_range: Option<JsString>,
}

pub struct StatisticsController {
    source_list: SourceList,
    time_range: Select,
    range_min: NumberInput,
    range_max: NumberInput,
    step_size: NumberInput,
    range_min_step: JsString,
    range_max_step: JsString,
    step_size_step: JsString,
    command: StatisticsCommand,
    should_update: bool,
    last_source_str: String,
    last_timeline_range: [f64; 2],
    last_update_time: f64,
    live_options_disabled: bool,
}

impl StatisticsController {
    /// `new StatisticsController(root)` after the controller's own `UUID`: the `SourceList`
    /// draws its UUID from `Math.random`.
    pub fn new(cx: &mut (impl TabCx + ?Sized)) -> Self {
        let uuid = time::create_uuid(cx.tab_rng());
        let mut controller = StatisticsController {
            source_list: SourceList::new(configs::statistics(), uuid, false),
            time_range: Select::with_options(
                TIME_RANGE_OPTIONS
                    .iter()
                    .map(|(value, text)| SelectOption::new(Some(value), text))
                    .collect(),
            ),
            range_min: NumberInput::new("0"),
            range_max: NumberInput::new("10"),
            step_size: NumberInput::new("1"),
            range_min_step: JsString::new(),
            range_max_step: JsString::new(),
            step_size_step: JsString::new(),
            command: StatisticsCommand::default(),
            should_update: true,
            last_source_str: String::new(),
            last_timeline_range: [0.0, 0.0],
            last_update_time: 0.0,
            live_options_disabled: false,
        };
        controller.update_histogram_inputs();
        controller
    }

    pub fn source_list(&self) -> &SourceList {
        &self.source_list
    }

    pub fn source_list_mut(&mut self) -> &mut SourceList {
        &mut self.source_list
    }

    fn time_range_value(&self) -> JsString {
        self.time_range.value()
    }

    /// The `.stats-*` control element values, in the template's order.
    pub fn histogram_inputs(&self) -> HistogramInputs {
        HistogramInputs {
            range_min: self.range_min.value().clone(),
            range_max: self.range_max.value().clone(),
            step_size: self.step_size.value().clone(),
            range_min_step: self.range_min_step.clone(),
            range_max_step: self.range_max_step.clone(),
            step_size_step: self.step_size_step.clone(),
            time_range: (self.time_range.selected_index() >= 0).then(|| self.time_range.value()),
        }
    }

    pub fn should_update(&self) -> bool {
        self.should_update
    }

    /// `option.disabled` of the `live-only` time range options, which `getCommand` sets.
    pub fn live_options_disabled(&self) -> bool {
        self.live_options_disabled
    }

    /// One run of the source list's own `requestAnimationFrame` loop, which the hub schedules
    /// after its tab loop. Statistics types have no previews, so only the first run (and a
    /// theme change) does anything: it re-renders every item, and a throw there ends the loop.
    pub fn source_list_frame(
        &mut self,
        cx: &mut (impl StatisticsCx + ?Sized),
    ) -> Result<(), JsError> {
        let time = if self.source_list.state().is_empty() {
            None
        } else {
            cx.tab_selection().get_render_time()
        };
        with_env(cx, |env| self.source_list.frame(env, time, None)).map_err(js_error)
    }

    /// `sourceList.setState(value)`: `clear()`, then `value.forEach(addListItem)`. An item
    /// `SourceListItemState` cannot hold stays in upstream's state after `addListItem` pushes it,
    /// so the port reports it as unmodeled instead of guessing.
    pub fn set_source_state(
        &mut self,
        cx: &mut (impl StatisticsCx + ?Sized),
        value: &JsValue,
    ) -> Result<(), JsError> {
        list_cx::set_source_state(&mut self.source_list, cx, value)
    }

    /// `select.value = v`, without the `change` listener's `shouldUpdate`.
    pub fn set_time_range(&mut self, v: JsString) {
        self.time_range.set_value(&v);
    }

    /// `input.value = v` on `.range-min`, without the `change` listeners.
    pub fn set_range_min(&mut self, v: JsString) {
        self.range_min.set_value(v);
    }

    /// `input.value = v` on `.range-max`, without the `change` listeners.
    pub fn set_range_max(&mut self, v: JsString) {
        self.range_max.set_value(v);
    }

    /// `input.value = v` on `.step-size`, without the `change` listeners.
    pub fn set_step_size(&mut self, v: JsString) {
        self.step_size.set_value(v);
    }

    /// `change` on `.time-range`.
    pub fn time_range_input(&mut self, v: JsString) {
        self.time_range.set_value(&v);
        self.should_update = true;
    }

    /// `change` on `.range-min`: `shouldUpdate` first, then `updateHistogramInputs`.
    pub fn range_min_input(&mut self, v: JsString) {
        self.range_min.set_value(v);
        self.should_update = true;
        self.update_histogram_inputs();
    }

    /// `change` on `.range-max`.
    pub fn range_max_input(&mut self, v: JsString) {
        self.range_max.set_value(v);
        self.should_update = true;
        self.update_histogram_inputs();
    }

    /// `change` on `.step-size`.
    pub fn step_size_input(&mut self, v: JsString) {
        self.step_size.set_value(v);
        self.should_update = true;
        self.update_histogram_inputs();
    }

    fn update_histogram_inputs(&mut self) {
        if self.step_size.number() <= 0.0 {
            self.step_size
                .set_value(JsString::from(number::to_string(clean_float(
                    value_to_number(&self.step_size_step) * 0.9,
                ))));
        }
        let range = math::abs(self.range_max.number() - self.range_min.number());
        if range / self.step_size.number() > MAX_BINS {
            self.step_size
                .set_value(JsString::from(number::to_string(clean_float(
                    range / MAX_BINS,
                ))));
        }
        let step = math::pow(10.0, math::floor(math::log10(self.step_size.number())));
        self.step_size_step = JsString::from(number::to_string(step));
        let min_max_step = math::pow(10.0, math::ceil(math::log10(self.step_size.number())));
        self.range_min_step = JsString::from(number::to_string(min_max_step));
        self.range_max_step = JsString::from(number::to_string(min_max_step));
    }

    fn is_valid(
        timestamp: f64,
        selection: &mut dyn Selection,
        timestamp_range: [f64; 2],
        state_ranges: &[RobotStateRange],
        time_range: &JsString,
    ) -> bool {
        let live_time = selection
            .get_current_live_time()
            .unwrap_or(timestamp_range[1]);
        let timeline_range = selection.get_timeline_range();
        if eq_str(time_range, "full") {
            return true;
        }
        if eq_str(time_range, "visible") {
            return timestamp >= timeline_range[0] && timestamp <= timeline_range[1];
        }
        if eq_str(time_range, "live-30") {
            return timestamp >= live_time - 30.0;
        }
        if eq_str(time_range, "live-10") {
            return timestamp >= live_time - 10.0;
        }
        let current_range = state_ranges
            .iter()
            .rfind(|range| range.start <= timestamp)
            .map(|range| range.mode);
        if eq_str(time_range, "enabled") {
            return current_range != Some(RobotMode::Disabled);
        }
        if eq_str(time_range, "auto") {
            return current_range == Some(RobotMode::Auto);
        }
        if eq_str(time_range, "teleop") {
            return current_range == Some(RobotMode::Teleop);
        }
        false
    }

    fn recompute(&mut self, cx: &mut (impl StatisticsCx + ?Sized)) -> Result<(), JsError> {
        let min = self.range_min.number();
        let max = self.range_max.number();
        let mut step = self.step_size.number();
        if step <= 0.0 {
            step = 1.0;
        }
        let mut bins = Vec::new();
        let mut i = min;
        while i < max && (bins.len() as f64) < MAX_BINS {
            bins.push(i);
            i += step;
        }

        let (state_ranges, akit_key, akit_timestamps, timestamp_range, sources) = {
            let log = cx.tab_log();
            let log = &*log;
            let akit_key = log
                .get_field_keys()
                .iter()
                .find(|key| AKIT_TIMESTAMP_KEYS.iter().any(|k| eq_str(key, k)))
                .cloned();
            let akit_timestamps = akit_key
                .as_ref()
                .and_then(|key| log.get_number(key, f64::NEG_INFINITY, f64::INFINITY, None, None));
            (
                get_robot_state_ranges(log),
                akit_key,
                akit_timestamps,
                log.get_timestamp_range(),
                self.source_list.displayed_state(log),
            )
        };

        let time_range = self.time_range_value();
        let selection = cx.tab_selection();
        let mut sample_times = Vec::new();
        if akit_key.is_some() {
            if let Some(akit) = &akit_timestamps {
                sample_times = akit
                    .timestamps
                    .iter()
                    .copied()
                    .filter(|&t| {
                        Self::is_valid(t, selection, timestamp_range, &state_ranges, &time_range)
                    })
                    .collect();
            }
        } else {
            let mut sample_time = timestamp_range[0];
            while sample_time < timestamp_range[1] {
                if Self::is_valid(
                    sample_time,
                    selection,
                    timestamp_range,
                    &state_ranges,
                    &time_range,
                ) {
                    sample_times.push(sample_time);
                }
                sample_time += DEFAULT_DT;
            }
        }

        let config = configs::statistics();
        let mut fields = Vec::new();
        let mut index = 0usize;
        while index < sources.len() {
            let source = &sources[index];
            let type_config = config
                .types
                .iter()
                .find(|t| eq_str(&source.item_type, &t.key));
            if type_config.is_some_and(|t| t.child_of.is_some()) {
                index += 1;
                continue;
            }
            let mut children: Vec<&SourceListItemState> = Vec::new();
            while index + 1 < sources.len()
                && config
                    .types
                    .iter()
                    .find(|t| eq_str(&sources[index + 1].item_type, &t.key))
                    .is_some_and(|t| t.child_of.is_some())
            {
                index += 1;
                children.push(&sources[index]);
            }

            if eq_str(&source.item_type, "independent") {
                field_command(
                    cx,
                    &mut fields,
                    &sample_times,
                    &bins,
                    (min, step),
                    source,
                    None,
                )?;
            } else {
                for child in children {
                    field_command(
                        cx,
                        &mut fields,
                        &sample_times,
                        &bins,
                        (min, step),
                        child,
                        Some(source),
                    )?;
                }
            }
            index += 1;
        }

        self.command = StatisticsCommand {
            change_counter: self.command.change_counter + 1.0,
            bins,
            step_size: step,
            fields,
        };
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn field_command(
    cx: &mut (impl StatisticsCx + ?Sized),
    fields: &mut Vec<StatisticsField>,
    sample_times: &[f64],
    bins: &[f64],
    (min, step): (f64, f64),
    source: &SourceListItemState,
    ref_source: Option<&SourceListItemState>,
) -> Result<(), JsError> {
    let (data, ref_data) = {
        let log = cx.tab_log();
        (
            log.get_number(
                &source.log_key,
                f64::NEG_INFINITY,
                f64::INFINITY,
                None,
                None,
            ),
            ref_source.and_then(|r| {
                log.get_number(&r.log_key, f64::NEG_INFINITY, f64::INFINITY, None, None)
            }),
        )
    };

    let mut index = 0usize;
    let mut samples: Vec<JsValue> = Vec::with_capacity(sample_times.len());
    for &sample_time in sample_times {
        let data = data.as_ref().ok_or_else(|| {
            JsError::type_error("Cannot read properties of undefined (reading 'timestamps')")
        })?;
        while index + 1 < data.timestamps.len() && data.timestamps[index + 1] < sample_time {
            index += 1;
        }
        samples.push(
            data.values
                .get(index)
                .map(|v| JsValue::Number(*v))
                .unwrap_or(JsValue::Undefined),
        );
    }

    let mut index = 0usize;
    let ref_samples: Option<Vec<JsValue>> = ref_data.map(|ref_data| {
        sample_times
            .iter()
            .map(|&sample_time| {
                while index + 1 < ref_data.timestamps.len()
                    && ref_data.timestamps[index + 1] < sample_time
                {
                    index += 1;
                }
                ref_data
                    .values
                    .get(index)
                    .map(|v| JsValue::Number(*v))
                    .unwrap_or(JsValue::Undefined)
            })
            .collect()
    });

    if let Some(ref_samples) = &ref_samples {
        if eq_str(&source.item_type, "relativeError") {
            for (sample, reference) in samples.iter_mut().zip(ref_samples) {
                *sample = JsValue::Number(subtract(sample, reference)?);
            }
        } else if eq_str(&source.item_type, "absoluteError") {
            for (sample, reference) in samples.iter_mut().zip(ref_samples) {
                *sample = JsValue::Number(math::abs(subtract(sample, reference)?));
            }
        }
    }

    let mut slots: Vec<Slot<JsValue>> = samples
        .iter()
        .map(|v| match v {
            JsValue::Undefined => Slot::Undefined,
            other => Slot::Value(other.clone()),
        })
        .collect();
    sort::array_sort_slots(&mut slots, |a, b| subtract(a, b).unwrap_or(f64::NAN));
    let samples: Vec<JsValue> = slots
        .into_iter()
        .map(|slot| match slot {
            Slot::Value(v) => v,
            Slot::Undefined | Slot::Hole => JsValue::Undefined,
        })
        .collect();

    let mut histogram_counts = vec![0.0; bins.len()];
    for value in &samples {
        if !matches!(value, JsValue::Null) {
            let bin_index = math::floor((ops::value_to_number(value)? - min) / step);
            if bin_index >= 0.0 && bin_index < bins.len() as f64 {
                histogram_counts[bin_index as usize] += 1.0;
            }
        }
    }

    let non_negative: Vec<JsValue> = samples.iter().filter(|v| ge(v)).cloned().collect();
    let positive: Vec<JsValue> = samples
        .iter()
        .filter(|v| stats::gt(v, &num(0.0)))
        .cloned()
        .collect();
    let stats_obj = field_stats(&samples, &non_negative, &positive)?;

    let color = source
        .options
        .get("color")
        .map(|c| JsValue::String(c.clone()))
        .unwrap_or(JsValue::Undefined);
    fields.push(StatisticsField {
        title: source.log_key.clone(),
        color,
        histogram_counts,
        stats: stats_obj,
    });
    Ok(())
}

fn num(x: f64) -> JsValue {
    JsValue::Number(x)
}

fn eq_str(a: &JsString, b: &str) -> bool {
    a.units().iter().copied().eq(b.encode_utf16())
}

fn ge(a: &JsValue) -> bool {
    stats::ge(a, &num(0.0))
}

fn subtract(a: &JsValue, b: &JsValue) -> Result<f64, JsError> {
    Ok(ops::value_to_number(a)? - ops::value_to_number(b)?)
}

fn value_to_number(s: &JsString) -> f64 {
    number::to_number(s.units())
}

fn clean_float(x: f64) -> f64 {
    let output = math::round(x * 1e6) / 1e6;
    if output == 0.0 { 0.0 } else { output }
}

fn field_stats(
    samples: &[JsValue],
    non_negative: &[JsValue],
    positive: &[JsValue],
) -> Result<JsObject, JsError> {
    let nan = || num(f64::NAN);
    let mut out = JsObject::new();
    out.insert("count", num(samples.len() as f64));
    out.insert(
        "min",
        if samples.is_empty() {
            nan()
        } else {
            stats::min_sorted(samples)
        },
    );
    out.insert(
        "max",
        if samples.is_empty() {
            nan()
        } else {
            stats::max_sorted(samples)
        },
    );
    out.insert(
        "mean",
        if samples.is_empty() {
            nan()
        } else {
            stats::mean(samples)?
        },
    );
    out.insert(
        "median",
        if samples.is_empty() {
            nan()
        } else {
            stats::median_sorted(samples)?
        },
    );
    out.insert(
        "mode",
        if samples.is_empty() {
            nan()
        } else {
            stats::mode_sorted(samples)?
        },
    );
    out.insert(
        "geometricMean",
        if non_negative.is_empty() {
            nan()
        } else {
            stats::log_average(non_negative)?
        },
    );
    out.insert(
        "harmonicMean",
        if positive.is_empty() {
            nan()
        } else {
            stats::harmonic_mean(positive)?
        },
    );
    out.insert(
        "quadraticMean",
        if samples.is_empty() {
            nan()
        } else {
            stats::root_mean_square(samples)?
        },
    );
    out.insert(
        "standardDeviation",
        if samples.len() < 2 {
            nan()
        } else {
            stats::sample_standard_deviation(samples)?
        },
    );
    out.insert(
        "medianAbsoluteDeviation",
        if samples.is_empty() {
            nan()
        } else {
            stats::median_absolute_deviation(samples)?
        },
    );
    out.insert(
        "interquartileRange",
        if samples.is_empty() {
            nan()
        } else {
            stats::interquartile_range(samples)?
        },
    );
    out.insert(
        "skewness",
        if samples.len() < 3 {
            nan()
        } else {
            stats::sample_skewness(samples)?
        },
    );
    for (key, p) in [
        ("percentile01", 0.01),
        ("percentile05", 0.05),
        ("percentile10", 0.1),
        ("percentile25", 0.25),
        ("percentile50", 0.5),
        ("percentile75", 0.75),
        ("percentile90", 0.9),
        ("percentile95", 0.95),
        ("percentile99", 0.99),
    ] {
        out.insert(
            key,
            if samples.is_empty() {
                nan()
            } else {
                stats::quantile_sorted(samples, &num(p))?
            },
        );
    }
    Ok(out)
}

#[derive(Clone, Default)]
pub struct StatisticsCommand {
    change_counter: f64,
    bins: Vec<f64>,
    step_size: f64,
    fields: Vec<StatisticsField>,
}

#[derive(Clone)]
struct StatisticsField {
    title: JsString,
    color: JsValue,
    histogram_counts: Vec<f64>,
    stats: JsObject,
}

impl TabCommand for StatisticsCommand {
    fn to_js(&self) -> JsValue {
        let mut command = JsObject::new();
        command.insert("changeCounter", num(self.change_counter));
        command.insert(
            "bins",
            JsValue::Array(self.bins.iter().map(|b| num(*b)).collect()),
        );
        command.insert("stepSize", num(self.step_size));
        command.insert(
            "fields",
            JsValue::Array(
                self.fields
                    .iter()
                    .map(|field| {
                        let mut f = JsObject::new();
                        f.insert("title", JsValue::String(field.title.clone()));
                        f.insert("color", field.color.clone());
                        f.insert(
                            "histogramCounts",
                            JsValue::Array(
                                field.histogram_counts.iter().map(|c| num(*c)).collect(),
                            ),
                        );
                        f.insert("stats", JsValue::Object(field.stats.clone()));
                        JsValue::Object(f)
                    })
                    .collect(),
            ),
        );
        JsValue::Object(command)
    }
}

impl<Cx: StatisticsCx + ?Sized> TabController<Cx> for StatisticsController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut state = JsObject::new();
        state.insert(
            "sources",
            JsValue::Array(self.source_list.state().iter().map(ToJs::to_js).collect()),
        );
        state.insert("timeRange", JsValue::String(self.time_range_value()));
        state.insert("rangeMin", num(self.range_min.number()));
        state.insert("rangeMax", num(self.range_max.number()));
        state.insert("stepSize", num(self.step_size.number()));
        JsValue::Object(state)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if !matches!(
            state,
            JsValue::Object(_)
                | JsValue::Array(_)
                | JsValue::Bytes(_)
                | JsValue::Date(_)
                | JsValue::ExtData(..)
        ) {
            return Ok(());
        }
        if ops::has(state, "sources")? {
            let sources = ops::get(state, "sources")?;
            self.set_source_state(cx, &sources)?;
        }
        if ops::has(state, "timeRange")?
            && let JsValue::String(v) = &ops::get(state, "timeRange")?
        {
            self.time_range.set_value(v);
        }
        if ops::has(state, "rangeMin")?
            && let JsValue::Number(v) = ops::get(state, "rangeMin")?
        {
            self.range_min
                .set_value(JsString::from(number::to_string(v)));
        }
        if ops::has(state, "rangeMax")?
            && let JsValue::Number(v) = ops::get(state, "rangeMax")?
        {
            self.range_max
                .set_value(JsString::from(number::to_string(v)));
        }
        if ops::has(state, "stepSize")?
            && let JsValue::Number(v) = ops::get(state, "stepSize")?
        {
            self.step_size
                .set_value(JsString::from(number::to_string(v)));
        }
        self.update_histogram_inputs();
        Ok(())
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        with_env(cx, |env| self.source_list.refresh(env)).map_err(js_error)?;
        self.should_update = true;
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        self.source_list.active_fields()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        eq_str(&self.time_range_value(), "visible")
    }

    fn source_lists(&self) -> Vec<&SourceList> {
        vec![&self.source_list]
    }

    fn source_list_input(
        &mut self,
        cx: &mut Cx,
        input: ListInput<'_>,
        errors: &mut Vec<JsError>,
    ) -> ListOutput {
        list_cx::route_lists(&mut [&mut self.source_list], cx, input, errors)
    }

    fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        self.live_options_disabled = cx.tab_selection().get_current_live_time().is_none();
        let sources_js = JsValue::Array(self.source_list.state().iter().map(ToJs::to_js).collect());
        let sources_str = json::stringify(&sources_js)
            .map_err(|e| JsError {
                name: e.name,
                message: e.message.to_string_lossy(),
            })?
            .unwrap_or_default();
        let current_time = time::time_clip(cx.now());
        let time_range = self.time_range_value();
        let is_live_mode = cx.tab_selection().get_current_live_time().is_some()
            && starts_with(&time_range, "live");
        let visible_changed = eq_str(&time_range, "visible")
            && cx.tab_selection().get_timeline_range() != self.last_timeline_range;
        if (self.should_update
            || sources_str != self.last_source_str
            || is_live_mode
            || visible_changed)
            && current_time - self.last_update_time > UPDATE_PERIOD_MS
        {
            self.should_update = false;
            self.last_source_str = sources_str;
            self.last_timeline_range = cx.tab_selection().get_timeline_range();
            self.last_update_time = current_time;
            self.recompute(cx)?;
        }
        Ok(Rc::new(self.command.clone()))
    }
}

fn starts_with(s: &JsString, prefix: &str) -> bool {
    s.units()
        .starts_with(&prefix.encode_utf16().collect::<Vec<u16>>())
}
