// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

use asrs_js::math;

/// A parsed CSS colour: straight-alpha channels in 0..=1, as Blink hands them to Skia.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::from_u8(0, 0, 0, 1.0);

    pub const fn from_u8(r: u8, g: u8, b: u8, a: f32) -> Rgba {
        Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }

    pub fn to_u8(self) -> [u8; 3] {
        [self.r, self.g, self.b].map(|c| (c * 255.0 + 0.5) as u8)
    }

    pub fn with_alpha_scaled(self, alpha: f32) -> Rgba {
        Rgba {
            a: self.a * alpha,
            ..self
        }
    }
}

/// `Color::ColorSpace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    SrgbLegacy,
    Srgb,
    SrgbLinear,
    DisplayP3,
    A98Rgb,
    ProPhoto,
    Rec2020,
    XyzD50,
    XyzD65,
    Lab,
    Oklab,
    Lch,
    Oklch,
    Hsl,
    Hwb,
}

impl Space {
    /// `IsLegacyColorSpace`.
    pub fn is_legacy(self) -> bool {
        matches!(self, Space::SrgbLegacy | Space::Hsl | Space::Hwb)
    }

    /// `IsLightnessFirstComponent`.
    pub fn lightness_first(self) -> bool {
        matches!(self, Space::Lab | Space::Oklab | Space::Lch | Space::Oklch)
    }

    /// `IsChromaSecondComponent`.
    fn chroma_second(self) -> bool {
        matches!(self, Space::Lch | Space::Oklch)
    }

    /// `Color::ColorSpaceToString`.
    pub fn name(self) -> &'static str {
        match self {
            Space::SrgbLegacy => "rgb",
            Space::Srgb => "srgb",
            Space::SrgbLinear => "srgb-linear",
            Space::DisplayP3 => "display-p3",
            Space::A98Rgb => "a98-rgb",
            Space::ProPhoto => "prophoto-rgb",
            Space::Rec2020 => "rec2020",
            Space::XyzD50 => "xyz-d50",
            Space::XyzD65 => "xyz-d65",
            Space::Lab => "lab",
            Space::Oklab => "oklab",
            Space::Lch => "lch",
            Space::Oklch => "oklch",
            Space::Hsl => "hsl",
            Space::Hwb => "hwb",
        }
    }

    /// `IsPredefinedColorSpace`: the spaces `color()` takes.
    pub fn is_predefined(self) -> bool {
        matches!(
            self,
            Space::Srgb
                | Space::SrgbLinear
                | Space::DisplayP3
                | Space::A98Rgb
                | Space::ProPhoto
                | Space::Rec2020
                | Space::XyzD50
                | Space::XyzD65
        )
    }

    /// `ColorFunction::MetadataForColorSpace::channel_name`.
    fn channel_names(self) -> [&'static str; 3] {
        match self {
            Space::Lab | Space::Oklab => ["l", "a", "b"],
            Space::Lch | Space::Oklch => ["l", "c", "h"],
            Space::Hsl => ["h", "s", "l"],
            Space::Hwb => ["h", "w", "b"],
            Space::XyzD50 | Space::XyzD65 => ["x", "y", "z"],
            _ => ["r", "g", "b"],
        }
    }

    /// `channel_percentage`: the value 100% means, `f64::NAN` where percentages
    /// do not apply.
    fn channel_percentage(self) -> [f64; 3] {
        match self {
            Space::SrgbLegacy => [255.0, 255.0, 255.0],
            Space::Lab => [100.0, 125.0, 125.0],
            Space::Oklab => [1.0, 0.4, 0.4],
            Space::Lch => [100.0, 150.0, f64::NAN],
            Space::Oklch => [1.0, 0.4, f64::NAN],
            Space::Hsl | Space::Hwb => [f64::NAN, 100.0, 100.0],
            _ => [1.0, 1.0, 1.0],
        }
    }

    /// `ColorChannelIsHue`.
    fn channel_is_hue(self, i: usize) -> bool {
        matches!(
            (self, i),
            (Space::Hsl | Space::Hwb, 0) | (Space::Lch | Space::Oklch, 2)
        )
    }
}

/// A colour as Blink's `Color` holds it: a space, three channels and alpha, each
/// channel possibly `none` (missing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parsed {
    pub space: Space,
    pub p: [f32; 3],
    pub none: [bool; 3],
    pub a: f32,
    pub a_none: bool,
}

impl Parsed {
    /// `Color::FromColorSpace`.
    fn from_space(space: Space, params: [Option<f64>; 3], alpha: Option<f64>) -> Parsed {
        let none = params.map(|v| v.is_none());
        let mut p = params.map(|v| v.unwrap_or(0.0) as f32);
        let a_none = alpha.is_none();
        let mut a = alpha.unwrap_or(0.0) as f32;
        if !a_none {
            a = a.clamp(0.0, 1.0);
        }
        if space.lightness_first() && !p[0].is_nan() {
            p[0] = if matches!(space, Space::Lab | Space::Lch) {
                p[0].clamp(0.0, 100.0)
            } else {
                p[0].clamp(0.0, 1.0)
            };
        }
        if space.chroma_second() {
            p[1] = p[1].max(0.0);
        }
        Parsed {
            space,
            p,
            none,
            a,
            a_none,
        }
        .finish_none(none)
    }

    fn finish_none(mut self, flags: [bool; 3]) -> Parsed {
        self.none = flags;
        self
    }

    pub fn legacy(c: [f32; 3], a: f32) -> Parsed {
        Parsed {
            space: Space::SrgbLegacy,
            p: c,
            none: [false; 3],
            a,
            a_none: false,
        }
    }

    pub fn alpha(self) -> f32 {
        self.a
    }

    /// `toSkColor4f()`, what a canvas paints with.
    pub fn to_rgba(self) -> Rgba {
        let [r, g, b] = conversions::to_srgb(self);
        Rgba { r, g, b, a: self.a }
    }

    /// The channels `SerializeLegacyColorAsCSSColor` writes.
    pub fn css_channels(self) -> [u8; 3] {
        let [mut r, mut g, mut b] = self.p;
        match self.space {
            Space::Hsl => {
                let (x, y, z) = conversions::hsl_to_srgb(r, g, b);
                [r, g, b] = [x + 1e-7, y + 1e-7, z + 1e-7].map(|v| (f64::from(v) * 255.0) as f32);
            }
            Space::Hwb => {
                let (x, y, z) = conversions::hwb_to_srgb(r, g, b);
                [r, g, b] = [x + 1e-7, y + 1e-7, z + 1e-7].map(|v| (f64::from(v) * 255.0) as f32);
            }
            _ => {}
        }
        [r, g, b].map(|v| f64::from(v).clamp(0.0, 255.0).round() as u8)
    }
}

/// `gfx` colour conversions and the `skcms` pieces they use: matrices are 3x3
/// row-major over `f32`, transfer functions the `{g,a,b,c,d,e,f}` 7-parameter
/// curves.
pub(crate) mod conversions {
    #![allow(clippy::excessive_precision)]
    use super::Space;

    type M3 = [[f32; 3]; 3];

    struct TrFn {
        g: f32,
        a: f32,
        b: f32,
        c: f32,
        d: f32,
        e: f32,
        f: f32,
    }

    const SRGB: TrFn = TrFn {
        g: 2.4,
        a: 1.0 / 1.055,
        b: 0.055 / 1.055,
        c: 1.0 / 12.92,
        d: 0.04045,
        e: 0.0,
        f: 0.0,
    };
    const G2_2: TrFn = TrFn {
        g: 2.2,
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 0.0,
        e: 0.0,
        f: 0.0,
    };
    const PROPHOTO: TrFn = TrFn {
        g: 1.8,
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 0.0,
        e: 0.0,
        f: 0.0,
    };
    const REC2020: TrFn = TrFn {
        g: 2.22222,
        a: 0.909672,
        b: 0.0903276,
        c: 0.222222,
        d: 0.0812429,
        e: 0.0,
        f: 0.0,
    };

    /// `skcms_TransferFunction_eval` for the `sRGBish` type every curve here is.
    fn eval(tf: &TrFn, x: f32) -> f32 {
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x * sign;
        sign * if x < tf.d {
            tf.c * x + tf.f
        } else {
            (tf.a * x + tf.b).powf(tf.g) + tf.e
        }
    }

    /// `skcmsTrFnEvalExt`.
    fn eval_ext(tf: &TrFn, x: f32) -> f32 {
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x * sign;
        if x >= tf.d && tf.a * x + tf.b < 0.0 {
            return sign * tf.e;
        }
        sign * if x < tf.d {
            tf.c * x + tf.f
        } else {
            (tf.a * x + tf.b).powf(tf.g) + tf.e
        }
    }

