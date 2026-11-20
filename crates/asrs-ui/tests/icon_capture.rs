use std::sync::Arc;

use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::theme::Appearance;
use gpui_kit::{
    AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb, size,
};

const CELL: f32 = 16.0;
const COLUMNS: usize = 24;
/// Chromium blends a filtered translucent image from the unrounded filter output, while a
/// `RenderImage` holds straight-alpha 8-bit colour, so a translucent pixel can land one level
/// away. Opaque pixels, where the filter output itself is visible, must be exact.
const TRANSLUCENT_TOLERANCE: u8 = 1;

struct Probe {
    chain: &'static str,
    input: String,
    opacity: f64,
    expected: [u8; 4],
}

struct Grid {
    background: u32,
    probes: Arc<Vec<Probe>>,
}

impl Render for Grid {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let mut root = div().size_full().relative().bg(rgb(self.background));
        for (i, p) in self.probes.iter().enumerate() {
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"12\" height=\"12\"><rect width=\"12\" height=\"12\" fill=\"{}\" fill-opacity=\"{}\"/></svg>",
                p.input, p.opacity
            );
            root = root.child(
                div()
                    .absolute()
                    .left(px((i % COLUMNS) as f32 * CELL))
                    .top(px((i / COLUMNS) as f32 * CELL))
                    .size(px(12.0))
                    .child(Icon::new(IconSource::Svg(svg.into()), p.chain).fit(IconFit::Natural)),
            );
        }
        root
    }
}

fn probes(scene: &str, scale: f64) -> Vec<Probe> {
    vectors::load("ui/tints-macos")
        .expect("committed ui vectors")
        .into_iter()
        .filter(|r| {
            r.string("scene").as_deref() == Some(scene)
                && r.field("scale").and_then(Json::as_f64) == Some(scale)
                && r.field("chain").is_some()
        })
        .map(|r| {
            let rgba = r.field("rgba").and_then(Json::as_array).unwrap();
            Probe {
                chain: Box::leak(r.string("chain").unwrap().into_boxed_str()),
                input: r.string("input").unwrap(),
                opacity: r.field("opacity").and_then(Json::as_f64).unwrap(),
                expected: [0, 1, 2, 3].map(|i| rgba[i].as_f64().unwrap() as u8),
            }
        })
        .collect()
}

fn check(appearance: Appearance, scale: f32) -> usize {
    let name = appearance.pick("ui-icons-light", "ui-icons-dark");
    let probes = Arc::new(probes(name, f64::from(scale)));
    assert!(!probes.is_empty(), "{name}@{scale}x has probes");
    let background = appearance.theme().background;
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let rows = probes.len().div_ceil(COLUMNS) as f32;
    let shared = probes.clone();
    let window = capture
        .open_window(
            size(px(COLUMNS as f32 * CELL), px(rows * CELL)),
            scale,
            |_, cx| {
                cx.new(|_| Grid {
                    background,
                    probes: shared,
                })
            },
        )
        .expect("open window");
    let shot = capture.capture(window.into()).expect("capture");
    let mut wrong = Vec::new();
    for (i, p) in probes.iter().enumerate() {
        let x = ((i % COLUMNS) as f32 * CELL + 6.0) * scale;
        let y = ((i / COLUMNS) as f32 * CELL + 6.0) * scale;
        let got = shot.pixel(x as u32, y as u32);
        let tolerance = if p.opacity < 1.0 {
            TRANSLUCENT_TOLERANCE
        } else {
            0
        };
        if (0..4).any(|c| got[c].abs_diff(p.expected[c]) > tolerance) {
            wrong.push(format!(
                "{} on {} at {}: {got:?}, Chromium {:?}",
                p.chain, p.input, p.opacity, p.expected
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{name}@{scale}x: {} of {} probes differ:\n{}",
        wrong.len(),
        probes.len(),
        wrong.join("\n")
    );
    probes.len()
}

fn main() {
    let mut total = 0;
    for appearance in [Appearance::Light, Appearance::Dark] {
        for scale in [1.0, 2.0] {
            total += check(appearance, scale);
        }
    }
    println!(
        "icon_capture: {total} probes match Chromium (opaque exact, translucent within one level)"
    );
}
