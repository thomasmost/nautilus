# Changelog

All notable changes to Nautilus are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Nautilus
has no releases yet; `Unreleased` holds everything on `main`.

## [Unreleased]

### Changed

- The model, the egui chrome, the settings window, and the winit
  application now come from the [osmena](https://github.com/thomasmost/osmena)
  crates, shared with Paladin. `nautilus-core` is gone; `nautilus-engine`
  implements osmena's `Engine` and `TabHost` traits; the binary is the
  brand, Chromium's process preamble, and the command line. No
  user-visible change.

### Added

- Browser windows open without the system title bar: the tab strip is
  the title bar (drag its empty space, double-click to maximize) and ends
  in the minimize, maximize, and close controls on Windows. "Use the
  system title bar and borders" in Settings restores the frame.
- Chromium's keyboard shortcuts (Cmd for Ctrl on macOS): reopen the last
  closed tab (Ctrl+Shift+T), jump to tab 1 to 8 and to the last tab
  (Ctrl+1 to Ctrl+9), Ctrl+PgDn/PgUp between tabs, Ctrl+F4 closes a tab,
  Alt+D focuses the address bar, Alt+Home (Cmd+Shift+H) goes to the
  homepage, F11 (Ctrl+Cmd+F) shows the page full screen.
- Keyboard shortcuts are remappable in Settings: a primary and a secondary
  slot per action, click one and press the new keys; a shortcut already
  in use moves over, and "Reset to defaults" undoes everything. The
  result is `keymap.toml` in the profile directory.

- The browser: Chromium 154 through the Chromium Embedded Framework, with
  Paladin's egui chrome and model. Windows and tabs, an address bar, back,
  forward, reload, and stop, the status overlay for hovered links, page
  cursors (hand over links, I-beam over text), favicons, `<select>` popups,
  Chromium's own page context menu, and page-opened tabs (`window.open`,
  middle-click) as new tabs in the same window.
- Tab selection and tab moves, as in Paladin: Ctrl-click (Cmd-click on
  macOS) adds a tab to the selection or removes it, Shift-click selects the
  range from the active tab; selected tabs are tinted. Right-click a tab for
  a native menu that acts on the selection (or on that tab): Close tab(s),
  Close other tabs, Move to new window, Move to window, Open in profile.
  Drag selected tabs onto another Nautilus window to move them there, or
  onto the desktop to open a new window with them. A moved tab is recreated
  at its URL in the destination.
- Crashed tabs: a page whose renderer process dies shows the tab's error
  panel with the reason; other tabs keep working.
- Profiles (`--profile <name>`, one Chromium data directory each), the
  settings window with homepage, new-tab URL, and chrome colour swatches,
  the "☰" window menu as a native popup, and the keyboard shortcuts.
- A Windows NSIS installer from cargo-packager, with the CEF runtime files.
