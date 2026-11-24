// Ported from Blink and Skia, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors and Google Inc.). See THIRD_PARTY_NOTICES.md.

use icu_properties::CodePointSetData;
use icu_properties::props::EmojiPresentation;

#[cfg(any(target_os = "linux", windows))]
use super::FontSpec;

/// The clusters of `text`, which starts at UTF-16 offset `from`, each with whether Blink's
/// `FontFallbackPriority` makes it emoji presentation: an `Emoji_Presentation` base, or a
/// sequence with VS16, a skin tone modifier, a keycap or a ZWJ; VS15 forces text.
pub(crate) fn emoji_clusters(text: &str, from: isize) -> Vec<(isize, isize, bool)> {
    let presentation = CodePointSetData::new::<EmojiPresentation>();
    let mut clusters: Vec<(isize, isize, bool)> = Vec::new();
    let mut at = from;
    let mut joined = false;
    for c in text.chars() {
        let units = c.len_utf16() as isize;
        let extends = matches!(c, '\u{fe0e}' | '\u{fe0f}' | '\u{200d}' | '\u{20e3}')
            || ('\u{1f3fb}'..='\u{1f3ff}').contains(&c)
            || ('\u{e0020}'..='\u{e007f}').contains(&c);
        match clusters.last_mut() {
            Some(last) if last.1 == at && (extends || joined) => {
                last.1 = at + units;
                match c {
                    '\u{fe0e}' => last.2 = false,
                    '\u{fe0f}' | '\u{200d}' | '\u{20e3}' => last.2 = true,
                    _ if extends => last.2 = true,
                    _ => {}
                }
            }
            _ => clusters.push((at, at + units, presentation.contains(c))),
        }
        joined = c == '\u{200d}';
        at += units;
    }
    clusters
}

/// Blink's `CachingWordShapeIterator` for space-normalized text: every space is a word of its
/// own and shapes apart from its neighbours.
pub(crate) fn words(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = if rest.starts_with(' ') {
            1
        } else {
            rest.find(' ').unwrap_or(rest.len())
        };
        let (word, tail) = rest.split_at(end);
        rest = tail;
        Some(word)
    })
}

/// HarfBuzz's `em_mult` scaling of whole font units to 16.16 at Blink's font scale, the size
/// truncated to 16.16.
pub(crate) fn hb_scale(units: i64, upem: u32, size: f32) -> i32 {
    let x_scale = (size * 65536.0) as i64;
    let x_mult = (x_scale << 16) / i64::from(upem);
    ((units * x_mult + 32768) >> 16) as i32
}

/// Skia's `SkScalarRoundToScalar`.
pub(crate) fn round(v: f32) -> f32 {
    (v + 0.5).floor()
}

/// `FontDescription::EffectiveFontSize`: Blink floors a font's zoomed size to 1/100 px.
#[cfg(any(target_os = "linux", windows))]
pub(crate) fn effective(spec: &FontSpec, zoom: f32) -> FontSpec {
    let zoomed = spec.size * zoom;
    let floored = (zoomed * 100.0).floor() / 100.0;
    if floored == zoomed {
        return *spec;
    }
    FontSpec {
        size: floored / zoom,
        ..*spec
    }
}
