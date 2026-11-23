// Ported from Skia and Chromium, m142 (BSD-3-Clause, Copyright 2009-2015 Google Inc. and The Chromium Authors). See THIRD_PARTY_NOTICES.md.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int};
use std::path::PathBuf;
use std::ptr;

use fontconfig_sys::{
    FcBool, FcCharSet, FcCharSetHasChar, FcConfig, FcConfigSubstitute, FcDefaultSubstitute,
    FcFontMatch, FcFontSet, FcFontSetDestroy, FcFontSort, FcMatchPattern, FcPattern,
    FcPatternAddBool, FcPatternAddDouble, FcPatternAddInteger, FcPatternAddString,
    FcPatternCreate, FcPatternDestroy, FcPatternGetBool, FcPatternGetCharSet,
    FcPatternGetInteger, FcPatternGetString, FcResult, FcResultMatch,
};

const FAMILY: &CStr = c"family";
const STYLE_WEIGHT: &CStr = c"weight";
const STYLE_WIDTH: &CStr = c"width";
const STYLE_SLANT: &CStr = c"slant";
const SCALABLE: &CStr = c"scalable";
const FONT_WRAPPER: &CStr = c"fontwrapper";
const FONT_FORMAT: &CStr = c"fontformat";
const FILE: &CStr = c"file";
const INDEX: &CStr = c"index";
const CHARSET: &CStr = c"charset";
const LANG: &CStr = c"lang";
const PIXEL_SIZE: &CStr = c"pixelsize";
const ANTIALIAS: &CStr = c"antialias";
const AUTOHINT: &CStr = c"autohint";
const HINTING: &CStr = c"hinting";
const HINT_STYLE: &CStr = c"hintstyle";
const RGBA: &CStr = c"rgba";

const SLANT_ROMAN: c_int = 0;
const SLANT_ITALIC: c_int = 100;
const WEIGHT_REGULAR: c_int = 80;
const WEIGHT_BOLD: c_int = 200;
const WIDTH_NORMAL: c_int = 100;
const RGBA_RGB: c_int = 1;

/// CSS weights and their fontconfig counterparts, as `SkFontConfigInterface_direct` maps them.
const WEIGHTS: [(f32, f32); 12] = [
    (100.0, 0.0),
    (200.0, 40.0),
    (300.0, 50.0),
    (350.0, 55.0),
    (380.0, 75.0),
    (400.0, 80.0),
    (500.0, 100.0),
    (600.0, 180.0),
    (700.0, 200.0),
    (800.0, 205.0),
    (900.0, 210.0),
    (1000.0, 215.0),
];

/// Families Skia takes for one another although fontconfig only substituted one for the other.
const METRIC_COMPATIBLE: [&[&str]; 5] = [
    &["arial", "arimo", "liberation sans"],
    &["times new roman", "tinos", "liberation serif"],
    &["courier new", "cousine", "liberation mono"],
    &["cambria", "caladea"],
    &["calibri", "carlito"],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hinting {
    None,
    Slight,
    Medium,
    Full,
}

/// What Chromium's `GetFontRenderParams` takes from fontconfig for one font.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderStyle {
    pub hinting: Hinting,
    pub autohint: bool,
    /// Text on an opaque layer is drawn with RGB-ordered LCD masks.
    pub lcd: bool,
}

#[derive(Clone, Debug)]
pub struct Match {
    pub path: PathBuf,
    /// The face index, with the named instance of a variable font in its upper half.
    pub index: u32,
    pub weight: f32,
    pub bold: bool,
    pub italic: bool,
}

struct Pattern(*mut FcPattern);

impl Drop for Pattern {
    fn drop(&mut self) {
        // SAFETY: the pattern was created by fontconfig and is owned here.
        unsafe { FcPatternDestroy(self.0) };
    }
}

