//! Keyboard shortcuts: a key chord maps to a command kind. Phase 2 adds
//! persistence to `keymap.toml` and an editor; the data model is already
//! here so that is a data change, not a rewrite.

use keyboard_types::{Code, Modifiers};
use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::model::{TabId, WindowId};

/// A physical key plus modifiers. Chords use the physical key code, not
/// the produced character, so `Cmd+Shift+]` is the same chord on every
/// keyboard layout and does not depend on what `Shift+]` types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub code: Code,
}

impl KeyChord {
    /// Only these modifiers take part in matching; lock keys and the like
    /// are ignored.
    pub const RELEVANT_MODIFIERS: Modifiers = Modifiers::CONTROL
        .union(Modifiers::SHIFT)
        .union(Modifiers::ALT)
        .union(Modifiers::META);

    pub fn new(modifiers: Modifiers, code: Code) -> KeyChord {
        KeyChord {
            modifiers: modifiers & Self::RELEVANT_MODIFIERS,
            code,
        }
    }
}

/// A [`Command`] without its window or tab argument. The keymap resolves a
/// chord to one of these; the app fills in the focused window and its
/// active tab with [`CommandKind::into_command`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandKind {
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    Back,
    Forward,
    Reload,
    Stop,
    FocusAddressBar,
    NewWindow,
    CloseWindow,
    OpenSettings,
    Quit,
}

impl CommandKind {
    /// Every kind, in a fixed order. Used to check that the defaults bind
    /// each one and, in Phase 2, to list them in the editor.
    pub const ALL: [CommandKind; 13] = [
        CommandKind::NewTab,
        CommandKind::CloseTab,
        CommandKind::NextTab,
        CommandKind::PreviousTab,
        CommandKind::Back,
        CommandKind::Forward,
        CommandKind::Reload,
        CommandKind::Stop,
        CommandKind::FocusAddressBar,
        CommandKind::NewWindow,
        CommandKind::CloseWindow,
        CommandKind::OpenSettings,
        CommandKind::Quit,
    ];

    pub fn into_command(self, window: WindowId, tab: TabId) -> Command {
        match self {
            CommandKind::NewTab => Command::NewTab { window, url: None },
            CommandKind::CloseTab => Command::CloseTab(tab),
            CommandKind::NextTab => Command::NextTab(window),
            CommandKind::PreviousTab => Command::PreviousTab(window),
            CommandKind::Back => Command::Back(tab),
            CommandKind::Forward => Command::Forward(tab),
            CommandKind::Reload => Command::Reload(tab),
            CommandKind::Stop => Command::Stop(tab),
            CommandKind::FocusAddressBar => Command::FocusAddressBar(window),
            CommandKind::NewWindow => Command::NewWindow,
            CommandKind::CloseWindow => Command::CloseWindow(window),
            CommandKind::OpenSettings => Command::OpenSettings,
            CommandKind::Quit => Command::Quit,
        }
    }
}

/// Which platform's conventions the default bindings follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Windows
        }
    }

    /// The "primary" shortcut modifier: Command on macOS, Control elsewhere.
    pub fn primary(self) -> Modifiers {
        match self {
            Platform::MacOs => Modifiers::META,
            Platform::Windows => Modifiers::CONTROL,
        }
    }
}

/// Ordered chord-to-command bindings. Several chords may map to one kind;
/// one chord maps to at most one kind.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    bindings: Vec<(KeyChord, CommandKind)>,
}

impl Keymap {
    /// The built-in bindings for a platform.
    pub fn defaults(platform: Platform) -> Keymap {
        let primary = platform.primary();
        let mut keymap = Keymap::default();
        let mut bind = |modifiers: Modifiers, code: Code, kind: CommandKind| {
            keymap.bind(KeyChord::new(modifiers, code), kind);
        };
        bind(primary, Code::KeyT, CommandKind::NewTab);
        bind(primary, Code::KeyW, CommandKind::CloseTab);
        bind(
            primary | Modifiers::SHIFT,
            Code::KeyW,
            CommandKind::CloseWindow,
        );
        bind(primary, Code::KeyN, CommandKind::NewWindow);
        bind(primary, Code::KeyL, CommandKind::FocusAddressBar);
        bind(primary, Code::KeyR, CommandKind::Reload);
        // Not bare Escape: pages need it to close menus and dialogs.
        bind(primary, Code::Period, CommandKind::Stop);
        bind(
            primary | Modifiers::SHIFT,
            Code::BracketRight,
            CommandKind::NextTab,
        );
        bind(
            primary | Modifiers::SHIFT,
            Code::BracketLeft,
            CommandKind::PreviousTab,
        );
        bind(Modifiers::CONTROL, Code::Tab, CommandKind::NextTab);
        bind(
            Modifiers::CONTROL | Modifiers::SHIFT,
            Code::Tab,
            CommandKind::PreviousTab,
        );
        bind(primary, Code::Comma, CommandKind::OpenSettings);
        bind(primary, Code::KeyQ, CommandKind::Quit);
        match platform {
            Platform::MacOs => {
                // Not Cmd+Arrow: those move the caret to line start and
                // end in text fields.
                bind(primary, Code::BracketLeft, CommandKind::Back);
                bind(primary, Code::BracketRight, CommandKind::Forward);
            }
            Platform::Windows => {
                bind(Modifiers::ALT, Code::ArrowLeft, CommandKind::Back);
                bind(Modifiers::ALT, Code::ArrowRight, CommandKind::Forward);
                bind(Modifiers::empty(), Code::F5, CommandKind::Reload);
            }
        }
        keymap
    }

