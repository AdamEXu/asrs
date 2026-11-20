/// One Blink `LayoutUnit`, stored as its raw 1/64 px value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LayoutUnit(pub i32);

impl LayoutUnit {
    /// `LayoutUnit::FromFloatRound`.
    pub fn from_float_round(v: f32) -> LayoutUnit {
        LayoutUnit(saturate((v * 64.0).round()))
    }

    /// The `LayoutUnit(float)` constructor, which truncates toward zero.
    pub fn from_float(v: f32) -> LayoutUnit {
        LayoutUnit(saturate(v * 64.0))
    }

    /// `LayoutUnit::Round`: halves go up.
    pub fn round(self) -> i32 {
        (i64::from(self.0) + 32).div_euclid(64) as i32
    }

    pub fn to_f32(self) -> f32 {
        self.0 as f32 / 64.0
    }
}

fn saturate(v: f32) -> i32 {
    if v.is_nan() { 0 } else { v as i32 }
}

/// Chromium's `clientWidth` / `clientHeight` for a box `size` logical px wide with no border,
/// padding or scrollbar: the device-space layout size divided by the zoom (which is the device
/// pixel ratio under zoom-for-DSF), truncated to a `LayoutUnit`, rounded half up.
pub fn client_size(size: f32, dpr: f32) -> i32 {
    let device = LayoutUnit::from_float_round(size * dpr);
    LayoutUnit::from_float(device.to_f32() / dpr).round()
}

/// One coordinate of `getBoundingClientRect()`: the device-space layout position divided by the
/// zoom in float.
pub fn client_rect_coord(position: f32, dpr: f32) -> f64 {
    f64::from(LayoutUnit::from_float_round(position * dpr).to_f32() / dpr)
}

/// The whole device pixel Chromium paints a box edge at.
pub fn device_snap(position: f32, dpr: f32) -> i32 {
    LayoutUnit::from_float_round(position * dpr).round()
}

/// `MouseEvent.clientX` / `clientY`: `std::floor` of the window position.
pub fn mouse_client(position: f32) -> f64 {
    f64::from(position).floor()
}

pub const DEFAULT_WIDTH: u32 = 300;
pub const DEFAULT_HEIGHT: u32 = 150;

/// The bitmap side that `canvas.width = value` (or `height`) produces: WebIDL `unsigned long`
/// conversion, then the reflected attribute's rule that values above 2^31 - 1 set the default.
pub fn bitmap_side(value: f64, default: u32) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let side = value.trunc().rem_euclid(4_294_967_296.0) as u32;
    if side > i32::MAX as u32 {
        default
    } else {
        side
    }
}

/// `CanvasRenderingContextHost::IsValidImageSize`: an invalid canvas draws nothing.
pub fn valid_bitmap(width: u32, height: u32) -> bool {
    const MAX_AREA: u64 = 32768 * 8192;
    const MAX_SIDE: u32 = 65535;
    width > 0
        && height > 0
        && width <= MAX_SIDE
        && height <= MAX_SIDE
        && u64::from(width) * u64::from(height) <= MAX_AREA
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_unit_round_goes_half_up() {
        assert_eq!(LayoutUnit(96).round(), 2);
        assert_eq!(LayoutUnit(95).round(), 1);
        assert_eq!(LayoutUnit(-96).round(), -1);
        assert_eq!(LayoutUnit(-97).round(), -2);
        assert_eq!(LayoutUnit(-32).round(), 0);
        assert_eq!(LayoutUnit(-33).round(), -1);
    }

    #[test]
    fn client_size_matches_blink() {
        assert_eq!(client_size(1100.0, 2.0), 1100);
        assert_eq!(client_size(100.5, 1.0), 101);
        assert_eq!(client_size(100.49, 1.0), 100);
        assert_eq!(client_size(100.25, 2.0), 100);
        assert_eq!(client_size(100.5, 2.0), 101);
        assert_eq!(client_size(333.0, 1.25), 333);
        assert_eq!(client_size(0.4, 1.0), 0);
    }

    #[test]
    fn client_rect_keeps_layout_precision() {
        assert_eq!(client_rect_coord(10.25, 1.0), 10.25);
        assert_eq!(client_rect_coord(10.0 + 1.0 / 128.0, 1.0), 10.015625);
        assert_eq!(client_rect_coord(10.25, 2.0), 10.25);
        assert_eq!(client_rect_coord(0.1, 1.25), f64::from(0.125f32 / 1.25));
    }

    #[test]
    fn device_snap_rounds_half_up() {
        assert_eq!(device_snap(10.5, 1.0), 11);
        assert_eq!(device_snap(10.25, 2.0), 21);
        assert_eq!(device_snap(10.2, 2.0), 20);
        assert_eq!(device_snap(-0.5, 1.0), 0);
    }

    #[test]
    fn mouse_client_floors() {
        assert_eq!(mouse_client(12.75), 12.0);
        assert_eq!(mouse_client(-0.25), -1.0);
        assert_eq!(mouse_client(7.0), 7.0);
    }

    #[test]
    fn bitmap_side_follows_webidl_and_reflection() {
        assert_eq!(bitmap_side(2200.0, DEFAULT_WIDTH), 2200);
        assert_eq!(bitmap_side(412.5, DEFAULT_WIDTH), 412);
        assert_eq!(bitmap_side(-0.5, DEFAULT_WIDTH), 0);
        assert_eq!(bitmap_side(-1.0, DEFAULT_WIDTH), DEFAULT_WIDTH);
        assert_eq!(bitmap_side(f64::NAN, DEFAULT_HEIGHT), 0);
        assert_eq!(bitmap_side(f64::INFINITY, DEFAULT_HEIGHT), 0);
        assert_eq!(bitmap_side(4_294_967_296.0 + 7.0, DEFAULT_HEIGHT), 7);
        assert_eq!(bitmap_side(2_147_483_648.0, DEFAULT_HEIGHT), DEFAULT_HEIGHT);
        assert_eq!(bitmap_side(2_147_483_647.0, DEFAULT_HEIGHT), 2_147_483_647);
    }

    #[test]
    fn bitmap_validity_matches_chromium_limits() {
        assert!(valid_bitmap(2200, 1300));
        assert!(!valid_bitmap(0, 10));
        assert!(!valid_bitmap(65536, 1));
        assert!(valid_bitmap(32768, 8192));
        assert!(!valid_bitmap(32768, 8193));
    }
}
