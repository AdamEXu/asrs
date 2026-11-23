//! The section 2.5 gates on one capture pair: layout (1), pixels outside text (2, window and
//! canvas dumps), text regions (3), the 3D viewport (4, `asrs_parity::pixel3d`), masks (5), and
//! the colour-patch rule.

use asrs_parity::pixel::{self, Rgb, Rgba};
use asrs_parity::pixel3d;

pub type Rect = [f64; 4];

pub const LAYOUT_TOLERANCE: f64 = 1.0;
pub const PIXEL_MAX_PCT: f64 = 0.2;
pub const PIXEL_MAX_BLOB: u64 = 64;
pub const TEXT_DILATE: f64 = 2.0;
pub const TEXT_BOX_TOLERANCE: f64 = 1.0;
pub const TEXT_MAX_PCT: f64 = 6.0;
pub const PATCH_REFERENCE_MAX: u8 = 1;
/// Of the visible span. Chromium animates a notched wheel tick in compositor-timed steps, and
/// `ScrollSensor` compounds the zoom per step, so upstream runs of the same input differ in the
/// last bits (measured 9e-16 of the span); a wrong delta scale or cap is off by 1e-3 and more.
pub const TIMELINE_TOLERANCE: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Standard,
    LowPower,
    Cinematic,
}

#[derive(Clone, Debug)]
pub struct Dump {
    pub file: String,
    pub rect: Rect,
    pub image: Rgba,
}

#[derive(Clone, Debug)]
pub struct GlDump {
    pub file: String,
    pub rect: Rect,
    pub image: Rgba,
    pub edges: Option<Vec<bool>>,
    pub mode: RenderMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Patch {
    pub rgb: [u8; 3],
    pub rect: Rect,
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub scale: f64,
    pub image: Rgba,
    /// Converted from the display's colour space, which leaves up to 1 level on a patch.
    pub normalized: bool,
    pub probes: Vec<(String, Vec<Rect>)>,
    pub texts: Vec<Rect>,
    pub masks: Vec<(String, Rect)>,
    pub canvases: Vec<Dump>,
    pub gl: Vec<GlDump>,
    pub patches: Option<Vec<Patch>>,
    /// The bytes of the `HubState` the hub last sent main with `save-state`.
    pub hub_state: Option<Vec<u8>>,
    /// `selection.getTimelineRange()` at the capture.
    pub timeline_range: Option<[f64; 2]>,
}

/// A mask that applies to every capture on one platform (`parity/masks.json`).
#[derive(Clone, Debug, PartialEq)]
pub enum PlatformMask {
    Rect(String, Rect),
    /// Pixels with alpha 0 on both sides: the vibrancy or acrylic backdrop. A pixel that is
    /// transparent on one side only is compared.
    Transparent(String),
    /// One device-pixel row, from the bottom when negative.
    DeviceRow(String, i64),
    /// One device-pixel column, from the right when negative.
    DeviceColumn(String, i64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub gate: &'static str,
    pub what: String,
    pub pass: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub checks: Vec<Check>,
    /// The window diff: reference greyed, gate 2 failures red, text-region differences orange,
    /// masked and dump-compared pixels blue.
    pub window_diff: Option<Rgba>,
    pub dump_diffs: Vec<(String, Rgba)>,
}

impl Outcome {
    pub fn pass(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.pass)
    }

    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| !c.pass)
    }

    pub fn push(
        &mut self,
        gate: &'static str,
        what: impl Into<String>,
        pass: bool,
        detail: String,
    ) {
        self.checks.push(Check {
            gate,
            what: what.into(),
            pass,
            detail,
        });
    }
}

fn edges_within(a: Rect, b: Rect, tolerance: f64) -> bool {
    let edges = |r: Rect| [r[0], r[1], r[0] + r[2], r[1] + r[3]];
    edges(a)
        .iter()
        .zip(edges(b))
        .all(|(p, q)| (p - q).abs() <= tolerance)
}

/// Boxes sorted by their top edge, to look ones near a box up in.
struct Boxes(Vec<Rect>);

impl Boxes {
    fn new(boxes: &[Rect]) -> Boxes {
        let mut boxes = boxes.to_vec();
        boxes.sort_by(|a, b| a[1].total_cmp(&b[1]));
        Boxes(boxes)
    }

    fn has(&self, near: Rect, tolerance: f64) -> bool {
        let from = self.0.partition_point(|b| b[1] < near[1] - tolerance);
        self.0[from..]
            .iter()
            .take_while(|b| b[1] <= near[1] + tolerance)
            .any(|b| edges_within(near, *b, tolerance))
    }
}

