//! Reports fatal startup errors. Windows release builds are GUI-subsystem
//! apps with no console, so stderr alone would hide them from the user.

/// Writes `message` to stderr and, in Windows release builds, also shows it
/// in a native message box titled "Nautilus". Most callers exit with their
/// own failure code afterwards; a window that fails to open while others are
/// still up is reported the same way and the app keeps running.
pub fn report_fatal(message: &str) {
    eprintln!("{message}");
    #[cfg(all(windows, not(debug_assertions)))]
    show_message_box(message);
}

#[cfg(all(windows, not(debug_assertions)))]
fn show_message_box(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    let text = to_wide(message);
    let caption = to_wide("Nautilus");
    // SAFETY: the parent is a null window, so the box is owned by no window.
    // `text` and `caption` are null-terminated UTF-16 buffers from `to_wide`,
    // and both live until `MessageBoxW` returns, which blocks until the user
    // dismisses the box.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Encodes `text` as UTF-16 with a trailing NUL, the form Win32 wide APIs take.
#[cfg(any(test, all(windows, not(debug_assertions))))]
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::to_wide;

    #[test]
    fn to_wide_appends_one_trailing_nul() {
        let wide = to_wide("Nautilus");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(wide.iter().filter(|&&c| c == 0).count(), 1);
        assert_eq!(wide.len(), "Nautilus".len() + 1);
    }

    #[test]
    fn to_wide_encodes_non_ascii_as_utf16() {
        // U+00E9 is one code unit; U+1F600 is a surrogate pair.
        assert_eq!(to_wide("\u{e9}"), vec![0x00e9, 0]);
        assert_eq!(to_wide("\u{1F600}"), vec![0xd83d, 0xde00, 0]);
    }

    #[test]
    fn to_wide_of_empty_is_just_nul() {
        assert_eq!(to_wide(""), vec![0]);
    }
}