    /// `skcms_TransferFunction_invert` for `sRGBish` curves.
    fn invert(src: &TrFn) -> TrFn {
        let d_l = src.c * src.d + src.f;
        let d_r = (src.a * src.d + src.b).powf(src.g) + src.e;
        let mut inv = TrFn {
            g: 0.0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        if (d_l - d_r).abs() > 1.0 / 512.0 {
            return inv;
        }
        inv.d = d_l;
        if inv.d > 0.0 {
            inv.c = 1.0 / src.c;
            inv.f = -src.f / src.c;
        }
        let k = src.a.powf(-src.g);
        inv.g = 1.0 / src.g;
        inv.a = k;
        inv.b = -k * src.e;
        inv.e = -src.b / src.a;
        if inv.a * inv.d + inv.b < 0.0 {
            inv.b = -inv.a * inv.d;
        }
        let s = eval(src, 1.0);
        let sign = if s < 0.0 { -1.0 } else { 1.0 };
        let s = s * sign;
        if s < inv.d {
            inv.f = 1.0 - sign * inv.c * s;
        } else {
            inv.e = 1.0 - sign * (inv.a * s + inv.b).powf(inv.g);
        }
        inv
    }

    fn apply(m: &M3, v: [f32; 3]) -> [f32; 3] {
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    /// `skcms_Matrix3x3_invert` (`f64` internal math like skcms).
    fn invert_m(m: &M3) -> Option<M3> {
        let a00 = f64::from(m[0][0]);
        let a01 = f64::from(m[1][0]);
        let a02 = f64::from(m[2][0]);
        let a10 = f64::from(m[0][1]);
        let a11 = f64::from(m[1][1]);
        let a12 = f64::from(m[2][1]);
        let a20 = f64::from(m[0][2]);
        let a21 = f64::from(m[1][2]);
        let a22 = f64::from(m[2][2]);
        let mut b0 = a00 * a11 - a01 * a10;
        let mut b1 = a00 * a12 - a02 * a10;
        let mut b2 = a01 * a12 - a02 * a11;
        let mut b3 = a20;
        let mut b4 = a21;
        let mut b5 = a22;
        let det = b0 * b5 - b1 * b4 + b2 * b3;
        if det == 0.0 {
            return None;
        }
        let invdet = 1.0 / det;
        b0 *= invdet;
        b1 *= invdet;
        b2 *= invdet;
        b3 *= invdet;
        b4 *= invdet;
        b5 *= invdet;
        Some([
            [
                (a11 * b5 - a12 * b4) as f32,
                (a12 * b3 - a10 * b5) as f32,
                (a10 * b4 - a11 * b3) as f32,
            ],
            [
                (a02 * b4 - a01 * b5) as f32,
                (a00 * b5 - a02 * b3) as f32,
                (a01 * b3 - a00 * b4) as f32,
            ],
            [b2 as f32, (-b1) as f32, b0 as f32],
        ])
    }

    /// `skcms_Matrix3x3_concat` (`A * B`).
    fn concat(a: &M3, b: &M3) -> M3 {
        let mut m = [[0.0; 3]; 3];
        for r in 0..3 {
            for c in 0..3 {
                m[r][c] = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
            }
        }
        m
    }

    /// `skcms_AdaptToXYZD50`: Bradford adaptation from white point (wx, wy) to D50.
    fn adapt_to_d50(wx: f32, wy: f32) -> M3 {
        let w = [wx / wy, 1.0, (1.0 - wx - wy) / wy];
        let d50 = [0.96422, 1.0, 0.82521];
        let xyz_to_lms: M3 = [
            [0.8951, 0.2664, -0.1614],
            [-0.7502, 1.7135, 0.0367],
            [0.0389, -0.0685, 1.0296],
        ];
        let lms_to_xyz: M3 = [
            [0.9869929, -0.1470543, 0.1599627],
            [0.4323053, 0.5183603, 0.0492912],
            [-0.0085287, 0.0400428, 0.9684867],
        ];
        let src = apply(&xyz_to_lms, w);
        let dst = apply(&xyz_to_lms, d50);
        let diag: M3 = [
            [dst[0] / src[0], 0.0, 0.0],
            [0.0, dst[1] / src[1], 0.0],
            [0.0, 0.0, dst[2] / src[2]],
        ];
        concat(&lms_to_xyz, &concat(&diag, &xyz_to_lms))
    }

    /// `skcms_PrimariesToXYZD50`.
    fn primaries_to_d50(pr: [f32; 8]) -> M3 {
        let [rx, ry, gx, gy, bx, by, wx, wy] = pr;
        let primaries: M3 = [
            [rx, gx, bx],
            [ry, gy, by],
            [1.0 - rx - ry, 1.0 - gx - gy, 1.0 - bx - by],
        ];
        let inv = invert_m(&primaries).unwrap();
        let w = [wx / wy, 1.0, (1.0 - wx - wy) / wy];
        let xyz = apply(&inv, w);
        let diag: M3 = [[xyz[0], 0.0, 0.0], [0.0, xyz[1], 0.0], [0.0, 0.0, xyz[2]]];
        let to_xyz = concat(&primaries, &diag);
        concat(&adapt_to_d50(wx, wy), &to_xyz)
    }

    fn fixed(v: i32) -> f32 {
        v as f32 / 65536.0
    }

    /// `SkNamedGamut::kSRGB` (linear sRGB -> XYZ D50).
    fn srgb_gamut() -> M3 {
        [
            [fixed(0x6FA2), fixed(0x6299), fixed(0x24A0)],
            [fixed(0x38F5), fixed(0xB785), fixed(0x0F84)],
            [fixed(0x0390), fixed(0x18DA), fixed(0xB6CF)],
        ]
    }

    fn adobe_gamut() -> M3 {
        [
            [fixed(0x9c18), fixed(0x348d), fixed(0x2631)],
            [fixed(0x4fa5), fixed(0xa02c), fixed(0x102f)],
            [fixed(0x04fc), fixed(0x0f95), fixed(0xbe9c)],
        ]
    }

    const DISPLAY_P3: M3 = [
        [0.515102, 0.291965, 0.157153],
        [0.241182, 0.692236, 0.0665819],
        [-0.00104941, 0.0418818, 0.784378],
    ];

    const REC2020_GAMUT: M3 = [
        [0.673459, 0.165661, 0.125100],
        [0.279033, 0.675338, 0.0456288],
        [-0.00193139, 0.0299794, 0.797162],
    ];

    const PROPHOTO_PRIMARIES: [f32; 8] = [
        0.7347, 0.2653, 0.1596, 0.8404, 0.0366, 0.0001, 0.34567, 0.35850,
    ];

    const XYZ_TO_LMS: M3 = [
        [0.8190224432164319, 0.3619062562801221, -0.12887378261216414],
        [0.0329836671980271, 0.9292868468965546, 0.03614466816999844],
        [
            0.048177199566046255,
            0.26423952494422764,
            0.6335478258136937,
        ],
    ];

    const OKLAB_TO_LMS: M3 = [
        [
            0.99999999845051981432,
            0.39633779217376785678,
            0.21580375806075880339,
        ],
        [
            1.0000000088817607767,
            -0.1055613423236563494,
            -0.063854174771705903402,
        ],
        [
            1.0000000546724109177,
            -0.089484182094965759684,
            -1.2914855378640917399,
        ],
    ];

    const D50_X: f32 = 0.9642;
    const D50_Y: f32 = 1.0;
    const D50_Z: f32 = 0.8251;

    fn srgb_transfer(c: [f32; 3]) -> [f32; 3] {
        c.map(|v| eval_ext(&SRGB, v))
    }
    fn srgb_inverse(c: [f32; 3]) -> [f32; 3] {
        c.map(|v| eval_ext(&invert(&SRGB), v))
    }

    /// `gfx::LabToXYZD50`.
    fn lab_to_xyz(l: f32, a: f32, b: f32) -> [f32; 3] {
        let y = (l + 16.0) / 116.0;
        let x = y + a / 500.0;
        let z = y - b / 200.0;
        let f = |t: f32| {
            let delta = 24.0 / 116.0;
            if t <= delta {
                (108.0 / 841.0) * (t - 16.0 / 116.0)
            } else {
                t * t * t
            }
        };
        [f(x) * D50_X, f(y) * D50_Y, f(z) * D50_Z]
    }

    /// `gfx::XYZD50ToLab`.
    fn xyz_to_lab(x: f32, y: f32, z: f32) -> [f32; 3] {
        let f = |t: f32| {
            let limit = (24.0f32 / 116.0).powi(3);
            if t <= limit {
                (841.0 / 108.0) * t + 16.0 / 116.0
            } else {
                t.powf(1.0 / 3.0)
            }
        };
        let x = f(x / D50_X);
        let y = f(y / D50_Y);
        let z = f(z / D50_Z);
        [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
    }

    /// `gfx::OklabToXYZD65`.
    fn oklab_to_xyz(l: f32, a: f32, b: f32) -> [f32; 3] {
        let lms = apply(&OKLAB_TO_LMS, [l, a, b]).map(|v| v * v * v);
        let lms_to_xyz = invert_m(&XYZ_TO_LMS).unwrap();
        apply(&lms_to_xyz, lms)
    }

    /// `gfx::XYZD65ToOklab`.
    fn xyz_to_oklab(xyz: [f32; 3]) -> [f32; 3] {
        let pow_ext = |x: f32| {
            if x < 0.0 {
                -(-x).powf(1.0 / 3.0)
            } else {
                x.powf(1.0 / 3.0)
            }
        };
        let lms = apply(&XYZ_TO_LMS, xyz).map(pow_ext);
        let lms_to_oklab = invert_m(&OKLAB_TO_LMS).unwrap();
        apply(&lms_to_oklab, lms)
    }

    /// `gfx::LchToLab`.
    fn lch_to_lab(l: f32, c: f32, h: f32) -> [f32; 3] {
        let rad = f64::from(h).to_radians() as f32;
        [l, c * rad.cos(), c * rad.sin()]
    }

    /// `gfx::LabToLch`.
    fn lab_to_lch(l: f32, a: f32, b: f32) -> [f32; 3] {
        [
            l,
            (a * a + b * b).sqrt(),
            f64::from(b).atan2(f64::from(a)).to_degrees() as f32,
        ]
    }

    /// `gfx::HSLToSRGB`.
    pub fn hsl_to_srgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
        if s == 0.0 {
            return (l, l, l);
        }
        let f = |n: f32| {
            let k = (f64::from(n + h / 30.0) % 12.0) as f32;
            let a = s * l.min(1.0 - l);
            l - a * (-1.0f32).max((k - 3.0).min(9.0 - k).min(1.0))
        };
        (f(0.0), f(8.0), f(4.0))
    }

    /// `gfx::SRGBToHSL`.
    fn srgb_to_hsl(r: f32, g: f32, b: f32) -> [f32; 3] {
        let min = r.min(g).min(b);
        let max = r.max(g).max(b);
        let mut hue = 0.0f32;
        let mut saturation = 0.0f32;
        let lightness = (min + max) / 2.0;
        let d = max - min;
        if d != 0.0 {
            saturation = if lightness == 0.0 || lightness == 1.0 {
                0.0
            } else {
                (max - lightness) / lightness.min(1.0 - lightness)
            };
            hue = if max == r {
                (g - b) / d + if g < b { 6.0 } else { 0.0 }
            } else if max == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
            hue *= 60.0;
        }
        if saturation < 0.0 {
            hue += 180.0;
            saturation = saturation.abs();
        }
        if hue >= 360.0 {
            hue -= 360.0;
        }
        [hue, saturation, lightness]
    }

    /// `gfx::HWBToSRGB`.
    pub fn hwb_to_srgb(h: f32, w: f32, b: f32) -> (f32, f32, f32) {
        if w + b >= 1.0 {
            let gray = w / (w + b);
            return (gray, gray, gray);
        }
        let (mut r, mut g, mut bl) = hsl_to_srgb(h, 1.0, 0.5);
        r += w - (w + b) * r;
        g += w - (w + b) * g;
        bl += w - (w + b) * bl;
        (r, g, bl)
    }

    /// `gfx::SRGBToHWB`.
    fn srgb_to_hwb(r: f32, g: f32, b: f32) -> [f32; 3] {
        let min = r.min(g).min(b);
        let max = r.max(g).max(b);
        let mut hue = 0.0f32;
        let d = max - min;
        if d != 0.0 {
            hue = if max == r {
                (g - b) / d + if g < b { 6.0 } else { 0.0 }
            } else if max == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
            hue *= 60.0;
        }
        if hue >= 360.0 {
            hue -= 360.0;
        }
        [hue, min, 1.0 - max]
    }

    fn xyz50_to_srgb_linear(v: [f32; 3]) -> [f32; 3] {
        apply(&invert_m(&srgb_gamut()).unwrap(), v)
    }
    fn xyz65_to_srgb_linear(v: [f32; 3]) -> [f32; 3] {
        let m = concat(
            &invert_m(&srgb_gamut()).unwrap(),
            &adapt_to_d50(0.3127, 0.3290),
        );
        apply(&m, v)
    }
    fn xyz50_to_srgb(v: [f32; 3]) -> [f32; 3] {
        srgb_inverse(xyz50_to_srgb_linear(v))
    }
    fn xyz65_to_d50(v: [f32; 3]) -> [f32; 3] {
        apply(&adapt_to_d50(0.3127, 0.3290), v)
    }
    fn xyz50_to_d65(v: [f32; 3]) -> [f32; 3] {
        apply(&invert_m(&adapt_to_d50(0.3127, 0.3290)).unwrap(), v)
    }

    /// `Color::ExportAsXYZD50Floats` (`none` channels resolved to 0 elsewhere).
    fn to_xyz50(space: Space, p: [f32; 3]) -> [f32; 3] {
        match space {
            Space::SrgbLegacy => {
                let v = srgb_transfer(p.map(|x| x / 255.0));
                apply(&srgb_gamut(), v)
            }
            Space::Srgb => apply(&srgb_gamut(), srgb_transfer(p)),
            Space::SrgbLinear => apply(&srgb_gamut(), p),
            Space::DisplayP3 => apply(&DISPLAY_P3, srgb_transfer(p)),
            Space::A98Rgb => apply(&adobe_gamut(), p.map(|v| eval_ext(&G2_2, v))),
            Space::ProPhoto => apply(
                &primaries_to_d50(PROPHOTO_PRIMARIES),
                p.map(|v| eval_ext(&PROPHOTO, v)),
            ),
            Space::Rec2020 => apply(&REC2020_GAMUT, p.map(|v| eval_ext(&REC2020, v))),
            Space::XyzD50 => p,
            Space::XyzD65 => xyz65_to_d50(p),
            Space::Lab => lab_to_xyz(p[0], p[1], p[2]),
            Space::Oklab => xyz65_to_d50(oklab_to_xyz(p[0], p[1], p[2])),
            Space::Lch => {
                let [l, a, b] = lch_to_lab(p[0], p[1], p[2]);
                lab_to_xyz(l, a, b)
            }
            Space::Oklch => {
                let [l, a, b] = lch_to_lab(p[0], p[1], p[2]);
                xyz65_to_d50(oklab_to_xyz(l, a, b))
            }
            Space::Hsl => {
                let (r, g, b) = hsl_to_srgb(p[0], p[1], p[2]);
                apply(&srgb_gamut(), srgb_transfer([r, g, b]))
            }
            Space::Hwb => {
                let (r, g, b) = hwb_to_srgb(p[0], p[1], p[2]);
                apply(&srgb_gamut(), srgb_transfer([r, g, b]))
            }
        }
    }

    /// `Color::ToSkColor4fInternal` without the (disabled-by-default) gamut map.
    pub fn to_srgb(c: super::Parsed) -> [f32; 3] {
        let p = c.p;
        match c.space {
            Space::Srgb => p,
            Space::SrgbLegacy => p.map(|v| v / 255.0),
            Space::SrgbLinear => srgb_inverse(p),
            Space::DisplayP3 => xyz50_to_srgb(apply(&DISPLAY_P3, srgb_transfer(p))),
            Space::A98Rgb => xyz50_to_srgb(apply(&adobe_gamut(), p.map(|v| eval_ext(&G2_2, v)))),
            Space::ProPhoto => xyz50_to_srgb(apply(
                &primaries_to_d50(PROPHOTO_PRIMARIES),
                p.map(|v| eval_ext(&PROPHOTO, v)),
            )),
            Space::Rec2020 => {
                xyz50_to_srgb(apply(&REC2020_GAMUT, p.map(|v| eval_ext(&REC2020, v))))
            }
            Space::XyzD50 => xyz50_to_srgb(p),
            Space::XyzD65 => srgb_inverse(xyz65_to_srgb_linear(p)),
            Space::Lab => xyz50_to_srgb(lab_to_xyz(p[0], p[1], p[2])),
            Space::Oklab => srgb_inverse(xyz65_to_srgb_linear(oklab_to_xyz(p[0], p[1], p[2]))),
            Space::Lch => {
                let [l, a, b] = lch_to_lab(p[0], p[1], p[2]);
                xyz50_to_srgb(lab_to_xyz(l, a, b))
            }
            Space::Oklch => {
                let [l, a, b] = lch_to_lab(p[0], p[1], p[2]);
                srgb_inverse(xyz65_to_srgb_linear(oklab_to_xyz(l, a, b)))
            }
            Space::Hsl => {
                let (r, g, b) = hsl_to_srgb(p[0], p[1], p[2]);
                [r, g, b]
            }
            Space::Hwb => {
                let (r, g, b) = hwb_to_srgb(p[0], p[1], p[2]);
                [r, g, b]
            }
        }
    }

    /// `AngleToUnitCircleDegrees`.
    fn wrap_degrees(angle: f32) -> f32 {
        let a = angle % 360.0;
        if a < 0.0 { a + 360.0 } else { a }
    }

    /// `Color::ConvertToColorSpace` (with missing components resolved to 0).
    pub fn convert(c: &mut super::Parsed, dest: Space) {
        if c.space == dest {
            return;
        }
        // ResolveMissingComponents.
        for i in 0..3 {
            if c.none[i] {
                c.p[i] = 0.0;
                c.none[i] = false;
            }
        }
        let p = c.p;
        c.p = match dest {
            Space::XyzD65 => {
                if c.space == Space::Oklab {
                    oklab_to_xyz(p[0], p[1], p[2])
                } else {
                    xyz50_to_d65(to_xyz50(c.space, p))
                }
            }
            Space::XyzD50 => to_xyz50(c.space, p),
            Space::SrgbLinear => xyz50_to_srgb_linear(to_xyz50(c.space, p)),
            Space::DisplayP3 => {
                let inv = invert_m(&DISPLAY_P3).unwrap();
                srgb_inverse(apply(&inv, to_xyz50(c.space, p)))
            }
            Space::A98Rgb => {
                let inv = invert_m(&adobe_gamut()).unwrap();
                apply(&inv, to_xyz50(c.space, p)).map(|v| eval_ext(&invert(&G2_2), v))
            }
            Space::ProPhoto => {
                let inv = invert_m(&primaries_to_d50(PROPHOTO_PRIMARIES)).unwrap();
                apply(&inv, to_xyz50(c.space, p)).map(|v| eval_ext(&invert(&PROPHOTO), v))
            }
            Space::Rec2020 => {
                let inv = invert_m(&REC2020_GAMUT).unwrap();
                apply(&inv, to_xyz50(c.space, p)).map(|v| eval_ext(&invert(&REC2020), v))
            }
            Space::Lab => {
                if c.space == Space::Lch {
                    lch_to_lab(p[0], p[1], p[2])
                } else {
                    let [x, y, z] = to_xyz50(c.space, p);
                    xyz_to_lab(x, y, z)
                }
            }
            Space::Oklab => {
                if c.space == Space::Oklab {
                    return;
                }
                if c.space == Space::Oklch {
                    c.space = Space::Oklab;
                    c.p = lch_to_lab(p[0], p[1], p[2]);
                    return;
                }
                let xyz65 = if c.space == Space::XyzD65 {
                    p
                } else {
                    xyz50_to_d65(to_xyz50(c.space, p))
                };
                xyz_to_oklab(xyz65)
            }
            Space::Lch => {
                let [l, a, b] = if c.space == Space::Lab {
                    p
                } else {
                    let [x, y, z] = to_xyz50(c.space, p);
                    xyz_to_lab(x, y, z)
                };
                let [l, chroma, h] = lab_to_lch(l, a, b);
                if chroma <= 0.02 {
                    c.none[2] = true;
                }
                [l, chroma, wrap_degrees(h)]
            }
            Space::Oklch => {
                let [l, chroma, h] = if c.space == Space::Oklab {
                    lab_to_lch(p[0], p[1], p[2])
                } else {
                    let xyz65 = if c.space == Space::XyzD65 {
                        p
                    } else {
                        xyz50_to_d65(to_xyz50(c.space, p))
                    };
                    let [l, a, b] = xyz_to_oklab(xyz65);
                    lab_to_lch(l, a, b)
                };
                if chroma <= 0.02 {
                    c.none[2] = true;
                }
                [l, chroma, wrap_degrees(h)]
            }
            Space::Srgb | Space::SrgbLegacy => {
                let srgb = if c.space == Space::Hsl {
                    let (r, g, b) = hsl_to_srgb(p[0], p[1], p[2]);
                    [r, g, b]
                } else if c.space == Space::Hwb {
                    let (r, g, b) = hwb_to_srgb(p[0], p[1], p[2]);
                    [r, g, b]
                } else if c.space == Space::SrgbLegacy {
                    p.map(|v| v / 255.0)
                } else if c.space != Space::Srgb {
                    xyz50_to_srgb(to_xyz50(c.space, p))
                } else {
                    p
                };
                if dest == Space::SrgbLegacy {
                    srgb.map(|v| v * 255.0)
                } else {
                    srgb
                }
            }
            Space::Hsl => {
                let srgb = if c.space == Space::SrgbLegacy {
                    p.map(|v| v / 255.0)
                } else if c.space == Space::Srgb {
                    p
                } else if c.space == Space::Hwb {
                    let (r, g, b) = hwb_to_srgb(p[0], p[1], p[2]);
                    [r, g, b]
                } else {
                    xyz50_to_srgb(to_xyz50(c.space, p))
                };
                let [h, s, l] = srgb_to_hsl(srgb[0], srgb[1], srgb[2]);
                if s == 0.0 {
                    c.none[0] = true;
                }
                [h, s, l]
            }
            Space::Hwb => {
                let srgb = if c.space == Space::SrgbLegacy {
                    p.map(|v| v / 255.0)
                } else if c.space == Space::Srgb {
                    p
                } else if c.space == Space::Hsl {
                    let (r, g, b) = hsl_to_srgb(p[0], p[1], p[2]);
                    [r, g, b]
                } else {
                    xyz50_to_srgb(to_xyz50(c.space, p))
                };
                let [h, w, bl] = srgb_to_hwb(srgb[0], srgb[1], srgb[2]);
                if w + bl >= 1.0 {
                    c.none[0] = true;
                }
                [h, w, bl]
            }
        };
        c.space = dest;
    }
}

/// Parses a canvas `fillStyle`/`strokeStyle` string the way Blink's CSS colour parser does for
/// the syntaxes painters use: hex, named colours, `transparent`, `rgb[a]()` and `hsl[a]()` in
/// comma and space forms. `None` means the assignment is ignored.
pub fn parse(css: &str) -> Option<Rgba> {
    parse_color(css).map(Parsed::to_rgba)
}

pub fn parse_color(css: &str) -> Option<Parsed> {
    // `ParseCanvasColorString`.
    let s = css.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'));
    if s.eq_ignore_ascii_case("currentcolor") {
        return Some(Parsed::legacy([0.0; 3], 1.0));
    }
    let lower = s.to_ascii_lowercase();
    // `CSSParser::ParseColor`: `SetNamedColor` on the raw string, then the
    // strict fast paths.
    if let Some(rgb) = named(&lower) {
        return Some(Parsed::legacy(rgb.map(f32::from), 1.0));
    }
    if let Some(p) = parse_fast(s) {
        return Some(p);
    }
    if lower == "transparent" {
        return Some(Parsed::legacy([0.0; 3], 0.0));
    }
    // `CSSParser::ParseSystemColor` with the default colour provider.
    if let Some(p) = system_color(&lower) {
        return Some(p);
    }
    // `ParseSingleValue(kColor)` accepts resolved absolute colours;
    // `ConsumeColorWithoutElementContext` the colour-mix and relative forms,
    // which `ResolveColorValue` then computes.
    let toks = tokenize(s);
    if toks.is_empty() || has_substitution(&toks) {
        return None;
    }
    let mut cur = Cursor { toks: &toks, i: 0 };
    match consume_color(&mut cur, false) {
        Some(node) if cur.at_end() => match node {
            Node::Color(mut c) => {
                resolve_non_finite(&mut c);
                Some(c)
            }
            Node::Mix(_) | Node::Relative(_) => resolve(&node),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `name` (already lower case) is a `ParseSystemColor` keyword. Canvas
/// paints report these unmodeled: upstream resolves them through the platform
/// `LayoutTheme`, which the port does not have.
pub fn is_system_color(name: &str) -> bool {
    system_color(name).is_some()
}

/// The `ui::ColorProvider` defaults `CSSParser::ParseSystemColor` resolves.
fn system_color(name: &str) -> Option<Parsed> {
    let (rgb, a): ([u8; 3], f32) = match name {
        "activeborder" | "buttonborder" | "inactiveborder" | "threeddarkshadow"
        | "threedhighlight" | "threedlightshadow" | "threedshadow" | "windowframe" => {
            ([0x76, 0x76, 0x76], 1.0)
        }
        "activecaption" | "buttontext" | "canvastext" | "captiontext" | "fieldtext"
        | "highlighttext" | "infotext" | "marktext" | "menutext" | "selecteditemtext"
        | "windowtext" => ([0x00, 0x00, 0x00], 1.0),
        "activetext" => ([0xff, 0x00, 0x00], 1.0),
        "appworkspace" | "background" | "canvas" | "field" | "inactivecaption"
        | "infobackground" | "menu" | "scrollbar" | "window" => ([0xff, 0xff, 0xff], 1.0),
        "buttonface" | "buttonhighlight" | "buttonshadow" | "threedface" => {
            ([0xef, 0xef, 0xef], 1.0)
        }
        "graytext" | "inactivecaptiontext" => ([0x80, 0x80, 0x80], 1.0),
        "highlight" => ([0x80, 0xbc, 0xfe], 0.6),
        "linktext" => ([0x00, 0x00, 0xee], 1.0),
        "mark" => ([0xff, 0xff, 0x00], 1.0),
        "selecteditem" => ([0xb3, 0xd7, 0xff], 1.0),
        "visitedtext" => ([0x55, 0x1a, 0x8b], 1.0),
        _ => return None,
    };
    Some(Parsed::legacy(rgb.map(f32::from), a))
}

/// The `[r, g, b]` of a named colour, `name` in lower case.
pub fn named(name: &str) -> Option<[u8; 3]> {
    let i = NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok()?;
    let v = NAMED[i].1;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// `Color::ParseHexColor` on the digits after `#`.
pub fn parse_hex(hex: &str) -> Option<Parsed> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let c: Vec<u8> = match hex.len() {
        3 | 4 => (0..hex.len())
            .map(|i| digit(i).map(|d| d * 17))
            .collect::<Option<_>>()?,
        6 | 8 => (0..hex.len() / 2)
            .map(|i| pair(i * 2))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let alpha = c.get(3).map_or(1.0, |&a| f32::from(a) / 255.0);
    Some(Parsed::legacy([c[0], c[1], c[2]].map(f32::from), alpha))
}

fn is_html_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

fn skip_spaces(chars: &[u8]) -> &[u8] {
    &chars[chars.iter().take_while(|b| is_html_space(**b)).count()..]
}

/// `FindLengthOfValidDouble`. Its SIMD variant differs only on a trailing `.`, which every
/// caller here rejects either way.
fn valid_double_length(chars: &[u8]) -> usize {
    let mut seen_mark = false;
    let length = chars
        .iter()
        .take_while(|&&c| {
            c.is_ascii_digit() || (c == b'.' && !std::mem::replace(&mut seen_mark, true))
        })
        .count();
    if length > 0 && chars[length - 1] == b'.' {
        0
    } else {
        length
    }
}

/// `ParsePositiveDouble` from Blink's CSS fast paths: the length of the double at the start of
/// `chars` (0 for none) and its value. Only the first seven decimals count. When at least seven
/// bytes of `chars` follow the decimal mark, the SSE2 and NEON paths scale the decimals as one
/// integer by a constant slightly above 1e-7; otherwise they are divided exactly.
pub fn parse_positive_double(chars: &[u8]) -> (usize, f64) {
    const DIV_1E7: f64 = f64::from_bits(1e-7_f64.to_bits() + 1);
    let length = valid_double_length(chars);
    if length == 0 {
        return (0, 0.0);
    }
    let mut value = 0.0;
    let mut position = 0;
    while position < length && chars[position] != b'.' {
        value = value * 10.0 + f64::from(chars[position] - b'0');
        position += 1;
    }
    position += 1;
    if position >= length {
        return (length, value);
    }
    let decimals = &chars[position..position + (length - position).min(7)];
    if chars.len() - position >= 7 {
        let fraction = decimals
            .iter()
            .fold(0u32, |acc, d| acc * 10 + u32::from(d - b'0'))
            * 10u32.pow(7 - decimals.len() as u32);
        return (length, value + f64::from(fraction) * DIV_1E7);
    }
    let (mut fraction, mut scale) = (0.0, 1.0);
    for d in decimals {
        fraction = fraction * 10.0 + f64::from(d - b'0');
        scale *= 10.0;
    }
    (length, value + fraction / scale)
}

#[derive(Clone, Copy, PartialEq)]
enum Terminator {
    Could,
    MustWhitespace,
    MustCharacter,
}

/// `SkipToTerminator`: advances `chars` only on success.
fn skip_to_terminator(chars: &mut &[u8], terminator: u8, status: &mut Terminator) -> bool {
    let mut current = skip_spaces(chars);
    let skipped = current.len() != chars.len();
    match *status {
        Terminator::Could if current.first() == Some(&terminator) => {
            *status = Terminator::MustCharacter;
            current = &current[1..];
        }
        Terminator::Could | Terminator::MustWhitespace => {
            *status = Terminator::MustWhitespace;
            if !skipped {
                return false;
            }
        }
        Terminator::MustCharacter => {
            if current.first() != Some(&terminator) {
                return false;
            }
            current = &current[1..];
        }
    }
    *chars = current;
    true
}

/// `ParseFloatWithMaxValue`: the value (clamped to `max` once its integer digits reach it) and
/// whether a `-` preceded it.
fn float_with_max(chars: &mut &[u8], max: f64) -> Option<(f64, bool)> {
    let mut current = skip_spaces(chars);
    let negative = current.first() == Some(&b'-');
    if negative {
        current = &current[1..];
    }
    if !current.first()?.is_ascii_digit() {
        return None;
    }
    let mut value = 0.0;
    while let Some(d) = current.first().filter(|d| d.is_ascii_digit()) {
        let new_value = value * 10.0 + f64::from(d - b'0');
        current = &current[1..];
        if new_value >= max {
            value = max;
            current = &current[current.iter().take_while(|d| d.is_ascii_digit()).count()..];
            break;
        }
        value = new_value;
    }
    if current.first() == Some(&b'.') {
        let (length, fraction) = parse_positive_double(current);
        if length == 0 {
            return None;
        }
        current = &current[length..];
        value += fraction;
    }
    *chars = current;
    Some((value, negative))
}

#[derive(Clone, Copy, PartialEq)]
enum ChannelUnit {
    Unknown,
    Number,
    Percentage,
}

/// `ParseColorNumberOrPercentage`.
fn color_channel(
    chars: &mut &[u8],
    terminator: u8,
    status: &mut Terminator,
    expect: &mut ChannelUnit,
) -> Option<u8> {
    let mut current = *chars;
    let (mut value, negative) = float_with_max(&mut current, 255.0)?;
    let percent = *current.first()? == b'%';
    if (*expect == ChannelUnit::Percentage && !percent)
        || (*expect == ChannelUnit::Number && percent)
    {
        return None;
    }
    if percent {
        *expect = ChannelUnit::Percentage;
        value = value / 100.0 * 255.0;
        if value > 255.0 {
            value = 255.0;
        }
        current = &current[1..];
    } else {
        *expect = ChannelUnit::Number;
    }
    if !skip_to_terminator(&mut current, terminator, status) {
        return None;
    }
    *chars = current;
    Some(if negative {
        0
    } else {
        value.round().min(255.0) as u8
    })
}

/// `ParsePercentage`: a percentage as a fraction in 0..=1.
fn percentage(chars: &mut &[u8], terminator: u8, status: &mut Terminator) -> Option<f64> {
    let mut current = *chars;
    let (value, negative) = float_with_max(&mut current, 100.0)?;
    if current.first() != Some(&b'%') {
        return None;
    }
    current = &current[1..];
    let value = if negative {
        0.0
    } else {
        (value * 0.01).min(1.0)
    };
    if !skip_to_terminator(&mut current, terminator, status) {
        return None;
    }
    *chars = current;
    Some(value)
}

/// `ParseAlphaValue`: the alpha as a byte; the value must end `chars`.
fn alpha_byte(chars: &[u8], terminator: u8) -> Option<u8> {
    const TENTHS: [u8; 10] = [0, 26, 51, 77, 102, 128, 153, 179, 204, 230];
    let mut chars = skip_spaces(chars);
    let negative = chars.first() == Some(&b'-');
    if negative {
        chars = &chars[1..];
    }
    let length = chars.len();
    if length < 2 || chars[length - 1] != terminator || !chars[length - 2].is_ascii_digit() {
        return None;
    }
    if !matches!(chars[0], b'0' | b'1' | b'.') {
        let double_length = valid_double_length(chars);
        return (double_length > 0 && chars.get(double_length) == Some(&terminator))
            .then_some(if negative { 0 } else { 255 });
    }
    if length == 2 && chars[0] != b'.' {
        return Some(if !negative && chars[0] == b'1' {
            255
        } else {
            0
        });
    }
    let body = &chars[..length - 1];
    if matches!(body, [b'0', b'.', d] | [b'.', d] if d.is_ascii_digit()) {
        return Some(if negative {
            0
        } else {
            TENTHS[usize::from(body[body.len() - 1] - b'0')]
        });
    }
    let (double_length, alpha) = parse_positive_double(chars);
    if double_length == 0 || chars.get(double_length) != Some(&terminator) {
        return None;
    }
    Some(if negative {
        0
    } else {
        (alpha.min(1.0) * 255.0).round() as u8
    })
}

#[derive(Clone, Copy)]
enum AngleUnit {
    Degrees,
    Gradians,
    Radians,
    Turns,
}

/// `ParseSimpleAngle`: length, value clamped to the float range, and unit (unitless is
/// degrees).
fn simple_angle(chars: &[u8]) -> Option<(usize, f64, AngleUnit)> {
    let negative = chars.first() == Some(&b'-');
    let (length, n) = parse_positive_double(&chars[usize::from(negative)..]);
    if length == 0 {
        return None;
    }
    let n = n.min(f64::from(f32::MAX));
    let (length, number) = if negative {
        (length + 1, -n)
    } else {
        (length, n)
    };
    let rest = &chars[length..];
    let unit_is = |name: &[u8]| {
        rest.len() >= name.len() && rest.iter().zip(name).all(|(c, n)| c | 0x20 == *n)
    };
    Some(if unit_is(b"deg") {
        (length + 3, number, AngleUnit::Degrees)
    } else if unit_is(b"grad") {
        (length + 4, number, AngleUnit::Gradians)
    } else if unit_is(b"rad") {
        (length + 3, number, AngleUnit::Radians)
    } else if unit_is(b"turn") {
        (length + 4, number, AngleUnit::Turns)
    } else {
        (length, number, AngleUnit::Degrees)
    })
}

/// `FastParseColorInternal` outside quirks mode: `#` hex and the `rgb[a]()` and `hsl[a]()`
/// forms its fast path accepts. `None` when Blink hands the string to its full parser.
pub fn parse_fast(css: &str) -> Option<Parsed> {
    let chars = css.as_bytes();
    if chars.len() >= 4 && chars[0] == b'#' {
        return parse_hex(&css[1..]);
    }
    let function = |name: &[u8]| {
        (chars.len() >= 5
            && chars.starts_with(name)
            && (chars[3] == b'(' || (chars[3] == b'a' && chars[4] == b'(')))
            .then(|| &chars[if chars[3] == b'a' { 5 } else { 4 }..])
    };
    if let Some(args) = function(b"rgb") {
        return fast_rgb(args);
    }
    function(b"hsl").and_then(fast_hsl)
}

fn fast_rgb(mut current: &[u8]) -> Option<Parsed> {
    let mut status = Terminator::Could;
    let mut expect = ChannelUnit::Unknown;
    let red = color_channel(&mut current, b',', &mut status, &mut expect)?;
    let green = color_channel(&mut current, b',', &mut status, &mut expect)?;
    let mut strict = Terminator::MustCharacter;
    let (blue, has_alpha) =
        if let Some(blue) = color_channel(&mut current, b',', &mut strict, &mut expect) {
            if status != Terminator::MustCharacter {
                return None;
            }
            (blue, true)
        } else if let Some(blue) = color_channel(&mut current, b'/', &mut strict, &mut expect) {
            if status != Terminator::MustWhitespace {
                return None;
            }
            (blue, true)
        } else {
            (
                color_channel(&mut current, b')', &mut strict, &mut expect)?,
                false,
            )
        };
    let alpha = if has_alpha {
        f32::from(alpha_byte(current, b')')?) / 255.0
    } else if current.is_empty() {
        1.0
    } else {
        return None;
    };
    Some(Parsed::legacy([red, green, blue].map(f32::from), alpha))
}

fn fast_hsl(current: &[u8]) -> Option<Parsed> {
    let mut current = skip_spaces(current);
    let (length, mut hue, unit) = simple_angle(current)?;
    hue = match unit {
        AngleUnit::Degrees => hue,
        AngleUnit::Radians => hue * (180.0 / std::f64::consts::PI),
        AngleUnit::Gradians => hue * (360.0 / 400.0),
        AngleUnit::Turns => hue * 360.0,
    };
    if hue < 0.0 {
        hue = hue % 360.0 + 360.0;
    } else if hue > 360.0 {
        hue %= 360.0;
    }
    current = &current[length..];
    let mut status = Terminator::Could;
    if !skip_to_terminator(&mut current, b',', &mut status) {
        return None;
    }
    let saturation = percentage(&mut current, b',', &mut status)?;
    let mut strict = Terminator::MustCharacter;
    let (lightness, has_alpha) = if let Some(l) = percentage(&mut current, b',', &mut strict) {
        if status != Terminator::MustCharacter {
            return None;
        }
        (l, true)
    } else if let Some(l) = percentage(&mut current, b'/', &mut strict) {
        if status != Terminator::MustWhitespace {
            return None;
        }
        (l, true)
    } else {
        (percentage(&mut current, b')', &mut strict)?, false)
    };
    let alpha = if has_alpha {
        f32::from(alpha_byte(current, b')')?) * (1.0 / 255.0)
    } else if current.is_empty() {
        1.0
    } else {
        return None;
    };
    Some(hsl(hue as f32, saturation as f32, lightness as f32, alpha))
}

fn hsl(h: f32, s: f32, l: f32, alpha: f32) -> Parsed {
    Parsed {
        space: Space::Hsl,
        p: [h, s, l],
        none: [false; 3],
        a: alpha,
        a_none: false,
    }
}

/// What Blink's full parser makes of a function's arguments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slow<T> {
    Valid(T),
    Invalid,
    /// Arguments outside the plain number and percentage tokens modelled here.
    Unmodeled,
}

const NAMED: [(&str, u32); 148] = [
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

/// The colour keywords Blink accepts in author style besides the named
/// colours, probed over every keyword of Chromium m142's
/// `css_value_keywords.json5`.
pub const COLOR_KEYWORDS: [&str; 44] = [
    "-webkit-activelink",
    "-webkit-link",
    "activeborder",
    "activecaption",
    "activetext",
    "appworkspace",
    "background",
    "buttonborder",
    "buttonface",
    "buttonhighlight",
    "buttonshadow",
    "buttontext",
    "canvas",
    "canvastext",
    "captiontext",
    "currentcolor",
    "field",
    "fieldtext",
    "graytext",
    "highlight",
    "highlighttext",
    "inactiveborder",
    "inactivecaption",
    "inactivecaptiontext",
    "infobackground",
    "infotext",
    "linktext",
    "mark",
    "marktext",
    "menu",
    "menutext",
    "scrollbar",
    "selecteditem",
    "selecteditemtext",
    "threeddarkshadow",
    "threedface",
    "threedhighlight",
    "threedlightshadow",
    "threedshadow",
    "transparent",
    "visitedtext",
    "window",
    "windowframe",
    "windowtext",
];

pub fn keyword(name: &str) -> bool {
    named(name).is_some() || COLOR_KEYWORDS.contains(&name)
}

/// The CSS Syntax tokenizer, reduced to the token types colour parsing sees.
/// `Delim` is any single-code-point token (`(`, `)`, `/`, `+`, ...); `Fn` holds
/// the already-tokenized argument list.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Fn(String, Vec<Tok>),
    Url(String),
    BadUrl,
    Str(String),
    BadStr,
    Number(f64),
    Dim(f64, String),
    Percent(f64),
    Hash(String),
    AtKeyword(String),
    Comma,
    Delim(u8),
}

impl Tok {
    fn ident(&self, name: &str) -> bool {
        matches!(self, Tok::Ident(s) if s.eq_ignore_ascii_case(name))
    }
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') || b >= 0x80
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

/// Consumes `\x` escape sequences in identifiers; returns (char-utf8-len-or-0
/// for a line break) and the byte length consumed.
fn consume_escape(b: &[u8], i: usize) -> (String, usize) {
    let mut j = i + 1;
    if j >= b.len() || b[j] == b'\n' {
        return ("\u{fffd}".into(), 1);
    }
    if b[j].is_ascii_hexdigit() {
        let start = j;
        while j < b.len() && j - start < 6 && b[j].is_ascii_hexdigit() {
            j += 1;
        }
        let cp = u32::from_str_radix(std::str::from_utf8(&b[start..j]).unwrap(), 16).unwrap();
        if j < b.len() && is_ws(b[j]) {
            j += 1;
            if b[j - 1] == b'\r' && b.get(j) == Some(&b'\n') {
                j += 1;
            }
        }
        let c =
            char::from_u32(cp).filter(|&c| c != '\0' && !(0xd800..0xe000).contains(&(c as u32)));
        return (c.unwrap_or('\u{fffd}').to_string(), j - i);
    }
    let c = std::str::from_utf8(&b[j..]).unwrap_or("").chars().next();
    match c {
        Some(c) => (c.to_string(), j + c.len_utf8() - i),
        None => ("\u{fffd}".into(), j - i),
    }
}

fn consume_name(b: &[u8], mut i: usize) -> (String, usize) {
    let mut name = String::new();
    while i < b.len() {
        if is_name_char(b[i]) {
            name.push(b[i] as char);
            i += 1;
        } else if b[i] == b'\\' && b.get(i + 1).is_some_and(|&n| n != b'\n') {
            let (s, n) = consume_escape(b, i);
            name.push_str(&s);
            i += n;
        } else {
            break;
        }
    }
    (name, i)
}

fn ident_starts(b: &[u8], i: usize) -> bool {
    match b.get(i) {
        Some(&c) if is_name_start(c) => true,
        Some(b'-') => match b.get(i + 1) {
            Some(&c) if is_name_start(c) || c == b'-' => true,
            Some(b'\\') => b.get(i + 2).is_some_and(|&c| c != b'\n'),
            _ => false,
        },
        Some(b'\\') => b.get(i + 1).is_some_and(|&c| c != b'\n'),
        _ => false,
    }
}

fn number_starts(b: &[u8], i: usize) -> bool {
    let digit = |j: usize| b.get(j).is_some_and(u8::is_ascii_digit);
    match b.get(i) {
        Some(b'+' | b'-') => digit(i + 1) || (b.get(i + 1) == Some(&b'.') && digit(i + 2)),
        Some(b'.') => digit(i + 1),
        Some(&c) => c.is_ascii_digit(),
        None => false,
    }
}

/// The number a `<number-token>` covers plus its byte length.
fn take_number(b: &[u8], i: usize) -> (f64, usize) {
    let digit = |j: usize| b.get(j).is_some_and(u8::is_ascii_digit);
    let mut j = i + usize::from(matches!(b.get(i), Some(b'+' | b'-')));
    while digit(j) {
        j += 1;
    }
    if b.get(j) == Some(&b'.') && digit(j + 1) {
        j += 1;
        while digit(j) {
            j += 1;
        }
    }
    if matches!(b.get(j), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(b.get(j + 1), Some(b'+' | b'-')));
        if digit(j + 1 + sign) {
            j += 1 + sign;
            while digit(j) {
                j += 1;
            }
        }
    }
    let text = std::str::from_utf8(&b[i..j]).unwrap_or("");
    (text.parse::<f64>().unwrap_or(0.0), j - i)
}

fn skip_ws_comments(b: &[u8], mut i: usize, end: usize) -> usize {
    loop {
        while i < end && is_ws(b[i]) {
            i += 1;
        }
        if i + 1 < end && b[i] == b'/' && b[i + 1] == b'*' {
            match b[i + 2..end].windows(2).position(|w| w == b"*/") {
                Some(p) => i += 4 + p,
                None => return end,
            }
        } else {
            return i;
        }
    }
}

/// Tokenizes `b[..end]`; stops after a `)`/`]`/`}` that closes the block, which
/// is reported as the second return value so the caller can continue.
fn tokenize_block(b: &[u8], mut i: usize, end: usize, nested: bool) -> (Vec<Tok>, usize) {
    let mut out = Vec::new();
    let mut depth = 0usize;
    loop {
        i = skip_ws_comments(b, i, end);
        if i >= end {
            break;
        }
        let c = b[i];
        match c {
            b')' | b']' | b'}' if nested && depth == 0 => break,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                out.push(Tok::Delim(c));
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                out.push(Tok::Delim(c));
                i += 1;
            }
            b'"' | b'\'' => {
                let mut j = i + 1;
                let mut s = String::new();
                let mut bad = false;
                loop {
                    if j >= end {
                        break;
                    }
                    if b[j] == c {
                        j += 1;
                        break;
                    }
                    if b[j] == b'\n' {
                        bad = true;
                        break;
                    }
                    if b[j] == b'\\' {
                        if j + 1 < end && b[j + 1] == b'\n' {
                            j += 2;
                            continue;
                        }
                        let (esc, n) = consume_escape(b, j);
                        s.push_str(&esc);
                        j += n;
                        continue;
                    }
                    let ch = std::str::from_utf8(&b[j..end])
                        .ok()
                        .and_then(|t| t.chars().next());
                    match ch {
                        Some(ch) => {
                            s.push(ch);
                            j += ch.len_utf8();
                        }
                        None => {
                            j += 1;
                        }
                    }
                }
                out.push(if bad { Tok::BadStr } else { Tok::Str(s) });
                i = j;
            }
            b'#' if i + 1 < end && (is_name_char(b[i + 1]) || b[i + 1] == b'\\') => {
                let (name, next) = consume_name(b, i + 1);
                out.push(Tok::Hash(name));
                i = next;
            }
            b'@' if ident_starts(b, i + 1) => {
                let (name, next) = consume_name(b, i + 1);
                out.push(Tok::AtKeyword(name));
                i = next;
            }
            _ if number_starts(b, i) => {
                let (v, n) = take_number(b, i);
                let j = i + n;
                if ident_starts(b, j) {
                    let (unit, next) = consume_name(b, j);
                    out.push(Tok::Dim(v, unit.to_ascii_lowercase()));
                    i = next;
                } else if b.get(j) == Some(&b'%') {
                    out.push(Tok::Percent(v));
                    i = j + 1;
                } else {
                    out.push(Tok::Number(v));
                    i = j;
                }
            }
            _ if ident_starts(b, i) => {
                let (name, next) = consume_name(b, i);
                if b.get(next) == Some(&b'(') {
                    let after = skip_ws_comments(b, next + 1, end);
                    if name.eq_ignore_ascii_case("url")
                        && !matches!(b.get(after), Some(b'"' | b'\''))
                    {
                        let (tok, j) = consume_url_token(b, next + 1, end);
                        out.push(tok);
                        i = j;
                    } else {
                        let (args, j) = tokenize_block(b, next + 1, end, true);
                        out.push(Tok::Fn(name, args));
                        i = j + usize::from(b.get(j) == Some(&b')'));
                    }
                } else {
                    out.push(Tok::Ident(name));
                    i = next;
                }
            }
            _ => {
                let ch = std::str::from_utf8(&b[i..end])
                    .ok()
                    .and_then(|t| t.chars().next());
                match ch {
                    Some(ch) => {
                        out.push(Tok::Delim(if ch.is_ascii() { ch as u8 } else { 0 }));
                        i += ch.len_utf8();
                    }
                    None => {
                        out.push(Tok::Delim(c));
                        i += 1;
                    }
                }
            }
        }
    }
    (out, i)
}

/// `consume-url-token`: the argument of `url(` taken literally up to `)`.
fn consume_url_token(b: &[u8], mut i: usize, end: usize) -> (Tok, usize) {
    i = skip_ws_comments(b, i, end);
    let mut value = String::new();
    let mut bad = false;
    while i < end {
        let c = b[i];
        if c == b')' {
            i += 1;
            let value = value.trim_end_matches([' ', '\t', '\n', '\r', '\x0c']);
            return (
                if bad {
                    Tok::BadUrl
                } else {
                    Tok::Url(value.into())
                },
                i,
            );
        }
        if is_ws(c) {
            let next = skip_ws_comments(b, i + 1, end);
            if next >= end || b[next] == b')' {
                i = next;
                continue;
            }
            bad = true;
        }
        if matches!(c, b'"' | b'\'' | b'(') || c < 0x20 && c != b'\t' || c == 0x7f {
            bad = true;
        }
        if c == b'\\' {
            if i + 1 < end && b[i + 1] != b'\n' {
                let (s, n) = consume_escape(b, i);
                value.push_str(&s);
                i += n;
                continue;
            }
            bad = true;
        }
        let ch = std::str::from_utf8(&b[i..end])
            .ok()
            .and_then(|t| t.chars().next());
        match ch {
            Some(ch) => {
                value.push(ch);
                i += ch.len_utf8();
            }
            None => {
                value.push('\u{fffd}');
                i += 1;
            }
        }
    }
    (if bad { Tok::BadUrl } else { Tok::Url(value) }, i)
}

/// The tokens of a whole value. `end` marks where each token's text ends so a
/// pending-substitution value can echo its exact source span.
pub struct Tokens {
    pub toks: Vec<Tok>,
}

pub fn tokenize(value: &str) -> Vec<Tok> {
    tokenize_block(value.as_bytes(), 0, value.len(), false).0
}

/// True when the token list holds a `var()`/`env()`/`attr()`/dashed function,
/// making the whole declaration a pending-substitution value.
fn has_substitution(toks: &[Tok]) -> bool {
    toks.iter().any(|t| match t {
        Tok::Fn(name, args) => {
            let n = name.to_ascii_lowercase();
            matches!(n.as_str(), "var" | "env" | "attr")
                || n.starts_with("--")
                || has_substitution(args)
        }
        _ => false,
    })
}

/// `CSSMathExpressionNode` folded to a literal, which is all a colour channel
/// can be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cat {
    Number,
    Percent,
    Angle,
    /// Any other calculation category (lengths, times, mixed sums): never
    /// acceptable to `ConsumeNumber`/`ConsumePercent`/`ConsumeAngle`.
    Other,
}

#[derive(Clone, Debug)]
pub struct Calc {
    pub v: f64,
    pub cat: Cat,
}

impl Calc {
    /// The literal's serialised form (`calc(<v>)`, `<v>%` or `<v>deg`).
    fn text(&self) -> String {
        let suffix = match self.cat {
            Cat::Percent => "%",
            Cat::Angle => "deg",
            _ => "",
        };
        format!("calc({}{})", fmt_num(self.v), suffix)
    }
}

type Vars<'a> = Option<&'a dyn Fn(&str) -> Option<f64>>;

/// Evaluates a `calc()` argument list (or a parenthesised group) to a literal.
/// `vars` resolves channel identifiers in relative colours; `pct_scale` is the
/// `max_value` percentages evaluate against (the channel's percentage base).
fn calc_sum(toks: &[Tok], i: &mut usize, vars: &Vars<'_>, pct_scale: f64) -> Option<Calc> {
    let mut left = calc_product(toks, i, vars, pct_scale)?;
    loop {
        let op = match toks.get(*i) {
            Some(Tok::Delim(b'+')) => 1.0,
            Some(Tok::Delim(b'-')) => -1.0,
            _ => break,
        };
        *i += 1;
        let right = calc_product(toks, i, vars, pct_scale)?;
        if (left.cat == Cat::Other) != (right.cat == Cat::Other) {
            return Some(Calc {
                v: f64::NAN,
                cat: Cat::Other,
            });
        }
        if left.cat != right.cat {
            return Some(Calc {
                v: f64::NAN,
                cat: Cat::Other,
            });
        }
        left = Calc {
            v: left.v + op * right.v,
            cat: left.cat,
        };
    }
    Some(left)
}

fn calc_product(toks: &[Tok], i: &mut usize, vars: &Vars<'_>, pct_scale: f64) -> Option<Calc> {
    let mut left = calc_term(toks, i, vars, pct_scale)?;
    loop {
        match toks.get(*i) {
            Some(Tok::Delim(b'*')) => {
                *i += 1;
                let right = calc_term(toks, i, vars, pct_scale)?;
                left = match (left.cat, right.cat) {
                    (Cat::Number, c) | (c, Cat::Number) if c != Cat::Other => Calc {
                        v: left.v * right.v,
                        cat: c,
                    },
                    (Cat::Number, Cat::Number) => Calc {
                        v: left.v * right.v,
                        cat: Cat::Number,
                    },
                    _ => {
                        return Some(Calc {
                            v: f64::NAN,
                            cat: Cat::Other,
                        });
                    }
                };
            }
            Some(Tok::Delim(b'/')) => {
                *i += 1;
                let right = calc_term(toks, i, vars, pct_scale)?;
                if right.cat != Cat::Number || left.cat == Cat::Other {
                    return Some(Calc {
                        v: f64::NAN,
                        cat: Cat::Other,
                    });
                }
                left = Calc {
                    v: left.v / right.v,
                    cat: left.cat,
                };
            }
            _ => break,
        }
    }
    Some(left)
}

fn angle_degrees(v: f64, unit: &str) -> Option<f64> {
    Some(match unit {
        "deg" => v,
        "grad" => v * 0.9,
        "rad" => v * 180.0 / std::f64::consts::PI,
        "turn" => v * 360.0,
        _ => return None,
    })
}

fn calc_term(toks: &[Tok], i: &mut usize, vars: &Vars<'_>, pct_scale: f64) -> Option<Calc> {
    match toks.get(*i)? {
        Tok::Number(v) => {
            *i += 1;
            Some(Calc {
                v: *v,
                cat: Cat::Number,
            })
        }
        Tok::Percent(v) => {
            *i += 1;
            Some(Calc {
                v: *v,
                cat: Cat::Percent,
            })
        }
        Tok::Dim(v, unit) => {
            *i += 1;
            match angle_degrees(*v, unit) {
                Some(v) => Some(Calc { v, cat: Cat::Angle }),
                None => Some(Calc {
                    v: f64::NAN,
                    cat: Cat::Other,
                }),
            }
        }
        Tok::Delim(b'(') => {
            *i += 1;
            let inner = calc_sum(toks, i, vars, pct_scale)?;
            if toks.get(*i) != Some(&Tok::Delim(b')')) {
                return None;
            }
            *i += 1;
            Some(inner)
        }
        Tok::Delim(b'-') => {
            *i += 1;
            let mut t = calc_term(toks, i, vars, pct_scale)?;
            t.v = -t.v;
            Some(t)
        }
        Tok::Ident(name) => {
            *i += 1;
            let lower = name.to_ascii_lowercase();
            let v = match lower.as_str() {
                "pi" => std::f64::consts::PI,
                "e" => std::f64::consts::E,
                "infinity" => f64::INFINITY,
                "-infinity" => f64::NEG_INFINITY,
                "nan" => f64::NAN,
                _ => vars.as_ref()?(&lower)?,
            };
            Some(Calc {
                v,
                cat: Cat::Number,
            })
        }
        Tok::Fn(name, args) => {
            *i += 1;
            calc_function(&name.to_ascii_lowercase(), args, vars, pct_scale)
        }
        _ => None,
    }
}

fn calc_arg(args: &[Tok], vars: &Vars<'_>, pct_scale: f64, n: usize) -> Option<Calc> {
    let mut i = 0;
    let args = args.get(n..)?;
    let c = calc_sum(args, &mut i, vars, pct_scale)?;
    (i == args.len()).then_some(c)
}

fn calc_function(name: &str, args: &[Tok], vars: &Vars<'_>, pct_scale: f64) -> Option<Calc> {
    let comma_split = |args: &[Tok]| -> Vec<Vec<Tok>> {
        let mut parts = vec![Vec::new()];
        for t in args {
            if *t == Tok::Comma {
                parts.push(Vec::new());
            } else {
                parts.last_mut().unwrap().push(t.clone());
            }
        }
        parts
    };
    let sum_cat = |cs: &[Calc]| -> Cat {
        let cat = cs[0].cat;
        if cs.iter().all(|c| c.cat == cat) {
            cat
        } else {
            Cat::Other
        }
    };
    match name {
        "calc" => {
            let mut i = 0;
            let c = calc_sum(args, &mut i, vars, pct_scale)?;
            (i == args.len()).then_some(c)
        }
        "min" | "max" | "hypot" => {
            let parts = comma_split(args);
            let cs: Vec<Calc> = parts
                .iter()
                .map(|p| calc_arg(p, vars, pct_scale, 0))
                .collect::<Option<_>>()?;
            if cs.is_empty() {
                return None;
            }
            let cat = sum_cat(&cs);
            let v = cs.iter().map(|c| c.v).fold(f64::NAN, |a, b| match name {
                "min" => a.min(b),
                "max" => a.max(b),
                _ => a + b * b,
            });
            Some(Calc {
                v: if name == "hypot" { v.sqrt() } else { v },
                cat,
            })
        }
        "clamp" => {
            let parts = comma_split(args);
            if parts.len() != 3 {
                return None;
            }
            let cs: Vec<Calc> = parts
                .iter()
                .map(|p| calc_arg(p, vars, pct_scale, 0))
                .collect::<Option<_>>()?;
            let cat = sum_cat(&cs);
            let v = cs[1].v.clamp(cs[0].v.min(cs[2].v), cs[0].v.max(cs[2].v));
            Some(Calc { v, cat })
        }
        "round" | "mod" | "rem" => {
            let parts = comma_split(args);
            let (strategy, parts) = if name == "round" && !parts.is_empty() {
                match parts[0].as_slice() {
                    [Tok::Ident(s)]
                        if matches!(
                            s.to_ascii_lowercase().as_str(),
                            "nearest" | "up" | "down" | "to-zero"
                        ) =>
                    {
                        (s.to_ascii_lowercase(), parts[1..].to_vec())
                    }
                    _ => ("nearest".to_string(), parts[..].to_vec()),
                }
            } else {
                (String::new(), parts[..].to_vec())
            };
            if parts.len() != 2 {
                return None;
            }
            let a = calc_arg(&parts[0], vars, pct_scale, 0)?;
            let b = calc_arg(&parts[1], vars, pct_scale, 0)?;
            if a.cat != b.cat {
                return Some(Calc {
                    v: f64::NAN,
                    cat: Cat::Other,
                });
            }
            let v = match name {
                "mod" => a.v % b.v.abs() * b.v.signum().abs().signum() + 0.0 * a.v.signum(),
                "rem" => a.v % b.v,
                _ => {
                    if b.v == 0.0 {
                        a.v
                    } else {
                        let n = a.v / b.v;
                        let n = match strategy.as_str() {
                            "up" => n.ceil(),
                            "down" => n.floor(),
                            "to-zero" => n.trunc(),
                            _ => n.round(),
                        };
                        n * b.v
                    }
                }
            };
            Some(Calc { v, cat: a.cat })
        }
        "abs" | "sign" => {
            let c = calc_arg(args, vars, pct_scale, 0)?;
            Some(if name == "abs" {
                Calc {
                    v: c.v.abs(),
                    cat: c.cat,
                }
            } else {
                Calc {
                    v: c.v.signum(),
                    cat: Cat::Number,
                }
            })
        }
        "sin" | "cos" | "tan" | "asin" | "acos" | "atan" | "pow" | "sqrt" | "log" | "exp"
        | "atan2" => {
            let parts = comma_split(args);
            let one = |args: &[Tok]| calc_arg(args, vars, pct_scale, 0);
            let v = match name {
                "sin" | "cos" | "tan" => {
                    let c = one(args)?;
                    let rad = match c.cat {
                        Cat::Number => c.v,
                        Cat::Angle => c.v.to_radians(),
                        _ => {
                            return Some(Calc {
                                v: f64::NAN,
                                cat: Cat::Other,
                            });
                        }
                    };
                    match name {
                        "sin" => rad.sin(),
                        "cos" => rad.cos(),
                        _ => rad.tan(),
                    }
                }
                "asin" | "acos" | "atan" => {
                    let c = one(args)?;
                    if c.cat != Cat::Number {
                        return Some(Calc {
                            v: f64::NAN,
                            cat: Cat::Other,
                        });
                    }
                    let v = match name {
                        "asin" => c.v.asin(),
                        "acos" => c.v.acos(),
                        _ => c.v.atan(),
                    };
                    return Some(Calc {
                        v: v.to_degrees(),
                        cat: Cat::Angle,
                    });
                }
                "atan2" => {
                    if parts.len() != 2 {
                        return None;
                    }
                    let (a, b) = (
                        calc_arg(&parts[0], vars, pct_scale, 0)?,
                        calc_arg(&parts[1], vars, pct_scale, 0)?,
                    );
                    let deg = |c: &Calc| match c.cat {
                        Cat::Angle => Some(c.v),
                        Cat::Number => Some(c.v),
                        _ => None,
                    };
                    return Some(Calc {
                        v: deg(&a)?.atan2(deg(&b)?).to_degrees(),
                        cat: Cat::Angle,
                    });
                }
                "pow" => {
                    if parts.len() != 2 {
                        return None;
                    }
                    let (a, b) = (
                        calc_arg(&parts[0], vars, pct_scale, 0)?,
                        calc_arg(&parts[1], vars, pct_scale, 0)?,
                    );
                    if a.cat != Cat::Number || b.cat != Cat::Number {
                        return Some(Calc {
                            v: f64::NAN,
                            cat: Cat::Other,
                        });
                    }
                    a.v.powf(b.v)
                }
                "sqrt" => {
                    let c = one(args)?;
                    if c.cat != Cat::Number {
                        return Some(Calc {
                            v: f64::NAN,
                            cat: Cat::Other,
                        });
                    }
                    c.v.max(0.0).sqrt()
                }
                "log" => {
                    let c = calc_arg(parts.first()?, vars, pct_scale, 0)?;
                    if c.cat != Cat::Number {
                        return Some(Calc {
                            v: f64::NAN,
                            cat: Cat::Other,
                        });
                    }
                    match parts.get(1) {
                        Some(base) => {
                            let b = calc_arg(base, vars, pct_scale, 0)?;
                            if b.cat != Cat::Number {
                                return Some(Calc {
                                    v: f64::NAN,
                                    cat: Cat::Other,
                                });
                            }
                            c.v.log(b.v)
                        }
                        None => c.v.ln(),
                    }
                }
                "exp" => {
                    let c = one(args)?;
                    if c.cat != Cat::Number {
                        return Some(Calc {
                            v: f64::NAN,
                            cat: Cat::Other,
                        });
                    }
                    c.v.exp()
                }
                _ => unreachable!(),
            };
            let cat = if matches!(name, "sin" | "cos" | "tan" | "pow" | "sqrt" | "log" | "exp") {
                Cat::Number
            } else {
                Cat::Angle
            };
            Some(Calc { v, cat })
        }
        _ => None,
    }
}

/// `f64` formatted the way WTF's `StringBuilder::AppendNumber` writes doubles
/// and floats (`%.6g`, matching `format_number` in asrs-tabs).
pub(crate) fn fmt_num(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let scientific = format!("{v:.5e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let trim = |s: &str| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            s.to_owned()
        }
    };
    if !(-4..6).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{}e{}{:02}", trim(mantissa), sign, exponent.unsigned_abs());
    }
    let decimals = usize::try_from(5 - exponent).unwrap_or(0);
    trim(&format!("{v:.decimals$}"))
}

/// One parsed channel of a colour function.
#[derive(Clone, Debug)]
pub(crate) enum Chan {
    None_,
    Number(f64),
    Percent(f64),
    /// A hue angle in degrees and its serialized literal (`10deg`, `0.5turn`).
    Angle(f64, String),
    /// A relative-colour channel variable (`r`, `h`, `alpha`, ...).
    Var(String),
    Calc(Calc),
}

#[derive(Clone, Copy, PartialEq)]
enum ChanType {
    None_,
    Number,
    Percentage,
    Relative,
}

/// A channel of a `lab()`/`oklab()`/`lch()`/`oklch()` that stayed unresolved
/// because of a `calc()`.
#[derive(Clone, Debug)]
pub enum UChan {
    None_,
    /// The literal: the value to serialise and whether it was a percentage.
    /// For an angle literal `deg` is `ComputeDegrees`, `raw` the unit value.
    Lit {
        raw: f64,
        pct: bool,
        deg: f64,
    },
    Calc(Calc, String),
}

/// `CSSUnresolvedColorValue`.
#[derive(Clone, Debug)]
pub struct Unresolved {
    pub space: Space,
    pub channels: [UChan; 3],
    /// `None` when no alpha was written, `Some(UChan::None_)` for `/ none`.
    pub alpha: Option<UChan>,
}

/// `CSSRelativeColorValue`, needed to resolve the colour for canvas.
#[derive(Clone, Debug)]
pub struct Rel {
    pub space: Space,
    pub origin: Node,
    pub(crate) channels: [Chan; 3],
    pub(crate) alpha: Option<Chan>,
}

/// `Color::HueInterpolationMethod`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HueMethod {
    Shorter,
    Longer,
    Increasing,
    Decreasing,
}

