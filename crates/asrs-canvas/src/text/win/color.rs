// Ported from Skia, Chromium m142 (BSD-3-Clause, Copyright 2011-2023 Google Inc.). See THIRD_PARTY_NOTICES.md.

//! Colour glyphs as `SkScalerContext_DW` draws them: the COLR version 1 paint tree DirectWrite
//! reads, or the coloured outlines of version 0.

use std::mem::ManuallyDrop;

use tiny_skia::{BlendMode, FilterQuality, Pixmap, PixmapPaint, Transform};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F, D2D1_GRADIENT_STOP};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_COLOR_COMPOSITE_MODE, DWRITE_COLOR_F, DWRITE_GLYPH_IMAGE_FORMATS_COLR_PAINT_TREE,
    DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN, DWRITE_MATRIX, DWRITE_MEASURING_MODE,
    DWRITE_PAINT_ELEMENT, DWRITE_PAINT_FEATURE_LEVEL_COLR_V1, DWRITE_PAINT_TYPE_COLOR_GLYPH,
    DWRITE_PAINT_TYPE_COMPOSITE, DWRITE_PAINT_TYPE_GLYPH, DWRITE_PAINT_TYPE_LAYERS,
    DWRITE_PAINT_TYPE_LINEAR_GRADIENT, DWRITE_PAINT_TYPE_NONE, DWRITE_PAINT_TYPE_RADIAL_GRADIENT,
    DWRITE_PAINT_TYPE_SOLID, DWRITE_PAINT_TYPE_SOLID_GLYPH, DWRITE_PAINT_TYPE_SWEEP_GRADIENT,
    DWRITE_PAINT_TYPE_TRANSFORM, IDWriteFactory2, IDWriteFontFace, IDWritePaintReader,
};

use super::outline::outline;
use super::shader::{self, Fill, Stop, Tile};
use super::typeface::Typeface;
use crate::aaa;
use crate::blit::Spans;
use crate::color::Rgba;
use crate::hairline::P;
use crate::path::Affine;
use crate::raster_clip::RasterClip;
use crate::scan::{self, Blend};
use crate::skpath::{Rect, SkPath, map_point};
use crate::text::Mask;

const ELEMENT_SIZE: u32 = size_of::<DWRITE_PAINT_ELEMENT>() as u32;

enum Source {
    /// The glyph's paint tree, read from the root.
    Tree {
        reader: IDWritePaintReader,
        root: DWRITE_PAINT_ELEMENT,
        clip: Option<Rect>,
    },
    /// Outlines at the text size and their colours.
    Layers(Vec<(SkPath, Rgba)>),
}

/// A glyph that has colours.
pub struct Glyph<'a> {
    face: &'a IDWriteFontFace,
    size: f32,
    matrix: Affine,
    source: Source,
}

fn affine(m: &DWRITE_MATRIX) -> Affine {
    Affine {
        a: f64::from(m.m11),
        b: f64::from(m.m12),
        c: f64::from(m.m21),
        d: f64::from(m.m22),
        e: f64::from(m.dx),
        f: f64::from(m.dy),
    }
}

fn rgba(c: &DWRITE_COLOR_F) -> Rgba {
    Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

fn stop(s: &D2D1_GRADIENT_STOP) -> Stop {
    let D2D1_COLOR_F { r, g, b, a } = s.color;
    (s.position, Rgba { r, g, b, a })
}

fn tile(mode: u32) -> Tile {
    const D2D1_EXTEND_MODE_WRAP: u32 = 1;
    const D2D1_EXTEND_MODE_MIRROR: u32 = 2;
    match mode {
        D2D1_EXTEND_MODE_WRAP => Tile::Repeat,
        D2D1_EXTEND_MODE_MIRROR => Tile::Mirror,
        _ => Tile::Clamp,
    }
}

fn clip_box(r: &D2D_RECT_F) -> Option<Rect> {
    (r.right > r.left && r.bottom > r.top).then_some(Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    })
}

