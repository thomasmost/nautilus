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

### C9: Blink cancels a drag whose `dragstart` handler hides the source from hit testing

A page that marks the dragged element on `dragstart` with
`pointer-events: none` (what most drag-and-drop code and libraries do,
alongside fading it) could not be dragged at all: the page saw
`dragstart` and then `dragend` a millisecond later, and no drag ever
reached the embedder. `MouseEventManager::TryStartDrag` dispatches
`dragstart` and then calls `DragController::StartDrag`, which, on the
legacy path, hit-tests the drag origin once more and returns false
unless the hit lands on the drag source; `pointer-events: none`
guarantees it does not, and `TryStartDrag` then dispatches `dragend`
itself and gives up. Nothing on the embedder's side is consulted, so
this is neither CEF's nor the windowless path's doing. Chromium's own
fix is the `GenerateDragOverlayBeforeDragStart` feature, which builds
the drag overlay before `dragstart` runs and skips the second hit test;
its comment in `runtime_enabled_features.json5` names this case. The
feature is `status: "stable"` in Chromium, so current Chrome handles
these pages, but it is not on in CEF 154's build.
**Workaround (2026-11-05):** `on_before_command_line_processing` appends
`--enable-blink-features=GenerateDragOverlayBeforeDragStart`. Verified
by driving a real drag of such an element: without the switch the drag
is cancelled at once, with it the drop zone gets `dragenter`, `drop`
and the source gets `dragend` with `dropEffect=move`. Remove the switch
on the CEF bump where the feature is on by default.

### C10: a media request left pending across a loss of focus fails when it is allowed

With a `CefPermissionHandler` that answers
`OnRequestMediaAccessPermission` later (returns true, keeps the
callback), calling `CefBrowserHost::SetFocus(false)` on the windowless
browser while the request waits makes the eventual
`Continue(requested_permissions)` fail: the page's `getUserMedia`
rejects with `AbortError: Failed due to shutdown` (Chromium's
`FAILED_DUE_TO_SHUTDOWN`, which `MediaStreamManager` reports when a
request is removed while its response is pending), also when focus
was given back before the answer. Seen on macOS (CEF 154) in all
seven runs with a scripted `set_focus(0)` 0.3 to 3 seconds into the
wait, and in one of two runs where the window really lost focus (the
other, blurred within a second of the request, was still granted
eight seconds later). Answers with no blur in the log succeeded after
waits of 0, 3 and 9 seconds, and a stream already granted survives
the blur. Each failing run logged `process_requirement.cc: Unable to
derive validation category for current process` from the browser
process within about a second of the blur; the line also appears at
other moments and may be unrelated. What removes the request was not
found, and nothing was reported upstream.
**Workaround (2026-11-17):** the question is a modal dialog of the
system's (osmena's `media_prompt`), answered inside the handler of the
event that reported the request, so the window's change of focus is
delivered after the answer. Do not turn the question into something
that stays open while events flow (a prompt in the chrome, an
asynchronous sheet) without settling this first.

### C11: `tabs.group` and `tabs.discard` on an Alloy-style tab crash the browser process

CEF resolves a windowless (Alloy-style) browser for the extension APIs
that take a tab id: one fallback in `ExtensionTabUtil::GetTabById`
finds it. Nothing else in Chromium knows such a browser: it is in no
`Browser` window and no `TabStripModel`. `chrome.tabs.group({tabIds})`
and `chrome.tabs.discard(tabId)` look the tab up, succeed, and then
reach for its tab strip; the browser process dies. Seen on CEF
154.0.34 (branch 8037), macOS, 2026-11-17: the test extension's last
record is the call, then the process is gone. CEF's maintainer has
said general tab API support for Alloy style is not planned. No
upstream issue found.
**Seen from an extension page (2026-11-18, macOS):** an
extension page opened as a tab gets its own Alloy tab id from
`chrome.tabs.getCurrent()`, and `chrome.tabs.group` on that id crashes the
browser process inside Chromium, after the page's other native calls return.
An id no tab has gets `No tab with id`, and a page with no script does not
crash. The crash is there with the bridge's code never called.
**Workaround:** a script ahead of each extension's service worker, and the
namespaces of each extension page, replace `chrome.tabs`, `tabGroups`,
`windows` and `sidePanel` and answer from Nautilus's own model, so these
calls never reach Chromium. An extension page's namespaces are replaced at
the first change to its document, before its first script runs
(`bridge/js/page_defer.js`). The engine refuses `tabs.group` and `ungroup`
for a bridged caller. Until a page is bridged an extension that calls either
crashes Nautilus.
**Blocks:** tab groups for extensions, unless bridged.