/// `CSSColorMixValue`.
#[derive(Clone, Debug)]
pub struct Mix {
    /// `None` when no `in <space>` argument was present (`kNone`).
    pub space: Option<Space>,
    pub method: HueMethod,
    pub c1: Node,
    pub c2: Node,
    /// The literal percentage (`30` for `30%`) or folded percent calc.
    pub p1: Option<f64>,
    pub p2: Option<f64>,
    /// Percentages that were calc(), serialised as `calc(...)` text.
    pub p1_calc: Option<String>,
    pub p2_calc: Option<String>,
}

/// A parsed paint or colour value for style serialization or canvas.
#[derive(Clone, Debug)]
pub enum Node {
    Color(Parsed),
    Unresolved(Unresolved),
    Relative(Box<Rel>),
    Mix(Box<Mix>),
    LightDark(Box<Node>, Box<Node>),
    /// A system colour or `currentcolor`: serializes as the keyword.
    Ident(&'static str),
    /// `url(...)` plus an optional `none`/colour fallback (paint only).
    Uri(String, Option<Box<Node>>),
    /// `none` as a paint fallback.
    NoneIdent,
    /// A pending-substitution value, serialized verbatim.
    Echo(String),
}

struct Cursor<'a> {
    toks: &'a [Tok],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.toks.get(self.i)
    }
    fn ident(&mut self, name: &str) -> bool {
        if matches!(self.peek(), Some(t) if t.ident(name)) {
            self.i += 1;
            return true;
        }
        false
    }
    fn comma(&mut self) -> bool {
        if matches!(self.peek(), Some(Tok::Comma)) {
            self.i += 1;
            return true;
        }
        false
    }
    fn at_end(&self) -> bool {
        self.i == self.toks.len()
    }
}

