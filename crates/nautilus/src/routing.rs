//! The window's routing decisions as pure functions and small state
//! machines: which side gets a pointer event, whether a key event is a
//! keymap command or goes to egui or the page, and where the page area sits
//! in physical pixels. The window reads egui
//! and winit state and feeds it in here, so every rule has a unit test.

use std::collections::{HashMap, HashSet};

use egui::Rect;
use keyboard_types::{Code, Modifiers};
use nautilus_core::{CommandKind, Keymap, TabId};
use nautilus_engine::PhysicalSize;
use winit::event::MouseButton;

use crate::input::{self, KeyRoute};

/// Who receives a pointer event: the chrome (egui) or the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerTarget {
    Egui,
    Page,
}

/// What the window knows about the pointer's position and egui's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerContext {
    /// The pointer is above the page area, or its position is unknown.
    pub over_chrome: bool,
    /// egui draws the page area itself (the crash panel).
    pub content_is_egui: bool,
    /// The pointer is over an interactable egui layer (menus, the error
    /// overlay).
    pub over_egui: bool,
    /// A popup is open, so a click outside it must reach egui to close it.
    pub popup_open: bool,
}

/// The side that gets a pointer event. A capture (set by a press) wins over
/// everything else; otherwise egui owns the pointer over the chrome and over
/// any egui layer or open popup, and the page owns the rest.
pub fn route_pointer(captured: Option<PointerTarget>, ctx: PointerContext) -> PointerTarget {
    if let Some(captured) = captured {
        return captured;
    }
    if ctx.over_chrome || ctx.content_is_egui || ctx.over_egui || ctx.popup_open {
        PointerTarget::Egui
    } else {
        PointerTarget::Page
    }
}

/// Pointer capture: the side that took the first button press gets every
/// pointer event until the last pressed button is released, so a drag that
/// crosses the toolbar edge ends where it started, and holding a second
/// button does not hand the pointer over in the middle.
#[derive(Debug, Default)]
pub struct PointerCapture {
    pressed: HashSet<MouseButton>,
    target: Option<PointerTarget>,
}

impl PointerCapture {
    /// The side holding the capture, if any.
    pub fn captured(&self) -> Option<PointerTarget> {
        self.target
    }

    /// A button went down while `current` was the pointer's side. Returns
    /// the side that gets this press.
    pub fn press(&mut self, button: MouseButton, current: PointerTarget) -> PointerTarget {
        if self.pressed.is_empty() {
            self.target = Some(current);
        }
        self.pressed.insert(button);
        self.target.unwrap_or(current)
    }

    /// A button went up while `current` was the pointer's side. Returns the
    /// side that gets this release. The capture ends with the last button.
    pub fn release(&mut self, button: MouseButton, current: PointerTarget) -> PointerTarget {
        let target = self.target.unwrap_or(current);
        self.pressed.remove(&button);
        if self.pressed.is_empty() {
            self.target = None;
        }
        target
    }

    /// Forget all buttons, for example when the window loses focus and the
    /// releases will arrive elsewhere.
    pub fn clear(&mut self) {
        self.pressed.clear();
        self.target = None;
    }
}

/// Where a key event goes, as decided by [`KeyRouter::route`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTarget {
    /// egui gets the key.
    Egui,
    /// The key fires a keymap command.
    Command(CommandKind),
    /// The page gets the key in this tab: the tab that received its press.
    Page(TabId),
}

/// One key event as the window reads it from winit.
#[derive(Clone, Copy, Debug)]
pub struct KeyStroke {
    pub code: Code,
    pub modifiers: Modifiers,
    pub pressed: bool,
    pub repeat: bool,
}

