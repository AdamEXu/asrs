#![allow(clippy::disallowed_methods)]

//! Replays `satellite/page`: upstream's real `satellite.html` in Blink, sent main's messages, keys
//! and window sizes. After every step the page's title, the messages it posted to main, the state
//! it last saved and its uncaught error are compared.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::Cell;
use std::rc::Rc;

use asrs_hub::hub::HubView;
use asrs_hub::satellite::SatelliteModel;
use asrs_hub::sidebar::{NoLayout, SidebarGeometry};
use asrs_hub::tabs::TabsError;
use asrs_js::ops;
use asrs_js::time::{Clock, SystemClock, V8MathRandom};
use asrs_js::value::JsValue;
use asrs_log::realm::JsRealm;
use asrs_model::messages::{MainToSatellite, NamedMessage, SatelliteToMain};
use asrs_model::selection::SelectionMode;
use asrs_model::source_list::{Rect, SourceListDom, Undisplayed};
use asrs_model::tab_controller::Command;
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::vectors;
use asrs_tabs::field_2d::Field2dCommand;
use asrs_tabs::table::TableCommand;

struct Window {
    size: Rc<Cell<(f64, f64)>>,
    sidebar: NoLayout,
    lists: Undisplayed,
}

impl HubView for Window {
    fn inner_height(&self) -> f64 {
        self.size.get().1
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        0.0
    }

    fn table_box(&self) -> Option<(Rect, f64)> {
        let (width, height) = self.size.get();
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        Some((rect, 1.0))
    }

    fn dark(&self) -> bool {
        false
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn source_list_dom(&mut self) -> &mut dyn SourceListDom {
        &mut self.lists
    }
}

fn js(value: &Value) -> JsValue {
    js_json::to_js(value).unwrap_or_else(|e| panic!("{e}"))
}

fn encoded(value: &JsValue) -> String {
    js_json::from_js(value).encode()
}

/// The command as the hub's own controller makes it, for the renderers that only read that.
fn command(kind: Option<TabType>, value: &JsValue) -> Command {
    let numbers = |key: &str| -> Vec<f64> {
        let list = value.get(key).and_then(JsValue::as_array);
        list.into_iter()
            .flatten()
            .map(|v| v.as_f64().expect("number"))
            .collect()
    };
    match kind {
        Some(TabType::Field2d) => Rc::new(Field2dCommand::from_js(value).expect("a 2D command")),
        Some(TabType::Table) => {
            let fields = value.get("fields").and_then(JsValue::as_array);
            assert_eq!(fields.map(<[JsValue]>::len), Some(0));
            Rc::new(TableCommand {
                timestamps: numbers("timestamps"),
                display_offset: 0.0,
                fields: Vec::new(),
                selection_mode: SelectionMode::Idle,
                selected_time: None,
                hovered_time: None,
            })
        }
        _ => Rc::new(value.clone()),
    }
}

struct Page {
    model: SatelliteModel,
    size: Rc<Cell<(f64, f64)>>,
    kind: Option<TabType>,
    editing: bool,
    uuid: Option<JsValue>,
}

impl Page {
    fn new(width: f64, height: f64) -> Page {
        let size = Rc::new(Cell::new((width, height)));
        let view = Box::new(Window {
            size: size.clone(),
            sidebar: NoLayout,
            lists: Undisplayed,
        });
        let clock: Rc<dyn Clock> = Rc::new(SystemClock);
        Page {
            model: SatelliteModel::new(
                JsRealm::new(Box::new(V8MathRandom::from_seed(1))),
                clock,
                view,
            ),
            size,
            kind: None,
            editing: false,
            uuid: None,
        }
    }