fn fmt_rect(r: Rect) -> String {
    format!("[{}, {}, {}, {}]", r[0], r[1], r[2], r[3])
}

/// Device-pixel span of a CSS rect grown by `grow` on every side, rounded outwards.
fn device_span(r: Rect, grow: f64, scale: f64, width: usize, height: usize) -> [usize; 4] {
    let clamp = |v: f64, max: usize| (v.max(0.0) as usize).min(max);
    [
        clamp(((r[0] - grow) * scale).floor(), width),
        clamp(((r[1] - grow) * scale).floor(), height),
        clamp(((r[0] + r[2] + grow) * scale).ceil(), width),
        clamp(((r[1] + r[3] + grow) * scale).ceil(), height),
    ]
}

fn fill(mask: &mut [bool], width: usize, span: [usize; 4]) {
    let [x0, y0, x1, y1] = span;
    for y in y0..y1 {
        mask[y * width + x0..y * width + x1].fill(true);
    }
}

fn rect_mask(rects: &[Rect], grow: f64, scale: f64, width: usize, height: usize) -> Vec<bool> {
    let mut mask = vec![false; width * height];
    for r in rects {
        fill(
            &mut mask,
            width,
            device_span(*r, grow, scale, width, height),
        );
    }
    mask
}

fn masked_pixels(
    reference: &Capture,
    port: &Capture,
    platform: &[PlatformMask],
    reasons: &mut Vec<String>,
) -> Vec<bool> {
    let (w, h) = (
        reference.image.width as usize,
        reference.image.height as usize,
    );
    let mut rects: Vec<Rect> = Vec::new();
    for (reason, r) in reference.masks.iter().chain(&port.masks) {
        rects.push(*r);
        reasons.push(format!("{reason} {}", fmt_rect(*r)));
    }
    let mut mask = rect_mask(&rects, 0.0, reference.scale, w, h);
    for m in platform {
        match m {
            PlatformMask::Rect(reason, r) => {
                fill(&mut mask, w, device_span(*r, 0.0, reference.scale, w, h));
                reasons.push(format!("{reason} {}", fmt_rect(*r)));
            }
            PlatformMask::Transparent(reason) => {
                let alpha = |img: &Rgba, i: usize| img.data[i * 4 + 3];
                let mut n = 0;
                for (i, m) in mask.iter_mut().enumerate() {
                    if alpha(&reference.image, i) == 0 && alpha(&port.image, i) == 0 {
                        *m = true;
                        n += 1;
                    }
                }
                reasons.push(format!("{reason} ({n} px)"));
            }
            PlatformMask::DeviceRow(reason, row) => {
                let y = if *row < 0 { h as i64 + row } else { *row };
                if (0..h as i64).contains(&y) {
                    fill(&mut mask, w, [0, y as usize, w, y as usize + 1]);
                }
                reasons.push(format!("{reason} (row {y})"));
            }
            PlatformMask::DeviceColumn(reason, column) => {
                let x = if *column < 0 {
                    w as i64 + column
                } else {
                    *column
                };
                if (0..w as i64).contains(&x) {
                    fill(&mut mask, w, [x as usize, 0, x as usize + 1, h]);
                }
                reasons.push(format!("{reason} (column {x})"));
            }
        }
    }
    mask
}

fn opaque(image: &Rgba) -> Rgb {
    pixel::composite(image, [0, 0, 0])
}

/// pixelmatch over black and over white: straight-alpha pixels that differ only in a colour
/// hidden by alpha 0 count as equal, anything visible over either background counts.
fn differing(a: &Rgba, b: &Rgba) -> Result<Vec<bool>, String> {
    let black = pixel::compare(
        &pixel::composite(a, [0, 0, 0]),
        &pixel::composite(b, [0, 0, 0]),
        [0, 0, 0],
    )?;
    let white = pixel::compare(
        &pixel::composite(a, [255, 255, 255]),
        &pixel::composite(b, [255, 255, 255]),
        [255, 255, 255],
    )?;
    Ok(black
        .mask
        .iter()
        .zip(&white.mask)
        .map(|(x, y)| *x || *y)
        .collect())
}

struct Region {
    pixels: usize,
    differing: usize,
    blob: u64,
}

