//! `cargo xtask pixel selftest`: the diff tool must pass a port run that equals the upstream run
//! and fail each planted fault. Every plant goes through the same files and loader as a real
//! comparison.

use std::fs;
use std::path::{Path, PathBuf};

use asrs_parity::json::Json;
use asrs_parity::pixel::{self, Rgba};
use asrs_parity::pixel3d;
use asrs_parity::utf16::Utf16String;
use lcms2::{CIExyY, CIExyYTRIPLE, Intent, PixelFormat, Profile, ToneCurve, Transform};

use super::gates::Rect;
use super::{
    CaptureResult, capture_names, compare_roots, compare_variant, parity_dir, read_json,
    variant_dirs,
};
use crate::Result;

/// Neighbours further apart than this are an edge: the synthetic mesh's shading gradient stays
/// interior while its silhouette does not.
const EDGE_STEP: u8 = 8;
const SHADING_DELTA: u8 = 3;

pub fn selftest(args: &[String]) -> Result {
    selftest_in(&parity_dir().join("pixel/selftest"), args)
}

fn selftest_in(work: &Path, args: &[String]) -> Result {
    if work.exists() {
        fs::remove_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    }
    let mut results = Vec::new();
    for scale in [1u32, 2] {
        let dir = work.join(format!("synthetic/{scale}x"));
        let icon = synthetic::write(&dir.join("upstream"), scale)?;
        results.extend(plant_all(
            &format!("synthetic {scale}x"),
            &dir.join("upstream"),
            &dir,
            Some(icon),
            true,
        )?);
        results.extend(plant_roots(
            &format!("synthetic {scale}x"),
            &dir.join("roots"),
            scale,
        )?);
    }
    for (i, arg) in args.iter().enumerate() {
        for variant in variant_dirs(Path::new(arg))? {
            let label = variant.display().to_string();
            let dir = work.join(format!("real/{i}-{}", sanitize(&label)));
            results.extend(plant_all(&label, &variant, &dir, None, false)?);
        }
    }
    let mut wrong = 0;
    for r in &results {
        let ok = r.pass == r.expected_pass;
        let mark = match (ok, r.enforced) {
            (true, _) => "  ok  ",
            (false, true) => "WRONG ",
            (false, false) => "  note",
        };
        if !ok && r.enforced {
            wrong += 1;
        }
        eprintln!(
            "{mark} {}: {} (expected {}, got {}){}",
            r.label,
            r.plant,
            word(r.expected_pass),
            word(r.pass),
            if r.failed.is_empty() {
                String::new()
            } else {
                format!("; failing: {}", r.failed.join(", "))
            }
        );
    }
    if wrong == 0 {
        eprintln!(
            "pixel selftest: {} plants behave as expected",
            results.iter().filter(|r| r.enforced).count()
        );
        Ok(())
    } else {
        Err(format!("pixel selftest: {wrong} plants misjudged"))
    }
}

fn word(pass: bool) -> &'static str {
    if pass { "pass" } else { "fail" }
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .rev()
        .take(60)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub struct PlantResult {
    pub label: String,
    pub plant: String,
    pub expected_pass: bool,
    pub pass: bool,
    pub enforced: bool,
    pub failed: Vec<String>,
}

fn judge(
    label: &str,
    plant: &str,
    expected_pass: bool,
    enforced: bool,
    results: &[CaptureResult],
) -> PlantResult {
    let mut failed = Vec::new();
    for r in results {
        if let Some(e) = &r.error {
            failed.push(format!("{}: {e}", r.label));
        }
        for c in r.outcome.failures() {
            failed.push(format!("gate {} {}", c.gate, c.what));
        }
    }
    failed.dedup();
    PlantResult {
        label: label.to_owned(),
        plant: plant.to_owned(),
        expected_pass,
        pass: !results.is_empty() && results.iter().all(CaptureResult::pass),
        enforced,
        failed,
    }
}

fn io<T>(path: &Path, r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| format!("{}: {e}", path.display()))
}

