mod render;
mod scene;
mod ssim;
mod view;

use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::{DQuat, DVec3};

use render::{Camera, Renderer};
use scene::SceneConfig;

pub const OUT: &str = "/Volumes/T9/asrs/target/spikes/s3";

pub fn scene_config() -> SceneConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../reference/AdvantageScope");
    SceneConfig {
        robot_path: root.join("bundledAssets/Robot_FRCKitBot/model.glb"),
        robot_translation: DVec3::new(2.0, 1.0, 0.0),
        robot_rotation: DQuat::from_xyzw(0.0, 0.0, (std::f64::consts::PI / 12.0).sin(), (std::f64::consts::PI / 12.0).cos()),
    }
}

pub fn cameras() -> [(&'static str, DVec3, DVec3); 2] {
    [
        ("field", DVec3::new(0.0, 6.0, -12.0), DVec3::new(0.0, 0.5, 0.0)),
        ("robot", DVec3::new(-0.6, 1.1, -0.6), DVec3::new(-2.0, 0.2, 1.0)),
    ]
}

fn bgra_to_rgba(v: &mut [u8]) {
    for p in v.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
}

fn reference(samples: u32) -> anyhow::Result<()> {
    let (scene, stats) = scene::build_standard(&scene_config())?;
    println!(
        "robot: primitives={} kept={} normal_vertices={} transparent_vertices={}; scene items={}",
        stats.primitives,
        stats.kept,
        stats.normal_vertices,
        stats.transparent_vertices,
        scene.items.len()
    );
    let mut r = Renderer::new(samples)?;
    println!("adapter: {:?} {:?}", r.adapter_info.name, r.adapter_info.backend);
    r.upload(&scene);
    let mut buf = Vec::new();
    for (name, position, target) in cameras() {
        for dark in [true, false] {
            let theme = if dark { "dark" } else { "light" };
            let cam = Camera {
                position,
                target,
                fov: 50.0,
                dark,
            };
            r.render(&scene, &cam, 2200, 1300, &mut buf);
            let mut rgba = buf.clone();
            bgra_to_rgba(&mut rgba);
            let ours = Path::new(OUT).join(format!("port_{name}_{theme}_msaa{samples}.png"));
            image::save_buffer(&ours, &rgba, 2200, 1300, image::ColorType::Rgba8)?;
            let up = Path::new(OUT).join(format!("upstream_{name}_{theme}.png"));
            match image::open(&up) {
                Ok(img) => {
                    let up = img.to_rgba8();
                    let s = ssim::ssim(&rgba, up.as_raw(), 2200, 1300);
                    let d = ssim::diff(&rgba, up.as_raw());
                    println!(
                        "{name}/{theme} msaa{samples}: ssim={s:.4} mean_abs={:.3} max_abs={} exact={:.2}% within2={:.2}%",
                        d.mean_abs,
                        d.max_abs,
                        d.exact_frac * 100.0,
                        d.within2_frac * 100.0
                    );
                    let mut heat = vec![0u8; rgba.len()];
                    for i in 0..rgba.len() / 4 {
                        let m = (0..3).map(|c| rgba[i * 4 + c].abs_diff(up.as_raw()[i * 4 + c])).max().unwrap();
                        let v = (m as u32 * 8).min(255) as u8;
                        heat[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, 255]);
                    }
                    image::save_buffer(
                        Path::new(OUT).join(format!("diff_{name}_{theme}_msaa{samples}.png")),
                        &heat,
                        2200,
                        1300,
                        image::ColorType::Rgba8,
                    )?;
                }
                Err(e) => println!("{name}/{theme}: no upstream image ({e})"),
            }
        }
    }
    Ok(())
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

pub fn summary(name: &str, v: &[f64]) -> String {
    let mut s = v.to_vec();
    let mean = s.iter().sum::<f64>() / s.len().max(1) as f64;
    format!(
        "{name}: n={} mean={mean:.3}ms p50={:.3} p95={:.3} p99={:.3} max={:.3}",
        s.len(),
        pct(&mut s, 0.5),
        pct(&mut s, 0.95),
        pct(&mut s, 0.99),
        pct(&mut s, 1.0)
    )
}

fn bench(w: u32, h: u32, n: usize, samples: u32) -> anyhow::Result<()> {
    let t = Instant::now();
    let (scene, _) = scene::build_standard(&scene_config())?;
    let build = t.elapsed();
    let mut r = Renderer::new(samples)?;
    r.upload(&scene);
    println!("scene build+glb load {:.1} ms, adapter {:?}", build.as_secs_f64() * 1e3, r.adapter_info.name);
    let mut buf = Vec::new();
    let (mut enc, mut gpu, mut copy, mut total) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..n {
        let a = i as f64 * 0.01;
        let cam = Camera {
            position: DVec3::new(12.37 * a.sin(), 6.0, -12.37 * a.cos()),
            target: DVec3::new(0.0, 0.5, 0.0),
            fov: 50.0,
            dark: true,
        };
        let t = Instant::now();
        let tm = r.render(&scene, &cam, w, h, &mut buf);
        if i >= 10 {
            total.push(t.elapsed().as_secs_f64() * 1e3);
            enc.push(tm.encode_ms);
            gpu.push(tm.gpu_and_map_ms);
            copy.push(tm.copy_ms);
        }
    }
    println!("bench {w}x{h} msaa{samples}, {n} frames");
    println!("{}", summary("encode+submit", &enc));
    println!("{}", summary("gpu+map wait", &gpu));
    println!("{}", summary("row copy", &copy));
    println!("{}", summary("total render+readback", &total));
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(1) {
        Some("ref") => reference(arg(2).and_then(|s| s.parse().ok()).unwrap_or(4)),
        Some("bench") => bench(
            arg(2).and_then(|s| s.parse().ok()).unwrap_or(2200),
            arg(3).and_then(|s| s.parse().ok()).unwrap_or(1300),
            arg(4).and_then(|s| s.parse().ok()).unwrap_or(300),
            arg(5).and_then(|s| s.parse().ok()).unwrap_or(4),
        ),
        Some("window") => {
            view::run(arg(2).and_then(|s| s.parse().ok()), arg(3).and_then(|s| s.parse().ok()).unwrap_or(4));
            Ok(())
        }
        _ => {
            view::run(None, 4);
            Ok(())
        }
    }
}
