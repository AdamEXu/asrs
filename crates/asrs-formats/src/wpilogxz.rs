//! `.wpilogxz` decompression as main's `historical-start` does it: the whole file piped through
//! `lzma-native` 8.0.6's `createDecompressor()`, keeping whatever output arrived before an error.
//!
//! What reaches the decoder depends on how lzma-native drives liblzma, so this follows its
//! `JSLzmaStream` and `LZMAStream::doLZMACode` rather than decoding in one call:
//! - each written chunk is split after every `YZ` (the end of an xz stream footer), and every
//!   piece is a separate `lzma_code` loop with `LZMA_RUN`;
//! - output is collected in 64 KiB buffers and handed on only when a buffer fills, the piece's
//!   input runs out or the stream ends; an error drops the buffer that was being filled;
//! - `LZMA_STREAM_END` drops the rest of the piece and restarts the auto decoder, and the next
//!   pieces skip leading zero bytes until one has data left;
//! - the end of input is one more loop with `LZMA_FINISH`, where a second call without progress
//!   is `LZMA_BUF_ERROR`.

use xz2::stream::{Action, Status, Stream};

const BUFSIZE: usize = 65536;

enum Coded {
    Ok,
    StreamEnd,
    Error,
}

fn auto_decoder() -> Option<Stream> {
    Stream::new_auto_decoder(u64::MAX, 0).ok()
}

fn code(stream: &mut Stream, input: &[u8], finish: bool, out: &mut Vec<u8>) -> Coded {
    let mut buf = vec![0u8; BUFSIZE];
    let mut pos = 0;
    let action = if finish { Action::Finish } else { Action::Run };
    loop {
        let (in_before, out_before) = (stream.total_in(), stream.total_out());
        let status = stream.process(&input[pos..], &mut buf, action);
        pos += (stream.total_in() - in_before) as usize;
        let produced = (stream.total_out() - out_before) as usize;
        let status = match status {
            Ok(Status::Ok) => Coded::Ok,
            Ok(Status::StreamEnd) => Coded::StreamEnd,
            _ => return Coded::Error,
        };
        if produced == BUFSIZE || pos == input.len() || matches!(status, Coded::StreamEnd) {
            out.extend_from_slice(&buf[..produced]);
            if matches!(status, Coded::StreamEnd) {
                return status;
            }
        }
        if produced == 0 && !finish {
            return Coded::Ok;
        }
    }
}

/// `chunk.indexOf('YZ')` splitting from `JSLzmaStream._transform`, as a list of pieces.
fn pieces(mut chunk: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some(index) = chunk.windows(2).position(|w| w == b"YZ") {
        let end = index + 2;
        if end == chunk.len() {
            break;
        }
        out.push(&chunk[..end]);
        chunk = &chunk[end..];
    }
    out.push(chunk);
    out
}

/// The bytes main hands to the decoder for a `.wpilogxz` file whose contents are `compressed`:
/// everything decoded before the end of input or the first error.
pub fn decompress(compressed: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let Some(mut stream) = auto_decoder() else {
        return out;
    };
    let mut finished = false;
    for piece in pieces(compressed) {
        let mut piece = piece;
        if finished {
            let zeros = piece.iter().take_while(|&&b| b == 0).count();
            piece = &piece[zeros..];
            if !piece.is_empty() {
                finished = false;
            }
        }
        if piece.is_empty() {
            continue;
        }
        match code(&mut stream, piece, false, &mut out) {
            Coded::Ok => {}
            Coded::Error => return out,
            Coded::StreamEnd => {
                finished = true;
                match auto_decoder() {
                    Some(fresh) => stream = fresh,
                    None => return out,
                }
            }
        }
    }
    if !finished {
        code(&mut stream, &[], true, &mut out);
    }
    out
}