/// `ConsumeAngle`/`ConsumeNumber`/`ConsumePercent` for one channel, with the
/// relative-colour channel-variable forms. `pct_base` is the channel's
/// percentage base.
fn consume_channel(
    cur: &mut Cursor<'_>,
    space: Space,
    i: usize,
    relative: bool,
) -> Option<(Chan, ChanType)> {
    if cur.ident("none") {
        return Some((Chan::None_, ChanType::None_));
    }
    let tok = cur.peek()?.clone();
    let chan = match tok {
        Tok::Number(v) => Some((Chan::Number(v), ChanType::Number)),
        Tok::Dim(v, ref unit) => {
            if space.channel_is_hue(i) {
                angle_degrees(v, unit).map(|d| {
                    (
                        Chan::Angle(d, format!("{}{}", fmt_num(v), unit.to_ascii_lowercase())),
                        ChanType::Number,
                    )
                })
            } else {
                None
            }
        }
        Tok::Percent(v) if !space.channel_is_hue(i) => {
            Some((Chan::Percent(v), ChanType::Percentage))
        }
        Tok::Fn(ref name, ref args) if name.eq_ignore_ascii_case("calc") => {
            let mut n = 0;
            calc_sum(args, &mut n, &None, 0.0)
                .filter(|_| n == args.len())
                .and_then(|c| match c.cat {
                    Cat::Angle if space.channel_is_hue(i) => {
                        Some((Chan::Calc(c), ChanType::Number))
                    }
                    Cat::Number => Some((Chan::Calc(c), ChanType::Number)),
                    Cat::Percent if !space.channel_is_hue(i) => {
                        Some((Chan::Calc(c), ChanType::Percentage))
                    }
                    _ => None,
                })
        }
        Tok::Ident(name) if relative => {
            let n = name.to_ascii_lowercase();
            (space.channel_names().contains(&n.as_str()) || n == "alpha")
                .then_some((Chan::Var(n), ChanType::Relative))
        }
        _ => None,
    }?;
    cur.i += 1;
    Some(chan)
}

