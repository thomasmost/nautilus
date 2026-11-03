# Upstream issues

Nautilus is a shell around CEF (Chromium), egui, and winit. Some things it
cannot do, or does badly, are upstream limits rather than Nautilus bugs.
This file tracks each one: what it is, which features depend on it, what
Nautilus does in the meantime, and what to do when it lands.

_State recorded as of 2026-10-19, against CEF 154.0.34 (`cef` crate
154.5.0), egui 0.34, winit 0.30._

## How to maintain this file

- Add an entry when a bug or a planned feature turns out to depend on an
  upstream change. Give it the next id in its group (`C` CEF and the `cef`
  crate, `E` egui and winit, `T` toolchain and packaging). Link the
  upstream issue if one exists; say "no upstream issue found" otherwise.
- On every CEF bump, re-check each `C` entry.
- `README.md` lists the user-visible limitations in one sentence each and
  points here for the detail.

## CEF and the `cef` crate

### C1: Sandbox needs CEF's bootstrap executable on Windows

CEF 128+ runs the sandboxed browser process from its own `bootstrap.exe`
(or `bootstrapc.exe`), which loads the application as a DLL exporting
`RunWinMain`; a plain executable cannot enable the sandbox. The `cef`
crate's `bundle-cef-app` tool and `sandbox` feature support that layout.
Nautilus starts with `no_sandbox` and a normal executable. **Blocks:** the
renderer sandbox (ROADMAP Phase 2). **Workaround:** none; site isolation
and separate renderer processes still apply.

### C2: Windowless browsers paint through system memory

The default off-screen path (`on_paint`) hands BGRA buffers that Nautilus
uploads to GL textures every frame. CEF's `shared_texture_enabled` path
(`on_accelerated_paint`) shares a D3D11/Metal texture instead; the `cef`
crate exposes it behind the `accelerated_osr` feature with wgpu. **Blocks:**
smooth video and scrolling at 4K. **Workaround:** the software path.

### C3: macOS needs an app bundle with helper apps

CEF on macOS loads `Chromium Embedded Framework.framework` from the bundle
and runs helper processes from per-kind helper apps; `cargo run` of a bare
binary cannot start. The `cef` crate's `bundle-cef-app` builds such a
bundle for its examples. **Blocks:** `cargo run` on macOS.
**Workaround (2026-10-20):** `cargo xtask bundle-mac` builds the bundle
(`xtask/src/bundle_mac.rs`); developing on macOS means rebuilding the
bundle and running the executable inside it.

### C5: the `cef` crate's clone of a lent string list is empty

A handler argument such as `on_favicon_urlchange`'s `icon_urls` is a
`CefStringList` wrapping the pointer CEF lent; `CefStringList::clone`
copies the opaque struct behind the pointer, and the copy has no handle,
so `list.clone().into_iter()` (the only iteration the crate offers, and
it consumes the list) yields nothing. `cef` 154.5.0; no upstream issue
found. **Blocks:** nothing now. **Workaround (2026-10-21):**
`strings_of` in `delegate.rs` reads such a list through
`cef::sys::cef_string_list_size` and `cef_string_list_value`. Favicons
never showed before it.

### C6: a fresh profile's first launch sometimes loads nothing on macOS Intel

On GitHub's `macos-15-intel` runner, the first launch of the bundle on
a profile that does not exist yet sometimes starts every navigation
and commits none: the tab reports `loading = true` and then no
`on_load_start`, title, or load end for as long as the browser runs
(120 s tried), the hidden `chrome://extensions` page never loads
either, and `--exit-after` still exits cleanly. A second launch of the
same profile loads within a second, every time so far. Seen in 9 of 17
fresh launches between 2026-10-23 and 2026-10-28, on the code of
2026-10-23 as much as on the current one, and with `--disable-gpu`
too, so it is not the runner's GPU-process crash loop (the runner has
no GPU; Chromium gives up on the GPU process after a few restarts and
renders in software either way). Not seen on the Apple silicon runner
or on Windows; no Intel Mac has been tried by hand. Unexplained:
Chromium logs nothing about it even at `verbose`. **Blocks:** nothing
known. **Workaround (2026-10-28):** the CI smoke test launches up to
three times on the same profile and warns when the title arrives only
on a later attempt. To dig further, run the bundle on the runner with
`RUST_LOG=nautilus_engine=debug`, `NAUTILUS_CEF_LOG=verbose` and
`NAUTILUS_CHROMIUM_SWITCHES` (see `CLAUDE.md`, Commands), and take a
`spindump` of the stalled browser process; one run with
`--disable-gpu` and one with `--enable-logging=stderr --v=1` ended in
a segmentation fault at shutdown (exit 139) after the stall, which a
plain run does not.

### C7: macOS offers a windowless browser's unhandled keys to the menu bar as synthetic events

