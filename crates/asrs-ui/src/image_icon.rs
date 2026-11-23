//! A raster `<img>` with `filter: drop-shadow()`, as Chromium's GPU raster draws it: the image
//! from its mip levels, then a Gaussian shadow of its alpha under it.

use asrs_canvas::Pixmap;
use gpui_kit::{Bounds, IntoElement, Pixels, Styled, canvas, point, px};

use crate::input::skia;

include!(concat!(env!("OUT_DIR"), "/download_icons.rs"));

/// One of upstream's `icons/download` images.
pub fn download_icon(file: &str) -> Option<&'static [u8]> {
    DOWNLOAD_ICONS
        .iter()
        .find(|(name, _)| *name == file)
        .map(|(_, bytes)| *bytes)
}

/// `drop-shadow(0px 0.5px <blur> <color>)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropShadow {
    /// In CSS px. Chromium takes it as the standard deviation.
    pub blur: f64,
    pub color: u32,
}

const SHADOW_OFFSET: f64 = 0.5;

/// Skia samples mip levels half a level sharper than their size asks for.
const LOD_BIAS: f64 = 0.5;

/// Premultiplied RGBA, 0 to 255.
#[derive(Clone)]
struct Level {
    size: usize,
    pixels: Vec<[f32; 4]>,
}

impl Level {
    fn decode(png: &[u8]) -> Option<Level> {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .ok()?;
        let mut buffer = vec![0; reader.output_buffer_size()?];
        let frame = reader.next_frame(&mut buffer).ok()?;
        let size = frame.width as usize;
        let rgba = frame.color_type == png::ColorType::Rgba
            && frame.bit_depth == png::BitDepth::Eight
            && frame.height as usize == size
            && size.is_power_of_two();
        if !rgba {
            return None;
        }
        let pixels = buffer[..frame.buffer_size()]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = f32::from(p[3]);
                let c = |v: u8| f32::from(v) * a / 255.0;
                [c(p[0]), c(p[1]), c(p[2]), a]
            })
            .collect();
        Some(Level { size, pixels })
    }

    fn at(&self, x: isize, y: isize) -> [f32; 4] {
        let clamp = |v: isize| v.clamp(0, self.size as isize - 1) as usize;
        self.pixels[clamp(y) * self.size + clamp(x)]
    }

    fn halved(&self) -> Level {
        let size = self.size / 2;
        let mut pixels = Vec::with_capacity(size * size);
        for y in 0..size as isize {
            for x in 0..size as isize {
                let corners = [(0, 0), (1, 0), (0, 1), (1, 1)];
                let sum = corners.iter().fold([0.0; 4], |sum: [f32; 4], (dx, dy)| {
                    let p = self.at(2 * x + dx, 2 * y + dy);
                    std::array::from_fn(|c| sum[c] + p[c])
                });
                pixels.push(sum.map(|v| v / 4.0));
            }
        }
        Level { size, pixels }
    }

    /// Bilinear at the centre of pixel `(x, y)` of `target` pixels across.
    fn sample(&self, x: usize, y: usize, target: usize) -> [f32; 4] {
        let map = |v: usize| (v as f32 + 0.5) * self.size as f32 / target as f32 - 0.5;
        let (u, v) = (map(x), map(y));
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let [a, b, c, d] = [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(i, j)| self.at(x0 + i, y0 + j));
        std::array::from_fn(|i| {
            let top = a[i] * (1.0 - fx) + b[i] * fx;
            let bottom = c[i] * (1.0 - fx) + d[i] * fx;
            top * (1.0 - fy) + bottom * fy
        })
    }
}

fn scaled(image: Level, target: usize) -> Vec<[f32; 4]> {
    let lod = ((image.size as f64 / target as f64).log2() - LOD_BIAS).max(0.0);
    let (floor, blend) = (lod.floor() as usize, lod.fract() as f32);
    let mut level = image;
    for _ in 0..floor {
        level = level.halved();
    }
    let next = level.halved();
    let mut out = Vec::with_capacity(target * target);
    for y in 0..target {
        for x in 0..target {
            let (near, far) = (level.sample(x, y, target), next.sample(x, y, target));
            out.push(std::array::from_fn(|i| {
                near[i] * (1.0 - blend) + far[i] * blend
            }));
        }
    }
    out
}

