#![allow(clippy::disallowed_methods)]

//! Replays `menus/context-*` vectors: each case is a hub or satellite message that makes
//! `main.ts` pop up a context menu. The menu's items are compared option by option with
//! `context::for_hub_message` / `for_satellite_message`, the popup position with the port's,
//! and each recorded click with the effects of the port's item, run against the same fakes:
//! messages per window, clipboard writes, windows opened (prompt windows answered with the
//! recorded reply) and `recent-units.json`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::BTreeMap;

use asrs_app::menus::bridge::APP_NAME;
use asrs_app::menus::context::{self, ContextMenu, Effect, MenuEnv, Prompt};
use asrs_app::menus::model::{ItemType, MenuItem};
use asrs_app::menus::roles::build_appended;
use asrs_js::json;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, NamedMessage, SatelliteToMain};
use asrs_parity::cve1::Value;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_services::constants::recent_units_filename;
use asrs_services::fs::{Fs, MemFs};
use asrs_services::recent_units;
use asrs_services::window_manager::{Platform, WindowKind, WindowManager};
use asrs_services::windows::Rect;
use asrs_ui::platform::SWATCH_SIZE;

const USER_DATA: &str = "/fake/userData";

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a string: {value:?}"))
        .to_string_lossy()
}

fn object(value: &Value) -> &[(Utf16String, Value)] {
    match value {
        Value::Object(pairs) => pairs,
        other => panic!("not an object: {other:?}"),
    }
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    object(value)
        .iter()
        .find(|(k, _)| k.to_string_lossy() == key)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no {key} in {value:?}"))
}

fn array(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value:?}"))
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Number(bits) => f64::from_bits(*bits),
        other => panic!("not a number: {other:?}"),
    }
}

fn js(value: &Value) -> JsValue {
    js_json::to_js(value).unwrap_or_else(|e| panic!("{e}: {value:?}"))
}

fn cve(value: &JsValue) -> Value {
    js_json::from_js(value)
}

fn rect(value: &Value) -> Rect {
    Rect {
        x: number(field(value, "x")),
        y: number(field(value, "y")),
        width: number(field(value, "width")),
        height: number(field(value, "height")),
    }
}

/// `nativeImage.createFromBitmap` as the generator records it: a 15 x 15 BGRA bitmap of one
/// colour, fully opaque.
fn swatch(value: &Value) -> [u8; 3] {
    let bitmap = field(value, "bitmap");
    let size = f64::from(SWATCH_SIZE);
    assert_eq!(number(field(bitmap, "width")), size);
    assert_eq!(number(field(bitmap, "height")), size);
    assert_eq!(number(field(bitmap, "length")), size * size * 4.0);
    assert_eq!(field(bitmap, "uniform"), &Value::Bool(true));
    let bgra: Vec<u8> = array(field(bitmap, "bgra"))
        .iter()
        .map(|v| number(v) as u8)
        .collect();
    assert_eq!(bgra[3], 255);
    [bgra[2], bgra[1], bgra[0]]
}

/// One upstream item's constructor options against the port's, option by option. An option set
/// to `undefined` is the same to Electron as one left out.
fn compare_items(want: &[Value], got: &[MenuItem], at: &str, compared: &mut usize) {
    assert_eq!(want.len(), got.len(), "{at}: item count");
    for (i, (want, got)) in want.iter().zip(got).enumerate() {
        let at = format!("{at}/{i}");
        let mut seen = MenuItem::default();
        let mut click = false;
        for (key, value) in object(want) {
            if *value == Value::Undefined {
                continue;
            }
            match key.to_string_lossy().as_str() {
                "label" => seen.label = Some(text(value)),
                "type" => {
                    seen.item_type = Some(ItemType::parse(&text(value)).expect("a known item type"))
                }
                "accelerator" => seen.accelerator = Some(text(value)),
                "checked" => seen.checked = Some(value.as_bool().expect("checked")),
                "enabled" => seen.enabled = Some(value.as_bool().expect("enabled")),
                "icon" => seen.icon = Some(swatch(value)),
                "click" => {
                    assert!(matches!(value, Value::Function(_)), "{at}: click");
                    click = true;
                }
                "submenu" => {
                    let submenu = got
                        .submenu
                        .as_deref()
                        .unwrap_or_else(|| panic!("{at}: the port has no submenu"));
                    compare_items(array(value), submenu, &at, compared);
                    seen.submenu = got.submenu.clone();
                }
                other => panic!("{at}: unknown item option {other:?}"),
            }
        }
        assert_eq!(click, got.command.is_some(), "{at}: click handler");
        seen.command = got.command;
        assert_eq!(&seen, got, "{at}");
        *compared += 1;
    }
}

