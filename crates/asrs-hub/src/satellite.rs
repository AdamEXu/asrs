//! `src/satellite.ts`: a satellite window's page. It shows one renderer, drawn from the commands
//! the hub's tab with the same UUID sends through main, and answers with selection setter calls,
//! table ranges, the aspect ratio its window should have and its saved state.
//!
//! The renderer is the only tab of a [`HubModel`] without the rest of the hub, so the views that
//! draw a hub's tabs draw a satellite's too.

use std::rc::Rc;

use asrs_js::math;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::Clock;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{
    HubToMain, MainToSatellite, NamedMessage, SatelliteToMain, SelectionSetter,
};
use asrs_model::selection::{Selection, SelectionMode, TimeSupplier};
use asrs_model::tab_controller::Command;
use asrs_model::tab_type::TabType;
use asrs_tabs::field_2d::Field2dRenderer;

use crate::hub::{APP_NAME, HubModel, HubView};
use crate::tabs::TabsError;

pub const SAVE_PERIOD_MS: f64 = 250.0;
const MAX_ASPECT_RATIO: f64 = 5.0;

/// `MockSelection`: setters go to main as `call-selection-setter`. No renderer calls the getters,
/// which upstream throws from.
#[derive(Debug, Default)]
pub struct SatelliteSelection {
    messages: Vec<NamedMessage<SatelliteToMain>>,
}

impl SatelliteSelection {
    fn call(&mut self, setter: SelectionSetter, args: Vec<JsValue>) {
        self.messages.push(NamedMessage::new(
            SatelliteToMain::CallSelectionSetter,
            ops::object([
                ("name", JsValue::string(setter.name())),
                ("args", JsValue::Array(args)),
            ]),
        ));
    }

    fn not_implemented<T>(&self, value: T) -> T {
        debug_assert!(false, "a satellite's selection has no getters");
        value
    }
}

fn pair([min, max]: [f64; 2]) -> JsValue {
    JsValue::Array(vec![JsValue::Number(min), JsValue::Number(max)])
}

fn nullable(value: Option<f64>) -> JsValue {
    value.map_or(JsValue::Null, JsValue::Number)
}

impl Selection for SatelliteSelection {
    fn get_mode(&self) -> SelectionMode {
        self.not_implemented(SelectionMode::Idle)
    }

    fn get_hovered_time(&self) -> Option<f64> {
        self.not_implemented(None)
    }

    fn set_hovered_time(&mut self, value: Option<f64>) {
        self.call(SelectionSetter::SetHoveredTime, vec![nullable(value)]);
    }

    fn get_selected_time(&mut self) -> Option<f64> {
        self.not_implemented(None)
    }

    fn set_selected_time(&mut self, time: f64) {
        self.call(
            SelectionSetter::SetSelectedTime,
            vec![JsValue::Number(time)],
        );
    }

    fn go_idle(&mut self) {
        self.call(SelectionSetter::GoIdle, Vec::new());
    }

    fn play(&mut self) {
        self.call(SelectionSetter::Play, Vec::new());
    }

    fn pause(&mut self) {
        self.call(SelectionSetter::Pause, Vec::new());
    }

    fn toggle_playback(&mut self) {
        self.call(SelectionSetter::TogglePlayback, Vec::new());
    }

    fn lock(&mut self) {
        self.call(SelectionSetter::Lock, Vec::new());
    }

    fn unlock(&mut self) {
        self.call(SelectionSetter::Unlock, Vec::new());
    }

    fn toggle_lock(&mut self) {
        self.call(SelectionSetter::ToggleLock, Vec::new());
    }

    fn step_cycle(&mut self, is_forward: bool) {
        self.call(SelectionSetter::StepCycle, vec![JsValue::Bool(is_forward)]);
    }

    fn set_live_connected(&mut self, _time_supplier: TimeSupplier) {
        self.not_implemented(());
    }

    fn set_live_disconnected(&mut self) {
        self.not_implemented(());
    }

    fn get_current_live_time(&self) -> Option<f64> {
        self.not_implemented(None)
    }

    fn get_render_time(&mut self) -> Option<f64> {
        self.not_implemented(None)
    }

    fn set_playback_speed(&mut self, _speed: f64) {
        self.not_implemented(());
    }

    fn set_playback_looping(&mut self, _looping: bool) {
        self.not_implemented(());
    }

