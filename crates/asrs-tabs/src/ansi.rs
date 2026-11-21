// Ported from ansi_up 6.0.6 (MIT, Copyright (c) 2011 github.com/drudru). See THIRD_PARTY_NOTICES.md.

//! Port of ansi_up 6.0.6 (`ansi_up.ts`) with the options upstream's ConsoleRenderer uses:
//! `use_classes` false (inline styles), `escape_html` true, the `{http, https}` URL allowlist.

use std::sync::LazyLock;

use asrs_js::number::parse_int;
use asrs_js::regex::RegExp;
use asrs_js::string::JsString;

const ESC: u16 = 0x1b;

const NORMAL: [Rgb; 8] = [
    Rgb(0, 0, 0),
    Rgb(187, 0, 0),
    Rgb(0, 187, 0),
    Rgb(187, 187, 0),
    Rgb(0, 0, 187),
    Rgb(187, 0, 187),
    Rgb(0, 187, 187),
    Rgb(255, 255, 255),
];

const BRIGHT: [Rgb; 8] = [
    Rgb(85, 85, 85),
    Rgb(255, 85, 85),
    Rgb(0, 255, 0),
    Rgb(255, 255, 85),
    Rgb(85, 85, 255),
    Rgb(255, 85, 255),
    Rgb(85, 255, 255),
    Rgb(255, 255, 255),
];

