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
