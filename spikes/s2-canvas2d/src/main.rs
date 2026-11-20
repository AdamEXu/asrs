mod canvas;
mod ct;
mod diff;
mod scenes;
mod text;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::canvas::{Canvas, parse_font};
use crate::text::{Dilation, GpuiText, SwashText, TextBackend, normalized_typo};

fn electron() -> PathBuf {
    std::env::var_os("S2_ELECTRON").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/oracle/upstream/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron")
    })
}

fn write_inputs(out: &Path) -> Result<()> {
    fs::create_dir_all(out)?;
    fs::write(out.join("scenes.json"), serde_json::to_vec(&scenes::all())?)?;
    let req = json!({ "fonts": scenes::measure_fonts(), "strings": scenes::MEASURE_STRINGS });
    fs::write(out.join("measure-req.json"), serde_json::to_vec(&req)?)?;
    Ok(())
}

fn chromium(out: &Path) -> Result<()> {
    write_inputs(out)?;
    let main_js = Path::new(env!("CARGO_MANIFEST_DIR")).join("electron/main.js");
    let status = Command::new(electron())
        .arg(main_js)
        .arg("--")
        .arg(out.join("scenes.json"))
        .arg(out.join("measure-req.json"))
        .arg(out.join("chromium"))
        .env_remove("ELECTRON_RUN_AS_NODE")
        .status()
        .context("running Electron")?;
    if !status.success() {
        bail!("Electron failed: {status}");
    }
    Ok(())
}

fn backend(name: &str) -> Box<dyn TextBackend> {
    match name {
        "gpui-dilate" => Box::new(GpuiText::new(Dilation::ByColor)),
        "gpui-nodilate" => Box::new(GpuiText::new(Dilation::Off)),
        "gpui-lut" => {
            let mut g = GpuiText::new(Dilation::Off);
            g.lut = Some(LUTS.get().expect("luts calibrated").clone());
            Box::new(g)
        }
        "ct-lut" => {
            let mut c = ct::CoreText::new();
            c.lut = Some(LUTS.get().expect("luts calibrated").clone());
            Box::new(c)
        }
        "ct-raw" => Box::new(ct::CoreText::new()),
        "swash" => Box::new(SwashText::new()),
        "swash-noopsz" => {
            let mut s = SwashText::new();
            s.opsz = false;
            Box::new(s)
        }
        other => panic!("unknown backend {other}"),
    }
}

const VARIANTS: &[&str] = &["ct-lut", "ct-raw", "gpui-lut", "gpui-dilate", "gpui-nodilate", "swash", "swash-noopsz"];

static LUTS: std::sync::OnceLock<Box<[[u8; 256]; 8]>> = std::sync::OnceLock::new();

fn calibrate(scenes: &[scenes::Scene], chrome_dir: &Path) -> Result<Box<[[u8; 256]; 8]>> {
    let mut luts = Box::new([[0u8; 256]; 8]);
    for (k, lut) in luts.iter_mut().enumerate() {
        let scene = scenes.iter().find(|s| s.name == format!("calib-{k}")).unwrap();
        let mut be = GpuiText::new(Dilation::Off);
        let ours = Canvas::run(scene, &mut be);
        let chrome = fs::read(chrome_dir.join(format!("{}.gpu.rgba", scene.name)))?;
        let mut samples: Vec<Vec<u8>> = vec![Vec::new(); 256];
        for (i, p) in ours.pixels().iter().enumerate() {
            samples[p.alpha() as usize].push(chrome[i * 4 + 3]);
        }
        let mut known: Vec<(usize, f64)> = Vec::new();
        for (raw, s) in samples.iter_mut().enumerate() {
            if raw == 0 {
                known.push((0, 0.0));
            } else if s.len() >= 3 {
                s.sort_unstable();
                known.push((raw, s[s.len() / 2] as f64));
            }
        }
        if known.last().unwrap().0 != 255 {
            known.push((255, 255.0));
        }
        for w in known.windows(2) {
            let ((x0, y0), (x1, y1)) = (w[0], w[1]);
            for x in x0..=x1 {
                lut[x] = (y0 + (y1 - y0) * (x - x0) as f64 / (x1 - x0) as f64).round() as u8;
            }
        }
        let n: usize = samples.iter().map(Vec::len).sum::<usize>() - samples[0].len();
        println!("calib-{k}: level {:#04x}, {} inked samples, {} raw levels seen, lut[64]={} lut[128]={} lut[192]={}",
            scenes::LUM_LEVELS[k], n, known.len(), lut[64], lut[128], lut[192]);
    }
    Ok(luts)
}

