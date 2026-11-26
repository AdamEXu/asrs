// Ported from skcms and Skia (SkColorSpace, SkColorSpaceXformSteps, SkRasterPipeline), Chromium m142 (BSD-3-Clause, Copyright Google Inc.). See THIRD_PARTY_NOTICES.md.

//! What Chromium does to a PNG with an embedded RGB matrix/TRC ICC profile that a canvas draws.
//! Blink tags the decoded pixels with `SkColorSpace::Make(profile)`, which snaps near-sRGB,
//! near-2.2 and near-linear curves to the named ones, and cc's software decode cache converts
//! them to the canvas's sRGB with `SkConvertPixels`: a highp raster pipeline of the source
//! transfer function, the gamut matrix, sRGB's inverse transfer function, a premultiply for an
//! image with alpha, and an 8-bit store that rounds to nearest even.

#![allow(clippy::excessive_precision)]

use std::collections::HashMap;

use tiny_skia::{IntSize, Pixmap};

use crate::pixels::to_unorm;

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

impl TransferFunction {
    const fn gamma(g: f32) -> TransferFunction {
        TransferFunction {
            g,
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        }
    }

    fn bits(&self) -> [u32; 7] {
        [self.g, self.a, self.b, self.c, self.d, self.e, self.f].map(f32::to_bits)
    }
}

const SRGB: TransferFunction = TransferFunction {
    g: 2.4,
    a: (1.0 / 1.055) as f32,
    b: (0.055 / 1.055) as f32,
    c: (1.0 / 12.92) as f32,
    d: 0.04045,
    e: 0.0,
    f: 0.0,
};

/// `skcms_sRGB_Inverse_TransferFunction`, which is also what `skcms_TransferFunction_invert`
/// gives for `SRGB`.
const SRGB_INVERSE: TransferFunction = TransferFunction {
    g: 0.416666657,
    a: 1.137283325,
    b: -0.0,
    c: 12.920000076,
    d: 0.003130805,
    e: -0.054969788,
    f: -0.0,
};

const TWO_DOT_TWO: TransferFunction = TransferFunction::gamma(2.2);

const LINEAR: TransferFunction = TransferFunction::gamma(1.0);

type Matrix = [[f32; 3]; 3];

const fn fixed(v: i32) -> f32 {
    v as f32 / 65536.0
}

/// `SkNamedGamut::kSRGB`.
const SRGB_TO_XYZ_D50: Matrix = [
    [fixed(0x6FA2), fixed(0x6299), fixed(0x24A0)],
    [fixed(0x38F5), fixed(0xB785), fixed(0x0F84)],
    [fixed(0x0390), fixed(0x18DA), fixed(0xB6CF)],
];

fn log2f(x: f32) -> f32 {
    let bits = x.to_bits() as i32;
    let e = bits as f32 * (1.0 / (1 << 23) as f32);
    let m = f32::from_bits(((bits & 0x007f_ffff) | 0x3f00_0000) as u32);
    e - 124.225514990 - 1.498030302 * m - 1.725879990 / (0.3520887068 + m)
}

fn exp2f(x: f32) -> f32 {
    if x > 128.0 {
        return f32::INFINITY;
    }
    if x < -127.0 {
        return 0.0;
    }
    let fract = x - x.floor();
    let fbits = (1u32 << 23) as f32
        * (x + 121.274057500 - 1.490129070 * fract + 27.728023300 / (4.84252568 - fract));
    if fbits >= i32::MAX as f32 {
        return f32::INFINITY;
    }
    if fbits < 0.0 {
        return 0.0;
    }
    f32::from_bits(fbits as i32 as u32)
}

/// skcms's scalar `powf_`.
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        0.0
    } else if x == 1.0 {
        1.0
    } else {
        exp2f(log2f(x) * y)
    }
}

/// `skcms_TransferFunction_eval` for an sRGB-ish function.
fn eval(tf: &TransferFunction, x: f32) -> f32 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x * sign;
    sign * if x < tf.d {
        tf.c * x + tf.f
    } else {
        powf(tf.a * x + tf.b, tf.g) + tf.e
    }
}

