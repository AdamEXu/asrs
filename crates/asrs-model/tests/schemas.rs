use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use asrs_js::json;
use asrs_js::value::ToJs;
use asrs_model::hub_state::HubState;
use asrs_model::json_file;
use asrs_model::messages::{
    DownloadToMain, HubToMain, MainToDownload, MainToHub, MainToSatellite, SatelliteToMain,
    SelectionSetter,
};
use asrs_model::preferences::merge_preferences;
use asrs_model::source_list_config::{SourceListItemState, SourceListTypeMemoryEntry};
use asrs_model::tab_type::TabType;

const HUB_STATE: &str = r##"{"sidebar":{"width":300,"expanded":["/AdvantageKit","/Real\ud800"]},"tabs":{"selected":1,"tabs":[{"type":0,"title":"","controller":null,"controllerUUID":"a","renderer":null,"controlsHeight":0},{"type":1,"title":"Line Graph","controller":{"leftSources":[{"type":"stepped","logKey":"/x","logType":"Number","visible":true,"options":{"color":"#2b66a2"}}],"leftLockedRange":null},"controllerUUID":"b","renderer":{"x":-1.5e-7},"controlsHeight":200}]}}"##;

#[test]
fn hub_state_round_trips() {
    let parsed = json::parse_str(HUB_STATE).unwrap();
    let state = HubState::from_js(&parsed).expect("well-formed hub state");
    assert_eq!(state.tabs.tabs[1].tab_type, TabType::LineGraph);
    assert_eq!(json::stringify(&state.to_js()).unwrap().unwrap(), HUB_STATE);
}

#[test]
fn hub_state_rejects_unknown_tab_types() {
    for bad in ["13", "1.5", "\"1\"", "-1"] {
        let text = HUB_STATE.replacen("\"type\":1,", &format!("\"type\":{bad},"), 1);
        let parsed = json::parse_str(&text).unwrap();
        assert!(HubState::from_js(&parsed).is_none(), "{bad}");
    }
}

#[test]
fn source_list_states_round_trip() {
    let item = r##"{"type":"stepped","logKey":"/x","logType":"Number","visible":false,"options":{"1":"a","color":"#2b66a2"}}"##;
    let parsed = json::parse_str(item).unwrap();
    let state = SourceListItemState::from_js(&parsed).unwrap();
    assert_eq!(json::stringify(&state.to_js()).unwrap().unwrap(), item);

    let entry = r#"{"type":"robot","options":{"bumpers":""}}"#;
    let parsed = json::parse_str(entry).unwrap();
    let state = SourceListTypeMemoryEntry::from_js(&parsed).unwrap();
    assert_eq!(json::stringify(&state.to_js()).unwrap().unwrap(), entry);
}