    fn set_grab_zoom_range(&mut self, range: Option<[f64; 2]>) {
        self.call(
            SelectionSetter::SetGrabZoomRange,
            vec![range.map_or(JsValue::Null, pair)],
        );
    }

    fn get_grab_zoom_range(&self) -> Option<[f64; 2]> {
        self.not_implemented(None)
    }

    /// The one setter call upstream sends without `args`.
    fn finish_grab_zoom(&mut self) {
        self.messages.push(NamedMessage::new(
            SatelliteToMain::CallSelectionSetter,
            ops::object([(
                "name",
                JsValue::string(SelectionSetter::FinishGrabZoom.name()),
            )]),
        ));
    }

    fn get_timeline_range(&mut self) -> [f64; 2] {
        self.not_implemented([0.0, 0.0])
    }

    fn get_timeline_is_max_zoom(&self) -> bool {
        self.not_implemented(false)
    }

    fn set_timeline_range(&mut self, _range: [f64; 2], _lock_max_zoom: bool) {
        self.not_implemented(());
    }

    fn apply_timeline_scroll(&mut self, dx: f64, dy: f64, width_pixels: f64) {
        self.call(
            SelectionSetter::ApplyTimelineScroll,
            [dx, dy, width_pixels].map(JsValue::Number).to_vec(),
        );
    }
}

/// What the page reads `document.title` back as: the `title` element's text with ASCII whitespace
/// stripped and collapsed. The HTML parser drops NUL characters.
fn document_title(text: &JsString) -> JsString {
    let mut out: Vec<u16> = Vec::new();
    for &unit in text.units().iter() {
        let space = matches!(unit, 0x09 | 0x0a | 0x0c | 0x0d | 0x20);
        match (space, out.last()) {
            (true, None | Some(0x20)) => {}
            (true, _) => out.push(0x20),
            (false, _) if unit == 0 => {}
            (false, _) => out.push(unit),
        }
    }
    if out.last() == Some(&0x20) {
        out.pop();
    }
    JsString::from_units(out)
}

pub struct SatelliteModel {
    hub: HubModel,
    /// `type`, as main sent it.
    type_value: JsValue,
    /// Whether `renderer` is set.
    has_renderer: bool,
    title: Option<JsString>,
    document_title: JsString,
    last_aspect_ratio: Option<f64>,
    last_command: Option<Command>,
    outbox: Vec<NamedMessage<SatelliteToMain>>,
}

impl SatelliteModel {
    /// Module evaluation of `satellite.ts`. The caller owns the [`SAVE_PERIOD_MS`] interval.
    pub fn new(realm: JsRealm, clock: Rc<dyn Clock>, view: Box<dyn HubView>) -> Self {
        SatelliteModel {
            hub: HubModel::satellite(realm, clock, view),
            type_value: JsValue::Null,
            has_renderer: false,
            title: None,
            document_title: JsString::from(APP_NAME),
            last_aspect_ratio: None,
            last_command: None,
            outbox: Vec::new(),
        }
    }

    /// The page and its one tab. Input handlers may run on it directly; [`Self::take_messages`]
    /// has what they sent main.
    pub fn hub(&self) -> &HubModel {
        &self.hub
    }

    pub fn hub_mut(&mut self) -> &mut HubModel {
        &mut self.hub
    }

    /// `document.title`.
    pub fn title(&self) -> &JsString {
        &self.document_title
    }

    fn set_title(&mut self, title: &JsString) {
        let mut text = JsString::new();
        if !self.type_value.is_null() {
            let icon = self.kind().map_or("", TabType::icon);
            text.push_str(&JsString::from(icon));
            text.push_str(&JsString::from(" "));
        }
        text.push_str(title);
        text.push_str(&JsString::from(" \u{2014} "));
        text.push_str(&JsString::from(APP_NAME));
        self.document_title = document_title(&text);
        self.title = Some(title.clone());
    }

    fn kind(&self) -> Option<TabType> {
        match self.type_value {
            JsValue::Number(n) => TabType::from_number(n),
            _ => None,
        }
    }

    fn is(&self, kind: TabType) -> bool {
        self.kind() == Some(kind)
    }