fn region(diff: &[bool], include: &[bool], width: usize, height: usize) -> Region {
    let bad: Vec<bool> = diff.iter().zip(include).map(|(d, i)| *d && *i).collect();
    Region {
        pixels: include.iter().filter(|i| **i).count(),
        differing: bad.iter().filter(|b| **b).count(),
        blob: pixel::max_blob(&bad, width, height),
    }
}

fn pct(n: usize, of: usize) -> f64 {
    if of == 0 {
        0.0
    } else {
        100.0 * n as f64 / of as f64
    }
}

pub fn compare(reference: &Capture, port: &Capture, platform: &[PlatformMask]) -> Outcome {
    let mut out = Outcome::default();
    port_masks(reference, port, &mut out);
    layout(reference, port, &mut out);
    window(reference, port, platform, &mut out);
    canvases(reference, port, &mut out);
    gl(reference, port, &mut out);
    if let Some(patches) = &reference.patches {
        colour_patches(patches, reference, port, &mut out);
    }
    state(reference, port, &mut out);
    out
}

fn state(reference: &Capture, port: &Capture, out: &mut Outcome) {
    if let Some(expected) = &reference.hub_state {
        let (pass, detail) = match &port.hub_state {
            None => (false, "the port recorded no HubState".to_owned()),
            Some(actual) if actual == expected => (true, format!("{} bytes equal", expected.len())),
            Some(actual) => {
                let at = expected
                    .iter()
                    .zip(actual)
                    .position(|(a, b)| a != b)
                    .unwrap_or(expected.len().min(actual.len()));
                let context = |bytes: &[u8]| {
                    String::from_utf8_lossy(
                        &bytes[at.saturating_sub(40)..(at + 40).min(bytes.len())],
                    )
                    .into_owned()
                };
                (
                    false,
                    format!(
                        "differ at byte {at}: upstream ...{}..., port ...{}...",
                        context(expected),
                        context(actual)
                    ),
                )
            }
        };
        out.push("state", "HubState", pass, detail);
    }
    if let Some(expected) = reference.timeline_range {
        let limit = TIMELINE_TOLERANCE * (expected[1] - expected[0]);
        let off = port
            .timeline_range
            .map(|r| (r[0] - expected[0]).abs().max((r[1] - expected[1]).abs()));
        out.push(
            "state",
            "timeline range",
            off.is_some_and(|d| d <= limit),
            format!(
                "upstream {expected:?}, port {:?}, off by {off:?} (max {limit:e})",
                port.timeline_range
            ),
        );
    }
}

/// The mask on both sides that covers a dumped canvas or 3D view, if any: such a view belongs
/// to a unit whose region the scenario masks.
fn covering_mask(rect: Rect, reference: &Capture, port: &Capture) -> Option<String> {
    let inside = |m: &Rect| {
        rect[0] >= m[0] - LAYOUT_TOLERANCE
            && rect[1] >= m[1] - LAYOUT_TOLERANCE
            && rect[0] + rect[2] <= m[0] + m[2] + LAYOUT_TOLERANCE
            && rect[1] + rect[3] <= m[1] + m[3] + LAYOUT_TOLERANCE
    };
    reference
        .masks
        .iter()
        .find(|(reason, m)| inside(m) && port.masks.iter().any(|(r, p)| r == reason && inside(p)))
        .map(|(reason, m)| format!("{reason} {}", fmt_rect(*m)))
}

fn port_masks(reference: &Capture, port: &Capture, out: &mut Outcome) {
    if port.masks.is_empty() {
        return;
    }
    let unpaired: Vec<String> = port
        .masks
        .iter()
        .filter(|(_, r)| {
            !reference
                .masks
                .iter()
                .any(|(_, u)| edges_within(*u, *r, LAYOUT_TOLERANCE))
        })
        .map(|(reason, r)| format!("{reason} {}", fmt_rect(*r)))
        .collect();
    out.push(
        "5 masks",
        "port masks",
        unpaired.is_empty(),
        if unpaired.is_empty() {
            format!(
                "{} masks, each within {LAYOUT_TOLERANCE} px of an upstream mask",
                port.masks.len()
            )
        } else {
            format!("no upstream mask for {}", unpaired.join("; "))
        },
    );
}

