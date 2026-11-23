#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use asrs_app::scenario::{self, COLUMNS, PATCH, patch_colours};
use asrs_parity::json::{self, Json};
use asrs_parity::{pixel, vectors};
use asrs_services::asset_loader::USER_ASSETS_README;

fn scenario_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../parity/scenarios/{name}.json"))
}

fn run(name: &str, out: &Path) {
    let bundled = vectors::workspace_root().join("reference/AdvantageScope/bundledAssets");
    let args: Vec<String> = [
        "--parity-scenario",
        scenario_file(name).to_str().unwrap(),
        "--out",
        out.join(name).to_str().unwrap(),
        "--bundled-assets",
        bundled.to_str().unwrap(),
    ]
    .map(str::to_owned)
    .to_vec();
    scenario::run(&args).unwrap_or_else(|e| panic!("{name}: {e}"));
}

/// `captures/launch.hub-state.json` from `cargo xtask app run hub-empty`: the HubState of a
/// fresh launch with the bundled assets, the same at every scale and theme.
const UPSTREAM_LAUNCH_STATE: &str = r#"{"sidebar":{"width":300,"expanded":[]},"tabs":{"selected":1,"tabs":[{"type":0,"title":"","controller":null,"controllerUUID":"l1ugzq6orak59osim6nbjg95jk004tbp","renderer":"","controlsHeight":0},{"type":1,"title":"","controller":{"leftSources":[],"rightSources":[],"discreteSources":[],"leftLockedRange":null,"rightLockedRange":null,"leftUnitConversion":{"autoTarget":null,"preset":null},"rightUnitConversion":{"autoTarget":null,"preset":null},"leftFilter":0,"rightFilter":0,"showRobotMode":false},"controllerUUID":"5cjpv5074dzw7uazrsgl98rmb6sefxlk","renderer":null,"controlsHeight":200},{"type":2,"title":"","controller":{"sources":[],"field":"FRC:Evergreen","orientation":0,"size":"large"},"controllerUUID":"wh3wn9bs3s67xj4y4r903ka9m3sipiye","renderer":{"zoom":1,"pan":[0,0],"trackingMode":0,"isFTC":false},"controlsHeight":200},{"type":3,"title":"","controller":{"sources":[],"game":"FRC:Evergreen"},"controllerUUID":"lv7btu9hoa0bkldr8ixndiipykwt8x5z","renderer":null,"controlsHeight":200}]}}"#;

/// `captures/resized.hub-state.json` from `cargo xtask app run hub-resize`: the HubState upstream
/// saved after the scenario's drags, the same at every scale and theme.
const UPSTREAM_RESIZED_STATE: &str = r#"{"sidebar":{"width":362,"expanded":[]},"tabs":{"selected":1,"tabs":[{"type":0,"title":"","controller":null,"controllerUUID":"documentation0000000000000000000","renderer":"","controlsHeight":0},{"type":1,"title":"Resized","controller":{"leftSources":[],"rightSources":[],"discreteSources":[],"leftLockedRange":null,"rightLockedRange":null,"leftUnitConversion":{"autoTarget":null,"preset":null},"rightUnitConversion":{"autoTarget":null,"preset":null},"leftFilter":0,"rightFilter":0,"showRobotMode":false},"controllerUUID":"linegraph00000000000000000000000","renderer":null,"controlsHeight":270}]}}"#;

fn record(path: &Path) -> Json {
    json::parse(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn main() {
    if asrs_parity::ci::skip_in_ci("parity scenario", "a GPU") {
        return;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("parity scenario: skipped, no headless renderer on this platform");
        return;
    }
    let out = std::env::temp_dir().join(format!("asrs-parity-scenario-{}", std::process::id()));
    run("colour-patch", &out);
    run("hub-empty", &out);
    run("hub-resize", &out);

    let colours = patch_colours();
    for scale in [1u32, 2] {
        let variant = out.join(format!("colour-patch/{scale}x-light"));
        let image = pixel::read_png(&variant.join("captures/patches.png")).unwrap();
        assert_eq!((image.width, image.height), (512 * scale, 256 * scale));
        let mut exact = 0;
        for (i, c) in colours.iter().enumerate() {
            let i = i as u32;
            let (x0, y0) = ((i % COLUMNS) * PATCH * scale, (i / COLUMNS) * PATCH * scale);
            let worst = (y0..y0 + PATCH * scale)
                .flat_map(|y| (x0..x0 + PATCH * scale).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let at = ((y * image.width + x) * 4) as usize;
                    (0..3)
                        .map(|k| image.data[at + k].abs_diff(c[k]))
                        .max()
                        .unwrap()
                })
                .max()
                .unwrap();
            assert_eq!(worst, 0, "patch {i} rgb{c:?} at {scale}x");
            exact += 1;
        }
        let patches = record(&variant.join("captures/patches.json"));
        assert_eq!(
            patches
                .get("patches")
                .and_then(Json::as_array)
                .unwrap()
                .len(),
            512
        );
        println!("parity scenario: colour patch at {scale}x {exact}/512 exact");
    }
    for scale in [1u32, 2] {
        for theme in ["light", "dark"] {
            let variant = out.join(format!("hub-empty/{scale}x-{theme}"));
            let image = pixel::read_png(&variant.join("captures/launch.png")).unwrap();
            assert_eq!((image.width, image.height), (1100 * scale, 650 * scale));
            let r = record(&variant.join("captures/launch.json"));
            assert_eq!(
                r.get("scale").and_then(Json::as_f64),
                Some(f64::from(scale))
            );
            assert_eq!(r.get("colourSpace"), Some(&Json::string("srgb")));
            let probed = |selector: &str| {
                let boxes = r.get("probes").and_then(|p| p.get(selector)).unwrap();
                boxes.to_compact()
            };
            let alpha = |x: u32, y: u32| {
                image.data[((y * scale * image.width + x * scale) * 4 + 3) as usize]
            };
            if cfg!(target_os = "linux") {
                assert_eq!(probed(".title-bar"), "[]");
                assert_eq!(probed(".title-bar-menu"), "[[25,1,250,36]]");
                assert_eq!(probed(".side-bar"), "[[0,38,300,612]]");
                for (x, y) in [(150, 300), (10, 10), (700, 300)] {
                    assert_eq!(alpha(x, y), 255, "({x}, {y}): nothing is translucent");
                }
            } else {
                assert_eq!(probed(".title-bar"), "[[0,0,1100,38]]");
                assert_eq!(
                    alpha(150, 300),
                    0,
                    "the vibrancy area is transparent, as upstream's"
                );
                assert_eq!(alpha(10, 10), 0, "the title bar over the sidebar too");
                assert_eq!(alpha(700, 300), 255);
            }

            let state =
                std::fs::read_to_string(variant.join("captures/launch.hub-state.json")).unwrap();
            assert_eq!(state, UPSTREAM_LAUNCH_STATE, "hub-empty {scale}x {theme}");
            let readme = std::fs::read(variant.join("files/userAssets/README.txt")).unwrap();
            assert_eq!(readme, USER_ASSETS_README.as_bytes());

            let variant = out.join(format!("hub-resize/{scale}x-{theme}"));
            let state =
                std::fs::read_to_string(variant.join("captures/resized.hub-state.json")).unwrap();
            assert_eq!(state, UPSTREAM_RESIZED_STATE, "hub-resize {scale}x {theme}");
        }
    }
    println!("parity scenario: hub-empty 1100x650 at 1x and 2200x1300 at 2x");
    println!("parity scenario: hub-empty and hub-resize save upstream's HubState bytes");
    let _ = std::fs::remove_dir_all(&out);
}