fn blend_mode(mode: DWRITE_COLOR_COMPOSITE_MODE) -> BlendMode {
    const MODES: [BlendMode; 28] = [
        BlendMode::Clear,
        BlendMode::Source,
        BlendMode::Destination,
        BlendMode::SourceOver,
        BlendMode::DestinationOver,
        BlendMode::SourceIn,
        BlendMode::DestinationIn,
        BlendMode::SourceOut,
        BlendMode::DestinationOut,
        BlendMode::SourceAtop,
        BlendMode::DestinationAtop,
        BlendMode::Xor,
        BlendMode::Plus,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::HardLight,
        BlendMode::SoftLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Multiply,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::Color,
        BlendMode::Luminosity,
    ];
    usize::try_from(mode.0)
        .ok()
        .and_then(|i| MODES.get(i).copied())
        .unwrap_or(BlendMode::Destination)
}

/// `SkMatrix::mapRect`.
fn map_rect(m: &Affine, r: &Rect) -> Rect {
    let corners = [
        (r.left, r.top),
        (r.right, r.top),
        (r.left, r.bottom),
        (r.right, r.bottom),
    ]
    .map(|(x, y)| map_point(m, P { x, y }));
    let fold = |f: fn(f32, f32) -> f32, start: f32, pick: fn(&P) -> f32| {
        corners.iter().map(pick).fold(start, f)
    };
    Rect {
        left: fold(f32::min, f32::INFINITY, |p| p.x),
        top: fold(f32::min, f32::INFINITY, |p| p.y),
        right: fold(f32::max, f32::NEG_INFINITY, |p| p.x),
        bottom: fold(f32::max, f32::NEG_INFINITY, |p| p.y),
    }
}

/// `SkRect::join`.
fn join(all: Option<Rect>, r: Rect) -> Option<Rect> {
    if r.is_empty() {
        return all;
    }
    Some(match all {
        None => r,
        Some(a) => Rect {
            left: a.left.min(r.left),
            top: a.top.min(r.top),
            right: a.right.max(r.right),
            bottom: a.bottom.max(r.bottom),
        },
    })
}

