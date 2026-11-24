// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011-2014 Google Inc.). See THIRD_PARTY_NOTICES.md.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};

use rustybuzz::ttf_parser::Tag;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_AXIS_VALUE, DWRITE_FONT_METRICS, DWRITE_FONT_SIMULATIONS,
    DWRITE_FONT_SIMULATIONS_BOLD, DWRITE_FONT_SIMULATIONS_NONE, DWRITE_FONT_SIMULATIONS_OBLIQUE,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_REGULAR, IDWriteFactory2, IDWriteFactory5,
    IDWriteFactory6, IDWriteFont, IDWriteFontFace, IDWriteFontFace2, IDWriteFontFace5,
    IDWriteFontFace7, IDWriteFontFamily, IDWriteFontFile, IDWriteFontFileStream,
    IDWriteInMemoryFontFileLoader, IDWriteLocalFontFileLoader,
};
use windows::core::Interface;

use super::settings::wide;

/// `SkFontStyle::isBold`.
const SEMI_BOLD: i32 = 600;

/// One range of a `gasp` table, its sizes inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GaspRange {
    pub min: i32,
    pub max: i32,
    pub version: u16,
    pub flags: u16,
}

impl GaspRange {
    const GRIDFIT: u16 = 1;
    pub const SYMMETRIC_SMOOTHING: u16 = 8;

    fn gridfit_only(&self) -> bool {
        self.flags == GaspRange::GRIDFIT
    }
}

