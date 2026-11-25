use asrs_canvas::text::{Family, FontSpec};

use crate::svg::Backend;
use crate::text::Align;
use crate::theme::Appearance;

/// `font: -webkit-small-control`, the UA font of `<input>`: 13.333 px Arial on macOS.
pub const CONTROL_FONT: FontSpec = FontSpec {
    family: Family::Arial,
    size: 40.0 / 3.0,
    weight: 400.0,
    italic: false,
    hinted: false,
};

/// [`CONTROL_FONT`] of a control in a page's initial HTML.
pub const PAGE_CONTROL_FONT: FontSpec = FontSpec {
    hinted: true,
    ..CONTROL_FONT
};

/// `body { font-family }` of `global.css` at the given size.
pub const fn system_font(size: f32) -> FontSpec {
    FontSpec {
        family: Family::System,
        size,
        weight: 400.0,
        italic: false,
        hinted: false,
    }
}

/// Chromium's `::placeholder` colour under `color-scheme: normal`, which no upstream page
/// changes, so it holds in dark mode too.
pub const PLACEHOLDER: u32 = 0x757575ff;
pub const NATIVE_BORDER: u32 = 0x767676ff;
/// The native theme's disabled text field and menu list, and the UA colour of disabled text.
pub const DISABLED_BORDER: u32 = 0x7676764d;
pub const DISABLED_BACKGROUND: u32 = 0xffffff99;
pub const LIGHTEN_LAYER: u32 = 0xa9a9a933;
pub const DISABLED_TEXT: u32 = 0x545454ff;
pub const NATIVE_RADIUS: f32 = 2.0;
pub const ACCENT: u32 = 0x0075ffff;
pub const NATIVE_FILL: u32 = 0xefefefff;
pub const NATIVE_TRACK_BORDER: u32 = match Backend::HOST {
    Backend::Graphite => 0x7676768f,
    Backend::Raster => 0x76767680,
};

/// How the border box is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    /// `appearance: auto`: Chromium's native text field, a white rounded rect inside a
    /// `#767676` stroke one zoomed pixel wide, radius 2 px, whatever the layout border is.
    Native,
    /// An author border and background, which turn the native appearance off.
    Css {
        border: u32,
        background: u32,
        radius: f32,
    },
}

/// A single-line `<input>` as one upstream stylesheet rule set styles it. Colours are
/// `0xRRGGBBAA`; lengths are CSS px before zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldStyle {
    pub font: FontSpec,
    pub align: Align,
    /// The layout border width; `Frame::Native` paints its own one pixel stroke over it.
    pub border: f32,
    /// Top, right, bottom, left.
    pub padding: [f32; 4],
    pub frame: Frame,
    pub color: u32,
    pub placeholder: u32,
    pub placeholder_opacity: f32,
    pub focus_background: Option<u32>,
    /// `::-webkit-search-cancel-button` restyled as `symbols/cancel.svg`, one `em` square.
    pub cancel_button: bool,
    pub cancel_filter: &'static str,
    /// Whether a number input has `::-webkit-inner-spin-button`.
    pub spin_button: bool,
}

const UA_PADDING: [f32; 4] = [1.0, 2.0, 1.0, 2.0];

impl FieldStyle {
    /// The UA defaults for a text, number or search input that no author rule restyles.
    pub const NATIVE: FieldStyle = FieldStyle {
        font: CONTROL_FONT,
        align: Align::Start,
        border: 2.0,
        padding: UA_PADDING,
        frame: Frame::Native,
        color: 0x000000ff,
        placeholder: PLACEHOLDER,
        placeholder_opacity: 1.0,
        focus_background: None,
        cancel_button: false,
        cancel_filter: "",
        spin_button: true,
    };

    /// `input.side-bar-search` in `hub.css`.
    pub fn sidebar_search(appearance: Appearance) -> FieldStyle {
        let dark = appearance == Appearance::Dark;
        FieldStyle {
            font: FontSpec {
                hinted: true,
                ..system_font(13.0)
            },
            align: Align::Start,
            border: 0.5,
            padding: [5.0, 5.0, 5.0, 28.0],
            frame: Frame::Css {
                border: if dark { 0xffffff4d } else { 0xb7b7b74d },
                background: if dark { 0xffffff0d } else { 0x0000000d },
                radius: 6.0,
            },
            color: if dark { 0xffffffff } else { 0x000000ff },
            placeholder: if dark { 0xffffff40 } else { 0x00000040 },
            placeholder_opacity: 1.0,
            focus_background: None,
            cancel_button: true,
            cancel_filter: if dark {
                crate::theme::chains::INVERT
            } else {
                ""
            },
            spin_button: false,
        }
    }