impl Pattern {
    fn new() -> Option<Pattern> {
        // SAFETY: no arguments; null is the only failure.
        let pattern = unsafe { FcPatternCreate() };
        (!pattern.is_null()).then_some(Pattern(pattern))
    }

    fn string(&self, key: &CStr, value: &str) {
        if let Ok(value) = CString::new(value) {
            // SAFETY: the pattern is live, and fontconfig copies the string.
            unsafe { FcPatternAddString(self.0, key.as_ptr(), value.as_ptr().cast()) };
        }
    }

    fn integer(&self, key: &CStr, value: c_int) {
        // SAFETY: the pattern is live.
        unsafe { FcPatternAddInteger(self.0, key.as_ptr(), value) };
    }

    fn double(&self, key: &CStr, value: f64) {
        // SAFETY: the pattern is live.
        unsafe { FcPatternAddDouble(self.0, key.as_ptr(), value) };
    }

    fn scalable(&self) {
        // SAFETY: the pattern is live.
        unsafe { FcPatternAddBool(self.0, SCALABLE.as_ptr(), 1) };
    }

    fn substitute(&self) {
        // SAFETY: the pattern is live; a null config is the current one.
        unsafe {
            FcConfigSubstitute(ptr::null_mut(), self.0, FcMatchPattern);
            FcDefaultSubstitute(self.0);
        }
    }

    fn font_match(&self) -> Option<Pattern> {
        let mut result: FcResult = FcResultMatch;
        // SAFETY: the pattern is live; the match is owned by the caller.
        let found = unsafe { FcFontMatch(ptr::null_mut(), self.0, &mut result) };
        (!found.is_null()).then_some(Pattern(found))
    }

    fn sort(&self) -> Option<FontSet> {
        let mut result: FcResult = FcResultMatch;
        let config: *mut FcConfig = ptr::null_mut();
        // SAFETY: the pattern is live; the set is owned by the caller.
        let set = unsafe { FcFontSort(config, self.0, 0, ptr::null_mut(), &mut result) };
        (!set.is_null()).then_some(FontSet(set))
    }
}

struct FontSet(*mut FcFontSet);

impl Drop for FontSet {
    fn drop(&mut self) {
        // SAFETY: the set was created by fontconfig and is owned here.
        unsafe { FcFontSetDestroy(self.0) };
    }
}

impl FontSet {
    fn patterns(&self) -> &[*mut FcPattern] {
        // SAFETY: fontconfig's set holds `nfont` pattern pointers, live as long as the set.
        unsafe {
            let set = &*self.0;
            if set.nfont <= 0 || set.fonts.is_null() {
                return &[];
            }
            std::slice::from_raw_parts(set.fonts, set.nfont as usize)
        }
    }
}

