//! The two `file-type` releases the `download` package consults: 11.1.0 for the extension it
//! appends to an extensionless file name, and 4.4.0 behind `archive-type`, which decides whether
//! a body is extracted. Only the extension of each match matters, so mimes are not kept.

/// What `fileType(buffer)` from file-type 11.1.0 does with a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guess {
    Ext(&'static str),
    Unknown,
    /// The ASF object walk meets an object whose 64-bit size reads as 0 and never advances, so
    /// upstream spins forever on the main thread.
    Stalls,
}

struct Buf<'a>(&'a [u8]);

impl Buf<'_> {
    fn at(&self, i: usize) -> Option<u8> {
        self.0.get(i).copied()
    }

    fn check(&self, offset: usize, header: &[u8]) -> bool {
        header
            .iter()
            .enumerate()
            .all(|(i, &h)| self.at(offset + i) == Some(h))
    }

    fn check_masked(&self, offset: usize, header: &[u8], mask: &[u8]) -> bool {
        header
            .iter()
            .zip(mask)
            .enumerate()
            .all(|(i, (&h, &m))| h == m & self.at(offset + i).unwrap_or(0))
    }

    fn is(&self, i: usize, values: &[u8]) -> bool {
        self.at(i).is_some_and(|b| values.contains(&b))
    }
}

/// file-type's `readUInt64LE`: a double sum, `NaN` once any byte is past the end.
fn read_u64_le(buf: &Buf<'_>, offset: f64) -> f64 {
    let mut n = 0.0;
    let mut mul = 1.0;
    for i in 0..8 {
        let at = offset + f64::from(i);
        let byte = if at >= 0.0 && at < buf.0.len() as f64 {
            f64::from(buf.0[at as usize])
        } else {
            return f64::NAN;
        };
        n += byte * mul;
        mul *= 256.0;
    }
    n
}

fn tar_checksum_matches(buf: &Buf<'_>) -> bool {
    let b = buf.0;
    if b.len() < 512 {
        return false;
    }
    let mut sum: i64 = 256;
    let mut signed_bit_sum: i64 = 0;
    for &byte in b[..148].iter().chain(&b[156..512]) {
        sum += i64::from(byte);
        signed_bit_sum += i64::from(byte & 0x80);
    }
    let read_sum = b[148..154]
        .iter()
        .fold(0i64, |acc, &d| acc * 8 + i64::from(d - b'0'));
    read_sum == sum || read_sum == sum - (signed_bit_sum << 1)
}

const OXML_CONTENT_TYPES: &[u8] = b"[Content_Types].xml";
const OXML_RELS: &[u8] = b"_rels/.rels";
const ZIP_LOCAL: &[u8] = &[0x50, 0x4b, 0x03, 0x04];

fn find_zip_header(buf: &Buf<'_>, start_at: usize) -> Option<usize> {
    (start_at..buf.0.len()).find(|&i| buf.check(i, ZIP_LOCAL))
}

