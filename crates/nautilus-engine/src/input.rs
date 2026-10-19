//! Nautilus's input events and their conversion to CEF's. The app builds
//! `EngineInput` from winit events; this keeps winit out of the engine and
//! CEF out of the app.

use keyboard_types::{Code, Key, KeyState, Location, Modifiers, NamedKey};

use crate::keycodes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Primary,
    Secondary,
    Auxiliary,
    Back,
    Forward,
    Other(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonAction {
    Down,
    Up,
}

/// A key event in `keyboard-types` terms (the W3C UI Events model).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub state: KeyState,
    pub key: Key,
    pub code: Code,
    pub location: Location,
    pub modifiers: Modifiers,
    pub repeat: bool,
}

/// Input for a tab. Positions are physical pixels relative to the top-left
/// of the page area (below the toolbar).
#[derive(Clone, Debug, PartialEq)]
pub enum EngineInput {
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        action: ButtonAction,
        button: MouseButton,
        x: f32,
        y: f32,
    },
    /// The pointer left the page area.
    MouseLeft,
    /// Scroll by `(dx, dy)` pixels with the pointer at `(x, y)`; positive
    /// `dy` scrolls towards the top of the page (wheel rolled away from
    /// the user), as in winit.
    Wheel {
        dx: f64,
        dy: f64,
        x: f32,
        y: f32,
    },
    Key(KeyInput),
    ImeStart,
    ImePreedit(String),
    ImeCommit(String),
    ImeDismissed,
}

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
pub(crate) fn key_events(input: &KeyInput) -> Vec<KeyEventSpec> {
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
    let character = text.unwrap_or(0);
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
                    windows_key_code,
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
        assert_eq!(events[1].character, u16::from(b'A'));
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

    #[test]
    fn mouse_button_flags() {
        assert_eq!(button_flag(MouseButton::Primary), FLAG_LEFT_BUTTON);
        assert_eq!(button_flag(MouseButton::Secondary), FLAG_RIGHT_BUTTON);
        assert_eq!(button_flag(MouseButton::Auxiliary), FLAG_MIDDLE_BUTTON);
        assert_eq!(button_flag(MouseButton::Back), 0);
    }
}
