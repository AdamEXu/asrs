// Ported from skcms, Chromium m142 (BSD-3-Clause, Copyright 2018 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! What Chromium does to an image tagged with an RGB matrix/TRC ICC profile when it draws it
//! into an sRGB canvas: skcms's tone curves (its approximate `pow`), the profile's matrix to XYZ
//! D50 followed by the inverse of sRGB's, sRGB's inverse transfer function, and a store that
//! rounds half up.

use std::collections::HashMap;

/// `skcms_TransferFunction`: `c*x + f` below `d`, `(a*x + b)^g + e` from `d` on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TransferFunction {
    g: f32,
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

const SRGB_TF: TransferFunction = TransferFunction {
    g: 2.4,
    a: (1.0 / 1.055) as f32,
    b: (0.055 / 1.055) as f32,
    c: (1.0 / 12.92) as f32,
    d: 0.04045,
    e: 0.0,
    f: 0.0,
};

/// `skcms_sRGB_profile`'s `toXYZD50`.
const SRGB_TO_XYZ_D50: [[f64; 3]; 3] = [
    [0.436065674, 0.385147095, 0.143066406],
    [0.222488403, 0.716873169, 0.060607910],
    [0.013916016, 0.097076416, 0.714096069],
];

#[derive(Clone, Debug, PartialEq)]
enum Curve {
    Parametric(TransferFunction),
    Table(Vec<f32>),
}

fn approx_log2(x: f32) -> f32 {
    let bits = x.to_bits() as i32;
    let e = bits as f32 * (1.0 / (1 << 23) as f32);
    let m = f32::from_bits(((bits & 0x007f_ffff) | 0x3f00_0000) as u32);
    e - 124.225_52 - 1.498_030_3 * m - 1.725_88 / (0.352_088_7 + m)
}

fn approx_exp2(x: f32) -> f32 {
    let fract = x - x.floor();
    let fbits = (1u32 << 23) as f32
        * (x + 121.274_06 - 1.490_129_1 * fract + 27.728_024 / (4.842_525_7 - fract));
    let fbits = fbits.max(0.0).min(f32::from_bits(0x4eff_0000));
    f32::from_bits(fbits as i32 as u32)
}

fn approx_pow(x: f32, y: f32) -> f32 {
    if x == 0.0 || x == 1.0 {
        x
    } else {
        approx_exp2(approx_log2(x) * y)
    }
}

impl TransferFunction {
    fn apply(&self, x: f32) -> f32 {
        let sign = x.to_bits() & 0x8000_0000;
        let x = f32::from_bits(x.to_bits() ^ sign);
        let v = if x < self.d {
            self.c * x + self.f
        } else {
            approx_pow(self.a * x + self.b, self.g) + self.e
        };
        f32::from_bits(sign | v.to_bits())
    }

    /// `skcms_TransferFunction_invert` for a function without a linear-segment offset.
    fn invert(&self) -> TransferFunction {
        let d_l = self.c * self.d + self.f;
        let a = self.a.powf(-self.g);
        TransferFunction {
            g: 1.0 / self.g,
            a,
            b: -a * self.e,
            c: if self.c == 0.0 { 0.0 } else { 1.0 / self.c },
            d: d_l,
            e: -self.b / self.a,
            f: if self.c == 0.0 { 0.0 } else { -self.f / self.c },
        }
    }

    fn approx_eq(&self, other: &TransferFunction) -> bool {
        let close = |a: f32, b: f32| (a - b).abs() < 0.001;
        close(self.g, other.g)
            && close(self.a, other.a)
            && close(self.b, other.b)
            && close(self.c, other.c)
            && close(self.d, other.d)
            && close(self.e, other.e)
            && close(self.f, other.f)
    }
}

