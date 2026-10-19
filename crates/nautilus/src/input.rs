//! winit input to engine input, and the key-routing rule. Coordinates are
//! converted by the window (toolbar offset, physical pixels); this module
//! maps the enums and decides where a key goes.

use keyboard_types::{Code, Modifiers};
use nautilus_core::{CommandKind, KeyChord, Keymap};
use nautilus_engine::{ButtonAction, EngineInput, MouseButton};
use winit::event::{ElementState, Ime, MouseScrollDelta};

/// Pixels per wheel "line". winit reports one wheel notch as one line on
/// Windows; Chromium scrolls about 100 px per notch, so this matches it.
const LINE_HEIGHT_PX: f32 = 100.0;

pub fn mouse_button(button: winit::event::MouseButton) -> MouseButton {
    match button {
        winit::event::MouseButton::Left => MouseButton::Primary,
        winit::event::MouseButton::Right => MouseButton::Secondary,
        winit::event::MouseButton::Middle => MouseButton::Auxiliary,
        winit::event::MouseButton::Back => MouseButton::Back,
        winit::event::MouseButton::Forward => MouseButton::Forward,
        winit::event::MouseButton::Other(n) => MouseButton::Other(n),
    }
}

pub fn button_action(state: ElementState) -> ButtonAction {
    match state {
        ElementState::Pressed => ButtonAction::Down,
        ElementState::Released => ButtonAction::Up,
    }
}

/// Wheel delta in pixels, `(dx, dy)`.
pub fn wheel_delta(delta: MouseScrollDelta) -> (f64, f64) {
    match delta {
        MouseScrollDelta::LineDelta(dx, dy) => {
            ((dx * LINE_HEIGHT_PX) as f64, (dy * LINE_HEIGHT_PX) as f64)
        }
        MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
    }
}

pub fn ime(ime: &Ime) -> Option<EngineInput> {
    match ime {
        Ime::Enabled => Some(EngineInput::ImeStart),
        Ime::Preedit(text, _) => Some(EngineInput::ImePreedit(text.clone())),
        Ime::Commit(text) => Some(EngineInput::ImeCommit(text.clone())),
        Ime::Disabled => Some(EngineInput::ImeDismissed),
    }
}

/// Where a key event goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyRoute {
    /// egui: a widget (the address bar) has keyboard focus, or egui got
    /// the press of this key.
    Egui,
    /// The initial press of a bound chord.
    Command(CommandKind),
    /// The page in the active tab.
    Page,
}

/// The routing rule: a bound chord with Cmd or Ctrl held fires its command
/// on the initial press even while an egui field (the address bar) has
/// focus, so shortcuts keep working there; otherwise egui takes every key
/// while it has focus; otherwise bound chords on their initial press
/// (auto-repeats and releases never trigger commands); otherwise the page.
///
/// This is the stateless first cut for one event, used as is by the
/// settings window (which has no page). Browser windows go through
/// [`crate::routing::KeyRouter`], which applies this rule to presses and
/// repeats and then sends repeats and releases to the side that got the
/// press, so a key that fired a command never reaches the page.
pub fn route_key(
    egui_wants_keyboard: bool,
    pressed: bool,
    repeat: bool,
    modifiers: Modifiers,
    code: Code,
    keymap: &Keymap,
) -> KeyRoute {
    let with_primary = modifiers.intersects(Modifiers::CONTROL | Modifiers::META);
    let bound = if pressed && !repeat {
        keymap.resolve(KeyChord::new(modifiers, code))
    } else {
        None
    };
    match (egui_wants_keyboard, bound) {
        (true, Some(kind)) if with_primary => KeyRoute::Command(kind),
        (true, _) => KeyRoute::Egui,
        (false, Some(kind)) => KeyRoute::Command(kind),
        (false, None) => KeyRoute::Page,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::Platform;

    #[test]
    fn line_deltas_scale_to_pixels_and_pixel_deltas_pass_through() {
        assert_eq!(
            wheel_delta(MouseScrollDelta::LineDelta(0.0, -1.0)),
            (0.0, -100.0)
        );
        assert_eq!(
            wheel_delta(MouseScrollDelta::PixelDelta(
                winit::dpi::PhysicalPosition::new(3.0, 4.0)
            )),
            (3.0, 4.0)
        );
    }

    #[test]
    fn buttons_map_one_to_one() {
        assert_eq!(
            mouse_button(winit::event::MouseButton::Left),
            MouseButton::Primary
        );
        assert_eq!(
            mouse_button(winit::event::MouseButton::Right),
            MouseButton::Secondary
        );
        assert_eq!(
            mouse_button(winit::event::MouseButton::Middle),
            MouseButton::Auxiliary
        );
        assert_eq!(
            mouse_button(winit::event::MouseButton::Other(7)),
            MouseButton::Other(7)
        );
        assert_eq!(button_action(ElementState::Pressed), ButtonAction::Down);
    }

    fn mac() -> Keymap {
        Keymap::defaults(Platform::MacOs)
    }

    #[test]
    fn egui_focus_takes_unbound_keys_but_primary_chords_still_fire() {
        let r = route_key(true, true, false, Modifiers::empty(), Code::KeyA, &mac());
        assert_eq!(r, KeyRoute::Egui, "typing goes to the address bar");
        let r = route_key(true, true, false, Modifiers::META, Code::KeyA, &mac());
        assert_eq!(r, KeyRoute::Egui, "Cmd+A is unbound: egui selects all");
        let r = route_key(true, true, false, Modifiers::META, Code::KeyT, &mac());
        assert_eq!(
            r,
            KeyRoute::Command(CommandKind::NewTab),
            "Cmd+T works while the address bar has focus"
        );
        let r = route_key(true, false, false, Modifiers::META, Code::KeyT, &mac());
        assert_eq!(r, KeyRoute::Egui, "the release goes to egui");
    }

    #[test]
    fn bound_chord_on_initial_press_is_a_command() {
        let r = route_key(false, true, false, Modifiers::META, Code::KeyT, &mac());
        assert_eq!(r, KeyRoute::Command(CommandKind::NewTab));
    }

    #[test]
    fn repeats_and_releases_of_chords_never_fire_commands() {
        let r = route_key(false, true, true, Modifiers::META, Code::KeyT, &mac());
        assert_eq!(r, KeyRoute::Page);
        let r = route_key(false, false, false, Modifiers::META, Code::KeyT, &mac());
        assert_eq!(r, KeyRoute::Page);
    }

    #[test]
    fn unbound_keys_reach_the_page() {
        let r = route_key(false, true, false, Modifiers::empty(), Code::KeyA, &mac());
        assert_eq!(r, KeyRoute::Page);
        let r = route_key(false, true, false, Modifiers::empty(), Code::Escape, &mac());
        assert_eq!(r, KeyRoute::Page, "Escape belongs to the page");
        let r = route_key(false, true, false, Modifiers::META, Code::ArrowLeft, &mac());
        assert_eq!(r, KeyRoute::Page, "Cmd+Left edits text in fields");
    }
}
