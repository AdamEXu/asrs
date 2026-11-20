use std::collections::HashMap;
use std::ffi::c_long;
use std::process::Command;
use std::ptr;

use asrs_canvas::text::{FontSpec, Mask, Shaped, ShapedGlyph, TextBackend};
use freetype_sys as ft;

unsafe extern "C" {
    fn FT_Library_Version(lib: ft::FT_Library, major: *mut i32, minor: *mut i32, patch: *mut i32);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hinting {
    None,
    Slight,
    Normal,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub hinting: Hinting,
    pub autohint: bool,
    pub subpixel: bool,
    pub truncate_size: bool,
}

impl Options {
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        Options {
            hinting: match var("X03_HINT").as_str() {
                "none" => Hinting::None,
                "normal" => Hinting::Normal,
                _ => Hinting::Slight,
            },
            autohint: var("X03_AUTOHINT") == "1",
            subpixel: var("X03_SUBPIXEL") != "0",
            truncate_size: var("X03_ROUNDSIZE") != "1",
        }
    }

    fn load_flags(&self) -> i32 {
        let mut flags = match self.hinting {
            Hinting::None => ft::FT_LOAD_NO_HINTING,
            Hinting::Slight => ft::FT_LOAD_TARGET_LIGHT,
            Hinting::Normal => ft::FT_LOAD_TARGET_NORMAL,
        };
        if self.autohint {
            flags |= ft::FT_LOAD_FORCE_AUTOHINT;
        }
        flags | ft::FT_LOAD_NO_BITMAP | ft::FT_LOAD_IGNORE_GLOBAL_ADVANCE_WIDTH
    }
}

struct Font {
    face: ft::FT_Face,
    data: Vec<u8>,
    path: String,
}

pub struct FreeType {
    lib: ft::FT_Library,
    options: Options,
    files: HashMap<u16, Font>,
    masks: HashMap<(u16, u64, u32, u8, u64, u64), Option<Mask>>,
}

pub fn fc_match(pattern: &str) -> String {
    let out = Command::new("fc-match")
        .args(["-f", "%{file}", pattern])
        .output()
        .expect("fc-match");
    String::from_utf8(out.stdout).unwrap()
}

fn fc_weight(css: u16) -> u32 {
    match css {
        0..=150 => 40,
        151..=250 => 40,
        251..=350 => 50,
        351..=450 => 80,
        451..=550 => 100,
        551..=650 => 180,
        _ => 200,
    }
}

impl FreeType {
    pub fn new(options: Options) -> Self {
        let mut lib = ptr::null_mut();
        unsafe {
            assert_eq!(ft::FT_Init_FreeType(&mut lib), 0);
        }
        FreeType {
            lib,
            options,
            files: HashMap::new(),
            masks: HashMap::new(),
        }
    }

    pub fn version(&self) -> String {
        let (mut a, mut b, mut c) = (0, 0, 0);
        unsafe { FT_Library_Version(self.lib, &mut a, &mut b, &mut c) };
        format!("{a}.{b}.{c}")
    }

    pub fn font_path(&mut self, spec: &FontSpec) -> String {
        self.font(spec).path.clone()
    }

    fn font(&mut self, spec: &FontSpec) -> &Font {
        let lib = self.lib;
        let arial = [10.0, 13.5, 21.25].contains(&spec.size);
        let key = spec.weight + if arial { 1 } else { 0 };
        self.files.entry(key).or_insert_with(|| {
            let family = if arial { "Arial" } else { "sans-serif" };
            let path = fc_match(&format!("{family}:weight={}", fc_weight(spec.weight)));
            let data = std::fs::read(&path).unwrap();
            let mut face = ptr::null_mut();
            unsafe {
                assert_eq!(
                    ft::FT_New_Memory_Face(lib, data.as_ptr(), data.len() as c_long, 0, &mut face),
                    0
                );
            }
            Font { face, data, path }
        })
    }

    fn set_size(&mut self, spec: &FontSpec, sx: f64, sy: f64) -> ft::FT_Face {
        let truncate = self.options.truncate_size;
        let face = self.font(spec).face;
        let to26 = |v: f64| {
            if truncate {
                (v * 64.0) as c_long
            } else {
                (v * 64.0).round() as c_long
            }
        };
        unsafe {
            assert_eq!(ft::FT_Set_Char_Size(face, to26(sx), to26(sy), 72, 72), 0);
        }
        face
    }

