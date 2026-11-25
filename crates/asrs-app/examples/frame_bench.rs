//! Frame time of the busiest scene of each view in the headless hub window, 2200x1300 device
//! pixels, playing back a real log, every sidebar field expanded. One JSON line per scene:
//! - `p50`, `p95`, `max`: a frame on the foreground thread, from the animation frame through
//!   model, layout, paint and scene building. Background tasks run inside it, because the headless
//!   executor has one thread, so it is an upper bound.
//! - `imageP95`: the same frame, then the 3D render thread's answer, then Metal drawing the scene
//!   and reading the image back.
//! - `pixels`: a hash of the images, which an optimization must not change.
//! - `cpu`: CPU time per frame over the whole process and the first pass.
//!
//! `frame_bench --cache <dir> [--assets <userData dir>] [--log <file>] [--scene <name>]...`

#![allow(clippy::disallowed_methods)]

use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_app::hub::Hub;
use asrs_app::views::frame::HubFrame;
use asrs_app::views::hub_host::{HubEntityHost, boot_messages};
use asrs_hub::hub::HubModel;
use asrs_js::json;
use asrs_js::string::JsString;
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::Log;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::preferences::{Field3dMode, Preferences};
use asrs_model::selection::Selection;
use asrs_services::asset_loader::{AssetFolders, AssetService};
use asrs_services::fs::RealFs;
use asrs_services::path::PathStyle;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{Entity, WindowHandle, px, size};

const FRAME: Duration = Duration::from_millis(16);
const FRAMES: usize = 300;
const IMAGE_FRAMES: usize = 60;
const WARM_FRAMES: usize = 30;
const SCALE: f32 = 2.0;
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const START: f64 = 20.0;
const END: f64 = 140.0;

const LOG: &str = "public-logs/rar1741/akit_25-03-15_15-13-09_inkok_q21.wpilog";

struct SceneSpec {
    name: &'static str,
    tab: u8,
    controller: String,
    renderer: &'static str,
    zooming: bool,
    mode: Field3dMode,
}

fn scene(name: &'static str, tab: u8, controller: String) -> SceneSpec {
    SceneSpec {
        name,
        tab,
        controller,
        renderer: "null",
        zooming: false,
        mode: Field3dMode::Standard,
    }
}

const COLORS: [&str; 6] = [
    "#2b66a2", "#e5b31b", "#af2437", "#80588e", "#e48b32", "#3b875a",
];

fn source(kind: &str, key: &str, log_type: &str, options: &str) -> String {
    format!(
        r#"{{"type":"{kind}","logKey":"{key}","logType":"{log_type}","visible":true,"options":{{{options}}}}}"#
    )
}

