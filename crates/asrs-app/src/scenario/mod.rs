//! Parity scenarios (`parity/scenarios/*.json`, the format `oracle/src/app/scenario.ts` reads):
//! the port runs the same file headless and writes the capture layout `cargo xtask pixel`
//! compares with the Electron driver's. Actions go through the hub's model APIs and the
//! window's input dispatch; `eval` and `dialog` exist only on the Electron side.

#[cfg(feature = "parity")]
mod capture;
mod patches;
#[cfg(feature = "parity")]
mod run;

pub use patches::{COLUMNS, PATCH, patch_colours};
#[cfg(feature = "parity")]
pub use run::{run, run_from_args};

use asrs_js::value::JsValue;
use asrs_model::messages::MainToHub;
use asrs_services::window_manager::{WindowKind, WindowManager};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    App,
    ColourPatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub fn name(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Move,
    Down,
    Up,
    Click,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// One `sendInputEvent` wheel event: `delta*` are Chromium's `WebMouseWheelEvent` deltas in
/// px, `wheel_ticks*` its notch counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wheel {
    pub x: f64,
    pub y: f64,
    pub delta_x: f64,
    pub delta_y: f64,
    pub wheel_ticks_x: f64,
    pub wheel_ticks_y: f64,
    pub precise: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A message from main, as `messagePort` delivers it.
    Main {
        name: MainToHub,
        data: JsValue,
    },
    /// `open-files` with the resolved paths.
    Open {
        files: Vec<String>,
        merge: bool,
    },
    /// `window.selection[method](...args)`.
    Selection {
        method: String,
        args: Vec<JsValue>,
    },
    Wait {
        ms: f64,
    },
    Idle,
    Mouse {
        kind: MouseKind,
        x: f64,
        y: f64,
        button: Button,
        click_count: usize,
    },
    Wheel(Wheel),
    Key {
        key: String,
        modifiers: Vec<String>,
    },
    /// Clicks the item at this label path of the context menu that popped up.
    Menu {
        path: Vec<String>,
    },
    /// Sends input and captures to the prompt window of this page, or back to the hub once the
    /// prompt has closed.
    Prompt {
        page: Option<WindowKind>,
    },
    Capture {
        name: String,
        probes: Vec<String>,
        masks: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scenario {
    pub name: String,
    pub page: Page,
    pub width: f64,
    pub height: f64,
    pub scales: Vec<u32>,
    pub themes: Vec<Theme>,
    pub gl: bool,
    /// Merged over the default preferences, as main merges `prefs.json`.
    pub prefs: JsValue,
    pub actions: Vec<Action>,
}

/// The content size of a hub window opened with no saved state, which is the Electron
/// driver's default window too.
pub fn default_window() -> (f64, f64) {
    let (_, options) = WindowManager::new().create_hub(None);
    (options.width, options.height)
}

const TOP_KEYS: [&str; 9] = [
    "page", "window", "scales", "themes", "gl", "prefs", "userData", "actions", "about",
];
const ACTION_KEYS: [&str; 13] = [
    "main",
    "open",
    "eval",
    "selection",
    "wait",
    "idle",
    "mouse",
    "wheel",
    "key",
    "dialog",
    "menu",
    "prompt",
    "capture",
];

fn keys(value: &JsValue) -> Vec<String> {
    value
        .as_object()
        .map(|o| o.keys().map(|k| k.to_string_lossy()).collect())
        .unwrap_or_default()
}

fn strings(value: Option<&JsValue>, what: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or_else(|| format!("{what} is not an array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| format!("{what} holds a non-string"))
        })
        .collect()
}

fn number(value: &JsValue, key: &str, default: Option<f64>) -> Result<f64, String> {
    match (value.get(key), default) {
        (None, Some(d)) => Ok(d),
        (v, _) => v
            .and_then(JsValue::as_f64)
            .ok_or_else(|| format!("{key} is not a number")),
    }
}

fn parse_action(action: &JsValue) -> Result<Action, String> {
    let kinds: Vec<String> = keys(action)
        .into_iter()
        .filter(|k| ACTION_KEYS.contains(&k.as_str()))
        .collect();
    let [kind] = &kinds[..] else {
        return Err(format!("needs exactly one of {}", ACTION_KEYS.join(", ")));
    };
    let value = action.get(kind).expect("the key is present");
    let text = || value.as_str().map(|s| s.to_string_lossy());
    Ok(match kind.as_str() {
        "main" => {
            let name = text().ok_or("main is not a message name")?;
            Action::Main {
                name: MainToHub::from_name(&name)
                    .ok_or_else(|| format!("{name:?} is not a message main sends a hub"))?,
                data: action.get("data").cloned().unwrap_or_default(),
            }
        }
        "open" => Action::Open {
            files: strings(Some(value), "open")?,
            merge: action
                .get("merge")
                .and_then(JsValue::as_bool)
                .unwrap_or(false),
        },
        "selection" => {
            let list = value.as_array().ok_or("selection is not an array")?;
            let (method, args) = list.split_first().ok_or("selection names no method")?;
            Action::Selection {
                method: method
                    .as_str()
                    .ok_or("selection's method is not a string")?
                    .to_string_lossy(),
                args: args.to_vec(),
            }
        }
        "wait" => Action::Wait {
            ms: value.as_f64().ok_or("wait is not a number")?,
        },
        "idle" => Action::Idle,
        "mouse" => Action::Mouse {
            kind: match text().as_deref() {
                Some("move") => MouseKind::Move,
                Some("down") => MouseKind::Down,
                Some("up") => MouseKind::Up,
                Some("click") => MouseKind::Click,
                other => return Err(format!("unknown mouse action {other:?}")),
            },
            x: number(action, "x", None)?,
            y: number(action, "y", None)?,
            button: match action.get("button").and_then(JsValue::as_str) {
                None => Button::Left,
                Some(b) => match b.to_string_lossy().as_str() {
                    "left" => Button::Left,
                    "right" => Button::Right,
                    "middle" => Button::Middle,
                    other => return Err(format!("unknown button {other:?}")),
                },
            },
            click_count: number(action, "clickCount", Some(1.0))? as usize,
        },
        "wheel" => Action::Wheel(Wheel {
            x: number(value, "x", None)?,
            y: number(value, "y", None)?,
            delta_x: number(value, "deltaX", Some(0.0))?,
            delta_y: number(value, "deltaY", Some(0.0))?,
            wheel_ticks_x: number(value, "wheelTicksX", Some(0.0))?,
            wheel_ticks_y: number(value, "wheelTicksY", Some(0.0))?,
            precise: value
                .get("precise")
                .and_then(JsValue::as_bool)
                .unwrap_or(false),
        }),
        "key" => Action::Key {
            key: text().ok_or("key is not a string")?,
            modifiers: strings(action.get("modifiers"), "modifiers")?,
        },
        "menu" => Action::Menu {
            path: strings(Some(value), "menu")?,
        },
        "prompt" => Action::Prompt {
            page: match text().as_deref() {
                Some("editRange") => Some(WindowKind::EditRange),
                Some("editFov") => Some(WindowKind::EditFov),
                Some("renameTab") => Some(WindowKind::RenameTab),
                Some("unitConversion") => Some(WindowKind::UnitConversion),
                Some("export") => Some(WindowKind::Export),
                None if value.is_null() => None,
                other => return Err(format!("unknown prompt page {other:?}")),
            },
        },
        "capture" => {
            let name = text().ok_or("capture is not a name")?;
            Action::Capture {
                name,
                probes: strings(action.get("probes"), "probes")?,
                masks: strings(action.get("masks"), "masks")?,
            }
        }
        "eval" => {
            return Err(
                "eval runs page script, which only the Electron driver can; \
                        use main, open, selection or input actions"
                    .into(),
            );
        }
        "dialog" => {
            return Err(
                "dialog answers a main-process dialog, and the port has no main yet".into(),
            );
        }
        _ => unreachable!("filtered to ACTION_KEYS"),
    })
}

impl Scenario {
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let root = asrs_js::json::parse_str(text).map_err(|e| format!("{name}: {e:?}"))?;
        if root.as_object().is_none() {
            return Err(format!("{name}: not a JSON object"));
        }
        if let Some(key) = keys(&root).iter().find(|k| !TOP_KEYS.contains(&k.as_str())) {
            return Err(format!("{name}: unknown key {key:?}"));
        }
        if root.get("userData").is_some_and(|u| !keys(u).is_empty()) {
            return Err(format!(
                "{name}: userData seeds files main reads, and the port has no main yet"
            ));
        }
        let page = match root.get("page").and_then(JsValue::as_str) {
            None => Page::App,
            Some(p) if p.to_string_lossy() == "app" => Page::App,
            Some(p) if p.to_string_lossy() == "colour-patch" => Page::ColourPatch,
            Some(p) => return Err(format!("{name}: unknown page {:?}", p.to_string_lossy())),
        };
        let (mut width, mut height) = default_window();
        if let Some(window) = root.get("window") {
            let dim = |key: &str| {
                window
                    .get(key)
                    .and_then(JsValue::as_f64)
                    .ok_or_else(|| format!("{name}: window.{key} is not a number"))
            };
            (width, height) = (dim("width")?, dim("height")?);
        }
        let scales = match root.get("scales") {
            None => vec![1, 2],
            Some(v) => v
                .as_array()
                .ok_or_else(|| format!("{name}: scales is not an array"))?
                .iter()
                .map(|s| match s.as_f64() {
                    Some(1.0) => Ok(1),
                    Some(2.0) => Ok(2),
                    other => Err(format!("{name}: scale {other:?} is not 1 or 2")),
                })
                .collect::<Result<_, _>>()?,
        };
        let themes = match root.get("themes") {
            None => vec![Theme::Light, Theme::Dark],
            v => strings(v, "themes")?
                .iter()
                .map(|t| match t.as_str() {
                    "light" => Ok(Theme::Light),
                    "dark" => Ok(Theme::Dark),
                    other => Err(format!("{name}: unknown theme {other:?}")),
                })
                .collect::<Result<_, _>>()?,
        };
        let mut actions = Vec::new();
        for (i, action) in root
            .get("actions")
            .and_then(JsValue::as_array)
            .ok_or_else(|| format!("{name}: no actions array"))?
            .iter()
            .enumerate()
        {
            actions.push(parse_action(action).map_err(|e| format!("{name}: action {i}: {e}"))?);
        }
        if !actions.iter().any(|a| matches!(a, Action::Capture { .. })) {
            return Err(format!("{name}: no capture action"));
        }
        Ok(Self {
            name: name.to_owned(),
            page,
            width,
            height,
            scales,
            themes,
            gl: root.get("gl") == Some(&JsValue::Bool(true)),
            prefs: root
                .get("prefs")
                .cloned()
                .unwrap_or_else(|| JsValue::Object(Default::default())),
            actions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario_file(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../parity/scenarios")
            .join(format!("{name}.json"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    #[test]
    fn colour_patch_scenario() {
        let s = Scenario::parse("colour-patch", &scenario_file("colour-patch")).unwrap();
        assert_eq!(s.page, Page::ColourPatch);
        assert_eq!((s.width, s.height), (512.0, 256.0));
        assert_eq!(s.scales, [1, 2]);
        assert_eq!(s.themes, [Theme::Light]);
        assert_eq!(
            s.actions,
            [Action::Capture {
                name: "patches".into(),
                probes: vec![],
                masks: vec![]
            }]
        );
    }

    #[test]
    fn hub_scenario_defaults_to_the_hub_window() {
        let s = Scenario::parse("hub-empty", &scenario_file("hub-empty")).unwrap();
        assert_eq!(s.page, Page::App);
        assert_eq!((s.width, s.height), default_window());
        assert_eq!(s.themes, [Theme::Light, Theme::Dark]);
        let Action::Capture { probes, .. } = &s.actions[0] else {
            panic!("{:?}", s.actions[0]);
        };
        assert_eq!(probes[0], ".title-bar");
    }

    #[test]
    fn every_action_kind_parses() {
        let s = Scenario::parse(
            "all",
            r#"{"prefs": {"theme": "dark"}, "actions": [
                {"main": "restore-state", "data": {"sidebar": {}}},
                {"open": ["app:drive"], "merge": true},
                {"selection": ["setSelectedTime", 2.5]},
                {"wait": 300},
                {"idle": true},
                {"mouse": "click", "x": 3, "y": 4, "button": "right", "clickCount": 2},
                {"wheel": {"x": 1, "y": 2, "deltaY": -40, "wheelTicksY": -1}},
                {"key": "a", "modifiers": ["shift"]},
                {"menu": ["Manual Units", "Edit Conversion..."]},
                {"prompt": "unitConversion"},
                {"prompt": null},
                {"capture": "c", "masks": [".side-bar"]}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            s.actions[0],
            Action::Main {
                name: MainToHub::RestoreState,
                data: asrs_js::json::parse_str(r#"{"sidebar": {}}"#).unwrap()
            }
        );
        assert_eq!(
            s.actions[1],
            Action::Open {
                files: vec!["app:drive".into()],
                merge: true
            }
        );
        assert_eq!(
            s.actions[2],
            Action::Selection {
                method: "setSelectedTime".into(),
                args: vec![JsValue::Number(2.5)]
            }
        );
        assert_eq!(s.actions[3], Action::Wait { ms: 300.0 });
        assert_eq!(s.actions[4], Action::Idle);
        assert_eq!(
            s.actions[5],
            Action::Mouse {
                kind: MouseKind::Click,
                x: 3.0,
                y: 4.0,
                button: Button::Right,
                click_count: 2
            }
        );
        assert_eq!(
            s.actions[6],
            Action::Wheel(Wheel {
                x: 1.0,
                y: 2.0,
                delta_x: 0.0,
                delta_y: -40.0,
                wheel_ticks_x: 0.0,
                wheel_ticks_y: -1.0,
                precise: false
            })
        );
        assert_eq!(
            s.actions[7],
            Action::Key {
                key: "a".into(),
                modifiers: vec!["shift".into()]
            }
        );
        assert_eq!(
            s.actions[8],
            Action::Menu {
                path: vec!["Manual Units".into(), "Edit Conversion...".into()]
            }
        );
        assert_eq!(
            s.actions[9],
            Action::Prompt {
                page: Some(WindowKind::UnitConversion)
            }
        );
        assert_eq!(s.actions[10], Action::Prompt { page: None });
        assert_eq!(s.prefs.get("theme"), Some(&JsValue::string("dark")));
    }

    #[test]
    fn electron_only_actions_are_refused() {
        let e =
            Scenario::parse("e", r#"{"actions": [{"idle": true}, {"eval": "1"}]}"#).unwrap_err();
        assert!(e.contains("action 1: eval"), "{e}");
        let e =
            Scenario::parse("d", r#"{"actions": [{"dialog": 0}, {"capture": "c"}]}"#).unwrap_err();
        assert!(e.contains("dialog"), "{e}");
        let e = Scenario::parse(
            "u",
            r#"{"userData": {"state.json": {}}, "actions": [{"capture": "c"}]}"#,
        )
        .unwrap_err();
        assert!(e.contains("userData"), "{e}");
        let e = Scenario::parse("m", r#"{"actions": [{"main": "nope"}, {"capture": "c"}]}"#)
            .unwrap_err();
        assert!(e.contains("nope"), "{e}");
    }
}