fn render(out: &Path) -> Result<()> {
    let scenes: Vec<scenes::Scene> = serde_json::from_slice(&fs::read(out.join("scenes.json"))?)?;
    let chrome_dir = out.join("chromium");
    let png_dir = out.join("png");
    fs::create_dir_all(&png_dir)?;
    let mut results = Vec::new();
    let _ = LUTS.set(calibrate(&scenes, &chrome_dir)?);
    for scene in scenes.iter().filter(|s| !s.name.starts_with("calib-")) {
        let w = (scene.width as f64 * scene.dpr) as u32;
        let h = (scene.height as f64 * scene.dpr) as u32;
        for mode in ["gpu", "cpu"] {
            let raw = fs::read(chrome_dir.join(format!("{}.{mode}.rgba", scene.name)))?;
            let img = diff::from_unpremul(w, h, &raw, scene.background);
            diff::save_png(&img, &png_dir.join(format!("{}.chromium-{mode}.png", scene.name)));
        }
        let gpu = fs::read(chrome_dir.join(format!("{}.gpu.rgba", scene.name)))?;
        let cpu = fs::read(chrome_dir.join(format!("{}.cpu.rgba", scene.name)))?;
        let gpu = diff::from_unpremul(w, h, &gpu, scene.background);
        let cpu = diff::from_unpremul(w, h, &cpu, scene.background);
        let gc = diff::compare(&gpu, &cpu, scene.background, None);
        println!("{:<22} chromium gpu vs cpu: {:>6.3}% pm, blob {:>4}, exact-mismatch {}", scene.name, gc.pm_pct, gc.max_blob, gc.exact_mismatch);
        results.push(json!({ "scene": scene.name, "variant": "chromium-cpu", "ref": "chromium-gpu", "stats": gc }));
        for variant in VARIANTS {
            let mut be = backend(variant);
            let t = Instant::now();
            let pixmap = Canvas::run(scene, be.as_mut());
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let ours = diff::from_premul(w, h, pixmap.data(), scene.background);
            diff::save_png(&ours, &png_dir.join(format!("{}.{variant}.png", scene.name)));
            for (mode, reference) in [("gpu", &gpu), ("cpu", &cpu)] {
                let dpath = png_dir.join(format!("{}.{variant}.diff-{mode}.png", scene.name));
                let st = diff::compare(reference, &ours, scene.background, Some(&dpath));
                println!(
                    "{:<22} {:<14} vs {mode}: {:>6.3}% pm ({:>6.2}% of inked), blob {:>5}, max ch {:>3}, mean {:.3}, raster {:.1} ms",
                    scene.name, variant, st.pm_pct, st.pm_pct_of_inked, st.max_blob, st.max_channel, st.mean_abs, ms
                );
                results.push(json!({ "scene": scene.name, "variant": variant, "ref": format!("chromium-{mode}"), "stats": st, "raster_ms": ms }));
            }
        }
    }
    fs::write(out.join("results.json"), serde_json::to_vec_pretty(&results)?)?;
    Ok(())
}

