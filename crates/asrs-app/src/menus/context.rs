//! The context menus `handleHubMessage` and the satellite message handler in
//! `src/main/electron/main.ts` pop up: sidebar copy, tuning copy, playback speed, new tab,
//! source-list type and clear, line graph axis, tab rename and the 2D and 3D camera menus
//! (`newTabPopup`, `select2DCameraPopup`, `select3DCameraPopup`).
//!
//! Each builder reads the message's `data` as `main.ts` does and returns the items it appends,
//! where it pops them up, and for each clickable item the [`Effect`]s its `click` has. Clicks
//! mutate objects the closures share, but a popup takes at most one click, so each item's
//! effects are computed from the message as received.

use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::regex::sites::capitalize_words;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::colors::ensure_theme_contrast;
use asrs_log::units::UNIT_GROUPS;
use asrs_model::messages::{HubToMain, MainToHub, MainToSatellite, NamedMessage, SatelliteToMain};
use asrs_model::source_list_config::{SourceListConfig, SourceListTypeConfig};
use asrs_model::tab_type::TabType;
use asrs_services::constants::noop_unit_conversion;
use asrs_services::window_manager::WindowKind;

use super::model::{Command, ItemType, MenuItem};

/// `LineGraphFilter`.
const FILTER_NONE: f64 = 0.0;
const FILTER_DIFFERENTIATE: f64 = 1.0;
const FILTER_INTEGRATE: f64 = 2.0;

/// What one click does, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `sendMessage(window, name, data)` to the window the menu popped up for.
    Send { name: &'static str, data: JsValue },
    /// `clipboard.writeText(text)`.
    WriteClipboard(JsValue),
    /// `updateRecents(preset)`: [`asrs_services::recent_units::update`].
    UpdateRecentUnits(JsValue),
    /// `openSourceListHelp(window, config)`.
    OpenSourceListHelp(JsValue),
    /// A modal prompt window whose reply has further effects.
    Prompt(Box<Prompt>),
}

impl Effect {
    fn send(name: MainToHub, data: JsValue) -> Effect {
        Effect::Send {
            name: name.name(),
            data,
        }
    }

    /// A [`Effect::Send`] as a message for a hub window.
    pub fn to_hub(&self) -> Option<NamedMessage<MainToHub>> {
        match self {
            Effect::Send { name, data } => {
                Some(NamedMessage::new(MainToHub::from_name(name)?, data.clone()))
            }
            _ => None,
        }
    }

    /// A [`Effect::Send`] as a message for a satellite window.
    pub fn to_satellite(&self) -> Option<NamedMessage<MainToSatellite>> {
        match self {
            Effect::Send { name, data } => Some(NamedMessage::new(
                MainToSatellite::from_name(name)?,
                data.clone(),
            )),
            _ => None,
        }
    }
}

/// The line graph axis an `edit-axis` reply describes, as the menu captured it.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisState {
    legend: JsValue,
    locked_range: JsValue,
    unit_conversion: JsValue,
    filter: JsValue,
}

impl AxisState {
    fn edit_axis(&self) -> Effect {
        Effect::send(
            MainToHub::EditAxis,
            ops::object([
                ("legend", self.legend.clone()),
                ("lockedRange", self.locked_range.clone()),
                ("unitConversion", self.unit_conversion.clone()),
                ("filter", self.filter.clone()),
            ]),
        )
    }

    fn with_preset(&self, auto_target: JsValue, preset: JsValue) -> Result<AxisState, JsError> {
        let mut unit_conversion = self.unit_conversion.clone();
        ops::set_key(
            &mut unit_conversion,
            &JsString::from("autoTarget"),
            auto_target,
        )?;
        ops::set_key(&mut unit_conversion, &JsString::from("preset"), preset)?;
        Ok(AxisState {
            unit_conversion,
            ..self.clone()
        })
    }
}

