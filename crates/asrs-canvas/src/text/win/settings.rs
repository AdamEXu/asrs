// Ported from Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! The text settings of the user that Chromium consults: `gfx::CachedFontRenderParams` and
//! `gfx::FontUtilWin`.

use std::ffi::c_void;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, EnumDisplayDevicesW,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::{
    FE_FONTSMOOTHINGCLEARTYPE, FE_FONTSMOOTHINGORIENTATIONBGR, FE_FONTSMOOTHINGORIENTATIONRGB,
    NONCLIENTMETRICSW, SPI_GETFONTSMOOTHING, SPI_GETFONTSMOOTHINGORIENTATION,
    SPI_GETFONTSMOOTHINGTYPE, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_ACTION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{HSTRING, w};

/// `SkFont::Edging` as `FontPlatformData::CreateSkFont` sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edging {
    Alias,
    AntiAlias,
    SubpixelAntiAlias,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSettings {
    pub edging: Edging,
    /// The display's subpixels run blue, green, red.
    pub bgr: bool,
}

impl TextSettings {
    /// ClearType on an RGB panel, as Windows ships.
    pub const DEFAULT: TextSettings = TextSettings {
        edging: Edging::SubpixelAntiAlias,
        bgr: false,
    };

    /// The user's settings now.
    pub fn system() -> TextSettings {
        let mut settings = TextSettings {
            edging: Edging::Alias,
            ..TextSettings::DEFAULT
        };
        if parameter(SPI_GETFONTSMOOTHING) == Some(0) {
            return settings;
        }
        settings.edging = Edging::AntiAlias;
        if parameter(SPI_GETFONTSMOOTHINGTYPE) != Some(FE_FONTSMOOTHINGCLEARTYPE) {
            return settings;
        }
        let structure = match avalon_key().as_ref().and_then(pixel_structure) {
            Some(structure) => structure,
            None => match parameter(SPI_GETFONTSMOOTHINGORIENTATION) {
                Some(FE_FONTSMOOTHINGORIENTATIONRGB) => 1,
                Some(FE_FONTSMOOTHINGORIENTATIONBGR) => 2,
                _ => 0,
            },
        };
        if matches!(structure, 1 | 2) {
            settings.edging = Edging::SubpixelAntiAlias;
            settings.bgr = structure == 2;
        }
        settings
    }
}

fn parameter(action: SYSTEM_PARAMETERS_INFO_ACTION) -> Option<u32> {
    let mut value = 0u32;
    // SAFETY: each action used here writes one 32-bit value.
    unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some((&raw mut value).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()?;
    Some(value)
}

/// `FontUtilWin::GetTextSettingsRegistryKey`: where the ClearType tuner keeps the primary
/// display's settings.
fn avalon_key() -> Option<HSTRING> {
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..DISPLAY_DEVICEW::default()
        };
        // SAFETY: `device` is a writable DISPLAY_DEVICEW whose size is set.
        if !unsafe { EnumDisplayDevicesW(None, index, &mut device, 0) }.as_bool() {
            break;
        }
        if device.StateFlags.contains(DISPLAY_DEVICE_PRIMARY_DEVICE) {
            let name = wide(&device.DeviceName);
            let name = name.rsplit('\\').next().unwrap_or_default();
            return Some(format!(r"SOFTWARE\Microsoft\Avalon.Graphics\{name}").into());
        }
    }
    None
}

fn pixel_structure(key: &HSTRING) -> Option<u32> {
    let (mut value, mut size) = (0u32, size_of::<u32>() as u32);
    // SAFETY: the value is a DWORD, which fits `value`.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            w!("PixelStructure"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast::<c_void>()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

/// `FontCache::MenuFontFamily`, which `system-ui` stands for.
pub fn menu_font_family() -> Option<String> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..NONCLIENTMETRICSW::default()
    };
    // SAFETY: `metrics` is a writable NONCLIENTMETRICSW whose size is set.
    unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            Some((&raw mut metrics).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()?;
    Some(wide(&metrics.lfMenuFont.lfFaceName)).filter(|name| !name.is_empty())
}

/// A null-terminated UTF-16 buffer.
pub fn wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
