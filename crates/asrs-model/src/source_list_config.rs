//! `src/shared/SourceListConfig.ts`. The configs are static per tab; the item states and the
//! type memory are persisted (in `HubState` controller state and type-memory.json).

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, ToJs};

#[derive(Clone, Debug, PartialEq)]
pub struct SourceListConfig {
    pub title: String,
    pub auto_advance: AutoAdvance,
    /// False when parent types (arrays, structs) are supported directly.
    pub allow_children_from_drag: bool,
    /// Remembers types and options per field when set.
    pub type_memory_id: Option<String>,
    pub types: Vec<SourceListTypeConfig>,
}

/// `autoAdvance: boolean | string`.
#[derive(Clone, Debug, PartialEq)]
pub enum AutoAdvance {
    /// `true` advances the type, `false` advances nothing.
    Type(bool),
    /// Advances the value of this option.
    Option(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceListTypeConfig {
    pub key: String,
    pub display: String,
    pub symbol: String,
    pub show_in_type_name: bool,
    /// An option key, or a hex colour starting with `#`.
    pub color: String,
    pub dark_color: Option<String>,
    pub source_types: Vec<String>,
    /// Identifies parents that share child types.
    pub parent_key: Option<String>,
    /// The parent key this child type attaches to.
    pub child_of: Option<String>,
    pub preview_type: PreviewSetting,
    pub initial_selection_option: Option<String>,
    pub show_docs: bool,
    pub options: Vec<SourceListOptionConfig>,
}

/// `previewType`: `SourceList` treats a missing key and an explicit `null` differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PreviewSetting {
    #[default]
    Unset,
    Disabled,
    Type(PreviewType),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewType {
    Rotation2d,
    Translation2d,
    Pose2d,
    Transform2d,
    Rotation3d,
    Translation3d,
    Pose3d,
    Transform3d,
    ModuleVelocities,
    RobotVelocities,
}

impl PreviewType {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewType::Rotation2d => "Rotation2d",
            PreviewType::Translation2d => "Translation2d",
            PreviewType::Pose2d => "Pose2d",
            PreviewType::Transform2d => "Transform2d",
            PreviewType::Rotation3d => "Rotation3d",
            PreviewType::Translation3d => "Translation3d",
            PreviewType::Pose3d => "Pose3d",
            PreviewType::Transform3d => "Transform3d",
            PreviewType::ModuleVelocities => "ModuleVelocities",
            PreviewType::RobotVelocities => "RobotVelocities",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceListOptionConfig {
    pub key: String,
    pub display: String,
    pub show_in_type_name: bool,
    pub values: Vec<SourceListOptionValueConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceListOptionValueConfig {
    pub key: String,
    pub display: String,
}

pub type SourceListState = Vec<SourceListItemState>;

/// Key order as `SourceList.addField` builds it.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceListItemState {
    pub item_type: JsString,
    pub log_key: JsString,
    pub log_type: JsString,
    pub visible: bool,
    pub options: JsObject<JsString>,
}

impl ToJs for SourceListItemState {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("type", self.item_type.to_js());
        o.insert("logKey", self.log_key.to_js());
        o.insert("logType", self.log_type.to_js());
        o.insert("visible", self.visible.to_js());
        o.insert("options", self.options.to_js());
        JsValue::Object(o)
    }
}

impl SourceListItemState {
    pub fn from_js(value: &JsValue) -> Option<SourceListItemState> {
        let o = value.as_object()?;
        Some(SourceListItemState {
            item_type: o.get("type")?.as_str()?.clone(),
            log_key: o.get("logKey")?.as_str()?.clone(),
            log_type: o.get("logType")?.as_str()?.clone(),
            visible: o.get("visible")?.as_bool()?,
            options: string_map(o.get("options")?)?,
        })
    }
}

/// Memory id, then log key, then the remembered entry.
pub type SourceListTypeMemory = JsObject<JsObject<SourceListTypeMemoryEntry>>;

#[derive(Clone, Debug, PartialEq)]
pub struct SourceListTypeMemoryEntry {
    pub entry_type: JsString,
    pub options: JsObject<JsString>,
}

impl ToJs for SourceListTypeMemoryEntry {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("type", self.entry_type.to_js());
        o.insert("options", self.options.to_js());
        JsValue::Object(o)
    }
}

impl SourceListTypeMemoryEntry {
    pub fn from_js(value: &JsValue) -> Option<SourceListTypeMemoryEntry> {
        let o = value.as_object()?;
        Some(SourceListTypeMemoryEntry {
            entry_type: o.get("type")?.as_str()?.clone(),
            options: string_map(o.get("options")?)?,
        })
    }
}

fn string_map(value: &JsValue) -> Option<JsObject<JsString>> {
    let o = value.as_object()?;
    let mut out = JsObject::new();
    for (k, v) in o.iter() {
        out.insert(k.clone(), v.as_str()?.clone());
    }
    Some(out)
}

impl SourceListConfig {
    /// Reads a config object as upstream holds it (`previewType` missing and `null` differ).
    pub fn from_js(value: &JsValue) -> Option<SourceListConfig> {
        let o = value.as_object()?;
        let auto_advance = match o.get("autoAdvance")? {
            JsValue::Bool(b) => AutoAdvance::Type(*b),
            JsValue::String(s) => AutoAdvance::Option(s.as_string()?),
            _ => return None,
        };
        Some(SourceListConfig {
            title: text(o.get("title")?)?,
            auto_advance,
            allow_children_from_drag: o.get("allowChildrenFromDrag")?.as_bool()?,
            type_memory_id: optional_text(o.get("typeMemoryId"))?,
            types: o
                .get("types")?
                .as_array()?
                .iter()
                .map(SourceListTypeConfig::from_js)
                .collect::<Option<_>>()?,
        })
    }
}

impl ToJs for SourceListConfig {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("title", self.title.to_js());
        o.insert(
            "autoAdvance",
            match &self.auto_advance {
                AutoAdvance::Type(b) => b.to_js(),
                AutoAdvance::Option(key) => key.to_js(),
            },
        );
        o.insert(
            "allowChildrenFromDrag",
            self.allow_children_from_drag.to_js(),
        );
        if let Some(id) = &self.type_memory_id {
            o.insert("typeMemoryId", id.to_js());
        }
        o.insert("types", self.types.to_js());
        JsValue::Object(o)
    }
}

