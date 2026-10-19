//! Physical key codes (`keyboard-types::Code`) to the platform key codes
//! CEF's key events carry: Windows virtual-key codes (`VK_*`, which
//! Chromium uses on every platform as `windows_key_code`) and the native
//! code of the platform (the macOS virtual key code; unused on Windows,
//! where Chromium reads the virtual-key code).

use keyboard_types::{Code, Key};

/// The Windows virtual-key code for a key. Letters and digits are their
/// ASCII values; punctuation uses the US-layout `VK_OEM_*` codes.
pub(crate) fn windows_key_code(code: Code, key: &Key) -> i32 {
    // A typed ASCII letter or digit names its VK directly, whatever the
    // layout; this keeps Ctrl+<letter> chords right on non-US layouts.
    if let Key::Character(s) = key
        && let [c] = s.as_bytes()
    {
        if c.is_ascii_alphabetic() {
            return i32::from(c.to_ascii_uppercase());
        }
        if c.is_ascii_digit() {
            return i32::from(*c);
        }
    }
    match code {
        Code::KeyA => 0x41,
        Code::KeyB => 0x42,
        Code::KeyC => 0x43,
        Code::KeyD => 0x44,
        Code::KeyE => 0x45,
        Code::KeyF => 0x46,
        Code::KeyG => 0x47,
        Code::KeyH => 0x48,
        Code::KeyI => 0x49,
        Code::KeyJ => 0x4A,
        Code::KeyK => 0x4B,
        Code::KeyL => 0x4C,
        Code::KeyM => 0x4D,
        Code::KeyN => 0x4E,
        Code::KeyO => 0x4F,
        Code::KeyP => 0x50,
        Code::KeyQ => 0x51,
        Code::KeyR => 0x52,
        Code::KeyS => 0x53,
        Code::KeyT => 0x54,
        Code::KeyU => 0x55,
        Code::KeyV => 0x56,
        Code::KeyW => 0x57,
        Code::KeyX => 0x58,
        Code::KeyY => 0x59,
        Code::KeyZ => 0x5A,
        Code::Digit0 => 0x30,
        Code::Digit1 => 0x31,
        Code::Digit2 => 0x32,
        Code::Digit3 => 0x33,
        Code::Digit4 => 0x34,
        Code::Digit5 => 0x35,
        Code::Digit6 => 0x36,
        Code::Digit7 => 0x37,
        Code::Digit8 => 0x38,
        Code::Digit9 => 0x39,
        Code::Backspace => 0x08,
        Code::Tab => 0x09,
        Code::Enter | Code::NumpadEnter => 0x0D,
        Code::ShiftLeft | Code::ShiftRight => 0x10,
        Code::ControlLeft | Code::ControlRight => 0x11,
        Code::AltLeft | Code::AltRight => 0x12,
        Code::Pause => 0x13,
        Code::CapsLock => 0x14,
        Code::Escape => 0x1B,
        Code::Space => 0x20,
        Code::PageUp => 0x21,
        Code::PageDown => 0x22,
        Code::End => 0x23,
        Code::Home => 0x24,
        Code::ArrowLeft => 0x25,
        Code::ArrowUp => 0x26,
        Code::ArrowRight => 0x27,
        Code::ArrowDown => 0x28,
        Code::PrintScreen => 0x2C,
        Code::Insert => 0x2D,
        Code::Delete => 0x2E,
        Code::MetaLeft => 0x5B,
        Code::MetaRight => 0x5C,
        Code::ContextMenu => 0x5D,
        Code::Numpad0 => 0x60,
        Code::Numpad1 => 0x61,
        Code::Numpad2 => 0x62,
        Code::Numpad3 => 0x63,
        Code::Numpad4 => 0x64,
        Code::Numpad5 => 0x65,
        Code::Numpad6 => 0x66,
        Code::Numpad7 => 0x67,
        Code::Numpad8 => 0x68,
        Code::Numpad9 => 0x69,
        Code::NumpadMultiply => 0x6A,
        Code::NumpadAdd => 0x6B,
        Code::NumpadSubtract => 0x6D,
        Code::NumpadDecimal => 0x6E,
        Code::NumpadDivide => 0x6F,
        Code::F1 => 0x70,
        Code::F2 => 0x71,
        Code::F3 => 0x72,
        Code::F4 => 0x73,
        Code::F5 => 0x74,
        Code::F6 => 0x75,
        Code::F7 => 0x76,
        Code::F8 => 0x77,
        Code::F9 => 0x78,
        Code::F10 => 0x79,
        Code::F11 => 0x7A,
        Code::F12 => 0x7B,
        Code::NumLock => 0x90,
        Code::ScrollLock => 0x91,
        Code::Semicolon => 0xBA,
        Code::Equal => 0xBB,
        Code::Comma => 0xBC,
        Code::Minus => 0xBD,
        Code::Period => 0xBE,
        Code::Slash => 0xBF,
        Code::Backquote => 0xC0,
        Code::BracketLeft => 0xDB,
        Code::Backslash => 0xDC,
        Code::BracketRight => 0xDD,
        Code::Quote => 0xDE,
        Code::IntlBackslash => 0xE2,
        _ => 0,
    }
}

