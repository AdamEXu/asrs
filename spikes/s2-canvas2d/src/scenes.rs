use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Deserialize, Clone)]
pub struct Scene {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub dpr: f64,
    pub background: [u8; 3],
    pub cmds: Vec<Value>,
}

struct Rec(Vec<Value>);

impl Rec {
    fn set(&mut self, prop: &str, v: impl Into<Value>) {
        self.0.push(json!(["=", prop, v.into()]));
    }
    fn call(&mut self, name: &str, args: &[Value]) {
        let mut v = vec![json!(name)];
        v.extend_from_slice(args);
        self.0.push(Value::Array(v));
    }
}

macro_rules! c {
    ($r:expr, $name:literal $(, $a:expr)* $(,)?) => {
        $r.call($name, &[$(json!($a)),*])
    };
}

pub const SYSTEM_FONT: &str = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";

pub fn all() -> Vec<Scene> {
    vec![
        line_graph("linegraph-light-2x", 2.0, false),
        line_graph("linegraph-dark-2x", 2.0, true),
        line_graph("linegraph-light-1x", 1.0, false),
        text_grid("text-light-2x", 2.0, false),
        text_grid("text-dark-2x", 2.0, true),
        text_grid("text-light-1x", 1.0, false),
        shapes("shapes-2x", 2.0),
        shapes("shapes-1x", 1.0),
    ]
    .into_iter()
    .chain(LUM_LEVELS.iter().enumerate().map(|(k, v)| calibration(k, *v)))
    .collect()
}

pub const LUM_LEVELS: [u8; 8] = [0x00, 0x24, 0x49, 0x6d, 0x92, 0xb6, 0xdb, 0xff];

fn calibration(k: usize, level: u8) -> Scene {
    let (width, height) = (520.0, 720.0);
    let mut r = Rec(Vec::new());
    c!(r, "scale", 2, 2);
    r.set("fillStyle", format!("#{level:02x}{level:02x}{level:02x}"));
    r.set("textBaseline", "alphabetic");
    let lines = [
        "Sphinx of black quartz, judge my vow: 0123456789 +-.,/%()",
        "Pack my box with five dozen liquor jugs! ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "jackdaws love my big sphinx of quartz [x] {y} <z> #42 @7 &~",
    ];
    let mut y = 16.0;
    for size in [12, 14, 16] {
        r.set("font", format!("{size}px {SYSTEM_FONT}"));
        for (i, line) in lines.iter().enumerate() {
            for q in 0..4 {
                c!(r, "fillText", line, 5.0 + q as f64 * 0.125 + i as f64 * 0.03, y);
                y += size as f64 * 1.4;
            }
        }
    }
    Scene {
        name: format!("calib-{k}"),
        width: width as u32,
        height: height as u32,
        dpr: 2.0,
        background: [0x80, 0x80, 0x80],
        cmds: r.0,
    }
}

fn scale_value(v: f64, from: [f64; 2], to: [f64; 2]) -> f64 {
    ((v - from[0]) / (from[1] - from[0])) * (to[1] - to[0]) + to[0]
}

