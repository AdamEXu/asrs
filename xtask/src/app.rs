use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};
use lcms2::{Intent, PixelFormat, Profile, Transform};

use crate::Result;
use crate::canvas::write;
use crate::gl;
use crate::oracle::{self, Paths};

const USAGE: &str = r"usage: cargo xtask app <command>

The Electron scenario driver (oracle/src/appMain.ts) runs upstream's real app on scenarios from
parity/scenarios/*.json with an isolated userData directory and drives it with executeJavaScript
and sendInputEvent only. Each run directory holds run.json, captures/<name>.png (capturePage)
with captures/<name>.json (probed boxes, text boxes, masks, colour space, timeline range),
per-canvas getImageData dumps (<name>.canvas-<i>.png), WebGL drawing-buffer dumps read right after
the draw with their render mode (<name>.gl-<i>.png), the HubState of the next save-state
(<name>.hub-state.json), files/ (what the app wrote to userData) and inputs/ (generated logs).

Captures are sRGB: Chromium composites with --force-color-profile=srgb. With --colour display it
keeps the display's colour space instead and the driver stores the display's ICC profile as
display.icc beside the captures; `normalize` converts those to sRGB.

  run [<scenario>...] [--scale 1|2] [--theme light|dark] [--gl <mode>] [--colour srgb|display]
      [--label <label>]
                  every variant (scales x themes) of each scenario (default: all), into
                  target/parity/app/<label>/<scenario>/<scale>x-<theme>/ (label default `run`).
                  --gl native|swiftshader|d3d11-warp picks the 3D switches (default ASRS_GL, or
                  native on macOS, swiftshader on Linux and d3d11-warp on Windows)
  tooltips [<scenario>...]
                  (macOS) the tooltips upstream gives AppKit at each capture of the scenarios
                  that have a reference in parity/tooltips, or of the named ones, where they
                  differ from the captures before, into parity/tooltips/<scenario>.json, with the generated logs in
                  parity/tooltips/inputs
  windows         where upstream's Electron puts windows opened without x and y and, on macOS,
                  what its window focus and blur events follow, on windows that are never shown,
                  into parity/windows/<os>.json. On Linux, under `ASRS_RFB_PORT=<port>
                  xtask/hosts/linux/display.sh`, for several screens and work areas
  compare <dir> <dir>
                  two run labels or directories: window captures equal outside the recorded
                  masks; canvas and drawing-buffer dumps, DOM dumps and written files equal
  normalize <dir> every window capture to sRGB through display.icc (lcms2, relative
                  colorimetric, 8 bit) as captures/<name>.srgb.png
  check [--scale 1|2]... [--theme light|dark]... [--gl <mode>]
                  the driver's exit criteria on this host: every scenario run twice and compared,
                  the colour-patch capture within 1 level per channel of its source, and the
                  colour patch with the display profile normalized and reported";

const SCALES: [u32; 2] = [1, 2];
const THEMES: [&str; 2] = ["light", "dark"];
const COLOURS: [&str; 2] = ["srgb", "display"];
const RANDOM_SEED: u32 = 1;

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    match args.split_first() {
        Some((cmd, rest)) if cmd == "run" => {
            let options = RunOptions::parse(rest)?;
            oracle::build_harness(&paths)?;
            run_scenarios(&paths, &options).map(|_| ())
        }
        Some((cmd, rest)) if cmd == "tooltips" => tooltips(&paths, rest),
        Some((cmd, [])) if cmd == "windows" => windows(&paths),
        Some((cmd, [a, b])) if cmd == "compare" => {
            let report = compare(&run_dir(&paths, a), &run_dir(&paths, b))?;
            report.print();
            report.result()
        }
        Some((cmd, [dir])) if cmd == "normalize" => {
            normalize_tree(&run_dir(&paths, dir)).map(|n| eprintln!("normalized {n} captures"))
        }
        Some((cmd, rest)) if cmd == "check" => check(&paths, rest),
        _ => Err(USAGE.to_owned()),
    }
}

fn run_dir(paths: &Paths, arg: &str) -> PathBuf {
    let dir = PathBuf::from(arg);
    if dir.is_dir() {
        dir
    } else {
        paths.root.join("target/parity/app").join(arg)
    }
}

#[derive(Default)]
struct RunOptions {
    scenarios: Vec<String>,
    scales: Vec<u32>,
    themes: Vec<String>,
    gl: Option<String>,
    colour: String,
    label: String,
}

impl RunOptions {
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = RunOptions {
            label: "run".to_owned(),
            colour: "srgb".to_owned(),
            ..Default::default()
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let mut value = |name: &str| {
                iter.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value\n\n{USAGE}"))
            };
            match arg.as_str() {
                "--scale" => options.scales.push(
                    value("--scale")?
                        .parse()
                        .ok()
                        .filter(|s| SCALES.contains(s))
                        .ok_or("--scale is 1 or 2")?,
                ),
                "--theme" => {
                    let theme = value("--theme")?;
                    if !THEMES.contains(&theme.as_str()) {
                        return Err("--theme is light or dark".to_owned());
                    }
                    options.themes.push(theme);
                }
                "--gl" => {
                    let gl = value("--gl")?;
                    options.gl = Some(gl::parse(&gl)?.to_owned());
                }
                "--colour" => {
                    options.colour = value("--colour")?;
                    if !COLOURS.contains(&options.colour.as_str()) {
                        return Err("--colour is srgb or display".to_owned());
                    }
                }
                "--label" => options.label = value("--label")?,
                flag if flag.starts_with("--") => {
                    return Err(format!("unknown option {flag}\n\n{USAGE}"));
                }
                name => options
                    .scenarios
                    .push(name.trim_end_matches(".json").to_owned()),
            }
        }
        Ok(options)
    }
}

/// `oracle/src/robotServer.ts` for one run, stopped when dropped.
pub(crate) struct Robot {
    child: Child,
    pub(crate) port: u16,
}

impl Robot {
    /// `None` for a scenario without a `robot`.
    pub(crate) fn start(paths: &Paths, scenario: &Path) -> Result<Option<Robot>> {
        if read_json(scenario)?.get("robot").is_none() {
            return Ok(None);
        }
        let mut child = oracle::oracle_command_in(paths, "robotServer", oracle::DEFAULT_ZONE)?
            .arg(scenario)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("running robotServer: {e}"))?;
        let mut line = String::new();
        let stdout = child.stdout.take().ok_or("robotServer has no stdout")?;
        let read = BufReader::new(stdout).read_line(&mut line);
        let mut robot = Robot { child, port: 0 };
        read.map_err(|e| format!("robotServer: {e}"))?;
        robot.port = line
            .trim()
            .strip_prefix("{\"port\":")
            .and_then(|rest| rest.strip_suffix('}')?.parse().ok())
            .ok_or_else(|| format!("robotServer printed {line:?}"))?;
        Ok(Some(robot))
    }
}

impl Drop for Robot {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A scenario's RLOG server for one variant: sends its file to the first connection, in one
/// message, and stops.
pub(crate) struct Live {
    pub(crate) port: u16,
}

const LIVE_HOLD: Duration = Duration::from_millis(1500);

impl Live {
    /// `None` for a scenario without `live`.
    pub(crate) fn start(paths: &Paths, scenario: &Path) -> Result<Option<Live>> {
        let scenario = read_json(scenario)?;
        let Some(live) = scenario.get("live") else {
            return Ok(None);
        };
        let file = live
            .get("rlog")
            .and_then(Json::as_str)
            .ok_or("live needs an rlog file")?;
        let file = paths.root.join(file.to_string_lossy());
        let log = fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("live: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = listener.accept() {
                let length = (log.len() as u32).to_be_bytes();
                let _ = socket
                    .write_all(&length)
                    .and_then(|()| socket.write_all(&log));
                std::thread::sleep(LIVE_HOLD);
            }
        });
        Ok(Some(Live { port }))
    }
}

struct Scenario {
    name: String,
    file: PathBuf,
    page: String,
    scales: Vec<u32>,
    themes: Vec<String>,
    gl: bool,
    copies: bool,
    classic_scrollbars: bool,
}

fn scenarios_dir(paths: &Paths) -> PathBuf {
    paths.root.join("parity/scenarios")
}

/// Whether the scenario runs on this platform: `platforms` lists `process.platform`s, all without
/// one.
pub(crate) fn runs_here(scenario: &Json) -> bool {
    let here = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let platforms = scenario.get("platforms").and_then(Json::as_array);
    platforms.is_none_or(|list| {
        list.iter()
            .any(|p| p.as_str().is_some_and(|p| p.to_string() == here))
    })
}

fn load_scenarios(paths: &Paths, only: &[String]) -> Result<Vec<Scenario>> {
    let dir = scenarios_dir(paths);
    let mut names: Vec<String> = fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    if let Some(missing) = only.iter().find(|n| !names.contains(n)) {
        return Err(format!("no scenario {missing} in {}", dir.display()));
    }
    names
        .into_iter()
        .filter(|n| only.is_empty() || only.contains(n))
        .map(|name| {
            let file = dir.join(format!("{name}.json"));
            read_json(&file).map(|value| (name, file, value))
        })
        .filter(|read| !only.is_empty() || !matches!(read, Ok((_, _, value)) if !runs_here(value)))
        .map(|read| {
            let (name, file, value) = read?;
            let numbers = |key: &str, default: &[u32]| -> Vec<u32> {
                value.get(key).and_then(Json::as_array).map_or_else(
                    || default.to_vec(),
                    |a| {
                        a.iter()
                            .filter_map(Json::as_f64)
                            .map(|n| n as u32)
                            .collect()
                    },
                )
            };
            let strings = |key: &str, default: &[&str]| -> Vec<String> {
                value.get(key).and_then(Json::as_array).map_or_else(
                    || default.iter().map(|s| (*s).to_owned()).collect(),
                    |a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(ToString::to_string))
                            .collect()
                    },
                )
            };
            Ok(Scenario {
                page: value
                    .get("page")
                    .and_then(Json::as_str)
                    .map_or_else(|| "app".to_owned(), ToString::to_string),
                scales: numbers("scales", &SCALES),
                themes: strings("themes", &THEMES),
                gl: value.get("gl") == Some(&Json::Bool(true)),
                classic_scrollbars: value
                    .get("scrollbars")
                    .and_then(Json::as_str)
                    .is_some_and(|style| style.to_string() == "classic"),
                copies: value
                    .get("actions")
                    .and_then(Json::as_array)
                    .is_some_and(|actions| actions.iter().any(|a| a.get("copy").is_some())),
                name,
                file,
            })
        })
        .collect()
}

/// Chromium reads these before the main script runs, so they go on the command line; the
/// driver refuses to start when one is missing or an unexpected GL or colour switch is present.
fn electron_switches(scale: u32, gl: Option<&str>, colour: &str) -> Vec<String> {
    let mut switches = vec![
        format!("--force-device-scale-factor={scale}"),
        "--lang=en-US".to_owned(),
        "--disable-accelerated-2d-canvas".to_owned(),
        format!("--js-flags=--random-seed={RANDOM_SEED}"),
        "--force-prefers-reduced-motion".to_owned(),
        "--disable-partial-raster".to_owned(),
    ];
    if colour == "srgb" {
        switches.push("--force-color-profile=srgb".to_owned());
    }
    let gl = gl.map_or(&[][..], gl::electron_switches);
    switches.extend(gl.iter().map(|s| (*s).to_owned()));
    switches
}

fn launch(paths: &Paths, scenario: &Scenario, variant: &Variant<'_>, out: &Path) -> Result {
    let Variant {
        scale,
        theme,
        gl,
        colour,
        tooltips,
    } = variant;
    let gl_switch = scenario.gl.then_some(*gl);
    let robot = Robot::start(paths, &scenario.file)?;
    let live = Live::start(paths, &scenario.file)?;
    let mut command = Command::new(paths.electron()?);
    command
        .args(electron_switches(*scale, gl_switch, colour))
        .arg(paths.dist.join("appMain.mjs"))
        .arg(&scenario.file)
        .arg(out)
        .arg(format!("--scale={scale}"))
        .arg(format!("--theme={theme}"))
        .arg(format!("--gl={gl}"))
        .arg(format!("--colour={colour}"))
        .args(robot.iter().map(|r| format!("--robot={}", r.port)))
        .args(live.iter().map(|l| format!("--live={}", l.port)))
        .args(scrollbar_defaults(scenario.classic_scrollbars))
        .current_dir(&paths.root)
        .env("ASRS_CACHE", &paths.cache)
        .env_remove("ELECTRON_RUN_AS_NODE");
    oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
    if scenario.copies {
        private_pasteboard(paths, &mut command)?;
    }
    if *tooltips {
        tooltip_log(paths, out, &mut command)?;
    }
    let status = command
        .status()
        .map_err(|e| format!("running Electron: {e}"))?;
    let record = out.join("run.json");
    if !record.exists() {
        return Err(format!(
            "{} {scale}x {theme}: the driver wrote no run.json ({status})",
            scenario.name
        ));
    }
    let errors = read_json(&record)?
        .get("errors")
        .and_then(Json::as_array)
        .map_or(0, <[Json]>::len);
    if !status.success() || errors > 0 {
        return Err(format!(
            "{} {scale}x {theme}: driver failed ({status}, {errors} errors in {})",
            scenario.name,
            record.display()
        ));
    }
    Ok(())
}

/// The scrollbar style, whatever the user's preference and devices are.
fn scrollbar_defaults(classic: bool) -> &'static [&'static str] {
    match (cfg!(target_os = "macos"), classic) {
        (false, _) => &[],
        (true, false) => &["-AppleShowScrollBars", "WhenScrolling"],
        (true, true) => &["-AppleShowScrollBars", "Always"],
    }
}

/// Gives Electron a pasteboard of its own (macOS): the user's would hold what scenarios copy.
pub(crate) fn private_pasteboard(paths: &Paths, command: &mut Command) -> Result {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    insert_library(paths, "pasteboard", command)?;
    command.env("ASRS_PRIVATE_PASTEBOARD", "1");
    Ok(())
}

/// Has Electron write the tooltips Chromium gives AppKit.
fn tooltip_log(paths: &Paths, out: &Path, command: &mut Command) -> Result {
    if !cfg!(target_os = "macos") {
        return Err("tooltips are recorded on macOS".to_owned());
    }
    insert_library(paths, "tooltips", command)?;
    let log = out.with_extension("tooltips");
    fs::write(&log, "").map_err(|e| format!("{}: {e}", log.display()))?;
    command.arg("--tooltips").env("ASRS_TOOLTIP_LOG", log);
    Ok(())
}

fn tooltips(paths: &Paths, names: &[String]) -> Result {
    oracle::build_harness(paths)?;
    let references = paths.root.join("parity/tooltips");
    let inputs = references.join("inputs");
    fs::create_dir_all(&inputs).map_err(|e| format!("{}: {e}", inputs.display()))?;
    let mut names = names.to_vec();
    if names.is_empty() {
        for entry in fs::read_dir(&references).map_err(|e| e.to_string())? {
            let name = entry.map_err(|e| e.to_string())?.file_name();
            names.extend(
                name.to_str()
                    .and_then(|n| Some(n.strip_suffix(".json")?.to_owned())),
            );
        }
    }
    for scenario in load_scenarios(paths, &names)? {
        let out = paths
            .root
            .join("target/parity/app/tooltips")
            .join(&scenario.name);
        fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let variant = Variant {
            scale: 1,
            theme: "light",
            gl: gl::default_mode()?,
            colour: "srgb",
            tooltips: true,
        };
        launch(paths, &scenario, &variant, &out)?;
        let mut captures = Vec::new();
        let mut seen = HashSet::new();
        let dir = out.join("captures");
        for action in read_json(&scenario.file)?
            .get("actions")
            .and_then(Json::as_array)
            .unwrap_or_default()
        {
            let Some(name) = action.get("capture").and_then(Json::as_str) else {
                continue;
            };
            let name = name.to_string();
            let file = dir.join(format!("{name}.tooltips.json"));
            if !file.exists() {
                continue;
            }
            let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            if !seen.insert(text.clone()) {
                continue;
            }
            captures.push(format!(
                "{}:{}",
                Json::string(&name).to_compact(),
                text.trim_end()
            ));
        }
        write(
            &references.join(format!("{}.json", scenario.name)),
            format!("{{\n{}\n}}\n", captures.join(",\n")).as_bytes(),
        )?;
        let generated = out.join("inputs");
        if generated.is_dir() {
            for entry in fs::read_dir(&generated).map_err(|e| e.to_string())? {
                let path = entry.map_err(|e| e.to_string())?.path();
                if path.extension().is_some_and(|e| e == "wpilog") {
                    let to = inputs.join(path.file_name().expect("a file"));
                    fs::copy(&path, &to).map_err(|e| format!("{}: {e}", to.display()))?;
                }
            }
        }
    }
    Ok(())
}

/// `BrowserWindow` options without `x` and `y`: the hub's on a first launch, upstream's other kinds
/// of window, and sizes that round or do not fit.
const PLACED: &str = r#"[
{"width":1100,"height":650,"minWidth":800,"minHeight":400,"titleBarStyle":"hiddenInset","vibrancy":"sidebar"},
{"width":1100,"height":650},
{"width":1100,"height":650,"titleBarStyle":"hiddenInset"},
{"width":1100,"height":650,"titleBarStyle":"hiddenInset","resizable":false},
{"width":1101,"height":651,"titleBarStyle":"hiddenInset"},
{"width":801,"height":400,"titleBarStyle":"hiddenInset"},
{"width":500,"height":500,"titleBarStyle":"hiddenInset"},
{"width":1700,"height":1300,"titleBarStyle":"hiddenInset"},
{"width":900,"height":500,"minWidth":200,"minHeight":100,"resizable":true},
{"width":901,"height":501,"resizable":false},
{"width":400,"height":473,"useContentSize":true,"resizable":false,"alwaysOnTop":true},
{"width":1400,"height":930},
{"width":1400,"height":940,"titleBarStyle":"hiddenInset"},
{"width":2400,"height":1500},
{"width":2400,"height":1500,"x":40,"y":50},
{"width":2400,"height":1500,"x":40,"y":50,"titleBarStyle":"hiddenInset"},
{"width":2400,"height":1500,"useContentSize":true}
]"#;

/// Screens as `<width>x<height>+<x>+<y>`, then `_NET_WORKAREA` where there is one.
const SCREENS: [(&[&str], Option<[u32; 4]>); 12] = [
    (&["1920x1080+0+0"], None),
    (&["1920x1080+0+0"], Some([0, 0, 1920, 1040])),
    (&["1920x1080+0+0"], Some([61, 29, 1859, 1051])),
    (&["1920x1080+0+0", "1280x1024+1920+56"], None),
    (
        &["1920x1080+0+0", "1280x1024+1920+56"],
        Some([0, 0, 1920, 1041]),
    ),
    (
        &["1920x1080+0+0", "1280x1024+1920+56"],
        Some([0, 30, 3200, 1050]),
    ),
    (
        &["1920x1080+0+0", "1280x1024+1920+56"],
        Some([1920, 56, 1280, 990]),
    ),
    (
        &["1920x1080+0+0", "1280x1024+1920+56"],
        Some([0, 0, 3000, 1080]),
    ),
    (&["1280x1024+0+700", "1920x1080+1280+0"], None),
    (
        &["1280x1024+0+700", "1920x1080+1280+0"],
        Some([0, 0, 3200, 1650]),
    ),
    (
        &["1280x1024+0+700", "1920x1080+1280+0"],
        Some([1280, 40, 1920, 1040]),
    ),
    (
        &["1000x800+0+0", "1920x1080+0+800", "1280x1024+1920+800"],
        Some([0, 25, 3200, 1855]),
    ),
];

fn windows(paths: &Paths) -> Result {
    let out = paths.root.join("target/parity/app/windows.json");
    let request = out.with_extension("request.json");
    write(
        &request,
        format!("{{\"settle\":300,\"options\":{PLACED}}}").as_bytes(),
    )?;
    let record = |scale: Option<u32>| -> Result<String> {
        let mut command = Command::new(paths.electron()?);
        if let Some(scale) = scale {
            command.args([
                format!("--force-device-scale-factor={scale}"),
                "--ozone-platform=x11".to_owned(),
            ]);
        }
        command
            .arg("--lang=en-US")
            .arg(paths.root.join("oracle/tools/windows.cjs"))
            .args([&request, &out])
            .current_dir(&paths.root)
            .env_remove("ELECTRON_RUN_AS_NODE");
        oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
        if cfg!(target_os = "macos") {
            insert_library(paths, "windowEvents", &mut command)?;
            command.env("ASRS_WINDOW_EVENTS", "1");
        }
        let status = command
            .status()
            .map_err(|e| format!("running Electron: {e}"))?;
        if !status.success() {
            return Err(format!("windows.cjs: {status}"));
        }
        fs::read_to_string(&out).map_err(|e| format!("{}: {e}", out.display()))
    };
    let mut records = Vec::new();
    if cfg!(target_os = "macos") {
        records.push(record(None)?);
    } else {
        let port = std::env::var("ASRS_RFB_PORT").map_err(|_| USAGE.to_owned())?;
        for (screens, work_area) in SCREENS {
            checked(
                Command::new(paths.root.join("xtask/hosts/linux/screens.py"))
                    .arg(&port)
                    .args(screens),
            )?;
            set_work_area(work_area)?;
            for scale in SCALES {
                let area = work_area.map_or("null".to_owned(), |area| format!("{area:?}"));
                records.push(format!(
                    "{{\"netWorkArea\":{area},\"scale\":{scale},{}",
                    record(Some(scale))?.trim_start_matches('{')
                ));
            }
        }
    }
    let version = oracle::ELECTRON_VERSION;
    let text = format!(
        "{{\"electron\":\"{version}\",\"records\":[\n{}\n]}}\n",
        records.join(",\n")
    );
    let file = format!("parity/windows/{}.json", std::env::consts::OS);
    write(&paths.root.join(file), text.as_bytes())
}

fn checked(command: &mut Command) -> Result {
    let status = command
        .status()
        .map_err(|e| format!("running {command:?}: {e}"))?;
    if !status.success() {
        return Err(format!("{command:?}: {status}"));
    }
    Ok(())
}

/// The property a window manager keeps for what panels leave free, set for it.
fn set_work_area(area: Option<[u32; 4]>) -> Result {
    let Some(area) = area else {
        return checked(Command::new("xprop").args(["-root", "-remove", "_NET_WORKAREA"]));
    };
    let value = area.map(|v| v.to_string()).join(", ");
    checked(Command::new("xprop").args([
        "-root",
        "-f",
        "_NET_WORKAREA",
        "32c",
        "-set",
        "_NET_WORKAREA",
        &value,
    ]))
}

/// Builds `oracle/tools/<name>.m` and has the command load it (macOS).
fn insert_library(paths: &Paths, name: &str, command: &mut Command) -> Result {
    let library = paths.root.join(format!("target/oracle/{name}.dylib"));
    checked(
        Command::new("cc")
            .args(["-dynamiclib", "-framework", "AppKit", "-o"])
            .arg(&library)
            .arg(paths.root.join(format!("oracle/tools/{name}.m"))),
    )?;
    command.env("DYLD_INSERT_LIBRARIES", library);
    Ok(())
}

struct Variant<'a> {
    scale: u32,
    theme: &'a str,
    gl: &'a str,
    colour: &'a str,
    tooltips: bool,
}

fn run_scenarios(paths: &Paths, options: &RunOptions) -> Result<PathBuf> {
    let root = paths.root.join("target/parity/app").join(&options.label);
    let gl = match &options.gl {
        Some(gl) => gl,
        None => gl::default_mode()?,
    };
    let mut failures = Vec::new();
    for scenario in load_scenarios(paths, &options.scenarios)? {
        for &scale in &scenario.scales {
            if !options.scales.is_empty() && !options.scales.contains(&scale) {
                continue;
            }
            for theme in &scenario.themes {
                if !options.themes.is_empty() && !options.themes.contains(theme) {
                    continue;
                }
                let out = root.join(&scenario.name).join(format!("{scale}x-{theme}"));
                let variant = Variant {
                    scale,
                    theme,
                    gl,
                    colour: &options.colour,
                    tooltips: false,
                };
                if let Err(e) = launch(paths, &scenario, &variant, &out) {
                    eprintln!("{e}");
                    failures.push(e);
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(root)
    } else {
        Err(format!(
            "{} runs failed:\n{}",
            failures.len(),
            failures.join("\n")
        ))
    }
}

fn read_json(path: &Path) -> Result<Json> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn rects(value: Option<&Json>) -> Vec<[f64; 4]> {
    value
        .and_then(Json::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .filter_map(|m| {
                    let r: Vec<f64> = m
                        .get("rect")
                        .and_then(Json::as_array)?
                        .iter()
                        .filter_map(Json::as_f64)
                        .collect();
                    r.try_into().ok()
                })
                .collect()
        })
}

fn mask_image(width: u32, height: u32, scale: f64, masks: &[[f64; 4]]) -> Vec<bool> {
    let mut masked = vec![false; (width * height) as usize];
    for [x, y, w, h] in masks {
        let clamp = |v: f64, max: u32| (v.max(0.0) as u32).min(max);
        let (x0, y0) = (
            clamp((x * scale).floor(), width),
            clamp((y * scale).floor(), height),
        );
        let (x1, y1) = (
            clamp(((x + w) * scale).ceil(), width),
            clamp(((y + h) * scale).ceil(), height),
        );
        for row in y0..y1 {
            let start = (row * width) as usize;
            masked[start + x0 as usize..start + x1 as usize].fill(true);
        }
    }
    masked
}

fn differing_pixels(a: &Rgba, b: &Rgba, masked: Option<&[bool]>) -> Option<usize> {
    if a.width != b.width || a.height != b.height {
        return None;
    }
    Some(
        a.data
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.data.as_chunks::<4>().0)
            .enumerate()
            .filter(|(i, (p, q))| p != q && !masked.is_some_and(|m| m[*i]))
            .count(),
    )
}

#[derive(Default)]
struct Report {
    compared: Vec<String>,
    failures: Vec<String>,
}

impl Report {
    fn print(&self) {
        for line in &self.compared {
            eprintln!("  {line}");
        }
        for line in &self.failures {
            eprintln!("FAIL {line}");
        }
    }

    fn result(&self) -> Result {
        if self.failures.is_empty() && !self.compared.is_empty() {
            Ok(())
        } else if self.compared.is_empty() {
            Err("nothing compared".to_owned())
        } else {
            Err(format!("{} comparisons failed", self.failures.len()))
        }
    }
}

fn variant_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = Vec::new();
    for scenario in sorted_entries(root)? {
        if !scenario.is_dir() {
            continue;
        }
        for variant in sorted_entries(&scenario)? {
            if variant.join("run.json").exists() {
                dirs.push(variant);
            }
        }
    }
    Ok(dirs)
}

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

fn without_key(value: &Json, key: &str) -> Json {
    match value {
        Json::Object(pairs) => Json::Object(
            pairs
                .iter()
                .filter(|(k, _)| !k.eq_str(key))
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    }
}

fn compare(a_root: &Path, b_root: &Path) -> Result<Report> {
    let mut report = Report::default();
    for a in variant_dirs(a_root)? {
        let rel = a.strip_prefix(a_root).expect("variant under its root");
        let label = rel.display().to_string();
        let b = b_root.join(rel);
        if !b.join("run.json").exists() {
            report
                .failures
                .push(format!("{label}: missing in {}", b_root.display()));
            continue;
        }
        compare_variant(&label, &a, &b, &mut report)?;
    }
    for b in variant_dirs(b_root)? {
        let rel = b.strip_prefix(b_root).expect("variant under its root");
        if !a_root.join(rel).join("run.json").exists() {
            report.failures.push(format!(
                "{}: missing in {}",
                rel.display(),
                a_root.display()
            ));
        }
    }
    Ok(report)
}

/// Capture names are `[a-z0-9-]`; a dotted stem (`<name>.hub-state.json`) is a side file.
fn capture_names(captures: &Path) -> Result<BTreeSet<String>> {
    Ok(sorted_entries(captures)?
        .iter()
        .filter_map(|e| {
            e.file_name()?
                .to_str()?
                .strip_suffix(".json")
                .filter(|stem| !stem.contains('.'))
                .map(str::to_owned)
        })
        .collect())
}

fn record_scale(record: &Json, file: &Path) -> Result<f64> {
    record
        .get("scale")
        .and_then(Json::as_f64)
        .ok_or_else(|| format!("{}: no scale", file.display()))
}

fn compare_variant(label: &str, a: &Path, b: &Path, report: &mut Report) -> Result {
    let captures = a.join("captures");
    let names = capture_names(&captures)?;
    for extra in capture_names(&b.join("captures"))?.difference(&names) {
        report
            .failures
            .push(format!("{label} {extra}: missing in {}", a.display()));
    }
    for name in &names {
        let file = format!("{name}.json");
        let (ja, jb) = (
            read_json(&captures.join(&file))?,
            read_json(&b.join("captures").join(&file))?,
        );
        let record = |j: &Json| without_key(&without_key(j, "timing"), "timelineRange");
        if record(&ja) != record(&jb) {
            report.failures.push(format!(
                "{label} {name}: DOM dump or capture record differs"
            ));
        }
        let range = |j: &Json| {
            j.get("timelineRange")
                .and_then(Json::as_array)
                .and_then(|r| Some([r.first()?.as_f64()?, r.get(1)?.as_f64()?]))
        };
        match (range(&ja), range(&jb)) {
            (None, None) => {}
            (Some(x), Some(y))
                if (x[0] - y[0]).abs().max((x[1] - y[1]).abs())
                    <= crate::pixel::TIMELINE_TOLERANCE * (x[1] - x[0]) =>
            {
                report.compared.push(format!(
                    "{label} {name}: timeline range within {:e} of the span",
                    crate::pixel::TIMELINE_TOLERANCE
                ));
            }
            (x, y) => report.failures.push(format!(
                "{label} {name}: timeline range {x:?} against {y:?}"
            )),
        }
        if let Some(state) = ja.get("hubState").and_then(Json::as_str) {
            let state = state.to_string();
            let read = |dir: &Path| {
                let path = dir.join(&state);
                fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
            };
            if read(&captures)? == read(&b.join("captures"))? {
                report
                    .compared
                    .push(format!("{label} {name}: HubState {state} byte-identical"));
            } else {
                report
                    .failures
                    .push(format!("{label} {name}: HubState {state} differs"));
            }
        }
        let scale = record_scale(&ja, &captures.join(&file))?;
        let image_a = pixel::read_png(&captures.join(format!("{name}.png")))?;
        let image_b = pixel::read_png(&b.join("captures").join(format!("{name}.png")))?;
        let mut masks = rects(ja.get("masks"));
        masks.extend(rects(jb.get("masks")));
        let masked = mask_image(image_a.width, image_a.height, scale, &masks);
        let masked_px = masked.iter().filter(|m| **m).count();
        match differing_pixels(&image_a, &image_b, Some(&masked)) {
            Some(0) => report.compared.push(format!(
                "{label} {name}: window {}x{} equal ({} masks, {masked_px} px masked)",
                image_a.width,
                image_a.height,
                masks.len()
            )),
            Some(n) => report.failures.push(format!(
                "{label} {name}: {n} window pixels differ outside the masks"
            )),
            None => report
                .failures
                .push(format!("{label} {name}: window captures differ in size")),
        }
        for (kind, key) in [("canvas", "canvases"), ("drawing buffer", "gl")] {
            let dumps = ja
                .get(key)
                .and_then(Json::as_array)
                .ok_or_else(|| format!("{label} {name}: capture record has no {key}"))?;
            for dump in dumps {
                let file = dump
                    .get("file")
                    .and_then(Json::as_str)
                    .ok_or_else(|| format!("{label} {name}: {kind} dump without a file"))?
                    .to_string();
                let pa = pixel::read_png(&captures.join(&file))?;
                let pb = pixel::read_png(&b.join("captures").join(&file))?;
                if pa == pb {
                    report.compared.push(format!(
                        "{label} {name}: {kind} {file} {}x{} byte-identical",
                        pa.width, pa.height
                    ));
                } else {
                    report.failures.push(format!(
                        "{label} {name}: {kind} {file} differs ({:?} px)",
                        differing_pixels(&pa, &pb, None)
                    ));
                }
            }
        }
    }
    if names.is_empty() {
        report.failures.push(format!("{label}: no captures"));
    }
    let files = |dir: &Path| -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        let root = dir.join("files");
        if root.exists() {
            collect_files(&root, &root, &mut out)?;
        }
        Ok(out)
    };
    let (fa, fb) = (files(a)?, files(b)?);
    if fa == fb {
        report.compared.push(format!(
            "{label}: {} written files equal ({})",
            fa.len(),
            fa.iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    } else {
        report
            .failures
            .push(format!("{label}: written files differ"));
    }
    Ok(())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result {
    for entry in sorted_entries(dir)? {
        if entry.is_dir() {
            collect_files(root, &entry, out)?;
        } else {
            let rel = entry
                .strip_prefix(root)
                .expect("file under its root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((
                rel,
                fs::read(&entry).map_err(|e| format!("{}: {e}", entry.display()))?,
            ));
        }
    }
    Ok(())
}

/// Converts a capture from the display's colour space to sRGB. Alpha is carried over unchanged.
pub(crate) fn normalize(image: &Rgba, icc: Option<&[u8]>) -> Result<Rgba> {
    let Some(icc) = icc else {
        return Ok(image.clone());
    };
    let input = Profile::new_icc(icc).map_err(|e| format!("display profile: {e}"))?;
    let srgb = Profile::new_srgb();
    let transform: Transform<[u8; 3], [u8; 3]> = Transform::new(
        &input,
        PixelFormat::RGB_8,
        &srgb,
        PixelFormat::RGB_8,
        Intent::RelativeColorimetric,
    )
    .map_err(|e| format!("display profile transform: {e}"))?;
    let pixels = image.data.as_chunks::<4>().0;
    let rgb: Vec<[u8; 3]> = pixels.iter().map(|p| [p[0], p[1], p[2]]).collect();
    let mut converted = vec![[0u8; 3]; rgb.len()];
    transform.transform_pixels(&rgb, &mut converted);
    let data = converted
        .iter()
        .zip(pixels)
        .flat_map(|(c, p)| [c[0], c[1], c[2], p[3]])
        .collect();
    Ok(Rgba {
        width: image.width,
        height: image.height,
        data,
    })
}

fn display_profile(variant: &Path) -> Result<Option<Vec<u8>>> {
    let record = read_json(&variant.join("run.json"))?;
    let source = record
        .get("displayProfile")
        .and_then(|p| p.get("source"))
        .and_then(Json::as_str)
        .map(ToString::to_string);
    match source.as_deref() {
        Some("none" | "forced-srgb") => Ok(None),
        Some(_) => fs::read(variant.join("display.icc"))
            .map(Some)
            .map_err(|e| format!("{}: {e}", variant.display())),
        None => Err(format!(
            "{}: run.json records no display profile",
            variant.display()
        )),
    }
}

fn normalize_variant(variant: &Path) -> Result<Vec<(String, Rgba)>> {
    let icc = display_profile(variant)?;
    let captures = variant.join("captures");
    let mut out = Vec::new();
    for entry in sorted_entries(&captures)? {
        let name = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let Some(name) = name.strip_suffix(".json").filter(|n| !n.contains('.')) else {
            continue;
        };
        let image = pixel::read_png(&captures.join(format!("{name}.png")))?;
        let srgb = normalize(&image, icc.as_deref())?;
        pixel::write_png(&captures.join(format!("{name}.srgb.png")), &srgb)?;
        out.push((name.to_owned(), srgb));
    }
    Ok(out)
}

fn normalize_tree(root: &Path) -> Result<usize> {
    let mut count = 0;
    for variant in variant_dirs(root)? {
        count += normalize_variant(&variant)?.len();
    }
    Ok(count)
}

struct PatchResult {
    patches: usize,
    exact: usize,
    max_delta: u8,
    over: Vec<String>,
}

fn check_patches(variant: &Path) -> Result<PatchResult> {
    let mut result = PatchResult {
        patches: 0,
        exact: 0,
        max_delta: 0,
        over: Vec::new(),
    };
    for (name, image) in normalize_variant(variant)? {
        let file = variant.join("captures").join(format!("{name}.json"));
        let record = read_json(&file)?;
        let scale = record_scale(&record, &file)?;
        let patches = record
            .get("patches")
            .and_then(Json::as_array)
            .ok_or_else(|| format!("{}: no patches", file.display()))?;
        for patch in patches {
            let rgb: [u8; 3] = patch
                .get("rgb")
                .and_then(Json::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Json::as_f64)
                        .map(|v| v as u8)
                        .collect::<Vec<_>>()
                })
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| format!("{}: patch without an rgb triple", file.display()))?;
            let [x, y, w, h] = rects(Some(&Json::Array(vec![patch.clone()])))
                .first()
                .copied()
                .ok_or("patch without a rect")?;
            let mut worst = 0u8;
            for py in (y * scale) as u32..((y + h) * scale) as u32 {
                for px in (x * scale) as u32..((x + w) * scale) as u32 {
                    let at = ((py * image.width + px) * 4) as usize;
                    for c in 0..3 {
                        worst = worst.max(image.data[at + c].abs_diff(rgb[c]));
                    }
                }
            }
            result.patches += 1;
            result.exact += usize::from(worst == 0);
            result.max_delta = result.max_delta.max(worst);
            if worst > 1 {
                result.over.push(format!(
                    "{name} rgb({},{},{}) off by {worst}",
                    rgb[0], rgb[1], rgb[2]
                ));
            }
        }
    }
    Ok(result)
}

fn colour_patch_results(root: &Path, scenario: &Scenario) -> Result<Vec<(String, PatchResult)>> {
    let mut results = Vec::new();
    for variant in sorted_entries(&root.join(&scenario.name))? {
        if variant.join("run.json").exists() {
            let label = variant
                .strip_prefix(root)
                .expect("under root")
                .display()
                .to_string();
            results.push((label, check_patches(&variant)?));
        }
    }
    Ok(results)
}

fn check(paths: &Paths, args: &[String]) -> Result {
    let mut options = RunOptions::parse(args)?;
    if !options.scenarios.is_empty() || options.label != "run" || options.colour != "srgb" {
        return Err(USAGE.to_owned());
    }
    oracle::build_harness(paths)?;
    let scenarios = load_scenarios(paths, &[])?;
    let mut roots = Vec::new();
    for label in ["check-a", "check-b"] {
        options.label = label.to_owned();
        roots.push(run_scenarios(paths, &options)?);
    }
    let mut failures = Vec::new();
    let report = compare(&roots[0], &roots[1])?;
    report.print();
    if let Err(e) = report.result() {
        failures.push(format!("repeat runs: {e}"));
    }
    let app_scenarios = scenarios.iter().filter(|s| s.page == "app").count();
    let gl_scenarios = scenarios.iter().filter(|s| s.gl).count();
    if app_scenarios < 5 || gl_scenarios == 0 {
        failures.push(format!(
            "{app_scenarios} app scenarios and {gl_scenarios} 3D scenarios; the check needs at least 5 and 1"
        ));
    }
    for scenario in scenarios.iter().filter(|s| s.page == "colour-patch") {
        for (label, r) in colour_patch_results(&roots[0], scenario)? {
            eprintln!(
                "  {label}: {}/{} patches exact after normalization, max delta {}",
                r.exact, r.patches, r.max_delta
            );
            if r.patches != 512 || !r.over.is_empty() {
                failures.push(format!(
                    "{label}: {} patches, {} over 1 level: {}",
                    r.patches,
                    r.over.len(),
                    r.over.join("; ")
                ));
            }
        }
        options.label = "check-display".to_owned();
        options.scenarios = vec![scenario.name.clone()];
        options.colour = "display".to_owned();
        let root = run_scenarios(paths, &options)?;
        for (label, r) in colour_patch_results(&root, scenario)? {
            eprintln!(
                "  {label} with the display profile (reported, not gated): {}/{} exact after lcms2 normalization, max delta {}, {} over 1 level",
                r.exact,
                r.patches,
                r.max_delta,
                r.over.len()
            );
        }
    }
    if failures.is_empty() {
        eprintln!("app check: passed");
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
