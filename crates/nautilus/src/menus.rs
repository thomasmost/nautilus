//! The browser's menus as data: which entries they hold, which are enabled,
//! and the `Command` each one issues. Pure and unit-tested; `native_menu`
//! turns an entry list into a native menu and maps the click back.

use nautilus_core::{Command, CommandKind, KeyChord, Keymap, TabId, Window, WindowId};

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item {
        label: String,
        enabled: bool,
        /// Shown beside the label as the shortcut, when the command has one.
        chord: Option<KeyChord>,
        command: Command,
    },
    Submenu {
        label: String,
        enabled: bool,
        entries: Vec<MenuEntry>,
    },
    Separator,
}

impl MenuEntry {
    fn item(label: impl Into<String>, enabled: bool, command: Command) -> MenuEntry {
        MenuEntry::Item {
            label: label.into(),
            enabled,
            chord: None,
            command,
        }
    }

    fn bound(
        keymap: &Keymap,
        kind: CommandKind,
        label: &str,
        window: WindowId,
        tab: TabId,
    ) -> MenuEntry {
        MenuEntry::Item {
            label: label.to_string(),
            enabled: true,
            chord: keymap.chord_for(kind),
            command: kind.into_command(window, tab),
        }
    }
}

/// The window menu behind the "☰" button.
pub fn window_menu(window: &Window, keymap: &Keymap) -> Vec<MenuEntry> {
    let tab = window.active;
    vec![
        MenuEntry::bound(keymap, CommandKind::NewWindow, "New window", window.id, tab),
        MenuEntry::bound(
            keymap,
            CommandKind::OpenSettings,
            "Settings",
            window.id,
            tab,
        ),
        MenuEntry::Separator,
        MenuEntry::bound(keymap, CommandKind::Quit, "Quit", window.id, tab),
    ]
}

/// The context menu of a tab, acting on `targets` (the selection, or the
/// right-clicked tab alone). `others` are the other windows (id and label)
/// and `profiles` the other profiles on this machine. Moves recreate the
/// tabs at their URLs: a Chromium browser is bound to the native window
/// it was created for.
pub fn tab_menu(
    window: &Window,
    targets: &[TabId],
    others: &[(WindowId, String)],
    profiles: &[String],
) -> Vec<MenuEntry> {
    let count = targets.len();
    let plural = |one: &str, many: &str| {
        if count == 1 {
            one.to_string()
        } else {
            many.replace("{n}", &count.to_string())
        }
    };
    let rest: Vec<TabId> = window
        .tabs
        .iter()
        .map(|t| t.id)
        .filter(|t| !targets.contains(t))
        .collect();
    let has_rest = !rest.is_empty();

    vec![
        MenuEntry::item(
            plural("Close tab", "Close {n} tabs"),
            true,
            Command::CloseTabs(targets.to_vec()),
        ),
        MenuEntry::item("Close other tabs", has_rest, Command::CloseTabs(rest)),
        MenuEntry::Separator,
        MenuEntry::item(
            plural("Move tab to new window", "Move {n} tabs to new window"),
            has_rest,
            Command::MoveTabsToNewWindow(targets.to_vec()),
        ),
        MenuEntry::Submenu {
            label: plural("Move tab to window", "Move {n} tabs to window"),
            enabled: !others.is_empty(),
            entries: others
                .iter()
                .map(|(id, label)| {
                    MenuEntry::item(
                        label.clone(),
                        true,
                        Command::MoveTabsToWindow {
                            tabs: targets.to_vec(),
                            window: *id,
                        },
                    )
                })
                .collect(),
        },
        MenuEntry::Submenu {
            label: plural("Open tab in profile", "Open {n} tabs in profile"),
            enabled: !profiles.is_empty(),
            entries: profiles
                .iter()
                .map(|name| {
                    MenuEntry::item(
                        name.clone(),
                        true,
                        Command::OpenTabsInProfile {
                            tabs: targets.to_vec(),
                            profile: name.clone(),
                        },
                    )
                })
                .collect(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::{Platform, Tab};
    use url::Url;

    fn window_with(n: u64) -> Window {
        let mut w = Window::with_tab(
            WindowId(1),
            Tab::new(TabId(1), Url::parse("https://a.example/").unwrap()),
        );
        for i in 2..=n {
            w.tabs.push(Tab::new(
                TabId(i),
                Url::parse(&format!("https://{i}.example/")).unwrap(),
            ));
        }
        w
    }

    fn labels(entries: &[MenuEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item { label, .. } | MenuEntry::Submenu { label, .. } => label.clone(),
                MenuEntry::Separator => "-".to_string(),
            })
            .collect()
    }

    fn enabled(entries: &[MenuEntry]) -> Vec<bool> {
        entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item { enabled, .. } | MenuEntry::Submenu { enabled, .. } => *enabled,
                MenuEntry::Separator => true,
            })
            .collect()
    }

    #[test]
    fn window_menu_carries_the_keymap_chords_and_commands() {
        let w = window_with(1);
        let keymap = Keymap::defaults(Platform::Windows);
        let entries = window_menu(&w, &keymap);
        assert_eq!(
            labels(&entries),
            vec!["New window", "Settings", "-", "Quit"]
        );
        let MenuEntry::Item { chord, command, .. } = &entries[0] else {
            panic!("first entry is an item");
        };
        assert_eq!(*chord, keymap.chord_for(CommandKind::NewWindow));
        assert_eq!(*command, Command::NewWindow);
        assert!(matches!(
            &entries[3],
            MenuEntry::Item {
                command: Command::Quit,
                ..
            }
        ));
    }

    #[test]
    fn tab_menu_for_one_tab_of_three() {
        let w = window_with(3);
        let entries = tab_menu(
            &w,
            &[TabId(2)],
            &[(WindowId(7), "Other".to_string())],
            &["work".to_string()],
        );
        assert_eq!(
            labels(&entries),
            vec![
                "Close tab",
                "Close other tabs",
                "-",
                "Move tab to new window",
                "Move tab to window",
                "Open tab in profile",
            ]
        );
        assert_eq!(enabled(&entries), vec![true; 6]);
        assert!(matches!(
            &entries[1],
            MenuEntry::Item { command: Command::CloseTabs(rest), .. } if *rest == vec![TabId(1), TabId(3)]
        ));
        let MenuEntry::Submenu {
            entries: windows, ..
        } = &entries[4]
        else {
            panic!("move to window is a submenu");
        };
        assert_eq!(labels(windows), vec!["Other"]);
        assert!(matches!(
            &windows[0],
            MenuEntry::Item { command: Command::MoveTabsToWindow { tabs, window: WindowId(7) }, .. }
                if *tabs == vec![TabId(2)]
        ));
        let MenuEntry::Submenu {
            entries: profiles, ..
        } = &entries[5]
        else {
            panic!("open in profile is a submenu");
        };
        assert!(matches!(
            &profiles[0],
            MenuEntry::Item { command: Command::OpenTabsInProfile { profile, .. }, .. } if profile == "work"
        ));
    }

    #[test]
    fn tab_menu_counts_the_selection_and_disables_what_cannot_apply() {
        let w = window_with(3);
        let all = [TabId(1), TabId(2), TabId(3)];
        let entries = tab_menu(&w, &all, &[], &[]);
        assert_eq!(labels(&entries)[0], "Close 3 tabs");
        assert_eq!(labels(&entries)[3], "Move 3 tabs to new window");
        // Close others, move to new window: nothing left; no other windows
        // or profiles: the submenus are disabled.
        assert_eq!(
            enabled(&entries),
            vec![true, false, true, false, false, false]
        );
    }
}
