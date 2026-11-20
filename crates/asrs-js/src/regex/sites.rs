//! One function per upstream regex use. Each is a direct scan equivalent to running the
//! literal through V8; the tests below hold each against `RegExp` on random inputs.

use std::sync::LazyLock;

use super::RegExp;
use crate::string::JsString;

const SLASH: u16 = b'/' as u16;
const BACKSLASH: u16 = b'\\' as u16;

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn piece(s: &[u16]) -> JsString {
    JsString::from_units(s.to_vec())
}

fn is_digit(u: u16) -> bool {
    (u16::from(b'0')..=u16::from(b'9')).contains(&u)
}

fn is_lower_hex(u: u16) -> bool {
    is_digit(u) || (u16::from(b'a')..=u16::from(b'f')).contains(&u)
}

fn starts_with_at(s: &[u16], at: usize, prefix: &[u16]) -> bool {
    s.get(at..at + prefix.len()) == Some(prefix)
}

/// `download.ts`: `part.replace(/^0+/, "")`.
pub fn strip_leading_zeros(s: &JsString) -> JsString {
    let u = s.units();
    let n = u.iter().take_while(|&&c| c == u16::from(b'0')).count();
    piece(&u[n..])
}

/// `shared/util.ts` `shiftColor`: `color.slice(1).match(/.{1,2}/g)`.
pub fn pairs_of_units(s: &JsString) -> Option<Vec<JsString>> {
    let mut out = Vec::new();
    for run in s.units().split(|&u| super::parse::is_line_terminator(u)) {
        out.extend(run.chunks(2).map(piece));
    }
    (!out.is_empty()).then_some(out)
}

/// `ConsoleRenderer.formatValue`:
/// `value.replace(/\\(?:033|x1b|x1B|e)(?=\[[0-9;:]*[a-zA-Z])/g, "\x1b")`.
pub fn console_escapes(s: &JsString) -> JsString {
    let u = s.units();
    let is_param = |c: u16| is_digit(c) || c == u16::from(b';') || c == u16::from(b':');
    let is_letter = |c: u16| c < 0x80 && (c as u8).is_ascii_alphabetic();
    let mut out = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        if u[i] == BACKSLASH {
            let name_len = ["033", "x1b", "x1B", "e"]
                .iter()
                .find(|name| starts_with_at(u, i + 1, &units(name)))
                .map(|name| name.len());
            if let Some(len) = name_len {
                let j = i + 1 + len;
                if u.get(j) == Some(&u16::from(b'[')) {
                    let params = u[j + 1..].iter().take_while(|&&c| is_param(c)).count();
                    if u.get(j + 1 + params).is_some_and(|&c| is_letter(c)) {
                        out.push(0x1b);
                        i = j;
                        continue;
                    }
                }
            }
        }
        out.push(u[i]);
        i += 1;
    }
    JsString::from_units(out)
}

/// `ConsoleRenderer.formatValue`: `normalized.replace(/\\r\\n|\\n/g, "\n")`.
pub fn console_newlines(s: &JsString) -> JsString {
    let u = s.units();
    let crlf = units("\\r\\n");
    let lf = units("\\n");
    let mut out = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        if starts_with_at(u, i, &crlf) {
            out.push(0x0a);
            i += crlf.len();
        } else if starts_with_at(u, i, &lf) {
            out.push(0x0a);
            i += lf.len();
        } else {
            out.push(u[i]);
            i += 1;
        }
    }
    JsString::from_units(out)
}

/// `ConsoleRenderer.formatValue`: `html.split(/(<[^>]*>)/g)`.
pub fn split_tags(s: &JsString) -> Vec<JsString> {
    let u = s.units();
    let mut out = Vec::new();
    let mut p = 0;
    let mut q = 0;
    while q < u.len() {
        if u[q] == u16::from(b'<') {
            if let Some(close) = u[q + 1..].iter().position(|&c| c == u16::from(b'>')) {
                let end = q + 1 + close + 1;
                out.push(piece(&u[p..q]));
                out.push(piece(&u[q..end]));
                p = end;
                q = end;
                continue;
            }
            break;
        }
        q += 1;
    }
    out.push(piece(&u[p..]));
    out
}

/// `LogKeyUtils.SCHEMA_REGEX` (`/(?:^|\/)\.schema\/(.*)/`): `isSchema` is `.is_some()`,
/// `getSchemaType` is the value.
pub fn schema_type(key: &JsString) -> Option<JsString> {
    let u = key.units();
    let marker = units(".schema/");
    let start = (0..u.len()).find_map(|i| {
        if i == 0 && starts_with_at(u, 0, &marker) {
            Some(marker.len())
        } else if u[i] == SLASH && starts_with_at(u, i + 1, &marker) {
            Some(i + 1 + marker.len())
        } else {
            None
        }
    })?;
    let len = u[start..]
        .iter()
        .take_while(|&&c| !super::parse::is_line_terminator(c))
        .count();
    Some(piece(&u[start..start + len]))
}