fn copy_dir(from: &Path, to: &Path) -> Result {
    io(to, fs::create_dir_all(to))?;
    for entry in io(from, fs::read_dir(from))? {
        let path = io(from, entry)?.path();
        let dest = to.join(path.file_name().expect("entry name"));
        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            io(&dest, fs::copy(&path, &dest))?;
        }
    }
    Ok(())
}

fn set(value: &mut Json, key: &str, new: Json) {
    if let Json::Object(pairs) = value {
        match pairs.iter_mut().find(|(k, _)| k.eq_str(key)) {
            Some((_, v)) => *v = new,
            None => {
                let mut k = Utf16String::new();
                k.push_str(key);
                pairs.push((k, new));
            }
        }
    }
}

fn write_json(path: &Path, value: &Json) -> Result {
    io(path, fs::write(path, value.to_pretty() + "\n"))
}

fn record_path(variant: &Path, name: &str) -> PathBuf {
    variant.join("captures").join(format!("{name}.json"))
}

fn image_path(variant: &Path, name: &str) -> PathBuf {
    variant.join("captures").join(format!("{name}.png"))
}

fn display_p3() -> Result<Profile> {
    let xy = |x, y| CIExyY { x, y, Y: 1.0 };
    let curve =
        ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
            .map_err(|e| format!("sRGB tone curve: {e}"))?;
    Profile::new_rgb(
        &xy(0.3127, 0.3290),
        &CIExyYTRIPLE {
            Red: xy(0.680, 0.320),
            Green: xy(0.265, 0.690),
            Blue: xy(0.150, 0.060),
        },
        &[&curve, &curve, &curve],
    )
    .map_err(|e| format!("Display P3 profile: {e}"))
}

/// What a Display P3 screen's `capturePage` returns for sRGB content.
fn to_p3(image: &Rgba, p3: &Profile) -> Result<Rgba> {
    let transform: Transform<[u8; 3], [u8; 3]> = Transform::new(
        &Profile::new_srgb(),
        PixelFormat::RGB_8,
        p3,
        PixelFormat::RGB_8,
        Intent::RelativeColorimetric,
    )
    .map_err(|e| format!("sRGB to P3: {e}"))?;
    let pixels = image.data.as_chunks::<4>().0;
    let rgb: Vec<[u8; 3]> = pixels.iter().map(|p| [p[0], p[1], p[2]]).collect();
    let mut out = vec![[0u8; 3]; rgb.len()];
    transform.transform_pixels(&rgb, &mut out);
    Ok(Rgba {
        width: image.width,
        height: image.height,
        data: out
            .iter()
            .zip(pixels)
            .flat_map(|(c, p)| [c[0], c[1], c[2], p[3]])
            .collect(),
    })
}

fn edges_of(image: &Rgba) -> Vec<bool> {
    let (w, h) = (image.width as usize, image.height as usize);
    let at = |x: usize, y: usize| &image.data[(y * w + x) * 4..(y * w + x) * 4 + 3];
    let mut edges = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = at(x, y);
            let mut neighbours = Vec::new();
            if x > 0 {
                neighbours.push(at(x - 1, y));
            }
            if x + 1 < w {
                neighbours.push(at(x + 1, y));
            }
            if y > 0 {
                neighbours.push(at(x, y - 1));
            }
            if y + 1 < h {
                neighbours.push(at(x, y + 1));
            }
            edges[y * w + x] = neighbours
                .iter()
                .any(|n| n.iter().zip(p).any(|(a, b)| a.abs_diff(*b) > EDGE_STEP));
        }
    }
    edges
}

fn mask_png(mask: &[bool], width: u32, height: u32) -> Rgba {
    Rgba {
        width,
        height,
        data: mask
            .iter()
            .flat_map(|m| {
                if *m {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 255]
                }
            })
            .collect(),
    }
}

