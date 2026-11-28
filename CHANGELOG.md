# Changelog

All notable changes to Nautilus are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Nautilus
has no releases yet; `Unreleased` holds everything on `main`.

## [Unreleased]

### Changed

- Chromium 156 (CEF 156.0.31, Chromium 156.0.8078.100; it was 154),
  with two milestones of Chromium's security fixes. A start no longer
  asks `accounts.google.com` for the profile's Google accounts.

- Three Chromium features are off, after Thorium's defaults. Google's
  AI Mode for Chrome's address bar, which Nautilus has no use for: with
  it on, Nautilus asked `www.google.com` at every start whether the
  profile is eligible, with the profile's Google cookies, and no longer
  does. And the Privacy Sandbox's advertising override with its
  `<iframe>` form: pages no longer get the fenced frames that ad
  auctions render in (`HTMLFencedFrameElement` is gone).

- The profile avatars are redrawn as plain silhouettes that read on
  the toolbar's small disc, where the outlined emoji they had been
  since the palettes turned to grey there. The whale is the one
  profiles had before the palettes. The emoji faces are their features
  on the disc itself. Every avatar keeps its name and its place in the
  dialog (osmena 7e4ebfa).
- The toolbar's and the strip's glyphs are drawn on one grid in one
  weight and are sharp at 100% scale. The extensions button is a
  puzzle piece in outline, like the star beside it; the Launch
  Settings' gear is a cog; the find bar's previous, next and close and
  the downloads popover's close are glyphs in a disc.
- Every profile is a window of the one Nautilus. Opening another
  profile (the Profiles menu, "Open in profile", a new or guest
  profile) no longer starts a second Nautilus: its window opens in the
  running one, so on macOS there is one Dock icon and one entry under
  Cmd+Tab, the menu bar follows the profile in front, and Cmd+` moves
  between the windows of all profiles. A profile that is open already
  comes forward. Closing a profile's last window closes that profile
  and leaves the others; Quit quits them all (each asks first while it
  has downloads going), and the next start opens the profile that was
  in front, with the others that were open when their settings bring
  their windows back. Starting Nautilus while it runs (from a
  terminal, a shortcut, with or without `--profile` and addresses)
  hands the request to the running one and exits. Each profile still
  has its own cookies, storage, cache and extensions.
- Where Chromium keeps a profile's data: one Chromium now runs every
  profile, with the default profile's `chromium/` directory as its
  root. The default profile's data stays where it was. Any other
  profile's cookies, storage and cache move, the first time it opens,
  from its own `chromium/Default/` to
  `profiles/default/chromium/profile-<name>/`; its extensions stay in
  its own `chromium/extensions/`. On Windows this signs such a profile
  out of websites once: Chromium encrypts cookies with a key kept
  beside its root directory, and the profile changes roots. Going back
  to an older build after the move shows those profiles empty (their
  data is in the new place); the default profile is unaffected.
- A profile cannot be opened while a Nautilus from before this change
  still has it open: quit that one first (the message says so).
- The scripted runs (`ci/side-panel/run.sh`, `phase5.py`) keep their
  profiles in a data directory of their own (`--data-dir`), so they no
  longer touch the profiles of the Nautilus in use, and are not handed
  to it.

- A strip with more tabs than fit says which way the rest are: the
  row fades into the strip at an end with tabs scrolled out of sight
  beyond it, instead of being cut off (osmena).
- Tabs narrow to 40 points, an icon between its paddings, before the
  strip scrolls, where every tab stopped at 64 (osmena). The active
  tab keeps 64, so its close control always shows, and a tab that is
  recording keeps room for its mark. A tab grows to its width when it
  is selected, its close control fading in, and shrinks back when
  another is. A narrow inactive tab is closed with the middle button
  or once selected.
- The strip's "+" no longer scrolls out of sight when there are more
  tabs than fit, and the window can always be moved: a stretch right
  of the "+" is kept free of tabs as the drag area, marked by a grip
  of dots that lights under the pointer (osmena).
- The Launch Panel's field completes to a page, without the `?query`
  and `#fragment` an address in the history may carry (osmena): `sho`
  completes to `shop.example/list`, not to
  `shop.example/list?page=2&sid=…`. The matches under the field still
  list the full addresses.
- The hooks scripted runs drive the browser with are in no packaged
  build: `NAUTILUS_TEST_CONTROL`, `NAUTILUS_TEST_PAGE_SCRIPT`,
  `NAUTILUS_PUMP_LOG` and `NAUTILUS_PUMP_TICK_MS` do nothing unless the
  browser was built with the `test-hooks` cargo feature (`cargo run
  --features test-hooks`, `cargo xtask bundle-mac --features
  test-hooks`). A build without it logs a warning when one is set.