fn measure(out: &Path) -> Result<()> {
    let entries: Vec<Value> = serde_json::from_slice(&fs::read(out.join("chromium/measure.json"))?)?;
    let mut gpui = GpuiText::new(Dilation::Off);
    let mut swash = SwashText::new();
    let mut ctx = ct::CoreText::new();
    let mut rows = Vec::new();
    let mut summary: std::collections::BTreeMap<String, (f64, f64, usize, usize, f64, f64, usize)> = Default::default();
    for e in &entries {
        let font_str = e["font"].as_str().unwrap();
        let text = e["text"].as_str().unwrap();
        let font = parse_font(font_str);
        let cw = e["width"].as_f64().unwrap();
        let gw = gpui.width(&font, text);
        let sw = swash.shape(&font, text).width;
        let ctw = ctx.width(&font, text);
        let (asc, desc) = normalized_typo(font.size);
        let em_ok = match (e["emHeightAscent"].as_f64(), e["emHeightDescent"].as_f64()) {
            (Some(a), Some(d)) => (a - asc).abs() < 1e-9 && (d - desc).abs() < 1e-9,
            _ => true,
        };
        let ceil10 = |w: f64| (w / 10.0).ceil();
        let s = summary.entry(font_str.to_owned()).or_default();
        s.0 = s.0.max((gw - cw).abs());
        s.1 += (gw - cw).abs();
        s.2 += (ceil10(gw) != ceil10(cw)) as usize;
        s.3 += 1;
        s.4 = s.4.max((sw - cw).abs());
        s.5 += (sw - cw).abs();
        s.6 += (!em_ok) as usize;
        rows.push(json!({
            "font": font_str, "text": text, "chromium": cw, "gpui": gw, "swash": sw,
            "gpui_delta": gw - cw, "ct": ctw, "ct_delta": ctw - cw, "swash_delta": sw - cw,
            "em_ascent_chromium": e["emHeightAscent"], "em_ascent_ours": asc,
        }));
    }
    println!("{:<70} {:>10} {:>10} {:>8} {:>10} {:>10} {:>6}", "font", "gpui max", "gpui mean", "ceil10", "swash max", "swash mean", "em!=");
    for (font, s) in &summary {
        println!(
            "{:<70} {:>10.5} {:>10.5} {:>8} {:>10.4} {:>10.4} {:>6}",
            font, s.0, s.1 / s.3 as f64, s.2, s.4, s.5 / s.3 as f64, s.6
        );
    }
    fs::write(out.join("measure-results.json"), serde_json::to_vec_pretty(&rows)?)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("all");
    let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "/Volumes/T9/asrs/target/spikes/s2-out".into()));
    match cmd {
        "gen" => write_inputs(&out),
        "chromium" => chromium(&out),
        "render" => render(&out),
        "measure" => measure(&out),
        "probe" => probe(),
        "all" => {
            chromium(&out)?;
            render(&out)?;
            measure(&out)
        }
        other => bail!("unknown command {other}"),
    }
}

fn probe() -> Result<()> {
    let entries: Vec<Value> = serde_json::from_slice(&fs::read("/Volumes/T9/asrs/target/spikes/s2-out/chromium/measure.json")?)?;
    for w in [180u16, 190, 195, 198, 199, 200, 201, 202, 205, 210, 220] {
        let mut ctx = ct::CoreText::new();
        let (mut maxd, mut sum) = (0.0f64, 0.0f64);
        for e in entries.iter().filter(|e| e["font"].as_str().unwrap().starts_with("200 12px")) {
            let mut font = parse_font(e["font"].as_str().unwrap());
            font.weight = w;
            let d = (ctx.width(&font, e["text"].as_str().unwrap()) - e["width"].as_f64().unwrap()).abs();
            maxd = maxd.max(d);
            sum += d;
        }
        println!("wght {w}: max {maxd:.5} sum {sum:.5}");
    }
    let mut g = GpuiText::new(Dilation::Off);
    let f = crate::text::FontSpec { size: 12.0, weight: 400 };
    let s = g.shape(&f, "0 Hello");
    eprintln!("width {} glyphs {:?}", s.width, s.glyphs.iter().map(|g| (g.id, g.x)).collect::<Vec<_>>());
    for gl in &s.glyphs {
        let m = g.rasterize(&f, gl.id, 2.0, 0, [0.0, 0.0, 0.0, 1.0]);
        eprintln!("{:?}", m.map(|m| (m.left, m.top, m.width, m.height, m.data.iter().map(|&v| v as u32).sum::<u32>())));
    }
    Ok(())
}