fn layout(reference: &Capture, port: &Capture, out: &mut Outcome) {
    for (selector, expected) in &reference.probes {
        let Some((_, actual)) = port.probes.iter().find(|(s, _)| s == selector) else {
            out.push("1 layout", selector, false, "not probed on the port".into());
            continue;
        };
        if expected.len() != actual.len() {
            out.push(
                "1 layout",
                selector,
                false,
                format!(
                    "{} boxes upstream, {} on the port",
                    expected.len(),
                    actual.len()
                ),
            );
            continue;
        }
        let off: Vec<String> = expected
            .iter()
            .zip(actual)
            .filter(|(e, a)| !edges_within(**e, **a, LAYOUT_TOLERANCE))
            .map(|(e, a)| format!("{} vs {}", fmt_rect(*e), fmt_rect(*a)))
            .collect();
        out.push(
            "1 layout",
            selector,
            off.is_empty(),
            if off.is_empty() {
                format!("{} boxes within {LAYOUT_TOLERANCE} px", expected.len())
            } else {
                off.join("; ")
            },
        );
    }
}

fn window(reference: &Capture, port: &Capture, platform: &[PlatformMask], out: &mut Outcome) {
    let (ri, pi) = (&reference.image, &port.image);
    if (ri.width, ri.height) != (pi.width, pi.height) || reference.scale != port.scale {
        let detail = format!(
            "upstream {}x{} at {}x, port {}x{} at {}x",
            ri.width, ri.height, reference.scale, pi.width, pi.height, port.scale
        );
        out.push("2 pixels", "window", false, detail.clone());
        out.push("3 text", "window", false, detail);
        return;
    }
    let (w, h) = (ri.width as usize, ri.height as usize);
    let scale = reference.scale;
    let mut reasons = Vec::new();
    let masked = masked_pixels(reference, port, platform, &mut reasons);
    let dumps: Vec<Rect> = reference
        .canvases
        .iter()
        .map(|d| d.rect)
        .chain(reference.gl.iter().map(|d| d.rect))
        .collect();
    let dumped = rect_mask(&dumps, 0.0, scale, w, h);
    let visible_texts: Vec<Rect> = reference
        .texts
        .iter()
        .copied()
        .filter(|t| !centre_masked(*t, reference, &masked))
        .collect();
    let text = rect_mask(&visible_texts, TEXT_DILATE, scale, w, h);
    let diff = match differing(ri, pi) {
        Ok(d) => d,
        Err(e) => {
            out.push("2 pixels", "window", false, e);
            return;
        }
    };
    let outside: Vec<bool> = (0..w * h)
        .map(|i| !masked[i] && !dumped[i] && !text[i])
        .collect();
    let inside: Vec<bool> = (0..w * h).map(|i| !masked[i] && text[i]).collect();
    let non_text = region(&diff, &outside, w, h);
    let non_text_pct = pct(non_text.differing, non_text.pixels);
    out.push(
        "2 pixels",
        "window",
        non_text_pct <= PIXEL_MAX_PCT && non_text.blob <= PIXEL_MAX_BLOB,
        format!(
            "{non_text_pct:.4} % of {} px differ (max {PIXEL_MAX_PCT}), largest blob {} px (max {PIXEL_MAX_BLOB}); masked: {}",
            non_text.pixels,
            non_text.blob,
            if reasons.is_empty() {
                "none".to_owned()
            } else {
                reasons.join(", ")
            }
        ),
    );
    let in_text = region(&diff, &inside, w, h);
    let text_pct = pct(in_text.differing, in_text.pixels);
    out.push(
        "3 text",
        "window",
        text_pct <= TEXT_MAX_PCT,
        format!(
            "{text_pct:.3} % of {} text-region px differ (max {TEXT_MAX_PCT})",
            in_text.pixels
        ),
    );
    text_boxes(&visible_texts, reference, port, &masked, out);
    out.window_diff = Some(window_diff(ri, &diff, &outside, &inside));
}

fn centre_masked(r: Rect, reference: &Capture, masked: &[bool]) -> bool {
    let (w, h) = (reference.image.width as f64, reference.image.height as f64);
    let x = ((r[0] + r[2] / 2.0) * reference.scale).floor();
    let y = ((r[1] + r[3] / 2.0) * reference.scale).floor();
    if x < 0.0 || y < 0.0 || x >= w || y >= h {
        return false;
    }
    masked[y as usize * w as usize + x as usize]
}