/// `createEditRangeWindow`, `createUnitConversionWindow`, `createRenameTabWindow` and
/// `createEditFovWindow`: the window opens with [`Prompt::initial`] posted to it, and the value
/// it posts back goes to [`Prompt::reply`].
#[derive(Clone, Debug, PartialEq)]
pub enum Prompt {
    EditRange { axis: AxisState },
    UnitConversion { initial: JsValue, axis: AxisState },
    RenameTab { index: JsValue, name: JsValue },
    EditFov { fov: JsValue },
}

impl Prompt {
    pub fn window(&self) -> WindowKind {
        match self {
            Prompt::EditRange { .. } => WindowKind::EditRange,
            Prompt::UnitConversion { .. } => WindowKind::UnitConversion,
            Prompt::RenameTab { .. } => WindowKind::RenameTab,
            Prompt::EditFov { .. } => WindowKind::EditFov,
        }
    }

    pub fn initial(&self) -> JsValue {
        match self {
            Prompt::EditRange { axis } => axis.locked_range.clone(),
            Prompt::UnitConversion { initial, .. } => initial.clone(),
            Prompt::RenameTab { name, .. } => name.clone(),
            Prompt::EditFov { fov } => fov.clone(),
        }
    }

    pub fn reply(&self, value: JsValue) -> Result<Vec<Effect>, JsError> {
        Ok(match self {
            Prompt::EditRange { axis } => vec![
                AxisState {
                    locked_range: value,
                    ..axis.clone()
                }
                .edit_axis(),
            ],
            Prompt::UnitConversion { axis, .. } => {
                if value.is_null() {
                    return Ok(Vec::new());
                }
                vec![
                    axis.with_preset(JsValue::Null, value.clone())?.edit_axis(),
                    Effect::UpdateRecentUnits(value),
                ]
            }
            Prompt::RenameTab { index, .. } => vec![Effect::send(
                MainToHub::RenameTab,
                ops::object([("index", index.clone()), ("name", value)]),
            )],
            Prompt::EditFov { .. } => vec![Effect::send(MainToHub::EditFov, value)],
        })
    }
}

/// A menu `main.ts` builds with `new Menu()` and `append`, and pops up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextMenu {
    pub items: Vec<MenuItem>,
    /// `x` and `y` of `menu.popup`, in window content coordinates; `None` when `main.ts` passes
    /// only the window, which opens the menu at the mouse.
    pub position: Option<(f64, f64)>,
    actions: Vec<Vec<Effect>>,
}

impl ContextMenu {
    /// The effects of the item whose command is `command`.
    pub fn effects(&self, command: Command) -> Option<&[Effect]> {
        match command {
            Command::ContextItem(index) => self.actions.get(index).map(Vec::as_slice),
            _ => None,
        }
    }

    fn click(&mut self, item: MenuItem, effects: Vec<Effect>) -> MenuItem {
        self.actions.push(effects);
        item.command(Command::ContextItem(self.actions.len() - 1))
    }

    fn append(&mut self, item: MenuItem) {
        self.items.push(item);
    }

    fn append_click(&mut self, item: MenuItem, effects: Vec<Effect>) {
        let item = self.click(item, effects);
        self.items.push(item);
    }

    fn at(mut self, x: f64, y: f64) -> ContextMenu {
        self.position = Some((math::round(x), math::round(y)));
        self
    }
}

/// What a builder reads besides the message: `nativeTheme.shouldUseDarkColors` and
/// `recent-units.json` as `jsonfile.readFileSync` parses it (`None` when the file is missing).
#[derive(Clone, Debug, Default)]
pub struct MenuEnv {
    pub dark: bool,
    pub recent_units: Option<JsValue>,
}

