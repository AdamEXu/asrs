// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright 2006-2012 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! The families `GetFallbackFamily` and `GetFallbackFamilyNameFromHardcodedChoices` try for a
//! character, in order, for English content.

use icu_properties::props::Script;
use icu_properties::{CodePointMapData, CodePointSetData, props::Emoji};

pub const EMOJI_LOCALE: &str = "und-Zsye";
pub const MONO_EMOJI_LOCALE: &str = "und-Zsym";
/// Blink's `kChineseSimplified`, which is not.
const HAN_LOCALE: &str = "zh-Hant";
const LOCALE: &str = "en";

const COLOR_EMOJI: &[&str] = &["Segoe UI Emoji", "Segoe UI Symbol"];
const MONO_EMOJI: &[&str] = &["Segoe UI Symbol", "Segoe UI Emoji"];
const MATH: &[&str] = &["Cambria Math", "Segoe UI Symbol", "Code2000"];
const SYMBOL: &[&str] = &["Segoe UI Symbol"];

/// How Blink asks for a fallback font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    Text,
    Emoji,
    EmojiText,
}

impl Priority {
    pub fn of(c: char, emoji: bool) -> Priority {
        if emoji {
            Priority::Emoji
        } else if CodePointSetData::new::<Emoji>().contains(c) {
            Priority::EmojiText
        } else {
            Priority::Text
        }
    }
}

/// The families to choose from and what is tried after the one that is present.
pub struct Families {
    /// The first of these that is installed is the only one tried.
    pub candidates: &'static [&'static str],
    /// The family tried where none of the candidates is installed.
    pub last_resort: Option<&'static str>,
    /// The pan-Unicode fonts, each tried after.
    pub wide_coverage: &'static [&'static str],
}

const CJK: &[&str] = &[
    "arial unicode ms",
    "ms pgothic",
    "simsun",
    "gulim",
    "pmingliu",
    "wenquanyi zen hei",
    "ar pl shanheisun uni",
    "ar pl zenkai uni",
    "han nom a",
    "code2000",
];

const COMMON: &[&str] = &[
    "tahoma",
    "arial unicode ms",
    "lucida sans unicode",
    "microsoft sans serif",
    "palatino linotype",
    "dejavu serif",
    "dejavu sasns",
    "freeserif",
    "freesans",
    "gentium",
    "gentiumalt",
    "ms pgothic",
    "simsun",
    "gulim",
    "pmingliu",
    "code2000",
];

/// `GetFontBasedOnUnicodeBlock`.
fn by_block(c: u32) -> Option<&'static [&'static str]> {
    Some(match c {
        0x1f600..=0x1f64f | 0x1f100..=0x1f1ff => MONO_EMOJI,
        0x1f0a0..=0x1f0ff
        | 0x2600..=0x26ff
        | 0x2b00..=0x2bff
        | 0x1f300..=0x1f5ff
        | 0x1f680..=0x1f6ff
        | 0x1f700..=0x1f77f
        | 0x2700..=0x27bf
        | 0x10330..=0x1034f => SYMBOL,
        0x2190..=0x21ff
        | 0x2200..=0x22ff
        | 0x2300..=0x23ff
        | 0x25a0..=0x25ff
        | 0x27c0..=0x27ef
        | 0x27f0..=0x27ff
        | 0x2900..=0x297f
        | 0x2980..=0x29ff
        | 0x2a00..=0x2aff
        | 0x1d400..=0x1d7ff
        | 0x1ee00..=0x1eeff
        | 0x1f780..=0x1f7ff => MATH,
        _ => return None,
    })
}

/// `GetScriptBasedOnUnicodeBlock`.
fn script_by_block(c: u32) -> Script {
    match c {
        0x3000..=0x303f => Script::Han,
        0x3040..=0x30ff => Script::Katakana,
        0x0600..=0x06ff => Script::Arabic,
        0x0e00..=0x0e7f => Script::Thai,
        0x0370..=0x03ff => Script::Greek,
        0x0900..=0x097f => Script::Devanagari,
        0x0530..=0x058f => Script::Armenian,
        0x10a0..=0x10ff => Script::Georgian,
        0x0c80..=0x0cff => Script::Kannada,
        0x10330..=0x1034f => Script::Gothic,
        _ => Script::Common,
    }
}