/// One font face, as `DWriteFontTypeface` and `HarfBuzzFace` hold it.
pub struct Typeface {
    pub face: IDWriteFontFace,
    pub shaper: rustybuzz::Face<'static>,
    pub metrics: DWRITE_FONT_METRICS,
    pub glyph_count: u16,
    pub bold: bool,
    pub italic: bool,
    pub family: String,
    /// The face, if it has colour glyphs, with its COLR version 1 interface where there is one.
    pub color: bool,
    pub paint: Option<IDWriteFontFace7>,
    pub typo: Option<(f32, f32)>,
    pub hinted: bool,
    pub cbdt: bool,
    pub path: Option<PathBuf>,
    gasp: Vec<(u16, u16)>,
    gasp_version: Option<u16>,
    strikes: Vec<u16>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    let bytes = data.get(at..at + 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn gasp_ranges(table: &[u8]) -> Option<(u16, Vec<(u16, u16)>)> {
    let version = u16_at(table, 0).filter(|&v| v <= 1)?;
    let count = u16_at(table, 2).filter(|&n| n <= 1024)?;
    let ranges = (0..usize::from(count))
        .map(|i| Some((u16_at(table, 4 + i * 4)?, u16_at(table, 6 + i * 4)?)))
        .collect::<Option<_>>()?;
    Some((version, ranges))
}

/// The square sizes `has_bitmap_strike` counts: the `EBLC` strikes it takes to have several glyphs
/// and every `EBSC` one.
fn strike_sizes(eblc: Option<&[u8]>, ebsc: Option<&[u8]>) -> Vec<u16> {
    const VERSION: u32 = 0x0002_0000;
    let sizes = |table: &[u8], record: usize| {
        (u32_at(table, 0) == Some(VERSION))
            .then(|| u32_at(table, 4))
            .flatten()
            .filter(|&n| n <= 1024 && table.len() >= 8 + record * n as usize)
            .map(|n| (0..n as usize).map(|i| 8 + i * record).collect::<Vec<_>>())
    };
    let mut out = Vec::new();
    let mut add = |table: &[u8], at: usize| {
        if table[at] == table[at + 1] {
            out.push(u16::from(table[at]));
        }
    };
    if let Some(table) = eblc {
        let Some(records) = sizes(table, 48) else {
            return Vec::new();
        };
        for at in records {
            // Skia compares the glyph range without swapping its bytes.
            let raw = |at: usize| u16_at(table, at).map_or(0, |v| u32::from(v.swap_bytes()));
            if raw(at + 42) >= raw(at + 40) + 3 {
                add(table, at + 44);
            }
        }
    } else {
        return Vec::new();
    }
    if let Some(table) = ebsc {
        for at in sizes(table, 28).unwrap_or_default() {
            add(table, at + 24);
        }
    }
    out
}

impl Typeface {
    pub fn new(factory: &IDWriteFactory2, font: &IDWriteFont) -> Option<Typeface> {
        // SAFETY: plain COM calls on live objects; `GetMetrics` writes the struct it is given.
        let (face, metrics, weight, style, family) = unsafe {
            let face = font.CreateFontFace().ok()?;
            let mut metrics = DWRITE_FONT_METRICS::default();
            face.GetMetrics(&mut metrics);
            let family = family_name(&font.GetFontFamily().ok()?);
            (face, metrics, font.GetWeight(), font.GetStyle(), family)
        };
        let FontFile { data, path, memory } = font_data(factory, &face)?;
        let face = memory
            .and_then(|file| private_face(factory, &face, file))
            .unwrap_or(face);
        // SAFETY: plain COM calls on a live face.
        let (index, glyph_count) = unsafe { (face.GetIndex(), face.GetGlyphCount()) };
        let mut shaper = rustybuzz::Face::from_slice(data, index)?;
        shaper.set_variations(&variations(&face));
        let raw = shaper.raw_face();
        let table = |tag: &[u8; 4]| raw.table(Tag::from_bytes(tag));
        let gasp = table(b"gasp").and_then(gasp_ranges);
        let color_face = face.cast::<IDWriteFontFace2>().ok();
        // SAFETY: a plain COM call on a live face.
        let color = color_face.is_some_and(|f| unsafe { f.IsColorFont() }.as_bool());
        Some(Typeface {
            glyph_count,
            metrics,
            bold: weight.0 >= SEMI_BOLD,
            italic: style != DWRITE_FONT_STYLE_NORMAL,
            family,
            color,
            paint: face.cast().ok().filter(|_| color),
            typo: table(b"OS/2").and_then(|os2| {
                let ascender = f32::from(u16_at(os2, 68)? as i16);
                Some((ascender, -f32::from(u16_at(os2, 70)? as i16)))
            }),
            hinted: table(b"maxp").is_some_and(|maxp| {
                u32_at(maxp, 0) == Some(0x0001_0000)
                    && u16_at(maxp, 26).is_some_and(|instructions| instructions != 0)
            }),
            cbdt: table(b"CBDT").is_some(),
            path,
            gasp_version: gasp.as_ref().map(|g| g.0),
            gasp: gasp.map(|g| g.1).unwrap_or_default(),
            strikes: strike_sizes(table(b"EBLC"), table(b"EBSC")),
            face,
            shaper,
        })
    }

    pub fn gasp_range(&self, size: i32) -> Option<GaspRange> {
        let mut min = -1;
        for &(max, flags) in &self.gasp {
            let max = i32::from(max);
            if min < size && size <= max {
                return Some(GaspRange {
                    min: min + 1,
                    max,
                    version: self.gasp_version?,
                    flags,
                });
            }
            min = max;
        }
        None
    }

    /// Whether text of `size` px uses the font's bitmaps.
    pub fn has_bitmap_strike(&self, size: i32) -> bool {
        let (min, max) = match self.gasp_range(size) {
            Some(range) if range.gridfit_only() => (range.min, range.max),
            _ => (size, size),
        };
        self.strikes
            .iter()
            .any(|&ppem| (min..=max).contains(&i32::from(ppem)))
    }

    /// `BitmapGlyphsBlockList`.
    pub fn avoids_bitmaps(&self) -> bool {
        matches!(self.family.as_str(), "Calibri" | "Courier New")
    }

    pub fn has_char(&self, c: char) -> bool {
        self.shaper.glyph_index(c).is_some_and(|g| g.0 != 0)
    }
}

/// The family's English name, or its first.
pub fn family_name(family: &IDWriteFontFamily) -> String {
    // SAFETY: the buffers hold the lengths DirectWrite reports plus the terminator.
    unsafe {
        let Ok(names) = family.GetFamilyNames() else {
            return String::new();
        };
        let (mut index, mut exists) = (0, false.into());
        let found = names.FindLocaleName(windows::core::w!("en-us"), &mut index, &mut exists);
        if found.is_err() || !exists.as_bool() {
            index = 0;
        }
        let Ok(length) = names.GetStringLength(index) else {
            return String::new();
        };
        let mut name = vec![0u16; length as usize + 1];
        if names.GetString(index, &mut name).is_err() {
            return String::new();
        }
        wide(&name)
    }
}

fn axis_values(face: &IDWriteFontFace) -> Vec<DWRITE_FONT_AXIS_VALUE> {
    let Ok(face) = face.cast::<IDWriteFontFace5>() else {
        return Vec::new();
    };
    // SAFETY: `values` holds the number of axes DirectWrite reports.
    unsafe {
        if !face.HasVariations().as_bool() {
            return Vec::new();
        }
        let count = face.GetFontAxisValueCount() as usize;
        let mut values = vec![DWRITE_FONT_AXIS_VALUE::default(); count];
        if face.GetFontAxisValues(&mut values).is_err() {
            return Vec::new();
        }
        values
    }
}

/// What sets a variable font's instances in one file apart.
pub fn axes(face: &IDWriteFontFace) -> Vec<(u32, u32)> {
    axis_values(face)
        .iter()
        .map(|v| (v.axisTag.0, v.value.to_bits()))
        .collect()
}

fn variations(face: &IDWriteFontFace) -> Vec<rustybuzz::Variation> {
    axis_values(face)
        .iter()
        .map(|v| rustybuzz::Variation {
            tag: Tag::from_bytes(&v.axisTag.0.to_le_bytes()),
            value: v.value,
        })
        .collect()
}

/// The face over the process's own copy of its file. For local files DirectWrite takes scaled
/// outlines from the Font Cache service, whose points round differently from the ones it scales
/// itself. Chromium's sandboxed renderers get font files from the browser process
/// (`DWriteFontProxy`) and never see the service.
fn private_face(
    factory: &IDWriteFactory2,
    face: &IDWriteFontFace,
    file: IDWriteFontFile,
) -> Option<IDWriteFontFace> {
    let axes = axis_values(face);
    // SAFETY: plain COM calls on live objects.
    unsafe {
        let (index, simulations) = (face.GetIndex(), face.GetSimulations());
        if axes.is_empty() {
            return factory
                .CreateFontFace(face.GetType(), &[Some(file)], index, simulations)
                .ok();
        }
        let factory: IDWriteFactory6 = factory.cast().ok()?;
        factory
            .CreateFontFaceReference(&file, index, simulations, &axes)
            .and_then(|reference| reference.CreateFontFace())
            .and_then(|face| face.cast())
            .ok()
    }
}

fn memory_file(
    factory: &IDWriteFactory2,
    data: &'static [u8],
    stream: &IDWriteFontFileStream,
) -> Option<IDWriteFontFile> {
    static LOADER: OnceLock<Option<IDWriteInMemoryFontFileLoader>> = OnceLock::new();
    let loader = LOADER.get_or_init(|| {
        // SAFETY: plain COM calls on live objects. The loader is registered once with the shared
        // factory and stays.
        unsafe {
            let factory: IDWriteFactory5 = factory.cast().ok()?;
            let loader = factory.CreateInMemoryFontFileLoader().ok()?;
            factory.RegisterFontFileLoader(&loader).ok()?;
            Some(loader)
        }
    });
    let length = u32::try_from(data.len()).ok()?;
    // SAFETY: `data` is never released, and DirectWrite holds `stream` for it.
    unsafe {
        loader
            .as_ref()?
            .CreateInMemoryFontFileReference(factory, data.as_ptr().cast(), length, stream)
            .ok()
    }
}

#[derive(Clone)]
struct FontFile {
    data: &'static [u8],
    path: Option<PathBuf>,
    memory: Option<IDWriteFontFile>,
}

type FontFiles = HashMap<Vec<u8>, (FontFile, IDWriteFontFileStream)>;

/// A font file's bytes: DirectWrite's own mapping, held once per process because faces borrow it
/// forever.
fn font_data(factory: &IDWriteFactory2, face: &IDWriteFontFace) -> Option<FontFile> {
    static FILES: Mutex<Option<FontFiles>> = Mutex::new(None);
    // SAFETY: the reference key, the path and the mapped fragment are read for the lengths
    // DirectWrite reports. The fragment is never released and its stream is kept, so the mapping
    // outlives every borrow.
    unsafe {
        let mut count = 1;
        let mut files: [Option<IDWriteFontFile>; 1] = [None];
        face.GetFiles(&mut count, Some(files.as_mut_ptr())).ok()?;
        let file = files[0].take()?;
        let (mut key, mut length): (*mut c_void, u32) = (std::ptr::null_mut(), 0);
        file.GetReferenceKey(&mut key, &mut length).ok()?;
        let id = std::slice::from_raw_parts(key.cast::<u8>(), length as usize).to_vec();
        let mut known = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let known = known.get_or_insert_default();
        if let Some((file, _)) = known.get(&id) {
            return Some(file.clone());
        }
        let loader = file.GetLoader().ok()?;
        let path = loader
            .cast::<IDWriteLocalFontFileLoader>()
            .ok()
            .and_then(|local| {
                let chars = local.GetFilePathLengthFromKey(key, length).ok()?;
                let mut path = vec![0u16; chars as usize + 1];
                local.GetFilePathFromKey(key, length, &mut path).ok()?;
                Some(PathBuf::from(wide(&path)))
            });
        let stream = loader.CreateStreamFromKey(key, length).ok()?;
        let size = stream.GetFileSize().ok()?;
        let (mut start, mut context) = (std::ptr::null_mut(), std::ptr::null_mut());
        stream
            .ReadFileFragment(&mut start, 0, size, &mut context)
            .ok()?;
        let data = std::slice::from_raw_parts(start.cast::<u8>().cast_const(), size as usize);
        let file = FontFile {
            memory: memory_file(factory, data, &stream),
            data,
            path,
        };
        known.insert(id, (file.clone(), stream));
        Some(file)
    }
}

/// `FirstMatchingFontWithoutSimulations`.
pub fn match_style(family: &IDWriteFontFamily, weight: f32, italic: bool) -> Option<IDWriteFont> {
    let mut weight = DWRITE_FONT_WEIGHT(weight as i32);
    let mut style = if italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };
    loop {
        // SAFETY: plain COM calls on a live family and font.
        let (font, simulations) = unsafe {
            let font = family
                .GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, style)
                .ok()?;
            let simulations = font.GetSimulations();
            (font, simulations)
        };
        let plain = weight == DWRITE_FONT_WEIGHT_REGULAR && style == DWRITE_FONT_STYLE_NORMAL;
        if simulations == DWRITE_FONT_SIMULATIONS_NONE || plain || has_bitmap_data(&font) {
            return Some(font);
        }
        let has = |s: DWRITE_FONT_SIMULATIONS| simulations.0 & s.0 != 0;
        if has(DWRITE_FONT_SIMULATIONS_BOLD) {
            weight = DWRITE_FONT_WEIGHT_REGULAR;
        } else if has(DWRITE_FONT_SIMULATIONS_OBLIQUE) {
            style = DWRITE_FONT_STYLE_NORMAL;
        } else {
            return Some(font);
        }
    }
}