    /// `div.field-value input` in `hub.css`: the sidebar's tuning value.
    pub fn field_value() -> FieldStyle {
        FieldStyle {
            border: 1.0,
            frame: Frame::Css {
                border: 0x888888ff,
                background: 0xffffffff,
                radius: 2.0,
            },
            placeholder_opacity: 0.6,
            focus_background: Some(0xffffaaff),
            spin_button: false,
            ..FieldStyle::NATIVE
        }
    }

    /// `div.setting-blocks input` in `hub.css`: centred text in the tab controls.
    pub fn setting_block() -> FieldStyle {
        FieldStyle {
            align: Align::Center,
            spin_button: false,
            ..FieldStyle::NATIVE
        }
    }

    /// The jump-to-time inputs of `renderers.css`.
    pub fn native() -> FieldStyle {
        FieldStyle::NATIVE
    }

    /// `input` in `prompts.css`.
    pub fn prompt() -> FieldStyle {
        FieldStyle {
            font: PAGE_CONTROL_FONT,
            ..FieldStyle::NATIVE
        }
    }

    /// `table.console-table input.filter` in `renderers.css`.
    pub fn console_filter() -> FieldStyle {
        FieldStyle {
            padding: [9.0, 2.0, 9.0, 2.0],
            cancel_button: true,
            ..FieldStyle::NATIVE
        }
    }
}

/// The two looks of `div.setting-blocks div.split` in `hub.css`, a segment at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitStyle {
    pub background: u32,
    pub border: u32,
    pub hover: u32,
    pub selected: u32,
    pub selected_border: u32,
    pub badge: u32,
    pub badge_text: u32,
    pub badge_hover_text: u32,
    pub badge_selected_text: u32,
    pub invert_icons: bool,
}

impl SplitStyle {
    pub fn of(appearance: Appearance) -> SplitStyle {
        appearance.pick(
            SplitStyle {
                background: 0xeeeeee,
                border: 0xaaaaaa,
                hover: 0xdddddd,
                selected: 0xaaaaaa,
                selected_border: 0x222222,
                badge: 0x000000,
                badge_text: 0xeeeeee,
                badge_hover_text: 0xdddddd,
                badge_selected_text: 0xaaaaaa,
                invert_icons: false,
            },
            SplitStyle {
                background: 0x444444,
                border: 0x666666,
                hover: 0x555555,
                selected: 0x888888,
                selected_border: 0xeeeeee,
                badge: 0xffffff,
                badge_text: 0x444444,
                badge_hover_text: 0x555555,
                badge_selected_text: 0x888888,
                invert_icons: true,
            },
        )
    }
}

pub const SPLIT_RADIUS: f32 = 10.0;
pub const SPLIT_BADGE: f32 = 18.0;
pub const SPLIT_BADGE_RADIUS: f32 = 3.0;
pub const SPLIT_BADGE_OPACITY: f32 = 0.85;
pub const SPLIT_ICON_FRACTION: f32 = 0.6;

/// `input.video-timeline-slider` in `hub.css`: `appearance: none`, a `#aaa` track and a black
/// 4 x 25 px thumb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderStyle {
    pub track: u32,
    pub thumb: u32,
    pub thumb_width: f32,
}

impl SliderStyle {
    pub const VIDEO_TIMELINE: SliderStyle = SliderStyle {
        track: 0xaaaaaa,
        thumb: 0x000000,
        thumb_width: 4.0,
    };
}

/// A CSS border width as Blink lays it out at device scale `scale`: below one device pixel it
/// rounds up to one, otherwise it is floored to whole device pixels. Returned in device px.
pub fn device_border(width: f32, scale: f32) -> f32 {
    let zoomed = width * scale;
    if zoomed > 0.0 && zoomed < 1.0 {
        1.0
    } else {
        zoomed.floor()
    }
}

/// `AdjustBorderWidthByZoom(1, zoom)`: the native theme's stroke in device px.
pub fn native_stroke(scale: f32) -> f32 {
    scale.floor().max(1.0)
}
