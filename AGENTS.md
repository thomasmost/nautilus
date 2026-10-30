# Nautilus

A desktop web browser for macOS and Windows. Chromium (through CEF) renders
pages; the model, the egui chrome, and the winit application come from
[osmena](https://github.com/thomasmost/osmena), the shell Nautilus shares
with Paladin (Servo). This repository holds only what is Chromium's: the
engine crate and the binary that brands it.

_Last updated: 2026-10-29 (updates over the air: the release job, the
build number, and osmena's check, download and restart; before that
the retrying macOS smoke test and the scripted-run environment
variables)._

## Architecture

Cargo workspace (Rust 1.97.1 pinned in `rust-toolchain.toml`, edition 2024).

| Crate                    | What it is |
| ------------------------ | ---------- |
| `crates/nautilus-engine` | `nautilus-engine` — the only crate that imports `cef`. Implements `osmena_engine::Engine` (`engine.rs`: `run_subprocess`, `Engine`, the message-pump bridge, `INFO`) and `osmena_engine::TabHost` (`host.rs`: the tabs of one window, each a windowless browser, handing the shell `PagePaint::Frame`s, and the page commands). `delegate.rs` turns CEF handlers into `EngineEvent`s (the context menu and downloads among them), `frame.rs` composes a tab's BGRA pixels with the popup widget, `input.rs` and `keycodes.rs` turn `EngineInput` into CEF events, `devtools.rs` opens a tab's developer tools, `mac_app.rs` gives winit's `NSApplication` the protocol CEF requires. `extensions.rs` drives Chromium's extension system through a hidden `chrome://extensions` page (`crx.rs` reads store downloads), `popup.rs` opens an action's popup in an owned window, `store.rs` reroutes the Chrome Web Store's own install buttons to the manager from inside the renderer. |
| `crates/nautilus`        | `nautilus` — the binary: `brand.rs` (the `Brand`), `main.rs` (dispatches Chromium's helper processes first, then logging, clap, and `osmena_shell::run`), `helper.rs` (the `nautilus-helper` bin for macOS), `build.rs` (the Windows icon resource), and the cargo-packager metadata in its `Cargo.toml`. |
| `xtask`                  | `cargo xtask icons`, through `osmena-xtask`, and `cargo xtask bundle-mac` (`bundle_mac.rs`: the macOS app bundle). |

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

- `cargo run -- [--profile <name>] [<url>...]` — run the browser
  (Windows; on macOS Chromium runs only from the bundle, below). The
  workspace sets `default-members = ["crates/nautilus"]`, so plain `cargo
  build`/`cargo test` touch only the binary; pass `--workspace` for the rest.
  `--exit-after <seconds>` (hidden, osmena's) quits after logging the tabs.
- `cargo xtask bundle-mac` — macOS: build the release binaries and lay out
  `target/release/Nautilus.app` (framework, helper apps, property lists,
  ad-hoc signature); needs `CEF_PATH`. Run the browser from inside it:
  `target/release/Nautilus.app/Contents/MacOS/Nautilus [args]`.
- `cargo test --workspace` — unit tests (nothing starts Chromium).
- `cargo clippy --workspace --all-targets -- -D warnings` — lint.
- `cargo fmt --all` — format; CI runs `cargo fmt --all --check`.
- `cargo xtask icons` — regenerate `assets/icons/` from `assets/logo.svg`.
- Packaging (see `README.md`): `cargo build --release -p nautilus`, then
  `cargo packager --release -p nautilus` (Windows), or `cargo xtask
  bundle-mac` (macOS; CI zips the bundle, runs it once as a smoke test
  on `ci/smoke.html`, and uploads it with its dSYM). The release profile
  keeps line tables (`debug = "line-tables-only"`), so the build also
  leaves `nautilus.pdb`; CI uploads it with the installer.
- Scripted runs: `RUST_LOG=nautilus_engine=debug` logs a tab's browser
  creation, load start, loading state and title (and the extension
  manager's page); `NAUTILUS_CHROMIUM_SWITCHES=name,name=value` appends
  Chromium switches in the browser process
  (`on_before_command_line_processing`); `NAUTILUS_CEF_LOG=info|verbose`
  raises CEF's own log level from warnings; `NAUTILUS_TEST_PAGE_SCRIPT`
  runs a script in a page when its load finishes. The macOS smoke test
  in CI launches the bundle up to three times on the `ci-smoke`
  profile because a fresh profile's first launch sometimes loads
  nothing on the Intel runner (C6 in `docs/upstream-issues.md`).

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
- **macOS runs from the bundle, and the bundle's names matter.**
  Chromium derives the helper apps' paths from the main executable's
  file name (`<Name> Helper.app` beside the framework, then the
  `(Renderer)` and other variants from that), so `bundle_mac.rs` names
  the executable and the helpers after `PRODUCT` ("Nautilus") and
  `Engine::new` also names `browser_subprocess_path` from
  `current_exe()` when the helper is there (`helper_path`). The
  framework is loaded at run time (`load_framework` in `engine.rs`,
  `../Frameworks` from the browser, `../../..` from a helper).
  `cargo run` cannot start Chromium on macOS. Before `cef::initialize`,
  `mac_app::adopt_cef_app_protocol` adds `isHandlingSendEvent` /
  `setHandlingSendEvent:` and a flag-setting `sendEvent:` to winit's
  `NSApplication` subclass at run time (CEF requires the protocol; winit
  already owns the application object).
- **macOS editing shortcuts are the embedder's.** Blink leaves Cmd+Z,
  Shift+Cmd+Z, Cmd+X, Cmd+C, Cmd+V, Shift+Cmd+V, and Cmd+A to the
  browser's Edit menu on macOS (it handles Ctrl+… itself elsewhere).
  `TabKeyboardHandler::on_key_event`, which CEF calls for a key the
  renderer did not handle, carries them out on the focused frame
  (`unhandled_key` in `delegate.rs`); on other platforms it does
  nothing. The handler is declared once per platform because the
  binding's `os_event` type differs.
- **Key events follow each platform's native message.** On Windows CEF
  builds a `ui::KeyEvent` from a `CefKeyEvent` (`TranslateUiKeyEvent` in
  its platform delegate): a `CHAR` event's character is read from
  `windows_key_code` (as `WM_CHAR`'s `wParam`; `character` is ignored),
  a key-down's DOM `key` from the virtual-key code and the layout, and
  the DOM `code` from `native_key_code`, which must be the scan code
  with `0xE000` for extended keys. `input::key_events` and
  `keycodes.rs` follow that: `0x41` on the down event and `'a'` on the
  `CHAR` event for the A key. On macOS CEF reads `native_key_code`
  (the `kVK_*` code) and `character` and ignores `windows_key_code`.
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
- **Store pages never reach Chromium's extension dialogs.** "Add to
  Chrome" calls `chrome.webstorePrivate.beginInstallWithManifest3`,
  whose install prompt is modal to the tab's native window; a
  windowless browser has none and Chromium crashes on the null host
  (C4 in `docs/upstream-issues.md`). `store.rs` is the render-process
  handler: in every main-frame V8 context of a store page it binds a
  function on `window` and evaluates a shim that replaces that method,
  `completeInstall`, `install`, `requestExtension`,
  `management.uninstall`, and `management.setEnabled` with functions
  that call it and answer the page at once ("user_cancelled" for the
  install). The function sends a process message; `TabClient::
  on_process_message_received` checks the frame is the main frame of a
  store page again and hands the `StoreRequest` to the extension
  manager through `DelegateState::store_request`. Two `cef` crate
  facts this relies on: `V8Context::eval` does nothing unless both
  `retval` and `exception` are `Some(&mut None)`, and doc comments are
  not accepted inside `wrap_*!` macro invocations.
- **A string list CEF lends a handler is read through its handle.**
  `CefStringList::clone` of a lent list is empty (C5 in
  `docs/upstream-issues.md`), so `strings_of` in `delegate.rs` reads
  `cef::sys::cef_string_list_value` directly; favicons come through
  it. Never `list.clone().into_iter()` on a handler argument.
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
  Alloy-style browser in a window owned by the browser window, placed
  under the button from the `ScreenRect` the shell passes: on Windows a
  `WS_POPUP` window CEF creates, on macOS a borderless `NSWindow` of
  our own (a subclass that accepts key status, which a borderless
  window refuses) added as a child window of the browser's, with CEF's
  view as its content (`popup.rs`, `platform`). The shell's rectangle is
  physical pixels from the top left of the main display, winit's
  convention; macOS flips it to AppKit's bottom-left points. The page
  reports its size through a `nautilus-popup-size:` console message.
  The shell closes popups when its window regains focus, moves, or
  resizes; Escape and `window.close()` close them too; a reopen within
  400 ms of a close is the toggle click and is ignored.
- **Packaging metadata** is `[package.metadata.packager]` in
  `crates/nautilus/Cargo.toml`. cargo-packager resolves its paths against that
  crate's directory (hence `../../`). The `resources` globs pick up the CEF
  runtime files the build copied into `target/release/`; they assume the
  default target dir and no `--target`.
- **Every green push to `main` is a release, and the browser updates
  from it.** The `release` job tags `build-<run number>`, attaches the
  package job's artifacts and `latest.toml` (`ci/update-manifest.sh`,
  osmena's `update::Manifest`), and the package job builds with
  `OSMENA_BUILD_NUMBER` set to the same run number, which `brand.rs`
  reads with `option_env!` into `Brand::build`. The check, download and
  restart are osmena's (`osmena_shell::update`); this repository owns
  only the brand's manifest URL
  (`releases/latest/download/latest.toml`) and the release job. The
  installer's silent mode (`/S`) kills every `nautilus.exe`, so the
  restart closes other profiles' windows too. Keep the asset names the
  release job lists in step with the package job's uploads.
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

## Status and handoff (2026-10-29)

Added 2026-10-29: updates over the air (see Conventions and osmena's
handoff). Verified on Windows with the debug build and a local manifest
server: the check, the download and verification into
`%APPDATA%\Nautilus\updates\<build>\`, the pill, and the click
quitting the browser, running a stand-in installer with `/S` after the
exit and relaunching the same profile. Not verified: the real
`nautilus_<version>_x64-setup.exe` in silent mode over the installed
copy (try it on the first release after this one: the pill should
appear within a minute of starting the installed build, and the
restart should come back on the new build number, shown in Settings),
the release job before its first run, and macOS.

## Status and handoff (2026-10-28)

The `package macos-15-intel` job had failed its smoke test twice on
cba28c0 (the page's title never arrived in 30 s). A branch run with
more logging (`ci/intel-smoke-diagnosis`, deleted after) showed the
tab's navigation starting and never committing on a fresh profile's
first launch, the hidden extensions page likewise, on the previous
green commit as much as on cba28c0, with `--disable-gpu` as well, and
never on a second launch of the same profile: an intermittent
first-launch stall of the Intel runner, not a regression (C6 in
`docs/upstream-issues.md`). The smoke step now retries a clean launch
without the title up to three times on the same profile and warns
when a later attempt is the one that passes. Kept from the diagnosis:
the debug logs around a tab's life, `NAUTILUS_CHROMIUM_SWITCHES` and
`NAUTILUS_CEF_LOG` (Commands). Not verified: the stall on a real Intel
Mac (none available); whether the retry's warning ever fires on Apple
silicon (it has not stalled there).

Also 2026-10-28, osmena's: the profile dialog's avatars in two
palettes, "Sea creatures" and "Assorted" (osmena `8a5e79d`). Verified
on Windows by scripted input: both palettes drawn in six columns with
Save and Cancel in view, a pick saved as `avatar = "turtle"` and shown
in the toolbar.

## Status and handoff (2026-10-26)

Added 2026-10-26, all of it osmena's (no engine change): profiles with a
name, colour and avatar, the avatar button at the toolbar's end whose
menu is the Profiles menu, the profile dialog (Edit…, Add Profile…),
Delete Profile, shared bookmarks and copied extensions. Copying
extensions reads the default profile's `extensions.toml` (written by
the shell on every `ExtensionsChanged`) and calls
`Engine::install_extension(Store { id })` for each missing id, so an
unpacked extension in the default profile makes the store install fail
and the status area says so. Verified on Windows by scripted input on
the debug build (profile `profile-probe`, deletable): the menu, the
Edit dialog with its live preview, Save (plum chrome, whale avatar,
`[profile]` in `settings.toml`), Add Profile (a second Nautilus starts
for `work-stuff`), the Delete Profile submenu, its confirmation, and the deletion itself (a stale `running.pid` left by a killed browser does not block it).
Not verified by hand: deleting the current profile (it starts the
default profile, quits, and removes its directory after
`cef::shutdown`), the "open in another window" refusal, and anything
on macOS.

## Status and handoff (2026-10-21)

What exists: Nautilus was started on 2026-10-19 as Paladin's feature set
on Chromium (CEF 154), and the same day its model and chrome moved to
osmena; this repository is the CEF engine crate and the binary. Public at
<https://github.com/thomasmost/nautilus>.

Added 2026-10-20 (evening), from a code review of the macOS side against
the Windows build: the macOS app bundle (`cargo xtask bundle-mac`, the
CI package job for macOS with its smoke test and artifact), the
`CefAppProtocol` adoption, the explicit helper path, and the Cmd editing
shortcuts in pages. None of it has run on a Mac by hand; CI's smoke test
is the only macOS run so far (check the latest `package macos-15` job
before QA: a green smoke step means the bundle starts, loads a page in
a renderer process from a helper app, and exits cleanly). What the
review could not settle without a Mac, in the order to try: (1) the
bundle opens from Finder after `xattr -dr com.apple.quarantine`; (2) a
page renders and scrolls (the GL 3.3 core context through CGL, the
frame upload); (3) right-click on a tab, on the page, and the ☰ menu:
muda's `popUpMenuPositioningItem` runs a nested run loop while winit's
handler is on the stack (winit queues events that arrive meanwhile, so
no re-entrancy is expected, but the engine's wake-ups land in that
queue too); (4) typing in a page field, then Cmd+C/V/X/A/Z there;
(5) Cmd-shortcuts of the keymap (Cmd+T, Cmd+W, Cmd+Q) and that none of
them leak into the page; (6) the frameless window: traffic lights over
the strip, dragging the strip, double-click; (7) "Inspect" (DevTools
opens in a window Chromium creates itself from a `WindowInfo` with no
parent view), "Print…" and "Save link as…" (system dialogs over an
off-screen browser); (8) Ctrl-click, Cmd-click, and Shift-click on
links; (9) a second window and tab drags; (10) Cmd+Q and the Dock's
Quit both exit with no `Nautilus Helper` process left. Known gaps on
macOS, not bugs: no `.dmg`, ad-hoc signature only, `cargo run` cannot
start Chromium. The menu bar arrived with osmena on 2026-10-23 and has
not been tried by hand either: check that Cmd+C/V/X/A/Z still reach
page fields (the bar now intercepts them and sends `PageCommand`s) and
the address bar, that Cmd+Q quits through the menu, and that Hide and
Minimize work.

Added 2026-10-22, from a review of click capture and the shell/page
boundary (osmena 0c4d71d): the user's "a link sometimes opens a
download" report is explained by the host stamping mouse events with
the modifiers of the last key event it saw; Windows sends no Alt
release when Alt+Tab takes the focus (nor does a tab closed by Ctrl+W
get its Ctrl release), so the next click on a link was an Alt+click,
which Chromium treats as "save link as". The host keeps no key
modifiers now; every pointer event brings its own from the shell, and
the shell releases held keys and buttons in the page on focus loss. Also
this day: `on_paint` copies only CEF's dirty rectangles into the frame
at an unchanged size and reports them (`Frame::dirty_rects`,
`FrameRef::dirty`) so the shell uploads only those; wheel fractions
carried between events (`input::wheel_steps`). Verified: `cargo test
--workspace`, clippy, formatting. Not verified by hand: any of it in a
running browser. The first thing to try: hold Alt, Alt+Tab away and
back with the mouse, click a link; it should open in place and the
download log line should not appear.

Added 2026-10-21 (later): favicons (never shown: the `cef` crate's
string-list clone, C5; fixed with `strings_of`), a log line naming
every download with its MIME type and `Content-Disposition` (the
user reports a link in Gmail sometimes downloading instead of
opening; a scripted walk of ups.com links, a Google redirect, and a
`target=_blank` link found no download, so the next report should
come with that line), and a clipboard check: a page's
`navigator.clipboard.writeText` and `execCommand("copy")` both
succeed on a click in a windowless tab (the user reported copy
buttons failing; not reproduced, site unknown).

Added 2026-10-21 (evening): the Intel macOS build, as a third entry in
both CI matrices (`macos-15-intel`); the zips carry the architecture.
Nothing in the code is architecture-specific: the cef crate fetches
the CEF distribution for the build's target and the bundling xtask
names the framework directory by `target_arch`. Not verified by hand
(no Intel Mac; the Intel bundle runs under Rosetta on Apple silicon).
GitHub retires the Intel runners with the macOS 15 image, expected in
fall 2027: decide in 2027 whether to keep the build (README,
Packaging).

Added 2026-10-21, from the user's reports on the installed Windows
build: (a) "Add to Chrome" on the Proton Pass store page crashed the
browser every time; a debugger attached to the live process plus CEF's
release symbols (`cef_binary_<version>_windows64_release_symbols`,
900 MB, the `.pdb` must sit at a short path) gave the stack through
`WebstorePrivateBeginInstallWithManifest3Function` into the null modal
dialog host; fixed by the store shim (see Conventions, C4). Verified on
Windows by evaluating the store's API call in the page under cdb: the
shim answered "user_cancelled", the manager downloaded and loaded
Proton Pass, its onboarding tab opened, no crash. Not verified by hand:
the real button click (the store showed "Switch to Chrome" to the
scripted fresh profile but "Add to Chrome" to the user's), "Remove
from Chrome" and the enable switch on a store page, macOS. (b) Typing
into page fields produced capitals and wrong punctuation: CEF on
Windows types `windows_key_code` of a `CHAR` event (see Conventions);
fixed, and `code` was empty on every key event (native key code 0),
fixed with a scan-code table. Verified with a logging test page.

Added 2026-10-20 (night), the first run on a Mac (Apple silicon,
macOS 15): the bundle built from this checkout starts, renders
example.com, shows the puzzle piece, and exits cleanly with
`--exit-after` and no helper left; the extension manager's hidden Views
window initialises, lists, and is destroyed at shutdown on macOS as on
Windows. Two parity gaps found against the Windows build and fixed:
the tab, page, and ☰ menus opened at the window's bottom edge (osmena's
`native_menu.rs`: muda mirrors y for an unflipped NSView, winit's is
flipped; y is mirrored first now), and extensions were off (`popup.rs`
now has a macOS window; `INFO.extensions` and the manager are gated on
Windows or macOS). Not verified by hand on macOS: the menus' new
position and the popup window itself (clicking the puzzle piece and a
pinned action, the popup taking the keyboard, Escape and focus closing
it, its size following the page); scripted input and screenshots from
the terminal were blocked by Accessibility and Screen Recording
permissions. Those are the first things to try.

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

CI: both check jobs pass; the package job builds the Windows installer
and the macOS bundle. The `cef` crate's generated handler signatures
differ by platform (`CursorHandle` on Windows, bare pointers on macOS;
the keyboard handler's `os_event` likewise), so a handler that compiles
on Windows can fail on the macOS job: `delegate.rs` keeps the
platform-typed aliases and per-platform declarations for those.

Open items, most useful first:
1. The macOS manual run (above), then a `.dmg` and a Developer ID
   signature.
2. The renderer sandbox through CEF's `bootstrap.exe` (C1).
3. Accelerated off-screen rendering (C2).
4. The manual checklist items above that were never exercised, and the
   extensions item (12) in full.
5. Extensions on macOS by hand: the popup window (above), then the
   checklist item (12) in full.
6. A freshly created profile can take more than 12 s before the first page
   paints; nothing is wrong, but screenshots taken earlier show a white page.

All three checkouts live side by side under `vibes/browser-project/`
(`osmena`, `paladin`, `nautilus`); Paladin's Servo and stylo fork checkouts
stay at `~/vibes/servo` and `~/vibes/stylo`. `~/.cargo/config.toml` on the
development Windows box and on the Mac carries a `[patch."https://github.com/thomasmost/osmena"]`
table pointing at the osmena checkout, so both browsers build against
uncommitted osmena changes; while it is active `cargo` rewrites the
browsers' `Cargo.lock` with path sources for the osmena crates. Before
committing a browser's lock, disable the table (rename its key), run
`cargo metadata` to put the git sources back, commit, and re-enable it.
Cargo fetches the pinned revision from GitHub even when the table is
active, so GitHub must be reachable.