    /// Bind `chord` to `kind`, replacing any previous binding of that chord.
    pub fn bind(&mut self, chord: KeyChord, kind: CommandKind) {
        // Normalise so a chord built with lock keys or other irrelevant
        // modifiers is stored in the same form `resolve` matches on.
        let chord = KeyChord::new(chord.modifiers, chord.code);
        self.bindings.retain(|(c, _)| *c != chord);
        self.bindings.push((chord, kind));
    }

    pub fn resolve(&self, chord: KeyChord) -> Option<CommandKind> {
        let chord = KeyChord::new(chord.modifiers, chord.code);
        self.bindings
            .iter()
            .find(|(c, _)| *c == chord)
            .map(|(_, kind)| *kind)
    }

    /// The first chord bound to `kind`, for showing hints in menus.
    pub fn chord_for(&self, kind: CommandKind) -> Option<KeyChord> {
        self.bindings
            .iter()
            .find(|(_, k)| *k == kind)
            .map(|(c, _)| *c)
    }

    pub fn bindings(&self) -> impl Iterator<Item = (KeyChord, CommandKind)> + '_ {
        self.bindings.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_defaults_use_command_key() {
        let k = Keymap::defaults(Platform::MacOs);
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::META, Code::KeyT)),
            Some(CommandKind::NewTab)
        );
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::CONTROL, Code::KeyT)),
            None
        );
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::META, Code::BracketLeft)),
            Some(CommandKind::Back)
        );
        assert_eq!(
            k.resolve(KeyChord::new(
                Modifiers::META | Modifiers::SHIFT,
                Code::BracketLeft
            )),
            Some(CommandKind::PreviousTab)
        );
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::META, Code::Period)),
            Some(CommandKind::Stop)
        );
    }

    #[test]
    fn keys_pages_need_are_left_unbound() {
        for platform in [Platform::MacOs, Platform::Windows] {
            let k = Keymap::defaults(platform);
            assert_eq!(
                k.resolve(KeyChord::new(Modifiers::empty(), Code::Escape)),
                None
            );
            assert_eq!(
                k.resolve(KeyChord::new(Modifiers::META, Code::ArrowLeft)),
                None
            );
            assert_eq!(
                k.resolve(KeyChord::new(Modifiers::META, Code::ArrowRight)),
                None
            );
        }
    }

    #[test]
    fn windows_defaults_use_control_key() {
        let k = Keymap::defaults(Platform::Windows);
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::CONTROL, Code::KeyT)),
            Some(CommandKind::NewTab)
        );
        assert_eq!(k.resolve(KeyChord::new(Modifiers::META, Code::KeyT)), None);
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::ALT, Code::ArrowLeft)),
            Some(CommandKind::Back)
        );
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::empty(), Code::F5)),
            Some(CommandKind::Reload)
        );
    }

    #[test]
    fn every_command_kind_has_a_default_chord_on_both_platforms() {
        for platform in [Platform::MacOs, Platform::Windows] {
            let k = Keymap::defaults(platform);
            for kind in CommandKind::ALL {
                assert!(
                    k.chord_for(kind).is_some(),
                    "{kind:?} has no default on {platform:?}"
                );
            }
        }
    }

    #[test]
    fn irrelevant_modifiers_are_ignored_when_matching() {
        let k = Keymap::defaults(Platform::MacOs);
        let with_caps_lock = KeyChord {
            modifiers: Modifiers::META | Modifiers::CAPS_LOCK,
            code: Code::KeyT,
        };
        assert_eq!(k.resolve(with_caps_lock), Some(CommandKind::NewTab));
    }

    #[test]
    fn bind_replaces_an_existing_chord() {
        let mut k = Keymap::defaults(Platform::MacOs);
        let chord = KeyChord::new(Modifiers::META, Code::KeyT);
        k.bind(chord, CommandKind::Reload);
        assert_eq!(k.resolve(chord), Some(CommandKind::Reload));
        assert_eq!(k.bindings().filter(|(c, _)| *c == chord).count(), 1);
    }

    #[test]
    fn bind_normalises_irrelevant_modifiers_so_the_chord_resolves() {
        let mut k = Keymap::default();
        let noisy = KeyChord {
            modifiers: Modifiers::META | Modifiers::CAPS_LOCK,
            code: Code::KeyT,
        };
        k.bind(noisy, CommandKind::NewTab);
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::META, Code::KeyT)),
            Some(CommandKind::NewTab)
        );
        assert_eq!(k.resolve(noisy), Some(CommandKind::NewTab));
        assert_eq!(
            k.bindings().next(),
            Some((
                KeyChord::new(Modifiers::META, Code::KeyT),
                CommandKind::NewTab
            ))
        );
    }

    #[test]
    fn bind_replaces_existing_chord_given_irrelevant_modifiers() {
        let mut k = Keymap::defaults(Platform::MacOs);
        let noisy = KeyChord {
            modifiers: Modifiers::META | Modifiers::CAPS_LOCK,
            code: Code::KeyT,
        };
        k.bind(noisy, CommandKind::Reload);
        assert_eq!(
            k.resolve(KeyChord::new(Modifiers::META, Code::KeyT)),
            Some(CommandKind::Reload)
        );
        assert_eq!(
            k.bindings()
                .filter(|(c, _)| c.code == Code::KeyT && c.modifiers == Modifiers::META)
                .count(),
            1
        );
    }

    #[test]
    fn into_command_fills_in_window_and_tab() {
        let w = WindowId(3);
        let t = TabId(9);
        assert_eq!(
            CommandKind::NewTab.into_command(w, t),
            Command::NewTab {
                window: w,
                url: None
            }
        );
        assert_eq!(
            CommandKind::CloseTab.into_command(w, t),
            Command::CloseTab(t)
        );
        assert_eq!(CommandKind::NextTab.into_command(w, t), Command::NextTab(w));
        assert_eq!(CommandKind::Quit.into_command(w, t), Command::Quit);
    }
}