fn line_graph(name: &str, dpr: f64, dark: bool) -> Scene {
    let (width, height) = (800.0, 400.0);
    let fg = if dark { "#eee" } else { "#222" };
    let mut r = Rec(Vec::new());
    c!(r, "scale", dpr, dpr);
    c!(r, "clearRect", 0, 0, width, height);
    r.set("font", format!("12px {SYSTEM_FONT}"));

    let graph_top = 20.0;
    let graph_height = height - graph_top - 35.0;
    let discrete_rows = 2.0;
    let graph_height_open = graph_height - discrete_rows * 20.0 - 5.0;
    let graph_left = 25.0 + 40.0;
    let graph_right = 25.0 + 30.0;
    let graph_width = width - graph_left - graph_right;
    let time_range = [0.0, 10.0];
    let x_of = |t: f64| scale_value(t, time_range, [graph_left, graph_left + graph_width]);
    let left_range = [-1.5, 1.5];
    let y_of =
        |v: f64| scale_value(v, left_range, [graph_top + graph_height_open, graph_top]);

    r.set("globalAlpha", 1);
    r.set("textAlign", "left");
    r.set("textBaseline", "middle");
    r.set("lineWidth", 1);
    r.set("lineCap", "round");

    let stripes: [(f64, f64, &str, &str); 4] = [
        (0.0, 1.5, "Disabled", "#808080"),
        (1.5, 3.25, "Autonomous", "#2b66a2"),
        (3.25, 9.6, "Teleoperated", "#2b66a2"),
        (9.6, 10.0, "Disabled", "#808080"),
    ];
    let top_y = graph_top + graph_height - 20.0;
    for (i, (s, e, text, color)) in stripes.iter().enumerate() {
        let (sx, ex) = (x_of(*s), x_of(*e));
        let toggle = i % 2 == 0;
        r.set("fillStyle", if toggle { *color } else { "#5a8ec4" });
        c!(r, "fillRect", sx, top_y, ex - sx, 15);
        if ex - sx > 10.0 {
            r.set("fillStyle", if toggle { "#ffffff" } else { "#000000" });
            c!(r, "fillText", text, sx + 5.0, top_y + 15.0 / 2.0, ex - sx - 10.0);
        }
    }
    let top_y2 = graph_top + graph_height - 40.0;
    c!(r, "beginPath");
    let mut toggle = false;
    let edges = [0.0, 0.7, 2.2, 2.9, 5.0, 6.35, 8.8, 10.0];
    for w in edges.windows(2) {
        let (sx, ex) = (x_of(w[0]), x_of(w[1]));
        toggle = !toggle;
        let (sy, ey) = if toggle { (top_y2 + 15.0, top_y2) } else { (top_y2, top_y2 + 15.0) };
        c!(r, "moveTo", sx, sy);
        c!(r, "lineTo", sx, ey);
        c!(r, "lineTo", ex, ey);
        r.set("fillStyle", "#e5b31b");
        c!(r, "fillText", if toggle { "true" } else { "false" }, sx + 5.0, top_y2 + 7.5, ex - sx - 10.0);
    }
    r.set("strokeStyle", "#e5b31b");
    c!(r, "stroke");

    let fields: [(&str, &str, f64); 3] = [("smooth", "#2b66a2", 1.0), ("smooth", "#e48b32", 3.0), ("stepped", "#af2437", 1.0)];
    for (kind, color, lw) in fields {
        r.set("strokeStyle", color);
        r.set("fillStyle", color);
        r.set("lineCap", "round");
        r.set("lineWidth", lw);
        c!(r, "beginPath");
        let n = 240;
        for i in 0..=n {
            let t = i as f64 / n as f64 * 10.0;
            let v = match (color, kind) {
                ("#2b66a2", _) => (t * 1.3).sin() * 1.1,
                ("#e48b32", _) => (t * 0.6).cos() * 0.8 + (t * 4.1).sin() * 0.15,
                _ => ((t * 1.7).floor() % 3.0) * 0.4 - 0.6,
            };
            let (x, y) = (x_of(t), y_of(v));
            if kind == "stepped" && i > 0 {
                let prev_t = (i - 1) as f64 / n as f64 * 10.0;
                let pv = ((prev_t * 1.7).floor() % 3.0) * 0.4 - 0.6;
                c!(r, "lineTo", x, y_of(pv));
            }
            if i == 0 {
                c!(r, "moveTo", x, y);
            } else {
                c!(r, "lineTo", x, y);
            }
        }
        c!(r, "stroke");
    }
    r.set("fillStyle", "#3fa34d");
    c!(r, "beginPath");
    for i in 0..30 {
        let t = 0.2 + i as f64 * 0.33;
        let (x, y) = (x_of(t), y_of(-1.2 + (t * 2.0).sin() * 0.1));
        let radius = 3.2;
        c!(r, "moveTo", x + radius, y);
        c!(r, "arc", x, y, radius, 0, std::f64::consts::PI * 2.0);
    }
    c!(r, "fill");

    let (zs, ze) = (x_of(6.1), x_of(7.4));
    r.set("globalAlpha", 0.25);
    r.set("fillStyle", "yellow");
    c!(r, "fillRect", zs, graph_top, ze - zs, graph_height);
    r.set("globalAlpha", 1);

    r.set("textBaseline", "top");
    r.set("textAlign", "center");
    let mark = |r: &mut Rec, t: f64, alpha: f64| {
        r.set("globalAlpha", alpha);
        r.set("lineWidth", 1);
        c!(r, "setLineDash", [5, 5]);
        r.set("strokeStyle", fg);
        r.set("fillStyle", fg);
        let x = x_of(t);
        c!(r, "beginPath");
        c!(r, "moveTo", x, graph_top);
        c!(r, "lineTo", x, graph_top + graph_height);
        c!(r, "stroke");
        c!(r, "setLineDash", Vec::<f64>::new());
        r.set("globalAlpha", 1);
    };
    mark(&mut r, 4.37, 0.35);
    mark(&mut r, 3.125, 1.0);
    let centered = |r: &mut Rec, text: &str, x: f64, w: f64, alpha: f64| {
        r.set("globalAlpha", alpha);
        r.set("strokeStyle", fg);
        r.set("fillStyle", fg);
        c!(r, "clearRect", x - w / 2.0 - 5.0, graph_top, w + 10.0, 19.0);
        c!(r, "strokeRect", x - w / 2.0 - 5.0, graph_top, w + 10.0, 19.0);
        c!(r, "fillText", text, x, graph_top + 5.0);
        r.set("globalAlpha", 1);
    };
    centered(&mut r, "+3.13s", x_of(3.125), 36.0, 1.0);
    centered(&mut r, "+4.37s", x_of(4.37), 36.0, 0.35);

    r.set("lineWidth", 1);
    r.set("strokeStyle", fg);
    c!(r, "clearRect", 0, 0, width, graph_top);
    c!(r, "clearRect", 0, graph_top + graph_height, width, height - graph_top - graph_height);
    c!(r, "clearRect", 0, graph_top, graph_left, graph_height);
    c!(r, "clearRect", graph_left + graph_width, graph_top, width - graph_left - graph_width, graph_height);
    c!(r, "strokeRect", graph_left, graph_top, graph_width, graph_height);

    r.set("lineWidth", 1);
    r.set("strokeStyle", fg);
    r.set("fillStyle", fg);
    r.set("textBaseline", "middle");
    r.set("textAlign", "right");
    let left_labels = ["1.5", "1", "0.5", "0", "-0.5", "-1", "-1.5"];
    for (i, label) in left_labels.iter().enumerate() {
        let v = 1.5 - i as f64 * 0.5;
        let y = y_of(v);
        r.set("globalAlpha", 1);
        c!(r, "fillText", label, graph_left - 15.0, y);
        c!(r, "beginPath");
        c!(r, "moveTo", graph_left, y);
        c!(r, "lineTo", graph_left - 5.0, y);
        c!(r, "stroke");
        r.set("globalAlpha", 0.1);
        c!(r, "beginPath");
        c!(r, "moveTo", graph_left, y);
        c!(r, "lineTo", graph_left + graph_width, y);
        c!(r, "stroke");
    }
    r.set("textAlign", "left");
    let right_range = [0.0, 12.5];
    for (i, label) in ["12 m/s", "10 m/s", "8 m/s", "6 m/s", "4 m/s", "2 m/s", "0 m/s"].iter().enumerate() {
        let v = 12.0 - i as f64 * 2.0;
        let y = scale_value(v, right_range, [graph_top + graph_height_open, graph_top]);
        r.set("globalAlpha", 1);
        c!(r, "fillText", label, graph_left + graph_width + 15.0, y);
        c!(r, "beginPath");
        c!(r, "moveTo", graph_left + graph_width, y);
        c!(r, "lineTo", graph_left + graph_width + 5.0, y);
        c!(r, "stroke");
    }
    r.set("textAlign", "center");
    for i in 0..=10 {
        let x = x_of(i as f64);
        r.set("globalAlpha", 1);
        c!(r, "fillText", format!("+{i}s"), x, graph_top + graph_height + 15.0);
        c!(r, "beginPath");
        c!(r, "moveTo", x, graph_top + graph_height);
        c!(r, "lineTo", x, graph_top + graph_height + 5.0);
        c!(r, "stroke");
        r.set("globalAlpha", 0.1);
        c!(r, "beginPath");
        c!(r, "moveTo", x, graph_top);
        c!(r, "lineTo", x, graph_top + graph_height);
        c!(r, "stroke");
    }

    Scene {
        name: name.into(),
        width: width as u32,
        height: height as u32,
        dpr,
        background: if dark { [0x22, 0x22, 0x22] } else { [0xff, 0xff, 0xff] },
        cmds: r.0,
    }
}

