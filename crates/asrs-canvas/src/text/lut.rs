// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2012 Google Inc.). See THIRD_PARTY_NOTICES.md.

use asrs_js::math::pow;

pub const ROWS: usize = 8;
pub const BYTES: usize = ROWS * 256;

/// Skia's mask-gamma preblend as measured from Chromium: one coverage table per 3-bit luminance
/// of the fill colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskGammaLut(Box<[[u8; 256]; ROWS]>);

impl MaskGammaLut {
    pub fn identity() -> Self {
        let mut rows = Box::new([[0u8; 256]; ROWS]);
        for row in rows.iter_mut() {
            for (i, v) in row.iter_mut().enumerate() {
                *v = i as u8;
            }
        }
        MaskGammaLut(rows)
    }

    pub fn from_rows(rows: [[u8; 256]; ROWS]) -> Self {
        MaskGammaLut(Box::new(rows))
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != BYTES {
            return None;
        }
        let mut rows = Box::new([[0u8; 256]; ROWS]);
        for (row, chunk) in rows.iter_mut().zip(bytes.as_chunks::<256>().0) {
            row.copy_from_slice(chunk);
        }
        Some(MaskGammaLut(rows))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.iter().flatten().copied().collect()
    }

    /// `SK_GAMMA_CONTRAST` of Chromium's Windows Skia. Electron leaves the contrast and gamma of
    /// the ClearType tuner unused: its renderers draw with these whatever the registry holds.
    pub const WINDOWS_CONTRAST: f32 = 1.0;
    /// `SK_GAMMA_SRGB`.
    pub const WINDOWS_GAMMA: f32 = 0.0;

    /// Skia's `SkMaskGamma` tables; a device gamma of 0 is sRGB.
    pub fn skia(contrast: f32, gamma: f32) -> Self {
        let contrast = f64::from((contrast * 255.0 + 0.5) as u8) / 255.0;
        let gamma = f64::from((gamma * 64.0) as u8) / 64.0;
        let mut rows = [[0u8; 256]; ROWS];
        for (bucket, row) in rows.iter_mut().enumerate() {
            *row = correcting_row(bucket, contrast, gamma);
        }
        MaskGammaLut::from_rows(rows)
    }

    pub fn row(&self, bucket: usize) -> &[u8; 256] {
        &self.0[bucket]
    }

    pub fn apply(&self, rgb: [u8; 3], coverage: u8) -> u8 {
        self.0[luminance_bucket(rgb)][coverage as usize]
    }

    /// The table committed for the platform this was built for, if it has one.
    pub fn embedded() -> Option<Self> {
        #[cfg(target_os = "macos")]
        return MaskGammaLut::from_bytes(include_bytes!(
            "../../../../parity/vectors/canvas/lut-macos.bin"
        ));
        #[cfg(target_os = "linux")]
        return MaskGammaLut::from_bytes(include_bytes!(
            "../../../../parity/vectors/canvas/lut-linux.bin"
        ));
        #[cfg(windows)]
        return MaskGammaLut::from_bytes(include_bytes!(
            "../../../../parity/vectors/canvas/lut-windows.bin"
        ));
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        return None;
    }
}

pub const LCD_BYTES: usize = 3 * BYTES;

/// The preblend of LCD text as measured from Chromium: coverage tables per colour channel, of
/// which a channel uses the one of its own 3-bit value in the text colour. Coverage comes out at
/// the 5 bits a channel that Skia's LCD masks blend with, widened to 8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LcdGammaLut(Box<[[[u8; 256]; ROWS]; 3]>);

impl LcdGammaLut {
    pub fn from_rows(rows: [[[u8; 256]; ROWS]; 3]) -> Self {
        LcdGammaLut(Box::new(rows))
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != LCD_BYTES {
            return None;
        }
        let mut rows = Box::new([[[0u8; 256]; ROWS]; 3]);
        for (row, chunk) in rows.iter_mut().flatten().zip(bytes.as_chunks::<256>().0) {
            row.copy_from_slice(chunk);
        }
        Some(LcdGammaLut(rows))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.iter().flatten().flatten().copied().collect()
    }

    pub fn row(&self, channel: usize, bucket: usize) -> &[u8; 256] {
        &self.0[channel][bucket]
    }

