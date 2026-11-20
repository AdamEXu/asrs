use std::path::{Path, PathBuf};
use std::sync::Arc;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};
use asrs_ui::button::IconButton;
use asrs_ui::capture::{Capture, HeadlessCapture};
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::symbols::Symbol;
use asrs_ui::theme::Appearance;
use gpui_kit::{
    AnyElement, AppContext, Context, IntoElement, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, PlatformInput, Render, Styled, Window, div, point, px, rgb,
    size,
};

struct Button {
    x: f32,
    y: f32,
    size: f32,
    icon: Symbol,
    state: String,
    fit: IconFit,
}

struct Probe {
    x: f32,
    y: f32,
    size: f32,
    chain: &'static str,
    input: String,
    opacity: f64,
}

struct Scene {
    name: String,
    width: f32,
    height: f32,
    appearance: Appearance,
    background: u32,
    buttons: Vec<Button>,
    probes: Vec<Probe>,
    patches: Vec<(f32, f32, u32)>,
    pointer: Option<(f32, f32, bool)>,
}

fn num(v: &Json, k: &str) -> f64 {
    v.get(k)
        .and_then(Json::as_f64)
        .unwrap_or_else(|| panic!("no number {k}"))
}

fn text(v: &Json, k: &str) -> String {
    v.get(k)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| panic!("no string {k}"))
}

fn list<'a>(v: &'a Json, k: &str) -> &'a [Json] {
    v.get(k).and_then(Json::as_array).unwrap_or_default()
}

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches('#'), 16).expect("hex colour")
}

fn load(path: &Path) -> Scene {
    let j = json::parse(&std::fs::read_to_string(path).expect("scene file")).expect("scene json");
    let bg: Vec<u32> = list(&j, "background")
        .iter()
        .map(|c| c.as_f64().unwrap() as u32)
        .collect();
    let pointer = j
        .get("pointer")
        .filter(|p| !matches!(p, Json::Null))
        .map(|p| {
            (
                num(p, "x") as f32,
                num(p, "y") as f32,
                matches!(p.get("down"), Some(Json::Bool(true))),
            )
        });
    Scene {
        name: text(&j, "name"),
        width: num(&j, "width") as f32,
        height: num(&j, "height") as f32,
        appearance: if matches!(j.get("dark"), Some(Json::Bool(true))) {
            Appearance::Dark
        } else {
            Appearance::Light
        },
        background: (bg[0] << 16) | (bg[1] << 8) | bg[2],
        buttons: list(&j, "buttons")
            .iter()
            .map(|b| Button {
                x: num(b, "x") as f32,
                y: num(b, "y") as f32,
                size: num(b, "size") as f32,
                icon: Symbol::named(&text(b, "icon")).expect("embedded symbol"),
                state: text(b, "state"),
                fit: b.get("fit").map_or(IconFit::BUTTON, |f| IconFit::Centered {
                    max: num(f, "max"),
                    scale: num(f, "scale"),
                }),
            })
            .collect(),
        probes: list(&j, "probes")
            .iter()
            .map(|p| Probe {
                x: num(p, "x") as f32,
                y: num(p, "y") as f32,
                size: num(p, "size") as f32,
                chain: Box::leak(text(p, "chain").into_boxed_str()),
                input: text(p, "input"),
                opacity: num(p, "opacity"),
            })
            .collect(),
        patches: list(&j, "patches")
            .iter()
            .map(|p| {
                (
                    num(p, "x") as f32,
                    num(p, "y") as f32,
                    hex(&text(p, "color")),
                )
            })
            .collect(),
        pointer,
    }
}

fn probe_svg(p: &Probe) -> String {
    let s = p.size;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{s}\" height=\"{s}\"><rect width=\"{s}\" height=\"{s}\" fill=\"{}\" fill-opacity=\"{}\"/></svg>",
        p.input, p.opacity
    )
}

struct View(Arc<Scene>);

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let scene = &self.0;
        let mut root = div().size_full().relative().bg(rgb(scene.background));
        for (i, b) in scene.buttons.iter().enumerate() {
            let button = IconButton::new(("button", i), b.icon)
                .size(b.size, b.size)
                .appearance(scene.appearance)
                .blurred(b.state == "blurred")
                .disabled(b.state == "disabled")
                .fit(b.fit);
            root = root.child(
                div()
                    .absolute()
                    .left(px(b.x))
                    .top(px(b.y))
                    .child(button)
                    .into_any_element(),
            );
        }
        for p in &scene.probes {
            let icon: AnyElement = Icon::new(IconSource::Svg(probe_svg(p).into()), p.chain)
                .fit(IconFit::Natural)
                .into_any_element();
            root = root.child(
                div()
                    .absolute()
                    .left(px(p.x))
                    .top(px(p.y))
                    .size(px(p.size))
                    .child(icon),
            );
        }
        for &(x, y, color) in &scene.patches {
            root = root.child(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .size(px(20.0))
                    .bg(rgb(color)),
            );
        }
        root
    }
}

fn at(shot: &Capture, scale: f32, x: f32, y: f32) -> Json {
    let p = shot.pixel((x * scale).floor() as u32, (y * scale).floor() as u32);
    Json::Array(p.iter().map(|&c| Json::Number(f64::from(c))).collect())
}

fn render(scene: &Arc<Scene>, scale: f32, out: &Path) {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let view = scene.clone();
    let window = capture
        .open_window(size(px(scene.width), px(scene.height)), scale, |_, cx| {
            cx.new(|_| View(view))
        })
        .expect("open window");
    let shot = capture.capture(window.into()).expect("first frame");
    let shot = if let Some((x, y, down)) = scene.pointer {
        let position = point(px(x), px(y));
        capture
            .context()
            .update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                if down {
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                            first_mouse: false,
                        }),
                        cx,
                    );
                }
            })
            .expect("dispatch pointer");
        capture.capture(window.into()).expect("pointer frame")
    } else {
        shot
    };
    let probes = scene
        .probes
        .iter()
        .map(|p| {
            Json::object([
                ("chain", Json::string(p.chain)),
                ("input", Json::string(&p.input)),
                ("opacity", Json::Number(p.opacity)),
                (
                    "rgba",
                    at(&shot, scale, p.x + p.size / 2.0, p.y + p.size / 2.0),
                ),
            ])
        })
        .collect();
    let sampled: &[Button] = if scene.pointer.is_some() {
        &scene.buttons
    } else {
        &[]
    };
    let buttons = sampled
        .iter()
        .map(|b| at(&shot, scale, b.x + b.size / 2.0, b.y + 1.0))
        .collect();
    let base = out.join(format!("{}@{}x", scene.name, scale));
    pixel::write_png(
        &base.with_extension("png"),
        &Rgba {
            width: shot.width,
            height: shot.height,
            data: shot.rgba.clone(),
        },
    )
    .expect("write png");
    let report = Json::object([
        ("probes", Json::Array(probes)),
        ("buttons", Json::Array(buttons)),
    ]);
    std::fs::write(base.with_extension("json"), report.to_pretty() + "\n").expect("write json");
    eprintln!("  {}@{}x", scene.name, scale);
}

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [scenes, out] = args.as_slice() else {
        eprintln!("usage: ui_scenes <scenes dir> <out dir>");
        std::process::exit(2);
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(scenes)
        .expect("scenes dir")
        .map(|e| e.expect("entry").path())
        .collect();
    files.sort();
    for file in files {
        let scene = Arc::new(load(&file));
        for scale in [1.0, 2.0] {
            render(&scene, scale, out);
        }
    }
}