/// Decides where key events go and remembers which side received each
/// press, so that side also receives the repeats and the release. The page
/// never sees half a key stroke (for example a key pressed in another window
/// before focus moved here, or a chord that fired a command), and egui never
/// misses the release of a key it saw go down (after Enter submits the
/// address bar, the field loses focus before the Enter release arrives).
///
/// A key the page holds remembers the tab that received its press. Its
/// repeats and release go to that tab even if another tab became active
/// meanwhile (Meta pressed in tab A, then Cmd+Shift+] makes B active, then
/// Meta is released: A gets the release, so A never keeps a key down). If
/// that tab has closed, the hold is dropped and nothing is delivered.
///
/// The full rule, in order:
/// 1. A release goes to the side that got the press, whoever has focus now;
///    a release nobody here saw pressed goes to egui while it has focus and
///    nowhere otherwise.
/// 2. A press or repeat is first put to [`input::route_key`]: the initial
///    press of a bound chord is a command (while egui has focus, only chords
///    with Cmd or Ctrl). A command takes its key away from the page, so the
///    chord's repeats and release never reach it.
/// 3. A press or repeat of a key the page holds goes to the tab that holds
///    it, or is dropped if that tab is gone.
/// 4. egui gets the key while it has focus, or if it got the key's press.
/// 5. A repeat nobody here saw pressed is dropped.
/// 6. Otherwise the active tab gets the press and holds the key until its
///    release.
#[derive(Debug, Default)]
pub struct KeyRouter {
    /// Keys the page holds, each with the tab that received its press.
    page_keys: HashMap<Code, TabId>,
    egui_keys: HashSet<Code>,
}

impl KeyRouter {
    /// Route one key event. `active` is the window's active tab, which gets
    /// new page presses; `live` says whether a tab still exists. `None`
    /// means nobody sees the event.
    pub fn route(
        &mut self,
        stroke: KeyStroke,
        egui_wants_keyboard: bool,
        active: Option<TabId>,
        live: impl Fn(TabId) -> bool,
        keymap: &Keymap,
    ) -> Option<KeyTarget> {
        let KeyStroke {
            code,
            modifiers,
            pressed,
            repeat,
        } = stroke;
        if !pressed {
            if let Some(tab) = self.page_keys.remove(&code) {
                return live(tab).then_some(KeyTarget::Page(tab));
            }
            if self.egui_keys.remove(&code) {
                return Some(KeyTarget::Egui);
            }
            return egui_wants_keyboard.then_some(KeyTarget::Egui);
        }
        let first_cut =
            input::route_key(egui_wants_keyboard, true, repeat, modifiers, code, keymap);
        if let KeyRoute::Command(kind) = first_cut {
            self.page_keys.remove(&code);
            return Some(KeyTarget::Command(kind));
        }
        if let Some(&tab) = self.page_keys.get(&code) {
            if live(tab) {
                return Some(KeyTarget::Page(tab));
            }
            // The tab closed while the key was held, so the hold goes too.
            self.page_keys.remove(&code);
            if repeat {
                return None;
            }
        }
        if first_cut == KeyRoute::Egui || self.egui_keys.contains(&code) {
            self.egui_keys.insert(code);
            return Some(KeyTarget::Egui);
        }
        if repeat {
            return None;
        }
        let tab = active?;
        self.page_keys.insert(code, tab);
        Some(KeyTarget::Page(tab))
    }

    /// Forget every key, for example when the window loses focus. egui does
    /// the same for its own held keys on `WindowFocused(false)`.
    pub fn clear(&mut self) {
        self.page_keys.clear();
        self.egui_keys.clear();
    }
}