fn script(c: char) -> Script {
    let code = u32::from(c);
    if 0xff00 < code && code < 0xff5f {
        return Script::Han;
    }
    match CodePointMapData::<Script>::new().get(c) {
        Script::Common | Script::Inherited => script_by_block(code),
        script => script,
    }
}

/// `InitializeScriptFontMap`, with the Han ideographs of a system whose locale does not
/// disambiguate them.
fn by_script(script: Script) -> &'static [&'static str] {
    match script {
        Script::Arabic => &["Tahoma", "Segoe UI"],
        Script::Armenian => &["Segoe UI", "Sylfaen"],
        Script::Bengali => &["Nirmala UI", "Vrinda"],
        Script::Brahmi => &["Segoe UI Historic"],
        Script::Braille => &["Segoe UI Symbol"],
        Script::Buginese => &["Leelawadee UI"],
        Script::CanadianAboriginal => &["Gadugi", "Euphemia"],
        Script::Carian => &["Segoe UI Historic"],
        Script::Cherokee => &["Gadugi", "Plantagenet"],
        Script::Coptic => &["Segoe UI Symbol"],
        Script::Cuneiform => &["Segoe UI Historic"],
        Script::Cypriot => &["Segoe UI Historic"],
        Script::Cyrillic => &["Times New Roman"],
        Script::Deseret => &["Segoe UI Symbol"],
        Script::Devanagari => &["Nirmala UI", "Mangal"],
        Script::EgyptianHieroglyphs => &["Segoe UI Historic"],
        Script::Ethiopic => &[
            "Nyala",
            "Abyssinica SIL",
            "Ethiopia Jiret",
            "Visual Geez Unicode",
            "GF Zemen Unicode",
            "Ebrima",
        ],
        Script::Georgian => &["Sylfaen", "Segoe UI"],
        Script::Glagolitic | Script::Gothic => &["Segoe UI Historic", "Segoe UI Symbol"],
        Script::Greek => &["Times New Roman"],
        Script::Gujarati => &["Nirmala UI", "Shruti"],
        Script::Gurmukhi => &["Nirmala UI", "Raavi"],
        Script::Hangul => &["Noto Sans KR", "Noto Sans CJK KR", "Malgun Gothic", "Gulim"],
        Script::Hebrew => &["David", "Segoe UI"],
        Script::Hiragana | Script::Katakana => &[
            "Noto Sans JP",
            "Noto Sans CJK JP",
            "Meiryo",
            "Yu Gothic",
            "MS PGothic",
            "Microsoft YaHei",
        ],
        Script::ImperialAramaic
        | Script::InscriptionalPahlavi
        | Script::InscriptionalParthian
        | Script::Kharoshthi
        | Script::Lycian
        | Script::Lydian
        | Script::OldPersian
        | Script::OldSouthArabian
        | Script::Shavian => &["Segoe UI Historic"],
        Script::Javanese => &["Javanese Text"],
        Script::Kannada => &["Tunga", "Nirmala UI"],
        Script::Khmer => &[
            "Leelawadee UI",
            "Khmer UI",
            "Khmer OS",
            "MoolBoran",
            "DaunPenh",
        ],
        Script::Lao => &[
            "Leelawadee UI",
            "Lao UI",
            "DokChampa",
            "Saysettha OT",
            "Phetsarath OT",
            "Code2000",
        ],
        Script::Latin => &["Times New Roman"],
        Script::Lisu => &["Segoe UI"],
        Script::Malayalam => &["Nirmala UI", "Kartika"],
        Script::MeeteiMayek | Script::OlChiki | Script::SoraSompeng => &["Nirmala UI"],
        Script::MeroiticCursive | Script::Ogham | Script::OldItalic | Script::OldTurkic => {
            &["Segoe UI Historic", "Segoe UI Symbol"]
        }
        Script::Mongolian => &["Mongolian Baiti"],
        Script::Myanmar => &["Myanmar Text", "Padauk", "Parabaik", "Myanmar3", "Code2000"],
        Script::NewTaiLue => &["Microsoft New Tai Lue"],
        Script::Nko | Script::Osmanya | Script::Tifinagh | Script::Vai => &["Ebrima"],
        Script::Oriya => &["Kalinga", "ori1Uni", "Lohit Oriya", "Nirmala UI"],
        Script::PhagsPa => &["Microsoft PhagsPa"],
        Script::Runic => &["Segoe UI Historic", "Segoe UI Symbol"],
        Script::Han => &[
            "Noto Sans SC",
            "Noto Sans CJK SC",
            "Microsoft YaHei",
            "simsun",
        ],
        Script::Sinhala => &["Iskoola Pota", "AksharUnicode", "Nirmala UI"],
        Script::Syriac => &["Estrangelo Edessa", "Estrangelo Nisibin", "Code2000"],
        Script::TaiLe => &["Microsoft Tai Le"],
        Script::Tamil => &["Nirmala UI", "Latha"],
        Script::Telugu => &["Nirmala UI", "Gautami"],
        Script::Thaana => &["MV Boli"],
        Script::Thai => &["Tahoma", "Leelawadee UI", "Leelawadee"],
        Script::Tibetan => &["Microsoft Himalaya", "Jomolhari", "Tibetan Machine Uni"],
        Script::Bopomofo => &[
            "Noto Sans TC",
            "Noto Sans CJK TC",
            "Microsoft JhengHei",
            "pmingli",
        ],
        Script::Yi => &["Microsoft Yi Baiti", "Nuosu SIL", "Code2000"],
        _ => &[],
    }
}

