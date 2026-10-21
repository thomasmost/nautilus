# Nautilus

A desktop web browser for macOS and Windows, written in Rust. Pages render
through Chromium, embedded with the Chromium Embedded Framework (CEF); the
window chrome (tab strip, address bar, settings) is drawn with egui. There
is no HTML in the chrome. Nautilus is the Chromium sibling of
[Paladin](https://github.com/thomasmost/paladin), which runs the same
chrome and model on the Servo engine: both are built on
[osmena](https://github.com/thomasmost/osmena), the shared shell, and
this repository holds only what is Chromium's.

## Features

- **Chromium under the hood.** Pages render through Chromium (CEF 154),
  with its site isolation, sandboxless for now (see Known limitations). The
  chrome is drawn with egui; menus are the system's own popup menus.
- **Crashes stay in their tab.** Each page runs in one of Chromium's
  renderer processes. A renderer that crashes shows an error panel in that
  tab with the reason; every other tab keeps working.
- **Multi-select tabs.** Cmd/Ctrl-click and Shift-click select several
  tabs, then drag them together onto another window or onto the desktop
  for a new one, or right-click to close them all, move them to a window,
  or open them in another profile.
- **Profiles.** Each profile has its own settings, cookies, and local
  storage; `--profile <name>` picks one, and a tab can be handed to another
  profile from its context menu.
- **Your colour.** Settings offers a row of swatches that tint the tab strip
  and toolbar per profile.
- **Sleek by default.** No system title bar: the tab strip is the title
  bar, with the window controls at its end. "Use the system title bar and
  borders" in Settings brings the frame back.
- **Chrome extensions.** Install from the Chrome Web Store (click
  "Add to Chrome" on an extension's page, pick "Install the extension
  on this page" from the puzzle-piece menu, or paste its URL or id into
  Settings) or load
  an unpacked directory; pin them to the toolbar, where their popups
  open under the button; switch them off or remove them in Settings.
- **Tab folders and bookmark groups.** Fold a selection of tabs behind
  one chip in the strip, or bookmark it as a named group to reopen from
  the menu.
- **Chromium's right-click menu.** Open a link in a new tab or window,
  save or copy it, copy or save an image, loop a video, copy, paste, or
  search selected text, print, view the source, or inspect an element in
  Chromium's developer tools; Ctrl-click and middle-click open links in
  background tabs, Shift-click in a new window.
- **Search from the address bar.** Text that is not a URL is searched,
  with Google unless Settings names another engine.
- **Keyboard first.** Every control is a command with Chromium's shortcut
  (Cmd for Ctrl on macOS), chords work even while the address bar has
  focus, and Settings remaps them the way a game's controls screen does:
  a primary and a secondary slot per action, click and press.
- **Engine-agnostic design.** Only one crate imports CEF, and it implements
  osmena's `Engine` trait; the model and the UI are osmena's, shared with
  Paladin and unit-tested without an engine.

## Stack

| Layer     | Choice                                                            |
| --------- | ----------------------------------------------------------------- |
| Language  | Rust 1.97.1 (pinned in `rust-toolchain.toml`), edition 2024       |
| Engine    | Chromium 154 through CEF (the `cef` crate 154.5), confined to `crates/nautilus-engine` |
| Shell     | [osmena](https://github.com/thomasmost/osmena): the model, the egui 0.34 chrome on winit 0.30, the settings window, pinned by git revision |
| Chrome GL | osmena's glutin surface (Chromium paints into memory, so the chrome owns the GL context) |
| Pages     | CEF off-screen rendering: each tab paints BGRA frames that osmena draws as a texture |
| Packaging | `cargo xtask bundle-mac` for the macOS `.app` (CI zips it), cargo-packager's NSIS installer on Windows |
| CI        | GitHub Actions on macOS and Windows runners                       |

## Workspace layout

```
crates/
  nautilus-engine/  CEF glue: osmena's Engine and TabHost over CEF, handlers -> events, frames
  nautilus/         The binary: the Brand, Chromium's process preamble, the command line, packaging metadata
xtask/              `cargo xtask icons` (osmena's icon task for logo.svg -> icns/ico/png)
assets/
  logo.svg          The shell logo (pure vector)
  icons/            Generated from logo.svg; committed
docs/
  design-plans/     Approved designs
  upstream-issues.md
```

## Local development

The model, chrome, and winit application come from the osmena crates,
pinned by git revision in the workspace `Cargo.toml`. To change them
alongside Nautilus, check osmena out next to this repository and patch the
source to it in `~/.cargo/config.toml` (osmena's README has the table);
then bump the `rev`s here once the change is pushed.

Prerequisites: a Rust toolchain via `rustup` (the pinned version installs on
first `cargo` run), and the tools that build CEF's C++ wrapper library:
CMake and Ninja on `PATH`, plus a C++ toolchain (Xcode Command Line Tools
on macOS; Visual Studio 2022 Build Tools with the "Desktop development with
C++" workload on Windows). No Python, LLVM, or GStreamer.

The `cef-dll-sys` crate's build script downloads the CEF binary distribution
(about 420 MB on Windows) into its `OUT_DIR` unless `CEF_PATH` points at a
shared copy, which survives `cargo clean` and is reused by every checkout.
To set one up once:

```sh
cargo install export-cef-dir --version 154.5.0 --locked
export-cef-dir --force ~/.local/share/cef        # downloads and unpacks
export CEF_PATH=~/.local/share/cef               # add to your shell profile
```

On Windows the build copies the CEF runtime files (`libcef.dll`, the
`.pak` resources, `locales/`, and the rest) next to the binaries in
`target/<profile>/`, so `cargo run` works from the tree.

On macOS Chromium runs only from an app bundle (the framework and its
helper apps have to sit at fixed places around the executable), so
`cargo run` cannot start it there. Build the bundle instead and run it:

```sh
cargo xtask bundle-mac            # builds the release binaries first
open target/release/Nautilus.app  # or, with arguments:
target/release/Nautilus.app/Contents/MacOS/Nautilus --profile dev https://example.com
```

1. **Build and run:** `cargo run` (Windows) or the bundle (macOS, above)
   opens a window at the homepage. Pass URLs to open them as tabs, or
   `--profile <name>` to use another profile.
2. **Icons:** edit `assets/logo.svg`, run `cargo xtask icons`, and commit
   the regenerated files in `assets/icons/`. CI fails if they are stale.

> The first build compiles CEF's `libcef_dll_wrapper` with CMake (a minute
> or two) and then Nautilus's own crates. Incremental builds take seconds.

Profiles live in `~/Library/Application Support/Nautilus/profiles/<name>/`
on macOS and `%APPDATA%\Nautilus\profiles\<name>\` on Windows, holding
`settings.toml` (homepage, new-tab URL, chrome colour) and Chromium's
`chromium/` directory (cookies, local storage, cache). A profile directory
belongs to one running Nautilus at a time.

## Tests

- `cargo test --workspace` — unit tests in `nautilus-engine` (frame
  composition, key mapping; nothing starts Chromium). The model, chrome,
  and routing tests live in osmena.
- `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo fmt --all --check` — what CI runs.

## Manual checklist

Run with `cargo run` after any change to the chrome, input handling, or
engine glue. Each item must pass.

1. **Launch.** A window opens with one tab at the homepage and the page
   renders. There is no system title bar: the tab strip ends in the ☰
   menu and the minimize, maximize, and close controls, and the taskbar
   shows the shell logo with the title Nautilus. Dragging the strip's
   empty space moves the window, double-clicking it maximizes, and the
   pointer becomes a resize arrow at every edge and corner.
2. **Address bar.** Type `example.com` and press Enter: the page loads, the
   tab title becomes "Example Domain", and the address bar shows
   `https://example.com/`. Typing `localhost:8080` targets `http://`.
   Type `rust programming language` and press Enter: a Google search for
   it loads. Settings has Search engine and Search URL fields; change
   the URL to `https://duckduckgo.com/?q={query}` and the next search
   goes there.
3. **History.** Navigate to a second page, then use Back and Forward: the
   buttons enable and disable with the history and the pages change. Reload
   reloads the current page; while a page loads, a Stop button appears
   beside it and stops the load.
4. **Tabs.** "+" or Cmd+T (Ctrl+T on Windows) opens a new tab, activates
   it, and puts the keyboard focus in the address bar with its text
   selected, so typing a URL and pressing Enter loads it in the new tab;
   clicking a tab title switches tabs and the address bar follows; "×" and
   middle-click close a tab and the neighbour to the right becomes active;
   closing the last tab closes the window and the process exits.
   **Selection.** With three tabs open, Cmd-click (Ctrl-click on Windows)
   the first: it gets a tinted fill while the third stays active; Shift-click
   the first from the third selects all three; a plain click leaves only
   the clicked tab selected. Right-click a selected tab: the menu offers
   "Close 3 tabs", "Close other tabs" (disabled when all are selected),
   "Move 3 tabs to new window" (same), "Move 3 tabs to window" (disabled
   with one window), and "Open 3 tabs in profile" (disabled with no other
   profile directory). Right-click an unselected tab: it becomes the active
   and only selected tab and the menu says "Close tab". Open a second window
   with Cmd+N, select two tabs in the first, and drag one of them onto the
   second window: both appear at the end of its strip (reloaded, with the
   previously active one active) and leave the first. Drag a tab onto the
   desktop: it opens in a new window. Dropping within the same window does
   nothing. "Open tab in profile" starts a second Nautilus for that profile
   with the tab's URL and closes the tab here. Both this menu and the "☰"
   menu are the system's own popup menus.
5. **Crashed tab.** Open `chrome://crash` in a tab: the tab's error panel
   says the renderer process crashed, the other tabs keep working, and the
   terminal shows `tab crashed: ...`; Back, Forward, and Reload clear the
   panel.
6. **Resize and HiDPI.** Resize the window: the page follows within a frame
   and stays aligned under the toolbar. Drag the window to a display with a
   different scale factor: text stays sharp and clicks land where expected.
7. **Input.** Scrolling with the wheel or trackpad over the page scrolls it;
   clicking a text field on a page and typing enters text; double-click
   selects a word; typing in the address bar does not reach the page.
   Hovering a link shows its URL in the status overlay at the bottom left
   and the pointer becomes a hand; over text it becomes an I-beam. A
   `<select>` opens its list over the page and picks on click.
   **Page menu.** Right-click a link: a native menu at the pointer offers
   Open link in new tab, Open link in new window, Save link as…, Copy
   link address, and Inspect. "Open link in new tab" adds a tab right
   after this one without leaving this one; "Copy link address" then
   Ctrl+V in the address bar pastes the URL. Right-click an image: Open
   image in new tab, Save image as…, Copy image, Copy image address.
   Right-click a text field: Undo, Redo, Cut, Copy, Paste, Paste as
   plain text, Delete, Select all, with their shortcuts, and the
   spelling suggestions for a misspelled word. On macOS, Cmd+Z, Cmd+X,
   Cmd+C, Cmd+V, Shift+Cmd+V, and Cmd+A pressed in the field do what
   those entries do (Nautilus carries them out itself; there is no
   Edit menu). Select some text and
   right-click it: Copy, Search Google for "…", Print…. Right-click
   empty page: Back, Forward, Reload (with their shortcuts), Save as…,
   Print…, View page source, Inspect. "Save link as…" opens the system's
   Save As dialog and, once saved, the status overlay reads "Saved
   <name>". "View page source" opens a `view-source:` tab. "Inspect"
   opens Chromium's developer tools in a window of their own, on the
   element; closing the tab closes them.
8. **Page-opened tabs.** Middle-click or Ctrl-click a link: a new tab
   opens at the URL right after the current one, which stays active
   (Ctrl+Shift-click activates the new tab). Shift-click a link: it
   opens in a new window. Let a page call `window.open`: a new tab opens
   at the URL in the same window and becomes active.
9. **Windows and menu.** Cmd+N (Ctrl+N on Windows), or New window chosen
   with the mouse from the "☰" menu, opens a second window; each window's
   tabs, address bar, and page are independent, and closing one leaves the
   other working.
10. **Shortcuts.** With Cmd on macOS or Ctrl on Windows: T new tab, W close
   tab (also Ctrl+F4 on Windows), Shift+T reopens the last closed tab,
   Shift+W close window, N new window, L focus the address bar with its
   text selected (also Alt+D on Windows), R reload (also F5 on Windows),
   period Stop, Shift+] and Shift+[ on macOS or Tab and
   Shift+Tab and PgDn/PgUp everywhere next and previous tab (wrapping),
   1 to 8 the tab at that position and 9 the last tab, comma opens
   settings, Q quits. Back and forward are Cmd+[ and Cmd+] on macOS and
   Alt+Left and Alt+Right on Windows; the homepage is Cmd+Shift+H or
   Alt+Home; F11 (Ctrl+Cmd+F on macOS) shows the page alone and again
   brings the chrome back. A chord held down fires once, and none of its
   key strokes reach the page. Shortcuts also work while the address bar
   has focus.
11. **Settings.** Settings (Cmd+, or chosen with the mouse from the menu)
    opens a window with Homepage, New tab URL, a row of chrome colour
    swatches, the "Use the system title bar and borders" checkbox, the
    restore checkbox, and the Keyboard shortcuts table. Edits are saved when a field loses
    focus or the window closes; relaunching opens the new homepage. Click a
    swatch: every browser window's tab strip takes that colour at once.
    **Title bar.** Tick "Use the system title bar and borders": every open
    window gets the system frame at once on Windows (on macOS, the next
    window opened); untick it and the strip is the title bar again.
    **Shortcuts.** Click the primary slot of New tab: it reads "Press
    keys…"; press Ctrl+Y (Cmd+Y): the slot shows it, `keymap.toml` in the
    profile directory holds `new-tab = ["Ctrl+KeyY"]` (Meta on macOS), and
    Ctrl+Y opens a tab while Ctrl+T no longer does. Give Reload's secondary
    slot Ctrl+Y: New tab loses it and a note under the table says so.
    Right-click a slot to clear it; "Reset to defaults" puts everything
    back and the file lists the defaults.
12. **Extensions.** The toolbar ends in a puzzle piece. Open
    <https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh>,
    click the puzzle piece: "Install the extension on this page" is
    enabled; pick it (or, when the store shows its "Add to Chrome"
    button, click that: the page says the install was cancelled, and
    the install still happens). Within a few seconds the puzzle menu lists uBlock
    Origin Lite; Settings (Ctrl+,) lists it under Extensions with
    Enabled ticked, and the profile has
    `chromium/extensions/ddkjiahejlhfcafbddmgiahcphecmpfh/manifest.json`
    with a `key` field. Tick "In toolbar": its icon appears beside the
    address bar (and the menu's "Pin to toolbar" submenu shows it
    checked). Click the icon: the extension's popup opens under it, sized
    to its content; Escape closes it, as does a click back into the
    window; clicking the icon again while it is open closes it. Untick
    Enabled: the icon goes; tick it again: it is back. "Load unpacked…"
    opens a folder dialog; cancelling it is not an error. Remove: the
    extension leaves the list, the menu, and the directory. Relaunch:
    installed extensions are still there, and no Chromium window appears
    (the management page runs in a window that is never shown).
13. **Folders and bookmarks.** Open three tabs. Right-click the second:
    the menu has "Close all selected (1)" (disabled with one selected),
    "Move tab to folder…", and "Bookmark all selected (1)…". Pick "Move
    tab to folder…": a prompt over the page offers "Folder A"; Enter.
    The strip shows a chip "▸ Folder A (1)" in the tab's place; click
    it: a list drops down with the tab, and clicking the tab there
    activates it. Right-click the chip: Expand, Rename…, Ungroup, Close
    folder (1 tab). Expand: the tab appears after the chip, which reads
    "▾ Folder A". Ctrl-click the other two tabs, right-click one, "Move
    2 tabs to folder…", pick Folder A from the existing-folder choice:
    the folder holds three tabs, together. "Bookmark all selected…" on a
    selection asks for a group name; type Work, Enter: the profile has
    `bookmarks.toml` with the pages, and ☰ → Bookmarks → Work lists
    them with "Open all (n)", which opens each in a new tab. Folders are
    gone after a relaunch; bookmarks stay.
14. **Clean exit.** Closing the last window and Cmd+Q both exit with code 0,
    and no `nautilus` process is left behind (Task Manager / `ps`).

## Packaging

### macOS

```sh
cargo xtask bundle-mac
```

builds the release binaries (`nautilus` and `nautilus-helper`), lays out
`target/release/Nautilus.app` the way CEF needs it (`Chromium Embedded
Framework.framework` under `Contents/Frameworks`, copied from the CEF
distribution at `CEF_PATH`, and the five helper apps beside it, `Nautilus
Helper`, `… (GPU)`, `… (Renderer)`, `… (Plugin)`, `… (Alerts)`, each a
copy of `nautilus-helper`), writes the property lists, and signs the
bundle ad hoc (Apple silicon runs no unsigned code; there is no
Developer ID yet). CI builds it on every push to `main`, starts it once
on the runner as a smoke test (a local page with `--exit-after`: the
browser must exit cleanly and must have read the page's title, which
proves the helper processes ran from the bundle), and uploads the
workflow artifact `nautilus-macos-15` with `Nautilus-macos.zip`,
`Nautilus.dSYM.zip` (the symbols, for crash logs) and the smoke test's
log. A bundle downloaded from CI carries the quarantine attribute and,
being signed ad hoc, is refused as "damaged" when opened; clear the
attribute first:

```sh
xattr -dr com.apple.quarantine Nautilus.app
```

There is no `.dmg` yet.

### Windows

`cargo install cargo-packager --version 0.11.8 --locked` once, then:

```bash
cargo build --release -p nautilus
cargo packager --release -p nautilus
```

`cargo packager` also runs `cargo build --release -p nautilus` itself (its
`before-packaging-command`), so the bundle always matches the current
source. The installer picks up `libcef.dll` and the other CEF runtime files
that the build copied into `target/release/` (the `resources` globs in
`crates/nautilus/Cargo.toml`). Output lands in `target/release/` as
`nautilus_<version>_x64-setup.exe` (NSIS, per-user install), unsigned. CI
builds it on every push to `main` and uploads it as the workflow artifact
`nautilus-windows-2025`, together with `nautilus.pdb` (the release profile
keeps line tables) so a crash in an installed build can be symbolized.

### Windows prerequisites

- Visual Studio 2022 Build Tools with the "Desktop development with C++"
  workload (MSVC v143 and a Windows 10/11 SDK).
- CMake and Ninja on `PATH` (`winget install Kitware.CMake Ninja-build.Ninja`).
- Rust via https://win.rustup.rs/; the pinned toolchain installs on first use.
- The Microsoft Visual C++ Redistributable, which the installer does not
  bundle: https://aka.ms/vs/17/release/vc_redist.x64.exe.

### Known limitations

Upstream causes and the features they block are tracked in
`docs/upstream-issues.md`.

- **No renderer sandbox.** CEF's sandbox on Windows requires the browser to
  be started through CEF's `bootstrap.exe` with the browser built as a DLL,
  and on macOS a signed helper layout; Nautilus starts CEF with
  `no_sandbox` until that is built (roadmap). Site isolation and separate
  renderer processes still apply.
- Page frames go through system memory (CEF's software off-screen path);
  CEF's shared-texture path would skip the copy and is a later step.
- `window.open` opens a tab at the URL but the opener gets no handle to it
  (the call returns null), so pages that script the new window do not work.
- No IME composition for page text fields yet: egui-winit only enables IME
  for its own fields.
- A download's progress shows only in the status overlay; there is no
  downloads list, and a download in progress is cancelled when the
  browser exits.
- Without the system title bar, Windows 11's snap-layouts flyout on the
  maximize control is not available (snap by dragging still is), and the
  frameless window has not been tried on macOS.
- macOS has had one run by hand (2026-10-20: launch, a page, the
  extension manager starting and shutting down); the rest of the manual
  checklist is untried there (see the handoff in `CLAUDE.md`).
- Extensions: an action popup on macOS is a borderless child window of
  the browser window (`popup.rs`); it was built from the Windows
  behaviour and not yet clicked by hand. Extension popups see Chromium's own tabs,
  not Nautilus's: `chrome.tabs.query({active: true})` finds the hidden
  management page, so a popup that acts on "the current tab" acts on
  nothing. An action without a popup cannot be clicked. The Web Store
  may show "Switch to Chrome" instead of an install button (install
  through the puzzle menu or Settings then); when it shows "Add to
  Chrome", the button installs through Nautilus but the page reports
  the install as cancelled. Store extensions do not update themselves.
  Themes and apps are not listed. Chromium records an install, switch, or
  removal in its preferences up to ten seconds later, so closing the
  browser within that time keeps the process alive, windowless, until
  it has.
- No macOS application menu: winit's default menu is turned off so that
  Cmd+Q reaches Nautilus's own shortcut handling. Chromium serves the
  editing shortcuts in pages (Cmd+C and the rest) from the Edit menu on
  macOS, so Nautilus carries those out itself for a key the page did
  not handle; other menu-driven shortcuts (Cmd+H to hide, Cmd+M to
  minimize) do nothing.
- Restore previous windows on launch is stored in `settings.toml` but not yet
  acted on.
- No bookmarks, history UI, downloads UI, extensions, sync, or auto-update.
- Thorium (a Chromium fork with compiler and feature patches) publishes no
  CEF distribution; building CEF from Thorium's tree is possible but is a
  multi-hour Chromium build, so Nautilus tracks stock CEF for now. See
  `docs/design-plans/2026-10-19-nautilus-chromium-browser.md`.

## Todo

- [ ] macOS `.dmg`, and a Developer ID signature in place of the ad-hoc one
- [ ] Renderer sandbox (`bootstrap.exe` on Windows, helper layout on macOS)
- [ ] Accelerated off-screen rendering (shared textures)
- [ ] IME composition for page text fields
- [ ] Restore previous windows on launch (the setting is stored but not acted on)
- [ ] A downloads list, history
