//! The `SourceListConfig` constants of the tab controllers (`*Controller_Config.ts`). The JSON
//! files are `JSON.stringify` of upstream's objects; the `dom/source-list/*` vectors check them.

use asrs_js::json;

use crate::source_list_config::SourceListConfig;

fn parse(text: &str) -> SourceListConfig {
    let value = json::parse_str(text).expect("bundled source list config is valid JSON");
    SourceListConfig::from_js(&value).expect("bundled source list config has the upstream shape")
}

/// `LineGraphController_NumericConfig`: the left and right axes.
pub fn line_graph_numeric() -> SourceListConfig {
    parse(include_str!("configs/line_graph_numeric.json"))
}

/// `LineGraphController_DiscreteConfig`.
pub fn line_graph_discrete() -> SourceListConfig {
    parse(include_str!("configs/line_graph_discrete.json"))
}

pub fn field_2d() -> SourceListConfig {
    parse(include_str!("configs/field_2d.json"))
}

pub fn field_3d() -> SourceListConfig {
    parse(include_str!("configs/field_3d.json"))
}

pub fn statistics() -> SourceListConfig {
    parse(include_str!("configs/statistics.json"))
}

pub fn swerve() -> SourceListConfig {
    parse(include_str!("configs/swerve.json"))
}

pub fn mechanism() -> SourceListConfig {
    parse(include_str!("configs/mechanism.json"))
}

pub fn points() -> SourceListConfig {
    parse(include_str!("configs/points.json"))
}

pub fn all() -> [(&'static str, SourceListConfig); 8] {
    [
        ("line_graph_numeric", line_graph_numeric()),
        ("line_graph_discrete", line_graph_discrete()),
        ("field_2d", field_2d()),
        ("field_3d", field_3d()),
        ("statistics", statistics()),
        ("swerve", swerve()),
        ("mechanism", mechanism()),
        ("points", points()),
    ]
}