fn text_boxes(
    visible: &[Rect],
    reference: &Capture,
    port: &Capture,
    masked: &[bool],
    out: &mut Outcome,
) {
    let ours: Vec<Rect> = port
        .texts
        .iter()
        .copied()
        .filter(|t| !centre_masked(*t, reference, masked))
        .collect();
    let (theirs_sorted, ours_sorted) = (Boxes::new(visible), Boxes::new(&ours));
    let mut problems = Vec::new();
    for r in visible {
        if !ours_sorted.has(*r, TEXT_BOX_TOLERANCE) {
            problems.push(format!("upstream run {} has no port run", fmt_rect(*r)));
        }
    }
    for o in &ours {
        if !theirs_sorted.has(*o, TEXT_BOX_TOLERANCE) {
            problems.push(format!("port run {} matches no upstream run", fmt_rect(*o)));
        }
    }
    let n = problems.len();
    problems.truncate(8);
    out.push(
        "3 text",
        "glyph-run boxes",
        n == 0,
        if n == 0 {
            format!("{} runs within {TEXT_BOX_TOLERANCE} px", visible.len())
        } else {
            format!("{n} unmatched: {}", problems.join("; "))
        },
    );
}

fn window_diff(reference: &Rgba, diff: &[bool], outside: &[bool], inside: &[bool]) -> Rgba {
    let rgb = opaque(reference);
    let data = rgb
        .data
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            let g = ((u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 9 + 170) as u8;
            match (diff[i], outside[i], inside[i]) {
                (true, true, _) => [255, 0, 0, 255],
                (true, _, true) => [255, 150, 0, 255],
                (_, false, false) => [g / 2, g / 2, g, 255],
                _ => [g, g, g, 255],
            }
        })
        .collect();
    Rgba {
        width: reference.width,
        height: reference.height,
        data,
    }
}

fn pair<T>(rect: Rect, candidates: &[T], rect_of: impl Fn(&T) -> Rect) -> Option<&T> {
    candidates
        .iter()
        .find(|c| edges_within(rect, rect_of(c), LAYOUT_TOLERANCE))
}

fn canvases(reference: &Capture, port: &Capture, out: &mut Outcome) {
    for dump in &reference.canvases {
        let what = format!("canvas {}", dump.file);
        if let Some(mask) = covering_mask(dump.rect, reference, port) {
            out.push("5 masks", what, true, format!("masked by {mask}"));
            continue;
        }
        let Some(ours) = pair(dump.rect, &port.canvases, |d| d.rect) else {
            out.push(
                "2 pixels",
                what,
                false,
                format!("no port canvas at {}", fmt_rect(dump.rect)),
            );
            continue;
        };
        let diff = match differing(&dump.image, &ours.image) {
            Ok(d) => d,
            Err(e) => {
                out.push("2 pixels", what, false, e);
                continue;
            }
        };
        let (w, h) = (dump.image.width as usize, dump.image.height as usize);
        let r = region(&diff, &vec![true; w * h], w, h);
        let p = pct(r.differing, r.pixels);
        out.push(
            "2 pixels",
            what,
            p <= PIXEL_MAX_PCT && r.blob <= PIXEL_MAX_BLOB,
            format!("{p:.4} % of {w}x{h} differ, largest blob {} px", r.blob),
        );
        if r.differing > 0 {
            out.dump_diffs.push((
                dump.file.clone(),
                pixel::diff_image(&opaque(&dump.image), &diff),
            ));
        }
    }
}

fn gl(reference: &Capture, port: &Capture, out: &mut Outcome) {
    for dump in &reference.gl {
        let what = format!("3D {}", dump.file);
        if let Some(mask) = covering_mask(dump.rect, reference, port) {
            out.push("5 masks", what, true, format!("masked by {mask}"));
            continue;
        }
        let Some(ours) = pair(dump.rect, &port.gl, |d| d.rect) else {
            out.push(
                "4 3D",
                what,
                false,
                format!("no port 3D view at {}", fmt_rect(dump.rect)),
            );
            continue;
        };
        if ours.mode != dump.mode {
            out.push(
                "4 3D",
                what,
                false,
                format!("upstream records {:?}, the port {:?}", dump.mode, ours.mode),
            );
            continue;
        }
        let Some(edges) = &ours.edges else {
            out.push(
                "4 3D",
                what,
                false,
                "the port readback has no edge mask".into(),
            );
            continue;
        };
        match pixel3d::gate4(&dump.image, &ours.image, edges) {
            Ok(g) => {
                let pass = if dump.mode == RenderMode::Cinematic {
                    g.cinematic_pass
                } else {
                    g.pass
                };
                out.push(
                    "4 3D",
                    what,
                    pass,
                    format!(
                        "{:?}: SSIM {:.5}, {:.3} % differ, interior max {} and {:.4} % of {} px, outlier blob {}, interior blob {}, mean dE00 {:.3}",
                        dump.mode,
                        g.ssim,
                        g.differing_pct,
                        g.interior_max_delta,
                        g.interior_differing_pct,
                        g.interior_pixels,
                        g.outlier_blob,
                        g.interior_blob,
                        g.mean_delta_e
                    ),
                );
            }
            Err(e) => out.push("4 3D", what, false, e),
        }
    }
}