fn consume_alpha(cur: &mut Cursor<'_>, space: Space, relative: bool) -> Option<(Chan, ChanType)> {
    let tok = cur.peek()?.clone();
    let chan = match tok {
        Tok::Number(v) => (Chan::Number(v), ChanType::Number),
        Tok::Percent(v) => (Chan::Percent(v), ChanType::Percentage),
        Tok::Ident(name) if name.eq_ignore_ascii_case("none") => (Chan::None_, ChanType::None_),
        Tok::Ident(name) if relative => {
            let n = name.to_ascii_lowercase();
            if !(space.channel_names().contains(&n.as_str()) || n == "alpha") {
                return None;
            }
            (Chan::Var(n), ChanType::Relative)
        }
        Tok::Fn(ref name, ref args) if name.eq_ignore_ascii_case("calc") => {
            let mut n = 0;
            let c = calc_sum(args, &mut n, &None, 0.0).filter(|_| n == args.len())?;
            match c.cat {
                Cat::Number => (Chan::Calc(c), ChanType::Number),
                Cat::Percent => (Chan::Calc(c), ChanType::Percentage),
                _ => return None,
            }
        }
        _ => return None,
    };
    cur.i += 1;
    Some(chan)
}

/// `MakePerColorSpaceAdjustments`.
fn adjust_channels(
    space: Space,
    relative: bool,
    legacy: bool,
    channels: &mut [Option<f64>; 3],
    alpha: &mut Option<f64>,
) {
    for (i, ch) in channels.iter_mut().enumerate() {
        if let Some(v) = ch
            && space.channel_is_hue(i)
        {
            if !v.is_finite() {
                *v = 0.0;
            }
            *v = (*v % 360.0 + 360.0) % 360.0;
        }
    }
    if space == Space::SrgbLegacy {
        for v in channels.iter_mut().flatten() {
            if !v.is_finite() {
                *v = if *v > 0.0 { 255.0 } else { 0.0 };
            } else if !relative {
                *v = v.clamp(0.0, 255.0);
            }
        }
        if !relative && let Some(a) = alpha {
            *a = (*a * 255.0).round() / 255.0;
        }
    }
    if matches!(space, Space::Hsl | Space::Hwb) {
        for i in [1usize, 2] {
            if let Some(v) = &mut channels[i] {
                let mut val = *v / 100.0;
                if legacy {
                    val = val.clamp(0.0, 1.0);
                } else if !relative {
                    val = val.max(0.0);
                }
                channels[i] = Some(val);
            }
        }
    }
}