/// `classify(tf) == skcms_TFType_sRGBish`.
fn is_srgbish(tf: &TransferFunction) -> bool {
    (tf.a + tf.b + tf.c + tf.d + tf.e + tf.f + tf.g).is_finite()
        && tf.g >= 0.0
        && tf.a >= 0.0
        && tf.c >= 0.0
        && tf.d >= 0.0
        && tf.a * tf.d + tf.b >= 0.0
}

/// The highp stages of `SkRasterPipeline_opts.h` with `mad` fused, as on arm64 and on x86-64
/// with FMA.
mod highp {
    fn approx_log2(x: f32) -> f32 {
        let e = x.to_bits() as f32 * (1.0 / (1 << 23) as f32);
        let m = f32::from_bits((x.to_bits() & 0x007f_ffff) | 0x3f00_0000);
        (-m).mul_add(1.498030302, e - 124.225514990) - 1.725879990 / (0.3520887068 + m)
    }

    fn approx_pow2(x: f32) -> f32 {
        let f = x - x.floor();
        let mut approx = (-f).mul_add(1.490129070, x + 121.274057500);
        approx += 27.728023300 / (4.84252568 - f);
        approx *= (1u32 << 23) as f32;
        approx = approx.max(0.0).min(0x7f80_0000_u32 as f32);
        f32::from_bits(approx.round_ties_even() as u32)
    }

    fn approx_powf(x: f32, y: f32) -> f32 {
        if x == 0.0 || x == 1.0 {
            x
        } else {
            approx_pow2(approx_log2(x) * y)
        }
    }

    fn signed(v: f32, f: impl Fn(f32) -> f32) -> f32 {
        let sign = v.to_bits() & 0x8000_0000;
        f32::from_bits(sign | f(f32::from_bits(v.to_bits() ^ sign)).to_bits())
    }

    /// `SkRasterPipeline::appendTransferFunction`'s `gamma_` or `parametric` stage.
    pub(super) fn transfer(tf: &super::TransferFunction, v: f32) -> f32 {
        if (tf.a, tf.b, tf.c, tf.d, tf.e, tf.f) == (1.0, 0.0, 0.0, 0.0, 0.0, 0.0) {
            signed(v, |v| approx_powf(v, tf.g))
        } else {
            signed(v, |v| {
                if v <= tf.d {
                    tf.c.mul_add(v, tf.f)
                } else {
                    approx_powf(tf.a.mul_add(v, tf.b), tf.g) + tf.e
                }
            })
        }
    }

    /// `matrix_3x3` on a row-major matrix.
    pub(super) fn matrix(m: &super::Matrix, [r, g, b]: [f32; 3]) -> [f32; 3] {
        m.map(|row| r.mul_add(row[0], g.mul_add(row[1], b * row[2])))
    }
}

/// `skcms_Matrix3x3_invert`.
fn invert(src: &Matrix) -> Option<Matrix> {
    let v = |r: usize, c: usize| f64::from(src[r][c]);
    let (a00, a01, a02) = (v(0, 0), v(1, 0), v(2, 0));
    let (a10, a11, a12) = (v(0, 1), v(1, 1), v(2, 1));
    let (a20, a21, a22) = (v(0, 2), v(1, 2), v(2, 2));
    let b0 = a00 * a11 - a01 * a10;
    let b1 = a00 * a12 - a02 * a10;
    let b2 = a01 * a12 - a02 * a11;
    let determinant = b0 * a22 - b1 * a21 + b2 * a20;
    if determinant == 0.0 {
        return None;
    }
    let invdet = 1.0 / determinant;
    if !(invdet as f32).is_finite() {
        return None;
    }
    let [b0, b1, b2, b3, b4, b5] = [b0, b1, b2, a20, a21, a22].map(|b| b * invdet);
    let m = [
        [
            a11 * b5 - a12 * b4,
            a12 * b3 - a10 * b5,
            a10 * b4 - a11 * b3,
        ],
        [
            a02 * b4 - a01 * b5,
            a00 * b5 - a02 * b3,
            a01 * b3 - a00 * b4,
        ],
        [b2, -b1, b0],
    ]
    .map(|row| row.map(|x| x as f32));
    m.iter().flatten().all(|x| x.is_finite()).then_some(m)
}