- The chrome's look, after Thorium with Zen's softness (osmena): the
  toolbar's Back, Forward, Reload and Stop are painted glyphs in round
  buttons that fill only under the pointer and fade when they cannot
  act; the address bar is a filled pill that turns white with a ring
  in the profile's colour while it has the keyboard. Each profile's
  colour now gives an accent, the deep shade of its swatch's family
  (its pastel in a dark system theme), to every focused field, the
  text selection, the picked suggestion and the lit chips; popups sit
  on a surface tinted by the colour with a soft shadow. The Launch
  Panel fades and rises in on a surface of its own, its field carries
  a magnifier, its tiles are cards on rows as tall as their tallest
  tile, and Most visited is a shelf of shortcut discs across the
  panel, so the mosaic has no hole.
- The toolbar button of an extension that has a popup and has also
  asked for its side panel on a click
  (`sidePanel.setPanelBehavior({openPanelOnActionClick: true})`) opens
  and closes the panel, as in Chrome; it used to open the popup.
- An extension's toolbar action with no popup now shows its button in
  the toolbar and the puzzle menu; pressing it sends the click to the
  extension's worker through the extension bridge.
- A loaded unpacked extension's copy in the profile no longer includes
  dot-files and dot-directories (`.git`, `.DS_Store`). An extension whose
  worker's entry is under one fails to load, with a message that says so.
- An unpacked extension that an earlier build loaded from the user's own
  folder is moved into a copy in the profile without a gap: the original is
  switched off while the copy is made, and the copy takes the state the
  original had. An extension with a `key` keeps its id. An extension without
  one gets a new id: the copy is listed beside the original, which stays in
  the list switched off, and the data stored under the original's id stays
  with it. Chromium does not let the engine remove the original, so it is
  removed by hand. If the copy cannot be made, the original stays switched
  off, with a message.
- An extension whose bridge cannot be put in is switched off with a message,
  and an extension page whose bridge cannot be put in refuses Chromium's
  `tabs`, `windows`, `tabGroups`, `commands` and `sidePanel` rather than
  leaving them callable.
- `captureVisibleTab` captures only a web page (`http`, `https`, `ftp`,
  `data`) or the extension's own pages. A browser page, another extension's
  page or a file page is refused, with a message that names its address.
- A message from a tab is delivered to an extension's listeners at once, with
  the tab's strip fields from the context's cache. A context whose listeners
  do not answer sends nothing, so a popup's or an offscreen document's answer
  is the one the sender gets, and a message no context answers settles as
  Chromium settles it. A worker's cache follows the tabs the browser opens,
  renames and closes, so a tab opened after the worker started (an extension
  page shown as a tab among them) carries its strip fields from its first
  message.

### Added

- Extension side panels. An extension that uses `chrome.sidePanel`
  shows its page in a panel docked at the right of the window, beside
  the page, under a header with the extension's icon and name and a
  close control (osmena 13b073b draws it; the page in it is one more
  off-screen Chromium browser here). The extension opens the panel
  itself (`sidePanel.open`), for the window, where it stays beside
  every tab, or for one tab, where it shows only while that tab is the
  active one; an extension that asked for it (`setPanelBehavior`) has
  the panel opened and closed by its toolbar button. `chrome.sidePanel`
  (`setOptions`, `getOptions`, `open`, `close`, `setPanelBehavior`,
  `getPanelBehavior`, `getLayout`, `onOpened`, `onClosed`) is answered
  by Nautilus, and `chrome.runtime.getContexts` lists an open panel as
  a `SIDE_PANEL` context. The panel is 360 points wide and is not
  drawn in full screen or in a window too narrow for it. It has no
  context menu, takes no text from an input method and cannot use the
  microphone or the camera; a window it opens becomes a tab; and
  `sidePanel.open` is not held to a user gesture, as Chrome holds it.
  A panel is not brought back after a restart; what an extension chose
  with `setPanelBehavior` is.
- Tab groups for extensions, as tab folders. `chrome.tabs.group` makes
  a folder in the strip, expanded and named "<extension name> Session"
  until the extension titles it; `chrome.tabGroups.update` renames,
  collapses and expands it, `chrome.tabs.ungroup` takes tabs out, and a
  folder whose last tab goes is gone. `tabGroups.get` and `query` list
  the folders, one made by hand among them, and `tabGroups.onCreated`,
  `onUpdated` and `onRemoved` fire whoever made the change. Folders
  have no colour: every group reports grey and a colour passed to
  `update` is ignored. `tabGroups.move` is refused, and tabs are
  grouped within their own window only.