    pub fn apply(&self, rgb: [u8; 3], coverage: [u8; 3]) -> [u8; 3] {
        [0, 1, 2].map(|c| self.0[c][usize::from(rgb[c] >> 5)][usize::from(coverage[c])])
    }

    /// The table committed for the platform this was built for, if it has LCD text. On Linux it is
    /// for fontconfig rules that ask for LCD (`stock.conf` of `xtask/hosts/linux/fonts.sh`).
    pub fn embedded() -> Option<Self> {
        #[cfg(target_os = "linux")]
        return LcdGammaLut::from_bytes(include_bytes!(
            "../../../../parity/vectors/canvas/lut-linux-lcd.bin"
        ));
        #[cfg(windows)]
        return LcdGammaLut::from_bytes(include_bytes!(
            "../../../../parity/vectors/canvas/lut-windows-lcd.bin"
        ));
        #[cfg(not(any(target_os = "linux", windows)))]
        return None;
    }
}

/// The colour `blend_lcd16_opaque` leaves where `color` text with coverage `mask`, after the
/// preblend, covers the opaque `background`.
pub fn blend_lcd(background: [u8; 3], color: [u8; 3], mask: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|c| blend_lcd_channel(background[c], color[c], mask[c] >> 3))
}

pub fn blend_lcd_channel(background: u8, color: u8, mask5: u8) -> u8 {
    blend32(background, color, i32::from(mask5 + (mask5 >> 4)))
}

/// `blend_lcd16` onto an opaque `background`: `blend_lcd` of a colour with `alpha`.
pub fn blend_lcd_alpha(background: [u8; 3], color: [u8; 3], alpha: u8, mask: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|c| {
        let mask5 = i32::from(mask[c] >> 3);
        let scale = ((mask5 + (mask5 >> 4)) * (i32::from(alpha) + 1)) >> 8;
        blend32(background[c], color[c], scale)
    })
}

/// `SkBlend32`.
fn blend32(background: u8, color: u8, scale: i32) -> u8 {
    let (src, dst) = (i32::from(color), i32::from(background));
    (dst + (((src - dst) * scale) >> 5)) as u8
}

fn to_luma(gamma: f64, v: f64) -> f64 {
    if gamma != 0.0 {
        pow(v, gamma)
    } else if v <= 0.04045 {
        v / 12.92
    } else {
        pow((v + 0.055) / 1.055, 2.4)
    }
}

fn from_luma(gamma: f64, v: f64) -> f64 {
    if gamma != 0.0 {
        pow(v, 1.0 / gamma)
    } else if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * pow(v, 1.0 / 2.4) - 0.055
    }
}

/// `SkTMaskGamma_build_correcting_lut`.
fn correcting_row(bucket: usize, contrast: f64, gamma: f64) -> [u8; 256] {
    let base = bucket << 5;
    let src = ((base | base >> 3 | base >> 6) & 255) as f64 / 255.0;
    let dst = 1.0 - src;
    let (lin_src, lin_dst) = (to_luma(gamma, src), to_luma(gamma, dst));
    let mut row = [0u8; 256];
    for (i, v) in row.iter_mut().enumerate() {
        let raw = i as f64 / 255.0;
        let a = raw + (1.0 - raw) * (contrast * lin_dst) * raw;
        let out = from_luma(gamma, lin_src * a + (1.0 - a) * lin_dst);
        *v = (255.0 * (out - dst) / (src - dst) + 0.5).floor() as u8;
    }
    row
}

/// The 3-bit luminance Skia picks a row by. Windows takes `SkComputeLuminance` of the colour as
/// `SkScalerContextRec` keeps it, 3 bits a channel. That is wrong on macOS, whose rule is open in
/// <https://github.com/AdamEXu/asrs/issues/362>.
pub fn luminance_bucket(rgb: [u8; 3]) -> usize {
    if cfg!(windows) {
        let [r, g, b] = rgb.map(|v| {
            let bits = u32::from(v) & 0xe0;
            bits | bits >> 3 | bits >> 6
        });
        return ((r * 54 + g * 183 + b * 19) >> 8) as usize >> 5;
    }
    let lum = 0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2]);
    (lum.round().clamp(0.0, 255.0) as usize) >> 5
}