/// The context menu `handleHubMessage` pops up for `message`, if it is one that does.
pub fn for_hub_message(
    message: &NamedMessage<HubToMain>,
    env: &MenuEnv,
) -> Result<Option<ContextMenu>, JsError> {
    let data = &message.data;
    Ok(Some(match message.name {
        HubToMain::AskOpenSidebarContextMenu => sidebar(data)?,
        HubToMain::AskOpenTuningContextMenu => tuning(data)?,
        HubToMain::AskPlaybackOptions => playback_options(data)?,
        HubToMain::AskNewTab => new_tab(data)?,
        HubToMain::SourceListTypePrompt => source_list_type(data, env.dark)?,
        HubToMain::SourceListClearPrompt => source_list_clear(data)?,
        HubToMain::AskEditAxis => edit_axis(data, env.recent_units.as_ref())?,
        HubToMain::AskRenameTab => rename_tab(data)?,
        HubToMain::Ask3dCamera => camera_3d(data)?,
        HubToMain::Ask2dCamera => camera_2d(data)?,
        _ => return Ok(None),
    }))
}

/// The context menu a satellite window's message handler pops up for `message`.
pub fn for_satellite_message(
    message: &NamedMessage<SatelliteToMain>,
) -> Result<Option<ContextMenu>, JsError> {
    Ok(Some(match message.name {
        SatelliteToMain::Ask3dCamera => camera_3d(&message.data)?,
        SatelliteToMain::Ask2dCamera => camera_2d(&message.data)?,
        _ => return Ok(None),
    }))
}

fn get(value: &JsValue, key: &str) -> Result<JsValue, JsError> {
    ops::get(value, key)
}

fn num(value: &JsValue) -> Result<f64, JsError> {
    ops::value_to_number(value)
}

fn label(value: &JsValue) -> String {
    ops::to_string(value).to_string_lossy()
}

fn eq_number(value: &JsValue, n: f64) -> bool {
    ops::strict_equals(value, &JsValue::Number(n))
}

fn not_null(value: &JsValue) -> bool {
    !value.is_null()
}

fn checkbox(text: impl Into<String>, checked: bool) -> MenuItem {
    MenuItem::new(text)
        .item_type(ItemType::Checkbox)
        .checked(checked)
}

/// `x: Math.round(rect.x + rect.width), y: Math.round(rect.y)`.
fn beside(menu: ContextMenu, rect: &JsValue) -> Result<ContextMenu, JsError> {
    let x = num(&get(rect, "x")?)? + num(&get(rect, "width")?)?;
    let y = num(&get(rect, "y")?)?;
    Ok(menu.at(x, y))
}

/// `x: Math.round(position[0]), y: Math.round(position[1])`.
fn at_position(menu: ContextMenu, position: &JsValue) -> Result<ContextMenu, JsError> {
    let x = num(&ops::index(position, 0)?)?;
    let y = num(&ops::index(position, 1)?)?;
    Ok(menu.at(x, y))
}

/// `ask-open-sidebar-context-menu`.
pub fn sidebar(data: &JsValue) -> Result<ContextMenu, JsError> {
    let mut menu = ContextMenu::default();
    for key in ["title", "fullTitle"] {
        let text = get(data, key)?;
        menu.append_click(
            MenuItem::new(format!("Copy \"{}\" to clipboard", label(&text))),
            vec![Effect::WriteClipboard(text)],
        );
    }
    at_position(menu, &get(data, "position")?)
}

/// `ask-open-tuning-context-menu`.
pub fn tuning(data: &JsValue) -> Result<ContextMenu, JsError> {
    let mut menu = ContextMenu::default();
    menu.append_click(
        MenuItem::new("Copy Tuned Values").enabled(ops::to_boolean(&get(data, "isTuningMode")?)),
        vec![Effect::WriteClipboard(get(data, "values")?)],
    );
    at_position(menu, &get(data, "position")?)
}

