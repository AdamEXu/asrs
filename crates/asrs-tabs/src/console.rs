//! Port of the console-line formatting and row filtering in
//! `src/shared/renderers/ConsoleRenderer.ts`.

use asrs_js::regex::sites;
use asrs_js::string::{JsError, JsString};

use crate::ansi::{AnsiStyle, AnsiUp, Rgb};

fn js(s: &str) -> JsString {
    JsString::from(s)
}

/// `ConsoleRenderer.formatValue`: normalizes spellings like `\x1b`, renders ANSI to HTML,
/// highlights a positive filter, then turns newlines into `<br />`.
pub fn format_value(value: &JsString, filter: &JsString) -> Result<JsString, JsError> {
    let normalized = sites::console_newlines(&sites::console_escapes(value));
    let mut ansi_up = AnsiUp::new();
    let mut html = ansi_up.ansi_to_html(&normalized);
    if !filter.is_empty() && !filter.starts_with(&js("!"), None) {
        let parts = sites::split_tags(&html);
        let filter_lower = filter.to_lower_case();
        let filter_len = filter.len() as i64;
        let mut formatted = Vec::with_capacity(html.len());
        for part in parts {
            if part.starts_with(&js("<"), None) && part.ends_with(&js(">"), None) {
                formatted.extend_from_slice(part.units());
                continue;
            }
            let part_lower = part.to_lower_case();
            let mut buf = Vec::with_capacity(part.len());
            let mut last_position: i64 = -1;
            let mut position: i64 = -1;
            while position + filter_len < part.len() as i64 && {
                position = part_lower.index_of(
                    &filter_lower,
                    Some(if position == -1 {
                        0.0
                    } else {
                        (position + filter_len) as f64
                    }),
                );
                position > -1
            } {
                let from = if last_position == -1 {
                    0.0
                } else {
                    (last_position + filter_len) as f64
                };
                buf.extend_from_slice(part.substring(from, Some(position as f64)).units());
                buf.extend("<span class=\"highlight\">".encode_utf16());
                buf.extend_from_slice(
                    part.substring(position as f64, Some((position + filter_len) as f64))
                        .units(),
                );
                buf.extend("</span>".encode_utf16());
                last_position = position;
            }
            if last_position != -1 {
                buf.extend_from_slice(
                    part.substring((last_position + filter_len) as f64, None)
                        .units(),
                );
                formatted.extend_from_slice(&buf);
            } else {
                formatted.extend_from_slice(part.units());
            }
        }
        html = JsString::from_units(formatted);
    }
    html.replace_all(&js("\n"), &js("<br />"))
}

/// The row predicate in `updateValueAvailability`: `filter` here is the raw FILTER_INPUT
/// value, exactly as upstream lowers it itself before matching.
pub fn line_visible(value: &JsString, filter_input: &JsString) -> bool {
    let filter = filter_input.to_lower_case();
    let negated = filter.starts_with(&js("!"), None);
    if !(if negated {
        filter.len() > 1
    } else {
        !filter.is_empty()
    }) {
        return true;
    }
    if negated {
        !value
            .to_lower_case()
            .includes(&filter.slice(1.0, None).to_lower_case(), None)
    } else {
        value
            .to_lower_case()
            .includes(&filter.to_lower_case(), None)
    }
}

/// One node of the DOM a console value cell holds after `innerHTML = formatValue(...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleNode {
    Text {
        text: JsString,
        style: AnsiStyle,
        link: Option<JsString>,
        highlight: bool,
    },
    LineBreak,
}

/// The cell content [`format_value`] describes, as the HTML parser builds it: text nodes with
/// the ansi_up rendition, enclosing link and filter highlight they inherit, and `<br />`.
///
/// Highlighting runs on escaped text, so a match can cut a character reference in two. The
/// parser then decodes the fragments on their own: `&amp` before a tag is a legacy reference
/// and reads as `&`, `&#x2` reads as U+0002, and `&am` stays literal. `text` carries those
/// decoded characters, not the source line.
pub fn format_value_nodes(
    value: &JsString,
    filter: &JsString,
) -> Result<Vec<ConsoleNode>, JsError> {
    Ok(parse_cell_html(&format_value(value, filter)?))
}