fn numbers(kind: &str, keys: &[&str], extra: &str) -> String {
    keys.iter()
        .zip(COLORS.iter().cycle())
        .map(|(key, color)| {
            let options = format!(r#""color":"{color}"{extra}"#);
            source(kind, key, "Number", &options)
        })
        .collect::<Vec<_>>()
        .join(",")
}

const LEFT: [&str; 6] = [
    "/RealOutputs/LoggedRobot/FullCycleMS",
    "/RealOutputs/LoggedRobot/UserCodeMS",
    "/RealOutputs/LoggedRobot/LogPeriodicMS",
    "/RealOutputs/Logger/ConduitSaveMS",
    "/RealOutputs/Logger/DriverStationMS",
    "/SystemStats/CPUTempCelsius",
];
const RIGHT: [&str; 6] = [
    "/SystemStats/BatteryVoltage",
    "/SystemStats/BatteryCurrent",
    "/RealOutputs/SwerveDrive/Modules/FL/Drive/CurrentA",
    "/RealOutputs/SwerveDrive/Modules/FR/Drive/CurrentA",
    "/RealOutputs/SwerveDrive/Modules/BL/Drive/CurrentA",
    "/RealOutputs/SwerveDrive/Modules/BR/Drive/CurrentA",
];
const POSE: &str = "/RealOutputs/Odometry/PoseEstimator/Pose2d";
const STATES: &str = "/RealOutputs/SwerveDrive/CurrentStates";
const DESIRED: &str = "/RealOutputs/SwerveDrive/DesiredStates";
const SPEEDS: &str = "/RealOutputs/SwerveDrive/CurrentChassisSpeeds";
const TAG: &str = "/RealOutputs/Odometry/Limelight/M2AprilTag";
const REEF: &str = "/RealOutputs/ASPoseHelper/Red/Reef/targets";
const MECHANISM: &str = "/Bench/Mechanism";
const CORNERS: &str = "/Bench/Corners";

fn line_graph() -> String {
    let unit = r#"{"autoTarget":null,"preset":null}"#;
    format!(
        r#"{{"leftSources":[{}],"rightSources":[{}],"discreteSources":[{},{}],"leftLockedRange":null,"rightLockedRange":null,"leftUnitConversion":{unit},"rightUnitConversion":{unit},"leftFilter":0,"rightFilter":0,"showRobotMode":true}}"#,
        numbers("stepped", &LEFT, r#","size":"normal""#),
        numbers("smooth", &RIGHT, r#","size":"normal""#),
        source(
            "stripes",
            "/DriverStation/Enabled",
            "Boolean",
            r##""color":"#80588e""##
        ),
        source(
            "stripes",
            "/RealOutputs/Arm/State",
            "String",
            r##""color":"#3b875a""##
        ),
    )
}

fn poses(models: bool) -> String {
    let (robot, ghost) = if models {
        (
            r#""model":"2026 KitBot""#,
            r##""model":"2026 KitBot","color":"#00ff00""##,
        )
    } else {
        (r#""bumpers":"""#, r##""color":"#00ff00""##)
    };
    let modules = r##""color":"#ff0000","arrangement":"0,1,2,3""##;
    let trajectory = r##""color":"#ff8c00","size":"normal""##;
    let vision = r##""color":"#00ff00","size":"normal""##;
    let mut sources = vec![
        source("robot", POSE, "Pose2d", robot),
        source(
            "swerveModuleVelocities",
            STATES,
            "SwerveModuleState[]",
            modules,
        ),
        source(
            "ghost",
            "/RealOutputs/Odometry/Limelight/Pose",
            "Pose2d",
            ghost,
        ),
        source("vision", TAG, "Pose3d", vision),
        source("trajectory", REEF, "Pose2d[]", trajectory),
        source("heatmap", POSE, "Pose2d", r#""timeRange":"full""#),
    ];
    if models {
        sources.push(source("axes", TAG, "Pose3d", ""));
        sources.push(source(
            "cone",
            REEF,
            "Pose2d[]",
            r##""color":"#ff0000","position":"center""##,
        ));
    } else {
        sources.push(source("arrow", REEF, "Pose2d[]", r#""position":"center""#));
    }
    sources.join(",")
}

fn scenes() -> Vec<SceneSpec> {
    let field_3d = format!(r#"{{"sources":[{}],"game":"FRC:2026 Field"}}"#, poses(true));
    let statistics = format!(
        r#"{{"sources":[{}],"timeRange":"visible","rangeMin":0,"rangeMax":100,"stepSize":1}}"#,
        numbers("independent", &[LEFT, RIGHT].concat(), "")
    );
    let table: Vec<String> = [&LEFT[..], &RIGHT[..], &[POSE, STATES]]
        .concat()
        .iter()
        .map(|key| format!(r#""{key}""#))
        .collect();
    let swerve = [
        source(
            "moduleVelocities",
            STATES,
            "SwerveModuleState[]",
            r##""color":"#ff0000","arrangement":"0,1,2,3""##,
        ),
        source(
            "moduleVelocities",
            DESIRED,
            "SwerveModuleState[]",
            r##""color":"#00ffff","arrangement":"0,1,2,3""##,
        ),
        source(
            "robotVelocities",
            SPEEDS,
            "ChassisSpeeds",
            r##""color":"#ff0000""##,
        ),
        source(
            "rotation",
            "/RealOutputs/Odometry/PoseEstimator/Pose2d/rotation",
            "Rotation2d",
            "",
        ),
    ];
    vec![
        scene("line graph", 1, line_graph()),
        SceneSpec {
            zooming: true,
            ..scene("line graph, zooming", 1, line_graph())
        },
        scene(
            "2D field",
            2,
            format!(
                r#"{{"sources":[{}],"field":"FRC:2026 Field","orientation":0,"size":"large"}}"#,
                poses(false)
            ),
        ),
        scene("3D field", 3, field_3d.clone()),
        SceneSpec {
            mode: Field3dMode::Cinematic,
            ..scene("3D field, cinematic", 3, field_3d)
        },
        scene("table", 4, format!("[{}]", table.join(","))),
        SceneSpec {
            renderer: r#"{"highlight":true}"#,
            ..scene("console", 5, r#""/RealOutputs/Console""#.to_owned())
        },
        scene("statistics", 6, statistics.clone()),
        SceneSpec {
            zooming: true,
            ..scene("statistics, zooming", 6, statistics)
        },
        scene(
            "joysticks",
            8,
            r#"["Xbox Controller (Blue)","Generic Joystick","Xbox Controller (Blue)","Generic Joystick","Xbox Controller (Blue)","Generic Joystick"]"#.to_owned(),
        ),
        scene(
            "swerve",
            9,
            format!(
                r#"{{"sources":[{}],"maxSpeed":4.5,"sizeX":0.7,"sizeY":0.5,"orientation":0}}"#,
                swerve.join(",")
            ),
        ),
        scene(
            "mechanism",
            10,
            format!("[{}]", source("mechanism", MECHANISM, "Mechanism2d", "")),
        ),
        scene(
            "points",
            11,
            format!(
                r#"{{"sources":[{}],"width":960,"height":720,"orientation":"xr,yd","origin":"ul"}}"#,
                source(
                    "circle",
                    CORNERS,
                    "Translation2d[]",
                    r#""size":"large","groupSize":"4""#
                )
            ),
        ),
        scene("metadata", 12, "null".to_owned()),
    ]
}

/// What the reference log has nothing of: a 24-segment mechanism and 64 points, both moving.
fn add_bench_fields(log: &mut Log) {
    let key = |name: &str| JsString::from(name);
    let mech = |name: &str| JsString::from(format!("{MECHANISM}/{name}"));
    log.put_string(&mech(".type"), 0.0, key("Mechanism2d"));
    log.put_string(&mech("backgroundColor"), 0.0, key("#000020"));
    log.put_number_array(&mech("dims"), 0.0, vec![4.0, 4.0]);
    log.put_number(&mech("root/x"), 0.0, 2.0);
    log.put_number(&mech("root/y"), 0.0, 0.5);
    let mut line = "root".to_owned();
    let mut angles = Vec::new();
    for i in 0..24 {
        line = format!("{line}/l{i}");
        log.put_string(&mech(&format!("{line}/.type")), 0.0, key("line"));
        log.put_number(&mech(&format!("{line}/length")), 0.0, 0.2);
        log.put_number(&mech(&format!("{line}/weight")), 0.0, 6.0);
        log.put_string(&mech(&format!("{line}/color")), 0.0, key("#ff8000"));
        angles.push(mech(&format!("{line}/angle")));
    }
    for step in 0..3000 {
        let time = START + f64::from(step) * 0.02;
        for (i, angle) in angles.iter().enumerate() {
            let at = f64::from(step) * 0.05 + i as f64;
            log.put_number(angle, time, 10.0 * asrs_js::math::sin(at));
        }
        let corners: Vec<[f64; 2]> = (0..64)
            .map(|i| {
                let at = f64::from(i) * 0.1 + f64::from(step) * 0.02;
                [
                    480.0 + 400.0 * asrs_js::math::cos(at),
                    360.0 + 300.0 * asrs_js::math::sin(at * 1.7),
                ]
            })
            .collect();
        log.put_translation_array(&key(CORNERS), time, &corners);
    }
}

struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: WindowHandle<HubFrame<HubEntityHost>>,
    hub: Entity<Hub>,
}

impl Page<'_> {
    fn main(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub.clone();
        self.headless.context().update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
        self.headless.context().run_until_parked();
    }

    fn model<R>(&mut self, f: impl FnOnce(&mut HubModel) -> R) -> R {
        let hub = self.hub.clone();
        self.headless
            .context()
            .update(|cx| hub.update(cx, |hub, cx| hub.update(cx, |model, _| f(model))))
    }

    fn frame(&mut self) {
        let cx = self.headless.context();
        cx.advance_clock(FRAME);
        cx.update_window(self.window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
        })
        .expect("window");
        cx.run_until_parked();
    }

    fn rendering(&mut self) -> bool {
        self.headless
            .context()
            .update(|cx| self.window.read_with(cx, |frame, _| frame.field3d_busy()))
            .expect("window")
    }

    fn wait_rendered(&mut self) {
        let start = Instant::now();
        while self.rendering() && start.elapsed() < LOAD_TIMEOUT {
            std::thread::yield_now();
            self.headless.context().run_until_parked();
        }
    }

    fn wait_idle(&mut self) {
        let start = Instant::now();
        let mut quiet = 0;
        while quiet < 5 {
            self.frame();
            let loading = self.model(|model| {
                let fields = model.loading_fields();
                fields
                    .iter()
                    .any(|key| !key.to_string_lossy().starts_with("/Bench"))
                    || model.loading().show == Some(true)
                    || model.title().to_string_lossy().contains("(Loading)")
            });
            assert!(
                start.elapsed() < LOAD_TIMEOUT,
                "the page never went idle (loading {loading}, 3D {})",
                self.rendering()
            );
            quiet = if loading || self.rendering() {
                0
            } else {
                quiet + 1
            };
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn image(&mut self) -> Vec<u8> {
        self.headless
            .context()
            .capture_screenshot(self.window.into())
            .expect("image")
            .into_raw()
    }
}

fn ps(field: &str) -> String {
    Command::new("ps")
        .args(["-o", field, "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn cpu_seconds() -> f64 {
    ps("cputime=")
        .split(':')
        .fold(0.0, |sum, part| sum * 60.0 + part.parse().unwrap_or(0.0))
}

fn memory_mb() -> f64 {
    ps("rss=").parse::<f64>().unwrap_or(f64::NAN) / 1024.0
}

fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1]
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

fn run(page: &mut Page<'_>, spec: &SceneSpec, expanded: &str) -> String {
    let preferences = Preferences {
        field3d_mode_ac: spec.mode,
        ..Preferences::default()
    };
    page.main(MainToHub::SetPreferences, preferences.to_js());
    let state = format!(
        r#"{{"sidebar":{{"width":300,"expanded":{expanded}}},"tabs":{{"selected":0,"tabs":[{{"type":{},"title":"","controller":{},"controllerUUID":"tab","renderer":{},"controlsHeight":200}}]}}}}"#,
        spec.tab, spec.controller, spec.renderer
    );
    let state = json::parse_str(&state).unwrap_or_else(|e| panic!("{}: {e:?}", spec.name));
    page.main(MainToHub::RestoreState, state);
    let zooming = spec.zooming;
    let select = |page: &mut Page<'_>| {
        page.model(|model| {
            let selection = model.page_mut().selection_mut();
            selection.set_timeline_range([0.0, END], false);
            selection.set_selected_time(START);
        });
    };
    select(page);
    page.wait_idle();
    page.model(|model| {
        let selection = model.page_mut().selection_mut();
        selection.set_playback_speed(1.0);
        selection.play();
    });
    let mut step = 0.0;
    let mut advance = |page: &mut Page<'_>| {
        if zooming {
            step += 0.1;
            page.model(|model| {
                let range = [step, END - step];
                model
                    .page_mut()
                    .selection_mut()
                    .set_timeline_range(range, false);
            });
        }
    };
    for _ in 0..WARM_FRAMES {
        advance(page);
        page.frame();
        page.wait_rendered();
        page.image();
    }
    let mut times = Vec::with_capacity(FRAMES);
    let cpu = cpu_seconds();
    for _ in 0..FRAMES {
        let start = Instant::now();
        advance(page);
        page.frame();
        times.push(ms(start));
        page.wait_rendered();
    }
    let cpu = (cpu_seconds() - cpu) * 1e3 / FRAMES as f64;
    let mut images = Vec::with_capacity(IMAGE_FRAMES);
    let mut pixels = DefaultHasher::new();
    for _ in 0..IMAGE_FRAMES {
        let start = Instant::now();
        advance(page);
        page.frame();
        page.wait_rendered();
        let image = page.image();
        images.push(ms(start));
        pixels.write(&image);
    }
    format!(
        r#"{{"scene":"{}","p50":{:.3},"p95":{:.3},"max":{:.3},"cpu":{cpu:.3},"imageP95":{:.3},"memoryMb":{:.1},"pixels":"{:016x}"}}"#,
        spec.name,
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        percentile(&times, 1.0),
        percentile(&images, 0.95),
        memory_mb(),
        pixels.finish(),
    )
}

/// `set-assets` data: upstream's bundled assets and those under `<user data>/userAssets`.
fn assets(user_data: &Path) -> JsValue {
    let bundled =
        asrs_parity::vectors::workspace_root().join("reference/AdvantageScope/bundledAssets");
    std::fs::create_dir_all(user_data).expect("user data");
    std::fs::write(user_data.join("prefs.json"), r#"{"userAssetsFolder":null}"#).expect("prefs");
    let mut service = AssetService::new(AssetFolders::new(
        PathStyle::host(),
        user_data.to_str().expect("a UTF-8 path"),
        bundled.to_str().expect("a UTF-8 path"),
    ));
    service.start(&RealFs).expect("assets");
    service.set_assets_data()
}

fn main() {
    let (mut cache, mut user_data, mut log, mut wanted) = (None, None, None, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--cache" => cache = Some(PathBuf::from(value)),
            "--assets" => user_data = Some(PathBuf::from(value)),
            "--log" => log = Some(PathBuf::from(value)),
            "--scene" => wanted.push(value),
            _ => panic!("unknown option {arg}"),
        }
    }
    let log = log.unwrap_or_else(|| cache.expect("--cache or --log").join(LOG));
    let user_data = user_data.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("asrs-frame-bench-{}", std::process::id()))
    });
    let scenes = scenes();
    for name in &wanted {
        assert!(scenes.iter().any(|s| s.name == name), "no scene {name:?}");
    }

    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let files = vec![log.to_str().expect("a UTF-8 path").to_owned()];
    let data = assets(&user_data);
    let window = headless
        .open_window(size(px(1100.0), px(650.0)), SCALE, |window, cx| {
            let preferences = Preferences::default().to_js();
            let boot = boot_messages(data, false, preferences, files);
            HubEntityHost::open_with(window, false, boot, Some(1), cx)
        })
        .expect("hub window");
    let hub = headless
        .context()
        .update(|cx| window.read_with(cx, |frame, _| frame.host().hub().clone()))
        .expect("window");
    let mut page = Page {
        headless: &mut headless,
        window,
        hub,
    };
    page.wait_idle();
    let expanded = page.model(|model| {
        let mut log = model.page().shared_log().borrow_mut();
        add_bench_fields(&mut log);
        let mut keys = std::collections::BTreeSet::new();
        for key in log.get_field_keys() {
            let key = key.to_string_lossy();
            for (i, _) in key.char_indices().skip(1).filter(|(_, c)| *c == '/') {
                keys.insert(key[..i].to_owned());
            }
        }
        let keys: Vec<String> = keys.iter().map(|key| format!(r#""{key}""#)).collect();
        format!("[{}]", keys.join(","))
    });
    for spec in &scenes {
        if wanted.is_empty() || wanted.iter().any(|name| name == spec.name) {
            println!("{}", run(&mut page, spec, &expanded));
        }
    }
}
