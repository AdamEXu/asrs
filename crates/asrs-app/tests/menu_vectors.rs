#![allow(clippy::disallowed_methods)]

//! Replays `menus/*` vectors. `template-*` holds the template upstream's setupMenu builds on each
//! platform and the template after each `update-preferences` rewrite; every item is compared
//! option by option with `app_menu::template` and `app_menu::apply_preferences`. `electron-darwin`
//! holds Electron's own `Menu.buildFromTemplate` result for the darwin template, compared field by
//! field with `roles::build_from_template`.

use asrs_app::menus::app_menu;
use asrs_app::menus::model::{ItemType, MenuItem};
use asrs_app::menus::roles::{BuiltItem, build_from_template};
use asrs_js::json;
use asrs_js::string::JsString;
use asrs_model::app_identity::AppIdentity;
use asrs_model::preferences::Preferences;
use asrs_parity::cve1::Value;
use asrs_parity::vectors::{self, Record};
use asrs_services::window_manager::Platform;

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a string: {value:?}"))
        .to_string_lossy()
}

fn object(value: &Value) -> &[(asrs_parity::utf16::Utf16String, Value)] {
    match value {
        Value::Object(pairs) => pairs,
        other => panic!("not an object: {other:?}"),
    }
}

fn array(value: &Value) -> &[Value] {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value:?}"))
}

fn platform_of(name: &str) -> Platform {
    match name {
        "darwin" => Platform::Mac,
        "win32" => Platform::Windows,
        "linux" => Platform::Linux,
        other => panic!("unknown platform {other}"),
    }
}

/// Compares one upstream template item with the port's, option by option. An option the
/// template sets to `undefined` is the same to Electron as one it leaves out.
fn compare_template(want: &[Value], got: &[MenuItem], at: &str, compared: &mut usize) {
    assert_eq!(want.len(), got.len(), "{at}: item count");
    for (i, (want, got)) in want.iter().zip(got).enumerate() {
        let at = format!("{at}/{i}");
        let mut seen = MenuItem::default();
        let mut click = false;
        for (key, value) in object(want) {
            let key = key.to_string_lossy();
            if *value == Value::Undefined {
                continue;
            }
            match key.as_str() {
                "role" => seen.role = Some(text(value)),
                "label" => seen.label = Some(text(value)),
                "type" => {
                    seen.item_type = Some(ItemType::parse(&text(value)).expect("a known item type"))
                }
                "accelerator" => seen.accelerator = Some(text(value)),
                "checked" => seen.checked = Some(value.as_bool().expect("checked")),
                "enabled" => seen.enabled = Some(value.as_bool().expect("enabled")),
                "visible" => seen.visible = Some(value.as_bool().expect("visible")),
                "click" => {
                    assert!(matches!(value, Value::Function(_)), "{at}: click");
                    click = true;
                }
                "submenu" => {
                    let submenu = got
                        .submenu
                        .as_deref()
                        .unwrap_or_else(|| panic!("{at}: the port has no submenu"));
                    compare_template(array(value), submenu, &at, compared);
                    seen.submenu = got.submenu.clone();
                }
                other => panic!("{at}: unknown template option {other:?}"),
            }
        }
        assert_eq!(click, got.command.is_some(), "{at}: click handler");
        seen.command = got.command;
        assert_eq!(&seen, got, "{at}");
        *compared += 1;
    }
}