### C12: the window and tab-strip APIs do not see Alloy-style tabs; `tabs.create` opens a Chromium window

`tabs.query`, `windows.getAll`, `windows.getCurrent` and
`tabGroups.query` list only the hidden management browser; no
`chrome.tabs` event fires for a Nautilus tab; a tab's `windowId` is -1
and its `index` -1. `tabs.get`, `scripting.executeScript`,
`tabs.sendMessage`, `tabs.update({url})`, `tabs.reload`,
`webNavigation` and `chrome.debugger` do work by tab id. The id is
Chromium's session id, different on every launch, and is not
`CefBrowser::GetIdentifier()`. `tabs.create` makes Chromium open a
Chrome-style window for about 100 ms before `StrayClient` moves its
URL into a Nautilus tab and closes it; the id it resolved with is dead
(`tabs.get` and `scripting.executeScript` on it hang or fail). No
upstream issue found; same cause as C11.
**Workaround:** the extension bridge (C11), with a tab-id table joined
from each tab's DevTools target id and `chrome.debugger.getTargets()`
called by a helper extension (`bridge/table.rs`). Phase 1 answers
`tabs.query` for a bridged extension; the rest is Phase 5.

### C13: `sidePanel.open` has nowhere to open

`sidePanel.setOptions` and `getOptions` store and return options.
`sidePanel.open({tabId})` rejects a Nautilus tab ("No tab with id") and
`open({windowId})` for the management window resolves and shows
nothing: the side panel is a feature of Chrome's browser window, and a
windowless tab has none. A call without a user gesture is refused
before either. No upstream issue found.
**Workaround (planned):** osmena's side panel, with `chrome.sidePanel`
bridged (Phase 6).

### C14: CEF's external message pump is not asked for all of Chromium's work

With `external_message_pump` on, CEF asks for work through
`on_schedule_message_pump_work`, and the application's event loop
spins it when woken. CEF's header on `master` says the application must
also run the work unasked, about every 33 ms ("The application must also
schedule CefDoMessageLoopWork() calls when no callback is received",
`cef_browser_process_handler.h`); the 154 header does not say it.
The mechanism: CEF's `MessagePumpExternal::Run` stops after a 10 ms
slice with no callback, and Chromium's work deduplicator then takes a
`DoWork` to be pending and does not ask again. Related: CEF issue 2968
(the replace rule for delayed requests), and 4332 (a second starvation
in the same pump on macOS, fixed on master 2026-11-10).

