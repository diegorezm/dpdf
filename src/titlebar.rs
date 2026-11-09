//! Paints the native title bar in the theme's colors. Uses the DWM
//! attributes added in Windows 11; on older systems the calls fail
//! harmlessly and the default title bar stays.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::Color;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_BORDER_COLOR: u32 = 34;
const DWMWA_CAPTION_COLOR: u32 = 35;
const DWMWA_TEXT_COLOR: u32 = 36;

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: isize,
        attribute: u32,
        value: *const core::ffi::c_void,
        size: u32,
    ) -> i32;
}

/// Colors the title bar of `window`. Returns false if the native window
/// doesn't exist yet (it is created shortly after the window is shown).
pub fn paint(
    window: &slint::Window,
    caption: Color,
    text: Color,
    border: Color,
    dark: bool,
) -> bool {
    let handle = window.window_handle();
    let Ok(handle) = handle.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return false;
    };
    let hwnd = win32.hwnd.get();
    set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, i32::from(dark));
    set(hwnd, DWMWA_CAPTION_COLOR, colorref(caption));
    set(hwnd, DWMWA_TEXT_COLOR, colorref(text));
    set(hwnd, DWMWA_BORDER_COLOR, colorref(border));
    true
}

fn set<T>(hwnd: isize, attribute: u32, value: T) {
    // SAFETY: `hwnd` is a live window handle from Slint, and `value` is a
    // plain integer whose size we pass, as DwmSetWindowAttribute expects.
    // Failure (e.g. on Windows 10) only means the attribute isn't applied.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            (&value as *const T).cast(),
            std::mem::size_of::<T>() as u32,
        );
    }
}

/// COLORREF is 0x00BBGGRR.
fn colorref(color: Color) -> u32 {
    u32::from(color.red()) | u32::from(color.green()) << 8 | u32::from(color.blue()) << 16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(Color::from_rgb_u8(0x12, 0x34, 0x56)), 0x0056_3412);
    }
}