impl Glyph<'_> {
    /// `None` for a glyph without colours.
    pub fn new<'a>(
        factory: &IDWriteFactory2,
        typeface: &'a Typeface,
        id: u16,
        size: f32,
        measuring: DWRITE_MEASURING_MODE,
        transform: DWRITE_MATRIX,
    ) -> Option<Glyph<'a>> {
        let source = tree(typeface, id)
            .or_else(|| layers(factory, &typeface.face, id, size, measuring, &transform))?;
        Some(Glyph {
            face: &typeface.face,
            size,
            matrix: affine(&transform),
            source,
        })
    }

    /// `matrix` from the em square the paint tree is in.
    fn em_matrix(&self) -> Affine {
        let size = f64::from(self.size);
        self.matrix.then(&Affine::scaling(size, size))
    }

    /// The mask's `[left, top, right, bottom]`.
    pub fn bounds(&self) -> [i32; 4] {
        let bounds = match &self.source {
            Source::Tree {
                clip: Some(clip), ..
            } => Some(map_rect(&self.em_matrix(), clip)),
            Source::Tree { reader, root, .. } => self.tree_bounds(reader, root, &self.em_matrix()),
            Source::Layers(layers) => layers
                .iter()
                .fold(None, |all, (path, _)| join(all, path.bounds()))
                .map(|r| map_rect(&self.matrix, &r)),
        };
        bounds.map(|r| r.round_out()).unwrap_or_default()
    }

    fn em_outline(&self, glyph: u32) -> Option<SkPath> {
        let id = u16::try_from(glyph).ok()?;
        let path = outline(self.face, self.size, &[id], None, None)?;
        let em = 1.0 / f64::from(self.size);
        Some(path.transformed(&Affine::scaling(em, em)))
    }

    fn tree_bounds(
        &self,
        reader: &IDWritePaintReader,
        element: &DWRITE_PAINT_ELEMENT,
        matrix: &Affine,
    ) -> Option<Rect> {
        let of_glyph = |glyph| Some(self.em_outline(glyph)?.transformed(matrix).bounds());
        let of_children = |count, matrix: &Affine| {
            let mut all = None;
            for_children(reader, count, |child| {
                if let Some(r) = self.tree_bounds(reader, child, matrix) {
                    all = join(all, r);
                }
            });
            all
        };
        // SAFETY: each union field is read under its paint type.
        unsafe {
            let paint = &element.paint;
            match element.paintType {
                DWRITE_PAINT_TYPE_LAYERS => of_children(paint.layers.childCount, matrix),
                DWRITE_PAINT_TYPE_SOLID_GLYPH => of_glyph(paint.solidGlyph.glyphIndex),
                DWRITE_PAINT_TYPE_GLYPH => of_glyph(paint.glyph.glyphIndex),
                DWRITE_PAINT_TYPE_COLOR_GLYPH => match clip_box(&paint.colorGlyph.clipBox) {
                    Some(clip) => Some(map_rect(matrix, &clip)),
                    None => of_children(1, matrix),
                },
                DWRITE_PAINT_TYPE_TRANSFORM => {
                    of_children(1, &matrix.then(&affine(&paint.transform)))
                }
                DWRITE_PAINT_TYPE_COMPOSITE => of_children(2, matrix),
                _ => None,
            }
        }
    }

    pub fn draw(&self) -> Option<Mask> {
        let [left, top, right, bottom] = self.bounds();
        let (width, height) = (
            u32::try_from(right - left).ok()?,
            u32::try_from(bottom - top).ok()?,
        );
        let mut pixmap = Pixmap::new(width, height)?;
        let origin = Affine::translation(f64::from(-left), f64::from(-top));
        let everything = RasterClip::Rect([0, 0, width as i32, height as i32]);
        match &self.source {
            Source::Tree { reader, root, clip } => {
                let matrix = origin.then(&self.em_matrix());
                let clip = match clip {
                    Some(clip) => clip_rect(&everything, &matrix, clip, false),
                    None => everything,
                };
                self.paint(reader, root, &matrix, &clip, &mut pixmap);
            }
            Source::Layers(layers) => {
                let matrix = origin.then(&self.matrix);
                for (path, color) in layers {
                    fill_path(&mut pixmap, &path.transformed(&matrix), &everything, *color);
                }
            }
        }
        Some(Mask {
            left,
            top,
            width,
            height,
            data: pixmap.take(),
            color: true,
        })
    }

    /// `drawColorV1Paint`.
    fn paint(
        &self,
        reader: &IDWritePaintReader,
        element: &DWRITE_PAINT_ELEMENT,
        matrix: &Affine,
        clip: &RasterClip,
        target: &mut Pixmap,
    ) {
        // SAFETY: each union field is read under its paint type.
        unsafe {
            let paint = &element.paint;
            match element.paintType {
                DWRITE_PAINT_TYPE_LAYERS => {
                    for_children(reader, paint.layers.childCount, |child| {
                        self.paint(reader, child, matrix, clip, target);
                    });
                }
                DWRITE_PAINT_TYPE_SOLID_GLYPH => {
                    if let Some(path) = self.em_outline(paint.solidGlyph.glyphIndex) {
                        let color = rgba(&paint.solidGlyph.color.value);
                        fill_path(target, &path.transformed(matrix), clip, color);
                    }
                }
                DWRITE_PAINT_TYPE_GLYPH => {
                    if let Some(path) = self.em_outline(paint.glyph.glyphIndex) {
                        let clip = clip.intersect_path(&path.transformed(matrix));
                        for_children(reader, 1, |child| {
                            self.paint(reader, child, matrix, &clip, target);
                        });
                    }
                }
                DWRITE_PAINT_TYPE_COLOR_GLYPH => {
                    let clip = match clip_box(&paint.colorGlyph.clipBox) {
                        Some(inner) => clip_rect(clip, matrix, &inner, true),
                        None => clip.clone(),
                    };
                    for_children(reader, 1, |child| {
                        self.paint(reader, child, matrix, &clip, target);
                    });
                }
                DWRITE_PAINT_TYPE_TRANSFORM => {
                    let matrix = matrix.then(&affine(&paint.transform));
                    for_children(reader, 1, |child| {
                        self.paint(reader, child, &matrix, clip, target);
                    });
                }
                DWRITE_PAINT_TYPE_COMPOSITE => {
                    let mode = blend_mode(paint.composite.mode);
                    self.composite(reader, mode, matrix, clip, target);
                }
                _ => {
                    let fill = fill_of(reader, element);
                    fill_clip(target, clip, matrix, &fill);
                }
            }
        }
    }

    fn composite(
        &self,
        reader: &IDWritePaintReader,
        mode: BlendMode,
        matrix: &Affine,
        clip: &RasterClip,
        target: &mut Pixmap,
    ) {
        let (width, height) = (target.width(), target.height());
        let everything = RasterClip::Rect([0, 0, width as i32, height as i32]);
        let mut layers = Vec::new();
        for_children(reader, 2, |child| {
            if let Some(mut layer) = Pixmap::new(width, height) {
                self.paint(reader, child, matrix, &everything, &mut layer);
                layers.push(layer);
            }
        });
        let [source, backdrop] = &mut layers[..] else {
            return;
        };
        let blend = PixmapPaint {
            blend_mode: mode,
            quality: FilterQuality::Nearest,
            ..PixmapPaint::default()
        };
        backdrop.draw_pixmap(0, 0, source.as_ref(), &blend, Transform::identity(), None);
        let spans = clip.apply(all_of(width, height));
        let pixels = backdrop.pixels();
        scan::blit_spans(target, &spans, Blend::SourceOver, |x, y| {
            let p = pixels[y * width as usize + x];
            [p.red(), p.green(), p.blue(), p.alpha()]
        });
    }
}