impl Curve {
    fn apply(&self, x: f32) -> f32 {
        match self {
            Curve::Parametric(tf) => tf.apply(x),
            Curve::Table(table) => {
                let last = (table.len() - 1) as f32;
                let i = (x.clamp(0.0, 1.0) * last).max(0.0);
                let lo = i.floor() as usize;
                let hi = (lo + 1).min(table.len() - 1);
                let t = i - lo as f32;
                table[lo] + (table[hi] - table[lo]) * t
            }
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn s15_fixed16(b: &[u8], at: usize) -> Option<f32> {
    Some(u32_at(b, at)? as i32 as f32 / 65536.0)
}

fn tag<'a>(icc: &'a [u8], signature: &[u8; 4]) -> Option<&'a [u8]> {
    let count = u32_at(icc, 128)? as usize;
    (0..count).find_map(|i| {
        let entry = 132 + 12 * i;
        if icc.get(entry..entry + 4)? != signature {
            return None;
        }
        let offset = u32_at(icc, entry + 4)? as usize;
        let size = u32_at(icc, entry + 8)? as usize;
        icc.get(offset..offset.checked_add(size)?)
    })
}

fn parse_curve(data: &[u8]) -> Option<Curve> {
    match data.get(0..4)? {
        b"curv" => {
            let count = u32_at(data, 8)? as usize;
            match count {
                0 => Some(Curve::Parametric(TransferFunction {
                    g: 1.0,
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 0.0,
                    e: 0.0,
                    f: 0.0,
                })),
                1 => {
                    let g =
                        f32::from(u16::from_be_bytes(data.get(12..14)?.try_into().ok()?)) / 256.0;
                    Some(Curve::Parametric(TransferFunction {
                        g,
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: 0.0,
                        e: 0.0,
                        f: 0.0,
                    }))
                }
                n => {
                    let table = (0..n)
                        .map(|i| {
                            let at = 12 + 2 * i;
                            data.get(at..at + 2)
                                .map(|v| f32::from(u16::from_be_bytes([v[0], v[1]])) / 65535.0)
                        })
                        .collect::<Option<Vec<f32>>>()?;
                    Some(Curve::Table(table))
                }
            }
        }
        b"para" => {
            let kind = u16::from_be_bytes(data.get(8..10)?.try_into().ok()?);
            let param = |i: usize| s15_fixed16(data, 12 + 4 * i);
            let g = param(0)?;
            let mut tf = TransferFunction {
                g,
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 0.0,
                e: 0.0,
                f: 0.0,
            };
            match kind {
                0 => {}
                1 => {
                    tf.a = param(1)?;
                    tf.b = param(2)?;
                    tf.d = -tf.b / tf.a;
                }
                2 => {
                    tf.a = param(1)?;
                    tf.b = param(2)?;
                    tf.e = param(3)?;
                    tf.d = -tf.b / tf.a;
                    tf.f = tf.e;
                }
                3 => {
                    tf.a = param(1)?;
                    tf.b = param(2)?;
                    tf.c = param(3)?;
                    tf.d = param(4)?;
                }
                4 => {
                    tf.a = param(1)?;
                    tf.b = param(2)?;
                    tf.c = param(3)?;
                    tf.d = param(4)?;
                    tf.e = param(5)?;
                    tf.f = param(6)?;
                }
                _ => return None,
            }
            Some(Curve::Parametric(tf))
        }
        _ => None,
    }
}

fn parse_xyz(data: &[u8]) -> Option<[f32; 3]> {
    if data.get(0..4)? != b"XYZ " {
        return None;
    }
    Some([
        s15_fixed16(data, 8)?,
        s15_fixed16(data, 12)?,
        s15_fixed16(data, 16)?,
    ])
}

fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    [
        [
            c(1, 1, 2, 2) / det,
            -c(0, 1, 2, 2) / det,
            c(0, 1, 1, 2) / det,
        ],
        [
            -c(1, 0, 2, 2) / det,
            c(0, 0, 2, 2) / det,
            -c(0, 0, 1, 2) / det,
        ],
        [
            c(1, 0, 2, 1) / det,
            -c(0, 0, 2, 1) / det,
            c(0, 0, 1, 1) / det,
        ],
    ]
}

/// The conversion of one source profile's pixels into sRGB.
#[derive(Clone, Debug, PartialEq)]
pub struct IccTransform {
    curves: [Curve; 3],
    matrix: [[f32; 3]; 3],
    encode: TransferFunction,
}

