pub struct Run {
    /// Glyph id and pen x in CSS pixels.
    pub glyphs: Vec<(u32, f64)>,
    pub width: f64,
}

fn to_hb(v: f32) -> i64 {
    (f64::from(v) * 65536.0) as i64
}

/// Blink's HarfBuzz shaping on Windows: advances from Skia's DirectWrite scaler in natural
/// measuring mode (design advance times size over upem, as f32), carried in HarfBuzz 16.16
/// fixed point, plus GPOS adjustments scaled by HarfBuzz from font units.
pub fn shape(data: &[u8], index: u32, upem: u16, size: f64, text: &str) -> Run {
    let face = rustybuzz::Face::from_slice(data, index).expect("rustybuzz face");
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(rustybuzz::Direction::LeftToRight);
    let shaped = rustybuzz::shape(&face, &[], buffer);
    let size32 = size as f32;
    let scale = to_hb(size32);
    let mut pen: i64 = 0;
    let mut glyphs = Vec::new();
    for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let gid = rustybuzz::ttf_parser::GlyphId(info.glyph_id as u16);
        let design = i32::from(face.glyph_hor_advance(gid).unwrap_or(0));
        let base = to_hb(size32 * design as f32 / f32::from(upem));
        let adjust = i64::from(pos.x_advance - design);
        let adjust = (adjust * scale + i64::from(upem) / 2).div_euclid(i64::from(upem));
        let offset = (i64::from(pos.x_offset) * scale).div_euclid(i64::from(upem));
        glyphs.push((info.glyph_id, (pen + offset) as f64 / 65536.0));
        pen += base + adjust;
    }
    Run {
        glyphs,
        width: pen as f64 / 65536.0,
    }
}