pub const MEASURE_STRINGS: &[&str] = &[
    "0", "1", "-1", "0.5", "-0.5", "1.5", "-1.5", "12.25", "-123.456", "1000000", "1e-7", "0.000123",
    "+0s", "+1s", "+10s", "+3.13s", "+4.37s", "+123.456s", "\u{0394}+1.24s", "12 m/s", "-3.5 rad/s",
    "Disabled", "Autonomous", "Teleoperated", "true", "false", "Hello, World!", "AVWAY Tij fi",
    "Robot/Drive/LeftVelocity", "NT:/SmartDashboard/Auto Chooser/selected", "The quick brown fox jumps over the lazy dog",
    "WWWWWWWWWW", "iiiiiiiiii", "0123456789", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz",
    "()[]{}<>!?@#$%^&*_+=|\\/:;\"',.~`", "Error: something went wrong", "x", " ", "  two  spaces  ",
];

pub fn measure_fonts() -> Vec<String> {
    vec![
        format!("12px {SYSTEM_FONT}"),
        format!("200 12px {SYSTEM_FONT}"),
        format!("200 14px {SYSTEM_FONT}"),
        format!("16px {SYSTEM_FONT}"),
    ]
}

fn text_grid(name: &str, dpr: f64, dark: bool) -> Scene {
    let (width, height) = (720.0, 460.0);
    let fg = if dark { "#eee" } else { "#222" };
    let mut r = Rec(Vec::new());
    c!(r, "scale", dpr, dpr);
    c!(r, "clearRect", 0, 0, width, height);
    r.set("font", format!("12px {SYSTEM_FONT}"));
    r.set("fillStyle", fg);
    r.set("textAlign", "left");
    let baselines = ["alphabetic", "top", "middle", "bottom"];
    let mut y = 18.0;
    for (i, text) in MEASURE_STRINGS.iter().take(21).enumerate() {
        r.set("textBaseline", baselines[i % 4]);
        let x = 10.0 + (i % 3) as f64 * 0.25 + (i % 7) as f64 * 0.1;
        c!(r, "fillText", text, x, y + (i % 5) as f64 * 0.2);
        y += 20.0;
    }
    r.set("textBaseline", "middle");
    r.set("textAlign", "right");
    let mut y = 18.0;
    for text in MEASURE_STRINGS.iter().skip(21).take(14) {
        c!(r, "fillText", text, 470.0, y);
        y += 20.0;
    }
    r.set("textAlign", "center");
    let colors = ["#ff0000", "#2b66a2", "#e5b31b", "#ffffff", "#000000", "#808080"];
    for (i, color) in colors.iter().enumerate() {
        r.set("fillStyle", *color);
        c!(r, "fillText", "Teleoperated 12.5", 590.0, 18.0 + i as f64 * 20.0);
    }
    r.set("fillStyle", fg);
    r.set("globalAlpha", 0.35);
    c!(r, "fillText", "+4.37s alpha", 590.0, 140.0);
    r.set("globalAlpha", 1);
    r.set("textAlign", "left");
    c!(r, "fillText", "Teleoperated compressed", 500.0, 170.0, 60);
    c!(r, "fillText", "Autonomous", 500.0, 190.0, 40);
    r.set("font", format!("200 12px {SYSTEM_FONT}"));
    c!(r, "fillText", "200 12px +1.5s 0123", 500.0, 220.0);
    r.set("font", format!("200 14px {SYSTEM_FONT}"));
    c!(r, "fillText", "200 14px Drag to zoom", 500.0, 245.0);
    r.set("font", format!("16px {SYSTEM_FONT}"));
    c!(r, "fillText", "16px Regular Wg", 500.0, 275.0);

    Scene {
        name: name.into(),
        width: width as u32,
        height: height as u32,
        dpr,
        background: if dark { [0x22, 0x22, 0x22] } else { [0xff, 0xff, 0xff] },
        cmds: r.0,
    }
}