fn click_paths(items: &[MenuItem], prefix: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    for (index, item) in items.iter().enumerate() {
        prefix.push(index);
        if item.command.is_some() {
            out.push(prefix.clone());
        }
        if let Some(submenu) = &item.submenu {
            click_paths(submenu, prefix, out);
        }
        prefix.pop();
    }
}

fn page(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::EditRange => "editRange.html",
        WindowKind::UnitConversion => "unitConversion.html",
        WindowKind::RenameTab => "renameTab.html",
        WindowKind::EditFov => "editFov.html",
        other => panic!("{other:?} is not a prompt"),
    }
}

#[derive(Debug, Default, PartialEq)]
struct Observed {
    sent: Vec<(String, Value)>,
    clipboard: Vec<Value>,
    windows: Vec<Opened>,
    writes: Vec<String>,
    recent_units: Option<String>,
}

#[derive(Debug, PartialEq)]
struct Opened {
    file: String,
    bounds: Rect,
    response: Option<Value>,
    destroyed: bool,
}

struct Fakes<'a> {
    asker: &'a str,
    asker_bounds: Rect,
    other_windows: Vec<&'a str>,
    fs: MemFs,
    observed: Observed,
    broadcasts: usize,
}

impl Fakes<'_> {
    fn message(&mut self, to: &str, name: &str, data: &JsValue) {
        let message =
            asrs_js::ops::object([("name", JsValue::string(name)), ("data", data.clone())]);
        self.observed.sent.push((to.to_owned(), cve(&message)));
    }

    fn run(&mut self, effects: &[Effect], response: Option<&Value>) {
        for effect in effects {
            match effect {
                Effect::Send { name, data } => {
                    let to = self.asker.to_owned();
                    self.message(&to, name, data);
                }
                Effect::WriteClipboard(text) => self.observed.clipboard.push(cve(text)),
                Effect::UpdateRecentUnits(preset) => {
                    recent_units::update(&self.fs, &recent_units_filename(USER_DATA), preset)
                        .expect("updateRecents");
                }
                Effect::OpenSourceListHelp(config) => {
                    let mut manager = WindowManager::for_platform(Platform::Mac);
                    let (parent, _) = manager.create_hub(Some(self.asker_bounds));
                    manager.set_bounds(parent, self.asker_bounds);
                    let (_, options) = manager.open_source_list_help(parent);
                    self.message("window:sourceListHelp.html", "set-config", config);
                    self.broadcasts += 1;
                    self.observed.windows.push(Opened {
                        file: "sourceListHelp.html".to_owned(),
                        bounds: Rect {
                            x: options.x.unwrap_or(0.0),
                            y: options.y.unwrap_or(0.0),
                            width: options.width,
                            height: options.height,
                        },
                        response: None,
                        destroyed: false,
                    });
                }
                Effect::Prompt(prompt) => self.prompt(prompt, response),
            }
        }
    }

    fn prompt(&mut self, prompt: &Prompt, response: Option<&Value>) {
        let mut manager = WindowManager::for_platform(Platform::Mac);
        let (parent, _) = manager.create_hub(Some(self.asker_bounds));
        manager.set_bounds(parent, self.asker_bounds);
        let kind = prompt.window();
        let (_, options) = manager.create_modal(kind, parent);
        let file = page(kind);
        self.observed
            .sent
            .push((format!("window:{file}"), cve(&prompt.initial())));
        if let Some(response) = response {
            let effects = prompt.reply(js(response)).expect("prompt reply");
            self.run(&effects, None);
        }
        self.observed.windows.push(Opened {
            file: file.to_owned(),
            bounds: Rect {
                x: options.x.unwrap_or(0.0),
                y: options.y.unwrap_or(0.0),
                width: options.width,
                height: options.height,
            },
            response: response.cloned(),
            destroyed: response.is_some(),
        });
    }
}

#[derive(Default)]
struct Tally {
    cases: usize,
    items: usize,
    clicks: usize,
    responses: usize,
    by_name: BTreeMap<String, usize>,
}