/// Turns a copy of an upstream variant into what a correct port would have written: sRGB
/// captures and an edge mask beside every 3D dump.
fn make_port(upstream: &Path, port: &Path) -> Result {
    copy_dir(upstream, port)?;
    for name in capture_names(port)? {
        let path = record_path(port, &name);
        let mut record = read_json(&path)?;
        let capture = super::load_capture(upstream, &name, false)?;
        if record.get("colourSpace") != Some(&Json::string("srgb")) {
            pixel::write_png(&image_path(port, &name), &capture.image)?;
            set(&mut record, "colourSpace", Json::string("srgb"));
        }
        if let Some(Json::Array(gl)) = record.get("gl").cloned() {
            let mut entries = Vec::new();
            for (mut entry, dump) in gl.into_iter().zip(&capture.gl) {
                let file = format!("{}.edges.png", dump.file.trim_end_matches(".png"));
                let edges = edges_of(&dump.image);
                pixel::write_png(
                    &port.join("captures").join(&file),
                    &mask_png(&edges, dump.image.width, dump.image.height),
                )?;
                set(&mut entry, "edges", Json::string(&file));
                entries.push(entry);
            }
            set(&mut record, "gl", Json::Array(entries));
        }
        write_json(&path, &record)?;
    }
    Ok(())
}

fn shift_right(image: &Rgba) -> Rgba {
    let w = image.width as usize;
    let mut data = image.data.clone();
    for row in data.chunks_mut(w * 4) {
        row.copy_within(0..(w - 1) * 4, 4);
    }
    Rgba {
        data,
        ..image.clone()
    }
}

fn fill(image: &mut Rgba, span: [usize; 4], colour: [u8; 4]) {
    let w = image.width as usize;
    for y in span[1]..span[3] {
        for x in span[0]..span[2] {
            image.data[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&colour);
        }
    }
}

fn span_of(rect: Rect, scale: f64) -> [usize; 4] {
    [
        (rect[0] * scale).floor() as usize,
        (rect[1] * scale).floor() as usize,
        ((rect[0] + rect[2]) * scale).ceil() as usize,
        ((rect[1] + rect[3]) * scale).ceil() as usize,
    ]
}

/// The 16 by 16 CSS px box on a uniform ring with the most ink inside, away from text and
/// dumps: an icon on a plain background.
fn find_icon(variant: &Path, name: &str) -> Result<Option<(Rect, [u8; 4])>> {
    let capture = super::load_capture(variant, name, false)?;
    let image = &capture.image;
    let (w, h) = (image.width as usize, image.height as usize);
    let s = capture.scale;
    let busy: Vec<Rect> = capture
        .texts
        .iter()
        .copied()
        .chain(capture.canvases.iter().map(|d| d.rect))
        .chain(capture.gl.iter().map(|d| d.rect))
        .chain(capture.masks.iter().map(|m| m.1))
        .collect();
    let overlaps = |a: Rect, b: Rect| {
        a[0] < b[0] + b[2] + 2.0
            && b[0] - 2.0 < a[0] + a[2]
            && a[1] < b[1] + b[3] + 2.0
            && b[1] - 2.0 < a[1] + a[3]
    };
    let px = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * w + x) * 4;
        [
            image.data[i],
            image.data[i + 1],
            image.data[i + 2],
            image.data[i + 3],
        ]
    };
    let mut best: Option<(usize, Rect, [u8; 4])> = None;
    let css = (w as f64 / s, h as f64 / s);
    let mut y = 0.0;
    while y + 16.0 <= css.1 {
        let mut x = 0.0;
        while x + 16.0 <= css.0 {
            let rect = [x, y, 16.0, 16.0];
            x += 2.0;
            if busy.iter().any(|b| overlaps(rect, *b)) {
                continue;
            }
            let [x0, y0, x1, y1] = span_of(rect, s);
            let ring = px(x0, y0);
            let uniform = (x0..x1).all(|i| px(i, y0) == ring && px(i, y1 - 1) == ring)
                && (y0..y1).all(|j| px(x0, j) == ring && px(x1 - 1, j) == ring);
            if !uniform {
                continue;
            }
            let ink = (y0..y1)
                .flat_map(|j| (x0..x1).map(move |i| (i, j)))
                .filter(|&(i, j)| (0..3).any(|c| px(i, j)[c].abs_diff(ring[c]) > 32))
                .count();
            if best.is_none_or(|(b, _, _)| ink > b) {
                best = Some((ink, rect, ring));
            }
        }
        y += 2.0;
    }
    let threshold = pixel3d::OUTLIER_BLOB_MAX.max(super::gates::PIXEL_MAX_BLOB as usize);
    Ok(best
        .filter(|(ink, _, _)| *ink > threshold)
        .map(|(_, r, c)| (r, c)))
}

