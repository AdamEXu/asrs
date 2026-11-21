//! `src/main/lite/dsLayout.ts`: `DEFAULT_DS_LAYOUT`, shared by the desktop "Reset to DS Layout"
//! menu item. The `include_docs` flag is `DISTRIBUTION !== Distribution.LiteDS`, `true` for the
//! full desktop build.

use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_model::hub_state::{HubState, SidebarState, TabState, TabsState};
use asrs_model::tab_type::TabType;

pub fn default_ds_layout(include_docs: bool) -> HubState {
    let mut tabs = Vec::new();
    if include_docs {
        tabs.push(TabState {
            tab_type: TabType::Documentation,
            title: JsString::from(""),
            controller: JsValue::Null,
            controller_uuid: JsString::from("0"),
            renderer: JsValue::string(""),
            controls_height: 0.0,
        });
    }
    tabs.push(TabState {
        tab_type: TabType::Console,
        title: JsString::from(""),
        controller: JsValue::string("DS:/Dscomm/Console"),
        controller_uuid: JsString::from("1"),
        renderer: ops::object([("highlight", JsValue::Bool(true))]),
        controls_height: 0.0,
    });
    tabs.push(line_graph(
        "Communication",
        "2",
        vec![stepped("DS:/Dscomm/Status/Timing/Rtt", "#3b875a", "normal")],
        vec![stepped(
            "DS:/Dscomm/Status/Timing/LostPackets",
            "#e5b31b",
            "normal",
        )],
        vec![
            stripes("DS:/Dscomm/Status/Connection/HasTcpConn", "#2b66a2"),
            stripes("DS:/Dscomm/Status/Robot/HasUserCode", "#80588e"),
            stripes("DS:/Dscomm/Status/Robot/HasUserCodeReady", "#e48b32"),
        ],
    ));
    tabs.push(line_graph(
        "Power",
        "3",
        vec![stepped(
            "DS:/Dscomm/Status/Power/Battery",
            "#e5b31b",
            "normal",
        )],
        vec![],
        vec![],
    ));
    tabs.push(line_graph(
        "CPU",
        "4",
        vec![
            stepped("DS:/Dscomm/Status/Resources/CPU", "#af2437", "bold"),
            stepped(
                "DS:/Dscomm/Status/DriverStation/DsCpuUtilization",
                "#80588e",
                "normal",
            ),
        ],
        vec![],
        vec![],
    ));
    tabs.push(TabState {
        tab_type: TabType::Joysticks,
        title: JsString::from(""),
        controller: JsValue::Array(
            ["Generic Joystick", "None", "None", "None", "None", "None"]
                .into_iter()
                .map(JsValue::string)
                .collect(),
        ),
        controller_uuid: JsString::from("5"),
        renderer: JsValue::Null,
        controls_height: 85.0,
    });
    HubState {
        sidebar: SidebarState {
            width: 300.0,
            expanded: vec![JsString::from("/Control"), JsString::from("/Status")],
        },
        tabs: TabsState {
            selected: if include_docs { 1.0 } else { 0.0 },
            tabs,
        },
    }
}

fn stepped(log_key: &str, color: &str, size: &str) -> JsValue {
    source("stepped", log_key, "Number", color, Some(size))
}

fn stripes(log_key: &str, color: &str) -> JsValue {
    source("stripes", log_key, "Boolean", color, None)
}

fn source(
    source_type: &str,
    log_key: &str,
    log_type: &str,
    color: &str,
    size: Option<&str>,
) -> JsValue {
    let mut options = JsObject::new();
    options.insert(JsString::from("color"), JsValue::string(color));
    if let Some(size) = size {
        options.insert(JsString::from("size"), JsValue::string(size));
    }
    ops::object([
        ("type", JsValue::string(source_type)),
        ("logKey", JsValue::string(log_key)),
        ("logType", JsValue::string(log_type)),
        ("visible", JsValue::Bool(true)),
        ("options", JsValue::Object(options)),
    ])
}

fn unit_conversion() -> JsValue {
    ops::object([("autoTarget", JsValue::Null), ("preset", JsValue::Null)])
}

fn line_graph(
    title: &str,
    uuid: &str,
    left_sources: Vec<JsValue>,
    right_sources: Vec<JsValue>,
    discrete_sources: Vec<JsValue>,
) -> TabState {
    TabState {
        tab_type: TabType::LineGraph,
        title: JsString::from(title),
        controller: ops::object([
            ("leftSources", JsValue::Array(left_sources)),
            ("rightSources", JsValue::Array(right_sources)),
            ("discreteSources", JsValue::Array(discrete_sources)),
            ("leftLockedRange", JsValue::Null),
            ("rightLockedRange", JsValue::Null),
            ("leftUnitConversion", unit_conversion()),
            ("rightUnitConversion", unit_conversion()),
            ("leftFilter", JsValue::Number(0.0)),
            ("rightFilter", JsValue::Number(0.0)),
            ("showRobotMode", JsValue::Bool(true)),
        ]),
        controller_uuid: JsString::from(uuid),
        renderer: JsValue::Null,
        controls_height: 200.0,
    }
}
