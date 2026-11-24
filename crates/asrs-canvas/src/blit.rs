// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! The real blitter's side of Skia's scan converters: an ordered list of horizontal coverage
//! runs, clipped to the canvas. Order matters because Skia blends straight into the pixels, so a
//! pixel that two blits touch is blended twice rather than having its coverage summed.

/// The blitter call a run came from, where blitters treat calls apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    /// `blitAntiH2` or `blitAntiV2`.
    Pair,
    /// `blitMask`.
    Mask,
}

#[derive(Clone, Debug, Default)]
pub struct Spans {
    width: i32,
    height: i32,
    runs: Vec<(i32, i32, u32, u32, Kind)>,
    alpha: Vec<u8>,
}

impl Spans {
    pub fn new(width: u32, height: u32) -> Spans {
        Spans {
            width: width as i32,
            height: height as i32,
            ..Spans::default()
        }
    }

    pub fn same_size(&self) -> Spans {
        Spans {
            width: self.width,
            height: self.height,
            ..Spans::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// `blitAntiH`: per-pixel coverage starting at `(x, y)`.
    pub fn anti_h(&mut self, x: i32, y: i32, alphas: &[u8]) {
        self.push(x, y, alphas, Kind::Plain);
    }

    /// One row of a `blitMask`.
    pub fn mask(&mut self, x: i32, y: i32, alphas: &[u8]) {
        self.push(x, y, alphas, Kind::Mask);
    }

    /// `blitAntiH2`: two pixels side by side, which the legacy blitters blend with their own
    /// arithmetic.
    pub fn anti_h2(&mut self, x: i32, y: i32, a0: u8, a1: u8) {
        self.push(x, y, &[a0, a1], Kind::Pair);
    }

    /// `blitAntiV2`: two pixels one above the other.
    pub fn anti_v2(&mut self, x: i32, y: i32, a0: u8, a1: u8) {
        self.push(x, y, &[a0], Kind::Pair);
        self.push(x, y + 1, &[a1], Kind::Pair);
    }

    fn push(&mut self, x: i32, y: i32, alphas: &[u8], kind: Kind) {
        if y < 0 || y >= self.height {
            return;
        }
        let start = (-x).max(0) as usize;
        let end = (alphas.len() as i64).min(i64::from(self.width) - i64::from(x));
        if end <= start as i64 {
            return;
        }
        let slice = &alphas[start..end as usize];
        if slice.iter().all(|&a| a == 0) {
            return;
        }
        self.runs.push((
            x + start as i32,
            y,
            self.alpha.len() as u32,
            slice.len() as u32,
            kind,
        ));
        self.alpha.extend_from_slice(slice);
    }

    pub fn h(&mut self, x: i32, y: i32, width: i32) {
        if width > 0 {
            self.anti_h(x, y, &vec![255; width as usize]);
        }
    }

    pub fn v(&mut self, x: i32, y: i32, height: i32, alpha: u8) {
        for i in 0..height {
            self.anti_h(x, y + i, &[alpha]);
        }
    }

    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32) {
        for i in 0..height {
            self.h(x, y + i, width);
        }
    }

    /// `SkBlitter::blitAntiRect`.
    pub fn anti_rect(&mut self, x: i32, y: i32, width: i32, height: i32, left: u8, right: u8) {
        if left > 0 {
            self.v(x, y, height, left);
        }
        let mut x = x + 1;
        if width > 0 {
            self.rect(x, y, width, height);
            x += width;
        }
        if right > 0 {
            self.v(x, y, height, right);
        }
    }

    /// Every run in blit order as `(x, y, coverage)`.
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32, &[u8])> {
        self.iter_kinds().map(|(x, y, a, _)| (x, y, a))
    }

    /// Every run in blit order as `(x, y, coverage, kind)`.
    pub fn iter_kinds(&self) -> impl Iterator<Item = (i32, i32, &[u8], Kind)> {
        self.runs.iter().map(|&(x, y, start, len, kind)| {
            (
                x,
                y,
                &self.alpha[start as usize..(start + len) as usize],
                kind,
            )
        })
    }

    /// The bounding box `[left, top, right, bottom)` of every run.
    pub fn bounds(&self) -> Option<[i32; 4]> {
        let mut it = self.iter();
        let (x, y, a) = it.next()?;
        let mut b = [x, y, x + a.len() as i32, y + 1];
        for (x, y, a) in it {
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x + a.len() as i32);
            b[3] = b[3].max(y + 1);
        }
        Some(b)
    }

    /// The last coverage each pixel received, for tests.
    pub fn flatten(&self, width: usize, height: usize) -> Vec<u8> {
        let mut out = vec![0; width * height];
        for (x, y, a) in self.iter() {
            for (i, &v) in a.iter().enumerate() {
                out[y as usize * width + x as usize + i] = v;
            }
        }
        out
    }
}