impl SourceListTypeConfig {
    pub fn from_js(value: &JsValue) -> Option<SourceListTypeConfig> {
        let o = value.as_object()?;
        let preview_type = match o.get("previewType") {
            None | Some(JsValue::Undefined) => PreviewSetting::Unset,
            Some(JsValue::Null) => PreviewSetting::Disabled,
            Some(JsValue::String(s)) => PreviewSetting::Type(PreviewType::parse(&s.as_string()?)?),
            Some(_) => return None,
        };
        Some(SourceListTypeConfig {
            key: text(o.get("key")?)?,
            display: text(o.get("display")?)?,
            symbol: text(o.get("symbol")?)?,
            show_in_type_name: o.get("showInTypeName")?.as_bool()?,
            color: text(o.get("color")?)?,
            dark_color: optional_text(o.get("darkColor"))?,
            source_types: o
                .get("sourceTypes")?
                .as_array()?
                .iter()
                .map(text)
                .collect::<Option<_>>()?,
            parent_key: optional_text(o.get("parentKey"))?,
            child_of: optional_text(o.get("childOf"))?,
            preview_type,
            initial_selection_option: optional_text(o.get("initialSelectionOption"))?,
            show_docs: o.get("showDocs")?.as_bool()?,
            options: o
                .get("options")?
                .as_array()?
                .iter()
                .map(SourceListOptionConfig::from_js)
                .collect::<Option<_>>()?,
        })
    }
}