Observed on macOS, CEF 154.0.34, with the window not in front (the
side-panel scripted run `ci/side-panel/run.sh pump`): extension calls
were not answered for seconds, and for the whole run in one case. With
the tick off, the instrumented build logged `no spin for 137354 ms; CEF
asked 0 time(s) since it ended; timer pending: false` and the latency
run did not finish (`task11/pump-tick0-run1`); a second tick-off run
failed the same way (`task12/tick0-control`). The replace rule with a
600000 ms interval, which leaves the timer pending and asks for nothing
sooner, also failed: two extension calls took 20005 and 20010 ms
(`task11/pump-tick600000-run1`). So a timer that CEF does not ask for,
at the interval the header names, is what cleared it.
**Workaround (2026-11-17):** the message pump runs CEF's work 30 times a
second when nothing else has asked, after every spin (`PumpScheduler`
in `engine.rs`: each delayed request is clamped to that interval, a
later request replaces the pending one, and the timer is armed after
every spin). Measured with `ci/side-panel/run.sh pump` (202 extension
calls each, the window unfocused): with the tick on, eight runs passed,
max 65 to 90 ms and p95 63 to 64 ms, and one failed (`task12/fix-run1`,
a hidden-tab stall); with it off, the two runs above failed. The
hidden-tab stall is a separate issue and is not upstream's: a tab behind
another is hidden, and a debugger's call to a hidden renderer waits for
the renderer's answer. Nautilus keeps a tab painted while an extension
debugs it: `bridge.js` reports each `chrome.debugger` attach and detach,
and `bridge/debugger.rs` keeps the set. An earlier version keyed this on
CEF's DevTools-agent callback, which fires for the engine's own
`Target.getTargetInfo` probe at every tab's creation, so no tab was ever
hidden. The run that showed it logged, for each tab, `C1PROBE attached
TabId(1) hidden=false` and then `hide TabId(1) dev_tools_attached=true
was_hidden(1)=false` (2026-11-18, three tabs). No upstream report was
looked for. In a build with the `test-hooks` feature, `NAUTILUS_PUMP_TICK_MS=0`
switches the tick off to reproduce; `NAUTILUS_PUMP_LOG=1` reports a pump
that has not spun. The idle cost of the tick was about 0.5 % CPU and
416 wake-ups against 0.0 % and 201 without it (sampled in the session;
no log kept). Not tested: lost wakes in osmena's `WinitWaker::wake` and
App Nap, which the logs do not point to; no run was made with the
window focused. Re-check on a CEF bump whether the header still asks
for it.

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

### C15: a popup widget is never painted while two other windowless views are

With GPU compositing, each windowless view is read back through a
video capturer of its own (`CefVideoConsumerOSR`, made in
`CefRenderWidgetHostViewOSR::ShowWithVisibility`), and so is a popup
widget, the list of a `<select>`. While two tab views are visible
(not `WasHidden`: two windows, each showing a tab), a popup widget in
either gets `OnPopupShow(true)` and `OnPopupSize` and then no
`OnPaint(PET_POPUP)` at all, for as long as it is open;
`Invalidate(PET_POPUP)` only asks the same capturer for a frame and
changes nothing, and neither does lowering the other view's
`windowless_frame_rate`. With one visible view the widget paints,
with a second view hidden it paints, and with
`--disable-gpu-compositing` (frames through the software output
device, no capturer) it paints whatever is visible. The bridge's
hidden pages (16 by 16, one frame a second) count too, though not
one for one: a tab, the helper's page and a relay page, all visible,
still paint the widget, and the same three with a second window's
tab told it is hidden at `OnPopupShow` do not (2026-11-23, 4 opens of
4, where the same run without an extension painted every one). With
the pages told they are hidden as well, the widget paints, with two
windows and with three. So no count of visible views explains what
was measured; the only arrangement relied on is one visible view and
its widget. Nothing in CEF's two files is shared
between browsers, so the limit is expected to be in viz's capture;
it was not traced further. Seen with CEF 154 on Windows, on a local
page with one `<select>`, 2026-11-22; macOS was not tried and is
expected to be the same (the report of 2026-11-19 came from a Mac
with two profiles open). **Blocks:** every `<select>` (and any other
popup widget) whenever a second window is open. **Workaround:**
`popup_pause.rs`: from `on_popup_show(true)`, which comes before the
widget's capturer exists, to `on_popup_show(false)`, every other tab
view that is painting, in every window of every profile, is told it
is hidden (`was_hidden(1)`), and shown again after (`show_view`, see
C16). The bridge's hidden pages are told they are hidden when they
are made and never shown (`bridge/hidden_page.rs`, since 2026-11-23;
for one day before that they were paused with the tabs). The cost:
while a list is open the other windows keep their last frame, and
their pages see `visibilitychange`. A windowless browser added later
must be listed there (`track`) or be hidden for as long as it lives,
or it brings the fault back. **Still open (2026-11-23):** with four
windows, each showing a tab, and no extension, the list was announced
and never painted in 4 opens of 4 though the three other views were
paused; with three windows, with and without an extension, it painted
every time. Not traced.

