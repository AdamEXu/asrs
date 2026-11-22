use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asrs_ui::capture::{Capture, HeadlessCapture};
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div,
    px, rgb, size,
};
use asrs_ui::theme::ThemeSource;

use super::{Action, COLUMNS, PATCH, Page, Scenario, Theme, patch_colours};
use crate::hub::EmptyHub;

const USAGE: &str = "usage: asrs --parity-scenario <scenario.json> --out <dir> \
[--scale 1|2]... [--theme light|dark]...";

/// `Some(exit code)` when the command line asks for a scenario run.
pub fn run_from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--parity-scenario") {
        return None;
    }
    Some(match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("asrs --parity-scenario: {e}");
            1
        }
    })
}

struct Options {
    scenario: PathBuf,
    out: PathBuf,
    scales: Vec<u32>,
    themes: Vec<String>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let (mut scenario, mut out) = (None, None);
    let (mut scales, mut themes) = (Vec::new(), Vec::new());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--parity-scenario" => scenario = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--scale" => scales.push(value()?.parse().map_err(|_| "--scale is 1 or 2")?),
            "--theme" => themes.push(value()?),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    Ok(Options {
        scenario: scenario.ok_or(USAGE)?,
        out: out.ok_or(USAGE)?,
        scales,
        themes,
    })
}

struct ColourPatches(Vec<[u8; 3]>);

impl Render for ColourPatches {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x000000))
            .children(self.0.iter().enumerate().map(|(i, c)| {
                let i = i as u32;
                div()
                    .absolute()
                    .left(px(((i % COLUMNS) * PATCH) as f32))
                    .top(px(((i / COLUMNS) * PATCH) as f32))
                    .size(px(PATCH as f32))
                    .bg(rgb(u32::from_be_bytes([0, c[0], c[1], c[2]])))
            }))
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let options = options(args)?;
    let name = options
        .scenario
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("scenario file has no name")?
        .to_owned();
    let text = std::fs::read_to_string(&options.scenario)
        .map_err(|e| format!("{}: {e}", options.scenario.display()))?;
    let scenario = Scenario::parse(&name, &text)?;
    let mut headless = HeadlessCapture::new(Arc::new(())).map_err(|e| e.to_string())?;
    for &scale in &scenario.scales {
        if !options.scales.is_empty() && !options.scales.contains(&scale) {
            continue;
        }
        for &theme in &scenario.themes {
            if !options.themes.is_empty() && !options.themes.iter().any(|t| t == theme.name()) {
                continue;
            }
            let dir = options.out.join(format!("{scale}x-{}", theme.name()));
            run_variant(&mut headless, &scenario, scale, theme, &dir)
                .map_err(|e| format!("{name} {scale}x {}: {e}", theme.name()))?;
        }
    }
    Ok(())
}

fn run_variant(
    headless: &mut HeadlessCapture,
    scenario: &Scenario,
    scale: u32,
    theme: Theme,
    dir: &Path,
) -> Result<(), String> {
    let captures = dir.join("captures");
    std::fs::create_dir_all(&captures).map_err(|e| format!("{}: {e}", captures.display()))?;
    headless.context().update(|cx| {
        cx.set_global(match theme {
            Theme::Light => ThemeSource::Light,
            Theme::Dark => ThemeSource::Dark,
        });
    });
    let content = size(px(scenario.width as f32), px(scenario.height as f32));
    let window: AnyWindowHandle = match scenario.page {
        Page::App => headless
            .open_window(content, scale as f32, |_, cx| cx.new(|_| EmptyHub))
            .map(Into::into),
        Page::ColourPatch => headless
            .open_window(content, scale as f32, |_, cx| {
                cx.new(|_| ColourPatches(patch_colours()))
            })
            .map(Into::into),
    }
    .map_err(|e| e.to_string())?;
    for action in &scenario.actions {
        let Action::Capture {
            name,
            probes,
            masks,
        } = action;
        let shot = headless.capture(window).map_err(|e| e.to_string())?;
        let viewport = headless
            .context()
            .update_window(window, |_, window, _| window.viewport_size())
            .map_err(|e| e.to_string())?;
        let expected = (
            (scenario.width * f64::from(scale)) as u32,
            (scenario.height * f64::from(scale)) as u32,
        );
        if (shot.width, shot.height) != expected
            || (
                f64::from(f32::from(viewport.width)),
                f64::from(f32::from(viewport.height)),
            ) != (scenario.width, scenario.height)
        {
            return Err(format!(
                "capture {name}: content is {}x{} px ({:?} logical), expected {}x{}",
                shot.width, shot.height, viewport, expected.0, expected.1
            ));
        }
        if !masks.is_empty() {
            return Err(format!(
                "capture {name}: mask selectors need parity ids (H10b)"
            ));
        }
        write_png(&captures.join(format!("{name}.png")), &shot)?;
        let record = capture_record(name, scenario, scale, theme, &shot, probes);
        write(&captures.join(format!("{name}.json")), &record)?;
    }
    headless
        .context()
        .update_window(window, |_, window, _| window.remove_window())
        .map_err(|e| e.to_string())?;
    write(&dir.join("run.json"), &run_record(scenario, scale, theme))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_png(path: &Path, shot: &Capture) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), shot.width, shot.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&shot.rgba))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn quote(text: &str) -> String {
    let mut out = String::new();
    asrs_js::json::quote_into(&text.encode_utf16().collect::<Vec<_>>(), &mut out);
    out
}

/// The process.platform name the Electron driver records, so both runs pick the same masks.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn run_record(scenario: &Scenario, scale: u32, theme: Theme) -> String {
    format!(
        "{{\n \"scenario\": {},\n \"scale\": {scale},\n \"theme\": {},\n \"window\": {{\"width\": {}, \"height\": {}}},\n \"platform\": {},\n \"arch\": {},\n \"capture\": \"headless\"\n}}\n",
        quote(&scenario.name),
        quote(theme.name()),
        scenario.width,
        scenario.height,
        quote(platform()),
        quote(std::env::consts::ARCH),
    )
}

fn capture_record(
    name: &str,
    scenario: &Scenario,
    scale: u32,
    theme: Theme,
    shot: &Capture,
    probes: &[String],
) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\n \"name\": {},\n \"scale\": {scale},\n \"theme\": {},\n \"width\": {},\n \"height\": {},\n \"image\": {},\n \"colourSpace\": \"srgb\",\n \"devicePixelRatio\": {scale},\n \"viewport\": [{}, {}],\n",
        quote(name),
        quote(theme.name()),
        shot.width,
        shot.height,
        quote(&format!("{name}.png")),
        scenario.width,
        scenario.height,
    );
    let probes: Vec<String> = probes.iter().map(|p| format!("{}: []", quote(p))).collect();
    let _ = writeln!(out, " \"probes\": {{{}}},", probes.join(", "));
    out.push_str(" \"texts\": [],\n \"masks\": [],\n \"canvases\": [],\n \"gl\": []");
    if scenario.page == Page::ColourPatch {
        let patches: Vec<String> = patch_colours()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let i = i as u32;
                format!(
                    "{{\"rgb\": [{}, {}, {}], \"rect\": [{}, {}, {PATCH}, {PATCH}]}}",
                    c[0],
                    c[1],
                    c[2],
                    (i % COLUMNS) * PATCH,
                    (i / COLUMNS) * PATCH
                )
            })
            .collect();
        let _ = write!(out, ",\n \"patches\": [\n  {}\n ]", patches.join(",\n  "));
    }
    out.push_str("\n}\n");
    out
}
