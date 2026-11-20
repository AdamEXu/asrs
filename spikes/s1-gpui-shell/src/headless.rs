use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{AppContext, HeadlessAppContext, px, size};

use crate::shell::Hub;

pub fn run(out: PathBuf) {
    std::fs::create_dir_all(&out).unwrap();
    let t = Instant::now();
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
        gpui_kit::platform::current_headless_renderer()
    });
    println!("headless context created in {:?}", t.elapsed());
    let handle = cx
        .open_window(size(px(1100.0), px(650.0)), |_, cx| {
            cx.new(|_| Hub {
                popup_at: None,
                custom_assets: false,
                wheel_log: Default::default(),
                action_log: Default::default(),
                focus: None,
            })
        })
        .unwrap();
    cx.run_until_parked();
    let mut prev: Option<image::RgbaImage> = None;
    for i in 0..3 {
        let _ = cx.update_window(handle.into(), |_, window, cx| {
            window.refresh();
            let _ = cx;
        });
        cx.run_until_parked();
        let t = Instant::now();
        let img = cx.capture_screenshot(handle.into()).unwrap();
        let el = t.elapsed();
        let same = prev.as_ref().map(|p| p.as_raw() == img.as_raw());
        println!(
            "headless capture {i}: {}x{} in {el:?}, identical to previous: {same:?}",
            img.width(),
            img.height()
        );
        if i == 0 {
            img.save(out.join("headless_hub.png")).unwrap();
        }
        prev = Some(img);
    }
    let scale = cx
        .update_window(handle.into(), |_, window, _| window.scale_factor())
        .unwrap();
    println!("headless scale_factor={scale}");
}

struct ImageProbe {
    img: Arc<gpui_kit::RenderImage>,
    offset: f32,
}

impl gpui_kit::Render for ImageProbe {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        _: &mut gpui_kit::Context<'_, Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        let img = self.img.clone();
        let off = self.offset;
        gpui_kit::div().size_full().bg(gpui_kit::rgb(0x808080)).child(
            gpui_kit::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let b = gpui_kit::Bounds::new(
                        gpui_kit::point(bounds.origin.x + px(off), bounds.origin.y + px(off)),
                        size(px(200.0), px(100.0)),
                    );
                    window
                        .paint_image(b, b, gpui_kit::Corners::default(), img, 0, false)
                        .unwrap();
                },
            )
            .size_full(),
        )
    }
}

pub fn image_exactness() {
    let text_system = gpui_kit::platform::current_platform(true).text_system();
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
        gpui_kit::platform::current_headless_renderer()
    });
    let (w, h) = (400u32, 200u32);
    let mut rgba = image::RgbaImage::new(w, h);
    let mut seed = 0x12345678u32;
    for p in rgba.pixels_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let a = if seed & 0x100 != 0 { 255 } else { (seed >> 24) as u8 };
        *p = image::Rgba([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, a]);
    }
    let mut bgra = rgba.clone().into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let img = Arc::new(gpui_kit::RenderImage::new(smallvec::smallvec![image::Frame::new(
        image::ImageBuffer::from_raw(w, h, bgra).unwrap()
    )]));
    for offset in [10.0f32, 10.25, 10.5] {
        let img = img.clone();
        let handle = cx
            .open_window(size(px(300.0), px(200.0)), move |_, cx| {
                cx.new(|_| ImageProbe { img, offset })
            })
            .unwrap();
        cx.run_until_parked();
        let shot = cx.capture_screenshot(handle.into()).unwrap();
        let o = (offset * 2.0).floor() as u32;
        let (mut exact_opaque, mut n_opaque, mut maxdiff_opaque, mut maxdiff_alpha) = (0, 0, 0i32, 0i32);
        for y in 0..h {
            for x in 0..w {
                let s = rgba.get_pixel(x, y).0;
                let d = shot.get_pixel(x + o, y + o).0;
                let a = s[3] as f32 / 255.0;
                let exp: Vec<i32> = (0..3)
                    .map(|i| (s[i] as f32 * a + 128.0 * (1.0 - a)).round() as i32)
                    .collect();
                let diff = (0..3).map(|i| (exp[i] - d[i] as i32).abs()).max().unwrap();
                if s[3] == 255 {
                    n_opaque += 1;
                    if diff == 0 {
                        exact_opaque += 1;
                    }
                    maxdiff_opaque = maxdiff_opaque.max(diff);
                } else {
                    maxdiff_alpha = maxdiff_alpha.max(diff);
                }
            }
        }
        println!(
            "image at logical offset {offset} (device {}): opaque px exact {exact_opaque}/{n_opaque}, max channel diff opaque={maxdiff_opaque}, translucent (straight alpha over #808080)={maxdiff_alpha}",
            offset * 2.0
        );
    }
}
