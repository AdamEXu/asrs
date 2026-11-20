pub(crate) fn hi(x: f64) -> i32 {
    (x.to_bits() >> 32) as i32
}

pub(crate) fn lo(x: f64) -> u32 {
    x.to_bits() as u32
}

pub(crate) fn from_words(hi: u32, lo: u32) -> f64 {
    f64::from_bits((u64::from(hi) << 32) | u64::from(lo))
}

pub(crate) fn with_hi(x: f64, hi: u32) -> f64 {
    from_words(hi, lo(x))
}

pub(crate) fn with_lo(x: f64, lo: u32) -> f64 {
    from_words(hi(x) as u32, lo)
}
