//! The Field2d renderer's `IMAGE`: a field asset's PNG decoded as Chromium decodes it for a
//! canvas, with straight alpha premultiplied and an embedded ICC profile converted to sRGB.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::SystemTime;

use asrs_canvas::Pixmap;
use asrs_canvas::color_profile::IccTransform;
use asrs_canvas::pixels::pixmap_from_straight_rgba;
use asrs_js::string::JsString;
use asrs_js::string::uri::decode_uri_component;

pub struct FieldImage {
    /// `[naturalWidth, naturalHeight]`.
    pub size: [f64; 2],
    pub pixmap: Arc<Pixmap>,
}

/// The file an asset's `path` names: `encodePath` URI-encodes each component.
fn file_path(src: &JsString) -> Option<PathBuf> {
    let decoded = decode_uri_component(src).ok()?;
    Some(PathBuf::from(decoded.to_string_lossy()))
}

thread_local! {
    /// The images something still shows, so that the windows and tabs of one field share its
    /// pixels.
    static LOADED: RefCell<HashMap<PathBuf, Loaded>> = RefCell::default();
}

struct Loaded {
    version: (u64, Option<SystemTime>),
    image: Weak<FieldImage>,
}

/// `None` where the image fails to load.
pub fn load(src: &JsString) -> Option<FieldImage> {
    decode(BufReader::new(File::open(file_path(src)?).ok()?))
}

/// [`load`], or the same image where something still shows it.
pub fn load_shared(src: &JsString) -> Option<Rc<FieldImage>> {
    let path = file_path(src)?;
    let file = File::open(&path).ok()?;
    let version = file
        .metadata()
        .map_or((0, None), |m| (m.len(), m.modified().ok()));
    LOADED.with_borrow_mut(|loaded| {
        loaded.retain(|_, l| l.image.strong_count() > 0);
        if let Some(image) = loaded
            .get(&path)
            .filter(|l| l.version == version)
            .and_then(|l| l.image.upgrade())
        {
            return Some(image);
        }
        let image = Rc::new(decode(BufReader::new(file))?);
        let entry = Loaded {
            version,
            image: Rc::downgrade(&image),
        };
        loaded.insert(path, entry);
        Some(image)
    })
}

fn decode(png: impl BufRead + Seek) -> Option<FieldImage> {
    let mut decoder = png::Decoder::new(png);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let transform = reader
        .info()
        .icc_profile
        .as_deref()
        .and_then(IccTransform::from_icc);
    let decoded = reader.output_buffer_size()?;
    let (width, height) = reader.info().size();
    let mut buffer = vec![0; decoded.max(rgba_len(width, height)?)];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let (width, height) = (frame.width, frame.height);
    buffer.truncate(rgba_len(width, height)?);
    match frame.color_type {
        png::ColorType::Rgba => {}
        png::ColorType::Rgb => expand(&mut buffer, |[r, g, b]| [r, g, b, 255]),
        png::ColorType::GrayscaleAlpha => expand(&mut buffer, |[v, a]| [v, v, v, a]),
        png::ColorType::Grayscale => expand(&mut buffer, |[v]| [v, v, v, 255]),
        png::ColorType::Indexed => return None,
    }
    let pixmap = match transform {
        Some(transform) => transform.pixmap(width, height, buffer)?,
        None => pixmap_from_straight_rgba(width, height, buffer)?,
    };
    Some(FieldImage {
        size: [f64::from(width), f64::from(height)],
        pixmap: Arc::new(pixmap),
    })
}

fn rgba_len(width: u32, height: u32) -> Option<usize> {
    (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)
}

