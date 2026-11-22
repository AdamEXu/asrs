//! The Field2d renderer's `IMAGE`: a field asset's PNG decoded as Chromium decodes it for a
//! canvas, with straight alpha premultiplied and an embedded ICC profile converted to sRGB.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;

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

/// `None` where the image fails to load.
pub fn load(src: &JsString) -> Option<FieldImage> {
    let file = File::open(file_path(src)?).ok()?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let transform = reader
        .info()
        .icc_profile
        .as_deref()
        .and_then(IccTransform::from_icc);
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let (width, height) = (frame.width, frame.height);
    let pixels = &buffer[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    let pixmap = match transform {
        Some(transform) => transform.pixmap(width, height, &rgba)?,
        None => pixmap_from_straight_rgba(width, height, &rgba)?,
    };
    Some(FieldImage {
        size: [f64::from(width), f64::from(height)],
        pixmap: Arc::new(pixmap),
    })
}