fn all_of(width: u32, height: u32) -> Spans {
    let mut spans = Spans::new(width, height);
    spans.rect(0, 0, width as i32, height as i32);
    spans
}

fn clip_rect(clip: &RasterClip, matrix: &Affine, r: &Rect, antialias: bool) -> RasterClip {
    if matrix.b != 0.0 || matrix.c != 0.0 {
        let mut path = SkPath::new();
        path.move_to(P {
            x: r.left,
            y: r.top,
        });
        for (x, y) in [(r.right, r.top), (r.right, r.bottom), (r.left, r.bottom)] {
            path.line_to(P { x, y });
        }
        path.close();
        return clip.intersect_path(&path.transformed(matrix));
    }
    let mut device = map_rect(matrix, r);
    if !antialias {
        for v in [
            &mut device.left,
            &mut device.top,
            &mut device.right,
            &mut device.bottom,
        ] {
            *v = (*v + 0.5).floor();
        }
    }
    clip.intersect_rect(device)
}

fn color8(color: Rgba) -> [u8; 4] {
    let [r, g, b] = color.to_u8();
    [r, g, b, (color.a * 255.0 + 0.5) as u8]
}

fn fill_path(target: &mut Pixmap, path: &SkPath, clip: &RasterClip, color: Rgba) {
    let mut spans = Spans::new(target.width(), target.height());
    aaa::fill_path(path, target.width(), target.height(), false, &mut spans);
    scan::blit_spans_solid(target, &clip.apply(spans), color8(color));
}

fn fill_clip(target: &mut Pixmap, clip: &RasterClip, matrix: &Affine, fill: &Fill) {
    let spans = clip.apply(all_of(target.width(), target.height()));
    match fill {
        Fill::Nothing => {}
        Fill::Solid(color) => scan::blit_spans_solid(target, &spans, color8(*color)),
        Fill::Gradient(gradient) => {
            if let Some(inverse) = matrix.inverse() {
                scan::blit_spans(target, &spans, Blend::SourceOver, |x, y| {
                    gradient.at(&inverse, x, y)
                });
            }
        }
    }
}

/// The fill of a solid or gradient paint element.
fn fill_of(reader: &IDWritePaintReader, element: &DWRITE_PAINT_ELEMENT) -> Fill {
    let stops = |count: u32| {
        let mut stops = vec![D2D1_GRADIENT_STOP::default(); count as usize];
        // SAFETY: `stops` holds the number of stops the element has.
        unsafe { reader.GetGradientStops(0, &mut stops) }.ok()?;
        Some(stops.iter().map(stop).collect::<Vec<_>>())
    };
    // SAFETY: each union field is read under its paint type.
    unsafe {
        let paint = &element.paint;
        match element.paintType {
            DWRITE_PAINT_TYPE_SOLID => Fill::Solid(rgba(&paint.solid.value)),
            DWRITE_PAINT_TYPE_LINEAR_GRADIENT => {
                let g = &paint.linearGradient;
                let points = [(g.x0, g.y0), (g.x1, g.y1), (g.x2, g.y2)];
                stops(g.gradientStopCount).map_or(Fill::Nothing, |stops| {
                    shader::linear(points, stops, tile(g.extendMode))
                })
            }
            DWRITE_PAINT_TYPE_RADIAL_GRADIENT => {
                let g = &paint.radialGradient;
                let (start, end) = (((g.x0, g.y0), g.radius0), ((g.x1, g.y1), g.radius1));
                stops(g.gradientStopCount).map_or(Fill::Nothing, |stops| {
                    shader::radial(start, end, stops, tile(g.extendMode))
                })
            }
            DWRITE_PAINT_TYPE_SWEEP_GRADIENT => {
                let g = &paint.sweepGradient;
                let center = (g.centerX, g.centerY);
                stops(g.gradientStopCount).map_or(Fill::Nothing, |stops| {
                    shader::sweep(
                        center,
                        (g.startAngle, g.endAngle),
                        stops,
                        tile(g.extendMode),
                    )
                })
            }
            _ => Fill::Nothing,
        }
    }
}