/// `LogKeyUtils.splitLogKey`: `key.split(SEPARATOR_REGEX)` (`/\/|:/`), or with `phoenix`,
/// `SEPARATOR_REGEX_PHOENIX` (`/\/|:|_/`).
pub fn split_key(key: &JsString, phoenix: bool) -> Vec<JsString> {
    key.units()
        .split(|&c| c == SLASH || c == u16::from(b':') || (phoenix && c == u16::from(b'_')))
        .map(piece)
        .collect()
}

/// `LogKeyUtils.MERGE_PREFIX_REGEX` (`/^\/?Log\d+/`): `isMergePrefix` is `.is_some()`,
/// `extractMergePrefix` is the value or `""`.
pub fn merge_prefix(key: &JsString) -> Option<JsString> {
    let u = key.units();
    let start = usize::from(u.first() == Some(&SLASH));
    if !starts_with_at(u, start, &units("Log")) {
        return None;
    }
    let digits = u[start + 3..].iter().take_while(|&&c| is_digit(c)).count();
    (digits > 0).then(|| piece(&u[..start + 3 + digits]))
}

/// `CSVDecoder`: `/^([0123456789abcdef]{2}-)+[0123456789abcdef]{2}(-\.\.\.)?$/.test(valueRaw)`.
pub fn is_csv_bytes(s: &JsString) -> bool {
    let mut u = s.units();
    let ellipsis = units("-...");
    if u.ends_with(&ellipsis) {
        u = &u[..u.len() - ellipsis.len()];
    }
    u.len() >= 5
        && u.len() % 3 == 2
        && u.iter().enumerate().all(|(i, &c)| {
            if i % 3 == 2 {
                c == u16::from(b'-')
            } else {
                is_lower_hex(c)
            }
        })
}

/// `CSVDecoder`: `/^[0123456789abcdef]{2}$/.test(valueRaw)`.
pub fn is_csv_byte(s: &JsString) -> bool {
    let u = s.units();
    u.len() == 2 && u.iter().all(|&c| is_lower_hex(c))
}

/// The pattern in `VideoProcessor.getVideoId`.
pub const YOUTUBE_PATTERN: &str =
    r#"(?:youtube\.com\/(?:[^\/]+\/.+\/|(?:v|e(?:mbed)?)\/|.*[?&]v=)|youtu\.be\/)([^"&?\/\s]{11})"#;

static YOUTUBE: LazyLock<RegExp> = LazyLock::new(|| {
    RegExp::new(YOUTUBE_PATTERN, "").expect("the YouTube pattern is in the supported subset")
});

/// `VideoProcessor.getVideoId`: `url.match(regex)?.[1]`.
pub fn youtube_video_id(url: &JsString) -> Option<JsString> {
    let u = url.units();
    YOUTUBE.exec(u, &mut 0)?.group(u, 1)
}

/// `VideoProcessor`: `/, [0-9]+x[0-9]+/.exec(text)?.index`.
pub fn dimensions_index(text: &JsString) -> Option<usize> {
    let u = text.units();
    (0..u.len()).find(|&i| {
        if !starts_with_at(u, i, &units(", ")) {
            return false;
        }
        let digits = u[i + 2..].iter().take_while(|&&c| is_digit(c)).count();
        let x = i + 2 + digits;
        digits > 0
            && u.get(x) == Some(&u16::from(b'x'))
            && u.get(x + 1).is_some_and(|&c| is_digit(c))
    })
}

/// `VideoProcessor`: `/[^0-9]/.exec(text)?.index`.
pub fn first_non_digit(text: &JsString) -> Option<usize> {
    text.units().iter().position(|&c| !is_digit(c))
}

/// `assetUtil` robot sort: `/^\d/.test(name)`.
pub fn starts_with_digit(s: &JsString) -> bool {
    s.units().first().is_some_and(|&c| is_digit(c))
}