/// `ResolveColorChannel` for literal channels (percent -> `v/100*base`).
fn lit_channel(chan: &Chan, base: f64) -> Option<f64> {
    match chan {
        Chan::Number(v) | Chan::Angle(v, _) => Some(*v),
        Chan::Percent(v) => Some(v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0 * base),
        Chan::Calc(c) => match c.cat {
            Cat::Number | Cat::Angle => Some(c.v),
            Cat::Percent => {
                Some(c.v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0 * base)
            }
            _ => None,
        },
        _ => None,
    }
}

fn lit_alpha(chan: &Chan) -> Option<f64> {
    let v = match chan {
        Chan::Number(v) | Chan::Angle(v, _) => *v,
        Chan::Percent(v) => v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0,
        Chan::Calc(c) => match c.cat {
            Cat::Number => c.v,
            Cat::Percent => c.v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0,
            _ => return None,
        },
        _ => return None,
    };
    Some(v.clamp(0.0, 1.0))
}

/// `ChannelIsResolvable`: literals and, outside lab-family spaces, calcs of a
/// single literal (any calc that folded, which is all the parser accepts).
fn channel_resolvable(space: Space, chan: &Chan) -> bool {
    match chan {
        Chan::Calc(_) => !matches!(space, Space::Lab | Space::Oklab | Space::Lch | Space::Oklch),
        _ => true,
    }
}

fn to_uchan(chan: &Chan) -> UChan {
    match chan {
        Chan::None_ => UChan::None_,
        Chan::Number(v) => UChan::Lit {
            raw: *v,
            pct: false,
            deg: *v,
        },
        Chan::Percent(v) => UChan::Lit {
            raw: *v,
            pct: true,
            deg: *v,
        },
        Chan::Angle(d, _) => UChan::Lit {
            raw: *d,
            pct: false,
            deg: *d,
        },
        Chan::Calc(c) => UChan::Calc(c.clone(), c.text()),
        Chan::Var(_name) => UChan::Lit {
            raw: 0.0,
            pct: false,
            deg: 0.0,
        },
    }
}

/// `ConsumeFunctionalSyntaxColor` for one function token's arguments, `space`
/// already resolved from the function name or `color()`'s space argument.
fn functional_color(args: &[Tok], space: Space) -> Option<Node> {
    let mut cur = Cursor { toks: args, i: 0 };
    let origin = if cur.ident("from") {
        Some(consume_color(&mut cur, true)?)
    } else {
        None
    };
    functional_channels(&args[cur.i..], space, origin)
}

/// `ConsumeFunctionalSyntaxColor` for one function token's channel arguments,
/// `space` resolved from the function name or `color()`'s space argument and
/// `origin` the colour after `from` for relative colours.
fn functional_channels(args: &[Tok], space: Space, origin: Option<Node>) -> Option<Node> {
    let mut cur = Cursor { toks: args, i: 0 };
    let relative = origin.is_some();
    let mut channels: [Chan; 3] = [Chan::None_, Chan::None_, Chan::None_];
    let mut types = [ChanType::None_; 3];
    let mut legacy = false;
    let mut has_none = false;
    for (i, ch) in channels.iter_mut().enumerate() {
        let (chan, ty) = consume_channel(&mut cur, space, i, relative)?;
        has_none |= ty == ChanType::None_;
        *ch = chan;
        types[i] = ty;
        if i < 2 {
            let comma = cur.comma();
            if legacy {
                if !comma {
                    return None;
                }
            } else if comma {
                if relative {
                    return None;
                }
                legacy = true;
            }
        }
    }
    let mut alpha = None;
    let mut alpha_type = ChanType::None_;
    let has_alpha = if legacy {
        if !space.is_legacy() {
            return None;
        }
        cur.comma()
    } else {
        if matches!(cur.peek(), Some(Tok::Delim(b'/'))) {
            cur.i += 1;
            true
        } else {
            false
        }
    };
    if has_alpha {
        let (a, t) = consume_alpha(&mut cur, space, relative)?;
        has_none |= t == ChanType::None_;
        alpha = Some(a);
        alpha_type = t;
    }
    if !cur.at_end() {
        return None;
    }
    if legacy {
        if has_none || space == Space::Hwb {
            return None;
        }
        if space == Space::SrgbLegacy {
            let mut pct = false;
            let mut num = false;
            for t in types {
                match t {
                    ChanType::None_ => {}
                    ChanType::Percentage => {
                        if num {
                            return None;
                        }
                        pct = true;
                    }
                    _ => {
                        if pct {
                            return None;
                        }
                        num = true;
                    }
                }
            }
        }
        if matches!(space, Space::Hsl | Space::Hwb) && types[1..].contains(&ChanType::Number) {
            return None;
        }
    }

    if relative {
        return Some(Node::Relative(Box::new(Rel {
            space,
            origin: origin.unwrap(),
            channels,
            alpha: if has_alpha { alpha } else { None },
        })));
    }

    if channels.iter().all(|c| channel_resolvable(space, c))
        && alpha.as_ref().is_none_or(|a| channel_resolvable(space, a))
    {
        let bases = space.channel_percentage();
        let mut chans: [Option<f64>; 3] = [None; 3];
        for i in 0..3 {
            if types[i] != ChanType::None_ {
                chans[i] =
                    lit_channel(&channels[i], if bases[i].is_nan() { 1.0 } else { bases[i] });
            }
        }
        let mut a: Option<f64> = Some(1.0);
        if has_alpha {
            a = if alpha_type == ChanType::None_ {
                None
            } else {
                lit_alpha(alpha.as_ref().unwrap())
            };
        }
        adjust_channels(space, false, legacy, &mut chans, &mut a);
        return Some(Node::Color(Parsed::from_space(space, chans, a)));
    }

    Some(Node::Unresolved(Unresolved {
        space,
        channels: channels.map(|c| to_uchan(&c)),
        alpha: if has_alpha {
            alpha.as_ref().map(to_uchan)
        } else {
            None
        },
    }))
}

/// `ColorSpaceFromFunctionName` for `rgb()`/`hsl()`/...; `None` when the name
/// is `color`, which supplies its space as an argument.
fn fn_space(name: &str) -> Option<Space> {
    Some(match name {
        "rgb" | "rgba" => Space::SrgbLegacy,
        "hsl" | "hsla" => Space::Hsl,
        "hwb" => Space::Hwb,
        "lab" => Space::Lab,
        "oklab" => Space::Oklab,
        "lch" => Space::Lch,
        "oklch" => Space::Oklch,
        _ => return None,
    })
}

fn predefined_space(name: &str) -> Option<Space> {
    Some(match name {
        "srgb" => Space::Srgb,
        "rec2020" => Space::Rec2020,
        "srgb-linear" => Space::SrgbLinear,
        "display-p3" => Space::DisplayP3,
        "a98-rgb" => Space::A98Rgb,
        "prophoto-rgb" => Space::ProPhoto,
        "xyz" | "xyz-d65" => Space::XyzD65,
        "xyz-d50" => Space::XyzD50,
        _ => return None,
    })
}

/// `ConsumeColorMixFunction`.
fn consume_mix(args: &[Tok]) -> Option<Node> {
    let mut cur = Cursor { toks: args, i: 0 };
    let mut space = None;
    let mut method = HueMethod::Shorter;
    if cur.ident("in") {
        let name = match cur.peek()? {
            Tok::Ident(n) => n.clone(),
            _ => return None,
        };
        let s = match name.to_ascii_lowercase().as_str() {
            "xyz" => Some(Space::XyzD65),
            n => fn_space(n)
                .or_else(|| predefined_space(n))
                .filter(|s| *s != Space::SrgbLegacy),
        }?;
        cur.i += 1;
        space = Some(s);
        if matches!(s, Space::Hsl | Space::Hwb | Space::Lch | Space::Oklch)
            && let Some(Tok::Ident(m)) = cur.peek()
        {
            let m = match m.to_ascii_lowercase().as_str() {
                "shorter" => Some(HueMethod::Shorter),
                "longer" => Some(HueMethod::Longer),
                "decreasing" => Some(HueMethod::Decreasing),
                "increasing" => Some(HueMethod::Increasing),
                _ => None,
            };
            if let Some(m) = m {
                cur.i += 1;
                if !cur.ident("hue") {
                    return None;
                }
                method = m;
            }
        }
    }
    if !cur.comma() {
        return None;
    }
    let pct = |cur: &mut Cursor<'_>| -> Option<Option<(f64, Option<String>)>> {
        let tok = cur.peek().cloned();
        Some(match tok {
            Some(Tok::Percent(v)) => {
                cur.i += 1;
                Some((v, None))
            }
            Some(Tok::Fn(name, args2)) if name.eq_ignore_ascii_case("calc") => {
                let mut n = 0;
                let c = calc_sum(&args2, &mut n, &None, 0.0).filter(|_| n == args2.len())?;
                if c.cat != Cat::Percent {
                    return None;
                }
                cur.i += 1;
                Some((c.v, Some(c.text())))
            }
            _ => None,
        })
    };
    let mut c1 = consume_color(&mut cur, true);
    let p1 = pct(&mut cur)?;
    if c1.is_none() {
        c1 = Some(consume_color(&mut cur, true)?);
    }
    if let Some((v, None)) = p1
        && !(0.0..=100.0).contains(&v)
    {
        return None;
    }
    if !cur.comma() {
        return None;
    }
    let mut c2 = consume_color(&mut cur, true);
    let p2 = pct(&mut cur)?;
    if c2.is_none() {
        c2 = Some(consume_color(&mut cur, true)?);
    }
    if let Some((v, None)) = p2
        && !(0.0..=100.0).contains(&v)
    {
        return None;
    }
    if p1.as_ref().is_some_and(|t| t.0 == 0.0 && t.1.is_none())
        && p2.as_ref().is_some_and(|t| t.0 == 0.0 && t.1.is_none())
    {
        return None;
    }
    if !cur.at_end() {
        return None;
    }
    Some(Node::Mix(Box::new(Mix {
        space,
        method,
        c1: c1?,
        c2: c2?,
        p1: p1.as_ref().map(|p| p.0),
        p2: p2.as_ref().map(|p| p.0),
        p1_calc: p1.and_then(|p| p.1),
        p2_calc: p2.and_then(|p| p.1),
    })))
}

