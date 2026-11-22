#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_parity::json::{self, Json};
use asrs_tabs::form;
use asrs_parity::pixel::{self, Rgba};
use asrs_ui::button::IconButton;
use asrs_ui::capture::{Capture, HeadlessCapture};
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::input::{
    Checkbox, FieldKind, FieldModel, FieldStyle, PlainText, Progress, RangeModel, Segment,
    SegmentedControl, Slider, SliderStyle, TextField, number_accepts,
};
use asrs_ui::symbols::Symbol;
use asrs_ui::text::snap_remainder;
use asrs_ui::theme::Appearance;
use gpui_kit::{
    AnyElement, AnyView, AppContext, Context, Corners, IntoElement, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, PlatformInput, Render, Styled, Window, div,
    point, px, rgb, size,
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

struct Control {
    kind: String,
    preset: String,
    value: Option<String>,
    placeholder: Option<String>,
    checked: bool,
    min: Option<String>,
    max: Option<String>,
    segments: Vec<Segment>,
    selected: Option<usize>,
}

struct Scene {
    name: String,
    controls: Vec<Control>,
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

fn opt_text(v: &Json, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
}

fn control(c: &Json) -> Control {
    Control {
        kind: text(c, "kind"),
        preset: text(c, "preset"),
        value: opt_text(c, "value"),
        placeholder: opt_text(c, "placeholder"),
        checked: matches!(c.get("checked"), Some(Json::Bool(true))),
        min: opt_text(c, "min"),
        max: opt_text(c, "max"),
        segments: list(c, "segments")
            .iter()
            .map(|s| match opt_text(s, "icon") {
                Some(icon) => Segment::Icon(IconSource::Symbol(
                    Symbol::named(&icon).expect("embedded symbol"),
                )),
                None => Segment::Badge(text(s, "label").into()),
            })
            .collect(),
        selected: c.get("selected").and_then(Json::as_f64).map(|n| n as usize),
    }
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
        controls: list(&j, "controls").iter().map(control).collect(),
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

/// `div.video-timeline-container { border-radius: 5px; overflow: hidden }` in `hub.css`.
const VIDEO_TIMELINE_RADIUS: f32 = 5.0;

struct Number(form::NumberInput);

impl FieldModel for Number {
    fn value(&self) -> String {
        self.0.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.0.set_value(JsString::from(value));
    }

    fn accepts(&self, c: char) -> bool {
        number_accepts(c)
    }
}

struct Range {
    input: form::RangeInput,
    min: f64,
    max: f64,
}

impl RangeModel for Range {
    fn value(&self) -> String {
        self.input.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.input.set_value(JsString::from(value));
    }

    fn proportion(&self) -> f64 {
        ((self.input.number() - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    fn value_at(&self, proportion: f64) -> String {
        asrs_js::number::to_string(self.min + proportion * (self.max - self.min))
    }
}

fn field<M: FieldModel>(
    model: M,
    kind: FieldKind,
    style: FieldStyle,
    c: &Control,
    rect: [f32; 4],
    scale: f32,
    window: &mut Window,
    cx: &mut Context<'_, View>,
) -> AnyView {
    cx.new(|cx| {
        let mut f = TextField::new(model, kind, style, window, cx);
        f.set_size(rect[2], rect[3]);
        f.set_subpixel(
            snap_remainder(rect[0], scale),
            snap_remainder(rect[1], scale),
        );
        if let Some(p) = &c.placeholder {
            f.set_placeholder(p.clone());
        }
        f
    })
    .into()
}

fn number(c: &Control) -> Number {
    Number(form::NumberInput::new(c.value.as_deref().unwrap_or("")))
}

enum Part {
    View(AnyView),
    Split(usize),
    Progress(f64),
    Checkbox(bool),
}

fn part(
    c: &Control,
    index: usize,
    rect: [f32; 4],
    appearance: Appearance,
    scale: f32,
    window: &mut Window,
    cx: &mut Context<'_, View>,
) -> Part {
    let value = c.value.clone().unwrap_or_default();
    let (model_kind, style) = match c.preset.as_str() {
        "sidebar-search" => (FieldKind::Search, FieldStyle::sidebar_search(appearance)),
        "console-filter" => (FieldKind::Search, FieldStyle::console_filter()),
        "field-value" => (FieldKind::Number, FieldStyle::field_value()),
        "setting-block" => (FieldKind::Number, FieldStyle::setting_block()),
        "table-jump" => (FieldKind::Number, FieldStyle::native()),
        "prompt" if c.kind == "number" => (FieldKind::Number, FieldStyle::native()),
        "prompt" => (FieldKind::Text, FieldStyle::native()),
        "video-timeline" => {
            let (min, max) = (
                c.min.clone().unwrap_or_default(),
                c.max.clone().unwrap_or_default(),
            );
            let mut input = form::RangeInput::new(&value, &min, &max);
            input.set_value(JsString::from(value.as_str()));
            let model = Range {
                input,
                min: min.parse().expect("min"),
                max: max.parse().expect("max"),
            };
            return Part::View(
                cx.new(|_| {
                    let mut slider = Slider::new(model, SliderStyle::VIDEO_TIMELINE);
                    slider.set_corner_radii(Corners::all(px(VIDEO_TIMELINE_RADIUS)));
                    slider
                })
                .into(),
            );
        }
        "split" => return Part::Split(index),
        "download" => {
            let v: f64 = value.parse().expect("progress value");
            let max: f64 = c.max.as_deref().unwrap_or("1").parse().expect("progress max");
            return Part::Progress(v / max);
        }
        "beta" => return Part::Checkbox(c.checked),
        other => panic!("unknown control preset {other}"),
    };
    Part::View(match model_kind {
        FieldKind::Number => field(number(c), model_kind, style, c, rect, scale, window, cx),
        _ => field(
            PlainText::new(&value),
            model_kind,
            style,
            c,
            rect,
            scale,
            window,
            cx,
        ),
    })
}

struct View {
    scene: Arc<Scene>,
    parts: Vec<([f32; 4], Part)>,
}

impl View {
    fn new(
        scene: Arc<Scene>,
        rects: Vec<[f32; 4]>,
        window: &mut Window,
        cx: &mut Context<'_, View>,
    ) -> View {
        let scale = window.scale_factor();
        let parts = scene
            .controls
            .iter()
            .zip(rects)
            .enumerate()
            .map(|(i, (c, rect))| (rect, part(c, i, rect, scene.appearance, scale, window, cx)))
            .collect();
        View { scene, parts }
    }
}

impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let scene = self.scene.clone();
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
        for (rect, part) in &self.parts {
            let element: AnyElement = match part {
                Part::View(view) => view.clone().into_any_element(),
                Part::Split(i) => {
                    let c = &scene.controls[*i];
                    SegmentedControl::new(("split", *i), c.segments.clone())
                        .selected(c.selected)
                        .size(rect[2], rect[3])
                        .appearance(scene.appearance)
                        .into_any_element()
                }
                Part::Progress(p) => Progress::new(*p).into_any_element(),
                Part::Checkbox(checked) => Checkbox::new("checkbox", *checked).into_any_element(),
            };
            root = root.child(
                div()
                    .absolute()
                    .left(px(rect[0]))
                    .top(px(rect[1]))
                    .w(px(rect[2]))
                    .h(px(rect[3]))
                    .child(element),
            );
        }
        root
    }
}

fn at(shot: &Capture, scale: f32, x: f32, y: f32) -> Json {
    let p = shot.pixel((x * scale).floor() as u32, (y * scale).floor() as u32);
    Json::Array(p.iter().map(|&c| Json::Number(f64::from(c))).collect())
}

/// The border boxes Blink gave the scene's controls, which the port takes as its layout.
fn control_rects(reference: &Path, scene: &Scene, scale: f32) -> Vec<[f32; 4]> {
    if scene.controls.is_empty() {
        return Vec::new();
    }
    let file = reference.join(format!("{}@{}x.json", scene.name, scale));
    let j = json::parse(&std::fs::read_to_string(&file).expect("reference capture"))
        .expect("reference json");
    list(&j, "controls")
        .iter()
        .map(|r| {
            let v: Vec<f32> = r
                .as_array()
                .unwrap_or_default()
                .iter()
                .map(|n| n.as_f64().expect("rect") as f32)
                .collect();
            [v[0], v[1], v[2], v[3]]
        })
        .collect()
}

fn render(scene: &Arc<Scene>, scale: f32, reference: &Path, out: &Path) {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let view = scene.clone();
    let rects = control_rects(reference, scene, scale);
    let window = capture
        .open_window(size(px(scene.width), px(scene.height)), scale, |window, cx| {
            cx.new(|cx| View::new(view, rects, window, cx))
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
            render(&scene, scale, &scenes.with_file_name("ref"), out);
        }
    }
}
