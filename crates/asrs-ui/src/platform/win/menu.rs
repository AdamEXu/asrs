//! Win32 popup menus.

use windows::Win32::Foundation::{HWND, POINT, TRUE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, ClientToScreen, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, GetMenuItemCount, HMENU, InsertMenuItemW,
    MENUITEMINFOW, MFS_CHECKED, MFS_DISABLED, MFT_SEPARATOR, MFT_STRING, MIIM_BITMAP, MIIM_FTYPE,
    MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    TPM_TOPALIGN, TrackPopupMenuEx,
};
use windows::core::{HSTRING, PWSTR};

use super::super::{MenuPath, PopupEntry, PopupItem, PopupMenu, SWATCH_SIZE};

pub struct NativeMenu {
    pub handle: HMENU,
    /// The entries without submenus, whose command ids are their positions here plus one.
    paths: Vec<MenuPath>,
    bitmaps: Vec<HBITMAP>,
}

impl NativeMenu {
    pub fn build(menu: &PopupMenu) -> Option<NativeMenu> {
        // SAFETY: no preconditions.
        let handle = unsafe { CreatePopupMenu() }.ok()?;
        let mut native = NativeMenu {
            handle,
            paths: Vec::new(),
            bitmaps: Vec::new(),
        };
        native.fill(handle, menu, &mut Vec::new())?;
        Some(native)
    }

    fn fill(&mut self, into: HMENU, menu: &PopupMenu, prefix: &mut MenuPath) -> Option<()> {
        for (index, item) in menu.items.iter().enumerate() {
            prefix.push(index);
            match item {
                PopupItem::Separator => insert(
                    into,
                    MENUITEMINFOW {
                        fMask: MIIM_FTYPE,
                        fType: MFT_SEPARATOR,
                        ..Default::default()
                    },
                )?,
                PopupItem::Entry(entry) if entry.visible => self.entry(into, entry, prefix)?,
                PopupItem::Entry(_) => {}
            }
            prefix.pop();
        }
        Some(())
    }

    fn entry(&mut self, into: HMENU, entry: &PopupEntry, prefix: &mut MenuPath) -> Option<()> {
        let text = match &entry.shortcut {
            Some(shortcut) => HSTRING::from(format!("{}\t{shortcut}", entry.label)),
            None => HSTRING::from(&entry.label),
        };
        let mut info = MENUITEMINFOW {
            fMask: MIIM_FTYPE | MIIM_STATE | MIIM_STRING,
            fType: MFT_STRING,
            dwTypeData: PWSTR(text.as_ptr().cast_mut()),
            ..Default::default()
        };
        if !entry.enabled {
            info.fState |= MFS_DISABLED;
        }
        if entry.checked {
            info.fState |= MFS_CHECKED;
        }
        if let Some(rgb) = entry.swatch {
            let bitmap = swatch(rgb)?;
            self.bitmaps.push(bitmap);
            info.fMask |= MIIM_BITMAP;
            info.hbmpItem = bitmap;
        }
        let Some(submenu) = &entry.submenu else {
            self.paths.push(prefix.clone());
            info.fMask |= MIIM_ID;
            info.wID = self.paths.len() as u32;
            return insert(into, info);
        };
        // SAFETY: no preconditions.
        let handle = unsafe { CreatePopupMenu() }.ok()?;
        info.fMask |= MIIM_SUBMENU;
        info.hSubMenu = handle;
        if insert(into, info).is_none() {
            // SAFETY: the menu was created above and belongs to no other.
            unsafe { DestroyMenu(handle) }.ok();
            return None;
        }
        self.fill(handle, submenu, prefix)
    }

    /// Runs the menu at client coordinates of `owner` (device px), or at the pointer.
    pub fn track(&self, owner: HWND, at: Option<(i32, i32)>) -> Option<MenuPath> {
        let mut point = POINT::default();
        // SAFETY: `point` outlives the calls; a closed `owner` makes them fail.
        let chosen = unsafe {
            match at {
                Some((x, y)) => {
                    point = POINT { x, y };
                    ClientToScreen(owner, &mut point).ok().ok()?;
                }
                None => GetCursorPos(&mut point).ok()?,
            }
            let flags = TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD;
            TrackPopupMenuEx(self.handle, flags.0, point.x, point.y, owner, None)
        };
        let id = usize::try_from(chosen.0).ok()?;
        self.paths.get(id.checked_sub(1)?).cloned()
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        // SAFETY: the menu, its submenus and their bitmaps are this value's.
        unsafe {
            DestroyMenu(self.handle).ok();
            for bitmap in &self.bitmaps {
                let _ = DeleteObject((*bitmap).into());
            }
        }
    }
}

fn insert(into: HMENU, mut info: MENUITEMINFOW) -> Option<()> {
    info.cbSize = size_of::<MENUITEMINFOW>() as u32;
    // SAFETY: the menu is open and `info`'s text outlives the call.
    unsafe {
        let count = u32::try_from(GetMenuItemCount(Some(into))).ok()?;
        InsertMenuItemW(into, count, TRUE.as_bool(), &info).ok()
    }
}

fn swatch([r, g, b]: [u8; 3]) -> Option<HBITMAP> {
    let side = SWATCH_SIZE as i32;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side,
            biHeight: -side,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: the section has `side * side` 32-bit pixels, all written before it is used.
    unsafe {
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            return None;
        }
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (side * side) as usize);
        pixels.fill(u32::from_be_bytes([0xff, r, g, b]));
        Some(bitmap)
    }
}
