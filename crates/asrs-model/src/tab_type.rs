//! `src/shared/TabType.ts`. The numeric values are written into saved layouts and state files.

use asrs_js::value::{JsValue, ToJs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum TabType {
    Documentation = 0,
    LineGraph = 1,
    Field2d = 2,
    Field3d = 3,
    Table = 4,
    Console = 5,
    Statistics = 6,
    Video = 7,
    Joysticks = 8,
    Swerve = 9,
    Mechanism = 10,
    Points = 11,
    Metadata = 12,
}

impl TabType {
    /// `getAllTabTypes()`: declaration order.
    pub const ALL: [TabType; 13] = [
        TabType::Documentation,
        TabType::LineGraph,
        TabType::Field2d,
        TabType::Field3d,
        TabType::Table,
        TabType::Console,
        TabType::Statistics,
        TabType::Video,
        TabType::Joysticks,
        TabType::Swerve,
        TabType::Mechanism,
        TabType::Points,
        TabType::Metadata,
    ];

    pub fn number(self) -> f64 {
        f64::from(self as u8)
    }

    /// The member `=== value`.
    pub fn from_number(value: f64) -> Option<TabType> {
        TabType::ALL.into_iter().find(|t| t.number() == value)
    }

    /// The TypeScript enum member name, which is also the enum object's reverse mapping.
    pub fn name(self) -> &'static str {
        match self {
            TabType::Documentation => "Documentation",
            TabType::LineGraph => "LineGraph",
            TabType::Field2d => "Field2d",
            TabType::Field3d => "Field3d",
            TabType::Table => "Table",
            TabType::Console => "Console",
            TabType::Statistics => "Statistics",
            TabType::Video => "Video",
            TabType::Joysticks => "Joysticks",
            TabType::Swerve => "Swerve",
            TabType::Mechanism => "Mechanism",
            TabType::Points => "Points",
            TabType::Metadata => "Metadata",
        }
    }

    /// `getDefaultTabTitle`.
    pub fn default_title(self) -> &'static str {
        match self {
            TabType::Documentation => "",
            TabType::LineGraph => "Line Graph",
            TabType::Field2d => "2D Field",
            TabType::Field3d => "3D Field",
            TabType::Table => "Table",
            TabType::Console => "Console",
            TabType::Statistics => "Statistics",
            TabType::Video => "Video",
            TabType::Joysticks => "Joysticks",
            TabType::Swerve => "Swerve",
            TabType::Mechanism => "Mechanism",
            TabType::Points => "Points",
            TabType::Metadata => "Metadata",
        }
    }

    /// `getTabIcon`. Mechanism is U+2699 U+FE0F; 2D Field has no variation selector.
    pub fn icon(self) -> &'static str {
        match self {
            TabType::Documentation => "\u{1f4d6}",
            TabType::LineGraph => "\u{1f4c9}",
            TabType::Field2d => "\u{1f5fa}",
            TabType::Field3d => "\u{1f440}",
            TabType::Table => "\u{1f522}",
            TabType::Console => "\u{1f4ac}",
            TabType::Statistics => "\u{1f4ca}",
            TabType::Video => "\u{1f3ac}",
            TabType::Joysticks => "\u{1f3ae}",
            TabType::Swerve => "\u{1f980}",
            TabType::Mechanism => "\u{2699}\u{fe0f}",
            TabType::Points => "\u{1f4cd}",
            TabType::Metadata => "\u{1f50d}",
        }
    }

    /// `getTabAccelerator`, in Electron accelerator syntax.
    pub fn accelerator(self) -> &'static str {
        match self {
            TabType::Documentation => "",
            TabType::LineGraph => "Alt+G",
            TabType::Field2d => "Alt+2",
            TabType::Field3d => "Alt+3",
            TabType::Table => "Alt+T",
            TabType::Console => "Alt+C",
            TabType::Statistics => "Alt+S",
            TabType::Video => "Alt+V",
            TabType::Joysticks => "Alt+J",
            TabType::Swerve => "Alt+D",
            TabType::Mechanism => "Alt+M",
            TabType::Points => "Alt+P",
            TabType::Metadata => "Alt+I",
        }
    }
}

impl ToJs for TabType {
    fn to_js(&self) -> JsValue {
        JsValue::Number(self.number())
    }
}
