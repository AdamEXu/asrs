//! `decompress-response` 3 on node's `zlib.createUnzip()`: a gzip or deflate response body.

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::inflate_flags::{
    TINFL_FLAG_HAS_MORE_INPUT, TINFL_FLAG_PARSE_ZLIB_HEADER,
    TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
};
use miniz_oxide::inflate::core::{DecompressorOxide, decompress};

use crate::Invalid;
use crate::zip::{MAX_UNPACKED, crc32};

/// What inflating a stream came to.
enum Step {
    /// It ended, after this much input.
    End(usize),
    /// The input ended first; what was inflated of it stays in the output.
    Short,
}

/// The body got reads for a response with this `content-encoding`. A compressed body that ends
/// early yields what it holds.
pub(crate) fn decode_body(encoding: Option<&str>, body: Vec<u8>) -> Result<Vec<u8>, Invalid> {
    if !matches!(encoding, Some("gzip" | "deflate")) {
        return Ok(body);
    }
    let mut out = Vec::new();
    decode_within(&body, MAX_UNPACKED as usize, &mut out)?;
    Ok(out)
}

fn decode_within(body: &[u8], limit: usize, out: &mut Vec<u8>) -> Result<(), Invalid> {
    if body.len() < 2 || body[..2] != [0x1f, 0x8b] {
        inflate(body, TINFL_FLAG_PARSE_ZLIB_HEADER, limit, out)?;
        return Ok(());
    }
    let mut rest = body;
    loop {
        match gunzip(rest, limit, out)? {
            Step::Short => return Ok(()),
            Step::End(used) => rest = &rest[used..],
        }
        if rest.first().is_none_or(|&b| b == 0) {
            return Ok(());
        }
    }
}

fn gunzip(data: &[u8], limit: usize, out: &mut Vec<u8>) -> Result<Step, Invalid> {
    let Some(body) = gzip_header(data)? else {
        return Ok(Step::Short);
    };
    let start = out.len();
    let Step::End(used) = inflate(&data[body..], 0, limit, out)? else {
        return Ok(Step::Short);
    };
    let trailer = body + used;
    let Some(check) = data.get(trailer..trailer + 8) else {
        return Ok(Step::Short);
    };
    let size = (out.len() - start) as u32;
    if check[..4] != crc32(&out[start..]).to_le_bytes() || check[4..] != size.to_le_bytes() {
        return Err(Invalid);
    }
    Ok(Step::End(trailer + 8))
}

/// Where the deflate data starts; `None` when the header is cut short.
fn gzip_header(data: &[u8]) -> Result<Option<usize>, Invalid> {
    const MAGIC: [u8; 3] = [0x1f, 0x8b, 8];
    let known = data.len().min(MAGIC.len());
    if data[..known] != MAGIC[..known] {
        return Err(Invalid);
    }
    let Some(&flags) = data.get(3) else {
        return Ok(None);
    };
    if flags & 0xe0 != 0 {
        return Err(Invalid);
    }
    let mut at = 10;
    if flags & 4 != 0 {
        let Some(len) = data.get(at..at + 2) else {
            return Ok(None);
        };
        at += 2 + usize::from(u16::from_le_bytes([len[0], len[1]]));
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            let text = data.get(at..).unwrap_or(&[]);
            let Some(end) = text.iter().position(|&b| b == 0) else {
                return Ok(None);
            };
            at += end + 1;
        }
    }
    if flags & 2 != 0 {
        let Some(check) = data.get(at..at + 2) else {
            return Ok(None);
        };
        if check != &crc32(&data[..at]).to_le_bytes()[..2] {
            return Err(Invalid);
        }
        at += 2;
    }
    Ok((at <= data.len()).then_some(at))
}

fn inflate(data: &[u8], flags: u32, limit: usize, out: &mut Vec<u8>) -> Result<Step, Invalid> {
    let flags = flags | TINFL_FLAG_HAS_MORE_INPUT | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let mut state = Box::<DecompressorOxide>::default();
    let start = out.len();
    let mut input = 0;
    let mut output = 0;
    loop {
        let room = (output * 2).max(1 << 15);
        out.resize(start + room, 0);
        let (status, read, written) =
            decompress(&mut state, &data[input..], &mut out[start..], output, flags);
        input += read;
        output += written;
        if start + output > limit {
            return Err(Invalid);
        }
        let step = match status {
            TINFLStatus::HasMoreOutput => continue,
            TINFLStatus::Done => Ok(Step::End(input)),
            TINFLStatus::NeedsMoreInput => Ok(Step::Short),
            _ => Err(Invalid),
        };
        out.truncate(start + output);
        return step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_unpacks_to_its_limit_and_no_further() {
        let data = vec![7; 100_000];
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        let unpacked = |limit| {
            let mut out = Vec::new();
            decode_within(&packed, limit, &mut out).map(|()| out)
        };
        assert!(unpacked(data.len()) == Ok(data.clone()));
        assert!(unpacked(data.len() - 1) == Err(Invalid));
        assert!(unpacked(0) == Err(Invalid));
    }
}
