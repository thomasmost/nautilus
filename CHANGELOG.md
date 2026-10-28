# Changelog

All notable changes to Nautilus are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Nautilus
has no releases yet; `Unreleased` holds everything on `main`.

## [Unreleased]

### Added

- Profiles can be named, coloured and given an avatar, and made and
  deleted from the browser: the avatar at the toolbar's end opens the
  Profiles menu (the macOS menu bar has the same), with Edit…, the other
  profiles, Add Profile… and Delete Profile. A profile may share the
  Default profile's bookmarks and copy its extensions; history is never
  shared. The window title names a non-default profile (osmena).
- The profile dialog offers its avatars in two palettes: "Sea
  creatures" (whale, spouting whale, dolphin, octopus, shell, fish,
  tropical fish, blowfish, turtle, penguin) and "Assorted" (osmena).
- Two environment variables for scripted runs: `NAUTILUS_CHROMIUM_SWITCHES`
  (Chromium command-line switches, comma-separated, `name` or
  `name=value`) and `NAUTILUS_CEF_LOG` (`info` or `verbose` for
  Chromium's own log on stderr, warnings otherwise).
- A fresh profile's chrome is Shell, a warm sand gradient over the tab
  strip fading into an off-white toolbar; the other swatches are still
  in the settings window. A profile that already chose a colour keeps
  it (osmena).
- On macOS, the menu bar: Nautilus (About, Settings…, Services, Hide,
  Hide Others, Show All, Quit), File, Edit, View, History, Bookmarks,
  Profiles, Tab and Window, with the keymap's shortcuts. The Edit
  entries (Cmd+C and the rest) act on the page or on a focused text
  field of the chrome.
- A new tab (Ctrl+T, the "+" button) opens the launcher, a large field
  over the window, instead of a blank page: type an address or a
  search and press Enter, or pick a page from the history's matches;
  Escape opens nothing. Browsing history (`history.toml` in the
  profile) and its matches under the address bar as you type (osmena).
- Tabs are reordered by dragging them along the strip; the others
  slide out of the way and tabs slide to their places after a close or
  reorder (osmena).
- The back and forward buttons of a mouse go back and forward in the
  active tab (osmena).
- A macOS build for Intel Macs. CI packages an Intel bundle
  (`nautilus-macos-15-intel`, `Nautilus-macos-x86_64.zip`) next to the
  Apple silicon one, which is now named `Nautilus-macos-arm64.zip`.

### Changed

- The chrome's typeface is Inter; the address bar and the launcher's
  field are pills, every input has the same padding, and widgets round
  like the tabs (osmena).
- A paint of the page copies and uploads only the rectangles Chromium
  says changed (a blinking caret, a hover, a GIF) instead of the whole
  frame, when the frame's size is unchanged and no `<select>` popup is
  open.
- Tabs in the strip are all one standard width instead of each taking
  its title's width; when they no longer fit they all shrink alike,
  down to a minimum, and only then does the strip scroll. Titles that
  do not fit are elided, and a narrow inactive tab shows its close
  control only while hovered (osmena).
- The tab strip fades from the chrome colour at its top into the
  toolbar at its bottom edge instead of meeting it along a hard line
  (osmena).

### Fixed

- The new-tab launcher opened dimmed, its field under the grey
  backdrop, once a launcher had been cancelled by a click outside it
  (osmena).
- Text fields showed no caret, and selections were grey, on any page
  reached by navigating from one site to another (a search typed on a
  homepage, a link to another domain) until the window lost and
  regained focus. Chromium starts the renderer of the new site without
  the page focus and asks the view whether it has it, which a
  windowless view never confirms; the engine now tells each committed
  main-frame navigation the focus the shell last gave the tab.
- Slow scrolling with a precision wheel or a trackpad moved the page in
  jumps or not at all: each event's distance was rounded to whole
  pixels on its own, and a small one rounded to nothing. The fraction
  is now carried to the next event.
- Clicking a link sometimes downloaded it instead of opening it, and
  sometimes opened it in a background tab, after the user had switched
  windows with Alt+Tab or closed a tab with Ctrl+W. Nautilus stamped
  mouse events with the modifiers of the last key event it saw, and
  Windows sends no Alt release when Alt+Tab takes the focus (nor does
  a closed tab get its Ctrl release), so the next click on a link was
  an Alt+click (save link) or a Ctrl+click (open in background). Mouse
  events now carry the modifiers from the window's own state (osmena).
- A mouse button held in a page when the window lost focus, or when a
  keyboard tab switch happened mid-drag, stayed down in that page; the
  shell now releases it there (osmena).
- The window redrew continuously, spinning Chromium's message loop each
  time, while the address bar had focus (osmena).
- A folder's drop-down list stayed open after a click on the page
  (osmena).
- Tabs never showed their page's favicon: the list of icon URLs CEF
  hands the engine came out empty through the `cef` crate's clone
  (`docs/upstream-issues.md` C5); it is read through the raw list now.
- "Add to Chrome" on a Chrome Web Store page crashed the browser.
  The store calls Chromium's `webstorePrivate` API, which answers with
  the install dialog modal to the tab's native window; a windowless
  browser has none and Chromium dereferenced null. Store pages now get
  a shim in the renderer that routes "Add to Chrome", "Remove from
  Chrome", and the enable switch to Nautilus's own extension manager,
  the same path as the puzzle menu; the page is told the dialog was
  cancelled and the extension appears in the puzzle menu seconds
  later.
- Windows: typing into a page produced upper-case letters whatever the
  Shift and Caps Lock state, and wrong characters for punctuation keys.
  CEF on Windows reads the character a `Char` key event types from its
  `windows_key_code` field (as `WM_CHAR` carries it), and Nautilus put the
  key's virtual-key code there; the `Char` event now carries the
  character.
- Windows: key events reached pages with an empty `code` (so a page
  telling keys apart by position, a game or an editor, saw none of them).
  CEF derives `code` from the native key code, which Nautilus left at 0;
  it now carries the key's scan code.
- macOS: the tab context menu, the page context menu, and the ☰ menu
  opened along the bottom edge of the window instead of at the pointer
  (osmena's fix: its menu code mirrored the y coordinate for an AppKit
  view with its origin at the bottom, but winit's view has its origin at
  the top).

- macOS: Cmd+Z, Shift+Cmd+Z, Cmd+X, Cmd+C, Cmd+V, Shift+Cmd+V, and
  Cmd+A in a page now undo, redo, cut, copy, paste, paste as plain
  text, and select all. Chromium leaves those to the application's Edit
  menu on macOS, and Nautilus has none; they are now carried out on the
  focused frame when the page itself did not handle the key.
- macOS: the application object now implements the protocol CEF
  requires of it (`CefAppProtocol`: whether `sendEvent:` is on the
  stack), which Chromium's main-thread message pump consults.
- Closing the browser after installing an extension in that session
  crashed inside Chromium's shutdown: the extension manager's hidden
  window never closed (its delegate answered "cannot close") and CEF
  shut down with it alive. The window now closes, and shutdown waits
  for it.

### Changed

- The model, the egui chrome, the settings window, and the winit
  application now come from the [osmena](https://github.com/thomasmost/osmena)
  crates, shared with Paladin. `nautilus-core` is gone; `nautilus-engine`
  implements osmena's `Engine` and `TabHost` traits; the binary is the
  brand, Chromium's process preamble, and the command line. No
  user-visible change.

### Added

- macOS: Chrome extensions, as on Windows. The puzzle-piece menu and the
  pinned actions appear in the toolbar, the Web Store install, unpacked
  loading, and the Extensions section of Settings work through the same
  hidden management page, and an action's popup opens under its button
  in a borderless window of the browser window's.
- macOS: `cargo xtask bundle-mac` builds `Nautilus.app` (the release
  binaries, CEF's framework, the five helper apps, the property lists,
  an ad-hoc signature). CI builds it on every push to `main`, starts it
  once as a smoke test, and uploads it with its symbols as the
  `nautilus-macos-15` artifact. Before this, Nautilus could not start on
  macOS at all.
- `--exit-after <seconds>` (hidden) quits through the normal path after
  logging the open tabs, for smoke tests of a build.
- The page's context menu is now the same native menu as the rest of the
  chrome, shaped like Chromium's: a link opens in a new tab (behind the
  current one, beside it) or a new window, is saved, or has its address
  copied; an image opens in a new tab, is saved, copied, or has its
  address copied; a video or audio loops, shows its controls, is saved,
  or has its address copied; selected text is copied, searched with the
  configured engine, or printed; an editable field gets spelling
  suggestions, Undo, Redo, Cut, Copy, Paste, Paste as plain text,
  Delete, and Select all; a plain page gets Back, Forward, Reload, Save
  as, Print, and View page source; Inspect opens Chromium's developer
  tools on the element in a window of their own.
- Downloads: a link to a file, and "Save … as" from the page menu, ask
  where to save in the system dialog; progress and the outcome show in
  the status overlay.
- Middle-click and Ctrl-click open a link in a background tab beside the
  current one (it used to take its place); Shift-click opens it in a new
  window; Ctrl+Shift-click in a tab in front.
- Address-bar text that is not a URL is searched, with Google by
  default; the Search engine and Search URL fields in Settings (or
  `[search]` in `settings.toml`, where `{query}` stands for the text)
  change the provider. A command-line argument that is not a URL is
  searched too.
- Tab folders: "Move tab to folder…" in a tab's context menu (acting on
  the selection) names a folder ("Folder A", "Folder B", … by default,
  or an existing one) whose tabs sit behind one chip in the strip; the
  chip lists its tabs on a click and its menu expands, collapses,
  renames, ungroups, or closes the folder. "Close all selected" closes
  the selection.
- Bookmark groups: "Bookmark all selected…" names a group (or picks an
  existing one); groups live in `bookmarks.toml` in the profile and open
  from the window menu's "Bookmarks" submenu, a page at a time or all at
  once.
- Chrome extensions. The toolbar ends in a puzzle-piece button whose
  menu lists the installed extensions (click one for its popup), pins
  them to the toolbar, installs the extension on the current Chrome Web
  Store page, opens the store, loads an unpacked directory, and opens
  the settings window, whose Extensions section switches each one on or
  off, pins or removes it, and installs by store URL or id. Pinned
  extensions sit beside the address bar; their popups open under the
  button and close on Escape, on a click back into the window, or when
  the window moves. Installs and removals take effect at once and
  persist. Extensions from the store are unpacked under the profile's
  `chromium/extensions/<id>/` and do not update themselves.
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
