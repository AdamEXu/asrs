//! Parity scenarios (`parity/scenarios/*.json`, the format `oracle/src/app/scenario.ts` reads):
//! the port runs the same file headless and writes the capture layout `cargo xtask pixel`
//! compares with the Electron driver's.

#[cfg(feature = "parity")]
mod capture;
mod patches;

#[cfg(feature = "parity")]
pub use capture::{run, run_from_args};
pub use patches::{COLUMNS, PATCH, patch_colours};

use asrs_js::value::JsValue;
use asrs_services::window_manager::WindowManager;

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

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
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
    pub actions: Vec<Action>,
}

/// The content size of a hub window opened with no saved state, which is the Electron
/// driver's default window too.
pub fn default_window() -> (f64, f64) {
    let (_, options) = WindowManager::new().create_hub(None);
    (options.width, options.height)
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

impl Scenario {
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let root = asrs_js::json::parse_str(text).map_err(|e| format!("{name}: {e:?}"))?;
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
        for action in root
            .get("actions")
            .and_then(JsValue::as_array)
            .ok_or_else(|| format!("{name}: no actions array"))?
        {
            let Some(capture) = action.get("capture").and_then(JsValue::as_str) else {
                let kind = action
                    .as_object()
                    .and_then(|o| o.keys().next().map(|k| k.to_string_lossy()))
                    .unwrap_or_default();
                return Err(format!(
                    "{name}: the port does not run {kind:?} actions yet (H10b)"
                ));
            };
            actions.push(Action::Capture {
                name: capture.to_string_lossy(),
                probes: strings(action.get("probes"), "probes")?,
                masks: strings(action.get("masks"), "masks")?,
            });
        }
        Ok(Self {
            name: name.to_owned(),
            page,
            width,
            height,
            scales,
            themes,
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
        let Action::Capture { probes, .. } = &s.actions[0];
        assert_eq!(probes[0], ".title-bar");
    }

    #[test]
    fn unsupported_actions_are_refused() {
        let e = Scenario::parse("line-graph", &scenario_file("line-graph")).unwrap_err();
        assert!(e.contains("\"open\""), "{e}");
    }
}