/// `skcms_Matrix3x3_concat(a, b)`, built without FP contraction.
fn concat(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|r| {
        std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c])
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Curve {
    Parametric(TransferFunction),
    /// A `curv` table of two or more big-endian `u16` entries.
    Table(Vec<u16>),
}

impl Curve {
    /// skcms's `eval_curve`.
    fn eval(&self, x: f32) -> f32 {
        match self {
            Curve::Parametric(tf) => eval(tf, x),
            Curve::Table(table) => {
                let ix = x.clamp(0.0, 1.0) * (table.len() - 1) as f32;
                let lo = ix as i32 as usize;
                let hi = f32::from_bits((ix + 1.0).to_bits() - 1) as i32 as usize;
                let t = ix - lo as f32;
                let l = f32::from(table[lo]) * (1.0 / 65535.0);
                let h = f32::from(table[hi]) * (1.0 / 65535.0);
                l + (h - l) * t
            }
        }
    }

    /// `skcms_AreApproximateInverses`.
    fn is_approximate_inverse_of(&self, inv: &TransferFunction) -> bool {
        let n = match self {
            Curve::Table(table) => table.len().max(256),
            Curve::Parametric(_) => 256,
        };
        let dx = 1.0 / (n - 1) as f32;
        let err = (0..n).fold(0.0f32, |err, i| {
            let x = i as f32 * dx;
            err.max((x - eval(inv, self.eval(x))).abs())
        });
        err < 1.0 / 512.0
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn s15_fixed16(b: &[u8], at: usize) -> Option<f32> {
    Some(u32_at(b, at)? as i32 as f32 * (1.0 / 65536.0))
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

/// skcms's `read_curve_curv` and `read_curve_para`.
fn parse_curve(data: &[u8]) -> Option<Curve> {
    match data.get(0..4)? {
        b"curv" => match u32_at(data, 8)? as usize {
            0 => Some(Curve::Parametric(LINEAR)),
            1 => Some(Curve::Parametric(TransferFunction::gamma(
                f32::from(u16_at(data, 12)?) * (1.0 / 256.0),
            ))),
            n if n <= 1 << 24 => (0..n)
                .map(|i| u16_at(data, 12 + 2 * i))
                .collect::<Option<Vec<u16>>>()
                .map(Curve::Table),
            _ => None,
        },
        b"para" => {
            let param = |i: usize| s15_fixed16(data, 12 + 4 * i);
            let mut tf = TransferFunction::gamma(param(0)?);
            match u16_at(data, 8)? {
                0 => {}
                1 => {
                    tf.a = param(1)?;
                    tf.b = param(2)?;
                    if tf.a == 0.0 {
                        return None;
                    }
                    tf.d = -tf.b / tf.a;
                }
                2 => {
                    tf.a = param(1)?;
                    tf.b = param(2)?;
                    tf.e = param(3)?;
                    if tf.a == 0.0 {
                        return None;
                    }
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
            is_srgbish(&tf).then_some(Curve::Parametric(tf))
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

/// `SkColorSpace::Make(profile)` followed by `SkColorSpace::MakeRGB`: `None` for sRGB itself, and
/// for a profile Skia cannot represent.
fn sk_color_space(curves: &[Curve; 3], to_xyz: &Matrix) -> Option<(TransferFunction, Matrix)> {
    invert(to_xyz)?;
    let tf = match curves {
        [
            Curve::Parametric(r),
            Curve::Parametric(g),
            Curve::Parametric(b),
        ] if r.bits() == g.bits() && r.bits() == b.bits() => *r,
        _ if curves
            .iter()
            .all(|c| c.is_approximate_inverse_of(&SRGB_INVERSE)) =>
        {
            SRGB
        }
        _ => return None,
    };
    if !is_srgbish(&tf) {
        return None;
    }
    let close = |a: f32, b: f32| (a - b).abs() < 0.001;
    let srgb_gamut = to_xyz
        .iter()
        .flatten()
        .zip(SRGB_TO_XYZ_D50.iter().flatten())
        .all(|(a, b)| (a - b).abs() < 0.01);
    let gamma_like =
        |g: f32| close(1.0, tf.a) && close(0.0, tf.b) && close(0.0, tf.e) && close(g, tf.g);
    let tf = if [
        (SRGB.a, tf.a),
        (SRGB.b, tf.b),
        (SRGB.c, tf.c),
        (SRGB.d, tf.d),
        (SRGB.e, tf.e),
        (SRGB.f, tf.f),
        (SRGB.g, tf.g),
    ]
    .iter()
    .all(|&(a, b)| close(a, b))
    {
        if srgb_gamut {
            return None;
        }
        SRGB
    } else if gamma_like(2.2) && tf.d <= 0.0 {
        TWO_DOT_TWO
    } else if (gamma_like(1.0) && tf.d <= 0.0)
        || (close(1.0, tf.c) && close(0.0, tf.f) && tf.d >= 1.0)
    {
        return Some((LINEAR, if srgb_gamut { SRGB_TO_XYZ_D50 } else { *to_xyz }));
    } else {
        tf
    };
    Some((tf, *to_xyz))
}

/// The conversion of one source colour space's pixels into the canvas's sRGB.
#[derive(Clone, Debug, PartialEq)]
pub struct IccTransform {
    linearize: Option<TransferFunction>,
    gamut: Option<Matrix>,
}

impl IccTransform {
    /// The transform for an RGB display profile with `rXYZ`/`gXYZ`/`bXYZ` and `rTRC`/`gTRC`/
    /// `bTRC` tags. `None` for sRGB, which Chromium draws untouched, and for a profile that
    /// `SkColorSpace` cannot represent (#245).
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
        let to_xyz: Matrix =
            std::array::from_fn(|row| std::array::from_fn(|col| columns[col][row]));
        let (tf, to_xyz) = sk_color_space(&curves, &to_xyz)?;
        let gamut = (to_xyz.iter().flatten().map(|v| v.to_bits()))
            .ne(SRGB_TO_XYZ_D50.iter().flatten().map(|v| v.to_bits()))
            .then(|| concat(&invert(&SRGB_TO_XYZ_D50).expect("sRGB inverts"), &to_xyz));
        Some(IccTransform {
            linearize: (tf.bits() != LINEAR.bits()).then_some(tf),
            gamut,
        })
    }

    /// One straight-alpha pixel, premultiplied as the canvas stores it.
    pub fn convert(&self, [r, g, b, a]: [u8; 4]) -> [u8; 4] {
        let mut rgb = [r, g, b].map(|v| f32::from(v) * (1.0 / 255.0));
        if let Some(tf) = &self.linearize {
            rgb = rgb.map(|v| highp::transfer(tf, v));
        }
        if let Some(m) = &self.gamut {
            rgb = highp::matrix(m, rgb);
        }
        rgb = rgb.map(|v| highp::transfer(&SRGB_INVERSE, v));
        if a != 255 {
            let alpha = f32::from(a) * (1.0 / 255.0);
            rgb = rgb.map(|v| v * alpha);
        }
        let [r, g, b] = rgb.map(to_unorm);
        [r, g, b, a]
    }

    /// A premultiplied pixmap of straight-alpha RGBA in this colour space.
    pub fn pixmap(&self, width: u32, height: u32, mut rgba: Vec<u8>) -> Option<Pixmap> {
        if rgba.len() != width as usize * height as usize * 4 {
            return None;
        }
        let mut cache: HashMap<[u8; 4], [u8; 4]> = HashMap::new();
        for p in rgba.as_chunks_mut::<4>().0 {
            *p = *cache.entry(*p).or_insert_with(|| self.convert(*p));
        }
        Pixmap::from_vec(rgba, IntSize::from_wh(width, height)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADOBE_RGB: [[u32; 3]; 3] = [
        [0x9c18, 0x4fa5, 0x04fc],
        [0x348d, 0xa02c, 0x0f95],
        [0x2631, 0x102f, 0xbe9c],
    ];

    const SRGB_COLUMNS: [[u32; 3]; 3] = [
        [0x6FA2, 0x38F5, 0x0390],
        [0x6299, 0xB785, 0x18DA],
        [0x24A0, 0x0F84, 0xB6CF],
    ];

    fn profile(columns: [[u32; 3]; 3], curve: Vec<u8>) -> Vec<u8> {
        let mut icc = vec![0u8; 128];
        icc[16..20].copy_from_slice(b"RGB ");
        icc[20..24].copy_from_slice(b"XYZ ");
        let tags: [(&[u8; 4], Vec<u8>); 6] = [
            (b"rXYZ", xyz(columns[0])),
            (b"gXYZ", xyz(columns[1])),
            (b"bXYZ", xyz(columns[2])),
            (b"rTRC", curve.clone()),
            (b"gTRC", curve.clone()),
            (b"bTRC", curve),
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

    /// Electron 39's CPU canvas `getImageData` after `drawImage` of a 4096x4096 PNG holding
    /// every 8-bit colour, tagged with the Evergreen fields' Adobe RGB (1998) profile. The port
    /// matches all 16777216 colours; these are a sample, with the colours where gamma 2.2 (Skia
    /// snaps the profile's 2.19921875) and the fused highp stages decide the level.
    #[test]
    fn adobe_rgb_matches_chromium_cpu_canvas() {
        let t =
            IccTransform::from_icc(&profile(ADOBE_RGB, curv(0x0233))).expect("Adobe RGB converts");
        for (src, chromium) in [
            ([0, 0, 0], [0, 0, 0]),
            ([255, 255, 255], [255, 255, 255]),
            ([92, 92, 92], [92, 92, 92]),
            ([168, 168, 168], [169, 170, 169]),
            ([176, 176, 176], [177, 178, 177]),
            ([0, 0, 30], [0, 0, 24]),
            ([0, 0, 36], [0, 0, 31]),
            ([0, 0, 68], [0, 0, 67]),
            ([0, 0, 79], [0, 0, 79]),
            ([0, 0, 107], [0, 0, 109]),
            ([6, 89, 235], [0, 88, 239]),
            ([9, 89, 235], [0, 89, 239]),
            ([255, 0, 0], [255, 0, 0]),
            ([0, 255, 0], [0, 255, 0]),
            ([0, 0, 255], [0, 0, 255]),
        ] {
            let [r, g, b] = src;
            assert_eq!(t.convert([r, g, b, 255])[..3], chromium, "{src:?}");
        }
    }

    #[test]
    fn a_pixmap_is_made_in_the_buffer_it_is_given() {
        let t =
            IccTransform::from_icc(&profile(ADOBE_RGB, curv(0x0233))).expect("Adobe RGB converts");
        let rgba: Vec<u8> = (0..=255).cycle().step_by(37).take(4 * 35).collect();
        let want: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&p| t.convert(p))
            .collect();
        let at = rgba.as_ptr();
        let pixmap = t.pixmap(7, 5, rgba).unwrap();
        assert_eq!(pixmap.data(), want);
        assert_eq!(pixmap.data().as_ptr(), at);
    }

    #[test]
    fn srgb_is_untouched() {
        let mut para = b"para\0\0\0\0\0\x03\0\0".to_vec();
        for v in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045] {
            para.extend_from_slice(&((v * 65536.0f64).round() as i32).to_be_bytes());
        }
        assert_eq!(IccTransform::from_icc(&profile(SRGB_COLUMNS, para)), None);
    }
}