/// `ask-playback-options`.
pub fn playback_options(data: &JsValue) -> Result<ContextMenu, JsError> {
    let speed = get(data, "speed")?;
    let looping = get(data, "looping")?;
    let mut menu = ContextMenu::default();
    for value in [0.25, 0.5, 1.0, 1.5, 2.0, 4.0, 8.0] {
        menu.append_click(
            checkbox(
                format!("{}%", number::to_string(value * 100.0)),
                eq_number(&speed, value),
            ),
            vec![Effect::send(
                MainToHub::SetPlaybackOptions,
                ops::object([
                    ("speed", JsValue::Number(value)),
                    ("looping", looping.clone()),
                ]),
            )],
        );
    }
    menu.append(MenuItem::separator());
    menu.append_click(
        checkbox("Loop Visible Range", ops::to_boolean(&looping)),
        vec![Effect::send(
            MainToHub::SetPlaybackOptions,
            ops::object([
                ("speed", speed),
                ("looping", JsValue::Bool(!ops::to_boolean(&looping))),
            ]),
        )],
    );
    beside(menu, &get(data, "rect")?)
}

/// `ask-new-tab` (`newTabPopup`); `data` is the button's rect.
pub fn new_tab(data: &JsValue) -> Result<ContextMenu, JsError> {
    let mut menu = ContextMenu::default();
    for &tab in &TabType::ALL[1..] {
        menu.append_click(
            MenuItem::new(format!("{} {}", tab.icon(), tab.default_title()))
                .accelerator(tab.accelerator()),
            vec![Effect::send(MainToHub::NewTab, tab.to_js())],
        );
    }
    beside(menu, data)
}

/// `source-list-clear-prompt`.
pub fn source_list_clear(data: &JsValue) -> Result<ContextMenu, JsError> {
    let mut menu = ContextMenu::default();
    menu.append_click(
        MenuItem::new("Clear All"),
        vec![Effect::send(
            MainToHub::SourceListClearResponse,
            ops::object([("uuid", get(data, "uuid")?)]),
        )],
    );
    beside(menu, &get(data, "rect")?)
}

/// `ask-rename-tab`.
pub fn rename_tab(data: &JsValue) -> Result<ContextMenu, JsError> {
    let mut menu = ContextMenu::default();
    menu.append_click(
        MenuItem::new("Rename..."),
        vec![Effect::Prompt(Box::new(Prompt::RenameTab {
            index: get(data, "index")?,
            name: get(data, "name")?,
        }))],
    );
    Ok(menu)
}

/// `ask-2d-camera` (`select2DCameraPopup`).
pub fn camera_2d(data: &JsValue) -> Result<ContextMenu, JsError> {
    let selected = get(data, "selectedIndex")?;
    let mut menu = ContextMenu::default();
    for (text, mode) in [
        ("Unlocked", 0.0),
        ("Locked to Robot", 1.0),
        ("Locked to Robot && Rotation", 2.0),
    ] {
        menu.append_click(
            checkbox(text, eq_number(&selected, mode)),
            vec![Effect::send(MainToHub::Set2dCamera, JsValue::Number(mode))],
        );
    }
    at_position(menu, &get(data, "position")?)
}

