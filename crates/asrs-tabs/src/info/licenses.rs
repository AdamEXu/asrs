use asrs_js::string::JsString;

/// The two `div`s of one entry of licenses.json.
#[derive(Clone, Debug, PartialEq)]
pub struct License {
    /// `div.module-text`'s lines, with a `<br>` between each two.
    pub module: Vec<JsString>,
    /// `div.license-text`'s text nodes, with a `<br>` between each two.
    pub parts: Vec<JsString>,
}

impl License {
    pub fn new(module: &JsString, text: &JsString) -> License {
        License {
            module: inner_text_lines(module),
            parts: parts(text),
        }
    }
}

const fn unit(c: char) -> u16 {
    c as u16
}

/// The lines `innerText = text` makes: a `<br>` for every `\n`, `\r\n` and lone `\r`.
fn inner_text_lines(text: &JsString) -> Vec<JsString> {
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut units = text.units().iter().copied().peekable();
    while let Some(u) = units.next() {
        if u == unit('\r') || u == unit('\n') {
            if u == unit('\r') {
                units.next_if_eq(&unit('\n'));
            }
            lines.push(JsString::from_units(std::mem::take(&mut line)));
        } else {
            line.push(u);
        }
    }
    lines.push(JsString::from_units(line));
    lines
}

fn replace_all(text: &[u16], search: &str, replacement: &str) -> Vec<u16> {
    let search: Vec<u16> = search.encode_utf16().collect();
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with(&search) {
            out.extend(replacement.encode_utf16());
            at += search.len();
        } else {
            out.push(text[at]);
            at += 1;
        }
    }
    out
}

/// The license text with its hard wraps removed: blank lines and lines that start with `>` or
/// `-` stay lines, every other newline joins.
fn parts(text: &JsString) -> Vec<JsString> {
    let mut clean = text.units().to_vec();
    for (search, replacement) in [
        ("\t", ""),
        ("\r\n", "\n"),
        ("\n\n", "<br><br>"),
        ("\n>", "<br>>"),
        ("\n-", "<br>-"),
        ("\n", " "),
    ] {
        clean = replace_all(&clean, search, replacement);
    }
    let space = unit(' ');
    clean.dedup_by(|a, b| *a == space && *b == space);
    if clean.first() == Some(&space) {
        clean.remove(0);
    }
    JsString::from_units(replace_all(&clean, "<br> ", "<br>"))
        .split(Some(&JsString::from("<br>")), None)
}