fn rewrite_image(port: &Path, name: &str, f: impl FnOnce(&Rgba) -> Rgba) -> Result {
    let path = image_path(port, name);
    let image = pixel::read_png(&path)?;
    pixel::write_png(&path, &f(&image))
}

fn plant_all(
    label: &str,
    upstream: &Path,
    work: &Path,
    icon: Option<(String, Rect)>,
    synthetic: bool,
) -> Result<Vec<PlantResult>> {
    let names = capture_names(upstream)?;
    let first = names
        .first()
        .ok_or_else(|| format!("{}: no captures", upstream.display()))?
        .clone();
    let has_patches = names.iter().any(|n| {
        read_json(&record_path(upstream, n))
            .is_ok_and(|r| matches!(r.get("patches"), Some(Json::Array(_))))
    });
    let has_gl = names.iter().any(|n| {
        read_json(&record_path(upstream, n)).is_ok_and(|r| {
            r.get("gl")
                .and_then(Json::as_array)
                .is_some_and(|a| !a.is_empty())
        })
    });
    let mut out = Vec::new();
    let run = |dir: &Path| compare_variant(label, upstream, dir);

    let identical = work.join("identical");
    make_port(upstream, &identical)?;
    out.push(judge(
        label,
        "identical inputs",
        true,
        true,
        &run(&identical)?,
    ));

    let shifted = work.join("shift");
    make_port(upstream, &shifted)?;
    rewrite_image(&shifted, &first, shift_right)?;
    out.push(judge(label, "1 px shift", false, true, &run(&shifted)?));

    let icon = match icon {
        Some(known) => Some(known),
        None => names
            .iter()
            .find_map(|n| {
                find_icon(upstream, n)
                    .ok()
                    .flatten()
                    .map(|f| (n.clone(), f))
            })
            .map(|(n, (rect, _))| (n, rect)),
    };
    match icon {
        Some((name, rect)) => {
            let dir = work.join("icon");
            make_port(upstream, &dir)?;
            let scale = super::load_capture(upstream, &name, false)?.scale;
            rewrite_image(&dir, &name, |image| {
                let mut image = image.clone();
                let span = span_of(rect, scale);
                let w = image.width as usize;
                let i = (span[1].saturating_sub(1) * w + span[0].saturating_sub(1)) * 4;
                let ring = [
                    image.data[i],
                    image.data[i + 1],
                    image.data[i + 2],
                    image.data[i + 3],
                ];
                fill(&mut image, span, ring);
                image
            })?;
            out.push(judge(
                label,
                &format!("missing icon at {rect:?} in {name}"),
                false,
                true,
                &run(&dir)?,
            ));
        }
        None => out.push(PlantResult {
            label: label.to_owned(),
            plant: "missing icon: no icon-like box found to remove".to_owned(),
            expected_pass: false,
            pass: true,
            enforced: false,
            failed: Vec::new(),
        }),
    }

    let p3 = display_p3()?;
    let p3_bytes = p3.icc().map_err(|e| format!("P3 profile bytes: {e}"))?;
    for (plant, tagged, display_run) in [
        ("P3 capture normalized", true, true),
        (
            "P3 capture of a display-colour run left unnormalized",
            false,
            true,
        ),
        ("P3 pixels with no colour metadata", false, false),
    ] {
        let base = match (tagged, display_run) {
            (true, _) => "p3-tagged",
            (false, true) => "p3-untagged",
            (false, false) => "p3-bare",
        };
        let dir = work.join(base);
        make_port(upstream, &dir)?;
        let reference = work.join(format!("{base}-upstream"));
        make_port(upstream, &reference)?;
        for name in capture_names(&reference)? {
            let srgb = super::load_capture(&reference, &name, false)?.image;
            pixel::write_png(&image_path(&reference, &name), &to_p3(&srgb, &p3)?)?;
            if tagged {
                let path = record_path(&reference, &name);
                let mut record = read_json(&path)?;
                set(&mut record, "colourSpace", Json::string("display.icc"));
                write_json(&path, &record)?;
            }
        }
        if display_run {
            io(
                &reference,
                fs::write(reference.join("display.icc"), &p3_bytes),
            )?;
            let path = reference.join("run.json");
            let mut run = read_json(&path)?;
            set(&mut run, "colour", Json::string("display"));
            set(
                &mut run,
                "displayProfile",
                Json::object([("source", Json::string("macos-nsscreen"))]),
            );
            write_json(&path, &run)?;
        }
        let results = compare_variant(label, &reference, &dir)?;
        let enforced = synthetic
            || match (tagged, display_run) {
                (true, _) => !has_patches,
                (false, true) => true,
                (false, false) => has_patches,
            };
        out.push(judge(label, plant, tagged, enforced, &results));
    }
    {
        let dir = work.join("p3-embedded");
        make_port(upstream, &dir)?;
        let reference = work.join("p3-embedded-upstream");
        make_port(upstream, &reference)?;
        let srgb = super::load_capture(&reference, &first, false)?.image;
        io(
            &reference,
            fs::write(
                image_path(&reference, &first),
                pixel::encode_png_with_icc(&to_p3(&srgb, &p3)?, &p3_bytes),
            ),
        )?;
        let results = compare_variant(label, &reference, &dir)?;
        out.push(judge(
            label,
            "P3-tagged PNG recorded as sRGB",
            false,
            true,
            &results,
        ));
    }

    if has_gl {
        let dir = work.join("shading");
        make_port(upstream, &dir)?;
        for name in capture_names(&dir)? {
            let record = read_json(&record_path(&dir, &name))?;
            for entry in record.get("gl").and_then(Json::as_array).unwrap_or(&[]) {
                let file = |key: &str| {
                    entry
                        .get(key)
                        .and_then(Json::as_str)
                        .map(|s| dir.join("captures").join(s.to_string()))
                };
                let (Some(image_file), Some(edges_file)) = (file("file"), file("edges")) else {
                    continue;
                };
                let mut image = pixel::read_png(&image_file)?;
                let edges = super::edges_from_png(&pixel::read_png(&edges_file)?);
                let zone = pixel3d::dilate(
                    &edges,
                    image.width as usize,
                    image.height as usize,
                    pixel3d::EDGE_RADIUS,
                );
                for (i, near_edge) in zone.iter().enumerate() {
                    if !near_edge {
                        for c in 0..3 {
                            let v = &mut image.data[i * 4 + c];
                            *v = v.saturating_add(SHADING_DELTA);
                        }
                    }
                }
                pixel::write_png(&image_file, &image)?;
            }
        }
        out.push(judge(
            label,
            "interior 3D shading change of 3 levels",
            false,
            true,
            &run(&dir)?,
        ));
    } else if synthetic {
        return Err(format!("{label}: the synthetic variant has no 3D dump"));
    }
    Ok(out)
}

