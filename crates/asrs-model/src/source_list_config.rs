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
