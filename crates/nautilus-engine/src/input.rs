//! Conversion of the browser's input events (`osmena_core::EngineInput`)
//! to CEF's. The shell builds the events from winit; this keeps winit
//! out of the engine and CEF out of the shell.

use keyboard_types::{Key, KeyState, Location, Modifiers, NamedKey};
use osmena_core::{KeyInput, MouseButton};

use crate::keycodes;

// CEF's `cef_event_flags_t` bits (include/internal/cef_types.h).
pub(crate) const FLAG_SHIFT: u32 = 1 << 1;
pub(crate) const FLAG_CONTROL: u32 = 1 << 2;
pub(crate) const FLAG_ALT: u32 = 1 << 3;
pub(crate) const FLAG_LEFT_BUTTON: u32 = 1 << 4;
pub(crate) const FLAG_MIDDLE_BUTTON: u32 = 1 << 5;
pub(crate) const FLAG_RIGHT_BUTTON: u32 = 1 << 6;
pub(crate) const FLAG_COMMAND: u32 = 1 << 7;
pub(crate) const FLAG_IS_KEY_PAD: u32 = 1 << 9;
pub(crate) const FLAG_IS_REPEAT: u32 = 1 << 13;

/// Keyboard modifier bits of a `keyboard-types` modifier set.
pub(crate) fn modifier_flags(modifiers: Modifiers) -> u32 {
    let mut flags = 0;
    if modifiers.contains(Modifiers::SHIFT) {
        flags |= FLAG_SHIFT;
    }
    if modifiers.contains(Modifiers::CONTROL) {
        flags |= FLAG_CONTROL;
    }
    if modifiers.contains(Modifiers::ALT) {
        flags |= FLAG_ALT;
    }
    if modifiers.contains(Modifiers::META) {
        flags |= FLAG_COMMAND;
    }
    flags
}

/// The whole wheel steps to send for a delta of `pixels` (in CEF's
/// units) on top of the fraction `carried` from earlier events, and the
/// fraction to carry on. CEF takes whole numbers; a precision wheel or
/// a trackpad sends less than one per event, which rounding each event
/// on its own would lose entirely.
pub(crate) fn wheel_steps(pixels: f64, carried: f64) -> (i32, f64) {
    let total = pixels + carried;
    let whole = total.trunc();
    (whole as i32, total - whole)
}

pub(crate) fn button_flag(button: MouseButton) -> u32 {
    match button {
        MouseButton::Primary => FLAG_LEFT_BUTTON,
        MouseButton::Secondary => FLAG_RIGHT_BUTTON,
        MouseButton::Auxiliary => FLAG_MIDDLE_BUTTON,
        MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => 0,
    }
}

/// One CEF key event, in CEF's terms but without its types so the mapping
/// is testable without a CEF build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyEventSpec {
    pub kind: KeyEventKind,
    pub modifiers: u32,
    pub windows_key_code: i32,
    pub native_key_code: i32,
    pub character: u16,
    pub unmodified_character: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyEventKind {
    RawKeyDown,
    KeyUp,
    Char,
}

/// The CEF key events for one `KeyInput`: a raw key-down (or key-up), and
/// for a press that produces text a `Char` event carrying it, as the
/// platform's own key handling would (`WM_KEYDOWN` then `WM_CHAR` on
/// Windows). Chords with Ctrl or Cmd produce no text.
///
/// The `Char` event carries the character in `windows_key_code` as well:
/// that is `WM_CHAR`'s `wParam`, and CEF on Windows reads the typed
/// character from there (`CefKeyEvent::FromCharacter(windows_key_code)`
/// in its platform delegate), not from `character`. With the virtual-key
/// code there instead, every letter typed into a page arrived as its
/// upper-case virtual key (`a` is `0x41`, `A`) and punctuation as the
/// characters that happen to share its codes.
///
/// On macOS the key-down and key-up carry a character even for a key
/// that types nothing ([`event_character`]).
pub(crate) fn key_events(input: &KeyInput) -> Vec<KeyEventSpec> {
    key_events_for(input, cfg!(target_os = "macos"))
}