/// SAFETY for the getters below: `pattern` is a live pattern, and the strings and charsets they
/// read stay owned by it.
fn get_string(pattern: *mut FcPattern, key: &CStr, n: c_int) -> Option<String> {
    let mut value = ptr::null_mut();
    unsafe {
        if FcPatternGetString(pattern, key.as_ptr(), n, &mut value) != FcResultMatch
            || value.is_null()
        {
            return None;
        }
        Some(
            CStr::from_ptr(value as *const c_char)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn get_integer(pattern: *mut FcPattern, key: &CStr) -> Option<c_int> {
    let mut value = 0;
    let result = unsafe { FcPatternGetInteger(pattern, key.as_ptr(), 0, &mut value) };
    (result == FcResultMatch).then_some(value)
}

fn get_bool(pattern: *mut FcPattern, key: &CStr) -> Option<bool> {
    let mut value: FcBool = 0;
    let result = unsafe { FcPatternGetBool(pattern, key.as_ptr(), 0, &mut value) };
    (result == FcResultMatch).then_some(value != 0)
}

fn get_charset(pattern: *mut FcPattern) -> Option<*mut FcCharSet> {
    let mut value = ptr::null_mut();
    let result = unsafe { FcPatternGetCharSet(pattern, CHARSET.as_ptr(), 0, &mut value) };
    (result == FcResultMatch && !value.is_null()).then_some(value)
}

/// `SkFontConfigInterface_direct`'s `map_ranges`, whose result is truncated to an integer.
fn map_ranges(value: f32, ranges: impl Iterator<Item = (f32, f32)> + Clone) -> f32 {
    let mut last = None;
    for (from, to) in ranges {
        if value < from {
            let Some((from0, to0)) = last else {
                return to;
            };
            let mapped: f32 = to0 + ((value - from0) * (to - to0) / (from - from0));
            return mapped.trunc();
        }
        last = Some((from, to));
    }
    last.map_or(0.0, |(_, to)| to)
}

fn is_valid(pattern: *mut FcPattern) -> bool {
    get_string(pattern, FONT_FORMAT, 0).is_some_and(|f| f == "TrueType" || f == "CFF")
        && get_string(pattern, FILE, 0).is_some_and(|f| std::fs::File::open(f).is_ok())
}

fn describe(pattern: *mut FcPattern) -> Option<Match> {
    let weight = get_integer(pattern, STYLE_WEIGHT).unwrap_or(WEIGHT_REGULAR);
    Some(Match {
        path: PathBuf::from(get_string(pattern, FILE, 0)?),
        index: get_integer(pattern, INDEX).unwrap_or(0) as u32,
        weight: map_ranges(weight as f32, WEIGHTS.iter().map(|&(css, fc)| (fc, css))).round(),
        bold: weight >= WEIGHT_BOLD,
        italic: get_integer(pattern, STYLE_SLANT).unwrap_or(SLANT_ROMAN) != SLANT_ROMAN,
    })
}

/// `IsFallbackFontAllowed`: requests any font may answer.
fn is_generic(family: &str) -> bool {
    ["", "sans", "serif", "monospace"]
        .iter()
        .any(|g| g.eq_ignore_ascii_case(family))
}

fn metric_compatible(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    METRIC_COMPATIBLE
        .iter()
        .any(|class| class.contains(&a.as_str()) && class.contains(&b.as_str()))
}

/// `SkFontConfigInterfaceDirect::matchFamilyName`: the face of `family` closest to the style, or
/// `None` where fontconfig only has an unrelated substitute.
pub fn match_family(family: &str, weight: f32, italic: bool) -> Option<Match> {
    let pattern = Pattern::new()?;
    if !family.is_empty() {
        pattern.string(FAMILY, family);
    }
    let fc_weight = map_ranges(weight, WEIGHTS.iter().copied());
    pattern.integer(STYLE_WEIGHT, fc_weight as c_int);
    pattern.integer(STYLE_WIDTH, WIDTH_NORMAL);
    pattern.integer(STYLE_SLANT, if italic { SLANT_ITALIC } else { SLANT_ROMAN });
    pattern.scalable();
    pattern.string(FONT_WRAPPER, "SFNT");
    pattern.substitute();
    let configured = get_string(pattern.0, FAMILY, 0).unwrap_or_default();
    let set = pattern.sort()?;
    let found = set.patterns().iter().copied().find(|&p| is_valid(p))?;
    if !is_generic(family) {
        let acceptable = (0..255)
            .map_while(|n| get_string(found, FAMILY, n))
            .any(|name| {
                name.eq_ignore_ascii_case(&configured)
                    || name.eq_ignore_ascii_case(family)
                    || metric_compatible(family, &name)
            });
        if !acceptable {
            return None;
        }
    }
    describe(found)
}

/// The family fontconfig picks for `family`, as `gfx::Font` names the desktop's UI font.
pub fn resolve_family(family: &str) -> Option<String> {
    let pattern = Pattern::new()?;
    pattern.scalable();
    pattern.string(FAMILY, family);
    pattern.substitute();
    get_string(pattern.font_match()?.0, FAMILY, 0)
}

/// `gfx::QueryFontconfig` with the defaults of `gfx::FontRenderParams`.
pub fn render_style(family: &str, size: f32, bold: bool, italic: bool) -> RenderStyle {
    let mut style = RenderStyle {
        hinting: Hinting::Medium,
        autohint: false,
        lcd: false,
    };
    let Some(pattern) = Pattern::new() else {
        return style;
    };
    pattern.scalable();
    if !family.is_empty() {
        pattern.string(FAMILY, family);
    }
    pattern.double(PIXEL_SIZE, f64::from(size.trunc()));
    pattern.integer(STYLE_SLANT, if italic { SLANT_ITALIC } else { SLANT_ROMAN });
    pattern.integer(STYLE_WEIGHT, if bold { WEIGHT_BOLD } else { WEIGHT_REGULAR });
    pattern.substitute();
    let Some(found) = pattern.font_match() else {
        return style;
    };
    let antialias = get_bool(found.0, ANTIALIAS).unwrap_or(true);
    style.autohint = get_bool(found.0, AUTOHINT).unwrap_or(false);
    if get_bool(found.0, HINTING) == Some(false) {
        style.hinting = Hinting::None;
    } else if let Some(hint) = get_integer(found.0, HINT_STYLE) {
        style.hinting = match hint {
            0 => Hinting::None,
            1 => Hinting::Slight,
            2 => Hinting::Medium,
            _ => Hinting::Full,
        };
    }
    style.lcd = antialias && get_integer(found.0, RGBA) == Some(RGBA_RGB);
    style
}

/// `gfx::GetFallbackFontForChar`: the first font, in fontconfig's order for a locale, that has a
/// character.
#[derive(Default)]
pub struct Fallback {
    sets: HashMap<String, Vec<(Match, FontCharSet)>>,
    owned: Vec<FontSet>,
}

struct FontCharSet(*mut FcCharSet);

// SAFETY: fontconfig's pattern and charset reference counts are atomic, the sets are owned here
// and never changed after `fallback_set` builds them.
unsafe impl Send for Fallback {}

impl Fallback {
    pub fn font_for(&mut self, c: char, locale: &str) -> Option<Match> {
        if !self.sets.contains_key(locale) {
            let list = self.fallback_set(locale).unwrap_or_default();
            self.sets.insert(locale.to_owned(), list);
        }
        self.sets[locale]
            .iter()
            // SAFETY: the charset belongs to a pattern of a set in `owned`.
            .find(|(_, set)| unsafe { FcCharSetHasChar(set.0, u32::from(c)) } != 0)
            .map(|(found, _)| found.clone())
    }

    fn fallback_set(&mut self, locale: &str) -> Option<Vec<(Match, FontCharSet)>> {
        let pattern = Pattern::new()?;
        pattern.string(LANG, locale);
        pattern.scalable();
        pattern.substitute();
        let set = pattern.sort()?;
        let list = set
            .patterns()
            .iter()
            .filter(|&&p| get_bool(p, SCALABLE) == Some(true) && is_valid(p))
            .filter_map(|&p| Some((describe(p)?, FontCharSet(get_charset(p)?))))
            .collect();
        self.owned.push(set);
        Some(list)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_map_as_skia_does() {
        let to_fc = |w| map_ranges(w, WEIGHTS.iter().copied());
        assert_eq!(to_fc(200.0), 40.0);
        assert_eq!(to_fc(400.0), 80.0);
        assert_eq!(to_fc(450.0), 90.0);
        assert_eq!(to_fc(1000.0), 215.0);
        assert_eq!(to_fc(1.0), 0.0);
    }

    #[test]
    fn metric_compatible_families_stand_in_for_each_other() {
        assert!(metric_compatible("Arial", "Liberation Sans"));
        assert!(!metric_compatible("Arial", "Liberation Serif"));
        assert!(!metric_compatible("Helvetica", "Liberation Sans"));
    }
}
