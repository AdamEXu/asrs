//! `src/hub/controllers/MetadataController.ts` and
//! `src/shared/renderers/MetadataRenderer.ts`: the command is a precomputed map refreshed
//! whenever fields change.

use std::rc::Rc;

use asrs_js::collation::locale_compare;
use asrs_js::json;
use asrs_js::ops;
use asrs_js::sort::{array_sort, default_compare};
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_key_utils::{MERGE_PREFIX, extract_merge_prefix, remove_merge_prefix};
use asrs_log::log::robot_state::METADATA_KEYS;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};

use crate::cx::TabCx;
use crate::dom_tree::inner_text;

pub struct MetadataController {
    command: JsObject,
}

impl MetadataController {
    /// `new MetadataController()`: calls `refresh` in its constructor.
    pub fn new(cx: &mut impl TabCx) -> Result<Self, JsError> {
        let mut this = Self {
            command: JsObject::new(),
        };
        this.refresh(cx)?;
        Ok(this)
    }
}

impl<Cx: TabCx + ?Sized> TabController<Cx> for MetadataController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        let log = cx.tab_log();
        let mut command = JsObject::new();
        for key in log.get_field_keys() {
            let unmerged = remove_merge_prefix(&key);
            let merge_prefix = extract_merge_prefix(&key);
            for &metadata_key in METADATA_KEYS {
                let metadata_key = JsString::from(metadata_key);
                if !unmerged.starts_with(&metadata_key, None) {
                    continue;
                }
                let mut clean_key = merge_prefix.clone();
                clean_key.push_str(&unmerged.slice(metadata_key.len() as f64, None));
                if command.get_key(&clean_key).is_none() {
                    let mut entry = JsObject::new();
                    entry.insert("generic", JsValue::Null);
                    entry.insert("real", JsValue::Null);
                    entry.insert("replay", JsValue::Null);
                    command.insert(clean_key.clone(), JsValue::Object(entry));
                }
                let Some(log_data) = log.get_string(&key, f64::INFINITY, f64::INFINITY, None, None)
                else {
                    continue;
                };
                let value = log_data
                    .values
                    .first()
                    .map_or(JsValue::Undefined, |v| JsValue::String(v.clone()));
                let slot = if metadata_key.includes(&JsString::from("RealMetadata"), None) {
                    "real"
                } else if metadata_key.includes(&JsString::from("ReplayMetadata"), None) {
                    "replay"
                } else {
                    "generic"
                };
                if let Some(JsValue::Object(entry)) = command.get_units_mut(clean_key.units()) {
                    entry.insert(slot, value);
                }
            }
        }
        self.command = command;
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    /// `METADATA_KEYS`.
    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        METADATA_KEYS.iter().map(|k| JsString::from(*k)).collect()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        false
    }

    fn command(&mut self, _cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        Ok(Rc::new(JsValue::Object(self.command.clone())))
    }
}

/// A value column, in the order upstream shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataColumn {
    Generic,
    Real,
    Replay,
}

impl MetadataColumn {
    pub const ALL: [MetadataColumn; 3] = [
        MetadataColumn::Generic,
        MetadataColumn::Real,
        MetadataColumn::Replay,
    ];

    /// The column's `th` text.
    pub fn title(self) -> &'static str {
        match self {
            MetadataColumn::Generic => "Value",
            MetadataColumn::Real => "Real",
            MetadataColumn::Replay => "Replay",
        }
    }

    fn key(self) -> &'static str {
        match self {
            MetadataColumn::Generic => "generic",
            MetadataColumn::Real => "real",
            MetadataColumn::Replay => "replay",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataCell {
    /// Line breaks are `\n`.
    pub text: JsString,
    pub no_data: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataRow {
    /// Line breaks are `\n`.
    pub key: JsString,
    /// One cell per column.
    pub cells: Vec<MetadataCell>,
}

/// What `MetadataRenderer` leaves in its root element. The header row is an empty cell, then
/// one per column. Exactly one of the table and the no-data alert is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetadataTable {
    pub columns: Vec<MetadataColumn>,
    pub rows: Vec<MetadataRow>,
    pub show_table: bool,
}

#[derive(Default)]
pub struct MetadataRenderer {
    last_data_string: String,
    table: MetadataTable,
    builds: u64,
}

impl MetadataRenderer {
    /// `new MetadataRenderer(element)`.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn table(&self) -> &MetadataTable {
        &self.table
    }

    /// How many times the rows' elements were built.
    pub fn builds(&self) -> u64 {
        self.builds
    }

    /// `render(command)`.
    pub fn render_command(&mut self, command: &JsObject) -> Result<(), JsError> {
        let data_string = json::stringify(&JsValue::Object(command.clone()))
            .map_err(|e| JsError {
                name: e.name,
                message: e.message.to_string_lossy(),
            })?
            .unwrap_or_default();
        if data_string == self.last_data_string {
            return Ok(());
        }
        self.last_data_string = data_string;
        self.builds += 1;

        self.table.columns.clear();
        self.table.rows.clear();
        let mut columns = Vec::new();
        for column in MetadataColumn::ALL {
            for value in command.values() {
                if !matches!(ops::get(value, column.key())?, JsValue::Null) {
                    columns.push(column);
                    break;
                }
            }
        }

        let mut keys: Vec<JsString> = command.keys().cloned().collect();
        array_sort(&mut keys, default_compare);
        array_sort(&mut keys, |a, b| locale_compare(&sort_key(a), &sort_key(b)));
        let mut rows = Vec::with_capacity(keys.len());
        for key in keys {
            let values = command.get_key(&key).cloned().unwrap_or_default();
            let mut cells = Vec::with_capacity(columns.len());
            for column in &columns {
                cells.push(match ops::get(&values, column.key())? {
                    JsValue::Null => MetadataCell {
                        text: JsString::from("NA"),
                        no_data: true,
                    },
                    value => MetadataCell {
                        text: inner_text(&ops::to_string(&value)),
                        no_data: false,
                    },
                });
            }
            rows.push(MetadataRow {
                key: inner_text(&key.substring(1.0, None)),
                cells,
            });
        }
        self.table = MetadataTable {
            columns,
            show_table: !rows.is_empty(),
            rows,
        };
        Ok(())
    }
}

/// A merged log's key without its prefix, for ordering. Without a second slash, `indexOf` is
/// -1 and upstream's `slice(-1)` keeps the last character.
fn sort_key(key: &JsString) -> JsString {
    let mut prefix = JsString::from("/");
    prefix.push_str(&JsString::from(MERGE_PREFIX));
    if !key.starts_with(&prefix, None) {
        return key.clone();
    }
    let slash = key.index_of(&JsString::from("/"), Some(prefix.len() as f64));
    key.slice(slash as f64, None)
}

impl<Cx: TabCx + ?Sized> TabRenderer<Cx> for MetadataRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, _cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        let command = command.to_js();
        let JsValue::Object(command) = &command else {
            return Err(JsError::type_error(
                "Cannot convert undefined or null to object",
            ));
        };
        self.render_command(command)
    }
}
