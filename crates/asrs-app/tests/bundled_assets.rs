#![allow(clippy::disallowed_methods)]

//! The bundled fields and robots through the port's asset service and into a hub window's
//! `set-assets`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::path::Path;

use asrs_app::views::frame::FrameHost;
use asrs_app::views::standalone::StandaloneHub;
use asrs_js::string::JsString;
use asrs_model::messages::MainToHub;
use asrs_parity::Value;
use asrs_parity::vectors;
use asrs_services::asset_loader::{AssetFolders, AssetService, USER_ASSETS_README};
use asrs_services::fs::RealFs;
use asrs_services::path::PathStyle;

fn without_paths(value: &Value) -> Value {
    match value {
        Value::Object(pairs) => Value::Object(
            pairs
                .iter()
                .filter(|(k, _)| k.to_string_lossy() != "path")
                .map(|(k, v)| (k.clone(), without_paths(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_paths).collect()),
        other => other.clone(),
    }
}

fn asset_paths(value: &Value) -> Vec<String> {
    ["field2ds", "field3ds", "robots", "joysticks"]
        .iter()
        .flat_map(|key| value.get(key).and_then(Value::as_array).unwrap_or(&[]))
        .map(|asset| {
            asset
                .get("path")
                .and_then(Value::as_str)
                .expect("every asset has a path")
                .to_string_lossy()
        })
        .collect()
}

/// The bundled fields and robots, loaded from upstream's real `bundledAssets` folder by the
/// port's asset service on the real filesystem, equal what upstream's loader produced for the
/// same files in `files/asset-loader` (apart from the folder prefix in `path`) and load into the
/// hub through `set-assets` without a load-failure alert.
#[test]
fn bundled_assets_load_into_set_assets() {
    let bundled = vectors::workspace_root().join("reference/AdvantageScope/bundledAssets");
    let user_data = std::env::temp_dir().join(format!("asrs-assets-{}", std::process::id()));
    std::fs::create_dir_all(&user_data).unwrap();
    std::fs::write(user_data.join("prefs.json"), "{\"userAssetsFolder\":null}").unwrap();
    let folders = AssetFolders::new(
        PathStyle::host(),
        user_data.to_str().unwrap(),
        bundled.to_str().unwrap(),
    );
    let mut service = AssetService::new(folders);
    let started = service.start(&RealFs);
    let readme = std::fs::read(user_data.join("userAssets").join("README.txt"));
    std::fs::remove_dir_all(&user_data).unwrap();
    started.unwrap();
    assert_eq!(readme.unwrap(), USER_ASSETS_README.as_bytes());

    let records = vectors::load("files/asset-loader").unwrap();
    let oracle = records
        .iter()
        .find(|r| r.string("op").as_deref() == Some("loadAssets") && r.field("result").is_some())
        .and_then(|r| r.value("result").ok())
        .expect("the oracle loaded the bundled assets");
    let data = service.set_assets_data();
    let port = js_json::from_js(&data);
    assert_eq!(
        without_paths(&port).encode(),
        without_paths(&oracle).encode()
    );
    let expected_paths = asset_paths(&oracle);
    let port_paths = asset_paths(&port);
    assert_eq!(port_paths.len(), expected_paths.len());
    for (port_path, oracle_path) in port_paths.iter().zip(&expected_paths) {
        let decoded = asrs_js::string::decode_uri_component(&JsString::from(port_path.as_str()))
            .unwrap()
            .to_string_lossy();
        let suffix = oracle_path
            .strip_prefix("/fake/bundledAssets/")
            .expect("bundled path");
        assert_eq!(
            Path::new(&decoded),
            bundled.join(suffix).as_path(),
            "{port_path}"
        );
        assert!(Path::new(&decoded).is_file(), "{decoded}");
    }
    let assets = service.assets();
    assert_eq!(assets.field2ds.len(), 2);
    assert_eq!(assets.robots.len(), 2);
    assert!(assets.load_failures.is_empty());

    let mut hub = StandaloneHub::new(false, false);
    let replies = hub.main_message(MainToHub::SetAssets, data);
    assert!(
        replies.is_empty(),
        "set-assets replied {} messages",
        replies.len()
    );
    assert_eq!(hub.hub().page().assets(), Some(assets));
}
