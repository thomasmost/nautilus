//! The application icon: the window icon (Windows title bar) and, on
//! macOS, the Dock icon, so `cargo run` shows the logo without an `.app`
//! bundle. Both come from the generated PNG checked in under `assets/`.

use winit::window::Icon;

const ICON_PNG: &[u8] = include_bytes!("../../../assets/icons/nautilus-256.png");

/// Decode the embedded PNG to straight RGBA8.
fn decode_icon() -> Result<(Vec<u8>, u32, u32), png::DecodingError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buffer)?;
    buffer.truncate(info.buffer_size());
    debug_assert_eq!(info.color_type, png::ColorType::Rgba);
    debug_assert_eq!(info.bit_depth, png::BitDepth::Eight);
    Ok((buffer, info.width, info.height))
}

/// The window icon. winit ignores it on macOS, where the Dock icon is
/// set separately.
pub fn window_icon() -> Option<Icon> {
    let (rgba, width, height) = match decode_icon() {
        Ok(decoded) => decoded,
        Err(e) => {
            log::warn!("cannot decode embedded icon: {e}");
            return None;
        }
    };
    match Icon::from_rgba(rgba, width, height) {
        Ok(icon) => Some(icon),
        Err(e) => {
            log::warn!("cannot build window icon: {e}");
            None
        }
    }
}

/// Set the Dock icon from the embedded PNG. Must run on the main thread
/// after the event loop (and so `NSApplication`) exists.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("set_dock_icon called off the main thread; skipping");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let data = NSData::with_bytes(ICON_PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        log::warn!("AppKit could not decode the embedded icon");
        return;
    };
    // SAFETY: on the main thread with a valid NSImage; AppKit retains it.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(not(target_os = "macos"))]
pub fn set_dock_icon() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes_to_256_square_rgba() {
        let (rgba, width, height) = decode_icon().unwrap();
        assert_eq!((width, height), (256, 256));
        assert_eq!(rgba.len(), 256 * 256 * 4);
    }
}
