# Nautilus

A desktop web browser for macOS and Windows. Chromium (through CEF) renders
pages, egui draws the chrome, winit owns the windows. Everything that
touches CEF lives in one crate so the engine could be replaced without
rewriting the model or the UI. Nautilus is the Chromium sibling of Paladin
(`../paladin`, Servo): the model and the chrome are the same design and
mostly the same code; keep them in step when a change belongs to both.

_Last updated: 2026-10-19 (project start)._

## Architecture

Cargo workspace (Rust 1.97.1 pinned in `rust-toolchain.toml`, edition 2024).

| Crate                    | What it is |
| ------------------------ | ---------- |
| `crates/nautilus-core`   | `nautilus-core` — the engine-free model: `model` (`Browser`/`Window`/`Tab`), `command` (`Command`, `dispatch`, `EngineAction`), `event` (`EngineEvent`, `Cursor`, `apply`), `keymap` (`Keymap`, `KeyChord`, `CommandKind`), `settings` (TOML), `profile` (paths). Depends on neither cef, egui, nor winit. |
| `crates/nautilus-engine` | `nautilus-engine` — the only crate that imports `cef`. `engine.rs` (`run_subprocess`, `Engine`, the message-pump bridge), `host.rs` (`TabHost`: the tabs of one window, each a windowless browser), `delegate.rs` (CEF handlers → `EngineEvent`), `frame.rs` (`Frame`: a tab's BGRA pixels with the popup widget composed in), `input.rs` (`EngineInput` → CEF events), `keycodes.rs` (key code tables). Re-exports `keyboard_types`, `PhysicalSize`, `RawWindowHandle`. |
| `crates/nautilus`        | `nautilus` — the binary: `main.rs` (dispatches Chromium's helper processes first), `helper.rs` (the `nautilus-helper` bin for macOS), `app.rs` (winit `App`), `window.rs` (`BrowserWindow`: egui chrome over the active tab's frame texture), `gl.rs` (`GlSurface` via glutin, `PageTexture`), `settings_window.rs`, `chrome.rs` (pure chrome drawing that returns `Command`s), `routing.rs` (pure, unit-tested pointer/key routing and page-area geometry), `menus.rs` (the window and tab menus as unit-tested `MenuEntry` data), `native_menu.rs` (shows a `MenuEntry` list as a native popup through `muda`), `input.rs` (winit → `EngineInput`, `route_key`), `keyutils.rs` (winit keys → `keyboard-types`, copied from servoshell, MPL-2.0 header must stay), `process.rs` (job object, `open_in_profile`), `icon.rs`, `fatal.rs`, and the cargo-packager metadata in its `Cargo.toml`. |
| `xtask`                  | `cargo xtask icons` (renders `assets/logo.svg` at 1024 px and area-averages the smaller icns/ico/png sizes from it). |

Data flow: the chrome and the keymap emit `Command`s; `nautilus_core::dispatch`
updates the model and returns `EngineAction`s, which the app forwards to the
window's `TabHost` or handles itself (open and close windows, settings, quit).
CEF handler callbacks become `EngineEvent`s that `nautilus_core::apply` folds
into the model. Both functions are pure and unit-tested; core ids never meet
CEF objects outside `nautilus-engine`.

Rendering: each tab is a windowless CEF browser. CEF calls the tab's
`RenderHandler::on_paint` with BGRA pixels, which land in the tab's `Frame`
and raise `EngineEvent::FrameReady`; the window then uploads the frame to a
GL texture (`gl::PageTexture`) and the chrome paints it as an egui image
under the toolbar. `<select>` popups arrive as a second paint element and
are composed into the frame.

Local setup is in `README.md`.

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

- **CEF stays in `nautilus-engine`.** The model and the UI only see
  `Command`, `EngineAction`, `EngineEvent`, `EngineInput`, `Frame`, and
  core ids. The binary never names a CEF type.
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
  (`Waker`), and the app calls `Engine::spin` (`do_message_loop_work`).
  Every CEF callback arrives on the main thread from inside `spin`, so the
  handlers use `Rc`/`RefCell`.
- **Browsers close on the message loop.** `TabHost::close_tab` asks the
  browser to close; `on_before_close` arrives later. `Engine::drop` pumps
  until the live-browser count is zero (bounded) and only then calls
  `cef::shutdown`. Windows (tab hosts) must drop before the engine.
- **The CEF binary distribution comes from `CEF_PATH`** (or is downloaded
  into `OUT_DIR` by `cef-dll-sys`). Bumping CEF means bumping the `cef`
  crate pin in the workspace `Cargo.toml`, re-running `export-cef-dir` for
  the matching version, and re-checking the handler signatures (they are
  generated from CEF's headers and change between releases).
- **Version pins move together.** `winit`, `egui`, `egui-winit`,
  `egui_glow`, and `glow` share a GL context and raw window handles; bump
  them together, and `glutin` with them.
- **Every user action is a `Command`.** New chrome controls and shortcuts add
  a `Command` (and a `CommandKind` if it needs a hotkey), never a direct
  engine call.
- **Menus are native, the rest of the chrome is egui.** A menu is a
  `Vec<MenuEntry>` from `menus.rs` (label, enabled, optional chord,
  `Command`), shown by `native_menu::NativeMenu` after the frame is
  painted (`BrowserWindow::show_menu`); the chrome only records a
  `MenuRequest`. Add entries in `menus.rs` with a test, not in `chrome.rs`.
  Chromium's own page context menu (right-click on content) stays
  Chromium's.
- **Page-opened tabs are URLs.** `on_before_popup` and `on_open_urlfrom_tab`
  cancel Chromium's popup and report `EngineEvent::NewTabRequested` with a
  `PendingTab` that holds the URL; `AdoptTab` creates a fresh browser at
  it. The opener gets no `window` handle.
- **Key routing has one rule.** It lives in `input::route_key` (Cmd/Ctrl
  chords fire even in the address bar; otherwise egui while focused, then
  bound chords on initial press, then the page). Browser windows apply it via
  `routing::KeyRouter`, which sends repeats and releases to the side that got
  the press. Change routing there, with a unit test, not in `window.rs`.
- **Input to CEF is logical pixels.** `EngineInput` carries physical pixels
  relative to the page area; `TabHost::input` divides by the scale factor
  and tracks click counts and held buttons, which CEF expects the embedder
  to supply.
- **Packaging metadata** is `[package.metadata.packager]` in
  `crates/nautilus/Cargo.toml`. cargo-packager resolves its paths against that
  crate's directory (hence `../../`). The `resources` globs pick up the CEF
  runtime files the build copied into `target/release/`; they assume the
  default target dir and no `--target`.
- **Generated icons are committed.** Edit `assets/logo.svg` (pure vector,
  no `<text>`, no `<image>`: the xtask builds resvg without font or raster
  support), run `cargo xtask icons`, commit the result. Never hand-edit
  `assets/icons/`.
- **Format before every commit:** `cargo fmt --all`. CI fails on formatting
  or clippy warnings.
- **`AGENTS.md` mirrors this file.** Keep them identical.

## Definition of Done

A task is complete only when:

1. **All tests pass** — `cargo test --workspace` is green, and the manual
   checklist in `README.md` passes for any change to the chrome, input
   handling, or engine glue.
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
