//! `src/hub/controllers/MechanismController.ts`.

use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::mechanism_state::{
    MechanismLine, MechanismState, get_mechanism_state, merge_mechanism_states,
};
use asrs_model::source_list::configs;
use asrs_model::source_list::{ListInput, ListOutput, SourceList};
use asrs_model::tab_controller::{Command, TabController};

use crate::cx::TabCx;
use crate::list_cx::{self, SourceListCx, js_error, with_env};

pub struct MechanismController {
    source_list: SourceList,
}

impl MechanismController {
    /// The source list draws one `createUUID()` after the controller's own.
    pub fn new<Cx: TabCx + ?Sized>(cx: &mut Cx) -> MechanismController {
        let uuid = asrs_js::time::create_uuid(cx.tab_rng());
        MechanismController {
            source_list: SourceList::new(configs::mechanism(), uuid, false),
        }
    }

    pub fn source_list(&self) -> &SourceList {
        &self.source_list
    }

    pub fn source_list_mut(&mut self) -> &mut SourceList {
        &mut self.source_list
    }
}

fn line_to_js(line: &MechanismLine) -> JsValue {
    let coord = |x: &Option<f64>| x.map_or(JsValue::Null, |x| x.to_js());
    let mut o = JsObject::new();
    o.insert(
        "start",
        line.start.iter().map(coord).collect::<Vec<_>>().to_js(),
    );
    o.insert(
        "end",
        line.end.iter().map(coord).collect::<Vec<_>>().to_js(),
    );
    o.insert(
        "color",
        line.color.as_ref().map_or(0.0_f64.to_js(), |c| c.to_js()),
    );
    o.insert("weight", coord(&line.weight));
    JsValue::Object(o)
}

fn state_to_js(state: &MechanismState) -> JsValue {
    let mut o = JsObject::new();
    o.insert("backgroundColor", state.background_color.to_js());
    o.insert("dimensions", state.dimensions.to_js());
    o.insert(
        "lines",
        state
            .lines
            .iter()
            .map(line_to_js)
            .collect::<Vec<_>>()
            .to_js(),
    );
    JsValue::Object(o)
}

impl<Cx: SourceListCx + ?Sized> TabController<Cx> for MechanismController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        self.source_list.state().to_js()
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        list_cx::set_source_state(&mut self.source_list, cx, state)
    }

    fn refresh(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        with_env(cx, |env| self.source_list.refresh(env)).map_err(js_error)
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        self.source_list.active_fields()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        true
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
        let time = cx.tab_selection().get_render_time();
        let log = cx.tab_log();
        let time = time.unwrap_or_else(|| log.get_timestamp_range()[1]);

        let mut states = Vec::new();
        for item in self.source_list.drawn_state(&log).map_err(js_error)? {
            if let Some(state) = get_mechanism_state(&log, &item.log_key, time) {
                states.push(state);
            }
        }
        states.reverse();
        let command = merge_mechanism_states(&states).map_or(JsValue::Null, |s| state_to_js(&s));
        Ok(Rc::new(command))
    }
}
