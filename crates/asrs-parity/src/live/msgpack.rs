//! The mock servers' MessagePack writer and reader, mirroring `oracle/src/live/msgpack.ts`.

#[derive(Clone, Debug, PartialEq)]
pub enum Packed {
    Nil,
    Bool(bool),
    Int(f64),
    F32(f64),
    F64(f64),
    Str(String),
    Bin(Vec<u8>),
    Arr(Vec<Packed>),
    Raw(Vec<u8>),
}

pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

pub fn is_safe_integer(n: f64) -> bool {
    n.is_finite() && n.trunc() == n && n.abs() <= MAX_SAFE_INTEGER
}

/// @msgpack/msgpack's rule for a JS number: a safe integer takes an integer form.
pub fn number(n: f64) -> Packed {
    if is_safe_integer(n) {
        Packed::Int(n)
    } else {
        Packed::F64(n)
    }
}

fn write_int(out: &mut Vec<u8>, n: f64) {
    let n = n as i64;
    if n >= 0 {
        if n < 0x80 {
            out.push(n as u8);
        } else if n < 0x100 {
            out.extend([0xcc, n as u8]);
        } else if n < 0x1_0000 {
            out.push(0xcd);
            out.extend((n as u16).to_be_bytes());
        } else if n < 0x1_0000_0000 {
            out.push(0xce);
            out.extend((n as u32).to_be_bytes());
        } else {
            out.push(0xcf);
            out.extend((n as u64).to_be_bytes());
        }
    } else if n >= -0x20 {
        out.push(n as u8);
    } else if n >= -0x80 {
        out.extend([0xd0, n as u8]);
    } else if n >= -0x8000 {
        out.push(0xd1);
        out.extend((n as i16).to_be_bytes());
    } else if n >= -0x8000_0000 {
        out.push(0xd2);
        out.extend((n as i32).to_be_bytes());
    } else {
        out.push(0xd3);
        out.extend(n.to_be_bytes());
    }
}

fn write_length(out: &mut Vec<u8>, len: usize, small: Option<(u8, usize)>, codes: [Option<u8>; 3]) {
    match (small, codes[0]) {
        (Some((tag, limit)), _) if len < limit => out.push(tag | len as u8),
        (_, Some(code)) if len < 0x100 => out.extend([code, len as u8]),
        _ if len < 0x1_0000 => {
            out.push(codes[1].expect("16-bit form"));
            out.extend((len as u16).to_be_bytes());
        }
        _ => {
            out.push(codes[2].expect("32-bit form"));
            out.extend((len as u32).to_be_bytes());
        }
    }
}

fn write(out: &mut Vec<u8>, value: &Packed) {
    match value {
        Packed::Nil => out.push(0xc0),
        Packed::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Packed::Int(n) => write_int(out, *n),
        Packed::F32(n) => {
            out.push(0xca);
            out.extend((*n as f32).to_be_bytes());
        }
        Packed::F64(n) => {
            out.push(0xcb);
            out.extend(n.to_be_bytes());
        }
        Packed::Str(s) => {
            write_length(
                out,
                s.len(),
                Some((0xa0, 32)),
                [Some(0xd9), Some(0xda), Some(0xdb)],
            );
            out.extend(s.as_bytes());
        }
        Packed::Bin(b) => {
            write_length(out, b.len(), None, [Some(0xc4), Some(0xc5), Some(0xc6)]);
            out.extend(b);
        }
        Packed::Arr(items) => {
            write_length(
                out,
                items.len(),
                Some((0x90, 16)),
                [None, Some(0xdc), Some(0xdd)],
            );
            items.iter().for_each(|item| write(out, item));
        }
        Packed::Raw(bytes) => out.extend(bytes),
    }
}

pub fn encode(values: &[Packed]) -> Vec<u8> {
    let mut out = Vec::new();
    values.iter().for_each(|v| write(&mut out, v));
    out
}

/// A decoded value; maps and extensions are read past and come back as `Skipped`.
#[derive(Clone, Debug, PartialEq)]
pub enum Unpacked {
    Nil,
    Skipped,
    Bool(bool),
    Number(f64),
    Str,
    Bin(Vec<u8>),
    Arr(Vec<Unpacked>),
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

type Read<T> = Result<T, ()>;

impl Reader<'_> {
    fn u8(&mut self) -> Read<u8> {
        let b = *self.bytes.get(self.pos).ok_or(())?;
        self.pos += 1;
        Ok(b)
    }

    /// Accumulated in f64 as the TS reader does, so values past 2^53 round identically.
    fn be(&mut self, width: usize) -> Read<f64> {
        let mut v = 0.0;
        for _ in 0..width {
            v = v * 256.0 + f64::from(self.u8()?);
        }
        Ok(v)
    }

    fn take(&mut self, n: usize) -> Read<&[u8]> {
        let end = self.pos.checked_add(n).ok_or(())?;
        let out = self.bytes.get(self.pos..end).ok_or(())?;
        self.pos = end;
        Ok(out)
    }