fn office_open_xml(buf: &Buf<'_>) -> Option<&'static str> {
    let mut header = 0usize;
    let mut oxml_found = false;
    let mut kind: Option<&'static str> = None;
    loop {
        let offset = header + 30;
        if !oxml_found {
            oxml_found = buf.check(offset, OXML_CONTENT_TYPES) || buf.check(offset, OXML_RELS);
        }
        if kind.is_none() {
            if buf.check(offset, b"word/") {
                kind = Some("docx");
            } else if buf.check(offset, b"ppt/") {
                kind = Some("pptx");
            } else if buf.check(offset, b"xl/") {
                kind = Some("xlsx");
            }
        }
        if oxml_found && kind.is_some() {
            return kind;
        }
        match find_zip_header(buf, offset) {
            Some(next) => header = next,
            None => return kind,
        }
    }
}

fn matroska(buf: &Buf<'_>) -> Option<&'static str> {
    let sliced = &buf.0[4.min(buf.0.len())..(4 + 4096).min(buf.0.len())];
    let id_pos =
        (0..sliced.len()).find(|&i| sliced[i] == 0x42 && sliced.get(i + 1) == Some(&0x82))?;
    let doc_type = id_pos + 3;
    let found = |name: &[u8]| {
        name.iter()
            .enumerate()
            .all(|(i, &c)| sliced.get(doc_type + i) == Some(&c))
    };
    if found(b"matroska") {
        Some("mkv")
    } else if found(b"webm") {
        Some("webm")
    } else {
        None
    }
}

const ASF_STREAM_PROPERTIES: [u8; 16] = [
    0x91, 0x07, 0xdc, 0xb7, 0xb7, 0xa9, 0xcf, 0x11, 0x8e, 0xe6, 0x00, 0xc0, 0x0c, 0x20, 0x53, 0x65,
];
const ASF_AUDIO: [u8; 16] = [
    0x40, 0x9e, 0x69, 0xf8, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b,
];
const ASF_VIDEO: [u8; 16] = [
    0xc0, 0xef, 0x19, 0xbc, 0x4d, 0x5b, 0xcf, 0x11, 0xa8, 0xfd, 0x00, 0x80, 0x5f, 0x5c, 0x44, 0x2b,
];

fn asf(buf: &Buf<'_>) -> Guess {
    let len = buf.0.len() as f64;
    let mut offset = 30.0f64;
    loop {
        let object_size = read_u64_le(buf, offset + 16.0);
        let at = (offset >= 0.0 && offset <= usize::MAX as f64).then_some(offset as usize);
        if at.is_some_and(|at| buf.check(at, &ASF_STREAM_PROPERTIES)) {
            let at = at.unwrap_or_default();
            if buf.check(at + 24, &ASF_AUDIO) {
                return Guess::Ext("wma");
            }
            if buf.check(at + 24, &ASF_VIDEO) {
                return Guess::Ext("wmv");
            }
            break;
        }
        offset += object_size;
        if offset.is_nan() || offset + 24.0 > len {
            break;
        }
        if object_size == 0.0 {
            return Guess::Stalls;
        }
    }
    Guess::Ext("asf")
}

fn iso_brand(brand: &[u8]) -> &'static str {
    match brand {
        b"mif1" | b"msf1" | b"heic" | b"heix" | b"hevc" | b"hevx" => "heic",
        b"qt  " => "mov",
        b"M4V " | b"M4VH" | b"M4VP" => "m4v",
        b"M4P " => "m4p",
        b"M4B " => "m4b",
        b"M4A " => "m4a",
        b"F4V " => "f4v",
        b"F4P " => "f4p",
        b"F4A " => "f4a",
        b"F4B " => "f4b",
        _ if brand.starts_with(b"3g2") => "3g2",
        _ if brand.starts_with(b"3g") => "3gp",
        _ => "mp4",
    }
}

/// `fileType(buffer)` from file-type 11.1.0.
pub fn file_type_11(bytes: &[u8]) -> Guess {
    let b = Buf(bytes);
    if bytes.len() <= 1 {
        return Guess::Unknown;
    }
    let ext = |e: &'static str| Guess::Ext(e);
    let tiff_le = b.check(0, &[0x49, 0x49, 0x2a, 0x00]);
    let tiff_be = b.check(0, &[0x4d, 0x4d, 0x00, 0x2a]);
    if b.check(0, &[0xff, 0xd8, 0xff]) {
        return ext("jpg");
    }
    if b.check(0, &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        return ext("png");
    }
    if b.check(0, &[0x47, 0x49, 0x46]) {
        return ext("gif");
    }
    if b.check(8, &[0x57, 0x45, 0x42, 0x50]) {
        return ext("webp");
    }
    if b.check(0, &[0x46, 0x4c, 0x49, 0x46]) {
        return ext("flif");
    }
    if (tiff_le || tiff_be) && b.check(8, &[0x43, 0x52]) {
        return ext("cr2");
    }
    if b.check(0, &[0x49, 0x49, 0x52, 0x4f, 0x08, 0x00, 0x00, 0x00, 0x18]) {
        return ext("orf");
    }
    if b.check(0, &[0x49, 0x49, 0x2a, 0x00, 0x10, 0xfb, 0x86, 0x01]) {
        return ext("arw");
    }
    if b.check(0, &[0x49, 0x49, 0x2a, 0x00, 0x08, 0x00, 0x00, 0x00, 0x2d]) {
        return ext("dng");
    }
    if b.check(0, &[0x49, 0x49, 0x2a, 0x00, 0x30, 0x3d, 0x72, 0x01, 0x1c]) {
        return ext("nef");
    }
    if tiff_le || tiff_be {
        return ext("tif");
    }
    if b.check(0, &[0x42, 0x4d]) {
        return ext("bmp");
    }
    if b.check(0, &[0x49, 0x49, 0xbc]) {
        return ext("jxr");
    }
    if b.check(0, &[0x38, 0x42, 0x50, 0x53]) {
        return ext("psd");
    }
    if b.check(0, ZIP_LOCAL) {
        if b.check(30, b"mimetypeapplication/epub+zip") {
            return ext("epub");
        }
        if b.check(30, b"META-INF/mozilla.rsa") {
            return ext("xpi");
        }
        if b.check(30, b"mimetypeapplication/vnd.oasis.opendocument.text") {
            return ext("odt");
        }
        if b.check(
            30,
            b"mimetypeapplication/vnd.oasis.opendocument.spreadsheet",
        ) {
            return ext("ods");
        }
        if b.check(
            30,
            b"mimetypeapplication/vnd.oasis.opendocument.presentation",
        ) {
            return ext("odp");
        }
        if let Some(kind) = office_open_xml(&b) {
            return ext(kind);
        }
    }
    if b.check(0, &[0x50, 0x4b]) && b.is(2, &[0x03, 0x05, 0x07]) && b.is(3, &[0x04, 0x06, 0x08]) {
        return ext("zip");
    }
    if b.check_masked(148, &[0x30; 6], &[0xf8; 6]) && tar_checksum_matches(&b) {
        return ext("tar");
    }
    if b.check(0, &[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07]) && b.is(6, &[0x00, 0x01]) {
        return ext("rar");
    }
    if b.check(0, &[0x1f, 0x8b, 0x08]) {
        return ext("gz");
    }
    if b.check(0, &[0x42, 0x5a, 0x68]) {
        return ext("bz2");
    }
    if b.check(0, &[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        return ext("7z");
    }
    if b.check(0, &[0x78, 0x01]) {
        return ext("dmg");
    }
    if b.check(4, b"free") || b.check(4, b"mdat") || b.check(4, b"moov") || b.check(4, b"wide") {
        return ext("mov");
    }
    if b.check(4, b"ftyp") && (8..12).all(|i| b.at(i).is_some_and(|v| v & 0x60 != 0)) {
        return ext(iso_brand(&bytes[8..12]));
    }
    if b.check(0, &[0x4d, 0x54, 0x68, 0x64]) {
        return ext("mid");
    }
    if b.check(0, &[0x1a, 0x45, 0xdf, 0xa3])
        && let Some(kind) = matroska(&b)
    {
        return ext(kind);
    }
    if b.check(0, b"RIFF") {
        if b.check(8, b"AVI") {
            return ext("avi");
        }
        if b.check(8, b"WAVE") {
            return ext("wav");
        }
        if b.check(8, b"QLCM") {
            return ext("qcp");
        }
    }
    if b.check(
        0,
        &[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9],
    ) {
        return asf(&b);
    }
    if b.check(0, &[0x00, 0x00, 0x01, 0xba]) || b.check(0, &[0x00, 0x00, 0x01, 0xb3]) {
        return ext("mpg");
    }
    let mut start = 0;
    while start < 2 && (start as f64) < bytes.len() as f64 - 16.0 {
        if b.check(start, b"ID3") || b.check_masked(start, &[0xff, 0xe2], &[0xff, 0xe6]) {
            return ext("mp3");
        }
        if b.check_masked(start, &[0xff, 0xe4], &[0xff, 0xe6]) {
            return ext("mp2");
        }
        if b.check_masked(start, &[0xff, 0xf8], &[0xff, 0xfc]) {
            return ext("mp2");
        }
        if b.check_masked(start, &[0xff, 0xf0], &[0xff, 0xfc]) {
            return ext("mp4");
        }
        start += 1;
    }
    if b.check(28, b"OpusHead") {
        return ext("opus");
    }
    if b.check(0, b"OggS") {
        if b.check(28, &[0x80, 0x74, 0x68, 0x65, 0x6f, 0x72, 0x61]) {
            return ext("ogv");
        }
        if b.check(28, &[0x01, 0x76, 0x69, 0x64, 0x65, 0x6f, 0x00]) {
            return ext("ogm");
        }
        if b.check(28, &[0x7f, 0x46, 0x4c, 0x41, 0x43]) {
            return ext("oga");
        }
        if b.check(28, b"Speex  ") {
            return ext("spx");
        }
        if b.check(28, &[0x01, 0x76, 0x6f, 0x72, 0x62, 0x69, 0x73]) {
            return ext("ogg");
        }
        return ext("ogx");
    }
    if b.check(0, b"fLaC") {
        return ext("flac");
    }
    if b.check(0, b"MAC ") {
        return ext("ape");
    }
    if b.check(0, b"wvpk") {
        return ext("wv");
    }
    if b.check(0, b"#!AMR\n") {
        return ext("amr");
    }
    if b.check(0, b"%PDF") {
        return ext("pdf");
    }
    if b.check(0, b"MZ") {
        return ext("exe");
    }
    if b.is(0, &[0x43, 0x46]) && b.check(1, b"WS") {
        return ext("swf");
    }
    if b.check(0, b"{\\rtf") {
        return ext("rtf");
    }
    if b.check(0, &[0x00, 0x61, 0x73, 0x6d]) {
        return ext("wasm");
    }
    let font_flavor = b.check(4, &[0x00, 0x01, 0x00, 0x00]) || b.check(4, b"OTTO");
    if b.check(0, b"wOFF") && font_flavor {
        return ext("woff");
    }
    if b.check(0, b"wOF2") && font_flavor {
        return ext("woff2");
    }
    if b.check(34, b"LP")
        && (b.check(8, &[0x00, 0x00, 0x01])
            || b.check(8, &[0x01, 0x00, 0x02])
            || b.check(8, &[0x02, 0x00, 0x02]))
    {
        return ext("eot");
    }
    if b.check(0, &[0x00, 0x01, 0x00, 0x00, 0x00]) {
        return ext("ttf");
    }
    if b.check(0, &[0x4f, 0x54, 0x54, 0x4f, 0x00]) {
        return ext("otf");
    }
    if b.check(0, &[0x00, 0x00, 0x01, 0x00]) {
        return ext("ico");
    }
    if b.check(0, &[0x00, 0x00, 0x02, 0x00]) {
        return ext("cur");
    }
    if b.check(0, &[0x46, 0x4c, 0x56, 0x01]) {
        return ext("flv");
    }
    if b.check(0, b"%!") {
        return ext("ps");
    }
    if b.check(0, &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]) {
        return ext("xz");
    }
    if b.check(0, b"SQLi") {
        return ext("sqlite");
    }
    if b.check(0, &[0x4e, 0x45, 0x53, 0x1a]) {
        return ext("nes");
    }
    if b.check(0, b"Cr24") {
        return ext("crx");
    }
    if b.check(0, b"MSCF") || b.check(0, b"ISc(") {
        return ext("cab");
    }
    if b.check(0, b"!<arch>\ndebian-binary") {
        return ext("deb");
    }
    if b.check(0, b"!<arch>") {
        return ext("ar");
    }
    if b.check(0, &[0xed, 0xab, 0xee, 0xdb]) {
        return ext("rpm");
    }
    if b.check(0, &[0x1f, 0xa0]) || b.check(0, &[0x1f, 0x9d]) {
        return ext("Z");
    }
    if b.check(0, b"LZIP") {
        return ext("lz");
    }
    if b.check(0, &[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]) {
        return ext("msi");
    }
    if b.check(
        0,
        &[
            0x06, 0x0e, 0x2b, 0x34, 0x02, 0x05, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01, 0x02,
        ],
    ) {
        return ext("mxf");
    }
    if b.check(4, &[0x47]) && (b.check(192, &[0x47]) || b.check(196, &[0x47])) {
        return ext("mts");
    }
    if b.check(0, b"BLENDER") {
        return ext("blend");
    }
    if b.check(0, &[0x42, 0x50, 0x47, 0xfb]) {
        return ext("bpg");
    }
    if b.check(
        0,
        &[
            0x00, 0x00, 0x00, 0x0c, 0x6a, 0x50, 0x20, 0x20, 0x0d, 0x0a, 0x87, 0x0a,
        ],
    ) {
        if b.check(20, b"jp2 ") {
            return ext("jp2");
        }
        if b.check(20, b"jpx ") {
            return ext("jpx");
        }
        if b.check(20, b"jpm ") {
            return ext("jpm");
        }
        if b.check(20, b"mjp2") {
            return ext("mj2");
        }
    }
    if b.check(0, b"FORM") {
        return ext("aif");
    }
    if b.check(0, b"<?xml ") {
        return ext("xml");
    }
    if b.check(60, b"BOOKMOBI") {
        return ext("mobi");
    }
    if b.check(
        0,
        &[
            0xab, 0x4b, 0x54, 0x58, 0x20, 0x31, 0x31, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a,
        ],
    ) {
        return ext("ktx");
    }
    if b.check(128, b"DICM") {
        return ext("dcm");
    }
    if b.check(0, b"MP+") || b.check(0, b"MPCK") {
        return ext("mpc");
    }
    if b.check(0, b"BEGIN:") {
        return ext("ics");
    }
    if b.check(0, &[0x67, 0x6c, 0x54, 0x46, 0x02, 0x00, 0x00, 0x00]) {
        return ext("glb");
    }
    if b.check(0, &[0xd4, 0xc3, 0xb2, 0xa1]) || b.check(0, &[0xa1, 0xb2, 0xc3, 0xd4]) {
        return ext("pcap");
    }
    if b.check(0, b"DSD ") {
        return ext("dsf");
    }
    if b.check(
        0,
        &[
            0x4c, 0x00, 0x00, 0x00, 0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
        ],
    ) {
        return ext("lnk");
    }
    if b.check(
        0,
        &[
            0x62, 0x6f, 0x6f, 0x6b, 0x00, 0x00, 0x00, 0x00, 0x6d, 0x61, 0x72, 0x6b, 0x00, 0x00,
            0x00, 0x00,
        ],
    ) {
        return ext("alias");
    }
    if b.check(0, b"Creative Voice File") {
        return ext("voc");
    }
    if b.check(0, &[0x0b, 0x77]) {
        return ext("ac3");
    }
    Guess::Unknown
}

/// `fileType(buffer)` from file-type 4.4.0, as far as `archive-type` can tell the results
/// apart: every check up to the last archive signature (`xz`) runs in upstream's order, and any
/// later match is reported as `None` exactly like a non-archive match would be.
fn file_type_4(bytes: &[u8]) -> Option<&'static str> {
    let b = Buf(bytes);
    if bytes.len() <= 1 {
        return None;
    }
    let tiff = b.check(0, &[0x49, 0x49, 0x2a, 0x00]) || b.check(0, &[0x4d, 0x4d, 0x00, 0x2a]);
    let checks = [
        (b.check(0, &[0xff, 0xd8, 0xff]), "jpg"),
        (
            b.check(0, &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
            "png",
        ),
        (b.check(0, &[0x47, 0x49, 0x46]), "gif"),
        (b.check(8, &[0x57, 0x45, 0x42, 0x50]), "webp"),
        (b.check(0, &[0x46, 0x4c, 0x49, 0x46]), "flif"),
        (tiff && b.check(8, &[0x43, 0x52]), "cr2"),
        (tiff, "tif"),
        (b.check(0, &[0x42, 0x4d]), "bmp"),
        (b.check(0, &[0x49, 0x49, 0xbc]), "jxr"),
        (b.check(0, &[0x38, 0x42, 0x50, 0x53]), "psd"),
        (
            b.check(0, ZIP_LOCAL) && b.check(30, b"mimetypeapplication/epub+zip"),
            "epub",
        ),
        (
            b.check(0, ZIP_LOCAL) && b.check(30, b"META-INF/mozilla.rsa"),
            "xpi",
        ),
        (
            b.check(0, &[0x50, 0x4b])
                && b.is(2, &[0x03, 0x05, 0x07])
                && b.is(3, &[0x04, 0x06, 0x08]),
            "zip",
        ),
        (b.check(257, b"ustar"), "tar"),
        (
            b.check(0, &[0x52, 0x61, 0x72, 0x21, 0x1a, 0x07]) && b.is(6, &[0x00, 0x01]),
            "rar",
        ),
        (b.check(0, &[0x1f, 0x8b, 0x08]), "gz"),
        (b.check(0, &[0x42, 0x5a, 0x68]), "bz2"),
        (b.check(0, &[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]), "7z"),
        (b.check(0, &[0x78, 0x01]), "dmg"),
        (
            (b.check(0, &[0x00, 0x00, 0x00]) && b.is(3, &[0x18, 0x20]) && b.check(4, b"ftyp"))
                || b.check(0, b"3gp5")
                || (b.check(
                    0,
                    &[
                        0x00, 0x00, 0x00, 0x1c, 0x66, 0x74, 0x79, 0x70, 0x6d, 0x70, 0x34, 0x32,
                    ],
                ) && b.check(16, b"mp41mp42isom"))
                || b.check(
                    0,
                    &[
                        0x00, 0x00, 0x00, 0x1c, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d,
                    ],
                )
                || b.check(
                    0,
                    &[
                        0x00, 0x00, 0x00, 0x1c, 0x66, 0x74, 0x79, 0x70, 0x6d, 0x70, 0x34, 0x32,
                        0x00, 0x00, 0x00, 0x00,
                    ],
                ),
            "mp4",
        ),
        (
            b.check(
                0,
                &[
                    0x00, 0x00, 0x00, 0x1c, 0x66, 0x74, 0x79, 0x70, 0x4d, 0x34, 0x56,
                ],
            ),
            "m4v",
        ),
        (b.check(0, &[0x4d, 0x54, 0x68, 0x64]), "mid"),
        (
            b.check(0, &[0x1a, 0x45, 0xdf, 0xa3]) && matroska(&b).is_some(),
            "mkv",
        ),
        (
            b.check(
                0,
                &[
                    0x00, 0x00, 0x00, 0x14, 0x66, 0x74, 0x79, 0x70, 0x71, 0x74, 0x20, 0x20,
                ],
            ) || b.check(4, b"free")
                || b.check(4, b"ftypqt  ")
                || b.check(4, b"mdat")
                || b.check(4, b"wide"),
            "mov",
        ),
        (b.check(0, b"RIFF") && b.check(8, b"AVI"), "avi"),
        (
            b.check(
                0,
                &[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11, 0xa6, 0xd9],
            ),
            "wmv",
        ),
        (b.check(0, &[0x00, 0x00, 0x01, 0xba]), "mpg"),
        (b.check(0, b"ID3") || b.check(0, &[0xff, 0xfb]), "mp3"),
        (b.check(4, b"ftypM4A") || b.check(0, b"M4A "), "m4a"),
        (b.check(28, b"OpusHead"), "opus"),
        (b.check(0, b"OggS"), "ogg"),
        (b.check(0, b"fLaC"), "flac"),
        (b.check(0, b"RIFF") && b.check(8, b"WAVE"), "wav"),
        (b.check(0, b"#!AMR\n"), "amr"),
        (b.check(0, b"%PDF"), "pdf"),
        (b.check(0, b"MZ"), "exe"),
        (b.is(0, &[0x43, 0x46]) && b.check(1, b"WS"), "swf"),
        (b.check(0, b"{\\rtf"), "rtf"),
        (b.check(0, &[0x00, 0x61, 0x73, 0x6d]), "wasm"),
        (
            b.check(0, b"wOFF") && (b.check(4, &[0x00, 0x01, 0x00, 0x00]) || b.check(4, b"OTTO")),
            "woff",
        ),
        (
            b.check(0, b"wOF2") && (b.check(4, &[0x00, 0x01, 0x00, 0x00]) || b.check(4, b"OTTO")),
            "woff2",
        ),
        (
            b.check(34, b"LP")
                && (b.check(8, &[0x00, 0x00, 0x01])
                    || b.check(8, &[0x01, 0x00, 0x02])
                    || b.check(8, &[0x02, 0x00, 0x02])),
            "eot",
        ),
        (b.check(0, &[0x00, 0x01, 0x00, 0x00, 0x00]), "ttf"),
        (b.check(0, &[0x4f, 0x54, 0x54, 0x4f, 0x00]), "otf"),
        (b.check(0, &[0x00, 0x00, 0x01, 0x00]), "ico"),
        (b.check(0, &[0x46, 0x4c, 0x56, 0x01]), "flv"),
        (b.check(0, b"%!"), "ps"),
        (b.check(0, &[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]), "xz"),
    ];
    checks.iter().find(|(hit, _)| *hit).map(|(_, ext)| *ext)
}

/// `archive-type`: the file-type 4 extension when it names an archive.
pub fn archive_type(bytes: &[u8]) -> Option<&'static str> {
    file_type_4(bytes)
        .filter(|ext| matches!(*ext, "7z" | "bz2" | "gz" | "rar" | "tar" | "zip" | "xz"))
}