fn parse_cell_html(html: &JsString) -> Vec<ConsoleNode> {
    #[derive(Clone)]
    enum Open {
        Style(AnsiStyle),
        Highlight,
        Link(JsString),
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut nodes = Vec::new();
    for part in sites::split_tags(html) {
        let u = part.units();
        if u.first() == Some(&u16::from(b'<')) && u.last() == Some(&u16::from(b'>')) {
            let tag = String::from_utf16_lossy(u);
            if tag == "<br />" {
                nodes.push(ConsoleNode::LineBreak);
            } else if tag == "</span>" || tag == "</a>" {
                stack.pop();
            } else if tag == "<span class=\"highlight\">" {
                stack.push(Open::Highlight);
            } else if let Some(css) = attribute(&tag, "<span style=\"") {
                stack.push(Open::Style(parse_css(&css)));
            } else if let Some(href) = attribute(&tag, "<a href=\"") {
                stack.push(Open::Link(JsString::from_units(decode_references(&href))));
            }
            continue;
        }
        let text = text_content(u);
        if text.is_empty() {
            continue;
        }
        let mut style = AnsiStyle::default();
        let mut link = None;
        let mut highlight = false;
        for open in &stack {
            match open {
                Open::Style(s) => style = *s,
                Open::Highlight => highlight = true,
                Open::Link(href) => link = Some(href.clone()),
            }
        }
        nodes.push(ConsoleNode::Text {
            text: JsString::from_units(text),
            style,
            link,
            highlight,
        });
    }
    nodes
}

fn attribute(tag: &str, prefix: &str) -> Option<Vec<u16>> {
    let value = tag.strip_prefix(prefix)?.strip_suffix("\">")?;
    Some(value.encode_utf16().collect())
}

fn parse_css(css: &[u16]) -> AnsiStyle {
    let mut style = AnsiStyle::default();
    for decl in String::from_utf16_lossy(css).split(';') {
        match decl {
            "font-weight:bold" => style.bold = true,
            "opacity:0.7" => style.faint = true,
            "font-style:italic" => style.italic = true,
            "text-decoration:underline" => style.underline = true,
            _ => {
                if let Some(rgb) = decl.strip_prefix("color:rgb(") {
                    style.fg = parse_rgb(rgb);
                } else if let Some(rgb) = decl.strip_prefix("background-color:rgb(") {
                    style.bg = parse_rgb(rgb);
                }
            }
        }
    }
    style
}

fn parse_rgb(s: &str) -> Option<Rgb> {
    let channel = |c: &str| {
        c.bytes().try_fold(0u8, |acc, d| {
            acc.checked_mul(10)?
                .checked_add(d.checked_sub(b'0').filter(|&d| d < 10)?)
        })
    };
    let mut it = s.strip_suffix(')')?.split(',').map(channel);
    Some(Rgb(it.next()??, it.next()??, it.next()??))
}

/// The characters the HTML parser puts in the DOM for one run of cell text: CR and CRLF
/// become LF, character references are decoded, and NUL is dropped as body content.
fn text_content(u: &[u16]) -> Vec<u16> {
    let mut normalized = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        if u[i] == 0x0d {
            normalized.push(0x0a);
            i += 1 + usize::from(u.get(i + 1) == Some(&0x0a));
        } else {
            normalized.push(u[i]);
            i += 1;
        }
    }
    let mut text = decode_references(&normalized);
    text.retain(|&c| c != 0);
    text
}