/// Visits the children of the reader's current element, leaving it current.
fn for_children(
    reader: &IDWritePaintReader,
    count: u32,
    mut visit: impl FnMut(&DWRITE_PAINT_ELEMENT),
) {
    for index in 0..count {
        let mut child = DWRITE_PAINT_ELEMENT::default();
        // SAFETY: the reader writes the element, whose size it is told.
        let moved = unsafe {
            if index == 0 {
                reader.MoveToFirstChild(&mut child, ELEMENT_SIZE)
            } else {
                reader.MoveToNextSibling(&mut child, ELEMENT_SIZE)
            }
        };
        if moved.is_err() {
            if index == 0 {
                return;
            }
            break;
        }
        visit(&child);
    }
    if count > 0 {
        // SAFETY: a plain COM call; a child is current.
        let _ = unsafe { reader.MoveToParent() };
    }
}

fn tree(typeface: &Typeface, id: u16) -> Option<Source> {
    let face = typeface.paint.as_ref()?;
    let mut root = DWRITE_PAINT_ELEMENT::default();
    let mut clip = D2D_RECT_F::default();
    // SAFETY: the reader writes the element, whose size it is told, and the clip box.
    let reader = unsafe {
        let reader = face
            .CreatePaintReader(
                DWRITE_GLYPH_IMAGE_FORMATS_COLR_PAINT_TREE,
                DWRITE_PAINT_FEATURE_LEVEL_COLR_V1,
            )
            .ok()?;
        reader
            .SetCurrentGlyph(u32::from(id), &mut root, ELEMENT_SIZE, &mut clip, None)
            .ok()?;
        reader
    };
    (root.paintType != DWRITE_PAINT_TYPE_NONE).then(|| Source::Tree {
        reader,
        root,
        clip: clip_box(&clip),
    })
}

fn layers(
    factory: &IDWriteFactory2,
    face: &IDWriteFontFace,
    id: u16,
    size: f32,
    measuring: DWRITE_MEASURING_MODE,
    transform: &DWRITE_MATRIX,
) -> Option<Source> {
    let advance = 0.0f32;
    let offset = DWRITE_GLYPH_OFFSET::default();
    let run = DWRITE_GLYPH_RUN {
        fontFace: ManuallyDrop::new(Some(face.clone())),
        fontEmSize: size,
        glyphCount: 1,
        glyphIndices: &id,
        glyphAdvances: &advance,
        glyphOffsets: &offset,
        isSideways: false.into(),
        bidiLevel: 0,
    };
    let mut out = Vec::new();
    // SAFETY: the run points at live values for the call and its face is released once; each
    // layer's run is read before the enumerator moves on.
    unsafe {
        let layers =
            factory.TranslateColorGlyphRun(0.0, 0.0, &run, None, measuring, Some(transform), 0);
        drop(ManuallyDrop::into_inner(run.fontFace));
        let layers = layers.ok()?;
        while layers.MoveNext().is_ok_and(|more| more.as_bool()) {
            let layer = layers.GetCurrentRun().ok()?.as_ref()?;
            let run = &layer.glyphRun;
            let count = run.glyphCount as usize;
            let glyphs = std::slice::from_raw_parts(run.glyphIndices, count);
            let advances = (!run.glyphAdvances.is_null())
                .then(|| std::slice::from_raw_parts(run.glyphAdvances, count));
            let offsets = (!run.glyphOffsets.is_null())
                .then(|| std::slice::from_raw_parts(run.glyphOffsets, count));
            let face = run.fontFace.as_ref()?;
            let path = outline(face, run.fontEmSize, glyphs, advances, offsets)?;
            let color = if layer.paletteIndex == 0xffff {
                Rgba::BLACK
            } else {
                rgba(&layer.runColor)
            };
            out.push((path, color));
        }
    }
    Some(Source::Layers(out))
}