impl ToJs for SourceListTypeConfig {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("key", self.key.to_js());
        o.insert("display", self.display.to_js());
        o.insert("symbol", self.symbol.to_js());
        o.insert("showInTypeName", self.show_in_type_name.to_js());
        o.insert("color", self.color.to_js());
        if let Some(dark) = &self.dark_color {
            o.insert("darkColor", dark.to_js());
        }
        o.insert("sourceTypes", self.source_types.to_js());
        if let Some(parent) = &self.parent_key {
            o.insert("parentKey", parent.to_js());
        }
        if let Some(child_of) = &self.child_of {
            o.insert("childOf", child_of.to_js());
        }
        match self.preview_type {
            PreviewSetting::Unset => {}
            PreviewSetting::Disabled => {
                o.insert("previewType", JsValue::Null);
            }
            PreviewSetting::Type(t) => {
                o.insert("previewType", t.as_str().to_js());
            }
        }
        if let Some(option) = &self.initial_selection_option {
            o.insert("initialSelectionOption", option.to_js());
        }
        o.insert("showDocs", self.show_docs.to_js());
        o.insert("options", self.options.to_js());
        JsValue::Object(o)
    }
}

impl PreviewType {
    pub fn parse(name: &str) -> Option<PreviewType> {
        Some(match name {
            "Rotation2d" => PreviewType::Rotation2d,
            "Translation2d" => PreviewType::Translation2d,
            "Pose2d" => PreviewType::Pose2d,
            "Transform2d" => PreviewType::Transform2d,
            "Rotation3d" => PreviewType::Rotation3d,
            "Translation3d" => PreviewType::Translation3d,
            "Pose3d" => PreviewType::Pose3d,
            "Transform3d" => PreviewType::Transform3d,
            "ModuleVelocities" => PreviewType::ModuleVelocities,
            "RobotVelocities" => PreviewType::RobotVelocities,
            _ => return None,
        })
    }
}

impl SourceListOptionConfig {
    pub fn from_js(value: &JsValue) -> Option<SourceListOptionConfig> {
        let o = value.as_object()?;
        Some(SourceListOptionConfig {
            key: text(o.get("key")?)?,
            display: text(o.get("display")?)?,
            show_in_type_name: o.get("showInTypeName")?.as_bool()?,
            values: SourceListOptionValueConfig::list_from_js(o.get("values")?)?,
        })
    }
}

impl ToJs for SourceListOptionConfig {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("key", self.key.to_js());
        o.insert("display", self.display.to_js());
        o.insert("showInTypeName", self.show_in_type_name.to_js());
        o.insert("values", self.values.to_js());
        JsValue::Object(o)
    }
}

impl SourceListOptionValueConfig {
    pub fn from_js(value: &JsValue) -> Option<SourceListOptionValueConfig> {
        let o = value.as_object()?;
        Some(SourceListOptionValueConfig {
            key: text(o.get("key")?)?,
            display: text(o.get("display")?)?,
        })
    }

    pub fn list_from_js(value: &JsValue) -> Option<Vec<SourceListOptionValueConfig>> {
        value
            .as_array()?
            .iter()
            .map(SourceListOptionValueConfig::from_js)
            .collect()
    }
}

impl ToJs for SourceListOptionValueConfig {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("key", self.key.to_js());
        o.insert("display", self.display.to_js());
        JsValue::Object(o)
    }
}

impl SourceListTypeMemoryEntry {
    pub fn memory_from_js(value: &JsValue) -> Option<SourceListTypeMemory> {
        let o = value.as_object()?;
        let mut out = JsObject::new();
        for (id, entries) in o.iter() {
            let mut inner = JsObject::new();
            for (key, entry) in entries.as_object()?.iter() {
                inner.insert(key.clone(), SourceListTypeMemoryEntry::from_js(entry)?);
            }
            out.insert(id.clone(), inner);
        }
        Some(out)
    }
}

fn text(value: &JsValue) -> Option<String> {
    value.as_str()?.as_string()
}

fn optional_text(value: Option<&JsValue>) -> Option<Option<String>> {
    match value {
        None | Some(JsValue::Undefined) => Some(None),
        Some(v) => text(v).map(Some),
    }
}