/// `ConsumeColor` (`ConsumeColorInternal`): keywords, `#` hex, functional
/// colours, `color-mix` and `light-dark`.
fn consume_color(cur: &mut Cursor<'_>, _nested: bool) -> Option<Node> {
    match cur.peek()? {
        Tok::Fn(name, args) => {
            let lower = name.to_ascii_lowercase();
            match lower.as_str() {
                "color-mix" => {
                    let node = consume_mix(args)?;
                    cur.i += 1;
                    Some(node)
                }
                "light-dark" => {
                    let mut inner = Cursor { toks: args, i: 0 };
                    let c1 = consume_color(&mut inner, true)?;
                    if !inner.comma() {
                        return None;
                    }
                    let c2 = consume_color(&mut inner, true)?;
                    if !inner.at_end() {
                        return None;
                    }
                    cur.i += 1;
                    Some(Node::LightDark(Box::new(c1), Box::new(c2)))
                }
                "color" => {
                    // Optional `from <color>` before the space argument.
                    let mut inner = Cursor { toks: args, i: 0 };
                    let origin = if inner.ident("from") {
                        Some(consume_color(&mut inner, true)?)
                    } else {
                        None
                    };
                    let sname = match inner.peek()? {
                        Tok::Ident(n) => n.clone(),
                        _ => return None,
                    };
                    let space = predefined_space(&sname.to_ascii_lowercase())?;
                    inner.i += 1;
                    let node = functional_channels(&args[inner.i..], space, origin)?;
                    cur.i += 1;
                    Some(node)
                }
                _ => match fn_space(&lower) {
                    Some(space) => {
                        let node = functional_color(args, space)?;
                        cur.i += 1;
                        Some(node)
                    }
                    None => None,
                },
            }
        }
        Tok::Hash(digits) => {
            let parsed = parse_hex(digits)?;
            cur.i += 1;
            Some(Node::Color(parsed))
        }
        Tok::Ident(name) => {
            let lower = name.to_ascii_lowercase();
            let keyword = if lower == "transparent" {
                Some("transparent")
            } else if NAMED
                .binary_search_by(|(n, _)| n.cmp(&lower.as_str()))
                .is_ok()
            {
                Some(
                    NAMED[NAMED
                        .binary_search_by(|(n, _)| n.cmp(&lower.as_str()))
                        .unwrap()]
                    .0,
                )
            } else {
                COLOR_KEYWORDS.iter().copied().find(|k| *k == lower)
            }?;
            cur.i += 1;
            Some(Node::Ident(keyword))
        }
        _ => None,
    }
}