    fn step(&mut self, step: &JsValue) -> Result<(), TabsError> {
        let keys = step.as_object().expect("step").keys();
        let key = keys.map(|k| k.to_string_lossy()).next().expect("a step");
        let value = step.get(&key).expect("its value");
        let pair = || {
            let list = value.as_array().expect("pair");
            (list[0].clone(), list[1].clone())
        };
        match key.as_str() {
            "post" => {
                let (name, data) = pair();
                let name = name.as_str().expect("name").to_string_lossy();
                // `console.warn("Unknown message from main process")`, which nothing records.
                let Some(name) = MainToSatellite::from_name(&name) else {
                    assert_eq!(name, "show");
                    return Ok(());
                };
                let kind = |v: Option<&JsValue>| {
                    v.and_then(JsValue::as_f64).and_then(TabType::from_number)
                };
                match name {
                    MainToSatellite::SetType => self.kind = kind(Some(&data)),
                    MainToSatellite::RestoreState => self.kind = kind(data.get("type")),
                    MainToSatellite::Render => {
                        let field = |key| data.get(key).cloned().unwrap_or_default();
                        let command = command(self.kind, &field("command"));
                        return self.model.render(command, &field("title"));
                    }
                    _ => {}
                }
                self.model
                    .handle_main_message(&NamedMessage::new(name, data))
            }
            "key" => {
                let name = value.as_str().expect("code").to_string_lossy();
                let code = name.rsplit('+').next().expect("code");
                // The listener's own check of its target, which the window's focus decides.
                if !self.editing {
                    self.model.key_down(code);
                }
                Ok(())
            }
            "resize" => {
                let (width, height) = pair();
                let number = |v: JsValue| v.as_f64().expect("size");
                self.size.set((number(width), number(height)));
                self.model.resize()
            }
            "focus" => {
                assert_eq!(value, &JsValue::string("input.data-table-jump-input"));
                self.editing = true;
                Ok(())
            }
            "unload" => {
                self.model.before_unload();
                Ok(())
            }
            other => panic!("unknown step {other}"),
        }
    }

    /// The messages posted since the last call, with the renderer's random UUID named as the
    /// recording names it.
    fn messages(&mut self) -> Vec<JsValue> {
        let messages = self.model.take_messages();
        messages
            .into_iter()
            .map(|message| {
                let mut data = message.data;
                if message.name == SatelliteToMain::AddTableRange {
                    let uuid = data.get("uuid").cloned().expect("uuid");
                    assert_eq!(uuid.as_str().expect("a UUID").units().len(), 32);
                    assert_eq!(*self.uuid.get_or_insert_with(|| uuid.clone()), uuid);
                    let named = data.as_object_mut().expect("range");
                    named.insert("uuid", JsValue::string("uuid-0"));
                }
                ops::object([
                    ("name", JsValue::string(message.name.name())),
                    ("data", data),
                ])
            })
            .collect()
    }

    fn saved(&mut self) -> JsValue {
        self.model.save_state();
        let message = self.model.take_messages().pop().expect("save-state");
        assert_eq!(message.name, SatelliteToMain::SaveState);
        message.data
    }
}

fn thrown(result: Result<(), TabsError>) -> Vec<JsValue> {
    match result {
        Ok(()) => Vec::new(),
        Err(TabsError::Thrown(e)) => vec![ops::object([
            ("name", JsValue::string(e.name)),
            ("message", JsValue::string(&e.message)),
        ])],
        Err(TabsError::Unmodeled(what)) => panic!("unmodeled: {what}"),
    }
}

#[test]
fn the_page_matches_blink() {
    let mut page = None;
    let mut flow = String::new();
    let mut failures = Vec::new();
    let (mut flows, mut steps, mut compared) = (0, 0, 0);
    let records = vectors::load("satellite/page").unwrap();
    let recorded = records.len();
    for record in records {
        match record.string("kind").expect("kind").as_str() {
            "open" => {
                flow = record.string("flow").expect("flow");
                let size = |key| record.value(key).unwrap().as_f64().expect("size");
                page = Some(Page::new(size("width"), size("height")));
                flows += 1;
            }
            "step" => {
                steps += 1;
                let page = page.as_mut().expect("an open page");
                let step = js(&record.value("step").unwrap());
                let state = js(&record.value("state").unwrap());
                let errors = thrown(page.step(&step));
                let messages = page.messages();
                let saved = page.saved();
                let actual = [
                    ("title", JsValue::String(page.model.title().clone())),
                    ("messages", JsValue::Array(messages)),
                    ("saved", saved),
                    ("errors", JsValue::Array(errors)),
                ];
                for (key, actual) in actual {
                    compared += 1;
                    let (expected, actual) =
                        (encoded(&state.get(key).cloned().unwrap()), encoded(&actual));
                    if expected != actual {
                        failures.push(format!(
                            "{flow}, line {}, {key}:\n  expected {expected}\n  actual   {actual}",
                            record.line
                        ));
                    }
                }
            }
            other => panic!("unknown record {other}"),
        }
    }
    assert_eq!((flows + steps, compared), (recorded, steps * 4));
    assert!(flows >= 40 && steps >= 300, "{flows} flows, {steps} steps");
    assert!(
        failures.is_empty(),
        "{} of {compared}:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