/// `ask-3d-camera` (`select3DCameraPopup`).
pub fn camera_3d(data: &JsValue) -> Result<ContextMenu, JsError> {
    let position = get(data, "position")?;
    let options = get(data, "options")?;
    let selected = get(data, "selectedIndex")?;
    let fov = get(data, "fov")?;
    let is_ftc = ops::to_boolean(&get(data, "isFTC")?);
    let mut menu = ContextMenu::default();
    let camera = |menu: &mut ContextMenu, text: &str, index: f64| {
        menu.click(
            checkbox(text, eq_number(&selected, index)),
            vec![Effect::send(MainToHub::Set3dCamera, JsValue::Number(index))],
        )
    };
    let orbit_field = camera(&mut menu, "Orbit Field", -1.0);
    menu.append(orbit_field);
    let orbit_robot = camera(&mut menu, "Orbit Robot", -2.0);
    menu.append(orbit_robot);
    let stations: &[(&str, f64)] = if is_ftc {
        &[
            ("Blue Left", -4.0),
            ("Blue Right", -5.0),
            ("Red Left", -6.0),
            ("Red Right", -7.0),
        ]
    } else {
        &[
            ("Auto", -3.0),
            ("Blue 1", -4.0),
            ("Blue 2", -5.0),
            ("Blue 3", -6.0),
            ("Red 1", -7.0),
            ("Red 2", -8.0),
            ("Red 3", -9.0),
        ]
    };
    let submenu = stations
        .iter()
        .map(|&(text, index)| camera(&mut menu, text, index))
        .collect();
    menu.append(
        MenuItem::new(if is_ftc {
            "Driver View"
        } else {
            "Driver Station"
        })
        .submenu(submenu),
    );
    menu.append_click(
        MenuItem::new("Set FOV..."),
        vec![Effect::Prompt(Box::new(Prompt::EditFov { fov }))],
    );
    let length = num(&get(&options, "length")?)?;
    if length > 0.0 {
        menu.append(MenuItem::separator());
    }
    let options = options
        .as_array()
        .ok_or_else(|| JsError::type_error("options.forEach is not a function"))?;
    for (index, option) in options.iter().enumerate() {
        menu.append_click(
            checkbox(label(option), eq_number(&selected, index as f64)),
            vec![Effect::send(
                MainToHub::Set3dCamera,
                JsValue::Number(index as f64),
            )],
        );
    }
    at_position(menu, &position)
}

/// `ask-edit-axis`. `recent_units` is `recent-units.json` as parsed, `None` when it is missing.
pub fn edit_axis(data: &JsValue, recent_units: Option<&JsValue>) -> Result<ContextMenu, JsError> {
    let legend = get(data, "legend")?;
    let mut menu = ContextMenu::default();
    if ops::strict_equals(&legend, &JsValue::string("discrete")) {
        let show = ops::to_boolean(&get(data, "showRobotMode")?);
        menu.append_click(
            checkbox("Show Robot Mode", show),
            vec![Effect::send(
                MainToHub::SetRobotModeVisible,
                ops::object([("showRobotMode", JsValue::Bool(!show))]),
            )],
        );
    } else {
        let axis = AxisState {
            legend: legend.clone(),
            locked_range: get(data, "lockedRange")?,
            unit_conversion: get(data, "unitConversion")?,
            filter: get(data, "filter")?,
        };
        let group = get(data, "autoUnitGroup")?;
        let selected = get(data, "autoUnitSelected")?;
        let default = get(data, "autoUnitDefault")?;
        let locked = not_null(&axis.locked_range);
        menu.append_click(
            checkbox("Lock Axis", locked),
            vec![
                AxisState {
                    locked_range: if locked {
                        JsValue::Null
                    } else {
                        JsValue::Array(vec![JsValue::Null, JsValue::Null])
                    },
                    ..axis.clone()
                }
                .edit_axis(),
            ],
        );
        menu.append_click(
            MenuItem::new("Edit Range...").enabled(locked),
            vec![Effect::Prompt(Box::new(Prompt::EditRange {
                axis: axis.clone(),
            }))],
        );
        menu.append(MenuItem::separator());
        unit_items(&mut menu, &axis, &group, &selected, &default)?;
        let recents = match recent_units {
            Some(value) => value.clone(),
            None => JsValue::Array(Vec::new()),
        };
        let manual = manual_units(&mut menu, &axis, &recents)?;
        menu.append(manual);
        menu.append(MenuItem::separator());
        for (text, filter) in [
            ("Differentiate", FILTER_DIFFERENTIATE),
            ("Integrate", FILTER_INTEGRATE),
        ] {
            let on = eq_number(&axis.filter, filter);
            menu.append_click(
                checkbox(text, on),
                vec![
                    AxisState {
                        filter: JsValue::Number(if on { FILTER_NONE } else { filter }),
                        ..axis.clone()
                    }
                    .edit_axis(),
                ],
            );
        }
        menu.append(MenuItem::separator());
    }
    menu.append_click(
        MenuItem::new("Help"),
        vec![Effect::OpenSourceListHelp(get(data, "config")?)],
    );
    menu.append_click(
        MenuItem::new("Clear All"),
        vec![Effect::send(MainToHub::ClearAxis, legend)],
    );
    beside(menu, &get(data, "rect")?)
}