fn greyscale(image: &Rgba) -> Rgba {
    let data = image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let g = ((u32::from(p[0]) * 3 + u32::from(p[1]) * 6 + u32::from(p[2])) / 10) as u8;
            [g, g, g, p[3]]
        })
        .collect();
    Rgba {
        data,
        ..image.clone()
    }
}

fn plant_roots(label: &str, work: &Path, scale: u32) -> Result<Vec<PlantResult>> {
    let upstream = work.join("upstream");
    let grey = upstream.join("ui/v");
    synthetic::write(&grey, scale)?;
    rewrite_image(&grey, "ui", greyscale)?;
    synthetic::write_patches(&upstream.join("colour-patch/v"), scale)?;
    let port = work.join("port");
    for variant in variant_dirs(&upstream)? {
        let rel = variant.strip_prefix(&upstream).expect("under root");
        make_port(&variant, &port.join(rel))?;
    }
    let mut out = Vec::new();
    out.push(judge(
        label,
        "identical runs with their colour patch",
        true,
        true,
        &compare_roots(&upstream, &port)?,
    ));

    let extra = work.join("port-extra");
    copy_dir(&port, &extra)?;
    copy_dir(&extra.join("ui/v"), &extra.join("ui/w"))?;
    out.push(judge(
        label,
        "a variant run by the port only",
        false,
        true,
        &compare_roots(&upstream, &extra)?,
    ));

    let alone = work.join("upstream-no-patch");
    copy_dir(&grey, &alone.join("ui/v"))?;
    out.push(judge(
        label,
        "an upstream run without its colour patch",
        false,
        true,
        &compare_roots(&alone, &port)?,
    ));

    let bare = work.join("upstream-p3-bare");
    copy_dir(&upstream, &bare)?;
    let p3 = display_p3()?;
    for variant in variant_dirs(&bare)? {
        for name in capture_names(&variant)? {
            rewrite_image(&variant, &name, |image| {
                to_p3(image, &p3).expect("sRGB to P3")
            })?;
        }
    }
    let grey_only = |results: Vec<CaptureResult>| -> Vec<CaptureResult> {
        results
            .into_iter()
            .filter(|r| r.label.starts_with("ui"))
            .collect()
    };
    let ui = grey_only(compare_roots(&bare, &port)?);
    out.push(judge(
        label,
        "P3 pixels with no colour metadata on a grey UI, colour patch in the same run",
        false,
        true,
        &ui,
    ));
    Ok(out)
}

