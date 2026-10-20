# Nautilus

A desktop web browser for macOS and Windows. Chromium (through CEF) renders
pages; the model, the egui chrome, and the winit application come from
[osmena](https://github.com/thomasmost/osmena), the shell Nautilus shares
with Paladin (Servo). This repository holds only what is Chromium's: the
engine crate and the binary that brands it.

_Last updated: 2026-10-19 (the page context menu, downloads, developer tools)._

## Architecture

Cargo workspace (Rust 1.97.1 pinned in `rust-toolchain.toml`, edition 2024).

| Crate                    | What it is |
| ------------------------ | ---------- |
| `crates/nautilus-engine` | `nautilus-engine` — the only crate that imports `cef`. Implements `osmena_engine::Engine` (`engine.rs`: `run_subprocess`, `Engine`, the message-pump bridge, `INFO`) and `osmena_engine::TabHost` (`host.rs`: the tabs of one window, each a windowless browser, handing the shell `PagePaint::Frame`s, and the page commands). `delegate.rs` turns CEF handlers into `EngineEvent`s (the context menu and downloads among them), `frame.rs` composes a tab's BGRA pixels with the popup widget, `input.rs` and `keycodes.rs` turn `EngineInput` into CEF events, `devtools.rs` opens a tab's developer tools. `extensions.rs` drives Chromium's extension system through a hidden `chrome://extensions` page (`crx.rs` reads store downloads), `popup.rs` opens an action's popup in an owned window. |
| `crates/nautilus`        | `nautilus` — the binary: `brand.rs` (the `Brand`), `main.rs` (dispatches Chromium's helper processes first, then logging, clap, and `osmena_shell::run`), `helper.rs` (the `nautilus-helper` bin for macOS), `build.rs` (the Windows icon resource), and the cargo-packager metadata in its `Cargo.toml`. |
| `xtask`                  | `cargo xtask icons`, through `osmena-xtask`. |

The osmena crates (`osmena-core`, `osmena-engine`, `osmena-shell`,
`osmena-xtask`) are git dependencies pinned by revision in the workspace
`Cargo.toml`; they version together, so move every `rev` at once. Their
docs describe the model, the chrome, the commands and events, and the
conventions that apply to all of it; this file covers Chromium's side.

Data flow: osmena's chrome and keymap emit `Command`s; `dispatch` updates
the model and returns `EngineAction`s, which the shell forwards to this
crate's `TabHost`. CEF handler callbacks become `EngineEvent`s that `apply`
folds into the model. Core ids never meet CEF objects outside
`nautilus-engine`.

Rendering: each tab is a windowless CEF browser. CEF calls the tab's
`RenderHandler::on_paint` with BGRA pixels, which land in the tab's `Frame`
and raise `EngineEvent::FrameReady`; `TabHost::paint_page` hands the frame
to the shell as a `PagePaint::Frame`, and the shell uploads it to a texture
under the toolbar. `<select>` popups arrive as a second paint element and
are composed into the frame. The chrome's GL context is osmena's glutin
surface (the `glutin` feature of `osmena-engine`).

## Commands

Run from the repo root, with `CEF_PATH` set (see `README.md`) and CMake and
Ninja on `PATH`:

- `cargo run -- [--profile <name>] [<url>...]` — run the browser. The
  workspace sets `default-members = ["crates/nautilus"]`, so plain `cargo
  build`/`cargo test` touch only the binary; pass `--workspace` for the rest.
- `cargo test --workspace` — unit tests (nothing starts Chromium).
- `cargo clippy --workspace --all-targets -- -D warnings` — lint.
- `cargo fmt --all` — format; CI runs `cargo fmt --all --check`.
- `cargo xtask icons` — regenerate `assets/icons/` from `assets/logo.svg`.
- Packaging (see `README.md`): `cargo build --release -p nautilus`, then
  `cargo packager --release -p nautilus` (Windows; macOS bundling is not
  done yet). The release profile keeps line tables
  (`debug = "line-tables-only"`), so the build also leaves `nautilus.pdb`;
  CI uploads it with the installer.

## Conventions

- **CEF stays in `nautilus-engine`.** The binary never names a CEF type;
  the shell sees only osmena's types.
- **Shared behaviour changes in osmena.** A change to the model, the
  chrome, routing, menus, or the settings window is an osmena change:
  patch the osmena checkout in `~/.cargo/config.toml` (osmena's README),
  make it there with its tests, push, and bump the `rev`s here. Only what
  Chromium needs lives in this repository.
- **Engine facts live in `INFO`.** `nautilus_engine::INFO` names the
  engine, its Chromium major (bump it with the `cef` pin), its data
  directory (`chromium/`), and its wheel distance; product strings live in
  `brand.rs`. Nothing else in this repository embeds a name.
- **`run_subprocess` first.** Chromium starts its renderer, GPU, and
  utility processes by running the executable again with `--type=...`.
  `main` calls `nautilus_engine::run_subprocess()` before anything else
  (before clap, logging, or windows) and exits with its code when it
  returns `Some`. On macOS the helper processes run `nautilus-helper`
  instead, from the app bundle.
- **One `Engine` per process, after `run_subprocess`.** CEF is
  process-global. `Engine::new` calls `cef::initialize` with an external
  message pump: CEF asks for work through
  `on_schedule_message_pump_work`, the engine wakes the winit event loop
  (osmena's `Waker`), and the shell calls `Engine::spin`
  (`do_message_loop_work`). Every CEF callback arrives on the main thread
  from inside `spin`, so the handlers use `Rc`/`RefCell`.
- **Browsers close on the message loop.** `TabHost::close_tab` asks the
  browser to close; `on_before_close` arrives later. `Engine::drop` pumps
  until the live-browser count is zero (bounded) and only then calls
  `cef::shutdown`. The shell drops windows (tab hosts) before the engine.
- **The CEF binary distribution comes from `CEF_PATH`** (or is downloaded
  into `OUT_DIR` by `cef-dll-sys`). Bumping CEF means bumping the `cef`
  crate pin in the workspace `Cargo.toml` and `INFO.version`, re-running
  `export-cef-dir` for the matching version, and re-checking the handler
  signatures (they are generated from CEF's headers and change between
  releases).
- **Page-opened tabs are URLs.** `on_before_popup` and `on_open_urlfrom_tab`
  cancel Chromium's popup and report `EngineEvent::NewTabRequested` with a
  `PendingTab` that holds the URL (`background` for the
  `NEW_BACKGROUND_TAB` disposition of a middle- or Ctrl-click), or
  `NewWindowRequested` for `NEW_WINDOW` (Shift-click); `AdoptTab`
  creates a fresh browser at it. The opener gets no `window` handle.
- **Input to CEF is logical pixels.** `EngineInput` carries physical pixels
  relative to the page area; `TabHost::input` divides by the scale factor
  and tracks click counts and held buttons, which CEF expects the embedder
  to supply.
- **The page context menu is osmena's; CEF's is declined.**
  `TabContextMenuHandler::run_context_menu` cancels CEF's callback at
  once, keeps the frame and point in `ViewState::context_menu`, and
  reports a `PageContext` read off `CefContextMenuParams` (link, media
  and its state flags, selection, editable flags, misspelling). The
  shell shows osmena's menu and sends `PageCommand`s to
  `TabHost::page_command`: the editing ones are `CefFrame` methods
  (`undo` … `paste_and_match_style` … `select_all`), spelling goes
  through the host (`replace_misspelling`, `add_word_to_dictionary`),
  "Save" is `start_download`, "Copy image" is `download_image` with a
  callback that reports `EngineEvent::ClipboardImage`, the media
  toggles run a script at the point (`elementFromPoint`; CEF exposes
  no media actions), "Print" is `print`, "View page source" is a new
  tab at `view-source:` (CEF's own `view_source` opens a text editor),
  and "Inspect" is `show_dev_tools` in a top-level window of its own
  (`devtools.rs`), closed with the tab. `ContextMenuClosed` drops the
  point.
- **Downloads ask where to save.** `TabDownloadHandler` allows every
  download, answers `on_before_download` with CEF's Save As dialog
  (owned by the tab's window), and reports progress and the outcome as
  `StatusChanged` text. Without a download handler an Alloy browser
  silently drops downloads.
- **Extensions go through `chrome://extensions`.** CEF has no extension
  API; the Chrome bootstrap's `chrome.developerPrivate` has everything.
  `ExtensionManager` keeps one Chrome-style browser on that page in a
  Views window that is never shown (a Chrome-style browser given a
  native parent opens a Chromium window of its own, and windowless
  browsers may not load that page) and runs scripts in it through the
  DevTools protocol (`Runtime.evaluate` with `awaitPromise`; each script
  is a promise of a plain object, and the result arrives in
  `on_dev_tools_method_result`). `CefFrame::ExecuteJavaScript` is not
  used there: on that browser it silently drops scripts after the first
  seconds. The page's change listener still reports through a console
  message. Loading an unpacked directory goes through `loadUnpacked`,
  whose folder dialog CEF routes to whichever browser it attributes it
  to, so every client the engine creates (tabs, the manager, popups,
  Chromium's own browsers) carries the same `DelegateState::dialog_handler`,
  which answers from one pending slot; with no answer pending CEF shows
  its own dialog. "Load unpacked…" asks with `RunFileDialog` on the
  focused tab's browser, whose window is visible (a dialog owned by the
  hidden window would be invisible and modal). A store install downloads
  the CRX with a CEF URL request, unpacks it on a worker thread (seconds
  spent inside a CEF callback stop the external message pump from
  asking for more work) under `chromium/extensions/<id>/`, writes the
  publisher's `key` into the manifest (the proof whose hash is the signed
  id; the store's own signing key is there too) so the id stays the
  store's, and loads it unpacked. Chromium keeps unpacked extensions
  across launches itself, in prefs it writes a few seconds later: a
  forced kill right after an install loses it, a normal exit does not.
- **A `cef` binding callback that returns an int defaults to 0.** The
  `wrap_*!` macros install every callback with the trait's default, which
  for an int return is `0`: `CefWindowDelegate::CanClose` then says "no"
  (CEF's own default is "yes"), so a window never closes and CEF's
  shutdown crashes with it alive. Implement such callbacks explicitly
  (`ManagerWindowDelegate::can_close` returns 1); `Engine::drop` waits for
  the manager's window to be destroyed as well as for every browser.
- **Browsers Chromium opens on its own get the default client.** An
  extension's `chrome.tabs.create`, or a link from the management page,
  makes Chromium create a browser Nautilus cannot show;
  `BrowserProcessHandler::default_client` gives it `StrayClient`, which
  turns its first URL into a tab of the last focused window
  (`DelegateState::request_tab_without_opener`) and closes it. Without a
  default client such browsers would be unmanaged and block shutdown.
- **Chromium's session is dropped at startup.** The management browser
  is a Chrome window to Chromium's session service, which restores it as
  a visible window after an unclean exit; `Engine::new` deletes
  `Default/Sessions` before `cef::initialize`.
- **DirectComposition is off.** With it on, the GPU process crashes
  three times and Chromium falls back to software rendering whenever a
  windowed Chrome-style browser exists beside the windowless tabs.
- **Action popups are owned windows.** `Popups` opens a windowed
  Alloy-style browser in a `WS_POPUP` window owned by the browser
  window, placed under the button from the `ScreenRect` the shell
  passes; the page reports its size through a `nautilus-popup-size:`
  console message. The shell closes popups when its window regains
  focus, moves, or resizes; Escape and `window.close()` close them too;
  a reopen within 400 ms of a close is the toggle click and is ignored.
- **Packaging metadata** is `[package.metadata.packager]` in
  `crates/nautilus/Cargo.toml`. cargo-packager resolves its paths against that
  crate's directory (hence `../../`). The `resources` globs pick up the CEF
  runtime files the build copied into `target/release/`; they assume the
  default target dir and no `--target`.
- **Generated icons are committed.** Edit `assets/logo.svg` (pure vector,
  no `<text>`, no `<image>`: osmena's xtask builds resvg without font or
  raster support), run `cargo xtask icons`, commit the result. Never
  hand-edit `assets/icons/`.
- **Format before every commit:** `cargo fmt --all`. CI fails on formatting
  or clippy warnings.
- **`AGENTS.md` mirrors this file.** Keep them identical.

## Definition of Done

A task is complete only when:

1. **All tests pass** — `cargo test --workspace` is green, and the manual
   checklist in `README.md` passes for any change to the engine glue or
   to the osmena revision.
2. **All checks pass** — `cargo fmt --all --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`, and
   `cargo build --workspace` are clean, and `cargo xtask icons` leaves
   `assets/icons/` unchanged.
3. **Docs are current** — `CHANGELOG.md` has an entry under `Unreleased` for
   any user-visible change, and this file's freshness stamp is updated when
   the architecture or commands change.

## Workflow

- Don't create a new branch unless specifically asked. Commit to the current branch.
- Commit messages follow Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, `ci:`).
- Design plans live in `docs/design-plans/`; the phase roadmap is `ROADMAP.md`.
- `docs/upstream-issues.md` tracks the CEF, egui, winit, and toolchain
  issues Nautilus depends on and the features each one blocks. Add an entry
  when a bug or feature turns out to be upstream; re-check every `C` entry
  on a CEF bump.

## Status and handoff (2026-10-19)

What exists: Nautilus was started today as Paladin's feature set on
Chromium (CEF 154), and the same day its model and chrome moved to osmena;
this repository is the CEF engine crate and the binary. Public at
<https://github.com/thomasmost/nautilus>.

Added 2026-10-20: the page context menu (osmena's, over CEF's
`ContextMenuParams`), downloads with the Save As dialog, developer tools
on "Inspect", background tabs for middle- and Ctrl-click, new windows for
Shift-click, and the address-bar search from osmena. Verified on Windows
by scripted runs against a local test page (`drive.ps1` in the session
scratchpad): every menu variant renders with its shortcuts; "Open link
in new tab" opens the tab beside the current one and leaves it active;
"Copy link address" pastes into the address bar; "Copy image" leaves a
160×100 image on the clipboard; clicking a download link and "Save link
as…" open the Save As dialog, and a download that fails (a 404) reports
"Could not save …" in the status overlay; "Print…" opens the system
print dialog; "View page source" opens a `view-source:` tab; "Inspect"
opens DevTools in its own window on the element; a search typed in the
address bar loads Google (which answered with its reCAPTCHA page for
the fresh profile); Ctrl+Q with DevTools open exits with no process
left. Not verified: the media entries (Loop, Show controls, save and
copy of a video), spelling suggestions, "Paste as plain text" and the
other editing entries by hand, Shift-click and middle-click on links, a
download that completes, DevTools closing when its tab closes, macOS.

Added the same day: Chrome extensions (see Conventions). Verified on
Windows: a content script runs in a windowless tab; an extension loaded
unpacked through the hidden management page is listed, persists across
launches, and its pinned icon appears in the toolbar; clicking the icon
opens the popup under it, sized to the page, with `chrome.storage` and
`chrome.tabs` available in it, Escape closes it, clicking the icon again
closes it, clicking the page closes it; a store install (uBlock Origin
Lite) downloads, unpacks with the `key` that keeps the store id, loads at
once, and is still there after a close within seconds of the install
and a relaunch; no Chromium window is visible, before or after a forced
kill; the settings window lists the extension with its switches. Not
verified by hand: the puzzle menu's entries (unit-tested as data), the
Extensions section's switches and buttons (enable, remove, "Load
unpacked…" and its dialog, install by id), a tab Chromium opens on its
own (`StrayClient`), and anything on macOS, where `INFO.extensions` is
`None`.

Verified on Windows earlier: `cargo test --workspace`, clippy, formatting,
icons unchanged; launch renders example.com with the Nautilus title and
icon; before the osmena move, also Ctrl+T with a typed URL, Ctrl-click
selection with the native tab menu, wheel scrolling, and a clean exit
with no leftover processes. Not verified: typing into page fields, link
hover status, Alt+Left, tab drags between windows, open-in-profile, the
crashed-tab panel (`chrome://crash`), anything on macOS.

CI: the Windows job passed. The first macOS job failed in `nautilus-engine`:
`cef::CursorHandle` is a Windows-only name in the `cef` crate; the fix in this
repository's history uses the platform typedef instead and is unverified on
macOS (no Mac was available). macOS also needs the app bundle before it can
run at all (C3 in `docs/upstream-issues.md`).

Open items, most useful first:
1. macOS app bundle with the framework and the five helper apps (C3, T1).
2. The renderer sandbox through CEF's `bootstrap.exe` (C1).
3. Accelerated off-screen rendering (C2).
4. The manual checklist items above that were never exercised, and the
   extensions item (12) in full.
5. Extensions on macOS: the management window is Views (portable), the
   popups are Win32 (`popup.rs`); `INFO.extensions` is gated on
   `cfg!(windows)`.
6. A freshly created profile can take more than 12 s before the first page
   paints; nothing is wrong, but screenshots taken earlier show a white page.

All three checkouts live side by side under `vibes/browser-project/`
(`osmena`, `paladin`, `nautilus`); Paladin's Servo and stylo fork checkouts
stay at `~/vibes/servo` and `~/vibes/stylo`. `~/.cargo/config.toml` on the
development Windows box carries a `[patch."https://github.com/thomasmost/osmena"]`
table pointing at the osmena checkout, so both browsers build against
uncommitted osmena changes; while it is active `cargo` rewrites the
browsers' `Cargo.lock` with path sources for the osmena crates. Before
committing a browser's lock, disable the table (rename its key), run
`cargo metadata` to put the git sources back, commit, and re-enable it.
Cargo fetches the pinned revision from GitHub even when the table is
active, so GitHub must be reachable.