impl IccTransform {
    /// The transform for an RGB display profile with `rXYZ`/`gXYZ`/`bXYZ` and `rTRC`/`gTRC`/
    /// `bTRC` tags, or `None` for any other profile and for one that is sRGB already, which
    /// Chromium draws untouched.
    pub fn from_icc(icc: &[u8]) -> Option<IccTransform> {
        if icc.get(16..20)? != b"RGB " || icc.get(20..24)? != b"XYZ " {
            return None;
        }
        let columns = [
            parse_xyz(tag(icc, b"rXYZ")?)?,
            parse_xyz(tag(icc, b"gXYZ")?)?,
            parse_xyz(tag(icc, b"bXYZ")?)?,
        ];
        let curves = [
            parse_curve(tag(icc, b"rTRC")?)?,
            parse_curve(tag(icc, b"gTRC")?)?,
            parse_curve(tag(icc, b"bTRC")?)?,
        ];
        let to_xyz: [[f32; 3]; 3] =
            std::array::from_fn(|row| std::array::from_fn(|col| columns[col][row]));
        let srgb_matrix = to_xyz.iter().zip(SRGB_TO_XYZ_D50).all(|(a, b)| {
            a.iter()
                .zip(b)
                .all(|(x, y)| (f64::from(*x) - y).abs() < 0.01)
        });
        let srgb_curves = curves
            .iter()
            .all(|c| matches!(c, Curve::Parametric(tf) if tf.approx_eq(&SRGB_TF)));
        if srgb_matrix && srgb_curves {
            return None;
        }
        let from_xyz = invert(SRGB_TO_XYZ_D50).map(|row| row.map(|v| v as f32));
        let matrix = std::array::from_fn(|r| {
            std::array::from_fn(|c| {
                from_xyz[r][0] * to_xyz[0][c]
                    + from_xyz[r][1] * to_xyz[1][c]
                    + from_xyz[r][2] * to_xyz[2][c]
            })
        });
        Some(IccTransform {
            curves,
            matrix,
            encode: SRGB_TF.invert(),
        })
    }

    pub fn convert(&self, rgb: [u8; 3]) -> [u8; 3] {
        let lin: [f32; 3] =
            std::array::from_fn(|i| self.curves[i].apply(f32::from(rgb[i]) * (1.0 / 255.0)));
        std::array::from_fn(|r| {
            let m = self.matrix[r];
            let v = lin[0] * m[0] + lin[1] * m[1] + lin[2] * m[2];
            let v = self.encode.apply(v.max(0.0)).clamp(0.0, 1.0);
            (v * 255.0 + 0.5) as u8
        })
    }

    /// Converts straight-alpha RGBA in place.
    pub fn apply(&self, rgba: &mut [u8]) {
        let mut cache: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
        for p in rgba.as_chunks_mut::<4>().0 {
            let rgb = [p[0], p[1], p[2]];
            let out = *cache.entry(rgb).or_insert_with(|| self.convert(rgb));
            p[..3].copy_from_slice(&out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adobe_rgb() -> Vec<u8> {
        let mut icc = vec![0u8; 128];
        icc[16..20].copy_from_slice(b"RGB ");
        icc[20..24].copy_from_slice(b"XYZ ");
        let tags: [(&[u8; 4], Vec<u8>); 6] = [
            (b"rXYZ", xyz([0x0000_9c18, 0x0000_4fa5, 0x0000_04fc])),
            (b"gXYZ", xyz([0x0000_348d, 0x0000_a02c, 0x0000_0f95])),
            (b"bXYZ", xyz([0x0000_2631, 0x0000_102f, 0x0000_be9c])),
            (b"rTRC", curv(0x0233)),
            (b"gTRC", curv(0x0233)),
            (b"bTRC", curv(0x0233)),
        ];
        icc.extend_from_slice(&(tags.len() as u32).to_be_bytes());
        let mut offset = 132 + 12 * tags.len();
        let mut data = Vec::new();
        for (sig, body) in &tags {
            icc.extend_from_slice(*sig);
            icc.extend_from_slice(&(offset as u32).to_be_bytes());
            icc.extend_from_slice(&(body.len() as u32).to_be_bytes());
            offset += body.len();
            data.extend_from_slice(body);
        }
        icc.extend_from_slice(&data);
        icc
    }

    fn xyz(v: [u32; 3]) -> Vec<u8> {
        let mut out = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            out.extend_from_slice(&c.to_be_bytes());
        }
        out
    }

    fn curv(gamma: u16) -> Vec<u8> {
        let mut out = b"curv\0\0\0\0".to_vec();
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&gamma.to_be_bytes());
        out
    }

    #[test]
    fn adobe_rgb_field_colours_match_chromium() {
        let t = IccTransform::from_icc(&adobe_rgb()).expect("matrix/TRC profile");
        for (src, chromium) in [
            ([92, 92, 92], [92, 92, 92]),
            ([168, 168, 168], [169, 170, 169]),
            ([176, 176, 176], [177, 178, 177]),
            ([255, 255, 255], [255, 255, 255]),
        ] {
            assert_eq!(t.convert(src), chromium, "{src:?}");
        }
    }
}