For a key the renderer did not handle and the client's `OnKeyEvent`
left alone, `CefBrowserPlatformDelegateNativeMac::HandleKeyboardEvent`
calls `[[NSApp mainMenu] performKeyEquivalent:]` with the event's
`os_event`. For a windowless browser that is the `NSEvent` CEF made up
from the `CefKeyEvent` in `TranslateWebKeyEvent`, whose modifier flags
are only those the CEF flags map to: none for a plain letter. The
key-down of every letter typed into a page field is "unhandled" (the
text comes from the `CHAR` event), and AppKit's hidden items in every
Edit menu match such an event although their key equivalents need the
Globe key: "Emoji & Symbols" (fn-E) and "Start Dictation…" (fn-D) on
macOS 15. A hardware-shaped event (from a `CGEvent`, flags `0x100`)
does not match them. Typing opened the emoji palette and the dictation
prompt and swallowed that E or D. Found 2026-11-03 against CEF 154.0.34
(source at its commit) with an AppKit probe calling `performKeyEquivalent:`
on the events CEF builds, and reproduced in the bundle with keys sent to
the tab (the palette and dictation actions replaced by logging stubs);
no upstream issue found. Windowed browsers (the extension popups) pass
the real event and are not affected. **Blocks:** nothing now.
**Workaround (2026-11-03):** the tab's `on_key_event` answers every key
as handled on macOS (`mac_unhandled_key` in `delegate.rs`); the menu bar
already had the real event before the window saw the key.

### C8: on macOS a key event without characters is a modifier change

`TranslateWebKeyEvent` makes the `NSEvent` an `NSEventTypeFlagsChanged`
when both `character` and `unmodified_character` are 0, and keeps the
CEF event's type only for `CHAR`. Chromium's `ui::IsKeyUpEvent` reads a
flags change as a release only for the modifier keys (by their flags)
and as a press for every other key code, so a key-up sent without
characters for Backspace, Delete, an arrow, Escape, Page Up/Down or a
function key reached the page as a second key-down: one Backspace
deleted two characters. This is CEF's own convention (its `cefclient`
sends AppKit's characters for every key), not a bug, but nothing in the
API says so. **Workaround (2026-11-03):** `input::event_character`
gives every key but a modifier the character AppKit's event would carry
(`NSDeleteCharacter`, the `NS…FunctionKey` range), and osmena gives a
modifier key's own event its modifier flag (Shift's press carries
Shift, its release not), so the flags change reads the right way.

## egui and winit

### E1: IME is enabled only for egui's own text fields

egui-winit calls `set_ime_allowed(false)` whenever no egui widget has
keyboard focus, so winit sends no `Ime` events while the page has focus.
**Blocks:** composed input (CJK) in page text fields. **Workaround:** the
`Ime` arm in `BrowserWindow::handle_event` is ready; the engine side
(`ime_set_composition`, `ime_commit_text`) is implemented.

### E2: winit drops text inserted without a composition (emoji palette, dictation)

winit 0.30's macOS view turns `insertText:replacementRange:` into an
`Ime::Commit` only while it has marked text and IME is enabled
(`view.rs`, "Commit only if we have marked text"). The emoji palette
(Ctrl+Cmd+Space) and dictation insert their text that way, with no
composition before it, so what they insert is lost, in page fields
(where IME is off, E1) and in the address bar alike. **Blocks:**
inserting emoji from the palette and dictated text. **Workaround:**
none yet; the palette opens, and typing works. A fix needs either a
winit change or osmena taking over that method of winit's view.

### C4: Chromium's extension dialogs crash a windowless browser

`chrome.webstorePrivate.beginInstallWithManifest3` (the store's "Add to
Chrome"), `chrome.management.uninstall`, and the enable prompts show
dialogs modal to the tab's native window (`ExtensionInstallPrompt` →
`constrained_window::CreateBrowserModalDialogViews` →
`GetModalDialogHost(parent)`); an off-screen browser has no native
window, the host lookup dereferences null, and the browser process
dies. Seen with CEF 154 on Windows (the user's crash of 2026-10-20,
resolved with CEF's release symbols). **Blocks:** Chromium's own install
and uninstall flows from store pages. **Workaround (2026-10-21):**
`store.rs` replaces those API methods in store pages' main frames with
ones that route the request to Nautilus's extension manager and answer
the page without a dialog.

## Toolchain and packaging

### T1: cargo-packager has no macOS helper-app support

cargo-packager builds a single-executable `.app`; CEF's layout (framework
in `Contents/Frameworks`, five helper apps) needs a custom step. **Blocks:**
the macOS `.dmg` through cargo-packager. **Workaround (2026-10-20):** the
bundle is laid out by `cargo xtask bundle-mac` without cargo-packager; a
`.dmg` would be `hdiutil` over that bundle.