fn has_bitmap_data(font: &IDWriteFont) -> bool {
    const EBDT: u32 = u32::from_le_bytes(*b"EBDT");
    // SAFETY: the table is released with the context it was read with.
    unsafe {
        let Ok(face) = font.CreateFontFace() else {
            return false;
        };
        let (mut data, mut size) = (std::ptr::null_mut(), 0);
        let (mut context, mut exists) = (std::ptr::null_mut(), false.into());
        let read = face.TryGetFontTable(EBDT, &mut data, &mut size, &mut context, &mut exists);
        if read.is_err() || !exists.as_bool() {
            return false;
        }
        face.ReleaseFontTable(context);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::settings::TextSettings;
    use super::super::{DirectWrite, FontSpec};
    use super::*;

    #[test]
    fn gasp_tables_parse() {
        let table = [0, 1, 0, 2, 0, 8, 0, 10, 0xff, 0xff, 0, 15];
        assert_eq!(gasp_ranges(&table), Some((1, vec![(8, 10), (0xffff, 15)])));
        assert_eq!(gasp_ranges(&[0, 2, 0, 0]), None);
        assert_eq!(gasp_ranges(&[0, 1, 0, 1, 0, 8]), None);
    }

    fn is_local(face: &IDWriteFontFace) -> bool {
        let (mut count, mut files) = (1, [None]);
        // SAFETY: plain COM calls on live objects; `files` has room for the one file.
        let loader = unsafe {
            face.GetFiles(&mut count, Some(files.as_mut_ptr())).unwrap();
            files[0].take().unwrap().GetLoader().unwrap()
        };
        loader.cast::<IDWriteLocalFontFileLoader>().is_ok()
    }

    fn backend() -> DirectWrite {
        DirectWrite::with_settings(Some(TextSettings::DEFAULT)).unwrap()
    }

    fn local_face(backend: &DirectWrite, family: &str) -> Option<IDWriteFontFace> {
        let (mut index, mut exists) = (0, false.into());
        // SAFETY: plain COM calls on a live collection.
        unsafe {
            let collection = &backend.collection;
            let name = windows::core::HSTRING::from(family);
            collection
                .FindFamilyName(&name, &mut index, &mut exists)
                .unwrap();
            let family = exists
                .as_bool()
                .then(|| collection.GetFontFamily(index).unwrap())?;
            let font = match_style(&family, 700.0, false).unwrap();
            Some(font.CreateFontFace().unwrap())
        }
    }

    #[test]
    fn faces_bypass_the_font_cache_service() {
        let mut checked = 0;
        // The second backend comes after the first one's factory is released.
        for _ in 0..2 {
            let mut backend = backend();
            for family in [
                "Arial",
                "Segoe UI",
                "Segoe UI Emoji",
                "Cambria Math",
                "Microsoft YaHei",
                "Bahnschrift",
            ] {
                let Some(local) = local_face(&backend, family) else {
                    continue;
                };
                let typeface = backend.match_family(family, 700.0, false).unwrap();
                let face = &backend.typefaces[typeface].face;
                assert!(is_local(&local) && !is_local(face), "{family}");
                // SAFETY: plain COM calls on live faces.
                let identity = |f: &IDWriteFontFace| unsafe {
                    (
                        f.GetIndex(),
                        f.GetGlyphCount(),
                        f.GetSimulations(),
                        f.GetType(),
                        axes(f),
                    )
                };
                assert_eq!(identity(&local), identity(face), "{family}");
                checked += 1;
            }
            let spec = FontSpec::parse("bold 12px system-ui").unwrap();
            for c in ['日', 'か', '\u{2699}', '\u{1f916}', 'ไ', '\u{16a0}'] {
                backend.fallback_file(&spec, c, false);
            }
            assert!(backend.typefaces.iter().all(|t| !is_local(&t.face)));
        }
        assert!(checked > 0);
    }

    #[test]
    fn instances_of_a_variable_font_stay_apart() {
        let mut backend = backend();
        let Some(regular) = backend.match_family("Bahnschrift", 400.0, false) else {
            return;
        };
        let bold = backend.match_family("Bahnschrift", 700.0, false).unwrap();
        let weight = u32::from_le_bytes(*b"wght");
        for (typeface, want) in [(regular, 400.0), (bold, 700.0)] {
            let typeface = &backend.typefaces[typeface];
            let axes = axis_values(&typeface.face);
            let axis = axes.iter().find(|v| v.axisTag.0 == weight).unwrap();
            assert_eq!(axis.value, want);
            assert_eq!(typeface.bold, want >= 600.0);
        }
        assert_eq!(
            backend.match_family("Bahnschrift", 700.0, false),
            Some(bold)
        );
    }

    #[test]
    fn only_square_strikes_with_glyphs_count() {
        let mut eblc = vec![0, 2, 0, 0, 0, 0, 0, 3];
        for (last, x, y) in [(0x0104u16, 12u8, 12u8), (0x0100, 13, 13), (0x0104, 14, 7)] {
            let mut record = [0u8; 48];
            record[42..44].copy_from_slice(&last.to_be_bytes());
            record[44] = x;
            record[45] = y;
            eblc.extend(record);
        }
        assert_eq!(strike_sizes(Some(&eblc), None), [12]);
        assert!(strike_sizes(None, None).is_empty());
    }
}
