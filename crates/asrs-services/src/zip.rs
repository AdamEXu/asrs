//! `decompress-unzip` 4 on `yauzl` 2.10: central-directory parsing, entry
//! decoding (stored and raw-deflate) and the file/directory classification
//! `decompress` uses when writing an archive out.

use crate::Invalid;

const CP437: &str = "\u{0}☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼ !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~⌂ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

pub struct ZipEntry {
    pub name: String,
    pub kind: EntryKind,
    /// `(externalFileAttributes >> 16) & 0xFFFF`, with decompress-unzip's 0 -> 493/420 default.
    pub mode: u32,
    pub data: Vec<u8>,
}

fn u16le(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32le(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn u64le(buf: &[u8], at: usize) -> u64 {
    u64::from(u32le(buf, at + 4)) * 0x1_0000_0000 + u64::from(u32le(buf, at))
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *e = c;
    }
    data.iter().fold(!0u32, |c, &b| {
        table[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    }) ^ !0
}

fn decode_name(bytes: &[u8], utf8: bool) -> String {
    if utf8 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let table: Vec<char> = CP437.chars().collect();
        bytes.iter().map(|&b| table[usize::from(b)]).collect()
    }
}

fn past(start: usize, len: usize, size: usize) -> bool {
    start.checked_add(len).is_none_or(|end| end > size)
}

fn validate_name(name: &str) -> Result<(), Invalid> {
    if name.contains('\\') {
        return Err(Invalid);
    }
    if name.starts_with('/')
        || name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && name[1..].starts_with(':')
    {
        return Err(Invalid);
    }
    if name.split('/').any(|seg| seg == "..") {
        return Err(Invalid);
    }
    Ok(())
}

/// `yauzl.fromBuffer` plus `decompress-unzip`'s entry extraction: every entry is
/// parsed and its data decoded before anything is returned, so a failure
/// anywhere means nothing was written.
pub fn read_entries(buf: &[u8]) -> Result<Vec<ZipEntry>, Invalid> {
    let size = buf.len();
    let window = (22 + 0xffff).min(size);
    if window < 22 {
        return Err(Invalid);
    }
    let start = size - window;
    let mut entry_count: u64 = 0;
    let mut cd_offset: u64 = 0;
    let mut found = false;
    for i in (0..=window - 22).rev() {
        let at = start + i;
        if u32le(buf, at) != 0x0605_4b50 {
            continue;
        }
        let eocdr = &buf[at..];
        if u16le(eocdr, 4) != 0 {
            return Err(Invalid);
        }
        if usize::from(u16le(eocdr, 20)) != eocdr.len() - 22 {
            return Err(Invalid);
        }
        entry_count = u64::from(u16le(eocdr, 10));
        cd_offset = u64::from(u32le(eocdr, 16));
        if u16le(eocdr, 10) == 0xffff || u32le(eocdr, 16) == 0xffff_ffff {
            let loc = at.checked_sub(20).ok_or(Invalid)?;
            if u32le(buf, loc) != 0x0706_4b50 {
                return Err(Invalid);
            }
            let eocdr64 = usize::try_from(u64le(buf, loc + 8)).map_err(|_| Invalid)?;
            if past(eocdr64, 56, size) || u32le(buf, eocdr64) != 0x0606_4b50 {
                return Err(Invalid);
            }
            entry_count = u64le(buf, eocdr64 + 32);
            cd_offset = u64le(buf, eocdr64 + 48);
        }
        found = true;
        break;
    }
    if !found {
        return Err(Invalid);
    }
    let mut entries = Vec::new();
    let mut cursor = usize::try_from(cd_offset).map_err(|_| Invalid)?;
    for _ in 0..entry_count {
        if past(cursor, 46, size) {
            return Err(Invalid);
        }
        let h = &buf[cursor..];
        if u32le(h, 0) != 0x0201_4b50 {
            return Err(Invalid);
        }
        let made_by = u16le(h, 4);
        let flags = u16le(h, 8);
        let method = u16le(h, 10);
        let mut compressed = u64::from(u32le(h, 20));
        let mut uncompressed = u64::from(u32le(h, 24));
        let name_len = usize::from(u16le(h, 28));
        let extra_len = usize::from(u16le(h, 30));
        let comment_len = usize::from(u16le(h, 32));
        let external = u32le(h, 38);
        let mut local_offset = u64::from(u32le(h, 42));
        if flags & 0x40 != 0 {
            return Err(Invalid);
        }
        cursor += 46;
        if past(cursor, name_len + extra_len + comment_len, size) {
            return Err(Invalid);
        }
        let name_bytes = &buf[cursor..cursor + name_len];
        let extra = &buf[cursor + name_len..cursor + name_len + extra_len];
        let utf8 = flags & 0x800 != 0;
        let mut name = decode_name(name_bytes, utf8);
        let mut extras = Vec::new();
        let mut i = 0;
        while i + 3 < extra.len() {
            let id = u16le(extra, i);
            let data_end = i + 4 + usize::from(u16le(extra, i + 2));
            if data_end > extra.len() {
                return Err(Invalid);
            }
            extras.push((id, &extra[i + 4..data_end]));
            i = data_end;
        }
        cursor += name_len + extra_len + comment_len;
        if compressed == 0xffff_ffff || uncompressed == 0xffff_ffff || local_offset == 0xffff_ffff {
            let eief = extras
                .iter()
                .find(|(id, _)| *id == 0x0001)
                .map(|(_, d)| *d)
                .ok_or(Invalid)?;
            let mut idx = 0usize;
            for field in [&mut uncompressed, &mut compressed, &mut local_offset] {
                if *field == 0xffff_ffff {
                    if idx + 8 > eief.len() {
                        return Err(Invalid);
                    }
                    *field = u64le(eief, idx);
                    idx += 8;
                }
            }
        }
        for (id, data) in &extras {
            if *id == 0x7075
                && data.len() >= 6
                && data[0] == 1
                && u32le(data, 1) == crc32(name_bytes)
            {
                name = String::from_utf8_lossy(&data[5..]).into_owned();
                break;
            }
        }
        if method == 0 && compressed != uncompressed + u64::from(flags & 1 != 0) * 12 {
            return Err(Invalid);
        }
        name = name.replace('\\', "/");
        validate_name(&name)?;
        if flags & 1 != 0 {
            return Err(Invalid);
        }
        let local = usize::try_from(local_offset).map_err(|_| Invalid)?;
        if past(local, 30, size) || u32le(buf, local) != 0x0403_4b50 {
            return Err(Invalid);
        }
        let data_start =
            local + 30 + usize::from(u16le(buf, local + 26)) + usize::from(u16le(buf, local + 28));
        if method != 0 && method != 8 {
            return Err(Invalid);
        }
        let compressed = usize::try_from(compressed).unwrap_or(usize::MAX);
        let raw: &[u8] = if compressed == 0 {
            &[]
        } else if past(data_start, compressed, size) {
            return Err(Invalid);
        } else {
            &buf[data_start..data_start + compressed]
        };
        let data = if method == 0 {
            raw.to_vec()
        } else {
            let limit = usize::try_from(uncompressed).map_err(|_| Invalid)?;
            let data = miniz_oxide::inflate::decompress_to_vec_with_limit(raw, limit)
                .map_err(|_| Invalid)?;
            if data.len() as u64 != uncompressed {
                return Err(Invalid);
            }
            data
        };
        let mut mode = (external >> 16) & 0xffff;
        let kind = if mode & 0xf000 == 0xa000 {
            EntryKind::Symlink
        } else if mode & 0xf000 == 0x4000 || (made_by >> 8 == 0 && external == 16) {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        if mode == 0 && kind == EntryKind::Directory {
            mode = 0o755;
        }
        if mode == 0 {
            mode = 0o644;
        }
        entries.push(ZipEntry {
            name,
            kind,
            mode,
            data,
        });
    }
    Ok(entries)
}

/// The `decompress-unzip` plugin: buffers its own file-type 3.9 does not call a zip yield no
/// entries. For every buffer, that check agrees with `archive-type`'s (file-type 4.4) `zip`.
pub fn decompress_unzip(buf: &[u8]) -> Result<Vec<ZipEntry>, Invalid> {
    if crate::file_type::archive_type(buf) == Some("zip") {
        read_entries(buf)
    } else {
        Ok(Vec::new())
    }
}