/// The macOS virtual key code (`kVK_*` in `HIToolbox/Events.h`).
#[cfg(target_os = "macos")]
pub(crate) fn native_key_code(code: Code) -> i32 {
    match code {
        Code::KeyA => 0x00,
        Code::KeyS => 0x01,
        Code::KeyD => 0x02,
        Code::KeyF => 0x03,
        Code::KeyH => 0x04,
        Code::KeyG => 0x05,
        Code::KeyZ => 0x06,
        Code::KeyX => 0x07,
        Code::KeyC => 0x08,
        Code::KeyV => 0x09,
        Code::IntlBackslash => 0x0A,
        Code::KeyB => 0x0B,
        Code::KeyQ => 0x0C,
        Code::KeyW => 0x0D,
        Code::KeyE => 0x0E,
        Code::KeyR => 0x0F,
        Code::KeyY => 0x10,
        Code::KeyT => 0x11,
        Code::Digit1 => 0x12,
        Code::Digit2 => 0x13,
        Code::Digit3 => 0x14,
        Code::Digit4 => 0x15,
        Code::Digit6 => 0x16,
        Code::Digit5 => 0x17,
        Code::Equal => 0x18,
        Code::Digit9 => 0x19,
        Code::Digit7 => 0x1A,
        Code::Minus => 0x1B,
        Code::Digit8 => 0x1C,
        Code::Digit0 => 0x1D,
        Code::BracketRight => 0x1E,
        Code::KeyO => 0x1F,
        Code::KeyU => 0x20,
        Code::BracketLeft => 0x21,
        Code::KeyI => 0x22,
        Code::KeyP => 0x23,
        Code::Enter => 0x24,
        Code::KeyL => 0x25,
        Code::KeyJ => 0x26,
        Code::Quote => 0x27,
        Code::KeyK => 0x28,
        Code::Semicolon => 0x29,
        Code::Backslash => 0x2A,
        Code::Comma => 0x2B,
        Code::Slash => 0x2C,
        Code::KeyN => 0x2D,
        Code::KeyM => 0x2E,
        Code::Period => 0x2F,
        Code::Tab => 0x30,
        Code::Space => 0x31,
        Code::Backquote => 0x32,
        Code::Backspace => 0x33,
        Code::Escape => 0x35,
        Code::MetaRight => 0x36,
        Code::MetaLeft => 0x37,
        Code::ShiftLeft => 0x38,
        Code::CapsLock => 0x39,
        Code::AltLeft => 0x3A,
        Code::ControlLeft => 0x3B,
        Code::ShiftRight => 0x3C,
        Code::AltRight => 0x3D,
        Code::ControlRight => 0x3E,
        Code::F17 => 0x40,
        Code::NumpadDecimal => 0x41,
        Code::NumpadMultiply => 0x43,
        Code::NumpadAdd => 0x45,
        Code::NumLock => 0x47,
        Code::NumpadDivide => 0x4B,
        Code::NumpadEnter => 0x4C,
        Code::NumpadSubtract => 0x4E,
        Code::F18 => 0x4F,
        Code::F19 => 0x50,
        Code::NumpadEqual => 0x51,
        Code::Numpad0 => 0x52,
        Code::Numpad1 => 0x53,
        Code::Numpad2 => 0x54,
        Code::Numpad3 => 0x55,
        Code::Numpad4 => 0x56,
        Code::Numpad5 => 0x57,
        Code::Numpad6 => 0x58,
        Code::Numpad7 => 0x59,
        Code::F20 => 0x5A,
        Code::Numpad8 => 0x5B,
        Code::Numpad9 => 0x5C,
        Code::F5 => 0x60,
        Code::F6 => 0x61,
        Code::F7 => 0x62,
        Code::F3 => 0x63,
        Code::F8 => 0x64,
        Code::F9 => 0x65,
        Code::F11 => 0x67,
        Code::F13 => 0x69,
        Code::F16 => 0x6A,
        Code::F14 => 0x6B,
        Code::F10 => 0x6D,
        Code::ContextMenu => 0x6E,
        Code::F12 => 0x6F,
        Code::F15 => 0x71,
        Code::Help => 0x72,
        Code::Home => 0x73,
        Code::PageUp => 0x74,
        Code::Delete => 0x75,
        Code::F4 => 0x76,
        Code::End => 0x77,
        Code::F2 => 0x78,
        Code::PageDown => 0x79,
        Code::F1 => 0x7A,
        Code::ArrowLeft => 0x7B,
        Code::ArrowRight => 0x7C,
        Code::ArrowDown => 0x7D,
        Code::ArrowUp => 0x7E,
        _ => 0,
    }
}

/// Windows: Chromium reads the virtual-key code; the native code (the
/// `WM_KEYDOWN` `lParam`) is only consulted for the extended-key and
/// repeat bits, which the event flags already carry.
#[cfg(not(target_os = "macos"))]
pub(crate) fn native_key_code(_code: Code) -> i32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_types::NamedKey;

    #[test]
    fn letters_and_digits_use_their_ascii_codes() {
        assert_eq!(
            windows_key_code(Code::KeyQ, &Key::Character("q".into())),
            0x51
        );
        assert_eq!(
            windows_key_code(Code::KeyQ, &Key::Character("Q".into())),
            0x51
        );
        assert_eq!(
            windows_key_code(Code::Digit7, &Key::Character("7".into())),
            0x37
        );
    }

    #[test]
    fn a_typed_letter_wins_over_the_physical_key() {
        // AZERTY: the key at the QWERTY "Q" position types "a".
        assert_eq!(
            windows_key_code(Code::KeyQ, &Key::Character("a".into())),
            0x41
        );
    }

    #[test]
    fn named_keys_map_to_their_virtual_keys() {
        assert_eq!(
            windows_key_code(Code::Enter, &Key::Named(NamedKey::Enter)),
            0x0D
        );
        assert_eq!(
            windows_key_code(Code::ArrowLeft, &Key::Named(NamedKey::ArrowLeft)),
            0x25
        );
        assert_eq!(windows_key_code(Code::F5, &Key::Named(NamedKey::F5)), 0x74);
        assert_eq!(
            windows_key_code(Code::Comma, &Key::Character(",".into())),
            0xBC
        );
    }
}