/// `main.ts` recent unit presets: `text.replace(/(^\w|\s\w|\/\w)/g, (m) => m.toUpperCase())`.
pub fn capitalize_words(s: &JsString) -> JsString {
    let u = s.units();
    let word = |i: usize| u.get(i).is_some_and(|&c| super::parse::is_word_unit(c));
    let space = |c: u16| super::parse::is_space_unit(c);
    let mut out = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        let len = if i == 0 && word(0) {
            1
        } else if (space(u[i]) || u[i] == SLASH) && word(i + 1) {
            2
        } else {
            0
        };
        if len == 0 {
            out.push(u[i]);
            i += 1;
        } else {
            out.extend_from_slice(piece(&u[i..i + len]).to_upper_case().units());
            i += len;
        }
    }
    JsString::from_units(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    const TOKENS: &[&str] = &[
        "0",
        "00",
        "1",
        "9",
        "a",
        "f",
        "F",
        "g",
        "x",
        "e",
        "z",
        "Z",
        "_",
        "-",
        "/",
        ":",
        ";",
        ".",
        "...",
        "-...",
        "\\",
        "\\n",
        "\\r\\n",
        "\\033",
        "\\x1b",
        "\\x1B",
        "\\e",
        "[",
        "[0;31m",
        "[1:2",
        "<",
        ">",
        "<b>",
        "</span>",
        " ",
        "\t",
        "\n",
        "\r",
        "\u{a0}",
        "\u{2028}",
        "\u{3000}",
        "\u{feff}",
        "Log",
        "/Log",
        "Log12",
        ".schema/",
        "/.schema/",
        "schema",
        ", ",
        ", 1920x1080",
        "x",
        "youtube.com/",
        "youtu.be/",
        "watch?v=",
        "&v=",
        "?v=",
        "embed/",
        "v/",
        "e/",
        "dQw4w9WgXcQ",
        "\"",
        "?",
        "&",
        "é",
        "😀",
        "ab-cd",
        "0f-",
        "Phoenix6",
    ];

    fn random_string(rng: &mut Rng) -> JsString {
        let mut out = Vec::new();
        for _ in 0..rng.below(9) {
            match rng.below(12) {
                0 => out.push(0xd800 + rng.below(0x800) as u16),
                _ => out.extend(TOKENS[rng.below(TOKENS.len())].encode_utf16()),
            }
        }
        JsString::from_units(out)
    }

    fn check<T: PartialEq + std::fmt::Debug>(
        pattern: &str,
        flags: &str,
        site: impl Fn(&JsString) -> T,
        engine: impl Fn(&RegExp, &[u16]) -> T,
    ) {
        let re = RegExp::new(pattern, flags).unwrap();
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ pattern.len() as u64);
        for _ in 0..20_000 {
            let s = random_string(&mut rng);
            assert_eq!(site(&s), engine(&re, s.units()), "{pattern} on {s:?}");
        }
    }

    fn first(re: &RegExp, s: &[u16], group: usize) -> Option<JsString> {
        re.exec(s, &mut 0)?.group(s, group)
    }

    #[test]
    fn sites_agree_with_engine() {
        check("^0+", "", strip_leading_zeros, |re, s| re.replace(s, &[]));
        check(".{1,2}", "g", pairs_of_units, |re, s| re.match_global(s));
        check(
            r"\\(?:033|x1b|x1B|e)(?=\[[0-9;:]*[a-zA-Z])",
            "g",
            console_escapes,
            |re, s| re.replace(s, &[0x1b]),
        );
        check(r"\\r\\n|\\n", "g", console_newlines, |re, s| {
            re.replace(s, &[0x0a])
        });
        check("(<[^>]*>)", "g", split_tags, |re, s| {
            re.split(s, None).into_iter().map(Option::unwrap).collect()
        });
        check(r"(?:^|\/)\.schema\/(.*)", "", schema_type, |re, s| {
            first(re, s, 1)
        });
        check(
            r"\/|:",
            "",
            |s| split_key(s, false),
            |re, s| re.split(s, None).into_iter().map(Option::unwrap).collect(),
        );
        check(
            r"\/|:|_",
            "",
            |s| split_key(s, true),
            |re, s| re.split(s, None).into_iter().map(Option::unwrap).collect(),
        );
        check(r"^\/?Log\d+", "", merge_prefix, |re, s| first(re, s, 0));
        check(
            r"^([0123456789abcdef]{2}-)+[0123456789abcdef]{2}(-\.\.\.)?$",
            "",
            is_csv_bytes,
            |re, s| re.test(s),
        );
        check("^[0123456789abcdef]{2}$", "", is_csv_byte, |re, s| {
            re.test(s)
        });
        check(", [0-9]+x[0-9]+", "", dimensions_index, |re, s| {
            re.exec(s, &mut 0).map(|m| m.index())
        });
        check("[^0-9]", "", first_non_digit, |re, s| {
            re.exec(s, &mut 0).map(|m| m.index())
        });
        check(r"^\d", "", starts_with_digit, |re, s| re.test(s));
        check(r"(^\w|\s\w|\/\w)", "g", capitalize_words, |re, s| {
            re.replace_with(s, |m| m.group(s, 0).unwrap().to_upper_case())
        });
    }

    #[test]
    fn csv_bytes_accepts_long_values() {
        let long = vec!["0a"; 5000].join("-") + "-...";
        assert!(is_csv_bytes(&JsString::from(long.as_str())));
        let re = RegExp::new(
            r"^([0123456789abcdef]{2}-)+[0123456789abcdef]{2}(-\.\.\.)?$",
            "",
        )
        .unwrap();
        let short = vec!["0a"; 200].join("-");
        assert!(re.test(JsString::from(short.as_str()).units()));
    }

    #[test]
    fn youtube_ids() {
        let id = |s: &str| youtube_video_id(&JsString::from(s)).map(|j| j.to_string_lossy());
        assert_eq!(
            id("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            id("https://youtu.be/dQw4w9WgXcQ?t=1").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            id("youtube.com/a/b/c/dQw4w9WgXcQ/").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(id("https://example.com/watch?v=dQw4w9WgXcQ"), None);
    }
}
