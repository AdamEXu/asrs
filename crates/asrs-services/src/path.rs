//! Node's `path.join`, `path.normalize` and `path.sep` for the two flavours main
//! runs under, on UTF-16 strings so paths taken from JSON keep any lone surrogates.

use asrs_js::string::JsString;

const SLASH: u16 = b'/' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const DOT: u16 = b'.' as u16;
const COLON: u16 = b':' as u16;
const QUESTION: u16 = b'?' as u16;

const WINDOWS_RESERVED_NAMES: [&str; 28] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStyle {
    Posix,
    Win32,
}

impl PathStyle {
    pub fn host() -> PathStyle {
        if cfg!(windows) {
            PathStyle::Win32
        } else {
            PathStyle::Posix
        }
    }

    /// `path.sep`.
    pub fn sep(self) -> u16 {
        match self {
            PathStyle::Posix => SLASH,
            PathStyle::Win32 => BACKSLASH,
        }
    }

    fn is_sep(self, unit: u16) -> bool {
        unit == SLASH || (self == PathStyle::Win32 && unit == BACKSLASH)
    }

    /// `path.join(...parts)`.
    pub fn join(self, parts: &[&JsString]) -> JsString {
        let mut joined: Option<Vec<u16>> = None;
        let mut first: &[u16] = &[];
        for part in parts.iter().map(|p| p.units()).filter(|p| !p.is_empty()) {
            match &mut joined {
                None => {
                    joined = Some(part.to_vec());
                    first = part;
                }
                Some(j) => {
                    j.push(self.sep());
                    j.extend_from_slice(part);
                }
            }
        }
        let Some(mut joined) = joined else {
            return JsString::from(".");
        };
        if self == PathStyle::Win32 {
            let mut needs_replace = true;
            let mut slashes = 0;
            if first.first().is_some_and(|&u| self.is_sep(u)) {
                slashes += 1;
                if first.len() > 1 && self.is_sep(first[1]) {
                    slashes += 1;
                    if first.len() > 2 {
                        if self.is_sep(first[2]) {
                            slashes += 1;
                        } else {
                            needs_replace = false;
                        }
                    }
                }
            }
            if needs_replace {
                while slashes < joined.len() && self.is_sep(joined[slashes]) {
                    slashes += 1;
                }
                if slashes >= 2 {
                    let mut replaced = vec![BACKSLASH];
                    replaced.extend_from_slice(&joined[slashes..]);
                    joined = replaced;
                }
            }
        }
        if self == PathStyle::Win32
            && joined
                .split(|&u| u == BACKSLASH)
                .filter(|part| !part.is_empty())
                .any(|part| {
                    index_of(part, COLON).is_some_and(|c| is_windows_reserved_name(part, c))
                })
        {
            return JsString::from_units(
                joined
                    .iter()
                    .map(|&u| if u == SLASH { BACKSLASH } else { u })
                    .collect(),
            );
        }
        self.normalize(&JsString::from_units(joined))
    }

    /// `path.normalize(path)`.
    pub fn normalize(self, path: &JsString) -> JsString {
        match self {
            PathStyle::Posix => normalize_posix(path.units()),
            PathStyle::Win32 => normalize_win32(path.units()),
        }
    }
}

fn normalize_string(path: &[u16], allow_above_root: bool, sep: u16, style: PathStyle) -> Vec<u16> {
    let mut res: Vec<u16> = Vec::new();
    let mut last_segment_length = 0usize;
    let mut last_slash: isize = -1;
    let mut dots: i32 = 0;
    let mut code: u16 = 0;
    for i in 0..=path.len() {
        if i < path.len() {
            code = path[i];
        } else if style.is_sep(code) {
            break;
        } else {
            code = SLASH;
        }
        if style.is_sep(code) {
            let i = i as isize;
            if last_slash == i - 1 || dots == 1 {
            } else if dots == 2 {
                if res.len() < 2
                    || last_segment_length != 2
                    || res[res.len() - 1] != DOT
                    || res[res.len() - 2] != DOT
                {
                    if res.len() > 2 {
                        match res.len().checked_sub(last_segment_length + 1) {
                            None => {
                                res.clear();
                                last_segment_length = 0;
                            }
                            Some(last_slash_index) => {
                                res.truncate(last_slash_index);
                                last_segment_length = match res.iter().rposition(|&u| u == sep) {
                                    Some(p) => res.len() - 1 - p,
                                    None => res.len(),
                                };
                            }
                        }
                        last_slash = i;
                        dots = 0;
                        continue;
                    } else if !res.is_empty() {
                        res.clear();
                        last_segment_length = 0;
                        last_slash = i;
                        dots = 0;
                        continue;
                    }
                }
                if allow_above_root {
                    if !res.is_empty() {
                        res.push(sep);
                    }
                    res.extend_from_slice(&[DOT, DOT]);
                    last_segment_length = 2;
                }
            } else {
                let segment = &path[(last_slash + 1) as usize..i as usize];
                if !res.is_empty() {
                    res.push(sep);
                }
                res.extend_from_slice(segment);
                last_segment_length = (i - last_slash - 1) as usize;
            }
            last_slash = i;
            dots = 0;
        } else if code == DOT && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
    }
    res
}

