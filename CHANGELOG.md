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