    fn render(&mut self, spec: &FontSpec, glyph: u32, scale: f64, x_scale: f64, phase: u8) -> Option<Mask> {
        let flags = self.options.load_flags();
        let subpixel = self.options.subpixel;
        let face = self.set_size(spec, spec.size * scale * x_scale, spec.size * scale);
        unsafe {
            if ft::FT_Load_Glyph(face, glyph, flags) != 0 {
                return None;
            }
            let slot = (*face).glyph;
            let outline = &mut (*slot).outline;
            if outline.n_contours == 0 {
                return None;
            }
            let (dx, extra) = if subpixel {
                (c_long::from(phase) * 16, 0)
            } else {
                (0, i32::from(phase >= 2))
            };
            ft::FT_Outline_Translate(outline, dx, 0);
            if std::env::var("X03_LCDAVG").is_ok() {
                ft::FT_Library_SetLcdFilter(self.lib, ft::FT_LCD_FILTER_DEFAULT);
                if ft::FT_Render_Glyph(slot, ft::FT_RENDER_MODE_LCD) != 0 {
                    return None;
                }
                let bm = &(*slot).bitmap;
                let (w, h) = (bm.width as usize / 3, bm.rows as usize);
                let mut data = vec![0u8; w * h];
                for row in 0..h {
                    let line = bm.buffer.offset(row as isize * bm.pitch as isize);
                    for col in 0..w {
                        let sum: u32 = (0..3).map(|k| u32::from(*line.add(col * 3 + k))).sum();
                        data[row * w + col] = ((sum + 1) / 3) as u8;
                    }
                }
                return Some(Mask {
                    left: (*slot).bitmap_left + extra,
                    top: -(*slot).bitmap_top,
                    width: w as u32,
                    height: h as u32,
                    data,
                });
            }
            let mut bbox = std::mem::zeroed::<ft::FT_BBox>();
            ft::FT_Outline_Get_CBox(outline, &mut bbox);
            let x0 = bbox.xMin.div_euclid(64);
            let y0 = bbox.yMin.div_euclid(64);
            let x1 = (bbox.xMax + 63).div_euclid(64);
            let y1 = (bbox.yMax + 63).div_euclid(64);
            let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
            if w == 0 || h == 0 {
                return None;
            }
            ft::FT_Outline_Translate(outline, -x0 * 64, -y0 * 64);
            if let Ok(mode) = std::env::var("X03_SKPATH") {
                let path = outline_path(outline, h as f32)?;
                let data = if mode == "2" {
                    let mut pm = tiny_skia::Pixmap::new(w as u32, h as u32)?;
                    let mut paint = tiny_skia::Paint::default();
                    paint.anti_alias = true;
                    paint.set_color_rgba8(0, 0, 0, 255);
                    pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tiny_skia::Transform::identity(), None);
                    pm.data().chunks(4).map(|p| p[3]).collect()
                } else {
                    let mut data = vec![0u8; w * h];
                    let c = if mode == "3" { asrs_canvas::scan::rasterize(&path, w as u32, h as u32) } else { asrs_canvas::scan::rasterize_analytic(&path, w as u32, h as u32) };
                    if let Some(c) = c {
                        for row in 0..c.height {
                            for col in 0..c.width {
                                let (x, y) = (c.x as usize + col, c.y as usize + row);
                                data[y * w + x] = c.alpha[row * c.width + col];
                            }
                        }
                    }
                    data
                };
                return Some(Mask { left: x0 as i32 + extra, top: -(y1 as i32), width: w as u32, height: h as u32, data });
            }
            let mut data = vec![0u8; w * h];
            let mut bitmap = std::mem::zeroed::<ft::FT_Bitmap>();
            bitmap.rows = h as _;
            bitmap.width = w as _;
            bitmap.pitch = w as _;
            bitmap.buffer = data.as_mut_ptr();
            bitmap.num_grays = 256;
            bitmap.pixel_mode = ft::FT_PIXEL_MODE_GRAY as _;
            if ft::FT_Outline_Get_Bitmap(self.lib, outline, &mut bitmap) != 0 {
                return None;
            }
            Some(Mask {
                left: x0 as i32 + extra,
                top: -(y1 as i32),
                width: w as u32,
                height: h as u32,
                data,
            })
        }
    }

    /// Linear advance in pixels at `size`, as Skia reports it for unhinted or slight hinting.
    fn linear_advance(&mut self, spec: &FontSpec, glyph: u32) -> f64 {
        let flags = self.options.load_flags();
        let face = self.set_size(spec, spec.size, spec.size);
        unsafe {
            ft::FT_Load_Glyph(face, glyph, flags);
            (*(*face).glyph).linearHoriAdvance as f64 / 65536.0
        }
    }
}