- Extension keyboard shortcuts. A shortcut an extension declares in its
  manifest (`commands`, with the key it suggests for this platform)
  runs its `chrome.commands.onCommand` listeners with the active tab,
  starting its service worker if it had stopped, or acts as a click on
  its toolbar button for `_execute_action`. The browser's own shortcuts
  win: a chord the keymap uses stays the keymap's, and Settings lists
  each extension's shortcuts and marks the ones that are not active
  (osmena). `chrome.commands.getAll` lists the commands with the
  shortcuts they declare, also for one that is not active.

- A tab pulled more than a tab's height off the strip comes away from
  it, and let go there it opens in a window of its own under the
  pointer, the size of the one it left. Dropped on another window's
  strip it lands between the tabs where it was let go, not at the end.
  A folder is dragged by its chip, whole, along the strip or off it
  into a window, keeping its name. A tab held at an end of a crowded
  strip scrolls it, and Escape puts a dragged tab back. A tab that
  changes window still loads its page again (osmena 880642d).

- A tab whose page is using the microphone or the camera carries a
  recording mark: a red dot left of its icon, slowly fading out and
  in, with the device named under the pointer (osmena draws it;
  Chromium's `OnMediaAccessChange` reports the use here). The dot
  stays however narrow the tab gets, goes when the page lets the
  device go, and a collapsed folder's chip carries it for its tabs.
- The icon at the left end of the Launch Panel's field says what Enter
  will open (osmena): the site's own favicon for a page, a magnifier
  for a search, sparkles for a question.
- The Launch Panel's field asks as well as searches (osmena): tap
  Shift twice and the field's text becomes a question, opened by
  Enter in Claude or ChatGPT, whichever the Launch Settings' new
  "Ask with" row names; the chip at the field's right end says which.
  Shift twice again goes back to searching.
- Extensions see the browser's own tabs and windows. `chrome.tabs` and
  `chrome.windows` are answered from the tab strip in service workers and
  in extension pages (a tab or an action popup): `tabs.query` lists every
  tab of every window in order with its window, index, active state and
  folder; `tabs.create`, `remove`, `update({active})`, `move`, `duplicate`
  and `windows.create` and `remove` act on the strip; the tab and window
  events fire as the strip changes; and, for a tab that has loaded, a
  tab's id is the one `chrome.scripting`, `chrome.debugger` and
  `chrome.webNavigation` accept. A restored tab that has not loaded yet is
  listed too and takes its real id when first shown (`tabs.onReplaced`).
  `tabs.discard`, which used to crash the browser, now answers without
  reaching Chromium, and `tabs.group` makes a tab folder (above). An
  unpacked extension is run from a
  copy inside the profile: choose "Load unpacked…" on the same folder again
  to pick up edits.
- Each profile gets a small helper extension, written into it on start,
  that the extension bridge uses to find DevTools targets. It is loaded
  only while an extension bridged by the bridge is on.

- The Launch Panel's field completes an address ahead of the typist
  (osmena). Before the first space, the most visited page whose
  address starts with the typing stands in the field, the rest of its
  address selected after the caret (`github.com` after `gi`), so
  typing on replaces it and Enter opens the page. Backspace drops the
  completion and leaves the typing, and a space makes the text a
  phrase; either way Enter then searches. The dropdown's first row is
  always what Enter opens: the completed page with its title, the
  typed address with "Open", or the words with "Search Google" (the
  engine's name), lit while no match is picked, with the matches under
  a hairline; while the text is a search the engine's name sits in a
  small chip at the field's right end.

- Blocked sites. A profile keeps websites it refuses to show, by
  registrable domain, so blocking `youtube.com` covers every subdomain:
  "Block on this profile" in a tab's menu (behind a confirmation) or
  the "Blocked sites" section of the settings window, which is also
  the only place a site is unblocked. Typed addresses are stopped by
  the shell; links, scripts and redirects are cancelled in Chromium's
  before-browse hook before any request goes out, and the tab shows a
  block page over `about:blank` with the attempted address in the
  address bar. Back returns to the page before; Reload re-rolls the
  page's line; unblocking brings the tabs back (osmena, with
  `on_before_browse` and `location.replace` here).

### Fixed

- `chrome.debugger.onDetach.hasListeners()` answers for the
  extension's own listeners. It was always true in an extension with
  the `debugger` permission, in its worker and in its pages, because
  the browser's own listener was counted.

- `chrome.debugger.attach` and `detach` called from an extension's
  page (a side panel's among them) no longer wait behind that page's
  other calls to the browser, which could hold them for seconds while
  a tab was opening.

- An extension's keyboard shortcut is read as Chrome reads it. One
  written with spaces around its parts (`Ctrl + Shift + Y`) or with a
  modifier named twice was not taken as a shortcut at all; and three
  kinds Chrome refuses were taken: Option with Command on macOS (as
  Ctrl+Alt elsewhere), more than two modifiers, and a media key for
  the toolbar button's own command.

- `tabGroups.update` on a group that is removed while the call is
  under way rejects at once with Chrome's `No group with id`, where it
  waited five seconds and then answered a group that no longer
  existed.

- `tabGroups.update` with a title that has spaces around it answers at
  once, with the title the folder gets (trimmed). It waited five
  seconds and answered the title as it was written, which no folder
  had.

- An extension's worker that lists the tabs as it starts is answered
  with the ids Chromium knows them by. Started early (an extension
  installed into a running browser, or a quick worker at launch) it
  could be told there were no tabs, or be given placeholder ids that
  Chromium's own `debugger`, `scripting` and `tabs.sendMessage` refuse
  with "No tab with id". Its tab calls now wait, two seconds at most,
  until the ids are known.

- The first message a side panel's page sends as it loads reaches the
  extension's worker without `sender.tab`, as every later one did and
  as in Chrome, where an extension tells its panel from its content
  scripts by that. The page now holds `runtime.sendMessage` for a
  moment, until the worker has been told which Chromium tab the panel
  is; before, the message got there first. A script that wraps
  `runtime.sendMessage` while that hold is in place (a library does it
  at load) keeps its wrapper once the hold ends. A call whose argument
  shape Chromium refuses throws at once, as Chromium's does; any other
  argument error comes after the hold. `chrome.tabs.create` answers with
  the new tab's Chromium id once the page has reported it, so an
  extension does not keep an id that the tab then replaces.

- A side panel's page that sends a message and closes itself, with the
  message still held (the hold above), no longer loses that message: the
  held messages go when the page is hidden for good, and when its own
  `window.close()` asks the engine to close it.

- `runtime.getContexts` in an extension with a side panel answers for
  the moment it was called. The side panels in the answer were read
  only once Chromium's part had come back, so an extension that asks
  for its contexts, opens its panel and then reads the answer could be
  told the panel was already open (the Claude extension decides by
  that whether to put the tab in its group). Not seen to happen.

- An extension that declares a keyboard command and is granted an
  optional permission while it runs (`permissions.request`) gets the
  new API in its running worker and pages for most APIs, as it did
  before commands were bridged: `cookies`, `downloads`,
  `notifications`, `scripting` and the others whose names sort before
  `sidePanel` (or `tabs`, for an extension without a side panel).
  Replacing `chrome.commands` had stopped Chromium's update of a
  running extension's APIs at `commands`.

- An extension's own scripts can no longer reach Chromium's `tabs`,
  `windows`, `tabGroups` or `sidePanel` functions through the
  bridge's object. `__nautilusBridge` carried them as a member
  (`native`), so any script of a bridged extension could call
  Chromium's own `tabs.group`, which ends the browser on these tabs.
  They are kept out of reach now.

- An extension context in which one of the replaced APIs cannot be
  put in no longer keeps Chromium's own for the ones after it. Each of
  `tabs`, `windows`, `tabGroups`, `commands` and `sidePanel` is put in
  on its own, in an extension's pages and in its service worker, and
  one that cannot be is refused on `chrome` and on `browser`; before,
  the first failure left every later one Chromium's, whose
  `tabs.group` ends the browser. Not seen in a real extension.

- An event for an extension's service worker that the worker did not
  acknowledge is sent once more, as it was meant to be. The second
  try was queued under a name read from memory that closing the
  extension's bridge page had just freed, so it was dropped, and the
  read itself was unsafe. Seen with the Claude extension started
  again on a profile that already had it: its log line named the
  extension as 32 bytes of 0xCD.

- Extensions that reach the browser through `browser.*` see
  Nautilus's tabs and windows. Chromium 156 gives an extension a
  `browser` object beside `chrome`, and only `chrome`'s `tabs` and
  `windows` were Nautilus's: an extension written against `browser`
  (Proton Pass reaches its tabs that way) was shown none of the
  browser's tabs, could open a Chromium window of its own with
  `browser.tabs.create`, and could end the browser with
  `browser.tabs.group`. `browser.tabs`, `windows`, `tabGroups`,
  `commands` and `sidePanel` are now the very objects `chrome` has.
- A tab that an extension debugs from one of its pages (a side panel,
  a popup, a page in a tab) and that is behind another tab answers at
  once. Only a debugger attached from the extension's service worker
  kept such a tab painting; attached from a page, every input command
  to the tab waited five seconds.
- The tab an extension's `action.onClicked` listener is given has its
  window and its place in the strip; it had `windowId` -1 and `index`
  -1.
- `chrome.runtime.getContexts` no longer lists a page of the browser's
  own (the extension bridge's) among an extension's contexts.

- A folder's chip no longer sits on top of one of its window's tabs.
  A folder and a tab that happened to carry the same number inside
  the browser (the first folder made in a window, and its first tab)
  were slid along the strip as one thing, so both were drawn halfway
  between their two places, the chip over the tab and the row pushed
  right (osmena 13b073b).

- On macOS a tab can be dragged. A press on most of a tab moved the
  whole window instead: the title bar macOS keeps, unseen, over the
  top of the strip took it. The window is now moved by the grip and
  the empty stretch right of the "+", as on Windows. A drag also no
  longer ends when the pointer crosses the window's edge, and a tab
  that widens as it is picked up stays under the pointer (osmena
  880642d).

- PDFs are shown again, as a tab's document and inside a page's frame.
  Since extensions' pages were given the browser's tabs and windows,
  Chromium's own PDF viewer, which is an extension page too, was given
  them as well and refused, so a PDF came up as an empty dark page.

- The default profile keeps its own cookies, storage and extensions
  after another profile has been open. Since every profile came into
  one Nautilus, a start that followed a session with a second profile
  could open the default profile's windows on that profile's Chromium
  data: both were signed in to the same accounts, and what was done
  in the default profile's windows was kept with the other profile's.
  Nothing of the default profile's own data was lost; it was not
  opened in such a session, and is as it was left.
- The toolbar's Reload is an arrow again, an arc turning clockwise
  into an arrowhead; it had been drawn as a ring with a tick on it.
- A `<select>` opens its list when more than one window is open. With
  two windows showing a page each (two profiles, or two windows of
  one), a click on a `<select>` took the focus and showed nothing:
  Chromium announced the list and never painted it. While a list is
  open the pages in the other windows now stop painting, keeping
  their last frame, and go on when it closes (C15 in
  `docs/upstream-issues.md`).
- A tab comes back when it is shown again. With two profiles open and
  an extension installed (or with three windows open), a tab that had
  been behind another, or in another window while a `<select>`'s list
  was open, froze for good the moment it was shown: its last picture
  stayed, it took no click and no key, and resizing the window did
  nothing to it; the tabs of a window went one after another as each
  took its turn behind the other. Chromium drops a hidden page's
  surface once two other pages are showing, an extension's unseen
  pages counting among them, and did not give it a new one. Every tab
  is now given one when it is shown, and an extension's unseen pages
  no longer count (C16 in `docs/upstream-issues.md`).
- A `<select>` opens its list with a second window open when an
  extension is installed too. The first fix held only in a profile
  without extensions: each extension keeps a page nobody sees, which
  Chromium counted as it counts a window's page, so with Proton Pass
  (or any extension) and a second window the list still never
  appeared. Those pages no longer count as showing at all.
- Two fingers spread or pinched on a touchpad zoom the page, a step
  at a time, as Ctrl (Cmd) and the wheel do (osmena). Before, the
  gesture scrolled the page on Windows and did nothing on macOS.
- The Launch Panel opens a typed address as typed (osmena). Typing
  `docs.rs`, or a whole URL, and pressing Enter went to the most
  visited page starting with it, often one deep inside the site; the
  field now completes only the host still being typed, or the very
  page typed, and never adds a path to an address.
- The extension switch on the manager's list did nothing: the engine sent a
  property Chromium rejects (`enabled`), so an extension was never switched on
  or off. It now uses `chrome.management.setEnabled`.
- `tabs.query` with a `url` filter finds a file page the extension may see
  (`file:///*`, `<all_urls>`) when it has the `tabs` permission; before, a file
  page was never found by that filter.
- A page shown as a tab whose Chromium tab id is 2 000 000 000 or more gets its
  row: the engine refused such an id as if it were one of its own, so the page's
  `sender.tab` and its window were missing.
- `captureVisibleTab` refuses the Chrome Web Store, as Chrome does.
- An extension without a service worker (a popup-only extension, or one
  with a background page) gets `chrome.tabs` and `chrome.windows` in its
  pages. Its page calls used to be refused after a wait, and they queued behind
  other extensions' calls.
- A page's listeners receive only the events the page registered, and an
  extension's worker is started by an event only when it listens for that
  event.
- An extension page's calls and screenshots fail after 10 seconds instead of
  waiting for an answer that never comes.
- Replacing an unpacked copy no longer deletes the working copy when the new
  one cannot be moved in.
- The bridge's worker port is taken only from the extension's relay page, and
  the relay takes only its worker's port, so a content script cannot take the
  bridge's calls. A forwarded answer is acted on only for the methods the
  engine forwards.
- A Manifest V3 host pattern under `permissions` no longer grants access, and
  `<all_urls>` and host patterns no longer match file pages. Host names match
  without regard to case.
- A reply to an extension page's call goes only to the page of the extension
  that asked.

- Calls and recordings in pages work: a page that asks for the
  microphone or the camera gets a question from the system, "Allow
  <site> to use your microphone?" with Allow and Block, where before
  every request was refused without one (a Twilio call ended with
  "31401: The browser or end-user denied permissions to user media").
  A site you allow keeps the device until you quit Nautilus; a site
  you block may ask again. Sharing the screen is still refused.

- "Sign in with Google" (and any sign-in that opens a popup) never
  came back signed in, and opened two tabs, one of them blank for
  good. A page's `window.open` used to be cancelled and replaced by an
  unrelated tab at the URL, so the call returned null, the new page
  had no `window.opener` to hand the result to, and a page that opens
  a blank named window and then sends the sign-in page to that name
  got a second tab. Chromium now creates the popup itself, shown as a
  tab: the opener gets its window, the popup its opener, and a window
  is found again by its name.
- Alt codes typed nothing on Windows: Alt+0151 on the numeric keypad
  now types an em dash, in a page and in the address bar. The shell
  delivers the character winit dropped (osmena); here the commit of
  text to a page names CEF's "replace nothing" range, without which
  `ImeCommitText` inserted nothing.
- Tab titles sat a pixel or two above the centre line of their
  favicon since they were made smaller: they were centred by a band
  that counted descenders in, which few letters have. They are centred
  by their capitals now, descenders hanging below as on a page
  (osmena).
- A fresh profile on Windows always showed the weather in Celsius,
  wherever it was: the units follow the system's locale, and osmena
  looked for it only where POSIX keeps it. The locale comes from the
  system now (osmena).

- The Launch Settings gear read as a steering wheel; it is a cog now.
  The weather tile's cloud had a dark bow tie across its middle, and
  its high and low read "H 20° L 14° °C" with the unit left dangling
  (osmena).
- A drag attempt on a draggable element left the page in its dragging
  state for good: the element stayed faded, hidden or deaf to the
  pointer (whatever its `dragstart` did), and never hovered again.
  Nautilus answered Chromium's drag with "abort", which ends it
  without the page's `dragend`. Drags are now carried out within the
  page (below), and a drop on the chrome, outside the window, or
  Escape mid-drag cancels one with `dragend` all the same.
- A page that marks the dragged element with `pointer-events: none` in
  its `dragstart` handler, which most drag-and-drop code does, could
  not be dragged at all: Blink cancelled the drag a millisecond after
  announcing it, because it hit-tests the drag origin again and that
  property makes the hit miss. Chromium fixes this with a feature that
  is on in Chrome but not in CEF's build yet, which Nautilus now
  enables (C9 in `docs/upstream-issues.md`).
- A mouse button pressed in a tab that then closed stayed pressed for
  the window's other tabs until the next click.
- Extensions' calls into the browser, and anything else waiting on
  Chromium's main thread, could stop being answered for seconds at a
  time, or until the next input, while the window was not in front.
  Chromium does not ask an embedder for every piece of its work; the
  message pump now also runs that work 30 times a second without being
  asked, as CEF's own sample does.
- Loading an unpacked extension no longer follows symlinks: a symlink
  inside the extension's folder is skipped, where it was copied as its
  target, and a folder that links to an ancestor (or that contains the
  profile's extension store) no longer makes the load recurse forever.
  A link skipped that way is logged as a warning that names it, and an
  extension whose worker's entry is itself a link, or is missing, fails
  to load with a message saying so (before, the worker simply failed).
  A load that fails leaves no partial copy in the profile. A worker path
  in an extension's manifest that names a drive, a stream or a Windows
  device name is refused.
- A tab that an extension is debugging (its `chrome.debugger`) and that
  was behind another tab stopped answering input and screenshots, so
  each of the debugger's calls to it waited about five seconds. Such a
  tab is now kept painting while the extension debugs it, and hidden
  again when the last debugger goes. The bridge reports each attach and
  detach, by tab id or by target id (`attach({targetId})`), for every
  extension it is in (a store install, "Load unpacked…"), from the
  extension's service worker and from its pages.
  A background tab that no reported debugger is on stays hidden as before.
- A tab that an extension is debugging is hidden again when the extension
  is reloaded and the browser sees its debugger session end. Before, a
  reload could leave such a tab painted with nothing debugging it. The
  browser now asks the extension's worker which debugger sessions still
  answer after it starts again, and forgets the others. A worker that
  stops and starts again keeps its sessions, so its tab stays painted.
  A reload that leaves the extension's page open with no event is seen by
  the extension's worker target, which the browser reads about every two
  seconds while an extension's page is open; the page is opened again and
  the worker's own answer decides the tab.
- A tab that an extension is debugging stays painted when the browser can
  no longer reach the extension's worker. The browser forgets a debugger
  only when the worker says its session has ended, when the extension is
  off or gone, or when the tab closes. A check that times out or fails
  closes the page and keeps the debugger, and so does giving up on a page.
  Before, giving up hid a debugged tab.
- The browser no longer starts an extension's stopped worker to ask it who
  it is when the worker's port closes. An idle worker stays stopped, as
  Chromium's idle stop intends, and a debugged tab stays painted.
- A page of the extension bridge that does not load (an extension's relay
  page, or the helper's) is opened at most five times in a row, the gap
  between its five openings 250, 500, 1000 and 2000 ms. Then the browser
  gives up on it until its extension changes: it logs once that it has done
  so, and the extension's clicks are refused. Before, such a page was
  reopened four times a second for as long as the browser ran.
- On Windows the scripted-run control directory (`NAUTILUS_TEST_CONTROL`,
  for test runs only) must lie inside the user's profile folder; any
  other directory, or one that cannot be found, is refused with a warning
  and the hook stays off.

### Added

- Restored tabs show their favicons and fresh titles without being
  opened: after "Continue where you left off" brings a session back,
  the tabs that wait for a click are loaded out of sight one after
  another, half a second apart from left to right, each let go again
  once its title and favicon are in. A tab clicked before its turn
  loads as before; one clicked during its turn is simply shown. The
  loads count for nothing in the history. Until its turn, a restored
  tab shows the icon last seen for its site (osmena).
- With nobody having chosen units, the weather tile measures the way
  the country it is showing does: a device that turns out to be in the
  United States reads Fahrenheit without anyone saying so, and a place
  named in France reads Celsius. A choice, once made, stands wherever
  the device goes (osmena).

- Fahrenheit and Celsius are a row of their own in the Launch
  Settings, labelled "Temperature" and named in words, where they were
  two bare degree signs under the weather place (osmena).

- The Launch Panel's weather tile follows this device: opening the
  panel asks the system where it is, which is what asks you for
  permission, and shows today's conditions there. Naming a place on
  the tile still overrides it and asks the system nothing, and the
  Launch Settings move between the two. If the system will not say,
  the tile says so and offers to look a place up instead. Windows
  answers through its own location service; macOS cannot yet, so the
  tile asks for a place name there (osmena).
- Drag and drop within a page: a draggable element, a selection, a
  link or an image dragged with the mouse reaches the drop targets on
  the same page (`dragenter`, `dragover`, `drop`), with Chromium's
  image of it under the pointer and the no-drop cursor where nothing
  accepts it. Dragging out of the browser, between tabs or windows,
  and dropping files in from the desktop are not there yet.

### Changed

- Tab titles and folder names are a notch smaller, and sit on the
  centre line of their favicon instead of about two pixels below it.
- The active tab is shaped like Chromium's: rounded on top, flaring
  out at the bottom into a one-pixel line along the strip's edge, in
  the toolbar's colour so it reads as part of the toolbar. The other
  tabs are solid fills lifted halfway to white from the theme colour,
  and tab titles are slightly smaller and dark on them (osmena).
- The ☰ menu button sits at the toolbar's right end, right of the
  profile avatar, instead of on the tab strip (osmena).

### Added

- Guest profiles and the Manage Profiles dialog, from osmena: the
  Profiles menu behind the avatar now reads like Chromium's (the
  current profile and Edit…, "Other Nautilus Profiles" over the rest,
  then Add Nautilus Profile…, Open Guest Profile and Manage Profiles…).
  A guest is a throwaway profile deleted when its window closes;
  Manage Profiles… opens, edits or deletes any profile from one
  dialog, which is where Delete moved from the menu.

### Changed

- The icon has no black outline, and its tile is a continuous-corner
  rounded square in Apple's proportion. The macOS icon is drawn on
  Apple's icon grid (`assets/logo-macos.svg`: the 824 px tile inset
  in a transparent canvas, with the template's shadow), so it sits in
  the Dock at the size of other apps.

### Fixed

- macOS: one Backspace deleted two characters in page fields, and an
  arrow, Delete, Escape, Page Up/Down or a function key acted twice
  (its release reached the page as a second press; C8 in
  `docs/upstream-issues.md`). Modifier keys reached pages with their
  press and release swapped (Shift's `keydown` without `shiftKey`).
- macOS: typing into a page opened the emoji palette (on E) or the
  dictation prompt (on D) and swallowed that letter: the page's
  unhandled keys went to the menu bar as made-up events that the
  system's hidden "Emoji & Symbols" and "Start Dictation…" items
  matched (C7). Only Ctrl+Cmd+Space opens the palette now.
- macOS: the Dock showed the window icon's PNG, with its black border,
  over the bundle's icon while Nautilus ran; it shows the bundle's
  icon now (osmena).
- The Windows installer could ship without Chromium's runtime files
  when CI's build cache was fully warm (release build 32 did): the
  package job now copies them from the CEF distribution itself before
  packaging and fails if the installer is too small to hold them.

### Added

- macOS: Nautilus can be the default browser. The bundle claims the
  `http` and `https` schemes and HTML pages, so System Settings lists
  it; a bar under the toolbar offers to make it the default while it
  is not (Make default, Don't ask again, Not now), and Settings has a
  Default browser section with the button and the switch (osmena).
  Links clicked in other apps, and pages opened with Nautilus from the
  Finder, open as tabs; a link that launched Nautilus opens in place
  of the home page.
- The Launch Panel, from osmena: Ctrl+T opens a wide panel with the
  search-or-address field on top and tiles under it: recent Google
  Drive files (after "Connect Google Drive", in builds made with a
  Google client id), today's weather at a place you name, Ask Claude
  and Ask ChatGPT fields, and the five most visited sites. The gear
  opens the Launch Settings: the search engine, the question
  templates, the weather place and units, the Google account, and the
  tiles dragged to reorder or closed to remove.
- Session restore, from osmena: "Continue where you left off" in
  Settings brings the windows and tabs back where they were, loading
  each tab when first shown; a crash leaves an offer to restore the
  last session; "Restart to update" restores it too.
- Downloads, from osmena, over Chromium's download handler: files go
  to the Downloads folder (or the one Settings names) under unique
  names, or the save dialog asks when Settings says so; the toolbar
  button with its progress ring and the popover (Ctrl+J) with pause,
  resume, cancel, open, show in folder, retry and clear; the quit
  confirmation while a download goes; the list kept across runs. The
  status overlay no longer carries download progress.
- Page zoom, from osmena, over Chromium's zoom levels: Ctrl and + or − (or the
  wheel), Ctrl+0, the ☰ menu's Zoom submenu; every tab on the site
  follows, the site remembers its level, a chip beside the address bar
  shows it, and Settings sets the default.
- Find in page, from osmena, over Chromium's find: Ctrl+F opens the
  bar, every match is highlighted and the active one scrolled to, with
  the count, Enter/F3 and Shift+Enter/Shift+F3 step, "Aa" matches case,
  Escape closes.
- Bookmarks, from osmena: the star beside the address bar (Ctrl+D) with
  its popover to name a bookmark, move it between groups or remove it;
  the bookmarks bar (Ctrl+Shift+B, or Settings) with favicon chips that
  drag to reorder, an overflow chip and the other groups as drop-downs;
  "Bookmark this tab…", "Bookmark all tabs…", import and export of an
  HTML bookmarks file and per-group rename and delete in the Bookmarks
  menu; bookmarks among the address bar's and the launcher's
  suggestions, marked with a star and ranked above history.
- Updates over the air. Every green push to `main` is now a GitHub
  release with the installer, the macOS bundles, the symbols and an
  update manifest; a CI build of Nautilus checks it 15 seconds after it
  starts and every six hours, downloads a newer build in the
  background, and shows a green "Restart to update" pill in the toolbar
  (also in the ☰ menu, the macOS application menu, and the new Updates
  section of Settings, which has the switch for the automatic checks,
  "Check now" and the release notes). The restart installs it and
  reopens the same profile; on Windows the silent installer closes any
  other Nautilus window first. A build made outside CI never updates
  itself (osmena).
- Profiles can be named, coloured and given an avatar, and made and
  deleted from the browser: the avatar at the toolbar's end opens the
  Profiles menu (the macOS menu bar has the same), with Edit…, the other
  profiles, Add Profile… and Delete Profile. A profile may share the
  Default profile's bookmarks and copy its extensions; history is never
  shared. The window title names a non-default profile (osmena).
- The profile dialog offers its avatars in three tabs: "Sea
  creatures" (22, among them shark, seal, otter, squid, jellyfish,
  crab, lobster, shrimp, oyster and coral), "Emoji" (the 24 most used
  emoji, flat and monochrome) and "Assorted" (osmena).
- The theme colour is a grid of seven hues, each in a pastel, the
  earlier shade and a deeper, more natural one (Mint, Forest, Pine;
  Lagoon, Ocean, Abyss; …), in the profile dialog and the settings
  window; saved colours are unchanged (osmena).
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