fn unit_items(
    menu: &mut ContextMenu,
    axis: &AxisState,
    group: &JsValue,
    selected: &JsValue,
    default: &JsValue,
) -> Result<(), JsError> {
    let disabled = |text: &str| MenuItem::new(text).enabled(false);
    if ops::strict_equals(group, &JsValue::string("none")) {
        menu.append(disabled("(No Unit Metadata)"));
        return Ok(());
    }
    if ops::strict_equals(group, &JsValue::string("inconsistent")) {
        menu.append(disabled("(Inconsistent Units)"));
        return Ok(());
    }
    let group_name = ops::to_string(group).to_string_lossy();
    let units = UNIT_GROUPS
        .iter()
        .find(|(name, _)| *name == group_name)
        .map(|(_, units)| *units)
        .ok_or_else(|| JsError::type_error("Cannot convert undefined or null to object"))?;
    for (unit, _) in units {
        let unit_js = JsValue::string(unit);
        let name = JsString::from(*unit);
        let mut text = name.char_at(0.0).to_upper_case();
        text.push_str(&name.slice(1.0, None));
        let mut text = text.to_string_lossy();
        if ops::strict_equals(&unit_js, default) {
            text.push_str(" [Default]");
        }
        let effects = vec![
            axis.with_preset(unit_js.clone(), JsValue::Null)?
                .edit_axis(),
        ];
        menu.append_click(
            checkbox(text, ops::strict_equals(selected, &unit_js)),
            effects,
        );
    }
    Ok(())
}

fn manual_units(
    menu: &mut ContextMenu,
    axis: &AxisState,
    recents: &JsValue,
) -> Result<MenuItem, JsError> {
    let preset = get(&axis.unit_conversion, "preset")?;
    let noop = noop_unit_conversion();
    let edit = menu.click(
        MenuItem::new("Edit Conversion..."),
        vec![Effect::Prompt(Box::new(Prompt::UnitConversion {
            initial: if preset.is_null() || preset.is_undefined() {
                noop.clone()
            } else {
                preset.clone()
            },
            axis: axis.clone(),
        }))],
    );
    let enabled = num(&get(recents, "length")?)? > 0.0;
    let presets = recents
        .as_array()
        .ok_or_else(|| JsError::type_error("recentUnits.map is not a function"))?;
    let mut recent_items = Vec::with_capacity(presets.len());
    for recent in presets {
        let effects = vec![
            axis.with_preset(JsValue::Null, recent.clone())?.edit_axis(),
            Effect::UpdateRecentUnits(recent.clone()),
        ];
        recent_items.push(menu.click(MenuItem::new(preset_label(recent)?), effects));
    }
    let recent_menu = MenuItem::new("Recent Presets")
        .item_type(ItemType::Submenu)
        .enabled(enabled)
        .submenu(recent_items);
    let disable = menu.click(
        checkbox("Disable Automatic Units", not_null(&preset)),
        vec![
            axis.with_preset(
                JsValue::Null,
                if preset.is_null() {
                    noop.clone()
                } else {
                    JsValue::Null
                },
            )?
            .edit_axis(),
        ],
    );
    let stringify = |value: &JsValue| {
        json::stringify(value).map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))
    };
    let reset_enabled = not_null(&preset) && stringify(&preset)? != stringify(&noop)?;
    let reset = menu.click(
        MenuItem::new("Reset Conversion").enabled(reset_enabled),
        vec![axis.with_preset(JsValue::Null, noop)?.edit_axis()],
    );
    Ok(MenuItem::new("Manual Units")
        .item_type(ItemType::Submenu)
        .submenu(vec![
            edit,
            recent_menu,
            MenuItem::separator(),
            disable,
            reset,
        ]))
}

