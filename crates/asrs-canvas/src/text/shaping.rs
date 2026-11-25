// Ported from Blink and Skia, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors and Google Inc.). See THIRD_PARTY_NOTICES.md.

use icu_properties::CodePointSetData;
use icu_properties::props::EmojiPresentation;
#[cfg(any(target_os = "linux", windows))]
use icu_properties::props::Script;

use super::{FontSpec, effective_size};

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

/// The byte ranges of `text` that `ScriptRunIterator` keeps together: characters of one script
/// and the common ones after, or before the first.
#[cfg(any(target_os = "linux", windows))]
pub(crate) fn script_runs(text: &str) -> Vec<(usize, usize)> {
    use icu_properties::script::ScriptWithExtensions;
    let scripts = ScriptWithExtensions::new();
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<Vec<Script>> = None;
    for (at, c) in text.char_indices() {
        let end = at + c.len_utf8();
        let of: Vec<Script> = scripts
            .get_script_extensions_val(c)
            .iter()
            .map(|s| {
                if s == Script::Katakana {
                    Script::Hiragana
                } else {
                    s
                }
            })
            .collect();
        let neutral = of
            .iter()
            .any(|&s| s == Script::Common || s == Script::Inherited);
        let shared: Vec<Script> = match &current {
            Some(scripts) if !neutral => {
                scripts.iter().copied().filter(|s| of.contains(s)).collect()
            }
            _ => Vec::new(),
        };
        match runs.last_mut() {
            Some(run) if neutral || current.is_none() || !shared.is_empty() => run.1 = end,
            _ => runs.push((at, end)),
        }
        if !neutral {
            current = Some(if shared.is_empty() { of } else { shared });
        }
    }
    runs
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

/// `spec` at its `FontDescription::EffectiveFontSize`.
pub(crate) fn effective(spec: &FontSpec, zoom: f32) -> FontSpec {
    let zoomed = spec.size * zoom;
    let floored = effective_size(zoomed);
    if floored == zoomed {
        return *spec;
    }
    FontSpec {
        size: floored / zoom,
        ..*spec
    }
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod tests {
    use super::*;

    #[test]
    fn scripts_split_and_common_characters_stay() {
        let runs = |text: &str| -> Vec<String> {
            script_runs(text)
                .into_iter()
                .map(|(from, to)| text[from..to].to_owned())
                .collect()
        };
        assert_eq!(runs("漢字かな交じり"), ["漢字", "かな", "交", "じり"]);
        assert_eq!(runs("カーかー"), ["カーかー"]);
        assert_eq!(runs("+4.37s"), ["+4.37s"]);
        assert_eq!(runs("Δt=5"), ["Δ", "t=5"]);
        assert_eq!(runs("café"), ["café"]);
        assert!(runs("").is_empty());
    }
}
