# Nautilus: Paladin's chrome on Chromium

_Approved 2026-10-19._

## Goal

A second browser with Paladin's feature set (windows and tabs, multi-tab
selection and moves, native menus, profiles, chrome colours, crashed-tab
isolation, installers) on a Chromium engine, so the user gets Chromium's
web compatibility and process model with the same chrome and workflow.

## Engine choice

Three ways to a Chromium-based browser were considered:

1. **CEF through the `cef` crate (tauri-apps/cef-rs).** Prebuilt Chromium
   from the CEF project (Spotify's automated builds), a Rust binding that
   tracks CEF releases (`cef` 154.5.0 = CEF 154.0.34 = Chromium
   154.0.8037), off-screen rendering, external message pump, handlers for
   everything Paladin's `EngineEvent` needs. The sys crate downloads the
   distribution and builds `libcef_dll_wrapper` with CMake and Ninja.
2. **Thorium.** A Chromium fork with AVX/PGO compiler flags and feature
   patches. It ships browsers, not a CEF distribution, and its patches
   live in a full Chromium tree: using it means a Chromium build from
   source (hours, 100+ GB) for every version, and maintaining the CEF
   layer on top.
3. **WebView2 (Windows) / WKWebView (macOS).** Only WebView2 is Chromium,
   and its process model and rendering are not under the embedder's
   control.

**Decision: CEF (1).** It gives Chromium now, on both platforms, with a
build that takes minutes. Thorium stays an option for later: CEF builds
from a patched Chromium tree are a known procedure, and nothing in
Nautilus depends on which Chromium the `libcef` binary came from.

## Architecture

The same three-crate shape as Paladin:

- `nautilus-core`: Paladin's model, commands, events, keymap, settings,
  and profiles, ported with renames. One addition: `EngineEvent::CursorChanged`
  with a `Cursor` enum, because Chromium reports page cursors and users
  expect the hand over links. The multi-tab selection logic (`selected`,
  `ToggleTabSelection`, `SelectTabRange`, `CloseTabs`, `MoveTabsToWindow`,
  `MoveTabsToNewWindow`, `OpenTabsInProfile`) is unchanged, tests included.
- `nautilus-engine`: the CEF glue. Replaces Paladin's Servo glue behind the
  same interface (`Engine`, `TabHost`, `EngineInput`), with two changes:
  the engine no longer owns GL (CEF paints into pixel buffers, so the
  binary owns the GL context through glutin and uploads frames as
  textures), and `run_subprocess` must run first in `main`.
- `nautilus`: Paladin's binary with `window.rs`, `app.rs`, `main.rs`, and
  `settings_window.rs` adapted; `chrome.rs`, `menus.rs`,
  `native_menu.rs`, `routing.rs`, `keyutils.rs`, `icon.rs`, `fatal.rs`,
  and `process.rs` carried over.

### Rendering

Windowless (off-screen) browsers. CEF's `RenderHandler` reports the view
size and scale (`view_rect`, `screen_info`) and receives BGRA frames
(`on_paint`) for the view and for popup widgets. The frame goes into a
per-tab `Frame`; the window uploads it to a GL texture when it changed and
egui draws the texture in the page area. Hidden tabs are told
`was_hidden(true)` so Chromium stops painting them.

The alternative, windowed browsers as child HWNDs/NSViews under the egui
window, would be faster (Chromium composites straight to the screen) but
puts a native child window over the page area: egui could not draw the
crash panel, drag overlays, or anything else over the page, and macOS
layering gets fiddly. Off-screen keeps the chrome in charge; the
accelerated shared-texture path can remove the copy later.

### Processes

Chromium's. Each page runs in a renderer process; GPU, network, and
utility processes are separate too. CEF starts them by running the
browser's executable with `--type=...`, which `run_subprocess` serves
before `main` does anything else. On Windows the browser joins a job
object with kill-on-close so a killed browser takes its helpers with it.
A dead renderer arrives as `on_render_process_terminated` and becomes
`EngineEvent::TabCrashed`, which the existing crash panel shows.

The sandbox is off: on Windows CEF's sandbox requires `bootstrap.exe` as
the process entry with the browser built as a DLL, and on macOS signed
helper apps. Both are Phase 2.

### Input

`EngineInput` is unchanged (physical pixels, `keyboard-types` keys).
`TabHost::input` converts to CEF's logical pixels, tracks held buttons and
multi-click counts, and turns a key press into the `RAWKEYDOWN` plus
`CHAR` pair Chromium expects, with Windows virtual-key codes from a
`Code` table (and macOS key codes for `native_key_code`).

### Menus and popups

Nautilus's menus (window menu, tab context menu) stay native through
`muda`. Chromium's own page context menu, `<select>` lists, and dialogs
are Chromium's: windowless browsers get the window's native handle as
their parent and `screen_point` places them.

Page-opened windows (`window.open`, middle-click) are cancelled and
reported as `NewTabRequested` with the target URL; the model opens them
as tabs. The opener gets no handle (a known limitation).

### What stays open

- macOS bundle: framework plus helper apps. `nautilus-helper` and the
  framework loader exist; the bundling step does not.
- Sandbox, accelerated OSR, IME, downloads, and the rest of Chromium's
  surface, as commands.