/// Spreads the `N` byte pixels packed at the start of `buffer` over it as RGBA.
fn expand<const N: usize>(buffer: &mut [u8], rgba: impl Fn([u8; N]) -> [u8; 4]) {
    for i in (0..buffer.len() / 4).rev() {
        let pixel = std::array::from_fn(|k| buffer[i * N + k]);
        buffer[i * 4..i * 4 + 4].copy_from_slice(&rgba(pixel));
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io::Cursor;

    use png::{BitDepth, ColorType, Encoder, Info};

    use super::*;

    fn adobe_rgb() -> Vec<u8> {
        let mut icc = vec![0u8; 128];
        icc[16..20].copy_from_slice(b"RGB ");
        icc[20..24].copy_from_slice(b"XYZ ");
        let xyz =
            |v: [u32; 3]| [b"XYZ \0\0\0\0".to_vec(), v.map(u32::to_be_bytes).concat()].concat();
        let curve = b"curv\0\0\0\0\0\0\0\x01\x02\x33".to_vec();
        let tags = [
            (b"rXYZ", xyz([0x9c18, 0x4fa5, 0x04fc])),
            (b"gXYZ", xyz([0x348d, 0xa02c, 0x0f95])),
            (b"bXYZ", xyz([0x2631, 0x102f, 0xbe9c])),
            (b"rTRC", curve.clone()),
            (b"gTRC", curve.clone()),
            (b"bTRC", curve),
        ];
        icc.extend((tags.len() as u32).to_be_bytes());
        let mut offset = 132 + 12 * tags.len();
        for (name, body) in &tags {
            icc.extend_from_slice(*name);
            icc.extend((offset as u32).to_be_bytes());
            icc.extend((body.len() as u32).to_be_bytes());
            offset += body.len();
        }
        icc.extend(tags.iter().flat_map(|(_, body)| body));
        icc
    }

    const SIZE: (u32, u32) = (7, 5);

    fn png(color: ColorType, depth: BitDepth, icc: Option<&[u8]>, data: &[u8]) -> Vec<u8> {
        let mut info = Info::with_size(SIZE.0, SIZE.1);
        info.color_type = color;
        info.bit_depth = depth;
        info.icc_profile = icc.map(Cow::Borrowed);
        if color == ColorType::Indexed {
            info.palette = Some(Cow::Owned((0..48).map(|v| v * 5).collect()));
            info.trns = Some(Cow::Owned((0..16).map(|v| v * 17).collect()));
        }
        let mut out = Vec::new();
        let mut writer = Encoder::with_info(&mut out, info)
            .unwrap()
            .write_header()
            .unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    }

    fn straight(color: ColorType, depth: BitDepth, data: &[u8]) -> Vec<[u8; 4]> {
        let bytes = if depth == BitDepth::Sixteen { 2 } else { 1 };
        data.chunks(color.samples() * bytes)
            .map(|p| {
                let p: Vec<u8> = p.iter().step_by(bytes).copied().collect();
                match (color, &p[..]) {
                    (ColorType::Rgba, &[r, g, b, a]) => [r, g, b, a],
                    (ColorType::Rgb, &[r, g, b]) => [r, g, b, 255],
                    (ColorType::GrayscaleAlpha, &[v, a]) => [v, v, v, a],
                    (ColorType::Grayscale, &[v]) => [v, v, v, 255],
                    (ColorType::Indexed, &[i]) => {
                        let at = |k: u8| (i * 3 + k) * 5;
                        [at(0), at(1), at(2), i * 17]
                    }
                    _ => unreachable!("{color:?}"),
                }
            })
            .collect()
    }

    #[test]
    fn every_kind_of_png_decodes_to_its_premultiplied_pixels() {
        let icc = adobe_rgb();
        let transform = IccTransform::from_icc(&icc).expect("Adobe RGB converts");
        let count = (SIZE.0 * SIZE.1) as usize;
        for (color, depth) in [
            (ColorType::Rgba, BitDepth::Eight),
            (ColorType::Rgb, BitDepth::Eight),
            (ColorType::GrayscaleAlpha, BitDepth::Eight),
            (ColorType::Grayscale, BitDepth::Eight),
            (ColorType::Rgba, BitDepth::Sixteen),
            (ColorType::Rgb, BitDepth::Sixteen),
            (ColorType::Indexed, BitDepth::Eight),
        ] {
            let bytes = if depth == BitDepth::Sixteen { 2 } else { 1 };
            let wrap = if color == ColorType::Indexed { 16 } else { 256 };
            let data: Vec<u8> = (0..count * color.samples() * bytes)
                .map(|i| ((i * 37 + 11) % wrap) as u8)
                .collect();
            let rgba = straight(color, depth, &data);
            for converted in [false, true] {
                let bytes = png(color, depth, converted.then_some(&icc[..]), &data);
                let image = decode(Cursor::new(bytes)).expect("decodes");
                let want: Vec<u8> = if converted {
                    rgba.iter().flat_map(|&p| transform.convert(p)).collect()
                } else {
                    rgba.iter()
                        .flat_map(|&p| asrs_canvas::pixels::premultiply(p))
                        .collect()
                };
                assert_eq!(image.size, [7.0, 5.0]);
                assert_eq!(image.pixmap.data(), want, "{color:?} {depth:?} {converted}");
            }
        }
    }

    #[test]
    fn one_image_is_shared_while_something_holds_it() {
        let path =
            std::env::temp_dir().join(format!("asrs-field-image-{}.png", std::process::id()));
        let src = JsString::from(path.to_string_lossy().as_ref());
        let data = |v: u8| vec![v; (SIZE.0 * SIZE.1) as usize];
        let write = |data: &[u8]| {
            std::fs::write(
                &path,
                png(ColorType::Grayscale, BitDepth::Eight, None, data),
            )
            .unwrap();
        };
        write(&data(9));
        let first = load_shared(&src).expect("loads");
        let second = load_shared(&src).expect("loads");
        assert!(Rc::ptr_eq(&first, &second));
        assert!(load(&src).is_some_and(|image| !Arc::ptr_eq(&image.pixmap, &first.pixmap)));

        let mut changed = data(9);
        changed[3] = 200;
        write(&changed);
        let third = load_shared(&src).expect("loads");
        assert!(!Rc::ptr_eq(&first, &third));
        assert_ne!(first.pixmap.data(), third.pixmap.data());

        let pixels = Arc::downgrade(&third.pixmap);
        drop(third);
        assert!(pixels.upgrade().is_none());
        std::fs::remove_file(&path).unwrap();
        assert!(load_shared(&src).is_none());
    }
}