const fn palette_256() -> [Rgb; 256] {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut out = [Rgb(0, 0, 0); 256];
    let mut i = 0;
    while i < 8 {
        out[i] = NORMAL[i];
        out[8 + i] = BRIGHT[i];
        i += 1;
    }
    let mut index = 16;
    let mut r = 0;
    while r < 6 {
        let mut g = 0;
        while g < 6 {
            let mut b = 0;
            while b < 6 {
                out[index] = Rgb(LEVELS[r], LEVELS[g], LEVELS[b]);
                index += 1;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }
    let mut level = 8u8;
    while index < 256 {
        out[index] = Rgb(level, level, level);
        level += 10;
        index += 1;
    }
    out
}

static PALETTE_256: LazyLock<[Rgb; 256]> = LazyLock::new(palette_256);

// The regex sources below are the exact patterns `rgx`/`rgxG` produce in ansi_up.ts once the
// template's whitespace and comments are stripped.
static CSI: LazyLock<RegExp> = LazyLock::new(|| {
    RegExp::new(
        r"^(?:\x1b\[([\x3c-\x3f]?)([\d;]*)([\x20-\x2f]?[\x40-\x7e]))|(?:\x1b\[[\x20-\x7e]*([\x00-\x1f:]))",
        "",
    )
    .expect("the ansi_up CSI pattern is in the supported subset")
});

static OSC_ST: LazyLock<RegExp> = LazyLock::new(|| {
    RegExp::new(
        r"(?:(\x1b\\)|(\x07))|([\x00-\x06]|[\x08-\x1a]|[\x1c-\x1f])",
        "g",
    )
    .expect("the ansi_up OSC ST pattern is in the supported subset")
});

static OSC: LazyLock<RegExp> = LazyLock::new(|| {
    RegExp::new(
        r"^\x1b\]8;[\x20-\x3a\x3c-\x7e]*;([\x21-\x7e]{0,512})(?:\x1b\\|\x07)([\x20-\x7e]+)\x1b\]8;;(?:\x1b\\|\x07)",
        "",
    )
    .expect("the ansi_up OSC pattern is in the supported subset")
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The rendition state an `SGR` sequence leaves behind: what ansi_up keeps in `fg`, `bg`,
/// `bold`, `faint`, `italic` and `underline`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnsiStyle {
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub underline: bool,
}

impl AnsiStyle {
    /// The inline `style` attribute ansi_up writes for this rendition, or `None` for plain
    /// text, which it emits without a `<span>`.
    pub fn css(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if self.bold {
            parts.push("font-weight:bold".into());
        }
        if self.faint {
            parts.push("opacity:0.7".into());
        }
        if self.italic {
            parts.push("font-style:italic".into());
        }
        if self.underline {
            parts.push("text-decoration:underline".into());
        }
        if let Some(Rgb(r, g, b)) = self.fg {
            parts.push(format!("color:rgb({r},{g},{b})"));
        }
        if let Some(Rgb(r, g, b)) = self.bg {
            parts.push(format!("background-color:rgb({r},{g},{b})"));
        }
        (!parts.is_empty()).then(|| parts.join(";"))
    }
}

/// One output block of `ansi_to_html`: styled text, or an OSC 8 hyperlink. ansi_up drops
/// hyperlinks whose scheme its allowlist rejects instead of emitting a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnsiBlock {
    Text { text: JsString, style: AnsiStyle },
    Link { url: JsString, text: JsString },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PacketKind {
    Eos,
    Text,
    Incomplete,
    Esc,
    Unknown,
    Sgr,
    OscUrl,
}

struct Packet {
    kind: PacketKind,
    text: Vec<u16>,
    url: Vec<u16>,
}

impl Packet {
    fn at(kind: PacketKind) -> Self {
        Packet {
            kind,
            text: Vec::new(),
            url: Vec::new(),
        }
    }
}

/// The `AnsiUp` state machine: rendition state plus the input buffer an `Incomplete` packet
/// leaves behind for the next `ansi_to_html` call.
#[derive(Default)]
pub struct AnsiUp {
    buffer: Vec<u16>,
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    bold: bool,
    faint: bool,
    italic: bool,
    underline: bool,
}

impl AnsiUp {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ansi_to_html`: appends `txt` to the buffer, drains every complete packet, and
    /// returns the HTML the produced blocks serialize to.
    pub fn ansi_to_html(&mut self, txt: &JsString) -> JsString {
        blocks_to_html(&self.ansi_to_spans(txt))
    }

    /// The converted output as a styled-span model a view can render.
    pub fn ansi_to_spans(&mut self, txt: &JsString) -> Vec<AnsiBlock> {
        self.buffer.extend_from_slice(txt.units());
        let mut blocks = Vec::new();
        loop {
            let packet = self.get_next_packet();
            match packet.kind {
                PacketKind::Eos | PacketKind::Incomplete => break,
                PacketKind::Esc | PacketKind::Unknown => continue,
                PacketKind::Text => {
                    if !packet.text.is_empty() {
                        blocks.push(AnsiBlock::Text {
                            text: JsString::from_units(packet.text),
                            style: AnsiStyle {
                                fg: self.fg,
                                bg: self.bg,
                                bold: self.bold,
                                faint: self.faint,
                                italic: self.italic,
                                underline: self.underline,
                            },
                        });
                    }
                }
                PacketKind::Sgr => self.process_ansi(&packet.text),
                PacketKind::OscUrl => {
                    if let Some(link) = process_hyperlink(&packet) {
                        blocks.push(link);
                    }
                }
            }
        }
        blocks
    }

    fn get_next_packet(&mut self) -> Packet {
        let mut pkt = Packet::at(PacketKind::Eos);
        let len = self.buffer.len();
        if len == 0 {
            return pkt;
        }
        let Some(pos) = self.buffer.iter().position(|&u| u == ESC) else {
            pkt.kind = PacketKind::Text;
            pkt.text = std::mem::take(&mut self.buffer);
            return pkt;
        };
        if pos > 0 {
            pkt.kind = PacketKind::Text;
            pkt.text = self.buffer.drain(..pos).collect();
            return pkt;
        }
        if len < 3 {
            pkt.kind = PacketKind::Incomplete;
            return pkt;
        }
        let next_char = self.buffer[1];
        if next_char != u16::from(b'[')
            && next_char != u16::from(b']')
            && next_char != u16::from(b'(')
        {
            pkt.kind = PacketKind::Esc;
            pkt.text = self.buffer.drain(..1).collect();
            return pkt;
        }
        if next_char == u16::from(b'[') {
            let Some(m) = CSI.exec(&self.buffer, &mut 0) else {
                pkt.kind = PacketKind::Incomplete;
                return pkt;
            };
            if m.range(4).is_some() {
                pkt.kind = PacketKind::Esc;
                pkt.text = self.buffer.drain(..1).collect();
                return pkt;
            }
            let private_mode = m.range(1).is_some_and(|(s, e)| e > s);
            let sgr = m
                .group(&self.buffer, 3)
                .is_some_and(|g| g.units() == units("m").as_slice());
            pkt.kind = if !private_mode && sgr {
                PacketKind::Sgr
            } else {
                PacketKind::Unknown
            };
            if let Some((s, e)) = m.range(2) {
                pkt.text = self.buffer[s..e].to_vec();
            }
            self.buffer.drain(..m.end());
            pkt
        } else if next_char == u16::from(b']') {
            if len < 4 {
                pkt.kind = PacketKind::Incomplete;
                return pkt;
            }
            if self.buffer[2] != u16::from(b'8') || self.buffer[3] != u16::from(b';') {
                pkt.kind = PacketKind::Esc;
                pkt.text = self.buffer.drain(..1).collect();
                return pkt;
            }
            let mut last_index = 0;
            for _ in 0..2 {
                let Some(m) = OSC_ST.exec(&self.buffer, &mut last_index) else {
                    pkt.kind = PacketKind::Incomplete;
                    return pkt;
                };
                if m.range(3).is_some() {
                    pkt.kind = PacketKind::Esc;
                    pkt.text = self.buffer.drain(..1).collect();
                    return pkt;
                }
            }
            let Some(m) = OSC.exec(&self.buffer, &mut 0) else {
                pkt.kind = PacketKind::Esc;
                pkt.text = self.buffer.drain(..1).collect();
                return pkt;
            };
            pkt.kind = PacketKind::OscUrl;
            if let Some((s, e)) = m.range(1) {
                pkt.url = self.buffer[s..e].to_vec();
            }
            if let Some((s, e)) = m.range(2) {
                pkt.text = self.buffer[s..e].to_vec();
            }
            self.buffer.drain(..m.end());
            pkt
        } else {
            pkt.kind = PacketKind::Unknown;
            self.buffer.drain(..3);
            pkt
        }
    }

    /// `process_ansi`: an SGR parameter list. Commands consume following parameters, so the
    /// queue drains left to right by index, as upstream's `shift()` loop does.
    fn process_ansi(&mut self, text: &[u16]) {
        let cmds: Vec<&[u16]> = text.split(|&u| u == u16::from(b';')).collect();
        let mut i = 0;
        while i < cmds.len() {
            let cmd = cmds[i];
            i += 1;
            let num = parse_int(cmd, 10.0);
            if num.is_nan() || num == 0.0 {
                self.fg = None;
                self.bg = None;
                self.bold = false;
                self.faint = false;
                self.italic = false;
                self.underline = false;
            } else if num == 1.0 {
                self.bold = true;
            } else if num == 2.0 {
                self.faint = true;
            } else if num == 3.0 {
                self.italic = true;
            } else if num == 4.0 {
                self.underline = true;
            } else if num == 21.0 {
                self.bold = false;
            } else if num == 22.0 {
                self.faint = false;
                self.bold = false;
            } else if num == 23.0 {
                self.italic = false;
            } else if num == 24.0 {
                self.underline = false;
            } else if num == 39.0 {
                self.fg = None;
            } else if num == 49.0 {
                self.bg = None;
            } else if (30.0..38.0).contains(&num) {
                self.fg = Some(NORMAL[num as usize - 30]);
            } else if (40.0..48.0).contains(&num) {
                self.bg = Some(NORMAL[num as usize - 40]);
            } else if (90.0..98.0).contains(&num) {
                self.fg = Some(BRIGHT[num as usize - 90]);
            } else if (100.0..108.0).contains(&num) {
                self.bg = Some(BRIGHT[num as usize - 100]);
            } else if (num == 38.0 || num == 48.0) && i < cmds.len() {
                let is_foreground = num == 38.0;
                let mode_cmd = cmds[i];
                i += 1;
                // ansi_up compares the raw parameter text, so "05" is not mode 5.
                if mode_cmd == units("5").as_slice() && i < cmds.len() {
                    let palette_index = parse_int(cmds[i], 10.0);
                    i += 1;
                    if (0.0..=255.0).contains(&palette_index) {
                        let color = PALETTE_256[palette_index as usize];
                        if is_foreground {
                            self.fg = Some(color);
                        } else {
                            self.bg = Some(color);
                        }
                    }
                }
                if mode_cmd == units("2").as_slice() && cmds.len() - i > 2 {
                    let r = parse_int(cmds[i], 10.0);
                    let g = parse_int(cmds[i + 1], 10.0);
                    let b = parse_int(cmds[i + 2], 10.0);
                    i += 3;
                    if (0.0..=255.0).contains(&r)
                        && (0.0..=255.0).contains(&g)
                        && (0.0..=255.0).contains(&b)
                    {
                        let color = Rgb(r as u8, g as u8, b as u8);
                        if is_foreground {
                            self.fg = Some(color);
                        } else {
                            self.bg = Some(color);
                        }
                    }
                }
            }
        }
    }
}

/// `process_hyperlink`: the allowlist is a plain JS object, so every `Object.prototype`
/// member name reads back truthy and is treated as an allowed scheme.
fn process_hyperlink(pkt: &Packet) -> Option<AnsiBlock> {
    let end = pkt
        .url
        .iter()
        .position(|&u| u == u16::from(b':'))
        .unwrap_or(pkt.url.len());
    let scheme = &pkt.url[..end];
    const OBJECT_PROTO: &[&str] = &[
        "__defineGetter__",
        "__defineSetter__",
        "__lookupGetter__",
        "__lookupSetter__",
        "__proto__",
        "constructor",
        "hasOwnProperty",
        "isPrototypeOf",
        "propertyIsEnumerable",
        "toLocaleString",
        "toString",
        "valueOf",
    ];
    let allowed = scheme == units("http").as_slice()
        || scheme == units("https").as_slice()
        || OBJECT_PROTO.iter().any(|name| scheme == units(name));
    allowed.then(|| AnsiBlock::Link {
        url: JsString::from_units(pkt.url.clone()),
        text: JsString::from_units(pkt.text.clone()),
    })
}

/// `escape_txt_for_html` with `escape_html` on.
fn escape(text: &[u16], out: &mut Vec<u16>) {
    for &u in text {
        match u8::try_from(u) {
            Ok(b'&') => out.extend("&amp;".encode_utf16()),
            Ok(b'<') => out.extend("&lt;".encode_utf16()),
            Ok(b'>') => out.extend("&gt;".encode_utf16()),
            Ok(b'"') => out.extend("&quot;".encode_utf16()),
            Ok(b'\'') => out.extend("&#x27;".encode_utf16()),
            _ => out.push(u),
        }
    }
}

/// Serializes a block list to exactly the HTML ansi_up's `blocks.join("")` produces.
pub fn blocks_to_html(blocks: &[AnsiBlock]) -> JsString {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            AnsiBlock::Text { text, style } => {
                if text.is_empty() {
                    continue;
                }
                let Some(css) = style.css() else {
                    escape(text.units(), &mut out);
                    continue;
                };
                out.extend("<span style=\"".encode_utf16());
                out.extend(css.encode_utf16());
                out.extend("\">".encode_utf16());
                escape(text.units(), &mut out);
                out.extend("</span>".encode_utf16());
            }
            AnsiBlock::Link { url, text } => {
                out.extend("<a href=\"".encode_utf16());
                escape(url.units(), &mut out);
                out.extend("\">".encode_utf16());
                escape(text.units(), &mut out);
                out.extend("</a>".encode_utf16());
            }
        }
    }
    JsString::from_units(out)
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}