pub fn families(c: char, priority: Priority, monospace: bool) -> Families {
    let symbols = match priority {
        Priority::Emoji => Some(COLOR_EMOJI),
        Priority::EmojiText => Some(MONO_EMOJI),
        Priority::Text => by_block(u32::from(c)),
    };
    if let Some(candidates) = symbols {
        return Families {
            candidates,
            last_resort: None,
            wide_coverage: COMMON,
        };
    }
    let script = script(c);
    let code = u32::from(c);
    let last_resort = match code >> 16 {
        1 => "code2001",
        2 if !(0x2ebf0..=0x2ee5f).contains(&code) => "simsun-extb",
        2 | 3 => "simsun-extg",
        _ => "lucida sans unicode",
    };
    Families {
        candidates: if code <= 0xffff {
            match script {
                Script::Arabic | Script::Hebrew if monospace => &["courier new"],
                _ => by_script(script),
            }
        } else {
            &[]
        },
        last_resort: Some(last_resort),
        wide_coverage: if script == Script::Han { CJK } else { COMMON },
    }
}

/// `FallbackLocaleForCharacter`.
pub fn locale(c: char, priority: Priority) -> &'static str {
    match priority {
        Priority::Emoji => EMOJI_LOCALE,
        Priority::EmojiText => MONO_EMOJI_LOCALE,
        Priority::Text if CodePointMapData::<Script>::new().get(c) == Script::Han => HAN_LOCALE,
        Priority::Text => LOCALE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_reach_blinks_families() {
        let first = |c: char, emoji: bool| {
            let families = families(c, Priority::of(c, emoji), false);
            let first = families.candidates.first().copied();
            first.or(families.last_resort).unwrap()
        };
        assert_eq!(first('\u{1f916}', true), "Segoe UI Emoji");
        assert_eq!(first('\u{2699}', false), "Segoe UI Symbol");
        assert_eq!(first('\u{221e}', false), "Cambria Math");
        assert_eq!(first('日', false), "Noto Sans SC");
        assert_eq!(first('か', false), "Noto Sans JP");
        assert_eq!(first('！', false), "Noto Sans SC");
        assert_eq!(first('한', false), "Noto Sans KR");
        assert_eq!(first('Ω', false), "Times New Roman");
        assert_eq!(first('\u{20000}', false), "simsun-extb");
        assert_eq!(first('\u{2192}', false), "Cambria Math");
        assert_eq!(first('\u{2603}', false), "Segoe UI Symbol");
        let monospace = families('א', Priority::Text, true);
        assert_eq!(monospace.candidates, ["courier new"]);
        assert_eq!(locale('日', Priority::Text), "zh-Hant");
    }
}
