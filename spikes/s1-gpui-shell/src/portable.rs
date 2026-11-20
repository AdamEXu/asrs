use gpui_kit::{Pixels, Point, Window};

pub struct Native;

impl Native {
    #[allow(non_snake_case)]
    pub fn sendEvent(&self, _: &Native) {}
}

pub fn ns_view(_: &Window) -> Option<Native> {
    None
}

pub fn ns_window(_: &Window) -> Option<Native> {
    None
}

pub fn install_sidebar_vibrancy(_: &Window) -> bool {
    false
}

pub fn window_facts(window: &Window) -> String {
    format!(
        "{} scale_factor={} active={}",
        std::env::consts::OS,
        window.scale_factor(),
        window.is_window_active()
    )
}

pub fn dump_main_menu() -> String {
    format!("(no native menu dump on {})", std::env::consts::OS)
}

pub fn native_popup_probe(_: &Native, _: Point<Pixels>) -> String {
    String::new()
}

pub fn cg_capture_window(_: &Window) -> anyhow::Result<image::RgbaImage> {
    anyhow::bail!("CGWindowListCreateImage is macOS only")
}

pub fn make_scroll(_: &Native, _: bool, _: i32, _: bool) -> Option<(String, Native)> {
    None
}
