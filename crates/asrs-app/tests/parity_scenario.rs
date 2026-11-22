#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use asrs_app::scenario::{self, COLUMNS, PATCH, patch_colours};
use asrs_parity::json::{self, Json};
use asrs_parity::pixel;

fn scenario_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../parity/scenarios/{name}.json"))
}

fn run(name: &str, out: &Path) {
    let args: Vec<String> = [
        "--parity-scenario",
        scenario_file(name).to_str().unwrap(),
        "--out",
        out.join(name).to_str().unwrap(),
    ]
    .map(str::to_owned)
    .to_vec();
    scenario::run(&args).unwrap_or_else(|e| panic!("{name}: {e}"));
}

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
        }
    }
    println!("parity scenario: hub-empty 1100x650 at 1x and 2200x1300 at 2x");
    let _ = std::fs::remove_dir_all(&out);
}
