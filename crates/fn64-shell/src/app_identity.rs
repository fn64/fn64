//! Player-facing shell identity: window title and fn64 application icon.
//!
//! A title belongs to the linked game profile, not to the generic shell.
//! `build.rs` therefore bakes `FN64_APP_TITLE` into the binary that consumed
//! it; changing the process environment after compilation cannot make an old
//! binary claim a new identity. The WM2000 runner supplies the title-specific
//! value, while content-free and other-title builds retain the generic name.
//!
//! The icon geometry is the controller-port mark published by fn64.github.io.
//! The checked-in SVG is the editable authority; the RGBA file beside it is a
//! deterministic 256x256 raster used directly so window creation needs no
//! image decoder or filesystem lookup.

use winit::window::Icon;

const GENERIC_WINDOW_TITLE: &str = "fn64 -- N64 recompilation";
const ICON_WIDTH: u32 = 256;
const ICON_HEIGHT: u32 = 256;
const ICON_RGBA: &[u8] = include_bytes!("../assets/fn64-app-icon.rgba");

pub const fn title(configured: Option<&'static str>) -> &'static str {
    match configured {
        Some(title) => title,
        None => GENERIC_WINDOW_TITLE,
    }
}

/// The title this binary was built with.
pub const WINDOW_TITLE: &str = title(option_env!("FN64_APP_TITLE"));

/// Construct the titlebar/taskbar icon supported by Windows and X11.
pub fn window_icon() -> Icon {
    Icon::from_rgba(ICON_RGBA.to_vec(), ICON_WIDTH, ICON_HEIGHT)
        .expect("fn64-shell: embedded app icon dimensions must match its RGBA byte count")
}

/// Install the process-wide Dock icon on macOS.
///
/// winit deliberately ignores `Window::set_window_icon` on macOS because
/// macOS has application icons rather than per-window icons. The native call
/// is therefore a required second half of the cross-platform mechanism.
#[cfg(target_os = "macos")]
pub fn install_platform_application_icon() {
    use std::ffi::c_uchar;
    use std::slice;

    use objc2::ClassType;
    use objc2_app_kit::{NSApplication, NSBitmapImageRep, NSDeviceRGBColorSpace, NSImage};
    use objc2_foundation::{MainThreadMarker, NSSize};

    let mtm = MainThreadMarker::new()
        .expect("fn64-shell: application icon installation must run on the main thread");
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut::<*mut c_uchar>(),
            ICON_WIDTH as isize,
            ICON_HEIGHT as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            ICON_WIDTH as isize * 4,
            32,
        )
        .expect("fn64-shell: AppKit refused the embedded app-icon bitmap")
    };
    let destination = unsafe { slice::from_raw_parts_mut(bitmap.bitmapData(), ICON_RGBA.len()) };
    destination.copy_from_slice(ICON_RGBA);

    let image = unsafe {
        NSImage::initWithSize(
            NSImage::alloc(),
            NSSize::new(ICON_WIDTH.into(), ICON_HEIGHT.into()),
        )
    };
    unsafe {
        image.addRepresentation(&bitmap);
        NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install_platform_application_icon() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_title_is_the_player_facing_title() {
        assert_eq!(
            title(Some("WrestleMania 2000 [built with fn64]")),
            "WrestleMania 2000 [built with fn64]"
        );
        assert_eq!(title(None), GENERIC_WINDOW_TITLE);
    }

    #[test]
    fn embedded_icon_is_complete_rgba_and_constructible() {
        assert_eq!(
            ICON_RGBA.len(),
            ICON_WIDTH as usize * ICON_HEIGHT as usize * 4
        );
        let _ = window_icon();
    }
}