/// Character references in a text run of the cell HTML. Every `&` there starts one of the five
/// escapes ansi_up writes, possibly cut short by a highlight tag, so the HTML tokenizer's rules
/// reduce to: a full escape, a legacy name without `;` (`amp`, `lt`, `gt`, `quot`), a hex
/// reference with at least one digit, or else a literal `&`.
fn decode_references(u: &[u16]) -> Vec<u16> {
    const NAMED: [(&str, u16); 4] = [("amp", 0x26), ("lt", 0x3c), ("gt", 0x3e), ("quot", 0x22)];
    let mut out = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        if u[i] != u16::from(b'&') {
            out.push(u[i]);
            i += 1;
            continue;
        }
        let rest = &u[i + 1..];
        let semicolon = |n: usize| usize::from(rest.get(n) == Some(&u16::from(b';')));
        if let Some((name, ch)) = NAMED
            .iter()
            .find(|(name, _)| rest.starts_with(&name.encode_utf16().collect::<Vec<_>>()))
        {
            out.push(*ch);
            i += 1 + name.len() + semicolon(name.len());
            continue;
        }
        if rest.starts_with(&[u16::from(b'#'), u16::from(b'x')]) {
            let digits = rest[2..]
                .iter()
                .take_while(|&&c| {
                    char::from_u32(u32::from(c)).is_some_and(|c| c.is_ascii_hexdigit())
                })
                .count();
            if digits > 0 {
                let code = rest[2..2 + digits].iter().fold(0u32, |acc, &c| {
                    acc * 16
                        + char::from_u32(u32::from(c))
                            .and_then(|c| c.to_digit(16))
                            .unwrap_or(0)
                });
                out.push(if code == 0 { 0xfffd } else { code as u16 });
                i += 3 + digits + semicolon(2 + digits);
                continue;
            }
        }
        out.push(u[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str, style: AnsiStyle, link: Option<&str>, highlight: bool) -> ConsoleNode {
        ConsoleNode::Text {
            text: js(s),
            style,
            link: link.map(js),
            highlight,
        }
    }

    fn nodes(value: &str, filter: &str) -> Vec<ConsoleNode> {
        format_value_nodes(&js(value), &js(filter)).unwrap()
    }

    // Expected nodes were read back from Electron's DOM after `innerHTML = formatValue(...)`.
    #[test]
    fn highlight_inside_escapes() {
        let plain = AnsiStyle::default();
        assert_eq!(
            nodes("a&b", ";"),
            [
                text("a&", plain, None, false),
                text(";", plain, None, true),
                text("b", plain, None, false)
            ]
        );
        assert_eq!(
            nodes("a&b", "am"),
            [
                text("a&", plain, None, false),
                text("am", plain, None, true),
                text("p;b", plain, None, false)
            ]
        );
        assert_eq!(
            nodes("'", "7"),
            [
                text("\u{2}", plain, None, false),
                text("7", plain, None, true),
                text(";", plain, None, false)
            ]
        );
        assert_eq!(
            nodes("<", "&"),
            [
                text("&", plain, None, true),
                text("lt;", plain, None, false)
            ]
        );
    }

    #[test]
    fn styles_links_and_breaks() {
        let red_bold = AnsiStyle {
            fg: Some(Rgb(187, 0, 0)),
            bold: true,
            ..AnsiStyle::default()
        };
        assert_eq!(
            nodes("\\x1b[1;31mx\\ny\r\n\0z", "y"),
            [
                text("x", red_bold, None, false),
                ConsoleNode::LineBreak,
                text("y", red_bold, None, true),
                text("\n", red_bold, None, false),
                ConsoleNode::LineBreak,
                text("z", red_bold, None, false),
            ]
        );
        assert_eq!(
            nodes("\x1b]8;;https://a.b/?x&y\x07go\x1b]8;;\x07", ""),
            [text(
                "go",
                AnsiStyle::default(),
                Some("https://a.b/?x&y"),
                false
            )]
        );
    }
}