### C16: a windowless view shown again after its surface was evicted never paints

While a windowless view is hidden (`WasHidden(true)`) Chromium may
evict its surface: `CefRenderWidgetHostViewOSR::is_evicted_` reads
true for it. It does so as soon as two other windowless views are
showing, whatever they are: two other windows' tabs, or one other
window's tab and the extension bridge's pages (16 by 16, never
looked at, but showing until they are told otherwise). Told it is
shown again (`WasHidden(false)`, `Invalidate`), the evicted view
gets no surface: the page reads `visibilityState` as `visible`, runs
its timers, and receives no animation frame, no paint and no input,
for as long as it lives. It is the same with
`--disable-gpu-compositing`, so it is not C15's capturer. `WasResized`
at that moment brings the view back, and so does a real change of
size, unless a resize reached the view while it was hidden: CEF
allows one resize in flight (`hold_resize_`) and ends it at the next
frame, a hidden view has no next frame, and from then on every
`WasResized` only sets `pending_resize_`. `NotifyScreenInfoChanged`
asks for the same synchronisation without that hold and brings the
view back in every case tried. Seen with CEF 154 on Windows,
2026-11-23: in the owner's browser (two profiles' windows and Proton
Pass in each; every tab of the second profile dead after its first
turn behind another tab, both of its views with `hold_resize_` and
`pending_resize_` set, read from a dump with CEF's symbols), and in
scripted runs where a page counts its own animation frames (three
windows without an extension; two windows with one). One window with
an extension never showed it, which is why it waited for the day
every profile came into one process. macOS was not tried. **Blocks:**
every tab switch once two other views are showing. **Workaround:**
`delegate::show_view`, the one way a view is told it is shown again
(`TabHost::show`, the end of a popup widget's pause, a debugger
attached to a hidden tab): `was_hidden(0)`, `notify_screen_info_changed`,
`invalidate`. And the bridge's pages are hidden from the start, so
they hold no surface against the tabs.

### C17: the global request context is the profile a Chrome-style window last used

CEF gives the global request context (a browser created with no
request context, when `cache_path` is the root) the profile
`ProfileManager::GetInitialProfileDir` names
(`ChromeBrowserContext::InitializeAsync`, `GetPrimaryUserProfile`),
and Chromium answers that with `profile.last_used` from `Local
State`, not with `Default`. Chromium sets that preference from its
own Chrome-style windows, and the extension manager's hidden window
is one in every profile. So once a profile other than the default one
had been open and was the last to have such a window, the next
launch's global context was that profile: the default profile's tabs,
extension manager and bridge ran in `profile-<name>/`, sharing its
cookies, storage and extensions with that profile's own windows, and
`Default/` was not opened at all. Seen with CEF 154 on Windows,
2026-11-23: the owner's Personal (default) and Work windows signed in
to the same Google account, `Default/` not written since the start
while `profile-work/` was, `profile.last_used` reading `profile-work`;
and in scripted runs on a scratch `--data-dir` (default, then work,
then default again: the third run wrote 49 files under
`profile-work/` and none under `Default/`). macOS was not tried; the
code is not per platform. **Blocks:** separate profiles, from the
second launch after another profile was open. **Workaround:**
`--profile-directory=Default` in `on_before_command_line_processing`,
which Chromium reads before the preference. Re-check on a CEF bump
with the three runs above.

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