fn key_events_for(input: &KeyInput, macos: bool) -> Vec<KeyEventSpec> {
    let mut modifiers = modifier_flags(input.modifiers);
    if input.repeat {
        modifiers |= FLAG_IS_REPEAT;
    }
    if input.location == Location::Numpad {
        modifiers |= FLAG_IS_KEY_PAD;
    }
    let windows_key_code = keycodes::windows_key_code(input.code, &input.key);
    let native_key_code = keycodes::native_key_code(input.code);
    let text = typed_text(&input.key);
    let character = event_character(&input.key, text, macos);
    let mut events = Vec::with_capacity(2);
    match input.state {
        KeyState::Down => {
            events.push(KeyEventSpec {
                kind: KeyEventKind::RawKeyDown,
                modifiers,
                windows_key_code,
                native_key_code,
                character,
                unmodified_character: character,
            });
            let chord = input
                .modifiers
                .intersects(Modifiers::CONTROL | Modifiers::META);
            if let Some(character) = text
                && !chord
            {
                events.push(KeyEventSpec {
                    kind: KeyEventKind::Char,
                    modifiers,
                    windows_key_code: i32::from(character),
                    native_key_code,
                    character,
                    unmodified_character: character,
                });
            }
        }
        KeyState::Up => events.push(KeyEventSpec {
            kind: KeyEventKind::KeyUp,
            modifiers,
            windows_key_code,
            native_key_code,
            character,
            unmodified_character: character,
        }),
    }
    events
}

/// The `character` (and `unmodified_character`) of a key's key-down and
/// key-up: the text it types, else 0 on Windows (CEF there reads a
/// key-down's virtual-key code and only a `CHAR` event's character).
///
/// On macOS CEF builds an `NSEvent` from the key event
/// (`TranslateWebKeyEvent` in its native platform delegate), and both
/// characters being 0 makes that an `NSEventTypeFlagsChanged`: a
/// modifier key's event, whose press or release Chromium reads off the
/// modifier flags (`ui::IsKeyUpEvent`). For any other key that answer is
/// always "press", so the release of Backspace was a second Backspace
/// (two characters gone per keystroke), an arrow's release moved the
/// caret again, and so on. Every key but a modifier therefore carries
/// what AppKit's own event for it would ([`mac_function_character`]),
/// which Chromium turns into no text.
fn event_character(key: &Key, text: Option<u16>, macos: bool) -> u16 {
    match text {
        Some(text) => text,
        None if macos => mac_function_character(key).unwrap_or(0),
        None => 0,
    }
}

/// AppKit's character for a key that types no text: `NSDeleteCharacter`
/// for Backspace, Escape's, and the `NS…FunctionKey` private-use
/// characters of `NSEvent.h` for the rest (the ones with no name share
/// one from that range, which Chromium treats the same). `None` for a
/// modifier key, whose event is a flags change on macOS as well.
fn mac_function_character(key: &Key) -> Option<u16> {
    /// A function-key character AppKit reserves but names no key with.
    const OTHER_FUNCTION_KEY: u16 = 0xF7FF;
    let Key::Named(named) = key else {
        // A character key whose text does not fit one event.
        return Some(OTHER_FUNCTION_KEY);
    };
    let character = match named {
        NamedKey::Shift
        | NamedKey::Control
        | NamedKey::Alt
        | NamedKey::AltGraph
        | NamedKey::Meta
        | NamedKey::CapsLock
        | NamedKey::Fn
        | NamedKey::FnLock
        | NamedKey::Symbol
        | NamedKey::SymbolLock => return None,
        NamedKey::Backspace => 0x7F,
        NamedKey::Escape => 0x1B,
        NamedKey::Enter => 0x0D,
        NamedKey::Tab => 0x09,
        NamedKey::ArrowUp => 0xF700,
        NamedKey::ArrowDown => 0xF701,
        NamedKey::ArrowLeft => 0xF702,
        NamedKey::ArrowRight => 0xF703,
        NamedKey::Insert => 0xF727,
        NamedKey::Delete => 0xF728,
        NamedKey::Home => 0xF729,
        NamedKey::End => 0xF72B,
        NamedKey::PageUp => 0xF72C,
        NamedKey::PageDown => 0xF72D,
        NamedKey::PrintScreen => 0xF72E,
        NamedKey::ScrollLock => 0xF72F,
        NamedKey::Pause => 0xF730,
        NamedKey::ContextMenu => 0xF735,
        // The keypad's Clear, where other keyboards have Num Lock.
        NamedKey::Clear | NamedKey::NumLock => 0xF739,
        NamedKey::Help => 0xF746,
        other => match function_key_number(other) {
            // NSF1FunctionKey is 0xF704, up to NSF35FunctionKey.
            Some(n) => 0xF704 + n - 1,
            None => OTHER_FUNCTION_KEY,
        },
    };
    Some(character)
}

