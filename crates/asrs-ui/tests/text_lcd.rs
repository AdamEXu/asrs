#![cfg(feature = "parity")]
#![allow(clippy::disallowed_methods)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::text::lut::{LcdGammaLut, ROWS};
use asrs_canvas::text::{
    Bounds, FixedAdvance, FontMetrics, Glyph, LcdMask, Mask, Shaped, TextBackend,
};
use asrs_ui::gpui::{HeadlessAppContext, NoopTextSystem};
use asrs_ui::text::{
    CacheStats, FontSpec, Text, TextStyle, TextSystem, faded, grayscale_layer, opaque_background,
    scroller,
};
use gpui_kit::{
    AnyElement, AnyWindowHandle, AppContext, Context, IntoElement, ParentElement, Render, Styled,
    Window, div, linear_color_stop, linear_gradient, px, rgb, size,
};

/// Glyphs of 2 by 8 px, `lcd` if it says so.
struct Blocks {
    lcd: bool,
}

const FULL: [u8; 48] = [0xff; 48];

impl TextBackend for Blocks {
    fn shape(&mut self, font: &FontSpec, text: &str, zoom: f32) -> Shaped {
        FixedAdvance.shape(font, text, zoom)
    }

    fn metrics(&mut self, font: &FontSpec) -> FontMetrics {
        FixedAdvance.metrics(font)
    }

    fn glyph_bounds(&mut self, glyph: Glyph) -> Option<Bounds> {
        FixedAdvance.glyph_bounds(glyph)
    }

    fn raw_mask(&mut self, _: Glyph, _: f64, _: f64, _: u8) -> Option<Arc<Mask>> {
        Some(Arc::new(Mask {
            left: 0,
            top: -8,
            width: 2,
            height: 8,
            data: FULL[..16].to_vec(),
            color: false,
        }))
    }

    fn raw_mask_transformed(&mut self, _: Glyph, _: [f64; 4], _: (u8, u8)) -> Option<Arc<Mask>> {
        None
    }

    fn raw_lcd_mask(&mut self, _: Glyph, _: f64, _: f64, _: u8) -> Option<Arc<LcdMask>> {
        self.lcd.then(|| {
            Arc::new(LcdMask {
                left: 0,
                top: -8,
                width: 2,
                height: 8,
                data: FULL.to_vec(),
            })
        })
    }
}

fn lut() -> LcdGammaLut {
    let row = std::array::from_fn(|i| i as u8);
    LcdGammaLut::from_rows([[row; ROWS]; 3])
}

type Build = Rc<dyn Fn() -> AnyElement>;

struct Page {
    build: Rc<RefCell<Build>>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        (self.build.borrow().clone())()
    }
}

struct Fixture {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    build: Rc<RefCell<Build>>,
}

impl Fixture {
    fn new(scale: f32, backend: Blocks, lut: Option<LcdGammaLut>) -> Fixture {
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let build: Rc<RefCell<Build>> = Rc::new(RefCell::new(Rc::new(|| div().into_any_element())));
        let page = build.clone();
        let window = cx
            .open_window(size(px(400.0), px(300.0)), |_, cx| {
                cx.new(|_| Page { build: page })
            })
            .expect("a window")
            .into();
        cx.update(|cx| TextSystem::set_backend(cx, Box::new(backend), lut));
        cx.simulate_scale_factor_change(window, scale);
        Fixture { cx, window, build }
    }

    fn lcd(scale: f32) -> Fixture {
        Fixture::new(scale, Blocks { lcd: true }, Some(lut()))
    }

    fn draw(&mut self, build: impl Fn() -> AnyElement + 'static) -> CacheStats {
        *self.build.borrow_mut() = Rc::new(build);
        let before = self.stats();
        self.redraw();
        let after = self.stats();
        CacheStats {
            rasterized: after.rasterized - before.rasterized,
            lcd: after.lcd - before.lcd,
            ..after
        }
    }

    fn redraw(&mut self) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .expect("a draw");
    }

    fn stats(&mut self) -> CacheStats {
        let id = self.window.window_id();
        self.cx.update(|cx| TextSystem::stats(cx, id))
    }
}

fn style() -> TextStyle {
    TextStyle::new(FontSpec::parse("14px system-ui").unwrap())
}

fn text(s: &str) -> Text {
    Text::new(s.to_owned(), style()).color(0x222222)
}

fn page(color: u32, child: impl IntoElement) -> AnyElement {
    div()
        .size_full()
        .bg(rgb(color))
        .child(child)
        .into_any_element()
}

#[test]
fn text_on_an_opaque_colour_is_lcd_and_keeps_one_image_per_colour() {
    let mut f = Fixture::lcd(1.0);
    let drawn = f.draw(|| page(0xffffff, text("run")));
    assert_eq!((drawn.rasterized, drawn.lcd), (1, 1));
    let hovered = f.draw(|| page(0xeeeeee, text("run")));
    assert_eq!(
        (hovered.rasterized, hovered.lcd, hovered.entries),
        (1, 1, 1)
    );
    let left = f.draw(|| page(0xffffff, text("run")));
    assert_eq!((left.rasterized, left.lcd), (0, 0));
}

