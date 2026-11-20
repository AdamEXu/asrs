#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::text::probe::Probe;
use asrs_ui::text::{
    Align, Direction, FontSpec, LineHeight, Overflow, Text, TextStyle, WhiteSpace, snap_remainder,
};
use gpui_kit::{
    AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb, size,
};

struct TextBox {
    id: String,
    x: f32,
    y: f32,
    width: Option<f32>,
    height: Option<f32>,
    style: TextStyle,
    color: u32,
    opacity: f32,
    text: String,
}

struct Scene {
    name: String,
    width: f32,
    height: f32,
    background: u32,
    boxes: Vec<TextBox>,
    patches: Vec<(f32, f32, u32)>,
}

fn num(v: &Json, k: &str) -> f64 {
    v.get(k)
        .and_then(Json::as_f64)
        .unwrap_or_else(|| panic!("no number {k}"))
}

fn opt(v: &Json, k: &str) -> Option<f32> {
    v.get(k).and_then(Json::as_f64).map(|n| n as f32)
}

fn text(v: &Json, k: &str) -> String {
    v.get(k)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| panic!("no string {k}"))
}

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches('#'), 16).expect("hex colour")
}

fn style(b: &Json) -> TextStyle {
    let mut s = TextStyle::new(FontSpec::parse(&text(b, "font")).expect("font"));
    let lh = text(b, "lineHeight");
    s.line_height = match lh.as_str() {
        "normal" => LineHeight::Normal,
        v => match v.strip_suffix("px") {
            Some(px) => LineHeight::Px(px.parse().expect("line-height px")),
            None => LineHeight::Number(v.parse().expect("line-height number")),
        },
    };
    s.align = match text(b, "align").as_str() {
        "left" => Align::Left,
        "right" => Align::Right,
        "center" => Align::Center,
        "end" => Align::End,
        _ => Align::Start,
    };
    s.direction = match text(b, "direction").as_str() {
        "rtl" => Direction::Rtl,
        _ => Direction::Ltr,
    };
    s.white_space = match text(b, "whiteSpace").as_str() {
        "pre" => WhiteSpace::Pre,
        "nowrap" => WhiteSpace::NoWrap,
        _ => WhiteSpace::Normal,
    };
    s.overflow = match text(b, "overflow").as_str() {
        "clip" => Overflow::Clip,
        "ellipsis" => Overflow::Ellipsis,
        _ => Overflow::Visible,
    };
    s
}

fn load(path: &Path) -> Scene {
    let j = json::parse(&std::fs::read_to_string(path).expect("scene file")).expect("scene json");
    let list = |k: &str| j.get(k).and_then(Json::as_array).unwrap_or_default();
    let bg: Vec<u32> = list("background")
        .iter()
        .map(|c| c.as_f64().unwrap() as u32)
        .collect();
    Scene {
        name: text(&j, "name"),
        width: num(&j, "width") as f32,
        height: num(&j, "height") as f32,
        background: (bg[0] << 16) | (bg[1] << 8) | bg[2],
        boxes: list("boxes")
            .iter()
            .map(|b| TextBox {
                id: text(b, "id"),
                x: num(b, "x") as f32,
                y: num(b, "y") as f32,
                width: opt(b, "width"),
                height: opt(b, "height"),
                style: style(b),
                color: hex(&text(b, "color")),
                opacity: num(b, "opacity") as f32,
                text: text(b, "text"),
            })
            .collect(),
        patches: list("patches")
            .iter()
            .map(|p| {
                (
                    num(p, "x") as f32,
                    num(p, "y") as f32,
                    hex(&text(p, "color")),
                )
            })
            .collect(),
    }
}

struct View(Arc<Scene>);

impl Render for View {
    fn render(&mut self, window: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let scene = &self.0;
        let mut root = div().size_full().relative().bg(rgb(scene.background));
        for b in &scene.boxes {
            let mut t = Text::new(b.text.clone(), b.style)
                .color(b.color)
                .probe(b.id.clone())
                .subpixel(snap_remainder(b.x, scale), snap_remainder(b.y, scale));
            if let Some(w) = b.width {
                t = t.w(px(w));
            }
            if let Some(h) = b.height {
                t = t.h(px(h));
            }
            root = root.child(
                div()
                    .absolute()
                    .left(px(b.x))
                    .top(px(b.y))
                    .opacity(b.opacity)
                    .child(t),
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

fn render(scene: &Arc<Scene>, scale: f32, out: &Path) {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    capture
        .context()
        .update(|cx| cx.set_global(Probe::default()));
    let view = scene.clone();
    let window = capture
        .open_window(size(px(scene.width), px(scene.height)), scale, |_, cx| {
            cx.new(|_| View(view))
        })
        .expect("open window");
    capture
        .context()
        .update(|cx| cx.global_mut::<Probe>().0.clear());
    let shot = capture.capture(window.into()).expect("capture");
    let records = capture
        .context()
        .update(|cx| std::mem::take(&mut cx.global_mut::<Probe>().0));
    let mut boxes = Vec::new();
    for r in records.iter().rev() {
        if boxes.iter().any(|b: &Json| {
            b.get("id")
                .and_then(Json::as_str)
                .is_some_and(|s| s.to_string_lossy() == r.id.as_ref())
        }) {
            continue;
        }
        let nums = |v: &[f64]| Json::Array(v.iter().map(|&n| Json::Number(n)).collect());
        boxes.push(Json::object([
            ("id", Json::string(r.id.as_ref())),
            ("bounds", nums(&r.bounds.map(f64::from))),
            ("width", Json::Number(r.width)),
            ("height", Json::Number(r.height)),
            (
                "fragments",
                Json::Array(
                    r.fragments
                        .iter()
                        .map(|(t, rect)| {
                            Json::object([("text", Json::string(t)), ("rect", nums(rect))])
                        })
                        .collect(),
                ),
            ),
        ]));
    }
    let base = out.join(format!("{}@{}x", scene.name, scale));
    pixel::write_png(
        &base.with_extension("png"),
        &Rgba {
            width: shot.width,
            height: shot.height,
            data: shot.rgba,
        },
    )
    .expect("write png");
    let report = Json::object([("boxes", Json::Array(boxes))]);
    std::fs::write(base.with_extension("json"), report.to_pretty() + "\n").expect("write json");
    eprintln!("  {}@{}x", scene.name, scale);
}

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [scenes, out] = args.as_slice() else {
        eprintln!("usage: text_scenes <scenes dir> <out dir>");
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