/// `n` for the function key Fn (F1 to F35).
fn function_key_number(key: &NamedKey) -> Option<u16> {
    let name = key.to_string();
    let n: u16 = name.strip_prefix('F')?.parse().ok()?;
    (1..=35).contains(&n).then_some(n)
}

/// The UTF-16 unit a key press types, if any: the character of a printable
/// key, `\r` for Enter, `\t` for Tab.
fn typed_text(key: &Key) -> Option<u16> {
    match key {
        Key::Character(s) => {
            let mut units = s.encode_utf16();
            let first = units.next()?;
            // Astral characters do not fit one event; skip them rather
            // than send half a surrogate pair.
            units.next().is_none().then_some(first)
        }
        Key::Named(NamedKey::Enter) => Some(u16::from(b'\r')),
        Key::Named(NamedKey::Tab) => Some(u16::from(b'\t')),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use keyboard_types::Code;

    use super::*;

    fn press(key: Key, code: Code, modifiers: Modifiers) -> KeyInput {
        KeyInput {
            state: KeyState::Down,
            key,
            code,
            location: Location::Standard,
            modifiers,
            repeat: false,
        }
    }

    #[test]
    fn a_letter_press_is_a_key_down_then_a_char() {
        let events = key_events(&press(
            Key::Character("a".into()),
            Code::KeyA,
            Modifiers::empty(),
        ));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, KeyEventKind::RawKeyDown);
        assert_eq!(events[0].windows_key_code, 0x41);
        assert_eq!(events[1].kind, KeyEventKind::Char);
        assert_eq!(events[1].character, u16::from(b'a'));
        // CEF on Windows types `windows_key_code` of a Char event.
        assert_eq!(events[1].windows_key_code, i32::from(b'a'));
    }

    #[test]
    fn a_char_event_carries_the_character_not_the_virtual_key() {
        let events = key_events(&press(
            Key::Character(";".into()),
            Code::Semicolon,
            Modifiers::empty(),
        ));
        assert_eq!(events[0].windows_key_code, 0xBA);
        assert_eq!(events[1].windows_key_code, i32::from(b';'));
        let events = key_events(&press(
            Key::Character("é".into()),
            Code::KeyE,
            Modifiers::empty(),
        ));
        assert_eq!(events[1].windows_key_code, i32::from('é' as u16));
    }

    #[test]
    fn a_control_chord_types_nothing() {
        let events = key_events(&press(
            Key::Character("a".into()),
            Code::KeyA,
            Modifiers::CONTROL,
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].modifiers & FLAG_CONTROL, FLAG_CONTROL);
    }

    #[test]
    fn enter_types_a_carriage_return_and_release_is_a_key_up() {
        let events = key_events(&press(
            Key::Named(NamedKey::Enter),
            Code::Enter,
            Modifiers::empty(),
        ));
        assert_eq!(events[1].character, 13);
        let mut release = press(Key::Named(NamedKey::Enter), Code::Enter, Modifiers::empty());
        release.state = KeyState::Up;
        let events = key_events(&release);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, KeyEventKind::KeyUp);
    }

    #[test]
    fn shift_and_repeat_set_their_flags() {
        let mut input = press(Key::Character("A".into()), Code::KeyA, Modifiers::SHIFT);
        input.repeat = true;
        let events = key_events(&input);
        assert_eq!(events[0].modifiers, FLAG_SHIFT | FLAG_IS_REPEAT);
        assert_eq!(events[0].windows_key_code, 0x41);
        assert_eq!(events[1].character, u16::from(b'A'));
        assert_eq!(events[1].windows_key_code, 0x41);
    }

    #[test]
    fn modifier_keys_have_no_text() {
        let events = key_events(&press(
            Key::Named(NamedKey::Shift),
            Code::ShiftLeft,
            Modifiers::SHIFT,
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].windows_key_code, 0x10);
    }

    fn release(key: Key, code: Code) -> KeyInput {
        KeyInput {
            state: KeyState::Up,
            ..press(key, code, Modifiers::empty())
        }
    }

    /// What CEF on macOS makes of an event: a flags change when both
    /// characters are empty (`TranslateWebKeyEvent`).
    fn is_flags_change(event: &KeyEventSpec) -> bool {
        event.character == 0 && event.unmodified_character == 0
    }

    #[test]
    fn on_macos_backspace_is_a_key_event_in_both_directions() {
        // A flags change for a key that is not a modifier is a press to
        // Chromium whichever way it goes: Backspace's release deleted a
        // second character.
        let down = key_events_for(
            &press(
                Key::Named(NamedKey::Backspace),
                Code::Backspace,
                Modifiers::empty(),
            ),
            true,
        );
        let up = key_events_for(
            &release(Key::Named(NamedKey::Backspace), Code::Backspace),
            true,
        );
        assert_eq!(down.len(), 1, "Backspace types nothing");
        assert_eq!(down[0].kind, KeyEventKind::RawKeyDown);
        assert_eq!(up[0].kind, KeyEventKind::KeyUp);
        for event in down.iter().chain(&up) {
            assert!(!is_flags_change(event));
            // NSDeleteCharacter, as AppKit's own event has it.
            assert_eq!(event.character, 0x7F);
            assert_eq!(event.windows_key_code, 0x08);
        }
    }

    #[test]
    fn on_macos_keys_without_text_carry_their_function_key_character() {
        for (key, code, character) in [
            (NamedKey::ArrowLeft, Code::ArrowLeft, 0xF702),
            (NamedKey::ArrowDown, Code::ArrowDown, 0xF701),
            (NamedKey::Delete, Code::Delete, 0xF728),
            (NamedKey::Escape, Code::Escape, 0x1B),
            (NamedKey::PageDown, Code::PageDown, 0xF72D),
            (NamedKey::F1, Code::F1, 0xF704),
            (NamedKey::F12, Code::F12, 0xF70F),
            (NamedKey::F20, Code::F20, 0xF717),
        ] {
            let up = key_events_for(&release(Key::Named(key), code), true);
            assert_eq!(up[0].kind, KeyEventKind::KeyUp, "{key:?}");
            assert_eq!(up[0].character, character, "{key:?}");
            assert_eq!(up[0].unmodified_character, character, "{key:?}");
            let down = key_events_for(&press(Key::Named(key), code, Modifiers::empty()), true);
            assert_eq!(down.len(), 1, "{key:?} types nothing");
        }
        // A key with no name of its own still is not a flags change.
        let up = key_events_for(&release(Key::Named(NamedKey::Dead), Code::Quote), true);
        assert!(!is_flags_change(&up[0]));
    }

    #[test]
    fn on_macos_modifier_keys_stay_flag_changes() {
        for (key, code) in [
            (NamedKey::Shift, Code::ShiftLeft),
            (NamedKey::Meta, Code::MetaRight),
            (NamedKey::Alt, Code::AltLeft),
            (NamedKey::CapsLock, Code::CapsLock),
        ] {
            let up = key_events_for(&release(Key::Named(key), code), true);
            assert!(is_flags_change(&up[0]), "{key:?}");
        }
    }

    #[test]
    fn on_windows_keys_without_text_carry_no_character() {
        let up = key_events_for(
            &release(Key::Named(NamedKey::Backspace), Code::Backspace),
            false,
        );
        assert_eq!(up[0].character, 0);
        let down = key_events_for(
            &press(
                Key::Named(NamedKey::ArrowLeft),
                Code::ArrowLeft,
                Modifiers::empty(),
            ),
            false,
        );
        assert_eq!(down[0].character, 0);
    }

    #[test]
    fn typed_text_is_the_character_on_both_platforms() {
        for macos in [false, true] {
            let up = key_events_for(&release(Key::Character("e".into()), Code::KeyE), macos);
            assert_eq!(up[0].character, u16::from(b'e'));
        }
    }

    #[test]
    fn wheel_steps_carry_the_fraction_to_the_next_event() {
        // Three events of 0.4 make one step on the third.
        let (steps, carried) = wheel_steps(0.4, 0.0);
        assert_eq!((steps, carried), (0, 0.4));
        let (steps, carried) = wheel_steps(0.4, carried);
        assert_eq!(steps, 0);
        let (steps, carried) = wheel_steps(0.4, carried);
        assert_eq!(steps, 1);
        assert!(carried.abs() < 0.3);
        // A whole notch is sent at once, and a reversal does not borrow.
        assert_eq!(wheel_steps(-3.0, 0.0), (-3, 0.0));
        let (steps, carried) = wheel_steps(-0.5, 0.25);
        assert_eq!(steps, 0);
        assert!((carried + 0.25).abs() < 1e-9);
    }

    #[test]
    fn mouse_button_flags() {
        assert_eq!(button_flag(MouseButton::Primary), FLAG_LEFT_BUTTON);
        assert_eq!(button_flag(MouseButton::Secondary), FLAG_RIGHT_BUTTON);
        assert_eq!(button_flag(MouseButton::Auxiliary), FLAG_MIDDLE_BUTTON);
        assert_eq!(button_flag(MouseButton::Back), 0);
    }
}