fn patch_delta(image: &Rgba, scale: f64, rect: Rect, rgb: [u8; 3]) -> Option<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let span = device_span(rect, 0.0, scale, usize::MAX, usize::MAX);
    if span[2] > w || span[3] > h || span[0] >= span[2] || span[1] >= span[3] {
        return None;
    }
    let mut worst = 0;
    for y in span[1]..span[3] {
        for x in span[0]..span[2] {
            let at = (y * w + x) * 4;
            for c in 0..3 {
                worst = worst.max(image.data[at + c].abs_diff(rgb[c]));
            }
        }
    }
    Some(worst)
}

fn colour_patches(patches: &[Patch], reference: &Capture, port: &Capture, out: &mut Outcome) {
    match &port.patches {
        Some(ours) if ours == patches => {}
        Some(ours) => {
            out.push(
                "colour patch",
                "patch list",
                false,
                format!(
                    "{} upstream patches, {} on the port, lists differ",
                    patches.len(),
                    ours.len()
                ),
            );
            return;
        }
        None => {
            out.push(
                "colour patch",
                "patch list",
                false,
                "the port recorded no patches".into(),
            );
            return;
        }
    }
    out.checks.push(patch_check(
        "upstream",
        reference,
        patches,
        reference_patch_limit(reference),
    ));
    out.checks.push(patch_check("port", port, patches, 0));
}

pub fn reference_patch_limit(capture: &Capture) -> u8 {
    if capture.normalized {
        PATCH_REFERENCE_MAX
    } else {
        0
    }
}