/// `ConsumeUrl`: `url(...)` as a token or `url("...")` as a function.
fn consume_url(cur: &mut Cursor<'_>) -> Option<String> {
    match cur.peek()? {
        Tok::Url(v) => {
            cur.i += 1;
            Some(v.clone())
        }
        Tok::Fn(name, args) if name.eq_ignore_ascii_case("url") => match args.as_slice() {
            [Tok::Str(v)] => {
                cur.i += 1;
                Some(v.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

/// `ConsumeSVGPaint` (paint = `stroke`/`fill`) or `ConsumeColor` for the whole
/// value. `Slow::Valid(node)` parses; `Slow::Invalid` is what Blink rejects;
/// `Slow::Unmodeled` is `Err` upstream. A pending-substitution value echoes.
pub fn parse_style_value(value: &str, paint: bool) -> Slow<Node> {
    let trimmed = trim_css(value);
    let toks = tokenize(trimmed);
    if toks.is_empty() {
        return Slow::Invalid;
    }
    if has_substitution(&toks) {
        // Pending-substitution values (`var()` &co.) echo verbatim upstream;
        // resolving them needs element context this port does not have.
        return Slow::Unmodeled;
    }
    let mut cur = Cursor { toks: &toks, i: 0 };
    let node = if paint {
        match cur.peek() {
            Some(Tok::Ident(n))
                if matches!(
                    n.to_ascii_lowercase().as_str(),
                    "none" | "context-fill" | "context-stroke"
                ) =>
            {
                cur.i += 1;
                if !cur.at_end() {
                    return Slow::Invalid;
                }
                return Slow::Valid(match n.to_ascii_lowercase().as_str() {
                    "none" => Node::NoneIdent,
                    name => Node::Ident(match name {
                        "context-fill" => "context-fill",
                        _ => "context-stroke",
                    }),
                });
            }
            _ => {
                if let Some(url) = consume_url(&mut cur) {
                    let fallback = if cur.at_end() {
                        None
                    } else if cur.ident("none") {
                        if !cur.at_end() {
                            return Slow::Invalid;
                        }
                        Some(Node::NoneIdent)
                    } else {
                        match consume_color(&mut cur, true) {
                            Some(c) if cur.at_end() => Some(c),
                            _ => return Slow::Invalid,
                        }
                    };
                    Node::Uri(url, fallback.map(Box::new))
                } else {
                    match consume_color(&mut cur, false) {
                        Some(n) if cur.at_end() => n,
                        _ => return Slow::Invalid,
                    }
                }
            }
        }
    } else {
        match consume_color(&mut cur, false) {
            Some(n) if cur.at_end() => n,
            _ => return Slow::Invalid,
        }
    };
    if matches!(node, Node::Relative(_)) {
        // `color-mix()` and `light-dark()` serialize in this port; a top-level
        // relative colour would need its origin resolved to say anything more.
        return Slow::Unmodeled;
    }
    Slow::Valid(node)
}

fn trim_css(s: &str) -> &str {
    s.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
}

/// `ResolveNonFiniteValues`.
fn resolve_non_finite(c: &mut Parsed) {
    let non_finite = |v: f32, lo: f32, hi: f32| {
        if v.is_nan() {
            v
        } else if v == f32::INFINITY {
            hi
        } else if v == f32::NEG_INFINITY {
            lo
        } else {
            v
        }
    };
    if c.space.lightness_first() {
        c.p[0] = non_finite(c.p[0], 0.0, 100.0);
    }
    if c.space.chroma_second() && c.p[1] == f32::NEG_INFINITY {
        c.p[1] = 0.0;
    }
    if c.space == Space::SrgbLegacy {
        for p in &mut c.p {
            *p = non_finite(*p, 0.0, 1.0);
        }
    }
    for p in &mut c.p {
        if p.is_nan() {
            *p = 0.0;
        }
    }
    c.a = non_finite(c.a, 0.0, 1.0);
}

/// `blink::Blend`.
fn blend(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn hue_interp(a: f32, b: f32, t: f32, method: HueMethod) -> f32 {
    let (mut v1, mut v2) = (a, b);
    match method {
        HueMethod::Shorter => {
            let diff = v2 - v1;
            if diff > 180.0 {
                v1 += 360.0;
            } else if diff < -180.0 {
                v2 += 360.0;
            }
        }
        HueMethod::Longer => {
            let diff = v2 - v1;
            if diff > 0.0 && diff < 180.0 {
                v1 += 360.0;
            } else if diff > -180.0 && diff <= 0.0 {
                v2 += 360.0;
            }
        }
        HueMethod::Increasing => {
            if v2 < v1 {
                v2 += 360.0;
            }
        }
        HueMethod::Decreasing => {
            if v1 < v2 {
                v1 += 360.0;
            }
        }
    }
    let v = blend(v1, v2, t) % 360.0;
    if v < 0.0 { v + 360.0 } else { v }
}

/// `Color::GetAnalogousMissingComponents` + carry-forward, for interpolation.
fn analogous_missing(from: &Parsed, dest: Space) -> [bool; 3] {
    let rgb_or_xyz = |s: Space| {
        matches!(
            s,
            Space::Srgb
                | Space::SrgbLinear
                | Space::DisplayP3
                | Space::A98Rgb
                | Space::ProPhoto
                | Space::Rec2020
                | Space::XyzD50
                | Space::XyzD65
                | Space::SrgbLegacy
        )
    };
    let is_lab = |s: Space| matches!(s, Space::Lab | Space::Oklab);
    let is_lch = |s: Space| matches!(s, Space::Lch | Space::Oklch);
    let n = from.none;
    match from.space {
        s if rgb_or_xyz(s) && rgb_or_xyz(dest) => n,
        s if is_lab(s) => {
            if is_lab(dest) {
                n
            } else if is_lch(dest) {
                [n[0], false, false]
            } else if dest == Space::Hsl {
                [false, false, n[0]]
            } else {
                [false; 3]
            }
        }
        s if is_lch(s) => {
            if is_lch(dest) {
                n
            } else if is_lab(dest) {
                [n[0], false, false]
            } else if dest == Space::Hsl {
                [false, false, n[0]]
            } else {
                [false; 3]
            }
        }
        Space::Hsl | Space::Hwb => {
            if is_lab(dest) || is_lch(dest) {
                [false, false, n[2]]
            } else if dest == Space::Hsl || dest == Space::Hwb {
                n
            } else {
                [false; 3]
            }
        }
        _ => [false; 3],
    }
}

fn convert_for_interpolation(c: &mut Parsed, dest: Space) {
    if c.space == dest {
        return;
    }
    let analogous = analogous_missing(c, dest);
    conversions::convert(c, dest);
    for i in 0..3 {
        if analogous[i] {
            c.p[i] = 0.0;
            c.none[i] = true;
        }
    }
}

fn substitute_missing(c1: &mut Parsed, c2: &mut Parsed) {
    for i in 0..3 {
        if c1.none[i] && !c2.none[i] {
            c1.p[i] = c2.p[i];
            c1.none[i] = false;
        } else if c2.none[i] && !c1.none[i] {
            c2.p[i] = c1.p[i];
            c2.none[i] = false;
        }
    }
    if c1.a_none && !c2.a_none {
        c1.a = c2.a;
        c1.a_none = false;
    } else if c2.a_none && !c1.a_none {
        c2.a = c1.a;
        c2.a_none = false;
    }
}

/// `Color::InterpolateColors` + `FromColorMix`.
fn interpolate(space: Space, method: HueMethod, mut c1: Parsed, mut c2: Parsed, t: f32) -> Parsed {
    convert_for_interpolation(&mut c1, space);
    convert_for_interpolation(&mut c2, space);
    substitute_missing(&mut c1, &mut c2);
    let premult = |c: &mut Parsed| -> f32 {
        if c.a_none {
            return c.a;
        }
        let a = c.a;
        if c.space != Space::Hsl && c.space != Space::Hwb {
            c.p[0] *= a;
        }
        c.p[1] *= a;
        if !c.space.chroma_second() {
            c.p[2] *= a;
        }
        c.a = 1.0;
        a
    };
    let a1 = premult(&mut c1);
    let a2 = premult(&mut c2);
    let opt = |n1: bool, n2: bool, v: f32| {
        if n1 && n2 { None } else { Some(v) }
    };
    let p0 = opt(
        c1.none[0],
        c2.none[0],
        if matches!(space, Space::Hsl | Space::Hwb) {
            hue_interp(c1.p[0], c2.p[0], t, method)
        } else {
            blend(c1.p[0], c2.p[0], t)
        },
    );
    let p1 = opt(c1.none[1], c2.none[1], blend(c1.p[1], c2.p[1], t));
    let p2 = opt(
        c1.none[2],
        c2.none[2],
        if space.chroma_second() {
            hue_interp(c1.p[2], c2.p[2], t, method)
        } else {
            blend(c1.p[2], c2.p[2], t)
        },
    );
    let alpha = opt(c1.a_none, c2.a_none, blend(a1, a2, t));
    let mut result = Parsed::from_space(
        space,
        [p0.map(f64::from), p1.map(f64::from), p2.map(f64::from)],
        alpha.map(f64::from),
    );
    // UnpremultiplyColor.
    if !result.a_none && result.a != 0.0 {
        if result.space != Space::Hsl && result.space != Space::Hwb {
            result.p[0] /= result.a;
        }
        result.p[1] /= result.a;
        if !result.space.chroma_second() {
            result.p[2] /= result.a;
        }
    }
    result
}

/// `CSSColorMixValue::Mix` + `NormalizePercentages`.
fn resolve_mix(mix: &Mix) -> Option<Parsed> {
    let space = mix.space.unwrap_or(Space::Oklab);
    let mut p1 = mix.p1.map_or(0.5, |v| v.clamp(0.0, 100.0) / 100.0);
    let mut p2 = mix.p2.map_or(0.5, |v| v.clamp(0.0, 100.0) / 100.0);
    if mix.p1.is_some() && mix.p2.is_none() {
        p2 = 1.0 - p1;
    } else if mix.p2.is_some() && mix.p1.is_none() {
        p1 = 1.0 - p2;
    }
    if p1 == 0.0 && p2 == 0.0 {
        return None;
    }
    let mut alpha_multiplier = 1.0f64;
    let scale = p1 + p2;
    if scale != 0.0 {
        p1 /= scale;
        p2 /= scale;
        if scale <= 1.0 {
            alpha_multiplier = scale;
        }
    }
    let mix_amount = if p1 == 0.0 { 1.0 } else { p2 };
    let c1 = resolve(&mix.c1)?;
    let c2 = resolve(&mix.c2)?;
    let mut result = interpolate(space, mix.method, c1, c2, mix_amount as f32);
    result.a *= alpha_multiplier as f32;
    if result.space.is_legacy() {
        conversions::convert(&mut result, Space::Srgb);
    }
    Some(result)
}

/// `UnresolvedRelativeColor::Resolve` (`from <color>` forms), for canvas.
fn resolve_relative(rel: &Rel) -> Option<Parsed> {
    let mut origin = resolve(&rel.origin)?;
    conversions::convert(&mut origin, rel.space);
    let names = rel.space.channel_names();
    let pct_bases = rel.space.channel_percentage();
    let mut map = std::collections::HashMap::new();
    for i in 0..3 {
        let mut v = f64::from(origin.p[i]);
        if matches!(rel.space, Space::Hsl | Space::Hwb) && i > 0 {
            v *= 100.0;
        }
        map.insert(names[i], v);
    }
    map.insert("alpha", f64::from(origin.a));
    let vars = |name: &str| map.get(name).copied();
    let eval = |chan: &Chan, base: f64| -> Option<Option<f64>> {
        Some(match chan {
            Chan::None_ => None,
            Chan::Number(v) | Chan::Angle(v, _) => Some(*v),
            Chan::Percent(v) => {
                Some(v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0 * base)
            }
            Chan::Var(name) => map.get(name.as_str()).copied(),

            Chan::Calc(c) => match c.cat {
                Cat::Number | Cat::Angle => Some(c.v),
                Cat::Percent => {
                    Some(c.v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0 * base)
                }
                _ => None,
            },
        })
    };
    let _ = vars;
    let mut chans: [Option<f64>; 3] = [None; 3];
    for i in 0..3 {
        let base = pct_bases[i];
        let base = if base.is_nan() {
            f64::from(f32::MAX)
        } else {
            base
        };
        chans[i] = eval(&rel.channels[i], base)?;
    }
    let mut alpha = match &rel.alpha {
        Some(a) => eval(a, 1.0)?,
        None => map.get("alpha").copied(),
    };
    adjust_channels(rel.space, true, false, &mut chans, &mut alpha);
    let mut c = Parsed::from_space(rel.space, chans, alpha);
    if rel.space.is_legacy() {
        conversions::convert(&mut c, Space::Srgb);
    }
    Some(c)
}

/// `CSSUnresolvedColorValue::Resolve`, for canvas.
fn resolve_unresolved(u: &Unresolved) -> Parsed {
    let bases = u.space.channel_percentage();
    let value = |c: &UChan, base: f64| -> Option<f64> {
        Some(match c {
            UChan::None_ => return None,
            UChan::Lit { raw, pct, deg } => {
                if *pct {
                    raw.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0 * base
                } else {
                    *deg
                }
            }
            UChan::Calc(c, _) => match c.cat {
                Cat::Number | Cat::Angle => c.v,
                Cat::Percent => c.v.clamp(f64::from(-f32::MAX), f64::from(f32::MAX)) / 100.0 * base,
                _ => return None,
            },
        })
    };
    let mut chans: [Option<f64>; 3] = [None; 3];
    for i in 0..3 {
        chans[i] = value(
            &u.channels[i],
            if bases[i].is_nan() { 1.0 } else { bases[i] },
        );
    }
    let mut alpha = match &u.alpha {
        Some(c) => value(c, 1.0).map(|v| v.clamp(0.0, 1.0)),
        None => None,
    };
    adjust_channels(u.space, false, false, &mut chans, &mut alpha);
    let mut c = Parsed::from_space(u.space, chans, alpha);
    if c.space.is_legacy() {
        conversions::convert(&mut c, Space::Srgb);
    }
    c
}

/// The `Color` a parsed node resolves to for canvas (`ResolveColor`,
/// `TryResolveAtParseTime` for relative colours and colour-mix).
pub fn resolve(node: &Node) -> Option<Parsed> {
    let mut c = match node {
        Node::Color(c) => *c,
        Node::Unresolved(u) => resolve_unresolved(u),
        Node::Relative(r) => resolve_relative(r)?,
        Node::Mix(m) => resolve_mix(m)?,
        Node::Ident(name) => {
            if *name == "transparent" {
                Parsed::legacy([0.0; 3], 0.0)
            } else {
                let rgb = named(name)?;
                Parsed::legacy(rgb.map(f32::from), 1.0)
            }
        }
        Node::LightDark(..) | Node::Uri(..) | Node::NoneIdent | Node::Echo(_) => {
            return None;
        }
    };
    resolve_non_finite(&mut c);
    Some(c)
}

/// `ColorParamToString`.
fn param_str(v: f32) -> String {
    if v.is_nan() {
        return "calc(nan)".to_owned();
    }
    if v == f32::INFINITY {
        return "calc(infinity)".to_owned();
    }
    if v == f32::NEG_INFINITY {
        return "calc(-infinity)".to_owned();
    }
    fmt_num(f64::from(v))
}

/// `ProcessChannelValue`: per-space clamping and percentage conversion of a
/// numeric literal channel in an unresolved colour's serialization.
fn processed_channel(space: Space, i: usize, v: f64, pct: bool) -> f64 {
    match space {
        Space::Hsl => {
            if i == 1 {
                v.max(0.0)
            } else {
                v
            }
        }
        Space::Lab => {
            let v = if i == 0 { v.clamp(0.0, 100.0) } else { v };
            if pct && i > 0 { v * 125.0 / 100.0 } else { v }
        }
        Space::Oklab => {
            let v = if i == 0 { v.clamp(0.0, 1.0) } else { v };
            if pct {
                if i == 0 { v / 100.0 } else { v * 0.4 / 100.0 }
            } else {
                v
            }
        }
        Space::Lch => {
            let v = match i {
                0 => v.clamp(0.0, 100.0),
                1 => v.max(0.0),
                _ => v,
            };
            if pct && i == 1 { v * 150.0 / 100.0 } else { v }
        }
        Space::Oklch => {
            let v = match i {
                0 => v.clamp(0.0, 1.0),
                1 => v.max(0.0),
                _ => v,
            };
            if pct {
                if i == 0 {
                    v / 100.0
                } else if i == 1 {
                    v * 0.4 / 100.0
                } else {
                    v
                }
            } else {
                v
            }
        }
        Space::Srgb | Space::SrgbLegacy => {
            let v = if pct { v * 255.0 / 100.0 } else { v };
            v.clamp(0.0, 255.0)
        }
        _ => v,
    }
}

/// The literal value an unresolved alpha serializes through, when it is one.
fn unresolved_alpha_value(space: Space, chan: &UChan) -> Option<f64> {
    match chan {
        UChan::Lit { raw, pct, .. } => {
            let v = raw.clamp(f64::from(-f32::MAX), f64::from(f32::MAX));
            Some(if *pct { v / 100.0 } else { v })
        }
        UChan::Calc(c, _) if space == Space::SrgbLegacy => Some(if c.cat == Cat::Percent {
            c.v / 100.0
        } else {
            c.v
        }),
        _ => None,
    }
}

/// `value->CssText()` for a channel that does not serialize through a number.
fn uchan_css(chan: &UChan) -> String {
    match chan {
        UChan::None_ => "none".to_owned(),
        UChan::Lit { raw, pct, .. } => {
            if *pct {
                format!("{}%", fmt_num(*raw))
            } else {
                fmt_num(*raw)
            }
        }
        UChan::Calc(_, text) => text.clone(),
    }
}

/// `CSSUnresolvedColorValue::CustomCSSText`.
fn serialize_unresolved(u: &Unresolved) -> String {
    let mut out = String::new();
    if u.space.is_predefined() {
        out.push_str("color(");
        out.push_str(u.space.name());
        out.push(' ');
    } else {
        out.push_str(u.space.name());
        out.push('(');
    }
    for (i, ch) in u.channels.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        match ch {
            UChan::None_ => out.push_str("none"),
            UChan::Lit { raw, pct, .. } => {
                out.push_str(&fmt_num(processed_channel(u.space, i, *raw, *pct)))
            }
            UChan::Calc(c, text) => {
                if u.space == Space::SrgbLegacy {
                    out.push_str(&fmt_num(c.v.clamp(0.0, 255.0)))
                } else {
                    out.push_str(text)
                }
            }
        }
    }
    match &u.alpha {
        None => {}
        Some(UChan::None_) => out.push_str(" / none"),
        Some(a) => {
            let lit = unresolved_alpha_value(u.space, a);
            if lit.is_none_or(|v| !v.is_finite() || v < 1.0) {
                out.push_str(" / ");
                match lit {
                    Some(v) => out.push_str(&fmt_num(v.clamp(0.0, 1.0))),
                    None => out.push_str(&uchan_css(a)),
                }
            }
        }
    }
    out.push(')');
    out
}

/// `channel->CssText()` inside a `CSSRelativeColorValue`.
fn rel_chan_text(chan: &Chan) -> String {
    match chan {
        Chan::None_ => "none".to_owned(),
        Chan::Number(v) => fmt_num(*v),
        Chan::Percent(v) => format!("{}%", fmt_num(*v)),
        Chan::Angle(_, text) => text.clone(),
        Chan::Var(name) => name.clone(),
        Chan::Calc(c) => c.text(),
    }
}

/// `CSSRelativeColorValue::CustomCSSText`.
fn serialize_relative(rel: &Rel) -> String {
    let mut out = String::new();
    if rel.space.is_predefined() {
        out.push_str("color(from ");
    } else {
        out.push_str(rel.space.name());
        out.push_str("(from ");
    }
    out.push_str(&serialize_css(&rel.origin));
    out.push(' ');
    if rel.space.is_predefined() {
        out.push_str(rel.space.name());
        out.push(' ');
    }
    for (i, ch) in rel.channels.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&rel_chan_text(ch));
    }
    if let Some(a) = &rel.alpha {
        out.push_str(" / ");
        out.push_str(&rel_chan_text(a));
    }
    out.push(')');
    out
}

/// `Color::SerializeInterpolationSpace`.
fn serialize_interp_space(space: Space, method: HueMethod) -> String {
    let mut out = space.name().to_owned();
    if matches!(space, Space::Hsl | Space::Hwb | Space::Lch | Space::Oklch) {
        out.push_str(match method {
            HueMethod::Shorter => "",
            HueMethod::Longer => " longer hue",
            HueMethod::Increasing => " increasing hue",
            HueMethod::Decreasing => " decreasing hue",
        });
    }
    out
}

/// `CSSColorMixValue::CustomCSSText`.
fn serialize_mix(mix: &Mix) -> String {
    let pct = |v: f64| format!("{}%", fmt_num(v));
    let p2_text = |v: Option<f64>, calc: &Option<String>| match v {
        Some(v) => Some(pct(v)),
        None => calc.clone(),
    };
    let (s1, s2) = if let Some(v1) = mix.p1 {
        if let Some(v2) = mix.p2 {
            if v1 == 50.0 && v2 == 50.0 {
                (None, None)
            } else if v1 + v2 == 100.0 {
                (Some(pct(v1)), None)
            } else {
                (Some(pct(v1)), Some(pct(v2)))
            }
        } else if mix.p2_calc.is_none() && v1 == 50.0 {
            (None, None)
        } else {
            (Some(pct(v1)), mix.p2_calc.clone())
        }
    } else if let Some(c1) = &mix.p1_calc {
        (Some(c1.clone()), p2_text(mix.p2, &mix.p2_calc))
    } else if let Some(v2) = mix.p2 {
        if v2 == 50.0 {
            (None, None)
        } else {
            (Some(pct(100.0 - v2)), None)
        }
    } else {
        (None, mix.p2_calc.clone())
    };
    let mut out = format!(
        "color-mix(in {}, {}",
        serialize_interp_space(mix.space.unwrap_or(Space::Oklab), mix.method),
        serialize_css(&mix.c1)
    );
    if let Some(s) = s1 {
        out.push(' ');
        out.push_str(&s);
    }
    out.push_str(", ");
    out.push_str(&serialize_css(&mix.c2));
    if let Some(s) = s2 {
        out.push(' ');
        out.push_str(&s);
    }
    out.push(')');
    out
}

/// `units / 10^places` without trailing zeros, for `units` in `0..=10^places`.
fn decimal(units: u32, places: usize) -> String {
    let scale = 10u32.pow(places as u32);
    let fraction = format!("{:0places$}", units % scale);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{}", units / scale)
    } else {
        format!("{}.{fraction}", units / scale)
    }
}

/// `Color::SerializeLegacyColorAsCSSColor`.
fn serialize_legacy(c: &Parsed) -> String {
    let [r, g, b] = c.css_channels();
    if c.a >= 1.0 && c.a.is_finite() {
        return format!("rgb({r}, {g}, {b})");
    }
    let int_alpha = math::min2(math::round((f64::from(c.a) + 1e-7) * 255.0), 255.0);
    let hundredths = math::round(int_alpha * 100.0 / 255.0);
    let two_decimal = (hundredths / 100.0) as f32;
    let alpha = if math::round(f64::from(two_decimal * 255.0)) == int_alpha {
        decimal(hundredths as u32, 2)
    } else {
        decimal(math::round(int_alpha * 1000.0 / 255.0) as u32, 3)
    };
    format!("rgba({r}, {g}, {b}, {alpha})")
}

/// `Color::SerializeInternal`.
fn serialize_internal(c: &Parsed) -> String {
    let mut out = String::new();
    if c.space.lightness_first() {
        out.push_str(c.space.name());
        out.push('(');
    } else {
        out.push_str("color(");
        out.push_str(c.space.name());
        out.push(' ');
    }
    for i in 0..3 {
        if i > 0 {
            out.push(' ');
        }
        if c.none[i] {
            out.push_str("none");
        } else {
            out.push_str(&param_str(c.p[i]));
        }
    }
    if c.a != 1.0 || c.a_none {
        out.push_str(" / ");
        if c.a_none {
            out.push_str("none");
        } else {
            out.push_str(&fmt_num(f64::from(c.a)));
        }
    }
    out.push(')');
    out
}

/// `CSSURIValue` serialization.
fn serialize_uri(url: &str) -> String {
    let mut out = String::from("url(\"");
    for ch in url.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' => out.push_str("\\a "),
            '\r' => out.push_str("\\d "),
            '\x0c' => out.push_str("\\c "),
            '\0'..='\x08' | '\x0b' | '\x0e'..='\x1f' | '\x7f' => {
                out.push_str(&format!("\\{:x} ", ch as u32));
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out.push(')');
    out
}

/// `CustomCSSText` of a parsed colour value: `SerializeAsCSSColor` for resolved
/// colours and the unresolved/relative/mix/light-dark forms for the rest.
pub fn serialize_css(node: &Node) -> String {
    match node {
        Node::Color(c) => {
            if c.space.is_legacy() {
                serialize_legacy(c)
            } else {
                serialize_internal(c)
            }
        }
        Node::Unresolved(u) => serialize_unresolved(u),
        Node::Relative(r) => serialize_relative(r),
        Node::Mix(m) => serialize_mix(m),
        Node::LightDark(a, b) => format!("light-dark({}, {})", serialize_css(a), serialize_css(b)),
        Node::Ident(name) => (*name).to_owned(),
        Node::NoneIdent => "none".to_owned(),
        Node::Uri(url, fallback) => match fallback {
            Some(fb) => format!("{} {}", serialize_uri(url), serialize_css(fb)),
            None => serialize_uri(url),
        },
        Node::Echo(text) => text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u8s(css: &str) -> Option<([u8; 3], f32)> {
        parse(css).map(|c| (c.to_u8(), c.a))
    }

    #[test]
    fn named_table_is_sorted() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn parses_the_scene_styles() {
        assert_eq!(u8s("#f00"), Some(([255, 0, 0], 1.0)));
        assert_eq!(u8s("#2B66A2"), Some(([0x2b, 0x66, 0xa2], 1.0)));
        assert_eq!(u8s("#2b66a280"), Some(([0x2b, 0x66, 0xa2], 128.0 / 255.0)));
        assert_eq!(u8s("Yellow"), Some(([255, 255, 0], 1.0)));
        assert_eq!(u8s("rgb(43, 102, 162)"), Some(([43, 102, 162], 1.0)));
        assert_eq!(u8s("rgba(0,0,0,0)"), Some(([0, 0, 0], 0.0)));
        assert_eq!(
            u8s("rgba(228,139,50,0.25)"),
            Some(([228, 139, 50], 64.0 / 255.0))
        );
        assert_eq!(
            u8s("rgb(10 20 30 / 50%)"),
            Some(([10, 20, 30], 128.0 / 255.0))
        );
        assert_eq!(u8s("hsl(120, 50%, 50%)"), Some(([64, 191, 64], 1.0)));
        assert_eq!(u8s("transparent"), Some(([0, 0, 0], 0.0)));
        assert_eq!(u8s("not a color"), None);
        assert_eq!(u8s("#12345"), None);
        assert_eq!(u8s("rgb(1, 2)"), None);
    }

    #[test]
    fn percentage_channels_scale_like_blink() {
        assert_eq!(u8s("rgb(100%, 50%, 0%)"), Some(([255, 128, 0], 1.0)));
    }

    #[test]
    fn closes_a_function_left_open_like_blink() {
        let black = Some(([0, 0, 0], 1.0));
        assert_eq!(u8s("rgb(0 0 0 / 10%"), Some(([0, 0, 0], 26.0 / 255.0)));
        assert_eq!(u8s("rgb(0 0 0 / 10%\t"), Some(([0, 0, 0], 26.0 / 255.0)));
        assert_eq!(u8s("rgb(0,0,0"), black);
        assert_eq!(u8s("rgb(0 0 0 "), black);
        assert_eq!(u8s(" RGB(1 2 3"), Some(([1, 2, 3], 1.0)));
        assert_eq!(u8s("rgb(1 2 3 / 50"), Some(([1, 2, 3], 1.0)));
        assert_eq!(u8s("rgb(1 2 3 / .5"), Some(([1, 2, 3], 128.0 / 255.0)));
        assert_eq!(u8s("rgba(1,2,3,0.5"), Some(([1, 2, 3], 128.0 / 255.0)));
        assert_eq!(u8s("rgb(10% 20% 30%"), Some(([26, 51, 77], 1.0)));
        assert_eq!(u8s("hsl(120, 50%, 50%"), Some(([64, 191, 64], 1.0)));
        assert_eq!(u8s("hsl(120 50% 50% / 0.5"), Some(([64, 191, 64], 0.5)));
        assert_eq!(u8s("hsla(120,50%,50%,0.5"), Some(([64, 191, 64], 0.5)));
        for invalid in [
            "rgb(0 0 0 /",
            "rgb(0,0,0,",
            "rgb(",
            "rgb(0 0",
            "rgb(0 0 0)x",
            "rgb(0 0 0))",
            "#fff(",
            "red(",
            "undefined",
        ] {
            assert_eq!(u8s(invalid), None, "{invalid}");
        }
    }
}