    /// `updateVisualizer()`. A type the switch does not know keeps the renderer there was, which
    /// for main's one `set-type` or `restore-state` is none.
    fn update_visualizer(&mut self) -> Result<(), TabsError> {
        if self.type_value.is_undefined() {
            let message = "Cannot read properties of undefined (reading 'toString')";
            return Err(JsError::type_error(message).into());
        }
        if let Some(kind) = self.kind() {
            let (tabs, page) = self.hub.tabs_mut();
            tabs.set_satellite_tab(page, &self.type_value, kind)?;
            self.has_renderer = true;
        }
        Ok(())
    }

    /// One message main posts on the port. An error is what the listener throws.
    pub fn handle_main_message(
        &mut self,
        message: &NamedMessage<MainToSatellite>,
    ) -> Result<(), TabsError> {
        let data = &message.data;
        match message.name {
            MainToSatellite::SetAssets => self.hub.page_mut().set_satellite_assets(data),
            MainToSatellite::SetPreferences => self.hub.page_mut().set_preferences(data)?,
            MainToSatellite::SetBattery => self.hub.page_mut().set_satellite_battery(data),
            MainToSatellite::SetType => {
                self.type_value = data.clone();
                self.update_visualizer()?;
            }
            MainToSatellite::RestoreState => {
                self.type_value = ops::get(data, "type")?;
                self.update_visualizer()?;
                if self.has_renderer {
                    let state = ops::get(data, "visualizer")?;
                    let (tabs, page) = self.hub.tabs_mut();
                    if let Some(tab) = tabs.tab_mut(0) {
                        tab.renderer.restore_state(page, &state)?;
                    }
                }
            }
            MainToSatellite::Render => {
                let command: Command = Rc::new(ops::get(data, "command")?);
                self.render(command, &ops::get(data, "title")?)?;
            }
            MainToSatellite::Set3dCamera | MainToSatellite::EditFov => {
                if self.is(TabType::Field3d) {
                    let value = data.as_f64().unwrap_or(f64::NAN);
                    let (tabs, page) = self.hub.tabs_mut();
                    if let Some(tab) = tabs.tab_mut(0) {
                        match message.name {
                            MainToSatellite::EditFov => tab.renderer.set_fov(page, value),
                            _ => tab.renderer.set_3d_camera(page, value),
                        }
                    }
                }
            }
            MainToSatellite::Set2dCamera => {
                if self.is(TabType::Field2d) {
                    let value = data.as_f64().unwrap_or(f64::NAN);
                    let (tabs, page) = self.hub.tabs_mut();
                    if let Some(tab) = tabs.tab_mut(0)
                        && let Some(renderer) = tab.renderer.downcast_mut::<Field2dRenderer>()
                    {
                        renderer.set_2d_camera(page, value)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// `render`: one frame's command and title of the hub's tab.
    pub fn render(&mut self, command: Command, title: &JsValue) -> Result<(), TabsError> {
        let unchanged = match &self.title {
            Some(old) => *title == JsValue::String(old.clone()),
            None => title.is_null(),
        };
        if !unchanged {
            ops::get(title, "replace")?;
            let JsValue::String(title) = title else {
                return Err(JsError::type_error("text.replace is not a function").into());
            };
            self.set_title(title);
        }
        self.last_command = Some(command);
        if !self.has_renderer {
            return Ok(());
        }
        let aspect_ratio = self.render_last_command()?;
        self.process_aspect_ratio(aspect_ratio);
        if self.is(TabType::Table) {
            let (tabs, page) = self.hub.tabs_mut();
            let range = tabs
                .tab_mut(0)
                .and_then(|tab| tab.renderer.timestamp_range(page));
            self.send_table_range(range);
        }
        Ok(())
    }

    fn render_last_command(&mut self) -> Result<Option<f64>, TabsError> {
        let Some(command) = self.last_command.clone() else {
            return Ok(None);
        };
        self.hub.lay_out_large_table();
        let (tabs, page) = self.hub.tabs_mut();
        let Some(tab) = tabs.tab_mut(0) else {
            return Ok(None);
        };
        tab.renderer.render(page, &*command)?;
        Ok(tab.renderer.aspect_ratio(page))
    }

    fn send_table_range(&mut self, range: Option<[f64; 2]>) {
        let (tabs, _) = self.hub.tabs_mut();
        let uuid = tabs
            .tab_mut(0)
            .and_then(|tab| tab.renderer.renderer_uuid().cloned())
            .unwrap_or_default();
        self.send(
            SatelliteToMain::AddTableRange,
            ops::object([
                ("uuid", JsValue::String(uuid)),
                ("range", range.map_or(JsValue::Null, pair)),
            ]),
        );
    }

    /// The renderer's `requestAnimationFrame` callbacks, and what they throw.
    pub fn animation_frame(&mut self) -> Vec<JsError> {
        let (tabs, page) = self.hub.tabs_mut();
        tabs.animation_frame(page)
    }

    /// The window's `resize` listener.
    pub fn resize(&mut self) -> Result<(), TabsError> {
        let null_command = match &self.last_command {
            Some(command) => command
                .downcast_ref::<JsValue>()
                .is_some_and(JsValue::is_null),
            None => true,
        };
        if !self.has_renderer || null_command {
            return Ok(());
        }
        let aspect_ratio = self.render_last_command()?;
        if aspect_ratio.is_some_and(|a| a != 0.0 && !a.is_nan()) {
            self.process_aspect_ratio(aspect_ratio);
        }
        Ok(())
    }

    fn process_aspect_ratio(&mut self, aspect_ratio: Option<f64>) {
        if aspect_ratio == self.last_aspect_ratio {
            return;
        }
        self.last_aspect_ratio = aspect_ratio;
        let clamped = aspect_ratio
            .map(|a| math::max2(math::min2(a, MAX_ASPECT_RATIO), 1.0 / MAX_ASPECT_RATIO));
        let lock = !self.is(TabType::Field2d);
        self.send(
            SatelliteToMain::SetAspectRatio,
            ops::object([
                ("aspectRatio", nullable(clamped)),
                ("lock", JsValue::Bool(lock)),
            ]),
        );
    }

    /// The `beforeunload` listener.
    pub fn before_unload(&mut self) {
        if self.is(TabType::Table) {
            self.send_table_range(None);
        }
    }

    /// The window `keydown` listener for a target that is not editable. Returns whether
    /// `preventDefault` was called.
    pub fn key_down(&mut self, code: &str) -> bool {
        let Some(selection) = self.hub.page_mut().satellite_mut() else {
            return false;
        };
        match code {
            "Space" => selection.toggle_playback(),
            "KeyL" => selection.toggle_lock(),
            "ArrowLeft" | "ArrowRight" => selection.step_cycle(code == "ArrowRight"),
            _ => return false,
        }
        true
    }

    /// The [`SAVE_PERIOD_MS`] interval's callback.
    pub fn save_state(&mut self) {
        let mut state = JsObject::new();
        state.insert("type", self.type_value.clone());
        let visualizer = if self.has_renderer {
            let hub = &self.hub;
            hub.tabs().tab(0).map_or(JsValue::Undefined, |tab| {
                tab.renderer().save_state(hub.page())
            })
        } else {
            JsValue::Undefined
        };
        state.insert("visualizer", visualizer);
        self.send(SatelliteToMain::SaveState, JsValue::Object(state));
    }

    fn send(&mut self, name: SatelliteToMain, data: JsValue) {
        self.collect();
        self.outbox.push(NamedMessage::new(name, data));
    }

    fn collect(&mut self) {
        let page = self.hub.page_mut();
        let renderer = page.take_outbox();
        let selection = page
            .satellite_mut()
            .map(|s| std::mem::take(&mut s.messages))
            .unwrap_or_default();
        self.outbox.extend(selection);
        for message in renderer {
            self.forward(message);
        }
    }

    /// A message a renderer sends with `window.sendMainMessage`, which in a satellite reaches
    /// main's satellite handler. One it does not know is logged there.
    pub fn forward(&mut self, message: NamedMessage<HubToMain>) {
        if let Some(name) = SatelliteToMain::from_name(message.name.name()) {
            self.outbox.push(NamedMessage::new(name, message.data));
        }
    }

    pub fn take_messages(&mut self) -> Vec<NamedMessage<SatelliteToMain>> {
        self.collect();
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_read_back_collapsed() {
        let title = |s: &str| document_title(&JsString::from(s)).to_string_lossy();
        assert_eq!(title(" a \t\r\n b  "), "a b");
        assert_eq!(title("\u{1f4c9} \0x"), "\u{1f4c9} x");
    }
}
