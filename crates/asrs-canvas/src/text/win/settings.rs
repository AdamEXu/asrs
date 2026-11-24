// Ported from Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! The text settings of the user that Chromium consults: `gfx::CachedFontRenderParams` and
//! `gfx::FontUtilWin`.

use std::ffi::c_void;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_PRIMARY_DEVICE, DISPLAY_DEVICEW, EnumDisplayDevicesW,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_DWORD, RegCloseKey, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FE_FONTSMOOTHINGCLEARTYPE, FE_FONTSMOOTHINGORIENTATIONBGR, FE_FONTSMOOTHINGORIENTATIONRGB,
    NONCLIENTMETRICSW, SPI_GETFONTSMOOTHING, SPI_GETFONTSMOOTHINGORIENTATION,
    SPI_GETFONTSMOOTHINGTYPE, SPI_GETNONCLIENTMETRICS, SYSTEM_PARAMETERS_INFO_ACTION,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{HSTRING, w};

use crate::text::lut::MaskGammaLut;

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
    pub contrast: f32,
    /// Skia's device gamma; 0 is sRGB.
    pub gamma: f32,
}

impl TextSettings {
    /// `SK_GAMMA_CONTRAST` and `SK_GAMMA_SRGB` of Chromium's Windows Skia, with ClearType on an
    /// RGB panel, as Windows ships.
    pub const DEFAULT: TextSettings = TextSettings {
        edging: Edging::SubpixelAntiAlias,
        bgr: false,
        contrast: MaskGammaLut::WINDOWS_CONTRAST,
        gamma: MaskGammaLut::WINDOWS_GAMMA,
    };

    /// The user's settings now.
    pub fn current() -> TextSettings {
        // SAFETY: a plain COM call.
        match unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) } {
            Ok(factory) => TextSettings::system(&factory),
            Err(_) => TextSettings::DEFAULT,
        }
    }

    pub fn system(factory: &IDWriteFactory) -> TextSettings {
        let mut settings = TextSettings::DEFAULT;
        let tuned = avalon_key().filter(key_exists);
        if let Some((contrast, gamma)) = tuned.as_ref().and_then(|_| rendering_params(factory)) {
            settings.contrast = contrast;
            settings.gamma = gamma;
        }
        settings.edging = Edging::Alias;
        if parameter(SPI_GETFONTSMOOTHING) == Some(0) {
            return settings;
        }
        settings.edging = Edging::AntiAlias;
        if parameter(SPI_GETFONTSMOOTHINGTYPE) != Some(FE_FONTSMOOTHINGCLEARTYPE) {
            return settings;
        }
        let structure = match tuned.as_ref().and_then(pixel_structure) {
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

fn key_exists(key: &HSTRING) -> bool {
    let mut handle = HKEY::default();
    // SAFETY: the out handle is closed once opened.
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, key, None, KEY_READ, &mut handle) != ERROR_SUCCESS {
            return false;
        }
        let _ = RegCloseKey(handle);
    }
    true
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

fn rendering_params(factory: &IDWriteFactory) -> Option<(f32, f32)> {
    // SAFETY: plain COM calls on a live factory.
    let (contrast, gamma) = unsafe {
        let params = factory.CreateRenderingParams().ok()?;
        (params.GetEnhancedContrast(), params.GetGamma())
    };
    let below_four = 4.0 - f32::EPSILON;
    Some((contrast.clamp(0.0, 1.0), gamma.clamp(0.0, below_four)))
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