impl TextBackend for FreeType {
    fn shape(&mut self, spec: &FontSpec, text: &str) -> Shaped {
        let data = self.font(spec).data.clone();
        let face = rustybuzz::Face::from_slice(&data, 0).unwrap();
        let upem = f64::from(face.units_per_em());
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let output = rustybuzz::shape(&face, &[], buffer);
        let mut x = 0.0;
        let mut glyphs = Vec::new();
        for (info, pos) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            let id = info.glyph_id;
            let hmtx = face
                .glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(id as u16))
                .unwrap_or(0) as f64;
            let kern = (f64::from(pos.x_advance) - hmtx) * spec.size / upem;
            glyphs.push(ShapedGlyph {
                id,
                x: x + f64::from(pos.x_offset) * spec.size / upem,
            });
            x += self.linear_advance(spec, id) + kern;
        }
        Shaped { glyphs, width: x }
    }

    fn typo_extents(&mut self, spec: &FontSpec) -> (f64, f64) {
        let face = self.font(spec).face;
        unsafe {
            let os2 = ft::FT_Get_Sfnt_Table(face, ft::ft_sfnt_os2) as *const ft::TT_OS2;
            ((*os2).sTypoAscender as f64, -((*os2).sTypoDescender as f64))
        }
    }

    fn raw_mask(&mut self, spec: &FontSpec, glyph: u32, scale: f64, x_scale: f64, phase: u8) -> Option<Mask> {
        let key = (spec.weight, spec.size.to_bits(), glyph, phase, scale.to_bits(), x_scale.to_bits());
        if let Some(mask) = self.masks.get(&key) {
            return mask.clone();
        }
        let mask = self.render(spec, glyph, scale, x_scale, phase);
        self.masks.insert(key, mask.clone());
        mask
    }
}


struct PathSink {
    b: tiny_skia::PathBuilder,
    h: f32,
}

fn pt(v: *const ft::FT_Vector, h: f32) -> (f32, f32) {
    unsafe { ((*v).x as f32 / 64.0, h - (*v).y as f32 / 64.0) }
}

extern "C" fn move_to(to: *const ft::FT_Vector, user: *mut std::ffi::c_void) -> i32 {
    let s = unsafe { &mut *(user as *mut PathSink) };
    let (x, y) = pt(to, s.h);
    s.b.close();
    s.b.move_to(x, y);
    0
}
extern "C" fn line_to(to: *const ft::FT_Vector, user: *mut std::ffi::c_void) -> i32 {
    let s = unsafe { &mut *(user as *mut PathSink) };
    let (x, y) = pt(to, s.h);
    s.b.line_to(x, y);
    0
}
extern "C" fn conic_to(c: *const ft::FT_Vector, to: *const ft::FT_Vector, user: *mut std::ffi::c_void) -> i32 {
    let s = unsafe { &mut *(user as *mut PathSink) };
    let (cx, cy) = pt(c, s.h);
    let (x, y) = pt(to, s.h);
    s.b.quad_to(cx, cy, x, y);
    0
}
extern "C" fn cubic_to(c1: *const ft::FT_Vector, c2: *const ft::FT_Vector, to: *const ft::FT_Vector, user: *mut std::ffi::c_void) -> i32 {
    let s = unsafe { &mut *(user as *mut PathSink) };
    let (ax, ay) = pt(c1, s.h);
    let (bx, by) = pt(c2, s.h);
    let (x, y) = pt(to, s.h);
    s.b.cubic_to(ax, ay, bx, by, x, y);
    0
}

fn outline_path(outline: *mut ft::FT_Outline, h: f32) -> Option<tiny_skia::Path> {
    let funcs = ft::FT_Outline_Funcs {
        move_to,
        line_to,
        conic_to,
        cubic_to,
        shift: 0,
        delta: 0,
    };
    let mut sink = PathSink { b: tiny_skia::PathBuilder::new(), h };
    unsafe {
        ft::FT_Outline_Decompose(outline, &funcs, &mut sink as *mut PathSink as *mut std::ffi::c_void);
    }
    sink.b.close();
    sink.b.finish()
}