mod synthetic {
    use std::fs;
    use std::path::Path;

    use asrs_parity::json::Json;
    use asrs_parity::pixel::{self, Rgba};

    use super::super::gates::Rect;
    use super::{io, write_json};
    use crate::Result;

    pub const WIDTH: f64 = 320.0;
    pub const HEIGHT: f64 = 200.0;
    const BACKGROUND: [u8; 4] = [236, 236, 236, 255];
    const TITLE: [u8; 4] = [222, 222, 224, 255];
    const SIDEBAR: [u8; 4] = [245, 245, 247, 255];
    const LINE: [u8; 4] = [196, 196, 200, 255];
    const INK: [u8; 4] = [30, 30, 30, 255];
    const ICON: Rect = [290.0, 11.0, 16.0, 16.0];
    const TEXTS: [Rect; 3] = [
        [110.0, 12.0, 120.0, 14.0],
        [10.0, 50.0, 80.0, 14.0],
        [10.0, 70.0, 60.0, 14.0],
    ];
    const CANVAS: Rect = [110.0, 130.0, 100.0, 50.0];
    const GL: Rect = [220.0, 130.0, 90.0, 60.0];
    const SWATCHES: [[u8; 4]; 8] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
        [255, 0, 255, 255],
        [0, 255, 255, 255],
        [0x2b, 0x66, 0xa2, 255],
        [0xe5, 0xb3, 0x1b, 255],
    ];

    struct Canvas {
        scale: usize,
        image: Rgba,
    }

    impl Canvas {
        fn new(css_w: f64, css_h: f64, scale: usize, colour: [u8; 4]) -> Self {
            let (w, h) = (css_w as usize * scale, css_h as usize * scale);
            Self {
                scale,
                image: Rgba {
                    width: w as u32,
                    height: h as u32,
                    data: colour.repeat(w * h),
                },
            }
        }

        fn rect(&mut self, r: Rect, colour: [u8; 4]) {
            let s = self.scale as f64;
            super::fill(&mut self.image, super::span_of(r, s), colour);
        }

        fn blit(&mut self, at: Rect, src: &Rgba) {
            let w = self.image.width as usize;
            let [x0, y0, ..] = super::span_of(at, self.scale as f64);
            for y in 0..src.height as usize {
                for x in 0..src.width as usize {
                    let s = &src.data[(y * src.width as usize + x) * 4..][..4];
                    let d = &mut self.image.data[((y0 + y) * w + x0 + x) * 4..][..4];
                    let a = u32::from(s[3]);
                    for c in 0..3 {
                        d[c] =
                            ((u32::from(s[c]) * a + u32::from(d[c]) * (255 - a) + 127) / 255) as u8;
                    }
                }
            }
        }
    }

    fn chart(scale: usize) -> Rgba {
        let mut c = Canvas::new(CANVAS[2], CANVAS[3], scale, [0, 0, 0, 0]);
        for i in 0..90 {
            let y = 25.0 + 15.0 * (f64::from(i) / 10.0).sin();
            c.rect(
                [5.0 + f64::from(i), y.round(), 1.0, 2.0],
                [43, 102, 162, 255],
            );
        }
        c.image
    }

    fn scene(scale: usize) -> Rgba {
        let (w, h) = (GL[2] as usize * scale, GL[3] as usize * scale);
        let mut data = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let inside =
                    (10 * scale..80 * scale).contains(&x) && (10 * scale..50 * scale).contains(&y);
                data.extend_from_slice(&if inside {
                    let fx = (x - 10 * scale) * 40 / (70 * scale);
                    let fy = (y - 10 * scale) * 20 / (40 * scale);
                    [100 + fx as u8, 120, 140 + fy as u8, 255]
                } else {
                    [30, 30, 40, 255]
                });
            }
        }
        Rgba {
            width: w as u32,
            height: h as u32,
            data,
        }
    }

    fn rects(rects: &[Rect]) -> Json {
        Json::Array(
            rects
                .iter()
                .map(|r| Json::Array(r.iter().map(|v| Json::Number(*v)).collect()))
                .collect(),
        )
    }

    pub fn write(dir: &Path, scale: u32) -> Result<(String, Rect)> {
        let s = scale as usize;
        let mut ui = Canvas::new(WIDTH, HEIGHT, s, BACKGROUND);
        ui.rect([0.0, 0.0, WIDTH, 38.0], TITLE);
        ui.rect([0.0, 37.0, WIDTH, 1.0], LINE);
        ui.rect([0.0, 38.0, 100.0, HEIGHT - 38.0], SIDEBAR);
        ui.rect([100.0, 38.0, 1.0, HEIGHT - 38.0], LINE);
        ui.rect(ICON, INK);
        ui.rect([ICON[0] + 3.0, ICON[1] + 3.0, 10.0, 10.0], TITLE);
        for t in TEXTS {
            let mut x = t[0] + 1.0;
            while x < t[0] + t[2] - 1.0 {
                ui.rect([x, t[1] + 2.0, 1.0, t[3] - 4.0], INK);
                x += 3.0;
            }
        }
        for (i, colour) in SWATCHES.iter().enumerate() {
            ui.rect([110.0 + i as f64 * 26.0, 60.0, 22.0, 22.0], *colour);
        }
        ui.rect([110.0, 92.0, 200.0, 20.0], [0, 122, 255, 255]);
        let chart = chart(s);
        let scene = scene(s);
        ui.blit(CANVAS, &chart);
        ui.blit(GL, &scene);

        let captures = dir.join("captures");
        io(&captures, fs::create_dir_all(&captures))?;
        pixel::write_png(&captures.join("ui.png"), &ui.image)?;
        pixel::write_png(&captures.join("ui.canvas-0.png"), &chart)?;
        pixel::write_png(&captures.join("ui.gl-1.png"), &scene)?;
        let num = |v: f64| Json::Number(v);
        let rect = |r: Rect| Json::Array(r.iter().map(|v| Json::Number(*v)).collect());
        run_json(dir, scale, WIDTH, HEIGHT)?;
        write_json(
            &captures.join("ui.json"),
            &Json::object([
                ("name", Json::string("ui")),
                ("scale", num(f64::from(scale))),
                ("width", num(f64::from(ui.image.width))),
                ("height", num(f64::from(ui.image.height))),
                ("image", Json::string("ui.png")),
                ("colourSpace", Json::string("srgb")),
                (
                    "probes",
                    Json::object([
                        (".title-bar", rects(&[[0.0, 0.0, WIDTH, 38.0]])),
                        (".side-bar", rects(&[[0.0, 38.0, 100.0, HEIGHT - 38.0]])),
                        (".icon", rects(&[ICON])),
                    ]),
                ),
                (
                    "texts",
                    Json::Array(
                        TEXTS
                            .iter()
                            .map(|t| {
                                Json::object([
                                    ("text", Json::string("synthetic")),
                                    ("rects", rects(&[*t])),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("masks", Json::Array(Vec::new())),
                (
                    "canvases",
                    Json::Array(vec![Json::object([
                        ("file", Json::string("ui.canvas-0.png")),
                        ("rect", rect(CANVAS)),
                    ])]),
                ),
                (
                    "gl",
                    Json::Array(vec![Json::object([
                        ("file", Json::string("ui.gl-1.png")),
                        ("rect", rect(GL)),
                    ])]),
                ),
            ]),
        )?;
        Ok(("ui".to_owned(), ICON))
    }

    fn run_json(dir: &Path, scale: u32, width: f64, height: f64) -> Result {
        let num = |v: f64| Json::Number(v);
        write_json(
            &dir.join("run.json"),
            &Json::object([
                ("scenario", Json::string("synthetic")),
                ("platform", Json::string("darwin")),
                ("colour", Json::string("srgb")),
                (
                    "displayProfile",
                    Json::object([("source", Json::string("forced-srgb"))]),
                ),
                ("scale", num(f64::from(scale))),
                (
                    "window",
                    Json::object([("width", num(width)), ("height", num(height))]),
                ),
            ]),
        )
    }

    pub fn write_patches(dir: &Path, scale: u32) -> Result {
        const SIDE: f64 = 16.0;
        let colours: Vec<[u8; 4]> = SWATCHES
            .iter()
            .copied()
            .chain((0..8).map(|i| [i * 36, i * 36, i * 36, 255]))
            .collect();
        let (w, h) = (SIDE * 8.0, SIDE * 2.0);
        let mut page = Canvas::new(w, h, scale as usize, [255, 255, 255, 255]);
        let mut patches = Vec::new();
        for (i, c) in colours.iter().enumerate() {
            let r = [(i % 8) as f64 * SIDE, (i / 8) as f64 * SIDE, SIDE, SIDE];
            page.rect(r, *c);
            patches.push(Json::object([
                (
                    "rgb",
                    Json::Array(c[..3].iter().map(|v| Json::Number(f64::from(*v))).collect()),
                ),
                ("rect", rects(&[r]).as_array().expect("one rect")[0].clone()),
            ]));
        }
        let captures = dir.join("captures");
        io(&captures, fs::create_dir_all(&captures))?;
        pixel::write_png(&captures.join("patches.png"), &page.image)?;
        run_json(dir, scale, w, h)?;
        let num = |v: f64| Json::Number(v);
        write_json(
            &captures.join("patches.json"),
            &Json::object([
                ("name", Json::string("patches")),
                ("scale", num(f64::from(scale))),
                ("width", num(f64::from(page.image.width))),
                ("height", num(f64::from(page.image.height))),
                ("image", Json::string("patches.png")),
                ("colourSpace", Json::string("srgb")),
                ("probes", Json::object(Vec::<(&str, Json)>::new())),
                ("texts", Json::Array(Vec::new())),
                ("masks", Json::Array(Vec::new())),
                ("canvases", Json::Array(Vec::new())),
                ("gl", Json::Array(Vec::new())),
                ("patches", Json::Array(patches)),
            ]),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn synthetic_plants_are_judged_right() {
        let work = std::env::temp_dir().join(format!("asrs-pixel-selftest-{}", std::process::id()));
        let result = super::selftest_in(&work, &[]);
        let _ = std::fs::remove_dir_all(&work);
        result.unwrap();
    }
}