fn gaussian(sigma: f64) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as i64;
    let weights: Vec<f64> = (-radius..=radius)
        .map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f64 = weights.iter().sum();
    weights.iter().map(|w| (w / total) as f32).collect()
}

/// `values` (`size` across) blurred along rows, and transposed.
fn blur_rows(values: &[f32], size: usize, kernel: &[f32]) -> Vec<f32> {
    let radius = kernel.len() / 2;
    let mut out = vec![0.0; values.len()];
    for y in 0..size {
        for x in 0..size {
            let mut sum = 0.0;
            for (k, weight) in kernel.iter().enumerate() {
                if let Some(at) = (x + k).checked_sub(radius).filter(|at| *at < size) {
                    sum += values[y * size + at] * weight;
                }
            }
            out[x * size + y] = sum;
        }
    }
    out
}

/// The image `target` device px across with its shadow, [`padding`] around it.
fn raster(
    png: &[u8],
    target: usize,
    invert: bool,
    shadow: DropShadow,
    scale: f64,
) -> Option<Pixmap> {
    let mut image = scaled(Level::decode(png)?, target);
    if invert {
        for p in &mut image {
            let a = p[3];
            *p = [a - p[0], a - p[1], a - p[2], a];
        }
    }
    let pad = padding(shadow, scale);
    let size = target + 2 * pad;
    let offset = (SHADOW_OFFSET * scale).floor() as usize;
    let mut alpha = vec![0.0; size * size];
    for y in 0..target {
        for x in 0..target {
            alpha[(y + pad + offset) * size + x + pad] = image[y * target + x][3] / 255.0;
        }
    }
    let kernel = gaussian(shadow.blur * scale);
    let alpha = blur_rows(&blur_rows(&alpha, size, &kernel), size, &kernel);
    let color = [16, 8, 0].map(|shift| ((shadow.color >> shift) & 0xff) as f32);
    let mut pixmap = Pixmap::new(size as u32, size as u32)?;
    let (out, _) = pixmap.data_mut().as_chunks_mut::<4>();
    for (i, out) in out.iter_mut().enumerate() {
        let (x, y) = (i % size, i / size);
        let over = match (x.checked_sub(pad), y.checked_sub(pad)) {
            (Some(x), Some(y)) if x < target && y < target => image[y * target + x],
            _ => [0.0; 4],
        };
        let under = alpha[i] * (1.0 - over[3] / 255.0);
        let pixel = [
            over[0] + color[0] * under,
            over[1] + color[1] * under,
            over[2] + color[2] * under,
            over[3] + 255.0 * under,
        ];
        *out = pixel.map(|v| v.round().clamp(0.0, 255.0) as u8);
    }
    Some(pixmap)
}

fn padding(shadow: DropShadow, scale: f64) -> usize {
    ((3.0 * shadow.blur + SHADOW_OFFSET) * scale).ceil() as usize
}

/// The image in a box `size` CSS px square on the device grid; its shadow paints outside the box.
pub fn image_icon(
    png: &'static [u8],
    size: f64,
    invert: bool,
    shadow: DropShadow,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, (), window, cx| {
            let scale = f64::from(window.scale_factor());
            let target = (size * scale).round() as usize;
            let pad = padding(shadow, scale) as f32 / window.scale_factor();
            let padded = Bounds {
                origin: point(bounds.origin.x - px(pad), bounds.origin.y - px(pad)),
                size: bounds.size,
            };
            let key = (
                "image-icon",
                png.as_ptr() as usize,
                target,
                invert,
                shadow.blur.to_bits(),
                shadow.color,
            );
            let draw = || raster(png, target, invert, shadow, scale);
            skia::paint(key, padded, draw, window, cx);
        },
    )
    .w(px(size as f32))
    .h(px(size as f32))
}