/// One table row from `(raw coverage, Chromium coverage)` pairs: the median Chromium value of
/// each raw value seen at least three times, `prior` or else linear in between, 0 and 255 fixed
/// at the ends.
pub fn fit_row(samples: &[(u8, u8)], prior: Option<&[u8; 256]>) -> [u8; 256] {
    let mut by_raw: Vec<Vec<u8>> = vec![Vec::new(); 256];
    for &(raw, chromium) in samples {
        by_raw[raw as usize].push(chromium);
    }
    let mut known: Vec<(usize, f64)> = vec![(0, 0.0)];
    for (raw, values) in by_raw.iter_mut().enumerate().skip(1) {
        if values.len() >= 3 {
            values.sort_unstable();
            known.push((raw, f64::from(values[values.len() / 2])));
        } else if let Some(prior) = prior {
            known.push((raw, f64::from(prior[raw])));
        }
    }
    if known.last().is_some_and(|&(raw, _)| raw != 255) {
        known.push((255, 255.0));
    }
    let mut row = [0u8; 256];
    for pair in known.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        for (x, v) in row.iter_mut().enumerate().take(x1 + 1).skip(x0) {
            *v = (y0 + (y1 - y0) * (x - x0) as f64 / (x1 - x0) as f64).round() as u8;
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_follow_grey_level() {
        let levels = [0x00, 0x24, 0x49, 0x6d, 0x92, 0xb6, 0xdb, 0xff];
        for (k, l) in levels.into_iter().enumerate() {
            assert_eq!(luminance_bucket([l, l, l]), k);
        }
        assert_eq!(luminance_bucket([0x22, 0x22, 0x22]), 1);
        assert_eq!(luminance_bucket([0xee, 0xee, 0xee]), 7);
        assert_eq!(luminance_bucket([0x7f, 0x7f, 0x7f]), 3);
        let pink = if cfg!(windows) { 6 } else { 5 };
        assert_eq!(luminance_bucket([0xff, 0xa6, 0xb8]), pink);
    }

    #[test]
    fn fit_interpolates_between_medians() {
        let mut samples = Vec::new();
        for v in [90, 100, 110, 200] {
            samples.push((100, v));
        }
        samples.extend([(50, 1), (50, 2)]);
        let row = fit_row(&samples, None);
        assert_eq!(row[0], 0);
        assert_eq!(row[100], 110);
        assert_eq!(row[50], 55);
        assert_eq!(row[255], 255);
        assert_eq!(row[178], 183);
        let row = fit_row(&samples, Some(&[7; 256]));
        assert_eq!(
            [row[0], row[50], row[100], row[178], row[255]],
            [0, 7, 110, 7, 7]
        );
    }

    #[test]
    fn lcd_channels_use_their_own_row() {
        let mut rows = [[[0u8; 256]; ROWS]; 3];
        for (c, channel) in rows.iter_mut().enumerate() {
            for (k, row) in channel.iter_mut().enumerate() {
                row.fill((c * 10 + k) as u8);
            }
        }
        let lut = LcdGammaLut::from_rows(rows);
        assert_eq!(lut.apply([0x00, 0x7f, 0xff], [9, 9, 9]), [0, 13, 27]);
        assert_eq!(lut.row(2, 5)[200], 25);
        let bytes = lut.to_bytes();
        assert_eq!((bytes.len(), bytes[(2 * ROWS + 5) * 256]), (LCD_BYTES, 25));
        assert_eq!(LcdGammaLut::from_bytes(&bytes), Some(lut));
        assert_eq!(LcdGammaLut::from_bytes(&bytes[1..]), None);
    }

    #[test]
    fn an_opaque_colour_blends_the_same_with_and_without_alpha() {
        for mask in 0..=255u8 {
            for (background, color) in [([255; 3], [0x22; 3]), ([0x22, 9, 200], [0xee, 80, 3])] {
                let masks = [mask, 255 - mask, mask / 2];
                assert_eq!(
                    blend_lcd(background, color, masks),
                    blend_lcd_alpha(background, color, 255, masks)
                );
                assert_eq!(blend_lcd_alpha(background, color, 0, masks), background);
            }
        }
        assert_eq!(blend_lcd_alpha([255; 3], [0; 3], 127, [255; 3]), [127; 3]);
    }

    #[test]
    fn bytes_round_trip() {
        let lut = MaskGammaLut::identity();
        assert_eq!(lut.to_bytes().len(), BYTES);
        assert_eq!(MaskGammaLut::from_bytes(&lut.to_bytes()), Some(lut));
        assert_eq!(MaskGammaLut::from_bytes(&[0; 10]), None);
    }
}
