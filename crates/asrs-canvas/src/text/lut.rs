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
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return None;
    }
}

pub fn luminance_bucket(rgb: [u8; 3]) -> usize {
    let lum = 0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2]);
    (lum.round().clamp(0.0, 255.0) as usize) >> 5
}

/// One table row from `(raw coverage, Chromium coverage)` pairs: the median Chromium value of
/// each raw value seen at least three times, linear in between, 0 and 255 fixed at the ends.
pub fn fit_row(samples: &[(u8, u8)]) -> [u8; 256] {
    let mut by_raw: Vec<Vec<u8>> = vec![Vec::new(); 256];
    for &(raw, chromium) in samples {
        by_raw[raw as usize].push(chromium);
    }
    let mut known: Vec<(usize, f64)> = vec![(0, 0.0)];
    for (raw, values) in by_raw.iter_mut().enumerate().skip(1) {
        if values.len() >= 3 {
            values.sort_unstable();
            known.push((raw, f64::from(values[values.len() / 2])));
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
    }

    #[test]
    fn fit_interpolates_between_medians() {
        let mut samples = Vec::new();
        for v in [90, 100, 110, 200] {
            samples.push((100, v));
        }
        samples.extend([(50, 1), (50, 2)]);
        let row = fit_row(&samples);
        assert_eq!(row[0], 0);
        assert_eq!(row[100], 110);
        assert_eq!(row[50], 55);
        assert_eq!(row[255], 255);
        assert_eq!(row[178], 183);
    }

    #[test]
    fn bytes_round_trip() {
        let lut = MaskGammaLut::identity();
        assert_eq!(lut.to_bytes().len(), BYTES);
        assert_eq!(MaskGammaLut::from_bytes(&lut.to_bytes()), Some(lut));
        assert_eq!(MaskGammaLut::from_bytes(&[0; 10]), None);
    }
}
