# Nautilus

A desktop web browser for macOS and Windows. Chromium (through CEF) renders
pages; the model, the egui chrome, and the winit application come from
[osmena](https://github.com/thomasmost/osmena), the shell Nautilus shares
with Paladin (Servo). This repository holds only what is Chromium's: the
engine crate and the binary that brands it.

_Last updated: 2026-11-27 (extension side panels, tab groups as folders and extension shortcuts: a side panel is one more view of the tab host (`panel.rs`, `TabHost::create_view`), `chrome.sidePanel`, `chrome.tabGroups` and `chrome.commands` are answered by the bridge, every replaced namespace is pinned on `chrome` and on `browser` (`pin` in `js/core.js`, C19), a debugger attached from an extension's page is reported, `ci/side-panel/panel.sh` is the scripted run, and the Claude extension's panel opened in a real run, see the status section of 2026-11-27, the extension side panel; before that, 2026-11-24, Chromium's own PDF viewer is not bridged: `pages::CHROMIUM_OWN`, see the status section of 2026-11-24, the PDF viewer; before that, 2026-11-23, Chromium 156: the `cef` pin is 156.7.0, see the status section of 2026-11-23, Chromium 156; before that, the same day, three Chromium features are off after Thorium's defaults, and a feature list is added to, never replaced: `DISABLED_FEATURES` and `add_to_list_switch` in `engine.rs`, C18, see the status section of 2026-11-23, the performance review; before that, the same day, the default profile is pinned to `Default/`: `--profile-directory=Default`, or Chromium hands the global context the profile used last, see the status section of 2026-11-23, the shared cookies, and C17; before that, the same day, a tab shown again is given a new surface: every way of showing a view goes through `delegate::show_view`, and the bridge's hidden pages are told they are hidden for as long as they live, see the status section of 2026-11-23, the frozen tabs, and C16; before that, the same day, a `<select>` opens with a second window open and an extension installed, see the status section of 2026-11-23, `<select>` with two windows and an extension, whose pausing of the bridge's pages the later change replaced; before that, 2026-11-22, a `<select>` opens with more than one window open: `popup_pause.rs` tells the other tab views they are hidden while a popup widget is open, see the status section of 2026-11-22 and C15; before that, 2026-11-19, every profile in one process: one `Runtime` (CEF) per process and an `Engine` per open profile, each profile but the default one a request context under the default profile's `chromium/`, see the status section of 2026-11-19, one process; before that, the relay and the tab probe choose their own DevTools message ids, see the status section of 2026-11-19, the message ids; before that, the scripted runs' hooks are behind the `test-hooks` cargo feature and in no packaged build, see the status section of 2026-11-19, the test hooks; before that, a worker's tab cache follows the tabs the engine opens, renames and closes, `js/runtime.js` with `js/core.js` `tap`, so `sender.tab` carries the strip's fields from an extension page's first message; see the status section of 2026-11-19, the sender's strip; before that, review 1 of Phase 5: extensions without a worker are served through their pages, listeners are kept per context, the bridge's ports are taken by address, the copy replacement and the migration are ordered, the capture reads only web and extension pages, page calls and screenshots are bounded; see the status section of 2026-11-19, review 1 of Phase 5; before that, an extension page reports its own Chromium tab id to the engine, `bridge/js/page_report.js` and `bridge/table.rs` `report`, so its row exists and `sender.tab` carries the strip's fields; a worker's call that finds no listener is sent again, `bridge/js/bridge.js` `resend`, so the restored launch's first call is answered; an extension page's namespaces are installed at its first document change, `bridge/js/page_defer.js`, because Chromium defines `chrome` in the page after its context is created; the browser no longer traps on the page; a page's calls are held for their tab and for their extension (`BridgeState::page_call`), so the scripted run passes 66 of its checks, see the status section of 2026-11-18 (page-call holds); before that, Phase 5, Task 13: the scripted run `ci/side-panel/phase5.py` is a command; its two passes stop at an extension page opened as a tab (no bridge in it, and a SIGTRAP in the browser process); the regression modes pass on the Phase 5 build, and a delivered event starts a stopped listening worker; before that, Phase 5, Task 12: extension pages are bridged on their own page lane (`bridge/pages.rs`), not the relay page or the helper's pages; the page's port is not hidden and its `chrome.debugger` calls are not reported; before that, review cycle 6 of the side panel's Phase 1: a worker-target change asks its relay which worker it reaches, and the answer decides (`policy::asks_on`, `policy::after_ask`); the two-refresh grace is gone; `identify` rejects in a zombie page; before that, review cycle 5 of the side panel's Phase 1: a worker-target change waits for a hello received after it (`policy::WorkerWatch`), the "hidden again" line on every forget, the hello's whole chain bounded at 2 s; before that, review cycle 4 of the side panel's Phase 1: the debugger rule (uncertainty never forgets), the three-state hello, the worker-target detector with its grace, the list hold and `relay_eval` controls, the zombie, idle and stop provocations; before that, review cycle 3 of the side panel's Phase 1: a worker's identity and debugger sessions, a hidden page given up after five openings, the harness's reload, stop and broken provocations; before that, review cycle 2 of the side panel's Phase 1: the harness judges each launch, relays reopen on the engine's turn, page loss and debugger reports by targetId, the Windows control-directory rule, the test extension probes onConnect; before that, 2026-11-17 the extension bridge's spike (a loader ahead of an extension's worker, a relay page, the tab-id table) and the message pump's fallback tick; before that a page's request for the microphone or the
camera is answered through osmena's dialog; popups are browsers
Chromium creates, so `window.open` keeps its opener; before that blocked sites over Chromium's before-browse
hook (osmena's list and block page); before that osmena's restored tabs warmed one by one
for their titles and favicons; before that drags carried out within
the page; before that macOS typing fixes: keys without text carry AppKit's characters,
unhandled keys stop at the tab; the macOS icon on Apple's grid; the
bundle claims web links for osmena's default-browser offer; osmena's
session restore, downloads, page zoom, find in page, bookmarks,
updates over the air)._

## Architecture

Cargo workspace (Rust 1.97.1 pinned in `rust-toolchain.toml`, edition 2024).

| Crate                    | What it is |
| ------------------------ | ---------- |
| `crates/nautilus-engine` | `nautilus-engine` — the only crate that imports `cef`. Implements `osmena_engine::Engine` (`engine.rs`: `run_subprocess`, `Engine`, the message-pump bridge, `INFO`) and `osmena_engine::TabHost` (`host.rs`: the tabs of one window, each a windowless browser, handing the shell `PagePaint::Frame`s, and the page commands). `delegate.rs` turns CEF handlers into `EngineEvent`s (the context menu and downloads among them), `frame.rs` composes a tab's BGRA pixels with the popup widget, `input.rs` and `keycodes.rs` turn `EngineInput` into CEF events, `devtools.rs` opens a tab's developer tools, `mac_app.rs` gives winit's `NSApplication` the protocol CEF requires. `extensions.rs` drives Chromium's extension system through a hidden `chrome://extensions` page (`crx.rs` reads store downloads), `popup.rs` opens an action's popup in an owned window, `store.rs` reroutes the Chrome Web Store's own install buttons to the manager from inside the renderer. `panel.rs` is a side panel's pure half (its page's address, a size per view, what a panel's view reports, the registry of open panels) and `commands.rs` an extension's keyboard commands (the manifest's shortcut as a chord, `commands.getAll`'s text). `bridge/` puts a script ahead of an extension's service worker (`rewrite.rs`, `install.rs`), reaches it through a relay page in a hidden browser (`relay.rs`), and joins tab ids with a helper extension's DevTools targets (`table.rs`); it answers `chrome.tabs`, `windows`, `tabGroups` and `commands` from the strip the shell publishes (`hub.rs`, `state.rs`, `router.rs`, `differ.rs`, `groups.rs`) and `chrome.sidePanel` from the engine's own stores (`side_panel.rs`), in workers and in extension pages (`pages.rs`), with the scripts under `bridge/js/`; `docs/bridge-protocol.md` has the messages. |
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
are composed into the frame; while one is open every other tab's view
is paused (`popup_pause.rs`, C15). A view told it is hidden is shown
again only through `delegate::show_view` (C16). The chrome's GL
context is osmena's glutin
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
  (`on_before_command_line_processing`; a list of features, as in
  `disable-features=A;B`, separates its names with `;` and is added
  to what CEF set, C18); `NAUTILUS_CEF_LOG=info|verbose`
  raises CEF's own log level from warnings; `NAUTILUS_TEST_PAGE_SCRIPT`
  runs a script in a page when its load finishes. The macOS smoke test
  in CI launches the bundle up to three times on the `ci-smoke`
  profile because a fresh profile's first launch sometimes loads
  nothing on the Intel runner (C6 in `docs/upstream-issues.md`). Every scripted launch passes osmena's hidden `--data-dir <dir>` (its own profiles, lock and Chromium root): without it the launch is handed to the Nautilus the user has open, or starts their default profile's Chromium under the test; `NAUTILUS_TEST_CONTROL=<dir>` makes the engine carry out `cmd-N.json` commands from that directory (`load` an unpacked extension through the bridge's rewrite, `click` an extension's action for the tab focused last, decided as the toolbar's button decides it (`ExtensionManager::click`) and reported in the command's log line as `sent to the bridge`, `not sent`, `opened or closed its side panel` or `the action has a popup, which the control does not open`, `extension_command` (`{"extension_command": {"extension": "<id>", "name": "<command>"}}`: that extension's shortcut as if pressed for the tab focused last, `_execute_action` as the click, any other name to its `commands.onCommand` listeners, logged as `handed to the bridge`), a raw `devtools` message through the management browser with its answers in `devtools.log`, `table`, `hold_list` (the extension list's reads are held for N seconds, then read again), `relay_eval` (an expression in an extension's relay page, its answer logged as INFO)). The hook is for scripted runs only and is powerful while set (it loads extensions and sends DevTools messages on the management browser's browser-wide channel). On Unix the engine refuses, with a logged warning and the hook off, a directory not owned by the profile's owner or writable by its group or others (use mode 0700); on Windows it refuses a directory that is not inside the user's own profile tree (the canonical `%USERPROFILE%`, with `%TEMP%` normally inside it, compared part by part and case-insensitively: `extensions.rs` `outside_profile`), and any platform without a rule refuses; `NAUTILUS_PUMP_LOG=1` reports a message pump that has not spun and `NAUTILUS_PUMP_TICK_MS` sets the pump's fallback interval (0 switches it off). `NAUTILUS_TEST_CONTROL`, `NAUTILUS_TEST_PAGE_SCRIPT`, `NAUTILUS_PUMP_LOG` and `NAUTILUS_PUMP_TICK_MS` exist only in a build with the `test-hooks` cargo feature (`cargo run --features test-hooks`, `cargo xtask bundle-mac --features test-hooks`): a packaged build has none of them and logs a warning when one is set, and the package job fails if a hook's log line is in the binary. Build the bundle with the feature before the scripted runs below. CI lints and tests with `--all-features` as well as without, so run both before a commit that touches a hook. `ci/side-panel/run.sh spike|pump` is the scripted run of the extension bridge on a throwaway profile (macOS; about six minutes; judge it by its exit status, never through a pipe); `reload`, `zombie`, `stop`, `idle` and `broken` are the provocations of review cycles 3 and 4 (I10: a reload and a worker stop with a debugger on a background tab, `zombie` with the extension list held during the reload; I11 and I14: a relay that cannot load, with a debugged tab; I15: an idle worker, which must not be started again), each about two minutes (`idle` up to four). A launch that exits non-zero or by a signal, is still up 120 s after its `--exit-after` (the script kills it, only if the pid is still the browser it launched), or logs a panic, a fatal runtime error, a RefCell borrowed twice (`already borrowed` or `already mutably borrowed`), or browsers still open at shutdown fails the run. `python3 ci/side-panel/phase5.py` is the scripted run of `chrome.tabs` and `chrome.windows` over the bridge (two suites, `main` and `restore`, on the throwaway profile `side-panel-p5`; judged by its exit status; `docs/bridge-protocol.md`, "The scripted run (phase5.py)"); on 2026-11-18, after the deferred install of an extension page's namespaces, the browser no longer traps and the page is bridged; a page's calls are held for their tab and extension, and 72 checks pass, with none failing (see the status of 2026-11-18, the page's report) (`docs/bridge-protocol.md`, "The scripted run (phase5.py)").
- `CONTROL=<dir> ci/side-panel/run.sh serve` — one launch of the bundle
  with no driver and no DevTools port, for a checker that drives the
  browser through the control directory itself: `CONTROL` (required,
  mode 0700; the script writes `cmd-1.json`, the `load`, and the caller
  `cmd-2.json` onward), `EXT_DIR` (the extension to load), `SECS`
  (`--exit-after`), `URLS`, `PROFILE`, `DATA_DIR` (a scratch data
  directory to keep across two launches) and `LOAD=0` (load nothing;
  the caller starts at `cmd-1.json`). The browser's log comes out on
  stdout, and the launch is judged as every launch is. A third-party
  extension is tried the same way, from a copy of its folder (the
  status section of 2026-11-27).
- `ci/side-panel/panel.sh panel|groups|behavior` and `panel.sh
  sample-global|sample-site <cookbook dir>` — the side panel, tab
  groups and commands in the real window (macOS; the same bundle;
  judged by the exit status, never through a pipe; `README.md`,
  "Tests", says what each mode checks). `panel_checks.py` starts the
  browser through `run.sh serve`, loads `ci/side-panel/panel-ext`, and
  reads what the extension reports to its server on 127.0.0.1:47616;
  captures of the launched browser's own window (found by its pid,
  `window_id.swift`) land in `ci/side-panel/out/`, which is ignored.
  `HOLD` (default 180) is how long the browser stays open after the
  checks, `BUDGET` (150) the `panel` suite's time. Run `panel`,
  `groups`, `phase5.py` and `run.sh pump` after any change to a script
  under `bridge/js/`: a changed script is rewritten into every bridged
  extension at its next listing.

## Conventions

- **CEF stays in `nautilus-engine`.** The binary never names a CEF type;
  the shell sees only osmena's types.
- **Shared behaviour changes in osmena.** A change to the model, the
  chrome, routing, menus, or the settings window is an osmena change:
  patch the osmena checkout in `~/.cargo/config.toml` (osmena's README),
  make it there with its tests, push, and bump the `rev`s here. Only what
  Chromium needs lives in this repository.
- **Extensions have one platform seam.** Windows and macOS run
  extensions; nothing else does yet. The gate is
  `cfg(any(windows, target_os = "macos"))`, and it stands in few
  places: `mod bridge` in `lib.rs` (the whole bridge, once), the
  `native` and `stub` halves of `ExtensionManager` (`extensions.rs`)
  and of `Popups` (`popup.rs`), the delegate's ends of the bridge (one
  `impl` block, two fields and the process-message hook in
  `delegate.rs`), and the script-context hook in `store.rs`. The engine
  itself has none: it reaches the bridge only through
  `ExtensionManager` (`strip_changed`, `extension_reply`,
  `poll_bridge`, `tick_bridge`, `release_bridge`, and for the host
  `action_clicked` and `command`), which the stub answers with
  nothing. Supporting another platform (Linux) means the
  two windows that are platform code, the manager's hidden window and
  the action popup, and then widening that one condition; the bridge
  has no platform code of its own. No build on another platform is
  made or tested, so expect the stub to need care the first time.
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
- **A view is shown again through `show_view`.** Telling a windowless
  view it is no longer hidden is `delegate::show_view(&host)`, never a
  bare `was_hidden(0)`: Chromium evicts a hidden view's surface once
  two other windowless views are showing, and only the
  screen-information notice in `show_view` gets it a new one in every
  case (C16). A windowless browser nobody looks at (the bridge's
  hidden pages) is told it is hidden when it is made and stays so:
  showing, it would count against the tabs' surfaces and against a
  popup widget's painting (C15).
- **One `Runtime` per process, an `Engine` per open profile, after
  `run_subprocess`.** CEF is process-global and starts once: `main`
  builds a `nautilus_engine::Runtime` and hands osmena a start closure
  that calls `Runtime::start(profile, waker)`. The first call
  initialises CEF (`Shared::initialize`, with an external message pump:
  CEF asks for work through `on_schedule_message_pump_work`, the
  scheduler wakes the winit event loop through osmena's `Waker`, and
  the shell calls `Engine::spin`, `do_message_loop_work`); every call
  returns that profile's `Engine`, which holds an `Rc` of the shared
  state. `cef::shutdown` is `Shared::drop`: it runs when the shell has
  dropped every engine and then the closure, inside the event loop.
  Every CEF callback arrives on the main thread from inside `spin`, so
  the handlers use `Rc`/`RefCell`; `spin` on any engine runs the one
  loop, so a callback for profile A can arrive during profile B's
  `spin`, and the `spinning` flag is the process's, not an engine's.
- **A profile is a request context.** Chromium keeps every profile
  under its root directory, which is the default profile's `chromium/`
  (`Profile::shared_engine_dir`), and takes a profile's directory only
  as a direct child of it. The default profile is Chromium's global
  context (`Default/` there, as it always was, so nothing of it
  moves), and only because `on_before_command_line_processing`
  passes `--profile-directory=Default`: without it the global context
  is whichever profile a Chrome-style window used last, and the
  default profile's windows share that profile's cookies (C17); any
  other profile is a `RequestContext` at
  `profile-<name>/` (`Profile::shared_engine_data`), made in
  `Shared::context_at`, which pumps the message loop until Chromium
  has loaded it (a browser cannot be created in it before) and fails
  if Chromium answered with an in-memory profile. Every browser of a
  profile is created in its context, which lives in its
  `DelegateState` (`request_context`): the five creation sites (a
  tab and a side panel's view in `host.rs`, `create_tab` and
  `create_view`, the extension manager's page in `extensions.rs`,
  a hidden page in `bridge/hidden_page.rs`, an action popup in
  `popup.rs`) pass it, and a new one must too, or it lands in the
  default profile. `DelegateState`, the extension manager and the
  bridge are per profile; a browser Chromium creates on its own finds
  its profile by its request context (`Delegates::of`). Paths given to
  Chromium are resolved first (`resolved_dir`: `/var` is a link on
  macOS, Chromium compares the root as a string, and it reports
  extension paths resolved). A profile from a build that ran one
  process per profile has its data in its own `chromium/Default/`:
  `adopt_profile_data` renames it into place the first time, never
  over data already there. Its extensions stay under its own
  `chromium/extensions/`, where Chromium's preferences name them by
  absolute path.
- **Browsers close on the message loop.** `TabHost::close_tab` asks the
  browser to close; `on_before_close` arrives later. `Engine::drop` pumps
  until its profile's live-browser count is zero (bounded) and lets go
  of the request context; CEF shuts down when the last engine and the
  `Runtime` are gone. The shell drops a profile's windows (tab hosts)
  before its engine.
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
  binding's `os_event` type differs. On macOS it then answers every
  key as handled (`mac_unhandled_key`): CEF would otherwise offer its
  synthetic `NSEvent` to the menu bar, whose hidden fn-E and fn-D
  items ("Emoji & Symbols", "Start Dictation…") match a plain E or D
  (C7 in `docs/upstream-issues.md`). The menu bar has had the real
  event already; do not let unhandled keys through to it again.
- **Key events follow each platform's native message.** On Windows CEF
  builds a `ui::KeyEvent` from a `CefKeyEvent` (`TranslateUiKeyEvent` in
  its platform delegate): a `CHAR` event's character is read from
  `windows_key_code` (as `WM_CHAR`'s `wParam`; `character` is ignored),
  a key-down's DOM `key` from the virtual-key code and the layout, and
  the DOM `code` from `native_key_code`, which must be the scan code
  with `0xE000` for extended keys. `input::key_events` and
  `keycodes.rs` follow that: `0x41` on the down event and `'a'` on the
  `CHAR` event for the A key. On macOS CEF reads `native_key_code`
  (the `kVK_*` code) and `character` and ignores `windows_key_code`,
  and an event whose `character` and `unmodified_character` are both 0
  is a modifier's flags change, which Chromium reads as a press for any
  other key, releases included (C8). So on macOS every key but a
  modifier carries a character (`input::event_character`: its text,
  else AppKit's `NSDeleteCharacter` or `NS…FunctionKey`), and a
  modifier key's own event carries its modifier flag only on the press
  (osmena's `keyutils::with_own_modifier`).
- **A popup is a browser Chromium creates; a clicked link is a URL.**
  `on_before_popup` (`window.open`, a link or form with a target)
  returns 0 and lets Chromium create the popup, having made it
  windowless in the opener's window and given it a tab client whose
  `ViewState` is no tab yet (`ViewState::popup_of`). Only that keeps
  `window.open`'s result, `window.opener` and window names working,
  which sign-in popups depend on; never go back to cancelling it and
  loading the URL in a fresh browser. The popup's own
  `on_after_created` reports `EngineEvent::NewTabRequested` with a
  `PendingTab` holding the view and the browser (`Pending::Popup`),
  and `AdoptTab` makes them the tab. Until then the view has no
  `TabId`: every event for a tab goes through `ViewState::report`,
  which holds it and replays it at `ViewState::adopt`; a discarded
  popup is closed. `on_open_urlfrom_tab` (a middle-, Ctrl- or
  Shift-click) still cancels and reports a `PendingTab` holding the
  URL (`Pending::Url`, `background` for `NEW_BACKGROUND_TAB`), or
  `NewWindowRequested` for `NEW_WINDOW`, which `on_before_popup` also
  does for that disposition; those get a fresh browser and no opener.
- **Input to CEF is logical pixels.** `EngineInput` carries physical pixels
  relative to the page area; `TabHost::input` divides by the scale factor
  and tracks click counts and held buttons, which CEF expects the embedder
  to supply.
- **Drags are carried out within the view.** A windowless browser
  expects its embedder to run the system drag Chromium starts
  (`RenderHandler::start_dragging`) and to feed Chromium's drop-target
  side meanwhile; the `cef` crate's default answers "abort", which
  ends the drag without the page's `dragend` and leaves the element
  as `dragstart` left it (faded, hidden, `pointer-events: none`:
  "draggable divs stop hovering"). `TabRenderHandler::start_dragging`
  answers "handled" and keeps a `DragSession` on the `ViewState`
  (Chromium's own clone of the drag data, the allowed operations, the
  drag image composed into the `Frame` under the pointer). No system
  drag starts: the shell's pointer capture already sends the pressed
  tab every move and the release, and `TabHost::drag_input` turns
  them into `drag_target_drag_enter`/`drag_over` while the pointer is
  over the view, `drag_target_drag_leave` off it, and on the primary
  release `drag_target_drop` (over the view) then
  `drag_source_ended_at` and `drag_source_system_drag_ended`, in CEF's
  order (target before source); Escape, a release off the view, or a
  release the shell synthesises on focus loss end it with no
  operation. `update_drag_cursor` gives the drop effect, shown as the
  no-drop cursor or the arrow; the page's own cursor is put back at
  the end, since Chromium does not repeat one it believes is still
  shown. Never return 1 from `start_dragging` without the two
  `drag_source_*` calls to follow: Blink suppresses every input event
  until they arrive. Not done: dragging out of the browser (an OLE
  `DoDragDrop` with a nested loop inside the callback, as cefclient's
  `osr_dragdrop_win.cc`), files dropped in from the desktop (winit's
  `DroppedFile` into `drag_target_*` with a `DragData` from
  `drag_data_create`), drags between tabs or windows. One Chromium
  feature goes with this: `on_before_command_line_processing` enables
  `GenerateDragOverlayBeforeDragStart`, without which Blink cancels
  every drag whose `dragstart` handler sets `pointer-events: none` on
  the element, before the embedder is ever asked (C9 in
  `docs/upstream-issues.md`).
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
- **The microphone and the camera are the user's to give.** An Alloy
  browser without a permission handler refuses every `getUserMedia`.
  `TabPermissionHandler::on_request_media_access_permission`
  (`delegate.rs`) keeps Chromium's callback with the permission bits
  in `ViewState::media_requests` under an id from `DelegateState`,
  reports `EngineEvent::MediaAccessRequested` (through
  `ViewState::report`, so a popup's request waits for its tab) and
  returns 1; `TabHost::answer_media_access` calls `cont` with exactly
  the bits that were asked for (anything else is Chromium's
  `INVALID_STATE`) or `cancel`. `media_request` maps the bits: the
  microphone and the camera only, so screen capture and an empty
  request return 0, which Chromium refuses. Who is asked, and what is
  remembered, is osmena's. The request must not stay pending across a
  loss of focus (C10 in `docs/upstream-issues.md`); osmena's modal
  dialog sees to that. `on_show_permission_prompt` (notifications,
  location and the rest) is not implemented: Chromium ignores those.
  Never add `--enable-media-stream`: it grants every page everything.
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
- **The pump runs CEF's work unasked.** `on_schedule_message_pump_work`
  is a hint, not a complete account: CEF leaves work behind after a
  10 ms slice and Chromium then does not ask again (C14). So
  `PumpScheduler` clamps every delayed request to 33 ms, lets a later
  request replace the pending one, and arms the timer after every
  `spin`. Do not remove the tick to save idle wake-ups without
  running `ci/side-panel/run.sh pump`.
- **The extension bridge reaches a worker through one of the
  extension's own pages.** CEF's frame hooks never fire for a service
  worker and `store.rs`'s binding has no way back into a renderer, so
  the bridge uses neither: `bridge.js` is loaded by a loader the
  rewritten manifest names, the loader sits beside the original entry
  (relative `importScripts` resolve against the worker's URL), and
  the engine talks to a relay page with `Runtime.evaluate`, as the
  manager talks to `chrome://extensions`. Every DevTools call carries
  an id of the caller's own, recorded before the call (CEF can answer
  inside it): CEF keeps an id that is not below the browser's next one,
  so ids must increase on each browser (the status section of
  2026-11-19, the message ids). Names in an extension's
  root must not start with `_`. Every extension is bridged when it loads
  (store install, "Load unpacked…", `NAUTILUS_TEST_CONTROL`'s `load`:
  `ExtensionManager::load_bridged`), from Phase 5 of the side-panel plan on.
  Extension pages are bridged too (`bridge/pages.rs`): the render handler in
  `store.rs` puts `pages::install` into each extension page's context when it
  is created, which evaluates the transport (the namespaces wait for the
  page's first document change, `js/page_defer.js`, because Chromium defines
  `chrome` in the page only after its context is created; the first change
  comes before the page's first script runs). Not the relay page, not the
  helper's pages. Not Chromium's own extensions either (`pages::CHROMIUM_OWN`:
  the PDF viewer, whose page reads the tab's zoom through `chrome.tabs`
  and draws nothing when refused); `extension_of` is the one gate, for
  the render process and the browser process alike. The page's calls come back over the page lane (`PageLane`, a process message the
  browser process answers from the calling frame's own address; `Hub::serves`
  refuses a page of an extension the bridge does not serve). An extension
  without a service worker is served through its pages alone
  (`BridgeState::set_extensions` takes each served extension's worker flag),
  and a page's listeners are recorded for its frame (`BridgeCall::page`), so an
  event reaches only the pages that registered it. A user folder that an
  earlier build loaded is moved into its copy by `bridge/migrate.rs`, which
  removes the original only once the copy is listed.
- **A tab a debugger is on is painted.** The bridge reports each
  bridged extension's `chrome.debugger` attach and detach, by tab id or
  by `{targetId}` (`debugger.attached` and `debugger.detached`,
  `docs/bridge-protocol.md`), and the worker's call waits for the engine
  to show the tab first. `bridge/debugger.rs` keeps which extension
  debugs which tab, and `table::TabTable::resolve` maps the debuggee; a
  hidden tab is shown while any debugs it and hidden again when the last
  debugger goes. Every extension that loads is bridged, and both kinds of
  context report: a worker through its transport (`js/bridge.js`,
  `Bridge::worker_call`), a page through `js/debugger.js` over the page lane
  (`Effect::Debugger`, `Bridge::page_debugger`), into the one set, since
  Chromium keeps a debugger session per extension and tab, not per context.
  A view the shell does not show raises no `FrameReady`
  (`ViewState::frame_painted`), so a tab painted for a debugger does not
  redraw its window.
  A worker's hello (`worker_hello`) gives each tab's session as attached,
  not-attached or unknown (`policy::tabs_to_forget`); only not-attached
  forgets a debugger, and `policy::forgets` is the one gate (review cycle 4:
  uncertainty never forgets). A reloaded extension's worker target changes
  (`policy::worker_target`, the helper lists it as type `worker`); the change
  asks the relay which worker it reaches (`identify`, from `check_worker_targets`),
  and the answer decides: a hello keeps the relay, and a failed answer closes it
  as a zombie, only for the latest change (`policy::asks_on`, `policy::after_ask`).
  A hello that arrives outside an ask decides nothing. Never key this on CEF's
  DevTools-agent callbacks: they fire for the engine's own
  `Target.getTargetInfo` probe and for every tab at creation, so they say
  nothing about an extension's debugger.
- **Relays are opened on the engine's turn, never in a callback.**
  `Bridge::sync` records what is wanted and closes what is gone;
  `Bridge::reopen` (`ExtensionManager::poll_bridge`, called from `spin`)
  opens what is wanted and not open, on `policy::next_opening`'s schedule
  (250 ms, then 500, 1000 and 2000 ms; five failed openings in a row, then
  given up until a sync sees the extension change: `Bridge::give_up`, which
  keeps its debuggers).
  A relay whose evaluation fails, or whose DevTools session ends, is closed
  in its callback and opened by the next turn. A
  debugger report is never failed by a page's loss (`policy::failed_by_loss`);
  a full queue of them applies its oldest against the table instead.
- **A CEF handler lends the bridge nothing of its own.** A call into
  the bridge can close the page the calling handler belongs to
  (`Bridge::close_relay`), and closing drops the handler while its
  callback runs: CEF holds no reference across the call. The handlers
  in `bridge/hidden_page.rs` clone their `Rc` of the bridge and their
  key first; a borrowed `&self.key` there was read after it was freed
  (2026-11-27, a test holds the file to it). A new handler that calls
  something able to close its own browser does the same.
- **A side panel's view is a tab's view with one more field, and
  never a tab to extensions.** osmena allocates the view's `TabId`
  and asks for it (`TabHost::create_view`, `close_view`,
  `set_view_origin`); the host makes a windowless browser with the
  tab client in the profile's request context, hidden until shown, on
  `popup_pause`'s list, with a size of its own (`panel::Views`,
  `ViewSizes`). `ViewState::panel` is what makes it a panel: it
  reports only what `panel::panel_reports` allows (its frames, its
  cursor, its close and its crash), refuses the microphone, the
  camera and the context menu, and opens what it asks for as a tab
  beside the window's shown tab. `create_view` never calls
  `extensions.track_tab`, so the view has no row in the tab-id table
  and no part in the debugger's show and hide; its page's tab report
  is kept beside the table (`table::PanelIds`), which is how a
  message from it loses `sender.tab`. Which panel is open where is
  the model's; the engine's `panel::PanelRegistry` in `DelegateState`
  only joins a request to the view made for it and answers a panel
  page's window (`window_of`). A request of the engine's own (a click
  that opens a panel, a `setOptions` that closes one) carries a token
  with `ENGINE_TOKEN` set and is settled in `DelegateState::note_reply`.
- **`chrome.sidePanel` is the engine's entirely.** No call of it
  reaches Chromium, whose side panel has no window here (C13).
  `js/side_panel.js` is written by hand, not with `define`
  (`docs/bridge-protocol.md` says why); its calls and
  `runtime.getContexts` leave `BridgeState::call` as
  `Effect::SidePanel` and are answered by `Hub::side_panel_call` from
  `bridge::side_panel::SidePanelStore` (the options, for the run) and
  the panel registry, never by `router::route`. What
  `setPanelBehavior` chose is the one thing kept, in
  `<profile's chromium>/side-panel-behavior.json`, written beside
  itself and renamed. An extension without the `sidePanel` permission
  gets nothing of the script but the relay page left out of its
  `getContexts`: its `window.close` and `getContexts` stay Chromium's,
  and neither waits on the engine for long in any extension
  (`GET_CONTEXTS_WAIT`, `CLOSE_WAIT`). A click on an action is one
  decision for the toolbar and the test control alike
  (`ExtensionManager::click`, `side_panel::decide_action_click`), in
  Chromium's order: the panel, the popup, `action.onClicked`. No user
  gesture is asked for (C20).
- **A replaced namespace is pinned on `chrome` and on `browser`,
  through `pin` and nothing else.** Chromium sets every API on both
  globals and sets them again in live contexts when the extensions
  change, so a replacement is put in by `__nautilusBridge.pin` alone
  (`js/core.js`; `define` and `js/side_panel.js` call it): the same
  object on both, not configurable, not writable (C19). No namespace
  script sets a property of `chrome` or `browser` itself. The rule is
  fail-closed: a function of a replaced namespace the engine does not
  know rejects (`<namespace>.<name> is not supported.`) and is never
  forwarded, and a context whose bridge cannot be put in has `tabs`,
  `windows`, `tabGroups`, `commands` and `sidePanel` refused on both
  globals (`__nautilusRefuseNative`, `js/refuse.js`, the first script
  of a page and of a worker), because Chromium's own
  `tabs.group` ends the browser process (C11). Each namespace is
  installed in its own guard (`install` in `js/core.js`, and the `try`
  `namespaces_script` puts around each script): one that fails is
  refused on both globals and the others are installed. A new
  namespace script gets a row in `NAMESPACES` (`bridge/mod.rs`) and in
  `bridge_check.mjs`'s list of the same name. Never fix a refused
  function by letting it through; give it a row in the router. The
  tests are `every_function_of_a_replaced_namespace_is_ours`,
  `tab_groups_and_commands_are_replaced_whole_and_refused_when_they_cannot_be`
  and `every_side_panel_function_is_ours` (`bridge/mod.rs`), with
  `ci/side-panel/bridge_check.mjs`.
- **A tab group is a tab folder, asked for through an
  `ExtensionRequest`.** `bridge/groups.rs` (pure) maps a group onto a
  folder: the id is the folder's number, every group is grey,
  `tabs.group`, `tabs.ungroup` and `tabGroups.update` become
  `GroupTabs`, `UngroupTabs` and `UpdateFolder`, and `tabGroups.move`
  is refused, the model having no request for it. A group call
  resolves on the published strip that shows its own change
  (`router::settle`, `Then::Group`, `groups::GroupThen`), not on the
  model's reply and not on any publish after it, since the strip is
  republished as it is when the tab-id table changes: an extension
  reads the group back straight after the promise. The group events
  are differences of two strips (`groups::group_events`), so a folder
  made by hand fires them too.
- **A command is reported with its delivery, never without it.** From
  the moment `Extension::commands` carries a chord, osmena takes that
  key from the page and calls `TabHost::extension_command`; an engine
  that reported the chord and did nothing would swallow the key. So
  the report (`commands.rs`, `read_manifest`, `on_list`) and the
  delivery (`ExtensionManager::command`, `Hub::command`,
  `BridgeState::command`, which sends `commands.onCommand` through
  `BridgeState::emit` to the contexts that listen) change together.
  An action command is the toolbar's click. The keymap is osmena's
  and the engine is not given it: `commands.getAll` reports the
  declared shortcut.
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
- **A list of features is added to, never replaced.** CEF sets
  `disable-features` itself before `on_before_command_line_processing`,
  and appending the switch again replaces the value, which crashes
  Chromium at startup (C18). Features go through `add_to_list_switch`
  (`engine.rs`), in the browser process only; the defaults are
  `DISABLED_FEATURES`, each with the reason and the Thorium patch it
  follows. A source patch of Thorium's can be followed only where a
  feature or a switch reaches the same code: Nautilus ships CEF's
  binary, so nothing of Thorium's build (`-O3`, AVX, ThinLTO, PGO) or
  of its source changes applies. Add a feature only with a run that
  shows its effect (a page probe, or a network log:
  `NAUTILUS_CHROMIUM_SWITCHES=log-net-log=<file>`).
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
  no `<text>`, no raster `<image>`: osmena's xtask builds resvg without
  font support), run `cargo xtask icons`, commit the result. Never
  hand-edit `assets/icons/`. The `.icns` comes from
  `assets/logo-macos.svg`, which places `logo.svg` on Apple's icon grid
  (the 824 px tile at 100 px in a transparent 1024 px canvas, 185.4 px
  continuous corners, the template's shadow): macOS before 26 masks
  nothing, so the shape and margin must be the icon's own. Keep
  `logo.svg`'s tile at 976 px from 24 with radius 219.6 px, or redo the
  placement in `logo-macos.svg`. osmena leaves the Dock to the bundle's
  icon.
- **The bundle claims web links.** `WEB_BROWSER_TYPES` in
  `bundle_mac.rs` puts `http`, `https` (`CFBundleURLTypes`) and HTML
  pages (`CFBundleDocumentTypes`) in the browser's `Info.plist`: that is
  what makes System Settings list Nautilus as a default browser, and
  what osmena's shell checks before it offers to become the default.
  The shell answers the system's `GURL` and `odoc` Apple Events itself.
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

## Status and handoff (2026-11-27, the extension side panel)

The owner: "I still can't open the Claude extension sidebar in
Nautilus." The Claude extension has no popup: it opens a side panel
with `chrome.sidePanel.setOptions` and `open` from its toolbar button
and from its shortcut (Cmd+E), and puts the tab in a tab group of its
own on every open. None of that had anywhere to go: Chromium's side
panel has no window here (C13), its `tabs.group` ends the browser on
these tabs (C11), and no key reached `commands.onCommand`. This is
Phases 6 and 7 of osmena's side panel plan and the documents of its
Phase 8, on the branch `side-panel-phases-6-8`, not merged and not
pushed; osmena's pin is `13b073b`.

What exists, by area (the conventions above say how each hangs
together, `docs/bridge-protocol.md` has the wire, the CHANGELOG what a
user sees):

- **The panel's view.** `panel.rs` (pure), `ViewState::panel` and the
  panel registry in `delegate.rs`, `TabHost::create_view`, `close_view`
  and `set_view_origin` in `host.rs`; `INFO.extensions.side_panels` is
  true.
- **`chrome.sidePanel`.** `bridge/side_panel.rs` (the store, the
  routes, Chromium's rejections, the action click, `getContexts`),
  `js/side_panel.js`, `Effect::SidePanel` and `Hub::side_panel_call`,
  the behaviour file, the events through `BridgeState::emit`; a panel
  page's current window from the registry; a panel page's message
  without `sender.tab` (`table::PanelIds`, `js/runtime.js`).
- **Tab groups.** `bridge/groups.rs`, `js/tab_groups.js`, the
  `tabGroups` arm and the `group` and `ungroup` rows of the router,
  `Then::Group`, the group events in the differ.
- **Commands.** `commands.rs`, the commands in `read_manifest` and
  `on_list`, `TabHost::extension_command`, `BridgeState::command`,
  `js/commands.js`, the test control's `extension_command`.
- **`browser`.** Every replacement goes through `pin` in `js/core.js`,
  onto `chrome` and `browser` (C19): on `main` since Chromium 156,
  `browser.tabs` and `browser.windows` were Chromium's own, which is
  how an extension written against `browser` reached C11 and C12.
- **The debugger from a page.** `js/debugger.js`, `Effect::Debugger`,
  `Bridge::page_debugger`; `ViewState::frame_painted`.
- **The click's tab** carries the strip's fields
  (`relay::click_message`, `Hub::strip_of`).
- **The harness.** `run.sh serve`, `panel.sh` with `panel_checks.py`,
  `ci/side-panel/panel-ext` and `popup-ext`, `window_id.swift`.
- **Found by the real run and fixed** (`7f58a24`): a hidden page's
  handlers lent the bridge their own key, read after the page was
  closed inside the call (the convention "A CEF handler lends the
  bridge nothing of its own").

Verified, all on the Mac (Apple silicon, CEF 156), every launch on a
scratch `--data-dir`:

- Unit tests and the gate at `79eb6c4`: `cargo fmt --all --check`,
  clippy with and without `--all-features`, `cargo build --workspace`,
  `cargo test --workspace` (349 and 3; 362 and 3 with all features),
  `node ci/side-panel/bridge_check.mjs`. Again at `7f58a24`, after the
  merge of `main`, the pin's move to `13b073b` and the fix: 350 and 3;
  363 and 3.
- Scripted, the bundle with `test-hooks` at `79eb6c4`, each exit 0
  with no check failing: `panel.sh panel` (116 checks: the panel for a
  window and a tab, the widths 1024 to 664 beside a panel of 360, the
  rejections, `getContexts`, `setOptions`, `window.close()`, the click
  through the control, a second window, `browser.*`, a second
  extension installed mid-run, a debugger from the panel's page on a
  background tab with each command answered in under a second, where
  three took 5003, 5004 and 5004 ms before), `panel.sh groups` (44),
  `panel.sh behavior` (23, two launches), `phase5.py` (76),
  `run.sh spike` (54), `pump` (7), `stop` (17), `idle` (9), `broken`
  (14). The captures were looked at: the panel at the right under its
  header, a tab's and a window's panel in turn, the folder in the
  strip.
- Scripted, earlier the same day, before the groups and commands work
  and not run again since: `panel.sh sample-global` (11) and
  `sample-site` (22) on the two side panel cookbooks of
  chrome-extensions-samples at `7314683`.
- The owner's own two extensions, each from a copy of its installed
  folder loaded through the control (the installed profile was only
  read), in scratch profiles. Claude 1.0.107, whose worker is a
  module: listed, bridged and on; the worker says hello; the control's
  `click` is `sent to the bridge` and the extension's `setOptions` and
  `open` make the panel's view (`side panel TabId(n):
  chrome-extension://…/sidepanel.html?tabId=…`); the capture shows the
  panel at the right under a header reading "Claude" with the
  extension's own page in it; `tabs.group` and `tabGroups.update`
  became `GroupTabs` and `UpdateFolder` and the strip shows a folder
  "Claude" around the tab; no bridged call was rejected. Signed out
  the page is its sign-in screen and `toggle-side-panel` through
  `extension_command` opens the panel but does not close it: the
  extension asks its panel's page to close itself and no close came
  from that screen. The owner then signed in by hand in that window,
  and on the same profile started again: the page is the extension's
  first-run screen, the command closes the panel (`sidePanel.closeSelf`
  from the page, `CloseSidePanel`, the view's browser closed) and the
  next opens it; a launch that ends with the panel open exits 0 with
  no browser left. That second launch's log also has two clicks the
  control did not send (`action clicked for TabId(2)` with no `TEST
  HOOK` line: the toolbar's button or the puzzle menu), after which
  the panel opened, and then the panel's view taking the keyboard,
  which no launch shows without a click in the panel: the owner was
  at that window. Nobody wrote down what they saw. A plain restart of
  a profile that has the extension
  (no `load`): hello in two seconds and a click opens the panel.
  Proton Pass 1.43.0, alone for 75 s on three pages and together with
  Claude: listed, bridged and on, three `tabs.query` and one
  `tabs.create` answered, no `windows` call, nothing rejected, its
  content scripts' messages reaching its worker; a `click` on it
  answers `the action has a popup, which the control does not open`.
  No launch had a panic, a RefCell borrowed twice or a browser left
  at shutdown; every one exited 0.

Not verified:

- By hand, apart from what the owner did in that one window: the
  pointer's place in a panel and a `<select>` there, typing in a
  panel and the keyboard coming back to the page, the header's close
  control, Cmd+E and any other extension shortcut by a real key, the
  keymap beating an extension's chord, Settings' list of shortcuts by
  eye, an extension's popup (the control opens none, so Proton Pass's
  popup and its `tabs` calls from a popup were not run), a popup from
  a keyboard `_execute_action`, a tab's panel against a window's with
  real tab switches, full screen with a panel, a panel across a
  display-scale change, a download started from a panel, a panel
  whose renderer crashes. README, checklist item 25.
- The Claude extension at work: no message was sent through it, so
  its `chrome.debugger`, `scripting`, `tabs.create` for a task,
  screenshots of a background tab, offscreen document and native
  messaging did not run here. Raindrop.io: not run at all.
- Windows: nothing of this has been built or run there locally (the
  bridge has no platform code; CI builds and tests it).
- After the pin moved to `13b073b` and `main` was merged: only the
  gate and the real-extension runs. `panel.sh`, `phase5.py` and the
  `run.sh` modes were not run again; `run.sh reload` and `zombie` not
  since Phase 5.
- C19's cost (an optional permission granted to a live context): read
  in Chromium's source, not seen.

Known and left:

- `README.md`, Known limitations, "Extension side panels, tab groups
  and shortcuts" has the differences from Chrome; they are not
  repeated here.
- Proton Pass's content scripts logged `This script should only be
  loaded in a browser extension.` three times in the first second of
  one launch, as the extension was installed into three tabs that
  were still loading; not in the launch with both extensions, and
  their messages reached the worker afterwards. The cause was not
  looked for, and whether `main` does the same was not tried.
- The control's `load` of a folder over a profile that already has
  that extension reloads it at start, and the Claude extension's
  worker then did not acknowledge `bridge.wake` twice, ten seconds
  each, and said hello only when a click started it. A plain restart
  does not do this. Not looked into further: it is the scripted way
  in, not a user's.
- `runtime.getContexts` is answered from the engine's state when
  Chromium's own answer has come back, which can be after a
  `sidePanel.open` made later in the same turn. The Claude extension
  asks, then opens, then reads the answer to decide between opening
  and closing; in every run here the answer was the state before the
  open. If it ever is not, the extension skips its grouping for that
  open. Not seen.
- The other DevTools observers and handlers of the crate
  (`extensions.rs`, `relay.rs`'s tab probes, `popup.rs`) were not
  read for the lending fault `7f58a24` fixed in `hidden_page.rs`.
- The open contract question: `commands.getAll` and the keymap's
  verdict need a defaulted `Engine` method in osmena.
- The by-hand acceptance, the Windows run, the pull requests and
  Paladin against the pin are the rest of Phase 8.

## Status and handoff (2026-11-24, the PDF viewer)

The owner: a PDF in a page's `<iframe>` would not show. No PDF did,
as a tab's document either: an empty dark page. Chromium's PDF viewer
is an extension page (`mhjfbmdgcfjbbpaeojofohoefgiehjai/index.html`),
so since the page lane (2026-11-19) it was given the bridge's `tabs`
and `windows`, and the hub, which does not serve it, refused its
calls ("the extension is not bridged."); its `browser_api.js` then
threw on the zoom settings and drew nothing. The fix is
`pages::CHROMIUM_OWN`, which `extension_of` refuses.

Verified on the Mac (Apple silicon, CEF 156 at
`~/.local/share/cef-156`), scratch `--data-dir`s, a local page with a
PDF in an `<iframe>` and the PDF as the tab's URL, a capture of each
window: the installed build 94 showed neither and logged the refusal;
the bundle of this tree shows both, with the viewer's toolbar, and
logs neither line. That is also the first run of the 156 bundle on
macOS here (it starts, loads, exits 0). The test's row was red first.
`cargo fmt --all --check`, clippy with and without `--all-features`,
`cargo test --workspace` (252 and 3; 263 and 3 with all features),
`cargo build --workspace`.

Not verified: Windows; the viewer's own controls by hand (zoom,
print, download, a click in the page); the scripted bridge runs
(`run.sh`, `phase5.py`); the manual checklist. Only the PDF viewer is
listed: another of Chromium's own extension pages that a user can
reach would be refused the same way and belongs in the list.

## Status and handoff (2026-11-23, Chromium 156)

The owner's word after the performance review below: "do it". The
`cef` pin is `=156.7.0` (CEF 156.0.31, Chromium 156.0.8078.100),
`INFO.version` is "156", the store's download URL names 156. No
source change was needed: every handler signature this crate
implements is as it was.

Verified on Windows, debug builds (`--features test-hooks`) and a
release build on scratch `--data-dir`s, each launch's exit status 0:

- C17, by value this time: `--profile default`, `--profile work`,
  `default`, `work`, `default`, a page that reads its cookie and
  `localStorage` before it writes them. Each profile found its own
  and never the other's, and `Default/` got no file while Work ran
  alone beyond the global context's own. Name the profile in such a
  run: a start without `--profile` opens the profile that was in
  front last, which reads as the bug.
- C18: `NAUTILUS_CHROMIUM_SWITCHES=disable-features=NoSuchFeature`
  exits 0. A network log of an idle start, fresh profile and second
  launch: the component updater only; no
  `www.google.com/async/folae` (a 156 build made before `AimEnabled`
  was in `DISABLED_FEATURES` did send it), and
  `accounts.google.com/ListAccounts` is gone in 156.
- Two profiles' windows in one process (a second start handed over),
  each page counting its animation frames: 60 or 61 a second in both
  for 18 s.
- The bridge: the test extension loaded through
  `NAUTILUS_TEST_CONTROL`, relay and helper pages ready, the table
  with the tab, the worker's hello, its `tabs.query` and listens.
- Input, posted to the window as messages (no focus needed): a click
  on a text field focused it, `h` and `i` arrived with their `key`
  and `code` and typed "hi", three wheel notches scrolled the page.
- regatta, release build: first window 29 ms, page interactive 1.33 s
  with the same second of proxy auto-detection as that day's 154
  runs. Its scroll and click scenarios did not run (the harness could
  not bring its window to the front while the machine was in use).
- `cargo fmt --all --check`, clippy with and without
  `--all-features`, `cargo test --workspace` with and without.

Not verified, and the bump's real risk: macOS entirely (the bundle,
the helpers, C7, C8, the bridge's scripted runs `run.sh` and
`phase5.py`, which exist only there; CI builds, bundles and
smoke-runs it); C15 and C16 (a `<select>` with other windows open, a
tab shown again after its surface was evicted: the workarounds are
unchanged and their provocations need the temporary hooks of those
days); C9 (the drag feature is still switched on, which is harmless
if 156 has it on already); C10 to C14; C4 (the store's install
button); an extension from the store; downloads, DevTools, printing,
the media prompt; the owner's profiles.

Left: the sandbox (C1); the shell's redraw pacing with swap interval
0 (the section below); Chromium 158 when the `cef` crate has it.

## Status and handoff (2026-11-23, the performance review)

The owner asked for a performance review against Thorium's repository
history, with what applies of its performance and security patches.

What Thorium is, read from its repository at `b2882865` (M158): build
arguments (`-O3`, AVX or AVX2, ThinLTO, PGO), which is where its speed
comes from, and about 150 source patches, nearly all its own UI,
branding, codecs and flags. None of the first kind applies to a
browser that ships CEF's binary. Of the second, three feature defaults
could be followed with a switch and were (the CHANGELOG's entry;
`DISABLED_FEATURES`). Its security is its Chromium milestone: 158
there, 154 here, 156 in the `cef` crate (156.7.0).

Measured on Windows with regatta against a release build of this
tree (scratch results, not kept): scrolling 59 frames a second of 60
(it was 47 on 2026-11-09), click to pixel 66 ms (Thorium 17), page
interactive 1.2 to 1.3 s, the same as Edge and ahead of Thorium that
day, a second of it Chromium's proxy auto-detection timing out on
`wpad` with a VPN up (329 ms on 2026-11-09 without). With the shell's
swap interval at 0 (a temporary switch in osmena's `glutin.rs`, not
committed) click to pixel was 50 ms: one frame of the 66 is the
window's own wait for the display, which also stops the message
pump. Taking that for real needs the shell to pace its own redraws
(pointer moves and egui's animations would otherwise redraw without
limit), which is osmena's work and not done.

Verified, debug and release builds on scratch `--data-dir`s: a page
that reports what it finds (`HTMLFencedFrameElement` a function
before, undefined after; `document.browsingTopics` is a stub in
Chromium 154 either way); a network log of an idle start (three
requests before: `accounts.google.com/ListAccounts`,
`www.google.com/async/folae`, `update.googleapis.com`; the second
gone after, and gone with `AimEnabled` alone); a run with
`NAUTILUS_CHROMIUM_SWITCHES=disable-features=NoSuchFeature`, which
exited 139 before `add_to_list_switch` and 0 after. `cargo fmt --all
--check`, clippy with and without `--all-features`, `cargo test
--workspace`.

Tried and left out: `ParallelDownloading` (Thorium's default). A
throttled 96 MB download from a server with ranges and a strong
validator still came over one connection, so nothing showed it
working. A release profile with ThinLTO: the measured gaps are the
display wait and CEF's own work, not this repository's code.

Not verified: macOS; the owner's profiles; sites that use fenced
frames (none is known to need them).

Left, in the order of what they are worth: the CEF bump to 156 (the
security catch-up; every `C` entry to re-check, the bridge's scripted
runs on the Mac); the sandbox (C1); the shell's redraw pacing with
swap interval 0; `ListAccounts` at start.

## Status and handoff (2026-11-23, the shared cookies)

The owner, on the build with the frozen-tabs fix: "my Work and
Personal Profiles are sharing cookies? I'm logged into the same
Google account across both". Personal is the default profile.

The cause is C17 in `docs/upstream-issues.md`: the default profile is
Chromium's global request context, and CEF gives that context the
profile `Local State` names as last used, which the extension
manager's Chrome-style window sets. After a session in which Work was
the last such profile, the next start opened the default profile's
tabs, extension manager and bridge in `profile-work/`. In the owner's
data directory `Default/` had not been written since that start
(9:07) while `profile-work/` had, `profile.last_used` read
`profile-work`, and both profiles' `extensions.toml` were written in
the same second.

The fix is one switch, `--profile-directory=Default`
(`DEFAULT_PROFILE_DIRECTORY`, `on_before_command_line_processing` in
`engine.rs`), which Chromium reads before the preference.

Verified on Windows with debug builds and a scratch `--data-dir`, a
local page that sets a cookie and `localStorage`, each launch with
`--exit-after 12`, counting the files written under each Chromium
profile during the launch. Before: default alone, 161 under
`Default/`; work, 162 under `profile-work/` (and `last_used` now
`profile-work`); default again, 0 under `Default/` and 49 under
`profile-work/`. After, with `last_used` forced to `profile-work`:
default, 49 under `Default/` and 0 under `profile-work/`; work, 49
under `profile-work/`; default again, 49 under `Default/` and 0 under
`profile-work/`. `cargo fmt --all --check`, clippy with and without
`--all-features`, `cargo test --workspace`.

Not verified: two profiles open in one process (each run had one);
the owner's own profiles and the release build; macOS; that the page
read back the earlier run's cookie (files were counted, not values).

Left for the owner: what was done in the default profile's windows
during an affected session (sign-ins, an extension installed or
removed) is in the other profile's Chromium data and stays there;
nothing moves it back. The default profile's own data was not opened
in such a session and is as it was.

## Status and handoff (2026-11-19, one process)

The owner's report: each profile was a macOS application of its own,
so Cmd+` did not move between profiles. Now one Nautilus runs every
open profile (osmena's change of the same date has the shell's side;
the two convention bullets "One `Runtime` per process" and "A profile
is a request context" have Chromium's). Made on the branch
`single-process` in a worktree beside osmena's and merged to `main` on
2026-11-20 at the owner's word, with osmena at `64da3e9`. That push is
a release: an installed Nautilus updates to it, and from then on moves
each non-default profile's Chromium data the first time it opens.

What changed: `engine.rs` (`Runtime`, `Shared`, `Delegates`,
`Engine::open`, `context_at`, `resolved_dir`, `adopt_profile_data`;
`cef::shutdown` moved to `Shared::drop`; the stray-browser client
finds its profile when its browser exists), `delegate.rs` (the
profile's request context), the four browser-creation sites, `main.rs`
(the `Runtime` in the start closure), and the scripted runs
(`--data-dir`).

Two faults the scripted runs found on the way, both from `/var` being
a symbolic link on macOS (the runs' data now lives under `$TMPDIR`):
Chromium names its root resolved and refused a profile directory given
unresolved, answering with an in-memory profile in which the extension
manager's browser then trapped (SIGTRAP at start); and the extension
manager took its own copy of an extension for a user's folder, because
Chromium reported the copy's path resolved and the store's was not.
`resolved_dir` resolves both, and `context_at` now fails if Chromium
kept no directory for the profile.

Verified on the Mac (Apple silicon) with the bundle built with
`--features test-hooks`, exit status in brackets: `cargo fmt --all
--check` [0]; clippy `-D warnings` with and without `--all-features`
[0, 0]; `cargo test --workspace` [0; 248 and 3 passed] and with
`--all-features` [0; 259 and 3]; `ci/side-panel/run.sh spike` [0, 54
PASS], which runs on a profile that is not the default one, so the
tabs, the extension manager, the helper, the relay pages and the
worker were all in a request context; `python3
ci/side-panel/phase5.py` [0, 74 PASS, 0 FAIL]. osmena's handoff lists
the two-profile runs (one pid for both profiles' windows, the handoff
from a second start, separate cookies and storage, close and reopen,
guests, deletion). The move: a profile made by the build before this
(`6076611`, under a scratch `HOME`) with the test extension loaded,
`localStorage` and a cookie set, then opened by this build on the same
directory: the log has "moved the profile's Chromium data", the
extension is listed and on, and the page read the same `localStorage`
and cookie.

Not verified: Windows (nothing ran; the cookie key there is per root,
so a moved profile is expected to be signed out once: reasoned, not
seen); the other `run.sh` modes (`pump`, `reload`, `zombie`, `stop`,
`idle`, `broken`); an action popup, DevTools and a download in a
profile that is not the default one (the popup passes the context; the
other two take it from their browser); a browser Chromium opens on its
own with two profiles open (`Delegates::of`); a store install in a
second profile; two profiles each running the bridge at once (every
run had one bridged profile); the owner's real profiles (nothing
touched `~/Library/Application Support/Nautilus`); anything by hand.

Known and left: the default profile's Chromium data is loaded whenever
Nautilus runs, even with only another profile open, since it is the
global context; a moved profile's old `chromium/` keeps about 6 MB of
Chromium's root-level files that nothing reads any more (only
`extensions/` there is still used); `profile-<name>` directories live
inside the default profile's `chromium/`, which is where Chromium needs
them and not where a person would look.

## Status and handoff (2026-11-23, the frozen tabs)

The owner, on build 89, with the default profile's window and a new
second profile's (Work), Proton Pass in each: "all the tabs are
frozen" in Work. Opening Gmail froze the tab beside it, then Gmail
froze; no paint, no click, no key, and resizing the window changed
nothing in the page. Closing and reopening the profile brought the
same back.

The cause is C16 in `docs/upstream-issues.md`, which has the detail:
Chromium evicts a hidden windowless view's surface once two other
windowless views are showing, CEF does not give the view a new one
when it is shown again, and a resize that reached it while hidden
leaves CEF holding every later resize. The bridge's hidden pages were
showing, as far as Chromium knew, so two profiles with an extension
each was enough; so are three windows with no extension. It is not
C15's capturer (the same with `--disable-gpu-compositing`), and it is
not `popup_pause`'s bookkeeping, which was the first suspect: in the
owner's process it had no owner and no paused view.

The fix, two parts. `delegate::show_view` is the one way a view is
told it is shown again: `was_hidden(0)`, `notify_screen_info_changed`
(the synchronisation `was_resized` asks for, without CEF's hold),
`invalidate`. `TabHost::show`, both ends of `popup_pause`, and the
bridge's `set_tab_debugged` call it. And a hidden page is told it is
hidden when it is made (`bridge/hidden_page.rs`), for good, so it
holds no surface against the tabs and counts against no popup
widget; `popup_pause`'s list of pages, added earlier the same day, is
gone again. The convention bullet "A view is shown again through
`show_view`" has the rule.

How it was found, for the next one like it: `cdb -pv -p <pid>`
attaches without stopping the owner's browser for more than a moment
and `.dump /ma` keeps the state; `nautilus.pdb` from the build's CI
artifact resolves `popup_pause`'s thread-local (the address of its
`VAL` symbol less `_tls_start`, added to the UI thread's TLS block)
though the build has no type information; CEF's release symbols
(`cef_binary_<version>_windows64_release_symbols.tar.bz2`, a 5.4 GB
`libcef.dll.pdb`) give `dt libcef!CefRenderWidgetHostViewOSR`, and a
search of the dump for that class's first vtable finds every view,
whose `is_evicted_`, `hold_resize_`, `pending_resize_` and
`is_showing_` said what had happened.

Verified on Windows with a debug build (`--features test-hooks`),
scratch `--data-dir`s, the test extension through
`NAUTILUS_TEST_CONTROL`, and temporary hooks, none committed: a page
that counts its animation frames each second, and a console line
that tells its own browser it is hidden and has it shown again
through `show_view` some seconds later, as a tab behind another is.
Frames per second after the view is shown again, old code against
new: two profiles' windows with the extension, 0 against 60; three
windows, no extension, 0 against 60; four windows with the
extension, 60; the windows resized (`SetWindowPos`) while the view
was hidden, where `was_resized` in place of the notice gave 0, 60.
The same through a `<select>` opening and closing in another window
every 8 s: three windows with the extension and four without, every
other view back at 60 after each of four lists (before: the second
view shown never came back, and with four windows none did).
A run of seven and a half minutes, two profiles' windows with the
extension, its pages hidden throughout: 53 lists announced and 53
painted, the other window back at 60 after each, and an action click
sent six and three quarter minutes in was taken by the worker in the
same second, after one attempt. `cargo fmt --all --check`, clippy with and without
`--all-features`, `cargo test --workspace`.

Not verified: the owner's own browser and profiles (the release
build with Proton Pass; the debug build and the test extension stood
in); a tab switch by hand, in either build (the hook calls the same
`show_view` on the same view, but no tab strip was clicked); input
reaching a view brought back (frames were counted, clicks were not
sent); macOS; the scripted bridge runs (`run.sh`, `phase5.py`: macOS
only), which now run with the relay and helper pages hidden for
good, so their timers are throttled as a background page's are; an
extension's `chrome.debugger` attach to a hidden tab, which now goes
through `show_view`.

Known and left: with four windows a `<select>`'s list is announced
and not painted (C15, "still open"); the other windows come back
after it. Why Chromium evicts at two showing views was not read in
its source; the number is what was measured.

## Status and handoff (2026-11-23, `<select>` with two windows and an extension)

Superseded in part the same day (the section above): the pages are
no longer paused with the tabs, they are hidden from the start, and
`track_page`/`forget_page` are gone. The measurements stand.

The owner, on build 89 (which has the fix of 2026-11-22, below): the
list still does not appear with a second window open. Their session
differed from the run that verified that fix in one thing that
matters: an extension (Proton Pass). Each bridged extension keeps a
relay page, and the profile a helper page, in windowless browsers
nobody sees (`bridge/hidden_page.rs`), and Chromium counts those
against a popup widget's painting as it counts a tab's view. C15's
"the bridge's hidden pages do not count" was wrong and is corrected
there, with what was measured; no simple count of views fits it.

The fix: `popup_pause.rs` lists the hidden pages too (`track_page`
from `HiddenPage::open`, `forget_page` when a `HiddenPage` is
dropped) and tells them they are hidden from `on_popup_show(true)`
to the owner's `on_popup_show(false)`, with the other tabs; a page
opened while a widget is open starts paused. What is left visible is
one view and its widget, the arrangement that has always painted.
The rule for the future is in C15: every windowless browser must be
on one of the two lists.

Verified on Windows with a debug build (`--features test-hooks`), a
scratch `--data-dir`, the test extension loaded through
`NAUTILUS_TEST_CONTROL`, and a local page that opens and closes its
own `<select>` every 8 s through a temporary hook (a page's console
line turned into `send_mouse_click_event`, and a log line for each
`PET_POPUP` paint; neither committed). Before: the default
profile's window, a second profile's window, the extension: `popup
widget shown` 4 times, painted 0. With one window and the extension:
painted every time. After: two profiles' windows, painted 4 of 4;
three profiles' windows, 4 of 4; without an extension, 3 of 3; and
an action click sent while a list was open, after three lists had
opened and closed, was taken by the worker after one attempt. Also
`cargo fmt --all --check`, clippy with and without `--all-features`,
`cargo test --workspace` (248 and 3).

Not verified: the owner's own profile with Proton Pass (the test
extension stood in for it); the list by eye (the paint was read from
the log; the composition is unchanged); macOS; the scripted bridge
runs (`run.sh`, `phase5.py`: macOS only), which matter here because
the relay's long poll now sits in a page that is told it is hidden
for as long as a list is open: a `Runtime.evaluate` answered during
that time in the run above, but a list left open for minutes was not
tried; an extension's action popup or DevTools open beside a list
(windowed browsers, on neither list).

## Status and handoff (2026-11-22, `<select>` with two windows: see 2026-11-23)

The owner's `<select>` that does not open (2026-11-19, below) was
reproduced on Windows and has nothing to do with the page: with two
tab views painting at once (two windows, each showing a tab; two
profiles are two windows) Chromium announces a popup widget and never
paints it. C15 in `docs/upstream-issues.md` has what was measured.
The fix is `popup_pause.rs`: a process-wide list of the tab views
(`track` from `TabHost::create_tab` and `adopt_tab`, `forget` when a
view or its browser goes), and from `on_popup_show(true)` to
`on_popup_show(false)` every other painting view is told it is hidden
(`ViewState::paused`, which `wants_hidden` now counts, so
`TabHost::show` and the debugger's rule respect it). The pause has an
owner: only the end of the owner's widget resumes the others, since
pausing a view closes a widget of its own.

Verified: `cargo test --workspace` (three tests of `plan_open`),
clippy with and without `--all-features`, fmt; and a debug build on
Windows, a scratch `--data-dir`, a local page with one `<select>` in
a cross-site frame, real clicks, captured with `PrintWindow`: with one
window the list shows; with a second window open it showed nothing
before the fix (`popup widget shown`, no paint) and shows after, in
either window, a click on an option picks it, and the other window
loads and paints a page afterwards.

Not verified: macOS (expected to be the same cause: nothing in the
path is per platform, and the Mac that reported it had two profiles
open; the external-popup-menu hypothesis below was never tested and
is probably wrong); two profiles rather than two windows of one (the
list is the process's, but only one profile was run); three windows;
a list open while a tab is switched in another window by an
extension; a video playing in the other window while a list is open
(it is expected to hold its frame and keep its sound); the installed
release build. Left: the cause inside Chromium, and whether the same
limit drops frames of a third window's page.

## Status and handoff (2026-11-19, `<select>` on macOS: see 2026-11-22)

The owner reported that an HTML `<select>` does not open in Nautilus
on macOS. It is not fixed; this is what is known, for whoever next
sits at a Mac.

On Windows it works: a debug build on a page with one `<select>`,
clicked once, logs `popup widget shown`, `popup widget at 100,140
sized 300x118`, and the capture shows the list under the control
(`on_popup_show`, `on_popup_size`, `on_paint` with `PET_POPUP`,
composed by `frame.rs`). Resizing the view closes the list, which is
Chromium's doing and correct.

Nothing in that path is per platform in this crate, so the difference
is expected to be Chromium's: on macOS (and Android) Chromium is built
with external popup menus, where a `<select>` asks the browser process
for a native menu instead of drawing a popup widget, and a windowless
view has no native menu to show. CEF is understood to switch that off
per view in the renderer when it knows the browser is windowless; a
view whose renderer never learned that would take the click and open
nothing. That is a hypothesis from reading, not from a run.

To tell, on a Mac: `RUST_LOG=nautilus_engine=debug`, click a
`<select>`, and look for the two `popup widget` lines (added for
this). If they are absent, the popup widget path is never taken and
the hypothesis stands: compare a tab the shell created with one
Chromium created (`window.open`, `on_before_popup`), and a first load
with a page reached after a cross-site navigation, and look in CEF's
own log (`NAUTILUS_CEF_LOG=verbose`) for `NOTIMPLEMENTED` from the
windowless view's popup menu or for "Timeout of new browser info
response". If they are present, the list is shown and painted but not
seen, and the place to look is the popup's rectangle against the view
scale (`on_popup_size` multiplies by it) and `frame.rs`'s composition.

## Status and handoff (2026-11-19, the message ids)

The deferred item of the bridge cleanup (PR #2): can the relay choose
its own DevTools message ids, as the manager and the screenshots do?
Yes. The rule, read in CEF's source at the pinned commit (154.0.34,
`14c5a08`, `libcef/browser/devtools/devtools_controller.cc`,
`CefDevToolsController::ExecuteDevToolsMethod`): a caller's id is kept
when it is not below the controller's `next_message_id_`, and the
counter then moves to one past it; an id below the counter (0 among
them) is replaced by the counter's value. The counter starts at 1 and
there is one controller per browser, made on the first DevTools call
and kept. `SendDevToolsMessage` (the test control's raw `devtools`
message) does not read or move the counter. So an id that increases on
each browser is always kept.

What changed (`bridge/relay.rs`, `bridge/hidden_page.rs`):

- `Bridge::evaluate` takes an id from one counter for every hidden
  page (`Inner::next_message`, from 1), records the pending entry under
  it, then calls. Nothing else sends on a hidden page's browser. An
  unsent call removes the entry. A returned id that differs moves the
  entry and logs `bridge: CEF replaced message id …` (a warning).
- A tab's probe sends `Target.getTargetInfo` as id 1 (`TAB_PROBE_ID`),
  known to its observer before the call. It is the first DevTools call
  on its tab's browser (`track` is called once per browser, at creation
  or at a popup's adoption); the hub's screenshots on the same browser
  start above `CAPTURE_IDS` (1,000,000), and a constant assertion keeps
  the probe's id below that.
- Gone: `Sending`, `reserve`, `settle`, the reservation fallback in
  `page_result`, the `sending` half of `drop_pending`, `RESERVED`,
  `Probe` and its `early` buffer. The manager's and the hub's ids, the
  generation check (`is_current`) and every log line's words are as
  they were.

Verified on the Mac (Apple silicon), exit status in brackets. Before
the change, on `ef87e1f` with a bundle built with `--features
test-hooks`: `run.sh` `spike` [0, 54 PASS], `pump` [0, 7], `reload`
[0, 7], `zombie` [0, 21], `broken` [0, 14], `stop` [0, 17], `idle`
[0, 9]; `phase5.py` [0, 74 PASS, 0 FAIL]. That run is also the first
harness run of PR #2's other commits (one delivery path, the helper's
page in the relays' map, the single platform gate). After the change:
`cargo fmt --all --check` [0]; clippy `-D warnings` with and without
`--all-features` [0, 0]; `cargo test --workspace` [0; 247 and 3
passed] and with `--all-features` [0; 258 and 3]; `node
ci/side-panel/bridge_check.mjs` [0]; `cargo xtask bundle-mac
--features test-hooks` [0]; the same eight runs, each once, with the
same statuses and counts. The warning's text is in the bundled binary
and in none of the launches' logs (`run.sh`'s with `SIDE_PANEL_LOGS`,
`phase5.py`'s `main.log` and `restore.log`), nor is `cannot ask`.

Not verified: Windows (nothing ran there; the code has no platform
branch); an answer that arrives inside the call was not provoked or
counted, so whether CEF ever does that here is still not known (the
entry is recorded first either way); each mode ran once per build, so
a rare race would not show. The local osmena checkout was 42 commits
behind the pinned revision, so the builds used a temporary project
patch table pointing at the `extension-side-panel` worktree (`9bafb0e`,
the pinned revision), not committed.

## Status and handoff (2026-11-19, the test hooks)

From a review of the branch before its merge: the scripted runs' hooks
were compiled into every build, the packaged ones included. They are
now behind the `test-hooks` cargo feature of `nautilus-engine` (passed
through by the `nautilus` binary's feature of the same name): the
command directory (`NAUTILUS_TEST_CONTROL`: `poll_control`, the list
hold, `control_log`, the relay's `inspect_relay`, `log_table` and
`any_tab`, and the directory rule, now in `test_control.rs`), the page
script (`NAUTILUS_TEST_PAGE_SCRIPT`, which was on `main` before the
branch), and the pump's counters, watch thread and interval switch
(`NAUTILUS_PUMP_LOG`, `NAUTILUS_PUMP_TICK_MS`; without the feature
`PumpStats` is empty and the interval is the default). `cargo xtask
bundle-mac --features test-hooks` builds the bundle the harness needs.

Also in CI: `ci/drop-stale-cmake-trees.sh` removes a restored CMake
build tree whose compiler or linker is gone. The Windows check had
failed in `cef-dll-sys`'s build script after a runner image brought a
newer MSVC toolset (14.51 to 14.52): CMake took the new compiler from
the command line and the old linker from the cached tree.

Verified on Windows: `cargo fmt --all --check`; clippy and `cargo test
--workspace` with and without `--all-features` (243 tests without the
feature, 249 with it); a release `nautilus.exe` built without the
feature holds none of `TEST HOOK`, `NAUTILUS_PUMP_LOG`,
`NAUTILUS_PUMP_TICK_MS`, `relay_eval`, `hold_list` or `devtools.log`.
The CMake failure was reproduced with a toolset reached through a
junction that is then removed, and the script cleared it.

Not verified: the scripted runs against a bundle built with the
feature (`run.sh`, `phase5.py`: macOS only, and nothing in them
changed but the build command they name); the cleanup script on a
runner with the newer image (the run after the fix landed on the older
one and dropped nothing); the package job's new check, which runs only
on a push to `main`.

Left for a follow-up, from the same review: `relay.rs` holds six jobs
in one `RefCell` and has almost no tests; DevTools evaluation is
written twice with two conventions for message ids (settled: the
status section of 2026-11-19, the message ids); clicks and events
are one delivery written twice; the helper's page is special-cased
beside the relays' map; three maps hold a tab's browser; the
`cfg(any(windows, target_os = "macos"))` gates are inconsistent
(`engine.rs` and `table.rs` use `bridge::pages` ungated).

## Status and handoff (2026-11-19, the sender's strip)

`phase5.py`'s `sender.tab carries the strip's fields` failed in all five final
passes of review 2 (73 PASS, 1 FAIL each): an extension page's message reached
the worker with `index: -1` and `windowId: -1`. The cause: the synchronous
delivery (`e9531ed`) reads the strip from a per-context cache primed once, from
`tabs.query` at the worker's start, and refreshed only after a message. The
page's tab is opened later, and the check is its first message, so the cache
never held it. The engine already sent the worker `tabs.onCreated` with the
whole tab, under its Chromium id, when it published the tab (before the page
could send; the failing run's log, the page's `table:` line followed by
`tabs.onCreated (delivery 16)`).

What changed: `js/core.js` has `tap(name, fn)`, the bridge's own listener: run
before the extension's, never seen by `hasListener` or `hasListeners`, and
announced with `bridge.listen` as an extension's first listener is. In a worker,
`js/runtime.js` taps `tabs.onCreated` (records the tab), `tabs.onReplaced` (moves
the entry to the new id and fetches its strip) and `tabs.onRemoved` (drops it).
A page taps nothing. The cost: every bridged worker is now sent each tab's
creation, replacement and removal, whether its extension listens for them or not.
`idle` and `stop` pass, so a stopped worker was not started again by them in
those runs. `CHANGELOG.md` and `docs/bridge-protocol.md` say the same.

Test, red first: `ci/side-panel/bridge_check.mjs`, after the primed-tab case
(a tab created after the prime, a replaced tab, a removed tab, the tap unseen by
the extension, a page asking for nothing). Red
(`evidence/phase-5/review2-n7-node-red.txt`, exit 1: the worker did not ask for
`tabs.onCreated`), green after the change (`review2-n7-node-green.txt`, exit 0).

Verified (exit status in brackets; outputs `evidence/phase-5/review2-n7-*`):
`cargo fmt --all --check` [0]; `cargo clippy --workspace --all-targets -- -D
warnings` [0]; `cargo test --workspace` [0; 257 and 2 passed]; `cargo build
--workspace --all-targets` [0]; `node ci/side-panel/bridge_check.mjs` [0];
`cargo xtask bundle-mac` [0]. `phase5.py`: three passes [0, 0, 0; 74 PASS, 0 FAIL
each]. The regression modes: `spike` [0, 54 PASS], `reload` [0, 7], `zombie` [0,
21], `stop` [0, 17], `idle` [0, 9], `broken` [0, 14]; `pump` [1, then 0 and 0].

Open: the first `pump` run failed `pump (d)` with "no page target for the
background tab" (`review2-n7-run-pump.txt`): the worker's `tabs.query` named a
background tab id that no `page` target in `chrome.debugger.getTargets()` had.
Its logs were not kept (`run.sh` removes them unless `SIDE_PANEL_LOGS` is set).
Two reruns with logs kept passed (`review2-n7-run-pump2.txt`,
`review2-n7-run-pump3.txt`, logs beside them); in those the table joined both tabs
in the same second the worker started. The likely reading is a race between the
bridge's `tabs.query` and Chromium's target list in the test worker, which this
change does not touch; it is not shown. Also open from review 1: Windows, the
hand-clicked runs, the keyed migration on the bundle.

## Status and handoff (2026-11-19, review 1 of Phase 5)

The code-review feedback on `fc83621..fcea1f1` (and the two commits after it)
was handled finding by finding: each one reproduced first (a red engine or node
test, a mutation, or a measurement on the bundle), then fixed, then committed.
The commits are `f2e854a`..`31a7dd7`; the docs are `5f071c8` and the status here.

What changed, by area (`bridge/state.rs` unless named):

- **Extensions without a worker** (`extensions.rs`, `set_extensions`): every
  enabled extension is served with its grants and a worker flag; only a worker
  is woken. A page's held call waits only behind earlier held calls of its own
  extension (`settle`), so one stalled extension cannot hold another. The bundle
  answered `chrome.tabs.query` in a probe page of a worker-less extension.
- **Listeners per context** (`page_listeners`, `BridgeCall::page`,
  `PageLane::emit_to`, `Hub::tick`): the worker is sent only the events it
  registered, a page only the events its frame registered, and a frame that is
  gone takes its registrations with it.
- **The bridge's ports** (`js/bridge.js`, `js/relay.js`): a port is adopted by
  its address (the relay page's URL for the worker; a URL inside this extension
  for the relay), not by the lack of a tab. The relay page is hosted in a tab,
  so its port has one. The first version of this check refused a sender tab; it
  was wrong, and it lost the first wake in the scripted run (`31a7dd7` corrects
  it, `review1-failing-run-wake-lines.txt`).
- **Forwards** (`router::FORWARDED`, `js/core.js`): the script takes a forward
  only for the methods the router makes; a test keeps the two lists equal.
- **Tab messages** (`js/runtime.js`): a context whose listeners do not answer
  sends nothing. A trade-off to decide: a message nobody answers now closes with
  Chromium's closed-channel error, not an `undefined` answer.
- **Migration and copies** (`bridge/migrate.rs`, `install.rs` `replace_dir`,
  `sweep_leftovers`): the original of a user folder is switched off while its
  copy is made and removed once the copy is listed; a copy replacement renames
  the old copy aside first, and the sweep removes only this copy's leftovers.
- **Capture** (`router.rs` `capturable`): only `http`, `https`, `ftp`, `data`
  and the extension's own pages; a file page is refused.
- **Bounds** (`js/page_transport.js`, `pages.rs` `refuse`, `hub.rs`
  `CAPTURE_WAIT`, `capture_spent`): a page's call is refused after 10 s; a
  screenshot fails when its browser is gone or after 10 s; DevTools results are
  queued and answered on the next tick, not inside their observer.
- **Fail closed** (`js/page_transport.js` `__nautilusRefuseNative`): a page whose
  bridge cannot be put in refuses Chromium's `tabs` and `windows`.
- **Patterns** (`pattern.rs`): MV3 takes host patterns from `host_permissions`
  only; `<all_urls>` and every pattern skip file pages; hosts match without case.
- **Minor**: the six `allow(dead_code)` on the view and popup items are gated
  (`delegate.rs`); plan and review labels are out of the comments; a page reply
  is checked against the extension that asked (`pages.rs` `deliverable`).

Measured on the bundle (`evidence/phase-5/review1-probe*`): a worker-less
extension's page answers `tabs.query`; its `srcdoc` and `about:blank` iframes
have a `chrome` object and no `chrome.tabs` (`typeof` undefined). The relay's
port sender carries a tab and the worker's carries
`chrome-extension://<id>/background/nautilus-loader.js`
(`review1-relay-sender-measure-run.txt`).

Tests, red first where the defect could be shown: `bridge/state.rs` (the
per-extension hold, the worker-less served extension, the per-context
listeners, the frame-gone case; a mutation routing page listeners to the worker
fails four of them); `bridge/router.rs` (`capture_takes_only_...`, the forward
list); `bridge/install.rs` (the replacement, the sweep, the concurrent load:
the shared-staging mutant fails 6 of 6); `bridge/migrate.rs` (five cases);
`bridge/pattern.rs` (MV3, the case, the file pages); `bridge/pages.rs` (the call
number, the delivery rule); `bridge/hub.rs` (`capture_spent`);
`ci/side-panel/bridge_check.mjs` (the forward, the ports, the silent listener,
the page timeout, the fail-closed paths; the old bridge scripts fail the port
checks). Two existing expectations changed, because they encoded a defect: the
silent-listener assertion (it expected `undefined` to the sender), and the
file-page rows of the pattern table (`<all_urls>` and `file:///tmp/*` now match
no file page). The node harness's page context answers its calls by default now.

Still open: `#[allow(dead_code)]` appears 16 times on this branch, 11 more than
on `origin/main` (in `extensions.rs`, `lib.rs`, `delegate.rs`), which the review
did not name; Windows was not built (only the Apple Silicon target is installed);
the scripted runs were not clicked by hand; the keyed extension's migration
(a user folder with a `key`) was not run on the bundle.

Verified on `31a7dd7` (exit status in brackets): `cargo fmt --all --check` [0];
`cargo clippy --workspace --all-targets -- -D warnings` [0]; `cargo test
--workspace` [0; 251 and 2 passed]; `cargo build --workspace --all-targets` [0];
`node ci/side-panel/bridge_check.mjs` [0]; `cargo xtask bundle-mac` [0].
`phase5.py`: three passes [0, 0, 0; 72 PASS, 0 FAIL each]. The regression
modes: `spike` [0, 54 PASS], `pump` [0, 7], `reload` [0, 7], `zombie` [0, 21],
`stop` [0, 17], `idle` [0, 9], `broken` [0, 14].

## Status and handoff (2026-11-18, the page's report)

An extension page shown as a tab now reports its own Chromium tab id, so the
table has a row for it as it has for a web page, and `sender.tab` carries the
strip's fields. The cause was in the table: the helper lists an extension page's
target as `type other` with no tab id, and `table::join` takes only `page`
targets with one. The report is one process message, `nautilus-bridge-tab`, from
`js/page_report.js` (run in the deferred install after the namespaces replace
Chromium's, reading `chrome.tabs.getCurrent()` from `core.js`'s `native`). The
browser process decides the view: `pages::report_source` accepts it only from the
main frame of a bridged extension's page (`extension_of`) whose browser is a tab
view (`Asker::View`). `pages::report_of` takes only `{"tab": n}`, a positive
integer below `SYNTHETIC_BASE`. `TabTable::report` (`bridge/table.rs`) records
or refuses it: no target yet, not a page of that extension, the view already has
an id (the same is `Same`, another is refused, so a listed row always wins), the
listing names the id for another target, or another tab has it. A recorded
report is a row only while the view is still that extension's page
(`prune_reports`), and `forget` drops it on close. A recorded report calls
`Hub::report_tab` -> `table_changed`, so the strip publishes the id at once; a
tab that was published synthetically takes its id by the existing
`tabs.onReplaced` path. The page's calls wait for the report, bounded by
`REPORT_WAIT_MS` (1 s), in `page_transport.js` through `__nautilusReportWait`.

Tests, each red first: `bridge/pages.rs` `report_tests` (the report's shape and
the source rule; red at `evidence/phase-5/task13f-rust-red.txt`, exit 101, then
green), `bridge/table.rs` `report_tests` (eight cases: naming, the same and a
different id, the listing, the helper's row winning, the view with no target,
navigation and closing; the same red file), and `ci/side-panel/bridge_check.mjs`
(the gate: a call waits for the report and the report comes first; a page that is
no tab releases at once; a failed or silent answer releases at the bound;
`task13f-js-red.txt`, exit 1, then `task13f-js-green.txt`, exit 0).

Measured: the page's first call (its `getCurrent`, timed in `p5page.js`) took
1993 ms and 2017 ms before the report (`task13f-before-p5-run1.out`,
`-run2.out`, the publish wait) and 154, 1, 8, 3, 1 and 5 ms in six final passes
(`task13f-final-p5-run1.out` to `-run6.out`). Six final passes of `phase5.py`,
each exit 0, 72 PASS and 0 FAIL. The `null` `getCurrent` of the earlier runs did
not recur; the page checks passed in all six.

Known limit: a page could report the id of a tab the helper has not listed yet;
the listing wins once it names that id for another target. The reporting is
done by the page's own script, which is extension code.

Verified (exit status in brackets): `cargo fmt --all --check` [0]; `cargo clippy
--workspace --all-targets -- -D warnings` [0]; `cargo test --workspace` [0; 227
and 2 passed]; `cargo build --workspace --all-targets` [0]; `node
ci/side-panel/bridge_check.mjs` [0]; `cargo xtask bundle-mac` [0]. `phase5.py`:
six passes [0 each; 72 PASS, 0 FAIL]. The regression modes: `spike` [0, 54 PASS],
`pump` [0, 7], `reload` [0, 7], `zombie` [0, 21], `stop` [0, 17], `idle` [0, 9],
`broken` [0, 14].

`CHANGELOG.md` is not changed: its first `Added` bullet says nothing this fix makes
untrue, and the brief allows only removing or correcting a clause.

## Status and handoff (2026-11-18, the restored worker's port)

A worker's own port, opened by its first call before any relay listens, is
closed at once by Chromium with "Receiving end does not exist". The bridge took
that close for the port in use closing, cleared the port and rejected the pending
call, so the restore suite's first `tabs.query` failed ("the browser stopped
listening"). Now a call whose port closed with that error is sent again on the
next port, every 100 ms for up to 3 s, then fails with "found no listener in the
browser" (`bridge/js/bridge.js`, `noListener`, `send`, `resend`). A call is posted
on the port in use at the time it is posted, after that port's hello. The trace
and its reading are in `docs/bridge-protocol.md` ("A port with no listener" and
"The restored worker's port"; evidence `evidence/phase-5/task13e-*`, not
committed).

Tests: `ci/side-panel/bridge_check.mjs`, its last block (three cases, the first
two with a worker whose port has no listener): red first (`task13e-port-red.txt`,
exit 1: the call rejected with "the browser stopped listening"), green after the
change (`task13e-port-green.txt`, exit 0). `ci/side-panel/phase5.py`, `done`: the
check accepted a `failed:` record; a test of the helper was red first
(`task13e-done-red.txt`, exit 1) and is green (`task13e-done-green.txt`, exit 0).
Both checks are tightened, not loosened: the main suite's "the suite finished"
has the same `done` and is tightened too.

Superseded by the page's report (the next section): the `sender.tab` item below is
fixed. As it stood before the report: `sender.tab carries the strip's fields`. The
helper lists an extension page's target as `type other` with no tab id, so
`table::join` (which takes only `page` targets with a tab id) never names the page's
Chromium id. Options and what each changes are in `docs/bridge-protocol.md`, "The
sender's tab". Also open: the page's own checks (`the page is bridged before its
scripts run`, `a page's current window`, `a page's tabs.group`) fail in some runs,
because the page's `getCurrent` can answer `null` when no publish is pending
(`state.rs` `page_call`); and `Failed to create API on Chrome object` in the
restore launch, not identified (the bridge's page transport runs in both launches).

Verified (exit status in brackets): `cargo fmt --all --check` [0]; `cargo clippy
--workspace --all-targets -- -D warnings` [0]; `cargo test --workspace` [0; 219
and 2 passed]; `cargo build --workspace --all-targets` [0];
`node ci/side-panel/bridge_check.mjs` [0]; `cargo xtask bundle-mac` [0].
`phase5.py` twice: pass 1 [1, 71 PASS, 1 FAIL], pass 2 [1, 71 PASS, 1 FAIL], the
FAIL being `sender.tab`. The regression modes: `spike` [0, 54 PASS], `pump` [0, 7],
`reload` [0, 7], `zombie` [0, 21], `stop` [0, 17], `idle` [0, 9], `broken` [0, 14].

`CHANGELOG.md` is not changed in this cycle, as the brief asked. The restored
worker's calls are a user-visible fix, so the owner's next pass owes an
`Unreleased` `Fixed` entry.

## Status and handoff (2026-11-18, page-call holds)

A page's call for a tab that the published strip does not hold yet is held until
the strip publishes, or for `PUBLISH_WAIT`; a call of an extension the hub does
not serve yet is held until it is served, or for the same bound, and then refused
with `the extension is not bridged.` (`BridgeState::page_call`, `Hold`, `settle`,
commit `ef21a61`). The page lane no longer drops such a call. Unit tests in
`bridge/state.rs`: the publish release, the bound, arrival order, an unpublished
view, and the unserved extension's release and refusal (each written first; the
red run failed to compile on the missing items, then the tab fixtures were fixed
where a `~` suffix made a tab deferred, which is ready at once).

Verified (exit status in brackets): fmt [0], clippy `-D warnings` [0], `cargo test
--workspace` [0; 219 and 2 passed], build [0], `bridge_check.mjs` [0], bundle [0].
`phase5.py` on the final tree: two passes, each 66 PASS, 3 FAIL (exit 1). An earlier pass of the same code: 64 PASS, 5 FAIL. The
regression modes `spike`, `pump`, `reload`, `zombie`, `stop`, `idle`, `broken` [0
each].

Still open, measured (details in `docs/bridge-protocol.md`, "The scripted run
(phase5.py)"): the page's own tab never gets a Chromium id in the table, so the
sender's `windowId` is `-1`; the restored worker's first call never reaches the
engine after its port closes silently (the restore suite fails); `Failed to create
API on Chrome object` in the restore launch, not identified; and `AC3.3`/`AC3.4`
failed once in two passes (a publish wait that ended before the table named the
tabs).

## Status and handoff (2026-11-18, the extension page's install)

An extension page's bridge was installed when its context was created, where
Chromium has not yet defined `chrome`. `core.js` failed closed, so the page kept
Chromium's own APIs, and a `tabs.group` on the page's own Alloy tab trapped the
browser process (upstream entry C11 in `docs/upstream-issues.md`).

What changed: the transport goes in at context creation, and the namespaces go
in once, at the first change to the page's document (`js/page_defer.js`,
`pages::deferred_namespaces`; the parser delivers that change before the page's
first script runs, and `chrome` exists by then). `pages.rs` has unit tests of the
composition; `ci/side-panel/bridge_check.mjs` checks the deferral: nothing is
replaced before the change, the page's calls are bridged after it, the install
runs once, and a missing `chrome` or document logs one error.

Verified (exit status in brackets): `cargo fmt --all --check` [0],
`cargo clippy --workspace --all-targets -- -D warnings` [0], `cargo test
--workspace` [0; 213 and 2 passed], `cargo build --workspace --all-targets` [0],
`node ci/side-panel/bridge_check.mjs` [0], `cargo xtask bundle-mac` [0];
`ci/side-panel/phase5.py` twice [1 both; 63 PASS, 6 FAIL]; the regression modes
`spike`, `pump`, `reload`, `zombie`, `stop`, `idle`, `broken` [0 each].

Its two open items, a page's first calls before its tab is published and a
restored page's calls with no reply, are handled by the page-call holds (next
section).

## Status and handoff (2026-11-18, review cycle 6 of Phase 1)

The sixth review left I17 (Important) and M16, M23, M24 (Minor). The kept
logs are `evidence/phase-1/review6-*` (not committed); names below are relative
to it. Each run's exit status is in the `.status` file beside its `.out`.

What changed:

- **A worker-target change asks its relay, and the answer decides (I17).**
  `policy::WorkerWatch` (one per extension) records the target and counts the
  changes (`observe`, `is_latest`). `policy::asks_on` says that a change with an
  open relay asks, and `policy::after_ask` decides: a hello keeps the relay; a
  failed answer closes it for the latest change; an answer to an earlier change
  is ignored. The ask is `identify`, sent from `relay.rs` `check_worker_targets`
  (`ask`, `decide_ask`, `Pending::Ask`). Removed: the two-refresh grace
  (`CHANGE_GRACE_REFRESHES`, `policy::after_change`), the hello counting
  (`WorkerWatch::hello`), and their tests (11 superseded, 9 added). The cycle 5
  defect: the new worker's hello came before the change was observed, so it
  did not count, and the live relay was closed after the grace.
- **`identify` rejects in a zombie (found while designing the ask).** The relay
  page answered `identify()` from the hello it cached before a reload. A zombie
  gets no port event, so the cache stayed, and an ask would have kept it. `relay.js`
  `contextAlive` checks `chrome.runtime.id` first. Node check
  `review6-identify-zombie-check.js`: red against the committed `relay.js`
  (`review6-identify-zombie-red.txt`, status 1: the stale hello `B` resolved),
  green on the tree (`review6-identify-zombie-green.txt`, status 0).
- **Driver checks (I17).** `ci/side-panel/driver.py` `change_checks` (spike and
  stop): each worker-target change of the extension was asked, the relay
  answered and was kept, and no close and no reopen follows before the next
  change. `zombie_checks`: the change was asked, the ask failed and the relay was
  closed, and no answer kept it. The change is matched by its line's phrase
  "worker target changed from": the ask's own line also says "worker target
  changed", and the first version of the check counted it as a second change
  (`review6-spike-prefix-driver-run1.out`, status 1, three checks failing, all
  from that; the engine behaved as intended in it, see the runs).
- **M16, the evidence names.** `review6-m16-check_evidence_names.py` (its last run: `review6-m16-check.txt`, status 0) resolves
  every backticked token and every evidence-shaped token in the five documents
  and the acceptance record. Fixed by eye in CLAUDE.md 746, 930, 1070 and
  1147-1168 (the slash forms and the fragments are now full file names, such as
  `task12-tick0-control.out` and `task12-f5-run1.out`), in the acceptance record (the
  task 13, F5 and brace forms, and the run-name fragments, now name full files), and
  in the cycle 5 M16 note (the run named early zombie is `review4-zombie-early.out`).
- **M23.** The cycle 5 closes, from the kept logs: spike run 1 06:21:39 to
  06:21:43 (4 s), spike run 2 06:27:34 to 06:27:37 (3 s), stop run 1 06:41:44 to
  06:41:48 (4 s). "Three to four seconds" is the accurate figure.
- **M24.** The Conventions bullet names `policy::asks_on` and `policy::after_ask`.

Tests and checks on the final tree (exit status in brackets; kept outputs in
`review6-*`):

- `cargo fmt --all --check` [0, `review6-fmt-check.txt.status`]
- `cargo test --workspace` [0, `review6-cargo-test-workspace.txt`: 147 + 2 passed;
  cycle 5 was 149 + 2]
- `cargo clippy --workspace --all-targets -- -D warnings` [0,
  `review6-cargo-clippy-workspace.txt.status`]
- `cargo build --workspace` [0, `review6-cargo-build-workspace.txt.status`]
- `cargo xtask bundle-mac` [0, `review6-bundle-mac.txt.status`]
- `sh -n ci/side-panel/run.sh` [0, `review6-sh-n.txt.status`]; the driver's AST
  and compile check [ok, `review6-driver-compile.txt`; compiled without writing
  bytecode, no `__pycache__`]; `node --check` on `bridge.js` and `relay.js` [0,
  `review6-node-check.txt`].
- The red step of the policy tests: `review6-i17-policy-tests-red.txt` [101]:
  nine ask tests failed against stub functions (their names are in the output). Green: `review6-i17-policy-tests-green.txt` [0].

Runs (final build; each launch's status in its `.status`; logs in the
`review6-*-logs/` directories (each keeps the driver's log, the launches' logs and
a results file, for example `review6-pump-run1-logs/results.jsonl`):

| Run (`run.sh`) | Status | Checks | Kept files (`evidence/phase-1/`) |
|---|---|---|---|
| `spike`, 1 | 0 | 54 PASS, 0 FAIL | `review6-spike-run1.out`, `review6-spike-run1-logs/` |
| `spike`, 2 | 0 | 54 PASS, 0 FAIL | `review6-spike-run2.out`, `review6-spike-run2-logs/` |
| `zombie`, 1 | 0 | 21 PASS, 0 FAIL | `review6-zombie-run1.out`, `review6-zombie-run1-logs/` |
| `zombie`, 2 | 0 | 21 PASS, 0 FAIL | `review6-zombie-run2.out`, `review6-zombie-run2-logs/` |
| `pump`, 1 | 0 | 7 PASS, 0 FAIL | `review6-pump-run1.out`, `review6-pump-run1-logs/` |
| `reload`, 1 | 0 | 7 PASS, 0 FAIL | `review6-reload-run1.out`, `review6-reload-run1-logs/` |
| `stop`, 1 | 0 | 14 PASS, 0 FAIL | `review6-stop-run1.out`, `review6-stop-run1-logs/` |
| `idle`, 1 | 0 | 9 PASS, 0 FAIL | `review6-idle-run1.out`, `review6-idle-run1-logs/` |
| `broken`, 1 | 0 | 14 PASS, 0 FAIL | `review6-broken-run1.out`, `review6-broken-run1-logs/` |

What the runs show:

- **Restart, kept (I17), spike 1 and 2, stop 1.** One change each, the
  restart after the stop: `review6-spike-run1-logs/run-2.log` lines 94 to 100
  (the new worker's hello at 15:15:56, line 94, before the change, line 97; the
  ask, line 98; "the relay is kept", line 100). Spike 2 has the same lines at
  15:21:53 (`review6-spike-run2-logs/run-2.log`), and stop 1 at 15:34:18
  (`review6-stop-run1-logs/run-1.log`, lines 133 to 139). No close and no reopen
  follows in any of the three (the driver's I17 checks).
- **Zombie (ask fails), zombie 1 and 2.** The change was asked and the relay
  "could not say which worker it reaches", and it was closed and opened again
  within the same second (`review6-zombie-run1-logs/run-1.log` lines 88 to 102,
  15:13:10; `review6-zombie-run2-logs/run-1.log` lines 86 to 100, 15:26:53). The
  reopened relay's identify (a debugger needed it) heard the hello, which
  forgot the tab; "hidden again" followed. The driver's path line is `forgotten
  by hello`.
- **Pump (I7, latency).** The tab was shown at attach ("was hidden; shown for
  the debugger of", `review6-pump-run1-logs/run-1.log` line 79) and hidden again
  after the detach (line 83). From `review6-pump-run1-logs/results.jsonl`,
  `Input.dispatchMouseEvent`: 40 calls, maximum 34 ms, none failed;
  `Page.captureScreenshot`: 40 calls, maximum 90 ms; no call took a second or more.
- **Reload.** The list showed the extension off during the reload, so the path
  is `sync` ("forgotten by sync"), and the tab was hidden again.
- **Idle.** No change and no ask after the stop, no restart in 90 s (I15).
- **Broken.** Fourteen checks pass, including the give-up and the refused click.

Not verified in this cycle:

- A live relay closed by a wrong `identify` failure (a worker that does not say
  hello within 5 s of the ask). The case is stated as the one wrong-close case and
  it is not provoked. The ask is not retried.
- An ask on a relay whose page is still loading (its probe not passed). The
  expected result is a failed ask and a close, with nothing in flight, but it was
  not provoked.
- A call in flight across a restart window. The stop run's quiet click is not
  timed against the change, so no run puts a `tabs.query` inside the window. Not
  deterministic in this harness, and not faked.
- A stop and restart repeated in a loop; a reload whose worker target id does not
  change; Windows (nothing ran there).
- The owner's toolbar click (Task 13 step 5).
- "hidden again" is logged twice in the same second for the same tab in spike run 1
  (`review6-spike-run1-logs/run-2.log` lines 70 and 80; spike run 2 was not checked
  for it). These are two real show-and-hide cycles, not a repeated log line: the test
  worker attaches and detaches the tab twice (calls 2 and 3, then 6 and 7 by
  `targetId`), and each pair shows the tab (lines 67 and 77) and hides it again. The
  seventh review read the log and found this; the tab state is right.

## Status and handoff (2026-11-18, review cycle 5 of Phase 1)

_Superseded in review cycle 6: the grace and `policy::after_change` are gone, and a restart's relay is kept by its ask (see review cycle 6 above). The cycle 5 runs and their figures stay as they were measured._

The fifth review left I16 (Important) and M16, M20, M21, M22 (Minor). The kept
logs are `evidence/phase-1/review5-*` (not committed); names below are relative
to it, and each run's exit status is in its `.status` file.

What changed:

- **A worker-target change waits for a hello received after it (I16).**
  `policy::WorkerWatch` (one per extension) replaces the three maps
  `worker_targets`, `changed_waiting` and `new_instance_heard` in `relay.rs`. A
  hello counts only while a change waits for it, and a change clears any hello
  that came before it was observed. The defect: a restart (A absent, B's hello
  through R), then B's `chrome.runtime.reload()` in the same window (C), kept
  the zombie R, because B's hello was consumed by the A-to-C change. The
  sequence is a test, `policy::tests::a_restart_then_a_reload_in_one_window_closes_the_zombie_relay`,
  with its neighbours (two reloads in a row, a hello on a freshly reopened
  relay, a same-instance hello, a restart whose hello comes after the change).
  Red first (`review5-i16-policy-tests-red.txt`, status 101): with the cycle 4 semantics, `a_restart_whose_hello_came_before_the_change_is_observed_is_not_kept_by_that_hello`,
  `a_restart_then_a_reload_in_one_window_closes_the_zombie_relay` and
  `two_reloads_in_a_row_keep_nothing_and_each_change_waits_for_its_own_hello`
  failed with `Keep` where `Wait` is required. Green after the change (149
  engine tests).
- **The cost, stated plainly.** A restart whose hello came before the refresh
  that sees the new target is closed after the two-refresh grace and opened
  again: one reopen per such restart. A wrongly closed live relay costs one
  reopen; a wrongly kept zombie is the defect. The alternative (ask the relay at
  the change which instance it reaches) was not built: it is a new mechanism.
- **"Hidden again" on every forget (M20, `relay.rs` `hide_again`).** `undebug`
  and `forget_debuggers` log `bridge: TabId(n) (Chromium tab m) hidden again: no
  debugger is left on it` when `set_tab_debugged` hid the tab, as the detach path
  does. The driver's zombie check requires that line (it was `hidden or
  forgot`, which was always true); the reload and stop checks judge "hidden
  again" by that line too, not by "has no debugger left".
- **The close's log line (wording only).** The line logged when a relay is closed
  for want of a hello after a change now says that the relay is closed and opened
  again, and names the zombie and the live-but-early case. The first version said
  "is a zombie", which the spike's live close contradicted; it is replaced, and
  the final runs use the new wording.
- **The hello's whole chain is bounded (M22, `bridge.js` `sessionStates`).**
  `getTargets` is inside the 2 s race, so `ready` settles within 2 s; on timeout
  the hello names no session. Checked in node with a fake `chrome`
  (`review5-m22-hello-check.js` against `review5-m22-hello-committed.txt` and
  `review5-m22-hello-fixed.txt`): the committed file never sent the hello when
  the list did not settle (no hello in 6 s), and sent it at 3.0 s when the list
  was slow; the fix sends it within 2.0 s in both cases, and every other case
  gets the same answer as before.
- **The old-shape hello (M21).** `table::hello_of` reads `attached` instead of
  `sessions` as a hello of its instance with no sessions. The doc said "no
  hello"; it now says what the code does, and the table test pins it.
- **M16.** The run named early zombie in this handoff is `review4-zombie-early.out` (the run's output file).
  The evidence names in the docs were checked mechanically
  (`review5-m16-check_evidence_names.py`, output in the acceptance record): 30 bare stems
  that name a run's group were rewritten to the run's `.out` file, two `/`
  shorthands in the acceptance record were corrected (the "pump x2" of cycle 3
  is one kept run), and `review5-*` names are kept by this cycle.

Tests and checks on the final tree (exit status in brackets; `review5-*` logs
where kept):

- Kept gate outputs: `cargo fmt --all --check` [0, `review5-fmt2.status`];
  `cargo clippy --workspace --all-targets -- -D warnings` [0,
  `review5-clippy2.status`]; `cargo test --workspace` [0, `review5-test2.txt`:
  149 + 2 passed; the cycle 4 count was 140 + 2, and the nine new tests are the
  sequence tests]; `cargo build --workspace` [0, `review5-build2.status`];
  `cargo xtask bundle-mac` [0, `review5-bundle2.status`].
- Claimed in the session, not kept: `sh -n ci/side-panel/run.sh` [0]; the
  driver's AST and compile check [ok, no `__pycache__` left in the worktree];
  `node --check` on `bridge.js` and `relay.js` [0]. The M22 hello check itself is
  kept (`review5-m22-hello-*`).

Runs (final build; each launch's exit status is in its `.status` file):

| Run (`run.sh`) | Status (kept `.status`) | Checks | Kept files (`evidence/phase-1/`) |
|---|---|---|---|
| `spike`, 1 | 0 (`review5-spike-run1.out.status`) | 50 PASS, 0 FAIL | `review5-spike-run1.out`, `review5-spike-run1-logs/` |
| `spike`, 2 | 0 (`review5-spike-run2.out.status`) | 50 PASS, 0 FAIL | `review5-spike-run2.out`, `review5-spike-run2-logs/` |
| `pump`, 1 | 0 (`review5-pump-run1.out.status`) | 7 PASS, 0 FAIL | `review5-pump-run1.out`, `review5-pump-run1-logs/` |
| `zombie`, 1 | 0 (`review5-zombie-run1.out.status`) | 18 PASS, 0 FAIL | `review5-zombie-run1.out`, `review5-zombie-run1-logs/` |
| `zombie`, 2 | 0 (`review5-zombie-run2.out.status`) | 18 PASS, 0 FAIL | `review5-zombie-run2.out`, `review5-zombie-run2-logs/` |
| `reload`, 1 | 0 (`review5-reload-run1.out.status`) | 7 PASS, 0 FAIL | `review5-reload-run1.out`, `review5-reload-run1-logs/` |
| `stop`, 1 | 0 (`review5-stop-run1.out.status`) | 10 PASS, 0 FAIL | `review5-stop-run1.out`, `review5-stop-run1-logs/` |
| `broken`, 1 | 0 (`review5-broken-run1.out.status`) | 14 PASS, 0 FAIL | `review5-broken-run1.out`, `review5-broken-run1-logs/` |

What the runs show:

- Pump (I7): the debugged tab was logged hidden before the attach, shown at it
  ("was hidden; shown for the debugger of"), and "hidden again" after the detach
  (`review5-pump-run1.out`, the I7 checks). `Input.dispatchMouseEvent`, from the
  pump's results file (`review5-pump-run1-logs/results.jsonl`, the `ms` field of
  each `call` record, 40 calls, all `ok`): maximum 9 ms. `Page.captureScreenshot`
  maximum 86 ms in the same file. No call took a second or more.
- Zombie, both runs: the detector saw the reload ("worker target changed"), no
  new instance said hello through the relay within two refreshes, and the relay
  was closed four seconds after the change (run 1: 06:35:24 to 06:35:28; run 2:
  06:37:09 to 06:37:13), reopened, its hello forgot the tab, and "hidden again"
  was logged. The driver's path line is `forgotten by hello` in both. In the
  zombie page `chrome.runtime.id` is `undefined` and `getManifest()` throws
  `Extension context invalidated.` (the INFO `relay_eval` line).
- Restart, in spike 1 and 2 and stop 1: each restart whose hello came before the
  refresh that saw the new target was closed after the grace and reopened, once
  (one "worker target changed" and one "is closed and opened again" line per
  run, three to four seconds apart; the spike's restart and its checks still passed, 50
  of 50). This is the cost stated above, measured.
- Reload 1: the list showed the extension off during the reload, so the path is
  `sync` ("forgotten by sync" in the driver's INFO line), and the tab was hidden
  again (`reload (I10): the session ended`).
- Stop 1: the stopped worker was not identified or restarted for 90 s; the click
  restarted it, the change was closed after the grace, and the restarted
  worker's session survived, so the tab stayed painted.
- Broken 1: five openings, the give-up logged once, "debuggers are kept", a click
  refused, no opening for 12 s, the debugged tab not hidden.

Not verified in this cycle:

- I16 was not provoked in the harness. The test extension has no control that
  reloads on a click (its only reloads are `provokeReload`, after an attach,
  and the stop mode restarts without one). The I16 case rests on the sequence
  tests, not on a run.
- Windows (nothing ran). Whether `Runtime.evaluate` answers on a paused or a
  hidden page. A stop and restart repeated in a loop.
- A restart whose hello comes after the change was observed (the case that keeps
  the relay with no reopen): no final run had one, so it rests on the sequence
  tests (`policy::tests::a_restart_whose_hello_comes_after_the_change_is_observed_is_kept`).
- The pump's `Input.dispatchMouseEvent` figure is measured per method this cycle
  (9 ms maximum over 40 calls). Cycle 4's 72 ms was the maximum over all methods.

For the next cycle: the grace stays two refreshes. If the restart reopen is
judged too costly, the next step is to ask the relay which instance it reaches
at the change (a new mechanism; the sequence tests already describe the cases it
must separate).

## Status and handoff (2026-11-18, review cycle 4 of Phase 1)

The fourth review left I10 (the zombie reload), I12 to I15 and M16 to M19. The
kept logs are `evidence/phase-1/review4-*` (not committed); names below are
relative to it. Exit statuses are in the matching `.status` files, where one
was kept.

What changed:

- **The debugger rule (policy.rs).** Uncertainty never forgets. `policy::Evidence`
  and `policy::forgets` are the one gate: a hello that names a tab
  `not-attached` (`tabs_to_forget`) and `sync`'s extension-gone are the only
  evidence-based forgets. A timeout, an unknown probe, a failed evaluation, an
  unreachable worker, a closed or given-up relay keep the debugger
  (`Evidence::Nothing`). `docs/bridge-protocol.md` states the rule.
- **The hello has three states (I13).** `bridge.js` probes each tab the target
  list names with a 1 s timeout; `not-attached` is only Chromium's "Debugger is
  not attached" or "No tab with id"; everything else is `unknown`. The hello is
  bounded at 2 s, `ready` never rejects, and a call waits at most that long.
  `table::hello_of` returns `Hello {instance, sessions}`; tests cover the three
  states. Whether `Runtime.evaluate` answers on a paused or hidden page was not
  verified; the timeout makes it harmless.
- **Give-up keeps debuggers (I14).** `Bridge::give_up` no longer forgets; the
  wording is corrected in code, docs and CHANGELOG. The `broken` mode attaches
  first and checks the tab stays shown (`review4-broken-run1.out`).
- **No identify on `gone` (I15).** `gone` asks nothing. `identify` happens only
  after a relay passes its probe with a debugger held, and logs its trigger
  ("asks its relay which worker it reaches"). Before, `gone` restarted the
  worker every 30 s. In `review4-idle-before-logs/run-1.log` each `port closed`
  line is followed by `asks its relay which worker it reaches` and then
  `runs a new worker instance`, four times in 90 s; `review4-idle-before.out`
  FAILs its stop check because the worker was listed again within 30 s.
- **The worker target (I10 a).** Each refresh (every 2 s while a relay is open,
  `REFRESH_INTERVAL`) reads the helper's target list. The worker is listed as
  type `worker` (the probe's shape; the brief's `service_worker` was wrong for
  this list, and the first version of the match missed it: `review4-zombie-early.out`).
  `policy::worker_target`: a different target is a change. A change keeps the
  relay if a new worker instance says hello through it (`policy::after_change`,
  `Keep`); otherwise, after `CHANGE_GRACE_REFRESHES` (2) refreshes, the relay is
  a zombie and is closed. `alive()` is removed.
- **A restart changes the target, so the relay is kept.** A worker stopped with
  `stopAllWorkers` and started again by a delivery comes back under a new target
  id (`review4-stop-run1.out`, `review4-spike-run2.out`). The first version
  closed the relay on every change, which failed a `tabs.query` in flight on the
  connected port (`spike` run 1: "the browser stopped listening"; its output was
  overwritten by the rerun, so this is claimed in the session, not kept). The
  grace fixes it: the restart's hello comes through the connected relay before
  the grace ends, so nothing is closed. The loop is bounded: a change starts no
  worker (the relay that the hello reaches is already connected), an idle stop is
  not a change (`WorkerTarget::Absent`), and each change closes at most one relay.
  Not measured: a repeated stop-and-restart loop (the runs had one restart).
- **Delivery across a change.** A kept relay interrupts nothing. A zombie relay's
  pending evaluations are dropped: a delivery not yet acknowledged is logged as
  unknown and never retried (at most once, `after_delivery`); a queued click waits
  for the new relay and is delivered once (`job_turn`); an old instance's tab list
  or debugger report is dropped or applied unanswered (`job_kept`, `reply_due`).
- **The zombie is reached on purpose (I10 b).** `hold_list N` (the extension list
  reads are skipped for N seconds, then read again) and `relay_eval` (an
  expression in a relay page, logged as INFO) are test controls. The driver's
  `zombie` mode holds the list before the worker's reload; it waits for the engine
  to log the hold first (an earlier run had the list read before the hold and
  proved nothing: `zombie` now checks that no sync line precedes the hold). The
  zombie run requires the detector path (`zombie_checks`).
- **M16, M19 (schedule).** The gap before the second opening is 250 ms, then
  500, 1000 and 2000 ms (`policy::gap_after`); the minimum 250 ms also holds
  after a success. `MAX_GAP` is gone. Failed openings and relays that passed
  their probe but could not reach the worker (`Opener::unreached`) count together.
- **M17.** `check()` prints detail only on FAIL; the reload check prints the path
  that forgot the debugger (`classify_forget`: sync, hello, worker target).
- **M18.** A queued debugger report of an old instance is applied and its answer
  is not sent (`reply_due`); a hello's session for a tab the table cannot name yet
  is kept pending and applied at the next join (`apply_pending`).

What the runs show (final build = the grace tree; all exit 0):

- `spike`, twice (`review4-spike-run1.out`, `review4-spike-run2.out`): 50 PASS each; unfocused
  202 calls, max 67 ms, p95 64 ms in run 2.
- `pump`, twice (`review4-pump-run1.out`, `review4-pump-run2.out`): 7 PASS each; 202 calls, max 72 ms
  in both, p95 63 ms; the debugged tab logged hidden before the attach, shown at
  it, and hidden again after the detach. Max is the slowest call of all kinds,
  so `Input.dispatchMouseEvent` is at most 72 ms.
- `reload` (`review4-reload-run1.out`): path = sync.
- `zombie`, three times (`review4-zombie-run1.out`, `review4-zombie-run2.out`, `review4-zombie-run3.out`): the detector
  saw the reload, the relay reopened, the hello forgot the tab, the tab is hidden
  again; path = hello. `relay_eval`: `chrome.runtime.id` is `undefined` in the
  zombie page; `getManifest()` throws `Extension context invalidated.`.
- `stop` (`review4-stop-run1.out`): no identify and no restart for 90 s after
  `stopAllWorkers`; the click restart's check ran; the session survived; the tab
  stayed painted.
- `idle` (`review4-idle-run1.out`): Chromium did not idle-stop the worker in 120 s
  with the debugger held; the mode then stopped it through DevTools (the INFO line
  says so); no identify and no restart in the 90 s after; the tab stayed shown.
- `broken` (`review4-broken-run1.out`): five openings, one give-up, "debuggers are
  kept", the debugged tab not hidden.
- Before-fix (pre-grace tree, `review4-zombie-before.out`, `review4-idle-before.out`): the
  detector's close disabled, and identify on `gone` restored. The zombie is not
  reached: the tab stays painted (the reload check FAILs, the zombie checks fail
  at the relay reopen). The idle run shows the 30 s loop above.
- Earlier builds, kept: `review4-zombie-early.out` (before the worker-type fix: the
  detector never matched), `review4-zombie-pre-grace-run1.out` (passed on the tree
  before the grace), `review4-idle-pre-grace-run1.out` and `review4-idle-early.out`
  (the idle mode before its DevTools fallback, the list hold not yet fixed).

Not verified in this cycle:

- Windows (nothing ran). Whether `Runtime.evaluate` answers on a paused or a hidden
  page. Whether Chromium idle-stops an extension worker with a debugger held (it
  did not in 120 s here; a worker started by the engine did stop every 30 s before
  the change). A stop-and-restart repeated in a loop. A reload that leaves the
  worker target id unchanged (no run had one).
- The pump's `Input.dispatchMouseEvent` latency is read off the run's maximum call
  (72 ms); the per-call figure for that method was not extracted.
- The first spike run's failure output was overwritten by its rerun: the failure
  is claimed in the session, not kept.

For the next cycle: the grace is two refreshes (about 4 s). A zombie is closed in
about 4 s after its reload, not at once; a shorter grace would be a faster zombie
at the cost of closing a relay whose hello is merely late.

## Status and handoff (2026-11-18, review cycle 3 of Phase 1)

The third review left five issues (I10, I11, M13 to M15). Commits
`5e305f7` (engine and worker: I10, I11) and `57cdc1f` (harness: I10, I11,
M14, M15) are on `extension-side-panel`; this part's commit carries the
docs. The kept logs are `evidence/phase-1/review3-*` (not committed); the
names below are relative to it. Exit statuses are in the matching
`*.status` files.

What changed:

- **Identity and sessions (I10).** `bridge.js` gives each worker start an
  instance id and sends a `hello` on every port: the instance and the tab
  ids whose debugger session still answers `sendCommand` (`getTargets`
  alone marks a tab attached for any client, the engine's probe too). Each
  call and reply carries the instance; a reply for another instance is
  ignored by the bridge. `relay.rs` `worker_hello` drops the old instance's
  queued tab lists and debugger reports, and forgets the tracked debuggers
  whose session no longer answers (`policy::tabs_to_forget`,
  `policy::identity`, `policy::job_belongs`, `table::hello_of`,
  `debugger::Debuggers::tabs_of`). The engine asks (`identify`) when a
  relay's port reports `gone` or the relay opens again, and only while the
  extension holds a debugger; `alive` is checked every 2 s for a relay that
  holds one (`check_alive`, `policy::check_due`).
- **Openings (I11).** `policy::next_opening`: five openings in a row
  without a successful probe, the gap 250 ms doubling to 8 s; then
  `Bridge::give_up` (one warning; debuggers forgotten; queued work dropped;
  clicks refused; the helper's queued tab lists answered with an error).
  `sync` restarts an extension's count when its wanted state changes. The
  count is cleared by the page's probe (`PROBE`), not its poll: a poll only
  answers when the worker calls, so an idle healthy relay never reports it.
- **M13.** The README no longer names `targetId` as unreported. The
  protocol doc's limits and the CHANGELOG were checked against the code.
- **M14.** The hang FAIL prints the allowed 120 s. The marker also matches
  "already mutably borrowed".
- **M15.** The watchdog checks `launched_still_up` (pid alive, not a
  zombie, command line starts with the browser's path) before a kill, and
  writes the hang marker only when the kill succeeds.

What the runs show (final build, `review3-final-*` and the sweeps below):

- `reload` (I10): the new worker's `sendCommand` failed (session ended) and
  the engine forgot the tab (`has no debugger left`); `review3-final-reload.out`
  exited 0. Seven reload runs with the `alive` check (`review3-reload-after-attempt3.out`
  to `review3-reload-after-attempt7.out`, `review3-final-reload.out`, and `review3-reload-final-extra-try.out`) all
  took the `sync` path: the list showed the extension off during the reload.
- **Not shown fixed: the zombie path.** On the build before `alive`, a
  reload that left the relay page open with no port event left the tab
  painted with its session gone (`review3-reload-after-attempt2.out`, status 1,
  no `has no debugger left` line; the same shape as the first attempt
  before any fix, `review3-reload-before-attempt1.out`). `alive` was added for
  it; the failure path it takes was run under a forced failure
  (`review3-control-alive-forced-fail.out`, status 1 by design: the stop check
  expects the session, which the forced forget does not keep). No run on
  the final build reached the zombie path, so whether `alive` fails on a
  real reload is not shown. Treat I10's reload case as open until a run
  shows it.
- `stop` (I10, identity): the session survived a `ServiceWorker.stopAllWorkers`
  stop; the restarted worker's `sendCommand` answered and the hello listed
  the tab (`review3-final-stop.out`, status 0; `review3-reload-stop-probe.out`).
  The engine kept the tab painted.
- `broken` (I11): five openings of the damaged relay, one give-up line,
  nothing for 12 s, a click refused (`review3-final-broken.out`, status 0). Before
  the change the same provocation opened it 229 times in 60 s, no give-up
  (`review3-broken-before.out`, status 1 as expected).
- `spike`, twice (`review3-final-spike1.out`, `review3-final-spike2.out`, status 0 each): 50 PASS,
  0 FAIL each; unfocused 202 calls, max 72 and 69 ms, p95 63 ms, no timeout.
- `pump`, twice (`review3-final-pump1.out`, `review3-final-pump2.out`, status 0 each): 7 PASS,
  0 FAIL each; 202 calls, max 78 and 86 ms, p95 63 to 64 ms, no timeout.
- Stub runs (`review3-stub-*`): a crash (status 3 → FAIL exit), a log with
  `already mutably borrowed` (FAIL on the marker), a healthy exit (PASS).
  The hang: the new script's FAIL reads "still up 120 s"
  (`review3-stub-hang-new.out`), the HEAD script's reads "still up 125 s"
  (`review3-stub-hang-old.out`); both killed the stub. A browser that exits half a
  second before the grace is not reported hung (`review3-stub-early.out`,
  status 0, no marker). A browser still up at the exact grace is killed and
  reported (`review3-stub-boundary-1.out`, status 1; it was up at the check).
  The window in which the HEAD script misreported a browser that exits
  within milliseconds of the grace was not reproduced.
- Checks on the final tree: `cargo fmt --all --check`, `cargo clippy
  --workspace --all-targets -- -D warnings`, `cargo test --workspace` (132
  passed in `nautilus-engine`, 2 in xtask), `cargo build --workspace`,
  `cargo xtask bundle-mac`, `sh -n`, `python3` ast parse of `driver.py` (no
  `__pycache__`), `node --check` on the four JS files
  (`review3-final-*.log`).

Not verified in this cycle:

- The zombie reload path (above). The Chromium behaviour of `alive()` in an
  invalidated context is not known; `alive` checks `chrome.runtime.id` and
  `getManifest` (a guess).
- I10's queued attach applied to a new worker: not provoked. The table had
  every tab in every run, so no debugger report was queued. `job_belongs` is
  unit-tested only.
- Windows (nothing ran), the Intel build, macOS 26, and the toolbar's own
  click (Task 13 step 5, not done).
- The sweep does not include a worker stopped while a debugger sits on a
  hidden tab (the tab is shown while debugged, so none was seen).

For the next cycle: the zombie path needs a run in which the list does not
show the extension off during a reload, or a different trigger; without it,
I10 stays open for the reload case.

## Status and handoff (2026-11-18, review cycle 2 of Phase 1)

The second review of Phase 1 left ten issues open: I6 to I9, M3, M7, M8,
and M10 to M12. All ten are addressed. The commits after `8cbeafe` name
the issue numbers (`git log 8cbeafe..HEAD`). The kept logs are in
`evidence/phase-1/` as `review2-*` (not committed); file names below are
relative to it.

What changed:

- **The harness judges each launch (I6).** `ci/side-panel/run.sh` fails
  a run when a browser launch exits non-zero or by a signal; when a
  launch is still up 120 s after its `--exit-after` (a watchdog kills it
  and marks the launch `hung`: a browser that ends by itself is never
  killed); or when its log has `panicked at`, `fatal runtime error`,
  `already borrowed` or `browser(s) still open at shutdown`. A failed
  launch ends the driver and skips the next launch. The stub trees (a
  crash, a logged panic, a healthy exit, a hang) are `review2-stub-*`
  and `review2-stub-statuses.txt`: the old script exited 0 for the crash
  stub (`review2-stub-old-crash.out`); the new script exits 1 for the
  crash, the panic and the hang (its FAIL line names the kill), and 0 for
  the healthy stub. `337af91`.
- **The debugged tab is the background one (I7).** The latency run
  debugs the tab the window is not showing (`tabs.query`), not the first
  page target. The engine logs `was hidden; shown for the debugger of …`
  when a debugged tab that was behind another is shown and `hidden
  again: no debugger is left on it` when its last debugger leaves, both
  with the Chromium id; the table logs its rows (`TabId(n) is Chromium
  tab m`) when they change. The driver requires, in the latency launch's
  log, the tab logged hidden before the attach, shown at it (the launch's
  last shown line is the latency run's own attach), and hidden again
  after the detach. `fd58965`, `53134d2`.
- **Debugger reports by target id (I8).** `bridge.js` reports
  `attach({targetId})` and `detach({targetId})`, and `onDetach` reports
  its source as given. The engine maps a debuggee through the table
  (`table::TabTable::resolve`, tested for a Nautilus tab, a target that
  is not one, and a tab the table has not joined yet). The worker attaches
  and detaches the first tab by target id and the driver checks both
  calls. `docs/bridge-protocol.md` "Limits in protocol 1" names the two
  unreported cases (extension pages; unbridged extensions). `699c5d4`.
- **Page loss and queueing are pure and tested (I9).**
  `policy::failed_by_loss`, `overflow`, `is_generation` and
  `job_turn(page, wanted)` decide which jobs fail when a page is gone,
  what the bounded queue drops, which page a callback belongs to, and
  when a job waits. A debugger report is never failed by a page's loss
  (it waits for the next table). A full queue of debugger reports applies
  its oldest against the table instead of dropping it. `Bridge::trim`
  keeps the queue within 256 after every push and every requeue. The
  debuggers of a relay that is gone are kept (only `sync`, when the
  extension is no longer wanted, forgets them): its worker may still be
  debugging, and forgetting them would show a debugged tab as hidden.
  `699c5d4`.
- **Relays are opened on the engine's turn (M10).** `sync` records what
  is wanted and closes what is gone; `Bridge::reopen`, called from `spin`
  by `ExtensionManager::poll_bridge` on every turn, opens the helper page
  and each missing wanted relay, at most once per 250 ms (`REOPEN_GAP`,
  which bounds a relay that dies at once). A relay whose evaluation
  fails, a retry, and an abandoned delivery close the relay and leave
  the reopening to the next turn. A click for a wanted relay that is
  between pages waits for it (`job_turn`). `699c5d4`, `b9d7911`.
- **A page whose DevTools session ends is gone (M10; found by the
  harness).** The relay-recovery check (below) closed a relay with
  `Target.closeTarget`. The old `page_detached` polled the relay again
  from inside its callback. CEF reported the session end to that call at
  once, and the loop ran until the browser had logged 22,561,827 lines in
  about two minutes (counted in the session before the 3.9 GB file was cut
  to its head; `review2-hung-loop-engine-launch2.head.log` keeps its first
  300 lines, which show the loop and CEF's reentrancy check); the browser
  did not exit and the watchdog ended it.
  `page_detached` now closes and removes the page as `page_gone` does,
  and the next turn reopens it. The loop was in the code before this
  cycle (the old `page_detached` polled too); no earlier run had closed
  a relay. `3683955`.
- **Pending evaluations are reserved before CEF is called (M11).**
  `evaluate` reserves its pending entry (`Sending`, `reserve`) before
  `execute_dev_tools_method` and settles it after (`settle`); an answer
  CEF gives inside the call is matched to the reservation in `page_result`.
  `refresh` sets `refreshing` before its call and clears it if the call
  fails. No borrow is held across the call. `699c5d4`.
- **Windows control directory (M3).** On Windows `control_refusal`
  canonicalises the directory and requires it to be inside the canonical
  `%USERPROFILE%` (`outside_profile`: a pure containment rule over path
  components, case-insensitive on Windows, tested on every platform). A
  directory that cannot be canonicalised, or no profile folder, is
  refused with a warning and the hook off; any platform without a rule
  refuses. Documented in the README and here. `2fe2469`.
- **The test extension probes its wrapper (M8).** `sw.js` reports
  `hasListeners()` before it adds a listener, with a spy added (both
  `hasListeners()` and `hasListener(spy)`) and after the spy is removed;
  the driver asserts false, true and true, then false. The U3 spy (the
  extension's own `onConnect` listener that must never see the port) is
  added after the probe, so it is still present for the U3 check.
  `fd58965`.
- **A skipped link and a missing worker entry (M12).** `install::copy_dir`
  logs each skipped symbolic link at warning level with its path.
  `install::entry_in_copy` makes the load fail with a message when the
  loader's entry is missing from the copy, or was one of the skipped
  links (tested for present, skipped and absent). A failed load leaves no
  copy in the store. `0add58b`.
- **Driver output.** Check details print only on failure (`1061e78`,
  `acd7415`); the relay check finds the relay by its URL, because DevTools
  lists a relay as type `other` (`812a0e7`).

Verification, final build (exit statuses are in `review2-*.status`):

- `cargo fmt --all --check`: exit 0. `cargo clippy --workspace
  --all-targets -- -D warnings`: exit 0. `cargo build --workspace`: exit 0.
  `cargo xtask bundle-mac`: exit 0. `sh -n ci/side-panel/run.sh`,
  `python3 -m py_compile ci/side-panel/driver.py` and `node --check` on
  `sw.js`, `lib.js` and `bridge.js`: exit 0.
- `cargo test --workspace`: `nautilus-engine` 120 passed (101 before this
  cycle; 19 new: the policy, table, install and containment tests), xtask
  2 passed, 0 failed (`review2-cargo-test-workspace.log`). The static checks
  and the clippy, build, fmt and bundle logs are `review2-static-checks.log`,
  `review2-cargo-clippy-workspace.log`, `review2-cargo-build-workspace.log`,
  `review2-cargo-fmt-check.log` and `review2-bundle-mac.log`.
- Mutation check of the loss rule (`review2-mutation-policy-fail.log`):
  removing the debugger-report exemption from `policy::failed_by_loss`
  made `a_page_loss_fails_its_queries_and_clicks_and_never_a_debugger_report`
  fail; restored, the policy tests passed (`review2-mutation-policy-restored.log`,
  13 of 13).
- Runs of `ci/side-panel/run.sh` on the final bundle (`review2-*.out`
  driver output, `review2-*.status` exit status, engine logs
  `review2-*-engine-launch*.log`, per-call records `review2-*-results.jsonl`):
  - spike, run 1: exit 0; 50 PASS, 0 FAIL; unfocused latency max 71 ms,
    p95 64 ms, 0 timeouts; the relay recovery passed (below).
  - spike, run 2: exit 0; 50 PASS, 0 FAIL; unfocused max 68 ms, p95 63 ms.
  - pump, runs 1, 2 and 3: exit 0 each; 7 PASS, 0 FAIL each; latency max
    75, 92 and 87 ms; p95 63 to 64 ms; 0 timeouts; no call of a second.
  - control, not a pass (see the measurement): exit 1; the driver's one
    FAIL is the latency check; `review2-control-pump1*`.
  - Superseded and not cited: a shakedown pump (`review2-shakedown-pump*`,
    before the driver's detail fix); a first official round, which used
    the earlier I7 check (the first shown line of the launch; it was
    replaced by the last shown line, `53134d2`); a spike run that finished
    with the relay-lookup FAIL (its type test; fixed in `812a0e7`); and a
    spike run stopped by hand while the loop above kept its browser up
    (the loop's head is kept).
- Relay recovery (I9, M10), spike run 1 (`review2-spike1-engine-launch2.log`):
  the driver closes the relay page with `Target.closeTarget` through the
  control's DevTools channel (`review2-spike1.out`: `relay (I9): Target.closeTarget
  closed the relay page`); the engine logs `bridge: page <relay> is gone
  (its DevTools session ended)` at line 100 and `ready` at line 108; a click
  after the reopening reaches the worker (`relay (M10): a click after the
  reopening reaches the worker`). The same check passes in spike run 2.

Measured (I7, the question the issue asked; `review2-*-results.jsonl`,
the latency run's calls): the background tab's `Input.dispatchMouseEvent`
took at most 11 ms in each of the five official runs (40 calls each,
minimum 0 ms). The control, the same tab with un-hiding disabled (a
temporary, reverted edit to `relay.rs` `set_tab_debugged`, never committed,
its engine log `review2-control-pump1-engine-launch1.log` with the hidden
line at line 13 and no shown line), took 5001 to 5015 ms per call, 27 of
them, all over one second; the latency run did not finish within the
driver's 140 s wait. The H5 stall therefore returns without the
un-hiding and does not return with it, on the same tab, aimed by the same
rule (the background tab). Nothing about it is left to investigate in
this cycle.

Not verified in this cycle:

- Windows: nothing ran there. Only the Windows build exercises the
  `cfg(windows)` branch of `control_refusal` (canonicalisation and
  `USERPROFILE` on real paths); the containment rule `outside_profile`
  itself is tested on every platform with synthetic paths, and the
  bridge's CEF paths are the same code as macOS.
- The reopen gate (`REOPEN_GAP`) is not unit-tested: it reads the clock
  inside the engine's turn. The runs show a relay that died long after
  its opening coming back within one turn, not the gate's bound.
- Lifetime paths not provoked in a run: render-process termination
  (`on_render_process_terminated`); the helper's page closing while
  debugger reports wait for a table; the queue's bound (256) and the
  overflow's apply path; the relay's retry and abandon paths (`Next::Retry`,
  `Next::Abandon`) with their closing; a page closed while an evaluation
  is in flight. The policy functions for these are unit-tested; the wiring
  was reasoned from the cef crate's handler signatures. The relay's
  closure by a DevTools session end (`Target.closeTarget`) and its reopening
  were provoked and are in the driver. This is the caveat the acceptance
  record carries for the same paths.
- The toolbar's own click (Task 13 step 5): still not done.
- DevTools ("Inspect") on a background tab (hidden, so slow, not
  measured); store-installed and "Load unpacked…" extensions (not
  bridged); `chrome.debugger` called from an extension page (not reported).
- The Intel build and macOS 26: not run.

Verified by the cycle-1 claims (rewritten from the kept logs in cycle 2)
are in the next section.

## Status and handoff (2026-11-18, review fixes to Phase 1)

The code review of Phase 1 found 15 issues; all are fixed (commits on
`extension-side-panel`, one per logical group; the repository's history
names the issue numbers). The evidence logs for the claims below are in
`evidence/phase-1/` beside the worktree, not committed.

What changed, by area:

- **The hidden-tab fix was wrong and is replaced (C1).** The 2026-11-17
  fix keyed "debugged" on CEF's DevTools-agent callbacks. Confirmed by a
  run before any change: with three tabs on a throwaway profile, every
  tab logged the attach at creation (`attached TabId(n) hidden=false`,
  caused by the probe's own `Target.getTargetInfo`) and every hide saw
  `dev_tools_attached=true`, so `was_hidden(1)` was never called and no
  tab was hidden. Now `bridge.js` wraps `chrome.debugger.attach`,
  `detach` and `onDetach` and reports each tab (`debugger.attached` and
  `debugger.detached`, answered only after the engine has applied the
  change). `bridge/debugger.rs` (pure, tested) keeps which extension
  debugs which tab; `Bridge::set_debugged` shows a hidden tab on its
  first debugger and hides it again on the last. `ViewState::dev_tools_attached`
  is gone; `ViewState::debugged` and `wants_hidden()` replace it.
- **Delivery identity and retry (I1).** The click's delivery id is set
  once and reused. `policy::after_delivery`: an evaluation failure is
  never retried (its outcome is unknown); a relay that never heard the
  worker retries once under the same id; a worker that took a delivery
  and failed is not sent it again. The protocol doc states the guarantee
  and its limit.
- **Page lifetime (I2).** `PageLife::on_before_close`, a new
  `PageRequest` (`on_render_process_terminated`) and the DevTools agent's
  detach all reach `Bridge::page_gone` or `page_detached`, which drop the
  page's pending evaluations, reset the helper's `refreshing` flag, fail
  the jobs that needed the page (a tab-list call gets an error answer;
  a debugger report waits for the table, see review cycle 2). Cycle 2
  changed the rest: a relay that is gone keeps its debuggers, and the
  next turn reopens it, never the callback (`Bridge::reopen`). Each page
  has a generation so a
  stale callback cannot touch its successor. The job queue is bounded
  at 256.
- **CEF is never called with the bridge's state borrowed.** The
  pump harness found this, not the unit tests: two cycle-1 runs' browsers
  aborted at exit ("RefCell already borrowed", `relay.rs:696`; the engine
  logs `review-pump-run1-engine.log` and `review-pump-run2-engine.log`,
  lines 82 to 85), and each harness output still printed `RESULT all
  checks passed` (`review-pump-run1.out` and `review-pump-run2.out`,
  line 12). `shutdown`, `evaluate`, and the debugger
  visibility updates now clone out the browser and view first. The
  harness's status does not show an abort; check the browser's log too.
- **Decisions are pure and tested (I5, M4).** `policy::relay_changes`,
  `policy::job_turn` and `policy::after_delivery`; `armed_after_spin`
  (`engine.rs`). `debugger::Debuggers` has its own tests.
- **Install and paths (I3, I4, M9).** The worker-path validator refuses
  `:` and Windows device names on every platform; `join_under` checks the
  loader's path before it is written. `copy_dir` uses `symlink_metadata`,
  skips links (a behaviour change: symlinked files are no longer copied),
  refuses a source that contains, equals or is inside the store, and
  copies into `<name>.partial` renamed into place.
- **Minor:** stale module names fixed; `TabTable::tab` is the reverse
  lookup the debugger report uses (no `allow`); the helper's extension
  and page load only while a bridged extension is on (M5); a finished
  load clears the dialog answer (M6); `hasListeners` and the bridge's own
  listener are hidden from `onConnect` (M8); the test control refuses a
  directory another user or group can write, on Unix (M3); the harness
  creates its control directories at 0700.

Verified on the Mac in cycle 1 (2026-11-18; each claim names its
kept file under `evidence/phase-1/`; "claimed in the session" marks a
claim no file keeps):

- `cargo test --workspace`: 101 passed in `nautilus-engine` and 2 in
  xtask, 0 failed; clippy and fmt clean (claimed in the session; no test
  output was kept).
- `ci/side-panel/run.sh pump`, three runs. Run 1 (`review-pump-final-run1.out`,
  line 10: `RESULT all checks passed`) has an engine log
  (`review-pump-final-run1-engine.log`) with no panic, abort or shutdown
  line. Runs 2 and 3 (`review-pump-final-run2.out` and `review-pump-final-run3.out`, line
  10: `RESULT all checks passed`) have no engine log kept, so no abort is
  shown for them. Exit statuses were not kept (claimed in the session:
  0). Latency (INFO lines): max 725 ms in run 1 (one
  `Page.captureScreenshot`; the rest of that run max 65 to 68 ms), 72 ms
  and 70 ms; p95 63 to 64 ms; 0 timeouts.
- `ci/side-panel/run.sh spike`, two runs: `review-spike-run1.out` and
  `review-spike-run2.out` each have `RESULT all checks passed` (line 42)
  and 34 PASS lines; unfocused max 165 ms and 88 ms. The engine log of
  spike run 1 (`review-spike-run1-engine.log`, line 13) shows the
  background tab hidden (`hidden, painting stopped = true`). The claim
  that the debugged tab was shown is not backed: that log has no shown
  line, and spike run 2 has no engine log (claimed in the session).
- The two cycle-1 pump runs whose browsers aborted exited with `run 1
  exited with status 134` (`review-pump-run1.out` and `review-pump-run2.out`,
  line 2); the harness reported them as passed (see the abort bullet above).
- Not verified: Windows (everything in the bridge builds there in
  principle, but nothing ran); the toolbar's own click (Task 13 step 5);
  DevTools ("Inspect") on a background tab: such a tab is hidden, so an
  inspection of it is slow (as before the bridge), and it is not
  measured; a store or "Load unpacked…" extension (still not bridged).

## Status and handoff (2026-11-17, the extension bridge)

Phase 1 of the side-panel plan (`docs/bridge-protocol.md` has the
messages; `docs/upstream-issues.md` has C11 to C14). What exists: the
manifest rewrite and the loader (`bridge/rewrite.rs`), the files on disk,
the copy of an unpacked folder and the helper extension written into the
profile (`bridge/install.rs`); the worker gets `nautilus-bridge/bridge.js`
(`bridge/js/`); the engine reaches it through a relay page, one of the
extension's own pages in a hidden browser (`bridge/relay.rs`: `HiddenPage`
and `Bridge`), with `Runtime.evaluate` over DevTools, and the worker's
calls come back through a long poll (`nautilusRelay.next()`); the tab-id
table joins each tab's DevTools target id with the helper's
`chrome.debugger.getTargets()` (`bridge/table.rs`); an action click goes
through `ExtensionManager::click_action`; the test control
(`NAUTILUS_TEST_CONTROL`, `poll_control`) and the scripted harness
(`ci/side-panel/`) drive it; the pump's fallback tick (`PumpScheduler`)
and a hidden-tab fix went in with it. That fix was wrong and is replaced
(see 2026-11-18). Only the test control's `load` bridges an extension in
this phase.

Verified, scripted: `ci/side-panel/run.sh spike` at the head of
`extension-side-panel` on 2026-11-17 (the bundle from
`cargo xtask bundle-mac`), three runs, each exiting 0 with
`RESULT all checks passed` and 34 PASS lines (`task13-run-A-before.out`,
`task13-run-B-after.out`, `task13-run-C-after.out`; exit statuses were not
kept). Run A was before the driver fix (4ea16f8); B and C were after it.
- Install: the worker runs the loader beside its entry
  (`/background/nautilus-loader.js`) and the bridge is in it
  (`{'protocol': 1, 'portHidden': True}`).
- (a) After a restart the worker runs with the bridge and the click
  reaches `action.onClicked`. After a stop: the driver saw the worker
  running, then `ServiceWorker.stopAllWorkers` over DevTools stopped it
  (B and C: "the worker stopped after 0.9 s (by DevTools)"); the click
  started it, with the bridge. Run A reads "0 s", the detection before
  4ea16f8, which did not look for the worker running first, so it is no
  evidence of a stop. Run 2's log has no `TEST HOOK: command … load`.
- (b) The click on the stopped worker was delivered once, to the worker
  it started (`delivery 2 (action.onClicked, tab …) sent`, then
  `delivery 2 taken by the worker after 1 attempt(s)`). Run 2 of B and C
  has three deliveries, one per click, each taken after one attempt.
- (c) `tabs.query` answered with the two Nautilus tabs, exactly one
  active, both before and after the stop; `scripting.executeScript`,
  `debugger.attach` and `debugger` `Runtime.evaluate` accepted each id
  (nine lines per launch); `bridge: table has 2 tab(s) of 6 target(s)`;
  the extension's own `onConnect` listeners never saw the bridge's port.
- (d) Unfocused (Finder in front from six seconds in; `focus = false` is
  the last focus line): 202 calls, max 87 ms (A), 80 ms (B) and 115 ms
  (C), p95 63 to 64 ms, 0 timeouts, none over one second.

The pump. Task 11 confirmed H1: CEF had work it did not ask for. With
the tick off on the instrumented build the pump logged `no spin for
137354 ms; CEF asked 0 time(s) since it ended; timer pending: false`
(claimed in the session; no engine log was kept) and the latency run did
not finish (`task11-pump-tick0-run1.out`, a FAIL); a second
tick-off run, on the fix build, failed the same way
(`task12-tick0-control.out`). With a 33 ms tick the run passed
(`task11-pump-tick33-run1.out`: max 90 ms, p95 63 ms). H2 (a lost delayed
request) is ruled out: with the replace rule and a 600000 ms interval,
the timer pending and nothing asking sooner, the run still stalled, two
calls taking 20005 and 20010 ms (claimed in the session;
`task11-pump-tick600000-run1.out` records only the failed latency check). H3 (a
lost wake) was not observed: the wakes stayed below the spins and no
`sample` was taken; F3 was not applied. H4 (App Nap) was not tested: no
run had the window focused; F4 was not applied. H5 (a debugger on a
hidden tab) was seen once with the tick on and before the hidden-tab fix:
27 `Input.dispatchMouseEvent` calls, each 5001 to 5011 ms, to a tab behind
another with a debugger attached (claimed in the session; `task12-fix-run1.out`
records the failed latency check only; review cycle 2 reproduced the stall,
see below). The F5 fix for it
(a tab with a DevTools agent attached is not hidden) hid no background
tab at all, because the agent attached for the probe's own target lookup;
the figures below from F5 runs were measured on that build, not on an
intended one (see 2026-11-18). Passing `run.sh pump` runs, 202
calls each, unfocused: F1 alone, `task12-fix-run2.out` and `task12-fix-run3.out`
(max 77 and 68 ms, p95 64 ms); with F5 (no tab hidden), five runs
`task12-f5-run1.out` to `task12-f5-run5.out` (max 65 to 77 ms, p95 63 to 64
ms). Without F5 one run in three failed (`task12-fix-run1.out`). Idle cost of the tick: about 0.5 %
CPU and 416 wake-ups, against 0.0 % and 201 without it (sampled in the
session; no log was kept).

The uncertainties U1 to U5, all held; no fallback was taken.
- U1: a windowless browser with no parent view loads a `chrome-extension://`
  page on macOS: `bridge: page helper ready` in the run logs.
- U2: `Runtime.evaluate` with `awaitPromise` stays pending while
  `nautilusRelay.next()` waits: the relay's poll stayed pending through
  every run and the calls were answered. The stop waits were under a
  second (0.9 s); the 40 s wait the plan named was not run.
- U3: `defineProperty` on `onConnect` holds for the worker: `portHidden`
  is true and no listener saw the port. Pages are not covered (below).
- U4: `chrome.action.onClicked.dispatch` from the bridge runs the
  listeners: the click reached `action.onClicked` in every run.
- U5: `chrome.debugger.getTargets()` works from the helper's page in a
  windowless browser: `bridge: table has 2 tab(s) of 6 target(s)`.

Verified by hand: nothing in this phase. The toolbar's own click (Task
13, step 5) is not done.

Not verified: Windows (nothing here has run there; the harness is `sh`
and `open -a Finder`); a module worker (`"type": "module"`) in a real run
(the loader for it is unit-tested only); a store-installed extension, or
one loaded by "Load unpacked…" (only the test control's `load` is
bridged); an extension that already has pages with their own `onConnect`
listeners (they receive the bridge's port until Phase 5 bridges pages); a
click while the helper's page is still loading; a tab adopted from a
page-opened popup (`Pending::Popup`) in the table; the focused-window
case of the latency run (no run had the window focused); `tabs.group` and
`tabs.discard` still crash for any extension that calls them. Also not
verified: App Nap and lost wakes (H3, H4); the stall with the tick off
was reproduced on one instrumented run and one fix-build run, not the
three the plan asked for; F5 rests on one bad run in three without it and
none in five with it.

For Phase 5: the transport is as `docs/bridge-protocol.md` says (the long
poll over `Runtime.evaluate` held, U2; no console fallback); `tabs.query`
ignores `queryInfo` and reports `windowId: 1`; `load_bridged` copies on
the UI thread and must move to the unpacking thread; a copied extension's
directory is `unpacked-<name>-<8 hex of the hash of the source path>`
(`install::copy_name`, the scheme Phase 5 keeps), so `Request::Uninstall`'s
cleanup (which deletes `<store>/<id>` only, `extensions.rs`) must learn to
delete it; the bridge must go into extension pages too, where it has to
hide the port the same way; the unpacked-load queue exists (`next_load`).

For Phase 6: a hidden page is `HiddenPage` in `relay.rs` (a windowless
browser with a 16 by 16 view and a DevTools observer); the panel's browser
can start from it but needs the tab's render handler.

For Phase 7: `ManifestFacts::commands` carries each command's name,
description and this platform's `suggested_key` as written;
`chrome.commands.onCommand.dispatch` worked in the probe from the worker's
context; the hidden-tab fix was replaced on 2026-11-18 (see that section).

## Status and handoff (2026-11-17, later)

The user's report: a call from a web softphone failed with "Twilio
31401: The browser or end-user denied permissions to user media".
Cause: the tab's client had no permission handler, and CEF's Alloy
style then refuses every media request without asking anyone. Now the
request goes to the user (see Conventions; the question, the dialog
and the run-long memory are osmena's). Made on the branch
`fix/media-permission` in a worktree beside osmena's and merged to
`main` the same day, with osmena at e111e50.

Verified on the Mac with the bundle and a local probe page that calls
`getUserMedia({audio: true})` and logs the outcome. Before:
`NotAllowedError: Permission denied`, no question. After: the system's
sheet on the window, and with Allow clicked by hand the page logs a
live track from the headset; a second request in the same run is
granted with no dialog; a refusal (scripted through a temporary hook
in osmena, since removed) gives `NotAllowedError` for each request and
asks again. Also `cargo test --workspace` (the permission bits among
them), clippy, fmt. The egui prompt this started as is what found C10:
with it, losing focus before answering made Allow fail.

Not verified: a real Twilio call (the report's own page), Block by a
real click, the camera, a request from a popup or from a background
tab, the installed app (its first use will also bring macOS's own
microphone question, since each ad-hoc build is a new app to the
system), Windows. Observed twice and not traced: "Teardown watchdog
expired; recording dump and terminating" from a helper at exit when a
microphone stream was still open; the browser exited with code 0 and
left no process.

## Status and handoff (2026-11-17)

The user's report: on a Metabase login page, "Sign in with Google"
opened two sign-in pages, one that never loaded and one that signed in
but never returned to Metabase. Cause: `on_before_popup` cancelled
every popup and opened an unrelated tab at its URL, so `window.open`
returned null and the sign-in page had no `window.opener` to post the
credential to; a page that opens a blank named window and then sends a
form or a second `window.open` to that name got a second tab, the
first staying blank. Now Chromium creates the popup (see Conventions).

Verified on the Mac with the bundle and local probe pages, run by
`NAUTILUS_TEST_PAGE_SCRIPT` with
`NAUTILUS_CHROMIUM_SWITCHES=disable-popup-blocking` (a scripted
`window.open` has no click behind it, and Chromium blocks it before
`on_before_popup`; a real click needs no switch). Before the change:
`window.open returned null`, `popup: opener is null`, the opener's
title unchanged, and three tabs for the blank-then-named-form page.
After: `window.open returned a window`, `popup: opener is present,
name=signin`, the opener's title set from the popup's `postMessage`,
two tabs for the named-form page, and a popup that posts and calls
`window.close()` leaves one tab; each run exits cleanly. Also `cargo
test --workspace` (the held-events and disposition tests among them),
clippy, fmt.

Not verified: the real Google sign-in by hand (scripted clicks need
Accessibility on this Mac), that the popup's tab paints and takes
input by eye (the probes read only logs), a popup opened in the
background, a popup whose opener's window closes before adoption
(`Pending::discard`), Windows. A popup's `width`/`height` features
are ignored: it is a tab.

## Status and handoff (2026-11-16)

Alt codes on Windows (Alt+0151 for an em dash) typed nothing. The
cause and most of the fix are osmena's (winit drops the `WM_CHAR`
that follows Alt's release; the shell's message hook delivers it, to a
page as `EngineInput::ImeCommit`). Here, `host.rs` now passes
`ime_commit_text` CEF's invalid range (`u32::MAX`, `u32::MAX`) for
"replace nothing": with `None` the call inserted nothing, which no
one had seen because no IME text reaches a page yet. Verified on
Windows with the debug build and `keybd_event` (Alt held, keypad
digits, Alt released): a textarea received `a`, U+2014, `b`, U+E9,
read from the title its `input` handler set; the address bar showed
`x—y`. Also `cargo test --workspace`, clippy, fmt. Not verified:
macOS (no Alt codes there; `ImeCommit` is otherwise unreachable),
`ime_set_composition`, which still passes `None` ranges and may need
the same.

## Status and handoff (2026-11-08)

Blocked sites over Chromium's before-browse hook (osmena has the list,
the block page and the settings section; the README's feature bullet
and checklist item 17 say what a user sees). `TabRequestHandler::
on_before_browse` (`delegate.rs`) asks the pure `blocked_navigation`
filter whether a main-frame URL falls under a blocked site. If it does,
the hook returns 1, which cancels the load before any request, and
pushes `EngineEvent::NavigationBlocked`. The list lives in
`DelegateState` (`blocked`, set by `Engine::configure_blocked_sites`).
`TabHost::replace_with_blank` runs `location.replace('about:blank')` on
the main frame, so a blocked tab's entry is swapped rather than buried
under the block page. `on_load_start` logs the URL of each main-frame
load. The filter's tests cover the subdomain rule, subframes, and the
non-web schemes (`about:`, `data:`, `view-source:`, `chrome:`,
`devtools:`, extension pages, text that is no URL).

Scripted run (debug build on Windows, `NAUTILUS_TEST_PAGE_SCRIPT` clicking
a link to example.com): `load started at file:///...`, the console line
`clicking`, `blocked navigation to https://example.com/`, then
`load started at about:blank`, and no `load started at https://example.com/`
in any run. With `NAUTILUS_CEF_LOG=verbose`, Chromium's log had no line
naming example.com. The test script runs again at every main-frame load
end, so the first version clicked twice and produced two blocked lines and
two about:blank loads; a guarded version that clicks once gave exactly one
of each.

Redirect (a 302 from a local server to https://example.com/, in a probe
profile): the hook saw it. The log has
`blocked navigation to https://example.com/ (a redirect)` (the file breaks
that line after "(a "), then one `load started at about:blank`, and the
exit line reads `at https://example.com/`. No `load started` named
example.com.

By hand: a typed example.com gives the block page with no hook line, and
Back returns to the previous page. Blocking the page of a tab from its
menu, with the real replacement, returns Back straight to the previous page
with no load of the blocked site after the block. Forward then shows a plain
blank page with `about:blank` in the address bar. That is the replaced entry
itself, known and accepted: nothing of the site is requested, and typing the
address again gives the block page. Remove in the settings window returns the
tab to the site on its own. "View page source" opens a `view-source:` tab.

Not verified: DevTools on a blocked tab; macOS; the puzzle menu's
"Manage extensions" opens the settings window by design, and typing
`chrome://extensions` into a `view-source:` tab did not navigate in the
run (not tried on a normal tab). Observed: a URL change to
`about:blank#x` on a blocked tab clears the block, since the blank-page
rule compares the exact string. A test script that set a hash on the
blank page did this in Nautilus (the exit line read `at
about:blank#once`), and an opener window can do the same with
`w.location = 'about:blank#x'`, even cross-origin. The effect is
cosmetic: the block page gives way to a plain blank page, nothing from
the blocked site is requested, and the hook still cancels any later
navigation to the site. The fix belongs in osmena's `apply` (match the
scheme and path, ignoring the fragment) as a follow-up. CEF logs
"Timeout of new browser info response" after a block load; it has not
been traced.

## Status and handoff (2026-11-05)

Drag and drop within a page, in two halves (see Conventions): the
embedder side (`DragSession`, the drop-target calls, the drag image)
and the Chromium feature that C9 describes, without which Blink
cancelled any drag whose `dragstart` handler set `pointer-events:
none` before the embedder saw it at all.

Verified by driving real drags with SendInput against the debug
build, on a probe page with six sources (a div that sets data, a
plain div, a link, an image, a div that sets `pointer-events: none`,
a div that only fades, and a text selection). Every one logs one
"drag started" and one "drag ended … dropped" in the engine, and the
page gets `drag`, `dragenter`, `drop` with the right
`dataTransfer.types` (`text/plain` for the divs, `text/uri-list` and
`text/html` for the link, `Files` for the image) and `dragend` with
`dropEffect=move`. The probe page and its driver are in the session
scratchpad (`dragsources.html`, `dragsources.ps1`). Also `cargo test
--workspace` (the frame's blending test among them), clippy, fmt.

Not verified: the cancel paths by hand (a release over the toolbar,
Escape mid-drag, Alt+Tab mid-drag) and whether the drag image looks
right by eye, since the probe only reads the event trace; macOS
entirely. If a drag ever sticks on the no-drop cursor after a drop,
`update_drag_cursor` did not arrive for the last move. Note that the
older `draggable-hover.html` probe in the scratchpad, written before
any of this was run, fails on its card A for the C9 reason and is
superseded by `dragsources.html`.

## Status and handoff (2026-11-03)

From the user's reports on the Apple silicon Mac (macOS 15.5), with
their causes:

1. and 2. The icon's black border and odd corner radius: `logo.svg`
   drew a black stroke and a 976 px tile with `rx=200`, and osmena set
   the Dock icon from that PNG at run time over the bundle's `.icns`.
   Now no outline, a continuous-corner tile, an `.icns` on Apple's grid
   (`logo-macos.svg`, through osmena-xtask's new macOS source), and
   osmena leaves the Dock alone in a bundle with an icon.
3. and 4. The emoji palette and the dictation prompt opening while
   typing: C7. Reproduced in the bundle with keys sent to the tab
   through a temporary hook (since removed) and the two actions
   replaced by logging stubs: with the old handler, typing "hello d"
   fired `orderFrontCharacterPalette:` on the E and `startDictation:`
   on the D; with the fix, neither, and every letter typed.
5. Backspace deleting two characters: C8. It happened on every
   Backspace, not sometimes: the same hook typed "hello" and one
   Backspace and the page saw two `keydown`s and "hel" before the fix,
   one and "hell" after; Shift's `keydown`/`keyup` now carry
   `shiftKey` true/false.
6. The default browser: osmena's (see its handoff); the bundle's
   `Info.plist` claims web links. Verified on this Mac: Launch
   Services lists Nautilus for `http` and `https`, the status reads
   "No" (Comet is the default), `open -a` of the bundle with a URL
   while it is not running loads that URL in place of the home page,
   with it running adds a tab, and an `.html` file opens. Not verified:
   the "Make default" button and the system's question (that would
   change the user's default browser), a real link from Mail, the bar
   and the Settings section by eye.

Also: `cargo test --workspace`, clippy, fmt; osmena's likewise. Not
verified: anything by hand with real keystrokes (Terminal has no
Accessibility permission here), Windows (no change there but the
icon's art and osmena's shared code; CI will tell), macOS 26. Known
and left: text from the emoji palette and dictation is lost (E2).

The osmena changes are in the local checkout only, through the patch
table: commit and push osmena first, then bump every osmena `rev` in
`Cargo.toml` here and restore `Cargo.lock` as the handoff below
says.

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