/// A recent preset's label: `From → To`, `x<factor>`, both, or neither.
fn preset_label(preset: &JsValue) -> Result<String, JsError> {
    let from = get(preset, "from")?;
    let to = get(preset, "to")?;
    let capitalized = |value: &JsValue| -> Result<JsString, JsError> {
        match value {
            JsValue::String(s) => Ok(capitalize_words(s)),
            JsValue::Undefined | JsValue::Null => Ok(JsString::from("undefined")),
            _ => Err(JsError::type_error(
                "preset.from?.replace is not a function",
            )),
        }
    };
    let mut from_to = JsString::new();
    if !from.is_undefined() && !to.is_undefined() {
        from_to.push_str(&capitalized(&from)?);
        from_to.push_str(&JsString::from(" \u{2192} "));
        from_to.push_str(&capitalized(&to)?);
    }
    let factor = get(preset, "factor")?;
    let factor_text = if eq_number(&factor, 1.0) {
        JsString::new()
    } else {
        if factor.is_null() || factor.is_undefined() {
            return Err(JsError::type_error(format!(
                "Cannot read properties of {} (reading 'toString')",
                if factor.is_null() {
                    "null"
                } else {
                    "undefined"
                }
            )));
        }
        let mut text = JsString::from("x");
        text.push_str(&ops::to_string(&factor));
        text
    };
    let both = !from_to.is_empty() && !factor_text.is_empty();
    let mut out = from_to;
    if both {
        out.push_str(&JsString::from(", "));
    }
    out.push_str(&factor_text);
    Ok(out.to_string_lossy())
}

/// `source-list-type-prompt`: the type and option menu of a source list item.
pub fn source_list_type(data: &JsValue, dark: bool) -> Result<ContextMenu, JsError> {
    let uuid = get(data, "uuid")?;
    let config_js = get(data, "config")?;
    let config = SourceListConfig::from_js(&config_js)
        .ok_or_else(|| JsError::type_error("source-list-type-prompt: malformed config"))?;
    let state = get(data, "state")?;
    let rect = get(data, "rect")?;
    let state_type = get(&state, "type")?;
    let respond = |state: JsValue| {
        Effect::send(
            MainToHub::SourceListTypeResponse,
            ops::object([("uuid", uuid.clone()), ("state", state)]),
        )
    };
    let icon = |key: &str| {
        key.starts_with('#')
            .then(|| asrs_js::color::hex_rgb(&ensure_theme_contrast(key, dark)))
    };

    let current = config
        .types
        .iter()
        .find(|t| ops::strict_equals(&JsValue::string(&t.key), &state_type))
        .ok_or_else(|| {
            JsError::type_error("Cannot read properties of undefined (reading 'options')")
        })?;
    let options = get(&state, "options")?;
    let mut menu = ContextMenu::default();
    let option_item = |menu: &mut ContextMenu, option_key: &str, key: &str, display: &str| {
        let mut new_state = state.clone();
        let effects =
            set_option(&mut new_state, option_key, key).map(|()| vec![respond(new_state)]);
        effects.map(|effects| {
            menu.click(
                checkbox(
                    display,
                    ops::strict_equals(
                        &JsValue::string(key),
                        &ops::get(&options, option_key).unwrap_or_default(),
                    ),
                )
                .icon(icon(key)),
                effects,
            )
        })
    };
    if let [option] = current.options.as_slice() {
        get(&options, &option.key)?;
        for value in &option.values {
            let item = option_item(&mut menu, &option.key, &value.key, &value.display)?;
            menu.append(item);
        }
    } else {
        for option in &current.options {
            get(&options, &option.key)?;
            let mut submenu = Vec::with_capacity(option.values.len());
            for value in &option.values {
                submenu.push(option_item(
                    &mut menu,
                    &option.key,
                    &value.key,
                    &value.display,
                )?);
            }
            menu.append(MenuItem::new(option.display.clone()).submenu(submenu));
        }
    }

    let log_type = get(&state, "logType")?;
    let valid: Vec<&SourceListTypeConfig> = config
        .types
        .iter()
        .filter(|t| {
            t.source_types
                .iter()
                .any(|s| ops::same_value_zero(&JsValue::string(s), &log_type))
                && t.child_of == current.child_of
        })
        .collect();
    if valid.len() > 1 {
        if !menu.items.is_empty() {
            menu.append(MenuItem::separator());
        }
        for type_config in valid {
            let is_current = ops::strict_equals(&state_type, &JsValue::string(&type_config.key));
            let initial = if is_current {
                None
            } else {
                type_config.options.iter().find(|o| {
                    type_config.initial_selection_option.as_deref() == Some(o.key.as_str())
                })
            };
            let item_type = if is_current {
                ItemType::Checkbox
            } else if initial.is_some() {
                ItemType::Submenu
            } else {
                ItemType::Normal
            };
            let item = MenuItem::new(type_config.display.clone())
                .item_type(item_type)
                .checked(is_current);
            match initial {
                Some(initial) => {
                    let mut submenu = Vec::with_capacity(initial.values.len());
                    for value in &initial.values {
                        let mut new_state = change_type(&state, type_config)?;
                        let mut new_options = get(&new_state, "options")?;
                        ops::set_key(
                            &mut new_options,
                            &JsString::from(initial.key.as_str()),
                            JsValue::string(&value.key),
                        )?;
                        ops::set_key(&mut new_state, &JsString::from("options"), new_options)?;
                        submenu.push(
                            menu.click(
                                MenuItem {
                                    label: Some(value.display.clone()),
                                    ..MenuItem::default()
                                }
                                .icon(icon(&value.key)),
                                vec![respond(new_state)],
                            ),
                        );
                    }
                    menu.append(item.submenu(submenu));
                }
                None => {
                    let new_state = change_type(&state, type_config)?;
                    menu.append_click(item, vec![respond(new_state)]);
                }
            }
        }
    }

    if menu.items.is_empty() {
        menu.append(MenuItem::new("No Options").enabled(false));
    }
    menu.append(MenuItem::separator());
    menu.append_click(
        MenuItem::new("Help"),
        vec![Effect::OpenSourceListHelp(config_js)],
    );
    beside(menu, &rect)
}