fn replay(id: &str, expect_clicks: bool) -> Tally {
    let records: Vec<Record> = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut dark = None;
    let mut hub_bounds = None;
    let mut satellite_bounds = None;
    let mut tally = Tally::default();
    let mut recorded = BTreeMap::<String, usize>::new();

    let mut window = String::new();
    let mut recents: Option<Value> = None;
    let mut menu: Option<ContextMenu> = None;
    let mut pending: Vec<Vec<usize>> = Vec::new();
    let mut last: Option<(Vec<usize>, bool)> = None;

    let finish = |pending: &Vec<Vec<usize>>, clicked: bool| {
        if expect_clicks {
            assert!(pending.is_empty(), "{id}: items never clicked: {pending:?}");
        } else {
            assert!(!clicked, "{id}: clicks recorded");
        }
    };

    for record in &records {
        let op = record.string("op").unwrap_or_default();
        *recorded.entry(op.clone()).or_default() += 1;
        let at = format!("{id} line {}", record.line);
        match op.as_str() {
            "config" => {
                assert_eq!(record.string("platform").as_deref(), Some("darwin"));
                dark = Some(match record.string("theme").as_deref() {
                    Some("dark") => true,
                    Some("light") => false,
                    other => panic!("{at}: theme {other:?}"),
                });
                hub_bounds = Some(rect(&record.value("hubBounds").expect("hubBounds")));
                satellite_bounds = Some(rect(
                    &record.value("satelliteBounds").expect("satelliteBounds"),
                ));
            }
            "case" => {
                if menu.is_some() {
                    finish(&pending, last.is_some());
                }
                window = record.string("window").expect("window");
                let name = record.string("name").expect("name");
                let data = js(&record.value("data").expect("data"));
                let recent = record.value("recentUnits").expect("recentUnits");
                recents = (recent != Value::Undefined).then_some(recent);
                let env = MenuEnv {
                    dark: dark.expect("config first"),
                    recent_units: recents.as_ref().map(js),
                };
                let built = match window.as_str() {
                    "hub" => context::for_hub_message(
                        &NamedMessage::new(HubToMain::from_name(&name).expect("hub message"), data),
                        &env,
                    ),
                    "satellite" => context::for_satellite_message(&NamedMessage::new(
                        SatelliteToMain::from_name(&name).expect("satellite message"),
                        data,
                    )),
                    other => panic!("{at}: window {other}"),
                };
                let built = built
                    .unwrap_or_else(|e| panic!("{at}: {e:?}"))
                    .unwrap_or_else(|| panic!("{at}: the port pops up no menu for {name}"));
                build_appended(&built.items, Platform::Mac, APP_NAME);
                pending.clear();
                click_paths(&built.items, &mut Vec::new(), &mut pending);
                pending.reverse();
                last = None;
                menu = Some(built);
                *tally.by_name.entry(name).or_default() += 1;
                tally.cases += 1;
            }
            "popup" => {
                let menu = menu.as_ref().expect("case first");
                assert_eq!(record.value("ownWindow").ok(), Some(Value::Bool(true)));
                let options = record.value("options").expect("options");
                let position = match object(&options) {
                    [] => None,
                    [(x, xv), (y, yv)]
                        if x.to_string_lossy() == "x" && y.to_string_lossy() == "y" =>
                    {
                        Some((number(xv), number(yv)))
                    }
                    other => panic!("{at}: popup options {other:?}"),
                };
                assert_eq!(
                    position.map(|(x, y)| (x.to_bits(), y.to_bits())),
                    menu.position.map(|(x, y)| (x.to_bits(), y.to_bits())),
                    "{at}: popup position"
                );
                compare_items(
                    array(&record.value("items").expect("items")),
                    &menu.items,
                    &at,
                    &mut tally.items,
                );
            }
            "click" => {
                let menu = menu.as_ref().expect("case first");
                let path: Vec<usize> = record
                    .field("path")
                    .and_then(|p| p.as_array())
                    .expect("path")
                    .iter()
                    .map(|v| v.as_f64().expect("index") as usize)
                    .collect();
                let windows = record.value("windows").expect("windows");
                let response = array(&windows).iter().find_map(|w| {
                    object(w)
                        .iter()
                        .find(|(k, _)| k.to_string_lossy() == "response")
                        .map(|(_, v)| v.clone())
                });
                match (&last, &response) {
                    (Some((previous, true)), Some(_)) if *previous == path => {
                        tally.responses += 1;
                    }
                    (_, None) => {
                        assert_eq!(pending.pop().as_ref(), Some(&path), "{at}: click order");
                    }
                    _ => panic!("{at}: a prompt reply for an item that opens no prompt"),
                }
                let item = MenuItem::at(&menu.items, &path).expect("clicked item");
                let effects = menu
                    .effects(item.command.expect("clickable"))
                    .expect("effects")
                    .to_vec();
                let is_prompt = effects.iter().any(|e| matches!(e, Effect::Prompt(_)));
                last = Some((path.clone(), is_prompt));

                let (asker, asker_bounds, other_windows) = match window.as_str() {
                    "hub" => ("hub", hub_bounds.unwrap(), vec!["hub", "satellite"]),
                    _ => (
                        "satellite",
                        satellite_bounds.unwrap(),
                        vec!["hub", "satellite"],
                    ),
                };
                let mut fakes = Fakes {
                    asker,
                    asker_bounds,
                    other_windows,
                    fs: MemFs::new(),
                    observed: Observed::default(),
                    broadcasts: 0,
                };
                let recents_file = recent_units_filename(USER_DATA);
                if let Some(recent) = &recents {
                    let text = json::stringify(&js(recent))
                        .expect("stringify")
                        .expect("json");
                    fakes.fs.put(&recents_file, text.as_bytes());
                }
                fakes.run(&effects, response.as_ref());
                fakes.observed.writes = fakes.fs.take_writes();
                fakes.observed.recent_units = fakes
                    .fs
                    .exists(&recents_file)
                    .then(|| String::from_utf8(fakes.fs.read(&recents_file).unwrap()).unwrap());

                let mut want = Observed {
                    clipboard: array(&record.value("clipboard").unwrap()).to_vec(),
                    writes: array(&record.value("writes").unwrap())
                        .iter()
                        .map(text)
                        .collect(),
                    recent_units: record.string("recentUnits"),
                    ..Observed::default()
                };
                for window in array(&windows) {
                    let response = object(window)
                        .iter()
                        .find(|(k, _)| k.to_string_lossy() == "response")
                        .map(|(_, v)| v.clone());
                    want.windows.push(Opened {
                        file: text(field(window, "file")),
                        bounds: rect(field(window, "bounds")),
                        response,
                        destroyed: field(window, "destroyed").as_bool().expect("destroyed"),
                    });
                }
                let mut broadcasts = 0;
                for sent in array(&record.value("sent").unwrap()) {
                    let to = text(field(sent, "to"));
                    let message = field(sent, "message").clone();
                    let is_broadcast = matches!(&message, Value::Object(_))
                        && object(&message).iter().any(|(k, v)| {
                            k.to_string_lossy() == "name" && text_eq(v, "set-preferences")
                        });
                    if is_broadcast {
                        assert!(
                            fakes.other_windows.contains(&to.as_str()),
                            "{at}: set-preferences to {to}"
                        );
                        broadcasts += 1;
                        continue;
                    }
                    want.sent.push((to, message));
                }
                assert_eq!(
                    broadcasts,
                    fakes.broadcasts * fakes.other_windows.len(),
                    "{at}: sendAllPreferences after the help window loads"
                );
                assert_eq!(fakes.observed, want, "{at}: click {path:?}");
                tally.clicks += 1;
            }
            other => panic!("{at}: unknown op {other:?}"),
        }
    }
    if menu.is_some() {
        finish(&pending, last.is_some());
    }
    assert_eq!(recorded.get("case").copied().unwrap_or(0), tally.cases);
    assert_eq!(recorded.get("popup").copied().unwrap_or(0), tally.cases);
    assert_eq!(
        recorded.get("click").copied().unwrap_or(0),
        tally.clicks,
        "{id}: clicks compared"
    );
    assert_eq!(recorded.get("config").copied(), Some(1));
    tally
}

fn text_eq(value: &Value, s: &str) -> bool {
    value.as_str().is_some_and(|v| v.to_string_lossy() == s)
}

#[test]
fn light_menus_and_clicks() {
    let tally = replay("menus/context-light", true);
    for name in [
        "ask-open-sidebar-context-menu",
        "ask-open-tuning-context-menu",
        "ask-playback-options",
        "ask-new-tab",
        "source-list-type-prompt",
        "source-list-clear-prompt",
        "ask-edit-axis",
        "ask-rename-tab",
        "ask-3d-camera",
        "ask-2d-camera",
    ] {
        assert!(
            tally.by_name.get(name).copied().unwrap_or(0) > 0,
            "no {name} case"
        );
    }
    assert!(tally.cases > 100, "{} cases", tally.cases);
    assert!(tally.items > 1900, "{} items", tally.items);
    assert!(tally.clicks > 1500, "{} clicks", tally.clicks);
    assert!(tally.responses > 100, "{} prompt replies", tally.responses);
}

#[test]
fn dark_swatches() {
    let tally = replay("menus/context-dark", false);
    assert_eq!(tally.clicks, 0);
    assert!(tally.cases > 50, "{} cases", tally.cases);
}
