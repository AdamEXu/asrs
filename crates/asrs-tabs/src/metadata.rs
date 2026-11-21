//! `src/hub/controllers/MetadataController.ts` and
//! `src/shared/renderers/MetadataRenderer.ts`: the command is a precomputed map refreshed
//! whenever fields change.

use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_key_utils::{extract_merge_prefix, remove_merge_prefix};
use asrs_log::log::robot_state::METADATA_KEYS;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};

use crate::cx::TabCx;

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

pub struct MetadataRenderer;

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

    fn render(&mut self, _cx: &mut Cx, _command: &dyn TabCommand) -> Result<(), JsError> {
        Ok(())
    }
}