fn normalize_posix(path: &[u16]) -> JsString {
    if path.is_empty() {
        return JsString::from(".");
    }
    let is_absolute = path[0] == SLASH;
    let trailing = path[path.len() - 1] == SLASH;
    let mut out = normalize_string(path, !is_absolute, SLASH, PathStyle::Posix);
    if out.is_empty() {
        return JsString::from(match (is_absolute, trailing) {
            (true, _) => "/",
            (false, true) => "./",
            (false, false) => ".",
        });
    }
    if trailing {
        out.push(SLASH);
    }
    if is_absolute {
        out.insert(0, SLASH);
    }
    JsString::from_units(out)
}

fn is_device_root(unit: u16) -> bool {
    (u16::from(b'A')..=u16::from(b'Z')).contains(&unit)
        || (u16::from(b'a')..=u16::from(b'z')).contains(&unit)
}

fn index_of(units: &[u16], unit: u16) -> Option<usize> {
    units.iter().position(|&u| u == unit)
}

fn is_windows_reserved_name(path: &[u16], colon_index: usize) -> bool {
    let device = JsString::from_units(path[..colon_index].to_vec()).to_upper_case();
    WINDOWS_RESERVED_NAMES
        .iter()
        .any(|name| device == JsString::from(*name))
}

/// `isWindowsReservedName(path, path.indexOf(":"))`: without a colon node slices to `-1`, so
/// a path that is a reserved name plus any one unit, like `CONx`, also counts.
fn has_reserved_device(path: &[u16]) -> bool {
    match index_of(path, COLON) {
        Some(colon_index) => is_windows_reserved_name(path, colon_index),
        None => !path.is_empty() && is_windows_reserved_name(path, path.len() - 1),
    }
}

fn normalize_win32(path: &[u16]) -> JsString {
    let style = PathStyle::Win32;
    let len = path.len();
    if len == 0 {
        return JsString::from(".");
    }
    let mut root_end = 0;
    let mut device: Option<Vec<u16>> = None;
    let mut is_absolute = false;
    let code = path[0];
    if len == 1 {
        return if code == SLASH {
            JsString::from("\\")
        } else {
            JsString::from_units(path.to_vec())
        };
    }
    if style.is_sep(code) {
        is_absolute = true;
        if style.is_sep(path[1]) {
            let mut j = 2;
            let mut last = j;
            while j < len && !style.is_sep(path[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part = &path[last..j];
                last = j;
                while j < len && style.is_sep(path[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !style.is_sep(path[j]) {
                        j += 1;
                    }
                    if first_part == [DOT] || first_part == [QUESTION] {
                        let mut d = vec![BACKSLASH, BACKSLASH];
                        d.extend_from_slice(first_part);
                        device = Some(d);
                        root_end = 4;
                        let possible = match index_of(path, COLON) {
                            Some(colon_index) if colon_index >= 4 => &path[4..=colon_index],
                            _ => &[][..],
                        };
                        if !possible.is_empty()
                            && is_windows_reserved_name(possible, possible.len() - 1)
                        {
                            let mut d = vec![BACKSLASH, BACKSLASH, QUESTION, BACKSLASH];
                            d.extend_from_slice(possible);
                            device = Some(d);
                            root_end = 4 + possible.len();
                        }
                    } else if j == len {
                        let mut out = vec![BACKSLASH, BACKSLASH];
                        out.extend_from_slice(first_part);
                        out.push(BACKSLASH);
                        out.extend_from_slice(&path[last..]);
                        out.push(BACKSLASH);
                        return JsString::from_units(out);
                    } else {
                        let mut d = vec![BACKSLASH, BACKSLASH];
                        d.extend_from_slice(first_part);
                        d.push(BACKSLASH);
                        d.extend_from_slice(&path[last..j]);
                        device = Some(d);
                        root_end = j;
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if let Some(colon_index) = index_of(path, COLON).filter(|&c| c > 0) {
        if is_device_root(code) && colon_index == 1 {
            device = Some(path[..2].to_vec());
            root_end = 2;
            if len > 2 && style.is_sep(path[2]) {
                is_absolute = true;
                root_end = 3;
            }
        } else if is_windows_reserved_name(path, colon_index) {
            device = Some(path[..=colon_index].to_vec());
            root_end = colon_index + 1;
        }
    }
    let mut tail = if root_end < len {
        normalize_string(&path[root_end..], !is_absolute, BACKSLASH, style)
    } else {
        Vec::new()
    };
    if tail.is_empty() && !is_absolute {
        tail.push(DOT);
    }
    if !tail.is_empty() && style.is_sep(path[len - 1]) {
        tail.push(BACKSLASH);
    }
    let dotted = |device: Option<&[u16]>, tail: &[u16]| {
        let mut out = vec![DOT, BACKSLASH];
        out.extend_from_slice(device.unwrap_or(&[]));
        out.extend_from_slice(tail);
        JsString::from_units(out)
    };
    if !is_absolute && device.is_none() && path.contains(&COLON) {
        if tail.len() >= 2 && is_device_root(tail[0]) && tail[1] == COLON {
            return dotted(None, &tail);
        }
        for (index, &unit) in path.iter().enumerate() {
            if unit == COLON && (index == len - 1 || style.is_sep(path[index + 1])) {
                return dotted(None, &tail);
            }
        }
    }
    if has_reserved_device(path) {
        return dotted(device.as_deref(), &tail);
    }
    let mut out = Vec::new();
    if let Some(d) = device {
        out.extend_from_slice(&d);
    }
    if is_absolute {
        out.push(BACKSLASH);
    }
    out.extend_from_slice(&tail);
    JsString::from_units(out)
}
