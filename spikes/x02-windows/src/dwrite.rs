use std::collections::HashMap;

use asrs_canvas::text::{FontSpec, Mask, Shaped, ShapedGlyph, TextBackend};
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, Interface};

use crate::shape;

#[derive(Clone, Copy, Debug)]
pub struct Variant {
    pub grid_fit: bool,
    pub cleartype: bool,
}

impl Variant {
    pub fn from_env() -> Variant {
        let flag = |k: &str, default: bool| {
            std::env::var(k)
                .map(|v| v == "1")
                .unwrap_or(default)
        };
        Variant {
            grid_fit: flag("X02_GRIDFIT", true),
            cleartype: flag("X02_CLEARTYPE", true),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FaceKey {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

pub struct Face {
    pub face: IDWriteFontFace,
    pub data: Vec<u8>,
    pub upem: u16,
    gasp: Vec<(u16, u16)>,
    typo: (f64, f64),
}

pub struct DWrite {
    factory: IDWriteFactory2,
    collection: IDWriteFontCollection,
    faces: HashMap<FaceKey, Face>,
    pub variant: Variant,
    pub default_family: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn gasp_table(data: &[u8]) -> Vec<(u16, u16)> {
    let Ok(face) = rustybuzz::ttf_parser::RawFace::parse(data, 0) else {
        return Vec::new();
    };
    let Some(gasp) = face.table(rustybuzz::ttf_parser::Tag::from_bytes(b"gasp")) else {
        return Vec::new();
    };
    let be = |i: usize| u16::from_be_bytes([gasp[i], gasp[i + 1]]);
    if gasp.len() < 4 || be(0) < 1 {
        return Vec::new();
    }
    let n = be(2) as usize;
    (0..n)
        .filter(|i| 4 + i * 4 + 4 <= gasp.len())
        .map(|i| (be(4 + i * 4), be(6 + i * 4)))
        .collect()
}

impl DWrite {
    pub fn new(variant: Variant) -> DWrite {
        unsafe {
            let factory: IDWriteFactory2 =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DWriteCreateFactory");
            let mut collection = None;
            factory
                .GetSystemFontCollection(&mut collection, false)
                .expect("system font collection");
            DWrite {
                factory,
                collection: collection.expect("collection"),
                faces: HashMap::new(),
                variant,
                default_family: "Segoe UI".to_owned(),
            }
        }
    }

    pub fn face(&mut self, key: &FaceKey) -> &Face {
        if !self.faces.contains_key(key) {
            let face = self.load(key);
            self.faces.insert(key.clone(), face);
        }
        &self.faces[key]
    }

    fn load(&self, key: &FaceKey) -> Face {
        unsafe {
            let mut index = 0;
            let mut exists = BOOL(0);
            let name = wide(&key.family);
            self.collection
                .FindFamilyName(windows::core::PCWSTR(name.as_ptr()), &mut index, &mut exists)
                .expect("FindFamilyName");
            assert!(exists.as_bool(), "no font family {}", key.family);
            let family = self.collection.GetFontFamily(index).expect("family");
            let font = family
                .GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT(i32::from(key.weight)),
                    DWRITE_FONT_STRETCH_NORMAL,
                    if key.italic {
                        DWRITE_FONT_STYLE_ITALIC
                    } else {
                        DWRITE_FONT_STYLE_NORMAL
                    },
                )
                .expect("matching font");
            let face = font.CreateFontFace().expect("face");
            let mut count = 1;
            let mut files = [None];
            face.GetFiles(&mut count, Some(files.as_mut_ptr()))
                .expect("files");
            let file = files[0].take().expect("file");
            let mut key_ptr = std::ptr::null_mut();
            let mut key_size = 0;
            file.GetReferenceKey(&mut key_ptr, &mut key_size)
                .expect("key");
            let loader: IDWriteLocalFontFileLoader =
                file.GetLoader().expect("loader").cast().expect("local loader");
            let len = loader
                .GetFilePathLengthFromKey(key_ptr, key_size)
                .expect("path length");
            let mut path = vec![0u16; len as usize + 1];
            loader
                .GetFilePathFromKey(key_ptr, key_size, &mut path)
                .expect("path");
            let path = String::from_utf16_lossy(&path[..len as usize]);
            let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
            let mut metrics = DWRITE_FONT_METRICS::default();
            face.GetMetrics(&mut metrics);
            let parsed = rustybuzz::ttf_parser::Face::parse(&data, face.GetIndex()).expect("parse");
            let os2 = parsed.tables().os2.expect("OS/2");
            let typo = (
                f64::from(os2.typographic_ascender()),
                -f64::from(os2.typographic_descender()),
            );
            eprintln!(
                "font {:?} -> {path} (index {}, upem {}, simulations {:?})",
                key,
                face.GetIndex(),
                metrics.designUnitsPerEm,
                face.GetSimulations()
            );
            Face {
                gasp: gasp_table(&data),
                face,
                data,
                upem: metrics.designUnitsPerEm,
                typo,
            }
        }
    }

    // FontSpec carries no family or style yet. The textstyles scenes use sans-serif only at sizes
    // the system-ui lines never use, so X02_TEXTSTYLES=1 recovers the family from the size.
    fn key(&self, font: &FontSpec) -> FaceKey {
        let sans = std::env::var("X02_TEXTSTYLES").is_ok_and(|v| v == "1")
            && [10.0, 13.5, 21.25, 16.0].contains(&font.size);
        FaceKey {
            family: if sans { "Arial".to_owned() } else { self.default_family.clone() },
            weight: font.weight,
            italic: sans && font.size == 16.0,
        }
    }

    /// The rendering size and mode Skia's `SkScalerContext_DW` picks for this device size.
    fn mode(&mut self, key: &FaceKey, real_size: f32) -> (f32, DWRITE_RENDERING_MODE) {
        let gdi = ((real_size * 64.0).round() / 64.0).max(1.0);
        let ppem = gdi.round() as u16;
        let face = self.face(key);
        let range = face.gasp.iter().find(|(max, _)| *max >= ppem);
        let mode = match range {
            Some((_, flags)) if flags & 8 != 0 => DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
            Some(_) => DWRITE_RENDERING_MODE_NATURAL,
            None => DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
        };
        (real_size, mode)
    }

    pub fn mask(
        &mut self,
        key: &FaceKey,
        glyph: u16,
        real_size: f32,
        xform: [f32; 4],
        sub: (f32, f32),
    ) -> Option<(Mask, usize)> {
        let (em, mode) = self.mode(key, real_size);
        let variant = self.variant;
        let face = self.face(key).face.clone();
        let (aa, texture, channels) = if variant.cleartype {
            (
                DWRITE_TEXT_ANTIALIAS_MODE_CLEARTYPE,
                DWRITE_TEXTURE_CLEARTYPE_3x1,
                3,
            )
        } else {
            (
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                DWRITE_TEXTURE_ALIASED_1x1,
                1,
            )
        };
        let advance = 0f32;
        let offset = DWRITE_GLYPH_OFFSET::default();
        let run = DWRITE_GLYPH_RUN {
            fontFace: std::mem::ManuallyDrop::new(Some(face)),
            fontEmSize: em,
            glyphCount: 1,
            glyphIndices: &glyph,
            glyphAdvances: &advance,
            glyphOffsets: &offset,
            isSideways: BOOL(0),
            bidiLevel: 0,
        };
        let matrix = DWRITE_MATRIX {
            m11: xform[0],
            m12: xform[1],
            m21: xform[2],
            m22: xform[3],
            dx: sub.0,
            dy: sub.1,
        };
        let grid = if variant.grid_fit {
            DWRITE_GRID_FIT_MODE_ENABLED
        } else {
            DWRITE_GRID_FIT_MODE_DISABLED
        };
        let mode = if variant.grid_fit {
            mode
        } else {
            DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC
        };
        let result = unsafe {
            let analysis = if variant.cleartype && variant.grid_fit {
                let f1: IDWriteFactory = self.factory.cast().ok()?;
                f1.CreateGlyphRunAnalysis(
                    &run,
                    1.0,
                    Some(&matrix),
                    mode,
                    DWRITE_MEASURING_MODE_NATURAL,
                    0.0,
                    0.0,
                )
            } else {
                self.factory.CreateGlyphRunAnalysis(
                    &run,
                    Some(&matrix),
                    mode,
                    DWRITE_MEASURING_MODE_NATURAL,
                    grid,
                    aa,
                    0.0,
                    0.0,
                )
            };
            let _ = std::mem::ManuallyDrop::into_inner(run.fontFace);
            let analysis = analysis.ok()?;
            let bounds = analysis.GetAlphaTextureBounds(texture).ok()?;
            if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
                return None;
            }
            let width = (bounds.right - bounds.left) as u32;
            let height = (bounds.bottom - bounds.top) as u32;
            let mut data = vec![0u8; (width * height) as usize * channels];
            analysis
                .CreateAlphaTexture(texture, &bounds, &mut data)
                .ok()?;
            Some(Mask {
                left: bounds.left,
                top: bounds.top,
                width,
                height,
                data,
            })
        };
        result.map(|m| (m, channels))
    }

    fn grey_mask(
        &mut self,
        key: &FaceKey,
        glyph: u32,
        real_size: f32,
        xform: [f32; 4],
        sub: (f32, f32),
    ) -> Option<Mask> {
        let (mut mask, channels) = self.mask(key, glyph as u16, real_size, xform, sub)?;
        if channels == 3 {
            mask.data = mask
                .data
                .chunks_exact(3)
                .map(|p| ((u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3) as u8)
                .collect();
        }
        Some(mask)
    }
}

impl TextBackend for DWrite {
    fn shape(&mut self, font: &FontSpec, text: &str) -> Shaped {
        let key = self.key(font);
        let size = font.size;
        let face = self.face(&key);
        let run = shape::shape(&face.data, face.face_index(), face.upem, size, text);
        Shaped {
            glyphs: run
                .glyphs
                .iter()
                .map(|g| ShapedGlyph { id: g.0, x: g.1 })
                .collect(),
            width: run.width,
        }
    }

    fn typo_extents(&mut self, font: &FontSpec) -> (f64, f64) {
        let key = self.key(font);
        self.face(&key).typo
    }

    fn raw_mask(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        scale: f64,
        x_scale: f64,
        phase: u8,
    ) -> Option<Mask> {
        let key = self.key(font);
        let real = (font.size * scale) as f32;
        self.grey_mask(
            &key,
            glyph,
            real,
            [x_scale as f32, 0.0, 0.0, 1.0],
            (f32::from(phase) / 4.0, 0.0),
        )
    }

    fn raw_mask_transformed(
        &mut self,
        font: &FontSpec,
        glyph: u32,
        matrix: [f64; 4],
        phase: (u8, u8),
    ) -> Option<Mask> {
        let key = self.key(font);
        let [a, b, c, d] = matrix;
        let vertical = c.hypot(d);
        if vertical == 0.0 {
            return None;
        }
        let real = (font.size * vertical) as f32;
        let m = [a / vertical, b / vertical, c / vertical, d / vertical];
        self.grey_mask(
            &key,
            glyph,
            real,
            [m[0] as f32, m[1] as f32, m[2] as f32, m[3] as f32],
            (f32::from(phase.0) / 4.0, f32::from(phase.1) / 4.0),
        )
    }
}

impl Face {
    pub fn face_index(&self) -> u32 {
        unsafe { self.face.GetIndex() }
    }
}