/// `state.options[optionKey] = value`.
fn set_option(state: &mut JsValue, option_key: &str, value: &str) -> Result<(), JsError> {
    let mut options = get(state, "options")?;
    ops::set_key(
        &mut options,
        &JsString::from(option_key),
        JsValue::string(value),
    )?;
    ops::set_key(state, &JsString::from("options"), options)
}

/// The type change shared by both click handlers of a type entry: `state.type` set, and each
/// of the type's options kept when valid, else its first value.
fn change_type(state: &JsValue, type_config: &SourceListTypeConfig) -> Result<JsValue, JsError> {
    let mut state = state.clone();
    ops::set_key(
        &mut state,
        &JsString::from("type"),
        JsValue::string(&type_config.key),
    )?;
    let options = get(&state, "options")?;
    let mut new_options = JsObject::new();
    for option in &type_config.options {
        let current = if ops::has(&options, &option.key)? {
            Some(get(&options, &option.key)?)
        } else {
            None
        };
        let valid = current.as_ref().filter(|value| {
            option
                .values
                .iter()
                .any(|v| ops::same_value_zero(&JsValue::string(&v.key), value))
        });
        let value = match valid {
            Some(value) => value.clone(),
            None => JsValue::string(
                &option
                    .values
                    .first()
                    .ok_or_else(|| {
                        JsError::type_error("Cannot read properties of undefined (reading 'key')")
                    })?
                    .key,
            ),
        };
        new_options.insert(option.key.as_str(), value);
    }
    ops::set_key(
        &mut state,
        &JsString::from("options"),
        JsValue::Object(new_options),
    )?;
    Ok(state)
}
