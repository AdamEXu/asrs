use std::fs;
use std::path::PathBuf;
use std::process::Command;

use asrs_parity::json::Json;
use asrs_parity::pixel::{self, Rgba};
use asrs_parity::pixel3d;

use crate::canvas::{platform, write};
use crate::oracle::{self, Paths};
use crate::{Result, cargo, gl};

const USAGE: &str = "usage: cargo xtask field3d <command>

3D reference scenes (parity/field3d/scenes/*.json) rendered by upstream's three 0.168 in Electron
and read back from the WebGL drawing buffer, against the port's wgpu renderer, at 1x and 2x,
dark and light. Files land in target/parity/field3d/<os>/.

commands:
  ref [<scene>...]    capture the references (ref/<scene>@<n>x-<theme>.png) with the 3D switches
                      of ASRS_GL (native, swiftshader or d3d11-warp; default native on macOS,
                      swiftshader elsewhere)
  run [<scene>...]    render with the port (example field3d_scenes) into port/ and apply gate 4:
                      SSIM >= 0.995, interior pixels (over 2 px from an edge of the port's id and
                      depth buffers) within 2 levels and at most 0.05 % differing, at most 1 %
                      of all pixels differing, no blob of over 4 px (edge zone included) more
                      than 32 levels off. Shadow maps must be equal texel for texel, and line
                      widths at the scene's widthProbes within 0.05 px. On Linux under swiftshader
                      the port runs on Electron's SwiftShader (VK_ICD_FILENAMES)
  bench [<args>...]   frame-time benchmark of the render thread (example field3d_bench)";

const SCALES: [u32; 2] = [1, 2];
const LINE_WIDTH_MAX_DIFF_PX: f64 = 0.05;
const THEMES: [&str; 2] = ["dark", "light"];
const STAMP: &str = "generator.txt";

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("ref") => reference(&paths, rest),
        Some("run") => run_port(&paths, rest),
        Some("bench") => bench(rest),
        _ => Err(USAGE.to_owned()),
    }
}

fn base_dir(paths: &Paths) -> PathBuf {
    paths.root.join("target/parity/field3d").join(platform())
}

fn scenes_dir(paths: &Paths) -> PathBuf {
    paths.root.join("parity/field3d/scenes")
}

fn stamp_text(paths: &Paths, gl: &str) -> Result<String> {
    let generator = oracle::hash_tree(
        &paths.root,
        &[
            "oracle/field3d",
            "oracle/src/canvas/png.ts",
            "parity/field3d/scenes",
        ],
    )?;
    Ok(format!("{generator} {gl}\n"))
}

fn reference(paths: &Paths, scenes: &[String]) -> Result {
    let gl = gl::default_mode()?;
    let out = paths.out.join("field3d");
    oracle::step("build 3D harness");
    let status = Command::new("node")
        .arg(paths.oracle_src.join("field3d/build.mjs"))
        .arg(&paths.upstream)
        .arg(&paths.harness)
        .arg(&out)
        .status()
        .map_err(|e| format!("running node: {e}"))?;
    if !status.success() {
        return Err(format!("3D harness build failed ({status})"));
    }
    let ref_dir = base_dir(paths).join("ref");
    let stamp = base_dir(paths).join(STAMP);
    if scenes.is_empty() && ref_dir.exists() {
        fs::remove_dir_all(&ref_dir).map_err(|e| format!("{}: {e}", ref_dir.display()))?;
    }
    if stamp.exists() {
        fs::remove_file(&stamp).map_err(|e| format!("{}: {e}", stamp.display()))?;
    }
    let mut command = Command::new(paths.electron()?);
    command
        .args(gl::electron_switches(gl))
        .arg(out.join("field3dMain.mjs"))
        .arg(scenes_dir(paths))
        .arg(&ref_dir)
        .args(scenes)
        .env("ORACLE_UPSTREAM_DIR", &paths.upstream)
        .env_remove("ELECTRON_RUN_AS_NODE");
    oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
    let status = command
        .status()
        .map_err(|e| format!("running Electron: {e}"))?;
    if !status.success() {
        return Err(format!("3D reference capture failed ({status})"));
    }
    if scenes.is_empty() {
        write(&stamp, stamp_text(paths, gl)?.as_bytes())?;
    }
    eprintln!("3D references in {}", ref_dir.display());
    Ok(())
}

fn example(name: &str) -> Result<PathBuf> {
    cargo(&[
        "build",
        "--quiet",
        "--release",
        "-p",
        "asrs-field3d",
        "--example",
        name,
    ])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::workspace_root().join("target"));
    Ok(target.join("release/examples").join(name))
}

fn scene_names(paths: &Paths, wanted: &[String]) -> Result<Vec<String>> {
    let dir = scenes_dir(paths);
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if let Some(n) = name.to_str().and_then(|n| n.strip_suffix(".json"))
            && (wanted.is_empty() || wanted.iter().any(|w| w == n))
        {
            names.push(n.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The width in device pixels of a horizontal line crossing column `x` between rows `y0` and
/// `y1`: the sum of each pixel's coverage, its colour's fraction of the way from the background
/// to the line colour.
fn line_width(
    image: &Rgba,
    x: u32,
    y0: u32,
    y1: u32,
    background: [f64; 3],
    color: [f64; 3],
) -> f64 {
    let span: Vec<f64> = (0..3).map(|c| color[c] - background[c]).collect();
    let norm: f64 = span.iter().map(|v| v * v).sum();
    (y0..y1.min(image.height))
        .map(|y| {
            let o = ((y * image.width + x) * 4) as usize;
            let along: f64 = (0..3)
                .map(|c| (f64::from(image.data[o + c]) - background[c]) * span[c])
                .sum();
            (along / norm).clamp(0.0, 1.0)
        })
        .sum()
}

fn width_probes(paths: &Paths, name: &str) -> Result<Vec<[f64; 4]>> {
    let path = scenes_dir(paths).join(format!("{name}.json"));
    let spec = asrs_parity::json::parse(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(spec
        .get("widthProbes")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let n = |k| p.get(k).and_then(Json::as_f64).unwrap_or(0.0);
            [n("x"), n("y0"), n("y1"), n("color")]
        })
        .collect())
}

fn is_cinematic(paths: &Paths, name: &str) -> Result<bool> {
    let path = scenes_dir(paths).join(format!("{name}.json"));
    let spec = asrs_parity::json::parse(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(matches!(spec.get("shadows"), Some(Json::Bool(true))))
}

fn edges_from_png(image: &Rgba) -> Vec<bool> {
    image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0] > 127)
        .collect()
}

fn diff_image(reference: &Rgba, port: &Rgba, edges: &[bool]) -> Rgba {
    let mut data = Vec::with_capacity(port.data.len());
    for (i, (a, b)) in reference
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(port.data.as_chunks::<4>().0.iter())
        .enumerate()
    {
        let delta = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        let grey = ((u32::from(a[0]) + u32::from(a[1]) + u32::from(a[2])) / 12) as u8;
        data.extend_from_slice(&match (delta, edges[i]) {
            (0, false) => [grey, grey, grey, 255],
            (0, true) => [0, 0, 96, 255],
            (_, true) => [255, 200, 0, 255],
            (_, false) => [255, 0, 0, 255],
        });
    }
    Rgba {
        width: port.width,
        height: port.height,
        data,
    }
}

fn run_port(paths: &Paths, wanted: &[String]) -> Result {
    let base = base_dir(paths);
    let current = fs::read_to_string(base.join(STAMP)).unwrap_or_default();
    let gl = gl::default_mode()?;
    if current != stamp_text(paths, gl)? {
        reference(paths, &[])?;
    }
    let exe = example("field3d_scenes")?;
    let port = base.join("port");
    if port.exists() {
        fs::remove_dir_all(&port).map_err(|e| format!("{}: {e}", port.display()))?;
    }
    let mut command = Command::new(&exe);
    gl::pair_port(paths, gl, &mut command)?;
    let status = command
        .arg(scenes_dir(paths))
        .arg(&port)
        .args(wanted)
        .status()
        .map_err(|e| format!("running {}: {e}", exe.display()))?;
    if !status.success() {
        return Err(format!("field3d_scenes failed ({status})"));
    }
    fs::create_dir_all(port.join("diff")).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    eprintln!(
        "{:<26} {:>8} {:>9} {:>6} {:>10} {:>6} {:>7} {:>5} | result",
        "capture", "ssim", "differ %", "imax", "idiffer %", "max", "dE00", "blob"
    );
    for name in scene_names(paths, wanted)? {
        let cinematic = is_cinematic(paths, &name)?;
        let probes = width_probes(paths, &name)?;
        for layer in 0.. {
            let file = format!("{name}.shadow{layer}.png");
            let (reference, ours) = (
                pixel::read_png(&base.join("ref").join(&file)),
                pixel::read_png(&port.join(&file)),
            );
            let label = format!("{name} shadow map {layer}");
            match (reference, ours) {
                (Err(_), Err(_)) => {
                    if cinematic && layer == 0 {
                        eprintln!("{label}: missing on both sides");
                        failed.push(label);
                    }
                    break;
                }
                (Ok(reference), Ok(ours))
                    if (reference.width, reference.height) == (ours.width, ours.height) =>
                {
                    let texels = reference
                        .data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(ours.data.as_chunks::<4>().0)
                        .filter(|(a, b)| a != b)
                        .count();
                    eprintln!("{label}: {texels} texels differ");
                    if texels > 0 {
                        failed.push(label);
                    }
                }
                (reference, ours) => {
                    let size = |r: &Result<Rgba>| match r {
                        Ok(i) => format!("{}x{}", i.width, i.height),
                        Err(_) => "missing".to_owned(),
                    };
                    eprintln!(
                        "{label}: upstream {}, port {}",
                        size(&reference),
                        size(&ours)
                    );
                    failed.push(label);
                }
            }
        }
        for scale in SCALES {
            let edges = edges_from_png(&pixel::read_png(
                &port.join(format!("{name}@{scale}x.edges.png")),
            )?);
            for theme in THEMES {
                let capture = format!("{name}@{scale}x-{theme}");
                let reference = pixel::read_png(&base.join(format!("ref/{capture}.png")))?;
                let ours = pixel::read_png(&port.join(format!("{capture}.png")))?;
                let g = pixel3d::gate4(&reference, &ours, &edges)
                    .map_err(|e| format!("{capture}: {e}"))?;
                let mut pass = g.pass;
                eprintln!(
                    "{capture:<26} {:>8.5} {:>9.3} {:>6} {:>10.4} {:>6} {:>7.4} {:>5} | {}",
                    g.ssim,
                    g.differing_pct,
                    g.interior_max_delta,
                    g.interior_differing_pct,
                    g.max_delta,
                    g.mean_delta_e,
                    g.outlier_blob,
                    if pass { "pass" } else { "FAIL" }
                );
                let background = if theme == "dark" {
                    [34.0; 3]
                } else {
                    [255.0; 3]
                };
                for [x, y0, y1, color] in &probes {
                    let hex = *color as u32;
                    let color =
                        [hex >> 16, (hex >> 8) & 255, hex & 255].map(|c| f64::from(c & 255));
                    if color == background {
                        continue;
                    }
                    let s = f64::from(scale);
                    let at = |image: &Rgba| {
                        line_width(
                            image,
                            (x * s) as u32,
                            (y0 * s) as u32,
                            (y1 * s) as u32,
                            background,
                            color,
                        )
                    };
                    let (theirs, mine) = (at(&reference), at(&ours));
                    let off = theirs < 0.5 || (theirs - mine).abs() > LINE_WIDTH_MAX_DIFF_PX;
                    eprintln!(
                        "    line #{hex:06x} at x={x}: upstream {theirs:.3} px, port {mine:.3} px, difference {:.3}{}",
                        (theirs - mine).abs(),
                        if off { " FAIL" } else { "" }
                    );
                    pass &= !off;
                }
                if g.max_delta > 0 {
                    pixel::write_png(
                        &port.join(format!("diff/{capture}.png")),
                        &diff_image(
                            &reference,
                            &ours,
                            &pixel3d::dilate(
                                &edges,
                                ours.width as usize,
                                ours.height as usize,
                                pixel3d::EDGE_RADIUS,
                            ),
                        ),
                    )?;
                }
                if !pass {
                    failed.push(capture.clone());
                }
                rows.push(Json::object([
                    ("capture", Json::string(&capture)),
                    ("ssim", Json::Number(g.ssim)),
                    ("differingPct", Json::Number(g.differing_pct)),
                    (
                        "interiorMaxDelta",
                        Json::Number(f64::from(g.interior_max_delta)),
                    ),
                    (
                        "interiorDifferingPct",
                        Json::Number(g.interior_differing_pct),
                    ),
                    ("maxDelta", Json::Number(f64::from(g.max_delta))),
                    ("meanDeltaE00", Json::Number(g.mean_delta_e)),
                    ("outlierBlob", Json::Number(g.outlier_blob as f64)),
                    ("cinematic", Json::Bool(cinematic)),
                    ("pass", Json::Bool(pass)),
                ]));
            }
        }
    }
    write(
        &port.join("report.json"),
        Json::Array(rows).to_pretty().as_bytes(),
    )?;
    if failed.is_empty() {
        eprintln!("gate 4: all captures pass");
        Ok(())
    } else {
        Err(format!("gate 4 failed: {}", failed.join(", ")))
    }
}

fn bench(args: &[String]) -> Result {
    let exe = example("field3d_bench")?;
    let status = Command::new(&exe)
        .args(args)
        .status()
        .map_err(|e| format!("running {}: {e}", exe.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("field3d_bench failed ({status})"))
    }
}
