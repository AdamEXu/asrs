//! `src/hub/controllers/JoysticksController.ts`.

use std::rc::Rc;

use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::joystick_state::{
    JOYSTICK_KEYS, JoystickState, get_joystick_state,
};
use asrs_log::util::check_array_type;
use asrs_model::tab_controller::{Command, TabController};

use crate::cx::TabCx;
use crate::select::Select;

/// Six `<select>`s in `table.joystick-selector`.
const SELECT_COUNT: usize = 6;

pub struct JoysticksController {
    selects: Vec<Select>,
}

impl JoysticksController {
    pub fn new<Cx: TabCx + ?Sized>(cx: &mut Cx) -> JoysticksController {
        let mut controller = JoysticksController {
            selects: (0..SELECT_COUNT).map(|_| Select::new()).collect(),
        };
        controller.reset_layout_options(cx);
        controller
    }

    /// `resetLayoutOptions`: `["None", "Generic Joystick", ...asset names]`, keeping
    /// each select's value when the option still exists.
    fn reset_layout_options<Cx: TabCx + ?Sized>(&mut self, cx: &mut Cx) {
        let mut options = vec![JsString::from("None"), JsString::from("Generic Joystick")];
        if let Some(assets) = cx.tab_assets() {
            options.extend(assets.joysticks.iter().map(|j| j.name.clone()));
        }
        for select in &mut self.selects {
            select.reset_options(&options);
        }
    }

    /// `SELECTS[index]`.
    pub fn select(&self, index: usize) -> Option<&Select> {
        self.selects.get(index)
    }

    pub fn select_mut(&mut self, index: usize) -> Option<&mut Select> {
        self.selects.get_mut(index)
    }
}

fn state_to_js(state: &JoystickState) -> JsValue {
    let mut o = JsObject::new();
    o.insert("mapping", JsString::from(state.mapping.name()).to_js());
    o.insert(
        "buttons",
        state
            .buttons
            .iter()
            .map(|b| {
                let mut o = JsObject::new();
                o.insert("id", (b.id as f64).to_js());
                o.insert("state", b.state.to_js());
                JsValue::Object(o)
            })
            .collect::<Vec<_>>()
            .to_js(),
    );
    o.insert(
        "axes",
        state
            .axes
            .iter()
            .map(|a| {
                let mut o = JsObject::new();
                o.insert("id", (a.id as f64).to_js());
                o.insert("value", a.value.to_js());
                JsValue::Object(o)
            })
            .collect::<Vec<_>>()
            .to_js(),
    );
    o.insert(
        "povs",
        state
            .povs
            .iter()
            .map(|p| {
                let mut o = JsObject::new();
                o.insert("id", (p.id as f64).to_js());
                o.insert("angle", p.angle.to_js());
                JsValue::Object(o)
            })
            .collect::<Vec<_>>()
            .to_js(),
    );
    o.insert(
        "touchpads",
        state
            .touchpads
            .iter()
            .map(|pad| {
                pad.iter()
                    .map(|f| {
                        let mut o = JsObject::new();
                        o.insert("x", f.x.to_js());
                        o.insert("y", f.y.to_js());
                        o.insert("down", f.down.to_js());
                        JsValue::Object(o)
                    })
                    .collect::<Vec<_>>()
                    .to_js()
            })
            .collect::<Vec<_>>()
            .to_js(),
    );
    JsValue::Object(o)
}

impl<Cx: TabCx + ?Sized> TabController<Cx> for JoysticksController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        self.selects
            .iter()
            .map(|s| s.value().to_js())
            .collect::<Vec<_>>()
            .to_js()
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if !check_array_type(state, "string") {
            return Ok(());
        }
        let JsValue::Array(values) = state else {
            return Ok(());
        };
        for (index, value) in values.iter().enumerate() {
            if let (Some(select), Some(JsValue::String(v))) =
                (self.selects.get_mut(index), Some(value))
            {
                select.set_value(v);
                if select.value().as_string().unwrap_or_default().is_empty() {
                    select.set_selected_index(0);
                }
            }
        }
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, cx: &mut Cx) -> Result<(), JsError> {
        self.reset_layout_options(cx);
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        let mut fields = Vec::new();
        for (index, select) in self.selects.iter().enumerate() {
            if select.value() != JsString::from("None") {
                for key in JOYSTICK_KEYS {
                    fields.push(JsString::from(format!(
                        "{key}{}",
                        number::to_string(index as f64)
                    )));
                }
            }
        }
        fields
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        true
    }

    fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        let time = cx.tab_selection().get_render_time();
        let log = cx.tab_log();
        let mut command = Vec::new();
        for (index, select) in self.selects.iter().enumerate() {
            let layout = select.value();
            if layout == JsString::from("None") {
                continue;
            }
            let state = match time {
                Some(time) => state_to_js(&get_joystick_state(&log, index as f64, time)?),
                None => state_to_js(&JoystickState::blank()),
            };
            let mut o = JsObject::new();
            o.insert("layout", layout.to_js());
            o.insert("state", state);
            command.push(JsValue::Object(o));
        }
        Ok(Rc::new(command.to_js()))
    }
}