/// The page area in physical pixels: its top edge, which pointer routing
/// compares against, and its size. Both edges are rounded to whole pixels
/// first and the size is the difference, so the size covers exactly the
/// pixel rows and columns the blit paints.
pub fn content_pixels(rect: Rect, pixels_per_point: f32) -> (f32, PhysicalSize<u32>) {
    let left = (rect.min.x * pixels_per_point).round();
    let right = (rect.max.x * pixels_per_point).round();
    let top = (rect.min.y * pixels_per_point).round();
    let bottom = (rect.max.y * pixels_per_point).round();
    let size = PhysicalSize::new(
        (right - left).max(1.0) as u32,
        (bottom - top).max(1.0) as u32,
    );
    (top, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;
    use nautilus_core::Platform;

    const LEFT: MouseButton = MouseButton::Left;
    const RIGHT: MouseButton = MouseButton::Right;
    const TAB_A: TabId = TabId(1);
    const TAB_B: TabId = TabId(2);

    fn ctx() -> PointerContext {
        PointerContext {
            over_chrome: false,
            content_is_egui: false,
            over_egui: false,
            popup_open: false,
        }
    }

    #[test]
    fn pointer_over_page_goes_to_page() {
        assert_eq!(route_pointer(None, ctx()), PointerTarget::Page);
    }

    #[test]
    fn pointer_over_chrome_goes_to_egui() {
        let c = PointerContext {
            over_chrome: true,
            ..ctx()
        };
        assert_eq!(route_pointer(None, c), PointerTarget::Egui);
    }

    #[test]
    fn egui_layers_popups_and_crash_panel_take_the_pointer() {
        for c in [
            PointerContext {
                over_egui: true,
                ..ctx()
            },
            PointerContext {
                popup_open: true,
                ..ctx()
            },
            PointerContext {
                content_is_egui: true,
                ..ctx()
            },
        ] {
            assert_eq!(route_pointer(None, c), PointerTarget::Egui, "{c:?}");
        }
    }

    #[test]
    fn a_capture_overrides_the_position() {
        let over_chrome = PointerContext {
            over_chrome: true,
            ..ctx()
        };
        assert_eq!(
            route_pointer(Some(PointerTarget::Page), over_chrome),
            PointerTarget::Page
        );
        assert_eq!(
            route_pointer(Some(PointerTarget::Egui), ctx()),
            PointerTarget::Egui
        );
    }

    #[test]
    fn press_and_release_keep_one_side_for_the_whole_click() {
        let mut capture = PointerCapture::default();
        assert_eq!(
            capture.press(LEFT, PointerTarget::Page),
            PointerTarget::Page
        );
        // Dragging onto the toolbar: the pointer is now egui's, but the
        // capture keeps the release with the page.
        let now = route_pointer(
            capture.captured(),
            PointerContext {
                over_chrome: true,
                ..ctx()
            },
        );
        assert_eq!(now, PointerTarget::Page);
        assert_eq!(capture.release(LEFT, now), PointerTarget::Page);
        assert_eq!(capture.captured(), None);
    }

    /// The reviewer's scenario: left pressed over the page, right pressed
    /// and released, pointer moved over the toolbar, left released over the
    /// toolbar. The page must see the left release, so right's release must
    /// not end the capture.
    #[test]
    fn releasing_one_button_does_not_end_a_capture_held_by_another() {
        let mut capture = PointerCapture::default();
        assert_eq!(
            capture.press(LEFT, PointerTarget::Page),
            PointerTarget::Page
        );

        // Right press and release while the pointer is still over the page.
        assert_eq!(
            capture.press(RIGHT, PointerTarget::Page),
            PointerTarget::Page
        );
        assert_eq!(
            capture.release(RIGHT, PointerTarget::Page),
            PointerTarget::Page
        );
        assert_eq!(capture.captured(), Some(PointerTarget::Page));

        // The pointer moves over the toolbar with left still held.
        let over_toolbar = PointerContext {
            over_chrome: true,
            ..ctx()
        };
        assert_eq!(
            route_pointer(capture.captured(), over_toolbar),
            PointerTarget::Page
        );

        // Left released over the toolbar still goes to the page.
        let now = route_pointer(capture.captured(), over_toolbar);
        assert_eq!(capture.release(LEFT, now), PointerTarget::Page);
        assert_eq!(capture.captured(), None);
        // After the last release the toolbar owns the pointer again.
        assert_eq!(
            route_pointer(capture.captured(), over_toolbar),
            PointerTarget::Egui
        );
    }

    #[test]
    fn a_second_button_pressed_over_the_chrome_joins_the_page_capture() {
        let mut capture = PointerCapture::default();
        capture.press(LEFT, PointerTarget::Page);
        // A right press while the pointer is over the chrome still belongs
        // to the page capture held by the left button.
        assert_eq!(
            capture.press(RIGHT, PointerTarget::Egui),
            PointerTarget::Page
        );
        assert_eq!(
            capture.release(RIGHT, PointerTarget::Egui),
            PointerTarget::Page
        );
        assert_eq!(
            capture.release(LEFT, PointerTarget::Egui),
            PointerTarget::Page
        );
        assert_eq!(capture.captured(), None);
    }

    #[test]
    fn a_release_without_a_press_here_uses_the_current_side() {
        let mut capture = PointerCapture::default();
        assert_eq!(
            capture.release(LEFT, PointerTarget::Egui),
            PointerTarget::Egui
        );
        assert_eq!(capture.captured(), None);
    }

    #[test]
    fn clearing_forgets_every_button() {
        let mut capture = PointerCapture::default();
        capture.press(LEFT, PointerTarget::Page);
        capture.clear();
        assert_eq!(capture.captured(), None);
        // A later press starts a fresh capture on whichever side it lands.
        assert_eq!(
            capture.press(RIGHT, PointerTarget::Egui),
            PointerTarget::Egui
        );
    }

    fn mac() -> Keymap {
        Keymap::defaults(Platform::MacOs)
    }

    /// Every tab exists.
    fn all_live(_: TabId) -> bool {
        true
    }

    fn stroke(code: Code, modifiers: Modifiers, pressed: bool, repeat: bool) -> KeyStroke {
        KeyStroke {
            code,
            modifiers,
            pressed,
            repeat,
        }
    }

    /// An unmodified key event through `keys`, with TAB_A active.
    fn plain(
        keys: &mut KeyRouter,
        code: Code,
        pressed: bool,
        repeat: bool,
        egui_wants_keyboard: bool,
    ) -> Option<KeyTarget> {
        keys.route(
            stroke(code, Modifiers::empty(), pressed, repeat),
            egui_wants_keyboard,
            Some(TAB_A),
            all_live,
            &mac(),
        )
    }

    /// A key event with Cmd held, as macOS shortcuts arrive, with TAB_A
    /// active.
    fn cmd(
        keys: &mut KeyRouter,
        code: Code,
        pressed: bool,
        repeat: bool,
        egui_wants_keyboard: bool,
    ) -> Option<KeyTarget> {
        keys.route(
            stroke(code, Modifiers::META, pressed, repeat),
            egui_wants_keyboard,
            Some(TAB_A),
            all_live,
            &mac(),
        )
    }

    #[test]
    fn page_key_gets_its_repeats_and_release() {
        let mut keys = KeyRouter::default();
        assert_eq!(
            plain(&mut keys, Code::KeyA, true, false, false),
            Some(KeyTarget::Page(TAB_A))
        );
        assert_eq!(
            plain(&mut keys, Code::KeyA, true, true, false),
            Some(KeyTarget::Page(TAB_A))
        );
        assert_eq!(
            plain(&mut keys, Code::KeyA, false, false, false),
            Some(KeyTarget::Page(TAB_A))
        );
        // Released: a later release is no longer the page's.
        assert_eq!(plain(&mut keys, Code::KeyA, false, false, false), None);
    }

    #[test]
    fn key_pressed_while_egui_has_focus_goes_to_egui() {
        let mut keys = KeyRouter::default();
        assert_eq!(
            plain(&mut keys, Code::KeyA, true, false, true),
            Some(KeyTarget::Egui)
        );
        assert_eq!(
            plain(&mut keys, Code::KeyA, true, true, true),
            Some(KeyTarget::Egui)
        );
        assert_eq!(
            plain(&mut keys, Code::KeyA, false, false, true),
            Some(KeyTarget::Egui)
        );
    }

    /// The reviewer's Enter case: egui gets Enter, the address bar loses
    /// focus on that press, and the release must still reach egui.
    #[test]
    fn egui_gets_the_release_after_it_lost_focus() {
        let mut keys = KeyRouter::default();
        assert_eq!(
            plain(&mut keys, Code::Enter, true, false, true),
            Some(KeyTarget::Egui)
        );
        assert_eq!(
            plain(&mut keys, Code::Enter, false, false, false),
            Some(KeyTarget::Egui)
        );
        // Nothing left to deliver.
        assert_eq!(plain(&mut keys, Code::Enter, false, false, false), None);
    }

    #[test]
    fn repeat_of_an_egui_key_still_goes_to_egui_after_focus_moves() {
        let mut keys = KeyRouter::default();
        plain(&mut keys, Code::Backspace, true, false, true);
        assert_eq!(
            plain(&mut keys, Code::Backspace, true, true, false),
            Some(KeyTarget::Egui)
        );
    }

    #[test]
    fn page_never_sees_a_repeat_or_release_it_did_not_start() {
        let mut keys = KeyRouter::default();
        // Repeat with no press from this window.
        assert_eq!(plain(&mut keys, Code::KeyB, true, true, false), None);
        // A release for a key pressed elsewhere, with no egui focus.
        assert_eq!(plain(&mut keys, Code::KeyB, false, false, false), None);
    }

    #[test]
    fn a_key_held_by_the_page_stays_with_the_page_even_if_egui_gets_focus() {
        let mut keys = KeyRouter::default();
        plain(&mut keys, Code::KeyC, true, false, false);
        // Focus moved to egui while C is held: the repeat and release stay
        // with the page so it never sees half a stroke.
        assert_eq!(
            plain(&mut keys, Code::KeyC, true, true, true),
            Some(KeyTarget::Page(TAB_A))
        );
        assert_eq!(
            plain(&mut keys, Code::KeyC, false, false, true),
            Some(KeyTarget::Page(TAB_A))
        );
    }

    /// Meta pressed in tab A, then Cmd+Shift+] makes B active: the release
    /// still targets A, the tab that saw the press. B gets no lone keyup.
    #[test]
    fn a_held_key_releases_in_the_tab_that_got_its_press() {
        let mut keys = KeyRouter::default();
        let meta = Code::MetaLeft;
        assert_eq!(
            keys.route(
                stroke(meta, Modifiers::empty(), true, false),
                false,
                Some(TAB_A),
                all_live,
                &mac(),
            ),
            Some(KeyTarget::Page(TAB_A))
        );
        // B is now active; a repeat of the held key still goes to A.
        assert_eq!(
            keys.route(
                stroke(meta, Modifiers::empty(), true, true),
                false,
                Some(TAB_B),
                all_live,
                &mac(),
            ),
            Some(KeyTarget::Page(TAB_A))
        );
        assert_eq!(
            keys.route(
                stroke(meta, Modifiers::empty(), false, false),
                false,
                Some(TAB_B),
                all_live,
                &mac(),
            ),
            Some(KeyTarget::Page(TAB_A))
        );
        // The release ended the hold: nothing more goes anywhere.
        assert_eq!(
            keys.route(
                stroke(meta, Modifiers::empty(), false, false),
                false,
                Some(TAB_B),
                all_live,
                &mac(),
            ),
            None
        );
    }

    /// A held key whose tab closed is dropped: its release goes nowhere and
    /// a fresh press goes to the active tab.
    #[test]
    fn a_held_key_is_dropped_when_its_tab_closes() {
        let mut keys = KeyRouter::default();
        plain(&mut keys, Code::KeyQ, true, false, false);
        let tab_a_gone = |tab: TabId| tab != TAB_A;
        assert_eq!(
            keys.route(
                stroke(Code::KeyQ, Modifiers::empty(), false, false),
                false,
                Some(TAB_B),
                tab_a_gone,
                &mac(),
            ),
            None
        );

        plain(&mut keys, Code::KeyQ, true, false, false);
        assert_eq!(
            keys.route(
                stroke(Code::KeyQ, Modifiers::empty(), true, true),
                false,
                Some(TAB_B),
                tab_a_gone,
                &mac(),
            ),
            None,
            "a repeat of a key whose tab closed is not delivered"
        );
        assert_eq!(
            keys.route(
                stroke(Code::KeyQ, Modifiers::empty(), true, false),
                false,
                Some(TAB_B),
                tab_a_gone,
                &mac(),
            ),
            Some(KeyTarget::Page(TAB_B)),
            "a fresh press goes to the active tab"
        );
    }

    #[test]
    fn clearing_forgets_both_sides() {
        let mut keys = KeyRouter::default();
        plain(&mut keys, Code::KeyA, true, false, false);
        plain(&mut keys, Code::Enter, true, false, true);
        keys.clear();
        assert_eq!(plain(&mut keys, Code::KeyA, false, false, false), None);
        assert_eq!(plain(&mut keys, Code::Enter, false, false, false), None);
    }

    /// Holding Cmd+T over the page opens one tab: the press is a command,
    /// and its repeats and release reach neither the page nor egui.
    #[test]
    fn a_command_chord_never_reaches_the_page() {
        let mut keys = KeyRouter::default();
        assert_eq!(
            cmd(&mut keys, Code::KeyT, true, false, false),
            Some(KeyTarget::Command(CommandKind::NewTab))
        );
        assert_eq!(cmd(&mut keys, Code::KeyT, true, true, false), None);
        assert_eq!(cmd(&mut keys, Code::KeyT, false, false, false), None);
    }

    /// A lost release left T with the page; a fresh Cmd+T press still fires
    /// the command and takes T away from the page, so the page never sees
    /// the chord's repeats or release.
    #[test]
    fn a_command_takes_its_key_back_from_the_page() {
        let mut keys = KeyRouter::default();
        plain(&mut keys, Code::KeyT, true, false, false);
        assert_eq!(
            cmd(&mut keys, Code::KeyT, true, false, false),
            Some(KeyTarget::Command(CommandKind::NewTab))
        );
        assert_eq!(cmd(&mut keys, Code::KeyT, true, true, false), None);
        assert_eq!(cmd(&mut keys, Code::KeyT, false, false, false), None);
    }

    /// Cmd+T in the address bar opens a tab; Cmd+A stays egui's select-all.
    #[test]
    fn primary_chords_fire_while_egui_has_focus() {
        let mut keys = KeyRouter::default();
        assert_eq!(
            cmd(&mut keys, Code::KeyT, true, false, true),
            Some(KeyTarget::Command(CommandKind::NewTab))
        );
        assert_eq!(
            cmd(&mut keys, Code::KeyA, true, false, true),
            Some(KeyTarget::Egui)
        );
    }

    #[test]
    fn content_size_covers_the_same_pixel_edges_as_the_blit() {
        // Edges 20.4 and 30.6 round to 20 and 31: eleven rows, starting at 20.
        let rect = Rect::from_min_max(pos2(0.5, 20.4), pos2(10.5, 30.6));
        let (top, size) = content_pixels(rect, 1.0);
        assert_eq!(top, 20.0);
        assert_eq!(size, PhysicalSize::new(10, 11));
    }

    #[test]
    fn content_scales_with_pixels_per_point() {
        // 40.2 * 2 = 80.4 -> 80; 300.6 * 2 = 601.2 -> 601.
        let rect = Rect::from_min_max(pos2(0.0, 40.2), pos2(100.0, 300.6));
        let (top, size) = content_pixels(rect, 2.0);
        assert_eq!(top, 80.0);
        assert_eq!(size, PhysicalSize::new(200, 521));
    }

    #[test]
    fn content_is_never_zero_sized() {
        let rect = Rect::from_min_max(pos2(5.0, 5.0), pos2(5.0, 5.0));
        let (_, size) = content_pixels(rect, 1.0);
        assert_eq!(size, PhysicalSize::new(1, 1));
    }
}
