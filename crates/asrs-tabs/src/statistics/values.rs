//! The values table of `src/shared/renderers/StatisticsRenderer.ts`.

use asrs_js::color::ensure_theme_contrast;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::JsError;
use asrs_js::value::JsValue;

use super::StatisticsCommand;

enum Line {
    Section(&'static str),
    Values(&'static str, f64, &'static str),
}

use Line::{Section, Values};

const LINES: [Line; 26] = [
    Section("Summary"),
    Values("Count", 0.0, "count"),
    Values("Min", 3.0, "min"),
    Values("Max", 3.0, "max"),
    Section("Center"),
    Values("Mean", 3.0, "mean"),
    Values("Median", 3.0, "median"),
    Values("Mode", 3.0, "mode"),
    Values("Geometric Mean", 3.0, "geometricMean"),
    Values("Harmonic Mean", 3.0, "harmonicMean"),
    Values("Quadratic Mean", 3.0, "quadraticMean"),
    Section("Spread"),
    Values("Standard Deviation", 3.0, "standardDeviation"),
    Values("Median Absolute Deviation", 3.0, "medianAbsoluteDeviation"),
    Values("Interquartile Range", 3.0, "interquartileRange"),
    Values("Skewness", 3.0, "skewness"),
    Section("Percentiles"),
    Values("1st Percentile", 3.0, "percentile01"),
    Values("5th Percentile", 3.0, "percentile05"),
    Values("10th Percentile", 3.0, "percentile10"),
    Values("25th Percentile", 3.0, "percentile25"),
    Values("50th Percentile", 3.0, "percentile50"),
    Values("75th Percentile", 3.0, "percentile75"),
    Values("90th Percentile", 3.0, "percentile90"),
    Values("95th Percentile", 3.0, "percentile95"),
    Values("99th Percentile", 3.0, "percentile99"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatisticsCell {
    pub text: String,
    /// `style.color` as assigned; `None` in the placeholder column shown without fields.
    pub color: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatisticsRow {
    /// `tr.section`: one cell.
    Section {
        title: &'static str,
        col_span: usize,
    },
    /// `tr.values`: the title cell, then `cells`.
    Values {
        title: &'static str,
        cells: Vec<StatisticsCell>,
    },
}

/// The rows of `.stats-values`, rebuilt when the command's change counter moves. A theme change
/// alone keeps the old colours.
#[derive(Clone, Debug, Default)]
pub struct StatisticsTable {
    change_counter: Option<f64>,
    rows: Vec<StatisticsRow>,
}

impl StatisticsTable {
    pub fn new() -> StatisticsTable {
        StatisticsTable::default()
    }

    pub fn rows(&self) -> &[StatisticsRow] {
        &self.rows
    }

    /// The values part of `render(command)`; `dark` is the `prefers-color-scheme` the cell
    /// colours contrast with. Returns whether the command was new, which is when upstream goes
    /// on to update the histogram's data. A throw leaves the rows built before it.
    pub fn render(&mut self, command: &StatisticsCommand, dark: bool) -> Result<bool, JsError> {
        if self.change_counter == Some(command.change_counter) {
            return Ok(false);
        }
        self.change_counter = Some(command.change_counter);
        self.rows.clear();
        for line in &LINES {
            match *line {
                Section(title) => self.rows.push(StatisticsRow::Section {
                    title,
                    col_span: (1 + command.fields.len()).max(2),
                }),
                Values(title, digits, stat) => {
                    let mut cells = Vec::with_capacity(command.fields.len().max(1));
                    let result = value_cells(command, digits, stat, dark, &mut cells);
                    self.rows.push(StatisticsRow::Values { title, cells });
                    result?;
                }
            }
        }
        Ok(true)
    }
}

fn value_cells(
    command: &StatisticsCommand,
    digits: f64,
    stat: &str,
    dark: bool,
    cells: &mut Vec<StatisticsCell>,
) -> Result<(), JsError> {
    for field in &command.fields {
        let color = match &field.color {
            JsValue::String(color) => ensure_theme_contrast(&color.to_string_lossy(), dark),
            color => {
                ops::get(color, "slice")?;
                return Err(JsError::type_error("color.slice is not a function"));
            }
        };
        cells.push(StatisticsCell {
            text: String::new(),
            color: None,
        });
        let cell = cells.last_mut().expect("just pushed");
        let value = field.stats.get(stat).cloned().unwrap_or_default();
        cell.text = if ops::value_to_number(&value)?.is_nan() {
            "-".to_owned()
        } else {
            to_fixed(&value, digits)?
        };
        cell.color = Some(color);
    }
    if command.fields.is_empty() {
        cells.push(StatisticsCell {
            text: "-".to_owned(),
            color: None,
        });
    }
    Ok(())
}

/// `value.toFixed(digits)`.
fn to_fixed(value: &JsValue, digits: f64) -> Result<String, JsError> {
    match value {
        JsValue::Number(n) => {
            number::to_fixed(*n, digits).map_err(|message| JsError::new("RangeError", message))
        }
        value => {
            ops::get(value, "toFixed")?;
            Err(JsError::type_error("value.toFixed is not a function"))
        }
    }
}