    fn signed(&mut self, width: usize) -> Read<f64> {
        let v = self.be(width)?;
        let half = 2f64.powi(8 * width as i32 - 1);
        Ok(if v >= half { v - 2.0 * half } else { v })
    }

    fn value(&mut self) -> Read<Unpacked> {
        let b = self.u8()?;
        Ok(match b {
            0x00..=0x7f => Unpacked::Number(f64::from(b)),
            0xe0..=0xff => Unpacked::Number(f64::from(b) - 256.0),
            0xa0..=0xbf => {
                self.take(usize::from(b & 0x1f))?;
                Unpacked::Str
            }
            0x90..=0x9f => self.array(usize::from(b & 0x0f))?,
            0x80..=0x8f => self.skip_map(usize::from(b & 0x0f))?,
            0xc0 => Unpacked::Nil,
            0xc2 => Unpacked::Bool(false),
            0xc3 => Unpacked::Bool(true),
            0xc4..=0xc6 => {
                let n = self.be(1 << (b - 0xc4))? as usize;
                Unpacked::Bin(self.take(n)?.to_vec())
            }
            0xc7..=0xc9 => {
                let n = self.be(1 << (b - 0xc7))? as usize;
                self.take(n + 1)?;
                Unpacked::Skipped
            }
            0xca => {
                let bytes = self.take(4)?;
                Unpacked::Number(f64::from(f32::from_be_bytes(
                    bytes.try_into().map_err(|_| ())?,
                )))
            }
            0xcb => {
                let bytes = self.take(8)?;
                Unpacked::Number(f64::from_be_bytes(bytes.try_into().map_err(|_| ())?))
            }
            0xcc..=0xcf => Unpacked::Number(self.be(1 << (b - 0xcc))?),
            0xd0..=0xd2 => Unpacked::Number(self.signed(1 << (b - 0xd0))?),
            0xd3 => {
                let hi = self.be(4)?;
                let lo = self.be(4)?;
                let hi = if hi >= 2_147_483_648.0 {
                    hi - 4_294_967_296.0
                } else {
                    hi
                };
                Unpacked::Number(hi * 4_294_967_296.0 + lo)
            }
            0xd4..=0xd8 => {
                self.take(1 + (1 << (b - 0xd4)))?;
                Unpacked::Skipped
            }
            0xd9..=0xdb => {
                let n = self.be(1 << (b - 0xd9))? as usize;
                self.take(n)?;
                Unpacked::Str
            }
            0xdc | 0xdd => {
                let n = self.be(if b == 0xdc { 2 } else { 4 })? as usize;
                self.array(n)?
            }
            0xde | 0xdf => {
                let n = self.be(if b == 0xde { 2 } else { 4 })? as usize;
                self.skip_map(n)?
            }
            _ => return Err(()),
        })
    }

    fn array(&mut self, n: usize) -> Read<Unpacked> {
        let mut out = Vec::new();
        for _ in 0..n {
            out.push(self.value()?);
        }
        Ok(Unpacked::Arr(out))
    }

    fn skip_map(&mut self, n: usize) -> Read<Unpacked> {
        for _ in 0..2 * n {
            self.value()?;
        }
        Ok(Unpacked::Skipped)
    }
}

/// Every value in `bytes` up to the first one that does not decode.
pub fn decode_all(bytes: &[u8]) -> Vec<Unpacked> {
    let mut reader = Reader { bytes, pos: 0 };
    let mut out = Vec::new();
    while reader.pos < bytes.len() {
        match reader.value() {
            Ok(v) => out.push(v),
            Err(()) => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_take_the_smallest_form() {
        let cases: [(f64, &str); 12] = [
            (0.0, "00"),
            (-0.0, "00"),
            (127.0, "7f"),
            (128.0, "cc80"),
            (65535.0, "cdffff"),
            (65536.0, "ce00010000"),
            (4_294_967_296.0, "cf0000000100000000"),
            (-32.0, "e0"),
            (-33.0, "d0df"),
            (-129.0, "d1ff7f"),
            (-32769.0, "d2ffff7fff"),
            (-2_147_483_649.0, "d3ffffffff7fffffff"),
        ];
        for (n, hex) in cases {
            let bytes = encode(&[number(n)]);
            let got: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, hex, "{n}");
            assert_eq!(decode_all(&bytes), vec![Unpacked::Number(n)]);
        }
        assert_eq!(number(0.5), Packed::F64(0.5));
    }

    #[test]
    fn decoding_stops_at_the_first_bad_value() {
        let mut bytes = encode(&[Packed::Arr(vec![
            Packed::Int(-1.0),
            Packed::Str("é".into()),
        ])]);
        bytes.extend([0xc1, 0x01]);
        assert_eq!(
            decode_all(&bytes),
            vec![Unpacked::Arr(vec![Unpacked::Number(-1.0), Unpacked::Str])]
        );
    }
}