fn update_json(update: &Value) -> String {
    let pairs: Vec<String> = object(update)
        .iter()
        .map(|(k, v)| format!("{:?}:{:?}", k.to_string_lossy(), text(v)))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

fn merge(prefs: &mut Preferences, json_text: &str) {
    let value = json::parse(&JsString::from(json_text)).expect("prefs JSON");
    prefs.merge(&value).expect("mergePreferences");
}

fn replay_template(id: &str) -> usize {
    let records: Vec<Record> = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut platform = None;
    let mut prefs = Preferences::default();
    let mut template = Vec::new();
    let mut compared = 0;
    let mut steps = 0;
    for record in &records {
        match record.string("op").as_deref() {
            Some("config") => {
                platform = Some(platform_of(&record.string("platform").expect("platform")))
            }
            Some("putPrefs") => merge(&mut prefs, &record.string("text").expect("text")),
            Some("template") => {
                let platform = platform.expect("config first");
                let step = record.value("step").expect("step").as_f64().expect("step");
                if step == 0.0 {
                    template = app_menu::template(platform, &prefs, AppIdentity::UPSTREAM);
                } else {
                    merge(
                        &mut prefs,
                        &update_json(&record.value("update").expect("update")),
                    );
                    app_menu::apply_preferences(&mut template, &prefs);
                }
                let want = record.value("template").expect("template");
                compare_template(
                    array(&want),
                    &template,
                    &format!("{id} step {step}"),
                    &mut compared,
                );
                steps += 1;
            }
            other => panic!("{id} line {}: unknown op {other:?}", record.line),
        }
    }
    let expected_steps = records
        .iter()
        .filter(|r| r.string("op").as_deref() == Some("template"))
        .count();
    assert_eq!(steps, expected_steps);
    assert!(steps > 0);
    compared
}

#[test]
fn darwin_template() {
    assert!(replay_template("menus/template-darwin") > 4 * 100);
}

#[test]
fn win32_template() {
    assert!(replay_template("menus/template-win32") > 4 * 90);
}

#[test]
fn linux_template() {
    assert!(replay_template("menus/template-linux") > 4 * 90);
}

#[test]
fn darwin_template_with_saved_prefs() {
    assert!(replay_template("menus/template-darwin-prefs") > 100);
}

fn compare_built(want: &[Value], got: &[BuiltItem], at: &str, compared: &mut usize) {
    assert_eq!(want.len(), got.len(), "{at}: item count");
    for (i, (want, got)) in want.iter().zip(got).enumerate() {
        let at = format!("{at}/{i} ({})", got.label);
        let optional = |value: &Value| match value {
            Value::Undefined | Value::Null => None,
            v => Some(text(v)),
        };
        let mut fields = 0;
        for (key, value) in object(want) {
            let key = key.to_string_lossy();
            let ok = match key.as_str() {
                "label" => text(value) == got.label,
                "role" => optional(value) == got.role,
                "type" => text(value) == got.item_type.as_str(),
                "accelerator" => optional(value) == got.accelerator,
                "defaultRoleAccelerator" => optional(value) == got.default_role_accelerator,
                "checked" => value.as_bool() == Some(got.checked),
                "enabled" => value.as_bool() == Some(got.enabled),
                "visible" => value.as_bool() == Some(got.visible),
                "sublabel" | "toolTip" => text(value).is_empty(),
                "registerAccelerator" => value.as_bool() == Some(got.register_accelerator),
                "acceleratorWorksWhenHidden" => {
                    value.as_bool() == Some(got.accelerator_works_when_hidden)
                }
                "submenu" => {
                    let submenu = got
                        .submenu
                        .as_deref()
                        .unwrap_or_else(|| panic!("{at}: the port has no submenu"));
                    compare_built(array(value), submenu, &at, compared);
                    true
                }
                other => panic!("{at}: unknown field {other:?}"),
            };
            assert!(ok, "{at}: {key} is {value:?} upstream, port has {got:?}");
            fields += 1;
        }
        assert_eq!(
            fields,
            12 + usize::from(got.submenu.is_some()),
            "{at}: fields"
        );
        *compared += 1;
    }
}

#[test]
fn electron_builds_the_darwin_menu() {
    let compared = replay_electron("menus/electron-darwin", Platform::Mac);
    assert!(compared > 120, "{compared} items");
}

#[test]
fn electron_builds_the_linux_menu() {
    let compared = replay_electron("menus/electron-linux/linux-x86_64", Platform::Linux);
    assert!(compared > 100, "{compared} items");
}

#[test]
fn electron_builds_the_win32_menu() {
    let compared = replay_electron("menus/electron-win32/windows-x86_64", Platform::Windows);
    assert!(compared > 100, "{compared} items");
}

fn replay_electron(vector: &str, platform: Platform) -> usize {
    let records = vectors::load(vector).unwrap_or_else(|e| panic!("{e}"));
    let mut compared = 0;
    let mut menus = 0;
    for record in &records {
        match record.string("op").as_deref() {
            Some("config") => {}
            Some("electron") => {
                assert_eq!(
                    record.string("appName").as_deref(),
                    Some(AppIdentity::UPSTREAM.name)
                );
                let template =
                    app_menu::template(platform, &Preferences::default(), AppIdentity::UPSTREAM);
                let built = build_from_template(&template, platform, AppIdentity::UPSTREAM.name);
                let want = record.value("menu").expect("menu");
                compare_built(array(&want), &built, "electron", &mut compared);
                menus += 1;
            }
            other => panic!("line {}: unknown op {other:?}", record.line),
        }
    }
    assert_eq!(menus, 1);
    compared
}
