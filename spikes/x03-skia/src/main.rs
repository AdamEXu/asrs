use std::fs;
use std::path::Path;

use serde_json::Value;
use skia_safe::{
    AlphaType, Color, ColorType, Font, FontHinting, FontMgr, FontStyle, ImageInfo, Paint,
    font::Edging, surfaces,
};

fn font_for(mgr: &FontMgr, css: &str) -> Font {
    let mut size = 10.0;
    let mut weight = 400;
    let mut italic = false;
    for token in css.split_whitespace() {
        if let Some(px) = token.strip_suffix("px") {
            size = px.parse().unwrap();
            break;
        }
        match token {
            "bold" => weight = 700,
            "italic" => italic = true,
            t => {
                if let Ok(w) = t.parse() {
                    weight = w;
                }
            }
        }
    }
    let family = if css.contains("system-ui") { "DejaVu Sans" } else { "Arial" };
    let style = FontStyle::new(
        weight.into(),
        skia_safe::font_style::Width::NORMAL,
        if italic { skia_safe::font_style::Slant::Italic } else { skia_safe::font_style::Slant::Upright },
    );
    let typeface = mgr.match_family_style(family, style).expect("typeface");
    let mut font = Font::from_typeface(typeface, size);
    font.set_hinting(match std::env::var("X03_HINT").as_deref() {
        Ok("none") => FontHinting::None,
        Ok("normal") => FontHinting::Normal,
        _ => FontHinting::Slight,
    });
    font.set_subpixel(true);
    font.set_edging(Edging::AntiAlias);
    font.set_linear_metrics(std::env::var("X03_LINEAR").as_deref() == Ok("1"));
    font
}

fn parse_colour(css: &str) -> Color {
    let h = css.trim_start_matches('#');
    let h = if h.len() == 3 {
        h.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        h.to_owned()
    };
    let v = u32::from_str_radix(&h, 16).unwrap();
    Color::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn render(scene: &Value, out: &Path) {
    let mgr = FontMgr::new();
    let dpr = scene["dpr"].as_f64().unwrap();
    let w = (scene["width"].as_f64().unwrap() * dpr) as i32;
    let h = (scene["height"].as_f64().unwrap() * dpr) as i32;
    let mut surface = surfaces::raster_n32_premul((w, h)).unwrap();
    let canvas = surface.canvas();
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    let mut font = font_for(&mgr, "10px sans-serif");
    let mut align = "start".to_owned();
    let mut baseline = "alphabetic".to_owned();
    for cmd in scene["cmds"].as_array().unwrap() {
        let c = cmd.as_array().unwrap();
        match c[0].as_str().unwrap() {
            "scale" => {
                canvas.scale((c[1].as_f64().unwrap() as f32, c[2].as_f64().unwrap() as f32));
            }
            "=" => match c[1].as_str().unwrap() {
                "font" => font = font_for(&mgr, c[2].as_str().unwrap()),
                "fillStyle" => {
                    paint.set_color(parse_colour(c[2].as_str().unwrap()));
                }
                "textAlign" => align = c[2].as_str().unwrap().to_owned(),
                "textBaseline" => baseline = c[2].as_str().unwrap().to_owned(),
                "globalAlpha" => {
                    paint.set_alpha_f(c[2].as_f64().unwrap() as f32);
                }
                _ => {}
            },
            "clearRect" => {}
            "fillText" => {
                let text = c[1].as_str().unwrap();
                let (mut x, mut y) = (c[2].as_f64().unwrap() as f32, c[3].as_f64().unwrap() as f32);
                let (width, _) = font.measure_str(text, None);
                if c.get(4).is_some() {
                    continue;
                }
                match align.as_str() {
                    "center" => x -= width / 2.0,
                    "right" | "end" => x -= width,
                    _ => {}
                }
                let typeface = font.typeface();
                let os2 = typeface.copy_table_data(u32::from_be_bytes(*b"OS/2")).unwrap();
                let asc = i16::from_be_bytes([os2[68], os2[69]]) as f32;
                let desc = -(i16::from_be_bytes([os2[70], os2[71]]) as f32);
                let size = font.size();
                let lr = |v: f32| (v * 64.0).round() / 64.0;
                let ascent = lr(asc * size / (asc + desc));
                let descent = lr(size) - ascent;
                y += match baseline.as_str() {
                    "top" => ascent,
                    "bottom" => -descent,
                    "middle" => (ascent - descent) / 2.0,
                    _ => 0.0,
                };
                canvas.draw_str(text, (x, y), &font, &paint);
            }
            _ => {}
        }
    }
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    assert!(surface.read_pixels(&info, &mut pixels, (w * 4) as usize, (0, 0)));
    let file = fs::File::create(out).unwrap();
    let mut encoder = png::Encoder::new(file, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenes = Path::new(&args[0]);
    let out = Path::new(&args[1]);
    fs::create_dir_all(out).unwrap();
    for name in &args[2..] {
        let text = fs::read_to_string(scenes.join(format!("{name}.json"))).unwrap();
        let scene: Value = serde_json::from_str(&text).unwrap();
        render(&scene, &out.join(format!("{name}.png")));
        eprintln!("{name}");
    }
}
