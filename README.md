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
- **Keyboard first.** Every control is a command with a platform-native
  shortcut, and chords work even while the address bar has focus.
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
| Packaging | cargo-packager: `.app` and `.dmg` on macOS (not yet wired up, see Packaging), NSIS installer on Windows |
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

1. **Build and run:** `cargo run` opens a window at the homepage. Pass URLs
   to open them as tabs, or `--profile <name>` to use another profile.
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

1. **Launch.** A window titled Nautilus opens with one tab at the homepage,
   the page renders, and the title bar shows the shell logo.
2. **Address bar.** Type `example.com` and press Enter: the page loads, the
   tab title becomes "Example Domain", and the address bar shows
   `https://example.com/`. Typing `localhost:8080` targets `http://`.
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
   `<select>` opens its list over the page and picks on click. Right-click
   on the page opens Chromium's own context menu at the pointer.
8. **Page-opened tabs.** Middle-click a link, or let a page call
   `window.open`: a new tab opens at the URL in the same window.
9. **Windows and menu.** Cmd+N (Ctrl+N on Windows), or New window chosen
   with the mouse from the "☰" menu, opens a second window; each window's
   tabs, address bar, and page are independent, and closing one leaves the
   other working.
10. **Shortcuts.** With Cmd on macOS or Ctrl on Windows: T new tab, W close
   tab, Shift+W close window, N new window, L focus the address bar with
   its text selected, R reload, period Stop, Shift+] and Shift+[ next and
   previous tab (wrapping), comma opens settings, Q quits. Back and forward
   are Cmd+[ and Cmd+] on macOS and Alt+Left and Alt+Right on Windows. A
   chord held down fires once, and none of its key strokes reach the page.
   Shortcuts also work while the address bar has focus.
11. **Settings.** Settings (Cmd+, or chosen with the mouse from the menu)
    opens a window with Homepage, New tab URL, a row of chrome colour
    swatches, and the restore checkbox. Edits are saved when a field loses
    focus or the window closes; relaunching opens the new homepage. Click a
    swatch: every browser window's tab strip takes that colour at once.
12. **Clean exit.** Closing the last window and Cmd+Q both exit with code 0,
    and no `nautilus` process is left behind (Task Manager / `ps`).

## Packaging

`cargo install cargo-packager --version 0.11.8 --locked` once, then on
Windows:

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

**macOS is not packaged yet.** CEF on macOS needs a specific bundle
layout: `Chromium Embedded Framework.framework` under `Contents/Frameworks`
and one helper app per process kind (`Nautilus Helper`, `... (GPU)`,
`... (Renderer)`, `... (Plugin)`, `... (Alerts)`), each wrapping the
`nautilus-helper` binary, which cargo-packager does not produce. The code
is ready for it (`nautilus-helper`, the framework loader in
`nautilus_engine::run_subprocess`); the bundling step is on the roadmap.
Until then `cargo run` on macOS fails to load the framework.

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
- No search engine: address-bar text that is not a URL is ignored.
- No macOS application menu: winit's default menu is turned off so that
  Cmd+Q reaches Nautilus's own shortcut handling.
- Restore previous windows on launch is stored in `settings.toml` but not yet
  acted on.
- No bookmarks, history UI, downloads UI, extensions, sync, or auto-update.
- Thorium (a Chromium fork with compiler and feature patches) publishes no
  CEF distribution; building CEF from Thorium's tree is possible but is a
  multi-hour Chromium build, so Nautilus tracks stock CEF for now. See
  `docs/design-plans/2026-10-19-nautilus-chromium-browser.md`.

## Todo

- [ ] macOS app bundle with the framework and helper apps
- [ ] Renderer sandbox (`bootstrap.exe` on Windows, helper layout on macOS)
- [ ] Accelerated off-screen rendering (shared textures)
- [ ] A search engine for address-bar text that is not a URL
- [ ] IME composition for page text fields
- [ ] Restore previous windows on launch (the setting is stored but not acted on)
- [ ] Downloads, bookmarks, history
