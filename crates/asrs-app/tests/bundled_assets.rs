#![allow(clippy::disallowed_methods)]

//! The bundled fields and robots through the port's asset service and into a hub window's
//! `set-assets`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use asrs_app::hub::{Hub, HubConfig, HubEvent};
use asrs_hub::hub::HubView;
use asrs_hub::sidebar::{NoLayout, SidebarGeometry};
use asrs_js::string::JsString;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_parity::Value;
use asrs_parity::vectors;
use asrs_services::asset_loader::{AssetFolders, AssetService, USER_ASSETS_README};
use asrs_services::fs::RealFs;
use asrs_services::path::PathStyle;
use asrs_ui::gpui::{
    AppContext, Context, Entity, HeadlessAppContext, IntoElement, NoopTextSystem, Render, Window,
    div, px, size,
};

struct View {
    sidebar: NoLayout,
}

impl HubView for View {
    fn inner_height(&self) -> f64 {
        650.0
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        92.0
    }

    fn dark(&self) -> bool {
        false
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn source_list_dom(&mut self) -> &mut dyn asrs_model::source_list::SourceListDom {
        asrs_model::source_list::Undisplayed::dom()
    }
}

struct Shell(#[allow(dead_code)] Entity<Hub>);

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
    }
}

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

    let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
    let slot = Rc::new(RefCell::new(None));
    let built = slot.clone();
    cx.open_window(size(px(1100.0), px(650.0)), move |window, cx| {
        let hub = cx.new(|cx| {
            Hub::new(
                Some(window),
                HubConfig::new(Box::new(View { sidebar: NoLayout })),
                cx,
            )
        });
        *built.borrow_mut() = Some(hub.clone());
        cx.new(|_| Shell(hub))
    })
    .expect("open a headless window");
    let hub = slot.borrow_mut().take().expect("the window built the hub");
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|cx| {
        cx.subscribe(&hub, move |_, event: &HubEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
        hub.update(cx, |hub, cx| {
            hub.handle_main_message(NamedMessage::new(MainToHub::SetAssets, data), cx)
        });
    });
    let replies: Vec<_> = events
        .borrow()
        .iter()
        .filter(|e| matches!(e, HubEvent::Main(_) | HubEvent::Console { warn: true, .. }))
        .cloned()
        .collect();
    assert!(replies.is_empty(), "set-assets replied {replies:?}");
    cx.update(|cx| assert_eq!(hub.read(cx).model().page().assets(), Some(assets)));
}