pub fn patch_check(side: &str, capture: &Capture, patches: &[Patch], limit: u8) -> Check {
    let mut exact = 0;
    let mut worst = 0;
    let mut over = Vec::new();
    for p in patches {
        match patch_delta(&capture.image, capture.scale, p.rect, p.rgb) {
            Some(d) => {
                exact += usize::from(d == 0);
                worst = worst.max(d);
                if d > limit {
                    over.push(format!(
                        "rgb({},{},{}) off by {d}",
                        p.rgb[0], p.rgb[1], p.rgb[2]
                    ));
                }
            }
            None => over.push(format!("rect {} outside the capture", fmt_rect(p.rect))),
        }
    }
    let n = over.len();
    over.truncate(6);
    Check {
        gate: "colour patch",
        what: side.to_owned(),
        pass: n == 0 && !patches.is_empty(),
        detail: format!(
            "{exact}/{} exact, max delta {worst} (limit {limit}){}",
            patches.len(),
            if n == 0 {
                String::new()
            } else {
                format!("; {n} over: {}", over.join("; "))
            }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(width: u32, height: u32, scale: f64) -> Capture {
        Capture {
            scale,
            image: Rgba {
                width,
                height,
                data: [200, 200, 200, 255].repeat((width * height) as usize),
            },
            normalized: false,
            probes: Vec::new(),
            texts: Vec::new(),
            masks: Vec::new(),
            canvases: Vec::new(),
            gl: Vec::new(),
            patches: None,
            hub_state: None,
            timeline_range: None,
        }
    }

    fn paint(c: &mut Capture, x: usize, y: usize, rgba: [u8; 4]) {
        let i = (y * c.image.width as usize + x) * 4;
        c.image.data[i..i + 4].copy_from_slice(&rgba);
    }

    fn failed(o: &Outcome) -> Vec<(&'static str, String)> {
        o.failures().map(|c| (c.gate, c.what.clone())).collect()
    }

    #[test]
    fn layout_allows_one_px_per_edge() {
        let mut a = capture(20, 20, 1.0);
        a.probes = vec![("a".into(), vec![[2.0, 2.0, 10.0, 10.0]])];
        let mut b = a.clone();
        b.probes = vec![("a".into(), vec![[3.0, 1.0, 10.0, 10.0]])];
        assert!(compare(&a, &b, &[]).pass());
        b.probes = vec![("a".into(), vec![[3.5, 2.0, 10.0, 10.0]])];
        assert_eq!(failed(&compare(&a, &b, &[])), [("1 layout", "a".into())]);
        b.probes = vec![("a".into(), vec![[2.0, 2.0, 10.0, 10.0]; 2])];
        assert!(!compare(&a, &b, &[]).pass());
        b.probes = Vec::new();
        assert!(!compare(&a, &b, &[]).pass());
    }

    #[test]
    fn blob_over_64_fails_even_under_the_rate() {
        let a = capture(200, 200, 1.0);
        let mut b = a.clone();
        for i in 0..65 {
            paint(&mut b, 10 + i, 10, [0, 0, 0, 255]);
        }
        let o = compare(&a, &b, &[]);
        assert_eq!(failed(&o), [("2 pixels", "window".into())]);
        let mut c = a.clone();
        for i in 0..64 {
            paint(&mut c, 10 + i, 10, [0, 0, 0, 255]);
        }
        assert!(compare(&a, &c, &[]).pass());
    }

    #[test]
    fn masks_cover_their_pixels_only() {
        let a = capture(30, 20, 2.0);
        let mut b = a.clone();
        for x in 0..30 {
            paint(&mut b, x, 0, [0, 0, 0, 255]);
        }
        for y in 0..20 {
            paint(&mut b, 29, y, [0, 0, 0, 255]);
        }
        for x in 0..10 {
            paint(&mut b, x, 10, [0, 0, 0, 0]);
        }
        let windows = [
            PlatformMask::DeviceRow("border".into(), 0),
            PlatformMask::DeviceColumn("border".into(), -1),
            PlatformMask::Transparent("backdrop".into()),
        ];
        assert_eq!(
            failed(&compare(&a, &b, &windows)),
            [("2 pixels", "window".into())]
        );
        let mut backdrop = a.clone();
        for x in 0..10 {
            paint(&mut backdrop, x, 10, [90, 90, 90, 0]);
        }
        let o = compare(&backdrop, &b, &windows);
        assert!(o.pass());
        assert!(
            o.checks[0].detail.contains("backdrop (10 px)"),
            "{}",
            o.checks[0].detail
        );
        let mut c = a.clone();
        c.masks = vec![("input".into(), [0.0, 0.0, 15.0, 0.5])];
        for x in 0..30 {
            paint(&mut c, x, 0, [0, 0, 0, 255]);
        }
        let mut upstream = a.clone();
        upstream.masks = c.masks.clone();
        let mut d = c.clone();
        paint(&mut d, 5, 5, [0, 0, 0, 255]);
        assert!(compare(&upstream, &c, &[]).pass());
        assert!(compare(&upstream, &d, &[]).pass());
        let mut e = c.clone();
        for x in 0..30 {
            paint(&mut e, x, 1, [0, 0, 0, 255]);
        }
        assert!(!compare(&upstream, &e, &[]).pass());
    }

    #[test]
    fn port_masks_need_an_upstream_mask() {
        let mut a = capture(30, 20, 1.0);
        a.masks = vec![("input".into(), [0.0, 0.0, 15.0, 2.0])];
        let mut b = a.clone();
        b.masks = vec![("input".into(), [1.0, 0.0, 14.0, 3.0])];
        assert!(compare(&a, &b, &[]).pass());
        b.masks.push(("hide".into(), [0.0, 5.0, 30.0, 10.0]));
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("5 masks", "port masks".into())]
        );
    }

    #[test]
    fn text_regions_take_six_percent_and_runs_one_px() {
        let mut a = capture(100, 40, 1.0);
        a.texts = vec![[10.0, 10.0, 40.0, 16.0]];
        let mut b = a.clone();
        b.texts = vec![[11.0, 10.0, 40.0, 16.0]];
        for x in 12..40 {
            paint(&mut b, x, 15, [0, 0, 0, 255]);
        }
        let o = compare(&a, &b, &[]);
        assert!(o.pass(), "{:?}", failed(&o));
        b.texts = vec![[12.0, 10.0, 40.0, 16.0]];
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("3 text", "glyph-run boxes".into())]
        );
        b.texts = a.texts.clone();
        for y in 9..27 {
            for x in 8..30 {
                paint(&mut b, x, y, [0, 0, 0, 255]);
            }
        }
        let gates: Vec<_> = failed(&compare(&a, &b, &[]))
            .into_iter()
            .map(|f| f.0)
            .collect();
        assert_eq!(gates, ["3 text"]);
    }

    #[test]
    fn colour_patches_exact_on_the_port_within_one_upstream() {
        let mut a = capture(8, 4, 2.0);
        a.patches = Some(vec![Patch {
            rgb: [200, 200, 200],
            rect: [0.0, 0.0, 2.0, 2.0],
        }]);
        let mut b = a.clone();
        assert!(compare(&a, &b, &[]).pass());
        paint(&mut a, 3, 3, [201, 200, 199, 255]);
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("colour patch", "upstream".into())]
        );
        a.normalized = true;
        assert!(compare(&a, &b, &[]).pass());
        paint(&mut a, 3, 3, [202, 200, 200, 255]);
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("colour patch", "upstream".into())]
        );
        paint(&mut a, 3, 3, [200, 200, 200, 255]);
        paint(&mut b, 0, 0, [200, 201, 200, 255]);
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("colour patch", "port".into())]
        );
        b.patches = None;
        assert!(!compare(&a, &b, &[]).pass());
    }

    #[test]
    fn dumps_must_pair_and_3d_needs_edges() {
        let dump = Rgba {
            width: 4,
            height: 4,
            data: [10, 20, 30, 255].repeat(16),
        };
        let mut a = capture(20, 20, 1.0);
        a.canvases = vec![Dump {
            file: "c.png".into(),
            rect: [1.0, 1.0, 4.0, 4.0],
            image: dump.clone(),
        }];
        a.gl = vec![GlDump {
            file: "g.png".into(),
            rect: [10.0, 10.0, 4.0, 4.0],
            image: dump.clone(),
            edges: None,
            mode: RenderMode::Standard,
        }];
        let mut b = a.clone();
        b.gl[0].edges = Some(vec![false; 16]);
        assert!(compare(&a, &b, &[]).pass());
        b.canvases[0].rect[0] = 3.0;
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("2 pixels", "canvas c.png".into())]
        );
        b.canvases = a.canvases.clone();
        b.gl[0].mode = RenderMode::Cinematic;
        assert_eq!(failed(&compare(&a, &b, &[])), [("4 3D", "3D g.png".into())]);
        b.gl[0].mode = RenderMode::Standard;
        b.gl[0].edges = None;
        assert_eq!(failed(&compare(&a, &b, &[])), [("4 3D", "3D g.png".into())]);
    }

    #[test]
    fn a_dump_is_skipped_only_under_a_mask_both_sides_record() {
        let mut a = capture(20, 20, 1.0);
        a.canvases = vec![Dump {
            file: "c.png".into(),
            rect: [1.0, 1.0, 4.0, 4.0],
            image: Rgba {
                width: 4,
                height: 4,
                data: [10, 20, 30, 255].repeat(16),
            },
        }];
        a.masks = vec![(".renderer-content".into(), [0.0, 0.0, 10.0, 10.0])];
        let mut b = a.clone();
        b.canvases.clear();
        let o = compare(&a, &b, &[]);
        assert!(o.pass(), "{:?}", o.checks);
        assert!(
            o.checks
                .iter()
                .any(|c| c.gate == "5 masks" && c.what == "canvas c.png")
        );
        b.masks[0].0 = ".tab-bar".into();
        assert!(!compare(&a, &b, &[]).pass());
        b.masks = vec![(".renderer-content".into(), [0.0, 0.0, 3.0, 3.0])];
        assert!(!compare(&a, &b, &[]).pass());
        b.masks.clear();
        assert!(!compare(&a, &b, &[]).pass());
    }

    #[test]
    fn hub_state_bytes_must_match_and_timeline_ranges_agree() {
        let mut a = capture(8, 8, 1.0);
        a.hub_state = Some(br#"{"sidebar":{"width":362}}"#.to_vec());
        a.timeline_range = Some([0.25, 9.75]);
        let mut b = a.clone();
        assert!(compare(&a, &b, &[]).pass());
        b.hub_state = Some(br#"{"sidebar":{"width":363}}"#.to_vec());
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("state", "HubState".into())]
        );
        b.hub_state = None;
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("state", "HubState".into())]
        );
        b.hub_state = a.hub_state.clone();
        b.timeline_range = Some([0.25, f64::from_bits(9.75f64.to_bits() + 4)]);
        assert!(compare(&a, &b, &[]).pass());
        b.timeline_range = Some([0.25, 9.75 + 1e-8]);
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("state", "timeline range".into())]
        );
        b.timeline_range = None;
        assert_eq!(
            failed(&compare(&a, &b, &[])),
            [("state", "timeline range".into())]
        );
    }
}