#[test]
fn json_file_pretty_layout_form() {
    let value = json::parse_str(r#"{"hubs":[],"satellites":[{}],"version":"dev"}"#).unwrap();
    assert_eq!(
        String::from_utf8(json_file::stringify_pretty(&value).unwrap()).unwrap(),
        "{\n  \"hubs\": [],\n  \"satellites\": [\n    {}\n  ],\n  \"version\": \"dev\"\n}\n"
    );
}

#[test]
fn update_preferences_on_a_primitive_file_drops_assignments_like_sloppy_mode() {
    let data = json::parse_str(r#"{"theme":"dark","rlogPort":1}"#).unwrap();
    for text in ["5", "\"prefs\"", "true"] {
        let mut base = json::parse_str(text).unwrap();
        merge_preferences(&mut base, &data).unwrap();
        assert_eq!(json::stringify(&base).unwrap().unwrap(), text);
    }
    let mut base = json::parse_str("null").unwrap();
    assert_eq!(
        merge_preferences(&mut base, &data).unwrap_err().name,
        "TypeError"
    );
}

fn names<T: Copy>(all: &[T], name: fn(T) -> &'static str) -> BTreeSet<String> {
    let set: BTreeSet<String> = all.iter().map(|m| name(*m).to_owned()).collect();
    assert_eq!(set.len(), all.len(), "duplicate message names");
    set
}

#[test]
fn message_names_round_trip() {
    for m in HubToMain::ALL {
        assert_eq!(HubToMain::from_name(m.name()), Some(*m));
    }
    for m in MainToHub::ALL {
        assert_eq!(MainToHub::from_name(m.name()), Some(*m));
    }
    assert_eq!(HubToMain::from_name("open-xr"), None);
}

fn upstream_source(path: &str) -> Option<String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/AdvantageScope/src");
    fs::read_to_string(root.join(path)).ok()
}

fn cases(source: &str) -> BTreeSet<String> {
    source
        .split("case \"")
        .skip(1)
        .filter_map(|rest| rest.split_once("\":").map(|(name, _)| name.to_owned()))
        .collect()
}

fn with(mut set: BTreeSet<String>, extra: &[&str]) -> BTreeSet<String> {
    set.extend(extra.iter().map(|s| (*s).to_owned()));
    set
}

fn without(mut set: BTreeSet<String>, drop: &[&str]) -> BTreeSet<String> {
    for name in drop {
        set.remove(*name);
    }
    set
}

/// Compares the enums with the `case` labels of each upstream receiver. Skipped when the
/// read-only upstream checkout is absent.
#[test]
fn message_names_match_upstream_receivers() {
    let (Some(hub), Some(satellite), Some(download), Some(main)) = (
        upstream_source("hub/hub.ts"),
        upstream_source("satellite.ts"),
        upstream_source("download.ts"),
        upstream_source("main/electron/main.ts"),
    ) else {
        eprintln!("reference/AdvantageScope not found; skipping");
        return;
    };

    let hub_handler = &hub[hub.find("async function handleMainMessage").unwrap()..];
    let setter_start = hub_handler.find("case \"call-selection-setter\":").unwrap();
    let setter_end = setter_start + hub_handler[setter_start..].find("\n    case \"").unwrap();
    let setters = cases(&hub_handler[setter_start..setter_end]);
    assert_eq!(
        cases(&hub_handler[..setter_start])
            .union(&cases(&hub_handler[setter_end..]))
            .cloned()
            .collect::<BTreeSet<_>>(),
        with(
            without(
                names(MainToHub::ALL, MainToHub::name),
                &["call-selection-setter"]
            ),
            &["set-active-xr-uuid"]
        )
    );
    assert_eq!(
        without(setters, &["call-selection-setter"]),
        names(SelectionSetter::ALL, SelectionSetter::name)
    );
    assert_eq!(
        without(
            cases(&satellite),
            &["ArrowLeft", "ArrowRight", "KeyL", "Space"]
        ),
        names(MainToSatellite::ALL, MainToSatellite::name)
    );
    assert_eq!(
        without(cases(&download), &["darwin", "linux", "win32", "lite"]),
        names(MainToDownload::ALL, MainToDownload::name)
    );

    let start = main.find("async function handleHubMessage").unwrap();
    let end = start + main[start..].find("\n}\n").unwrap();
    assert_eq!(
        without(cases(&main[start..end]), &["none", "inconsistent"]),
        with(
            names(HubToMain::ALL, HubToMain::name),
            &["open-xr", "confirm-xr-close", "update-xr-command"]
        )
    );

    let end = main
        .find("Unknown message from satellite renderer process")
        .unwrap();
    let start = main[..end].rfind("switch (message.name)").unwrap();
    assert_eq!(
        cases(&main[start..end]),
        names(SatelliteToMain::ALL, SatelliteToMain::name)
    );

    let start = main.find("function handleDownloadMessage").unwrap();
    let end = start + main[start..].find("\n}\n").unwrap();
    assert_eq!(
        cases(&main[start..end]),
        names(DownloadToMain::ALL, DownloadToMain::name)
    );
}