fn shapes(name: &str, dpr: f64) -> Scene {
    let (width, height) = (600.0, 300.0);
    let mut r = Rec(Vec::new());
    c!(r, "scale", dpr, dpr);
    c!(r, "clearRect", 0, 0, width, height);
    r.set("strokeStyle", "#222");
    for (i, lw) in [1.0, 1.5, 2.0, 3.0, 6.0].iter().enumerate() {
        r.set("lineWidth", *lw);
        for (j, cap) in ["butt", "round", "square"].iter().enumerate() {
            r.set("lineCap", *cap);
            let x = 20.0 + j as f64 * 60.0;
            let y = 20.0 + i as f64 * 22.0 + 0.3 * j as f64;
            c!(r, "beginPath");
            c!(r, "moveTo", x, y);
            c!(r, "lineTo", x + 45.0, y + 7.0);
            c!(r, "stroke");
        }
    }
    r.set("lineWidth", 1);
    r.set("lineCap", "butt");
    for (i, join) in ["miter", "round", "bevel"].iter().enumerate() {
        r.set("lineJoin", *join);
        r.set("lineWidth", 5);
        let x = 220.0 + i as f64 * 60.0;
        c!(r, "beginPath");
        c!(r, "moveTo", x, 70);
        c!(r, "lineTo", x + 20.0, 20);
        c!(r, "lineTo", x + 40.0, 70);
        c!(r, "stroke");
    }
    r.set("lineJoin", "miter");
    r.set("lineWidth", 1);
    c!(r, "setLineDash", [5, 5]);
    c!(r, "beginPath");
    c!(r, "moveTo", 220, 100);
    c!(r, "lineTo", 400, 100);
    c!(r, "moveTo", 220.5, 110.5);
    c!(r, "lineTo", 400.5, 110.5);
    c!(r, "moveTo", 220, 120);
    c!(r, "lineTo", 400, 160);
    c!(r, "stroke");
    c!(r, "setLineDash", Vec::<f64>::new());
    r.set("fillStyle", "#2b66a2");
    c!(r, "fillRect", 420, 20, 50.5, 30.25);
    r.set("globalAlpha", 0.4);
    r.set("fillStyle", "#e48b32");
    c!(r, "fillRect", 440.3, 35.7, 50, 30);
    r.set("globalAlpha", 1);
    c!(r, "clearRect", 450.5, 40.5, 10, 10);
    r.set("strokeStyle", "#af2437");
    r.set("lineWidth", 2);
    c!(r, "strokeRect", 500, 20, 60, 40);
    c!(r, "beginPath");
    c!(r, "arc", 60, 200, 40, 0, std::f64::consts::PI * 2.0);
    r.set("fillStyle", "#3fa34d");
    c!(r, "fill");
    c!(r, "beginPath");
    c!(r, "arc", 160, 200, 40, 0.3, 2.5);
    c!(r, "stroke");
    c!(r, "beginPath");
    c!(r, "arc", 260, 200, 40, 0.3, 2.5, true);
    c!(r, "closePath");
    c!(r, "stroke");
    c!(r, "beginPath");
    c!(r, "moveTo", 330, 170);
    c!(r, "lineTo", 400, 240);
    c!(r, "lineTo", 340, 250);
    c!(r, "closePath");
    r.set("fillStyle", "#808080");
    c!(r, "fill");
    r.set("strokeStyle", "#222");
    r.set("lineWidth", 1);
    c!(r, "stroke");
    c!(r, "beginPath");
    for i in 0..60 {
        let x = 420.0 + i as f64 * 2.5;
        let y = 200.0 + (i as f64 * 0.37).sin() * 40.0;
        if i == 0 {
            c!(r, "moveTo", x, y);
        } else {
            c!(r, "lineTo", x, y);
        }
    }
    r.set("lineWidth", 1);
    r.set("lineCap", "round");
    r.set("lineJoin", "round");
    r.set("strokeStyle", "#2b66a2");
    c!(r, "stroke");

    Scene {
        name: name.into(),
        width: width as u32,
        height: height as u32,
        dpr,
        background: [0xff, 0xff, 0xff],
        cmds: r.0,
    }
}
