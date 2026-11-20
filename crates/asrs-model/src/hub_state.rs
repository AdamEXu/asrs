//! `src/shared/HubState.ts`: the hub window state saved in the state file and in exported
//! layouts. Key order follows `Sidebar.saveState` and `Tabs.saveState`.
//!
//! `from_js` accepts only the well-formed shape; how a hub restores a malformed one is up to the
//! sidebar and tabs restore code, which reads the raw value.

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};

use crate::tab_type::TabType;

#[derive(Clone, Debug, PartialEq)]
pub struct HubState {
    pub sidebar: SidebarState,
    pub tabs: TabsState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SidebarState {
    pub width: f64,
    pub expanded: Vec<JsString>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabsState {
    pub selected: f64,
    pub tabs: Vec<TabState>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabState {
    pub tab_type: TabType,
    pub title: JsString,
    pub controller: JsValue,
    pub controller_uuid: JsString,
    pub renderer: JsValue,
    pub controls_height: f64,
}

impl ToJs for HubState {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("sidebar", self.sidebar.to_js());
        o.insert("tabs", self.tabs.to_js());
        JsValue::Object(o)
    }
}

impl ToJs for SidebarState {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("width", self.width.to_js());
        o.insert("expanded", self.expanded.to_js());
        JsValue::Object(o)
    }
}

impl ToJs for TabsState {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("selected", self.selected.to_js());
        o.insert("tabs", self.tabs.to_js());
        JsValue::Object(o)
    }
}

impl ToJs for TabState {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("type", self.tab_type.to_js());
        o.insert("title", self.title.to_js());
        o.insert("controller", self.controller.clone());
        o.insert("controllerUUID", self.controller_uuid.to_js());
        o.insert("renderer", self.renderer.clone());
        o.insert("controlsHeight", self.controls_height.to_js());
        JsValue::Object(o)
    }
}

fn string(o: &JsObject, key: &str) -> Option<JsString> {
    o.get(key)?.as_str().cloned()
}

fn number(o: &JsObject, key: &str) -> Option<f64> {
    o.get(key)?.as_f64()
}

impl HubState {
    pub fn from_js(value: &JsValue) -> Option<HubState> {
        let o = value.as_object()?;
        Some(HubState {
            sidebar: SidebarState::from_js(o.get("sidebar")?)?,
            tabs: TabsState::from_js(o.get("tabs")?)?,
        })
    }
}

impl SidebarState {
    pub fn from_js(value: &JsValue) -> Option<SidebarState> {
        let o = value.as_object()?;
        Some(SidebarState {
            width: number(o, "width")?,
            expanded: o
                .get("expanded")?
                .as_array()?
                .iter()
                .map(|v| v.as_str().cloned())
                .collect::<Option<_>>()?,
        })
    }
}

impl TabsState {
    pub fn from_js(value: &JsValue) -> Option<TabsState> {
        let o = value.as_object()?;
        Some(TabsState {
            selected: number(o, "selected")?,
            tabs: o
                .get("tabs")?
                .as_array()?
                .iter()
                .map(TabState::from_js)
                .collect::<Option<_>>()?,
        })
    }
}

impl TabState {
    pub fn from_js(value: &JsValue) -> Option<TabState> {
        let o = value.as_object()?;
        Some(TabState {
            tab_type: TabType::from_number(number(o, "type")?)?,
            title: string(o, "title")?,
            controller: o.get("controller").cloned().unwrap_or_default(),
            controller_uuid: string(o, "controllerUUID")?,
            renderer: o.get("renderer").cloned().unwrap_or_default(),
            controls_height: number(o, "controlsHeight")?,
        })
    }
}