#[test]
fn text_over_two_colours_is_lcd_on_both() {
    let mut f = Fixture::lcd(1.0);
    let drawn = f.draw(|| {
        let selection = div().absolute().left_0().top_0().w(px(10.0)).h(px(30.0));
        page(
            0xffffff,
            div().child(selection.bg(rgb(0x3399ff))).child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .child(text("a longer run")),
            ),
        )
    });
    assert_eq!((drawn.rasterized, drawn.lcd, drawn.entries), (2, 2, 1));
}

#[test]
fn text_is_grayscale_where_chromium_has_no_opaque_layer() {
    let mut f = Fixture::lcd(1.0);
    type Case = (&'static str, fn() -> AnyElement);
    let gray: [Case; 6] = [
        ("no background", || text("run").into_any_element()),
        ("opacity", || {
            page(0xffffff, div().opacity(0.5).child(text("run")))
        }),
        ("a layer", || {
            page(0xffffff, grayscale_layer(true, text("run")))
        }),
        ("one text", || page(0xffffff, text("run").grayscale())),
        ("a gradient", || {
            let (from, to) = (
                linear_color_stop(rgb(0), 0.0),
                linear_color_stop(rgb(1), 1.0),
            );
            div()
                .size_full()
                .bg(linear_gradient(90.0, from, to))
                .child(text("run"))
                .into_any_element()
        }),
        ("a translucent page", || {
            let color = gpui_kit::rgba(0xffffff80);
            div()
                .size_full()
                .bg(color)
                .child(text("run"))
                .into_any_element()
        }),
    ];
    for (name, build) in gray {
        let drawn = f.draw(build);
        assert_eq!(drawn.lcd, 0, "{name}");
        assert_eq!(drawn.entries, 1, "{name}");
    }
    let lcd = f.draw(|| page(0xffffff, text("run").alpha(0.5)));
    assert_eq!(lcd.lcd, 1, "a translucent colour");
    let lcd = f.draw(|| page(0xffffff, faded(0.25, text("run"))));
    assert_eq!(lcd.lcd, 1, "opacity that preserves LCD text");
    let gray = f.draw(|| page(0xffffff, faded(0.75, div().opacity(0.5).child(text("run")))));
    assert_eq!(gray.lcd, 0, "opacity within it");
}

#[test]
fn a_grayscale_window_has_no_lcd_text() {
    let mut f = Fixture::lcd(1.0);
    let id = f.window.window_id();
    f.cx.update(|cx| TextSystem::set_grayscale(cx, id));
    assert_eq!(f.draw(|| page(0xffffff, text("run"))).lcd, 0);
}

fn own(child: impl IntoElement) -> AnyElement {
    opaque_background(false, div().bg(rgb(0xaaaaaa)).child(child)).into_any_element()
}

#[test]
fn a_grayscale_window_keeps_lcd_text_over_an_opaque_background_of_its_own() {
    type Case = (f32, &'static str, u64, fn() -> AnyElement);
    let cases: [Case; 11] = [
        (1.0, "the page", 0, || page(0xffffff, text("run"))),
        (1.0, "its own", 1, || page(0xffffff, own(text("run")))),
        (1.0, "the page's in it", 0, || {
            page(0xffffff, opaque_background(false, text("run")))
        }),
        (1.0, "opacity in it", 1, || {
            page(0xffffff, own(text("run").grayscale()))
        }),
        (1.0, "a layer in it", 0, || {
            page(0xffffff, own(grayscale_layer(true, text("run"))))
        }),
        (1.0, "a placeholder", 0, || {
            page(0xffffff, own(text("run").page_layer()))
        }),
        (1.0, "in a layer", 1, || {
            page(0xffffff, grayscale_layer(true, own(text("run"))))
        }),
        (1.0, "under the page's translucent box", 0, || {
            let under = div().absolute().size_full().bg(rgb(0xaaaaaa));
            let over = div().absolute().size_full().bg(gpui_kit::rgba(0xff000080));
            let run = opaque_background(false, div().absolute().child(text("run")));
            let layers = div().size_full().child(opaque_background(false, under));
            page(0xffffff, layers.child(over).child(run))
        }),
        (2.0, "its own", 0, || page(0xffffff, own(text("run")))),
        (2.0, "composited", 1, || {
            let own = div().bg(rgb(0xaaaaaa)).child(text("run"));
            page(0xffffff, opaque_background(true, own))
        }),
        (2.0, "in a scroller", 1, || {
            page(0xffffff, scroller(true, own(text("run"))))
        }),
    ];
    for (scale, name, lcd, build) in cases {
        let mut f = Fixture::lcd(scale);
        let id = f.window.window_id();
        f.cx.update(|cx| TextSystem::set_grayscale(cx, id));
        assert_eq!(f.draw(build).lcd, lcd, "{scale} {name}");
    }
    let mut f = Fixture::lcd(1.0);
    assert_eq!(
        f.draw(|| page(0xffffff, own(text("run").grayscale()))).lcd,
        0
    );
    assert_eq!(
        f.draw(|| page(0xffffff, opaque_background(false, text("run"))))
            .lcd,
        1
    );
    let placeholder = f.draw(|| page(0xffffff, own(text("run").page_layer())));
    assert_eq!(placeholder.lcd, 1);
}

#[test]
fn scrollers_are_grayscale_where_chromium_composites_them() {
    for (scale, overflows, lcd) in [(1.0, true, 1), (2.0, true, 0), (2.0, false, 1)] {
        let mut f = Fixture::lcd(scale);
        let drawn = f.draw(move || page(0xffffff, scroller(overflows, text("run"))));
        assert_eq!(drawn.lcd, lcd, "{scale} {overflows}");
        let after = f.draw(|| page(0xffffff, text("beside")));
        assert_eq!(after.lcd, 1, "{scale} {overflows}");
    }
}

#[test]
fn an_animated_background_keeps_few_images_of_a_run() {
    let mut f = Fixture::lcd(1.0);
    let frame = Rc::new(Cell::new(0));
    let color = frame.clone();
    let mut most = 0;
    for n in 0..500 {
        frame.set(n);
        let color = color.clone();
        let drawn = f.draw(move || page(0x100000 + color.get(), text("run")));
        assert_eq!((drawn.lcd, drawn.entries), (1, 1));
        most = most.max(drawn.bytes);
    }
    let glyphs = 3;
    let image = 8 * 4 * (style().font.size as usize * glyphs);
    assert!(most <= 64 + 3 + 4 * image, "{most} bytes of one run");
    assert_eq!(f.stats().evicted, 496);
}

#[test]
fn a_run_on_many_colours_of_one_frame_keeps_them_all() {
    let mut f = Fixture::lcd(1.0);
    let rows = || {
        let rows = (0..9u32).map(|i| {
            let bg = rgb(0x101010 * (i + 1));
            div().h(px(20.0)).bg(bg).child(text("run"))
        });
        page(0xffffff, div().children(rows))
    };
    let first = f.draw(rows);
    assert_eq!((first.lcd, first.entries, first.evicted), (9, 1, 0));
    assert_eq!(f.draw(rows).rasterized, 0);
    let other = f.draw(|| page(0xffffff, text("run")));
    assert_eq!((other.lcd, other.evicted), (1, 6));
}

#[test]
fn the_cache_stays_within_its_budget_on_many_backgrounds() {
    let mut f = Fixture::lcd(1.0);
    let (bytes, entries) = (256 << 10, 400);
    f.cx.update(|cx| TextSystem::set_budget(cx, bytes, entries));
    for n in 0..200u32 {
        let drawn = f.draw(move || {
            let rows = (0..30).map(|row| {
                let bg = rgb(0x010101 * (n % 7 * 16 + row % 2));
                div().h(px(10.0)).bg(bg).child(text(&format!("{n}/{row}")))
            });
            page(0xffffff, div().children(rows))
        });
        assert!(drawn.bytes <= bytes, "{drawn:?}");
        assert!(drawn.entries <= entries, "{drawn:?}");
    }
    assert!(f.stats().evicted > 0);
}

/// macOS has neither a table nor LCD masks; either alone keeps every run as it was.
#[test]
fn without_a_table_or_masks_text_is_drawn_as_without_either() {
    let runs = |backend: Blocks, lut: Option<LcdGammaLut>| {
        let mut f = Fixture::new(2.0, backend, lut);
        let drawn = f.draw(|| {
            let row = div().bg(rgb(0xeeeeee)).child(text("row").underline());
            page(0xffffff, div().child(text("run")).child(row))
        });
        (drawn, f.stats())
    };
    let plain = runs(Blocks { lcd: false }, None);
    assert_eq!(
        (plain.0.rasterized, plain.0.lcd, plain.0.entries),
        (2, 0, 2)
    );
    assert_eq!(runs(Blocks { lcd: true }, None), plain);
    assert_eq!(runs(Blocks { lcd: false }, Some(lut())), plain);
}

#[test]
fn only_windows_and_linux_have_lcd_text() {
    let has = cfg!(any(windows, target_os = "linux"));
    assert_eq!(LcdGammaLut::embedded().is_some(), has);
    if cfg!(target_os = "macos") {
        let mut backend = asrs_canvas::text::platform_backend().expect("CoreText");
        let font = FontSpec::parse("14px system-ui").unwrap();
        for glyph in backend.shape_run(&font, "Text 123", 1.0).glyphs {
            assert_eq!(backend.raw_lcd_mask(glyph.glyph, 2.0, 1.0, 0), None);
        }
    }
}
