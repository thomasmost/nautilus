# The extension bridge's protocol

Protocol 1 (side-panel plan, Phase 1). What travels between the engine
and the bridge script in an extension's service worker. Nothing outside
`nautilus-engine` sees it. Phases 5 to 7 add calls and events here and
say so in this file; a change that an older `bridge.js` on disk would
misread raises the number in the port's name.

## The parties

- **The bridge** (`src/bridge/js/bridge.js`): written into each bridged
  extension as `nautilus-bridge/bridge.js` and loaded ahead of the
  extension's own worker by `nautilus-loader.js`, which the rewritten
  manifest names as `background.service_worker` and which sits beside
  the original entry file. It sets `self.__nautilusTransport =
  {protocol: 1, portHidden}` (`self.__nautilusBridge` is kept free for
  the namespaces' object Phase 5 adds).
- **The relay page** (`nautilus-bridge/relay.html`, `relay.js`): one of
  the extension's own pages, loaded by the engine in a hidden windowless
  browser (`HiddenPage` in `relay.rs`: a 16 by 16 view that nothing
  paints, with a DevTools observer), one per bridged, enabled extension.
- **The engine** (`src/bridge/relay.rs`, `Bridge`; the pure parts are
  `table.rs`, `rewrite.rs` and `install.rs`).
- **The helper** (`src/bridge/js/helper/`, written as `nautilus-helper/`
  into the profile's `chromium/extensions/` at every start, id
  `gblknehfeeaadaalhiogpllhfpelpngj`): an extension with the `debugger`
  permission and one page, loaded in a hidden browser. Never listed to
  the model.

## Engine and relay page: DevTools

The engine calls functions of `window.nautilusRelay` with
`Runtime.evaluate` (`awaitPromise`, `returnByValue`) through
`CefBrowserHost::ExecuteDevToolsMethod` on the relay's browser; the
promise's value comes back in `OnDevToolsMethodResult`. The engine
passes id 0 and keeps the id the call returns, since the controller may
raise a caller's id. There is no other channel: the page reaches the
engine only by resolving a promise the engine is waiting on.

| Call | Resolves with | Use |
|------|---------------|-----|
| `nautilusRelay.next()` | `[{call: {id, method, args}}, …]`, when the worker has asked something | The engine keeps exactly one pending per relay and calls it again after each answer. It stays pending for as long as the worker is quiet. |
| `nautilusRelay.reply(id, {result} \| {error})` | `true` if posted | The answer to call `id`. `error` is a message; the bridge rejects the extension's promise with it. |
| `nautilusRelay.deliver({id, event, tabId})` | `{ok, error?, attempts, received}` | Hand an event to the worker, starting it if stopped. |

The helper: `nautilusHelper.targets()` resolves with
`[{id, tabId, type, url, title}]` from `chrome.debugger.getTargets()`
(`tabId` null when the target has none). The engine joins the list with
each tab's own target id (`Target.getTargetInfo`), in `table.rs`.

## Relay page and bridge: a runtime port

A `chrome.runtime.connect` port named `nautilus-bridge-1`. Either end
opens it: the relay to deliver (which starts a stopped worker), the
bridge when the worker was started by something else and has a call.
Each end uses the newest port. An idle port does not keep the worker
alive. The bridge wraps `chrome.runtime.onConnect`'s `addListener`,
`removeListener`, `hasListener` and `hasListeners` so that the extension's
own listeners never receive this port, and `hasListener` and
`hasListeners` do not count the bridge's own listener. The wrapper is in the worker only:
extension pages have no bridge in protocol 1 (see the limits below).

| Direction | Message | Meaning |
|-----------|---------|---------|
| bridge → relay | `{call: {id, method, args}}` | A bridged API call; `id` counts up per worker start. |
| relay → bridge | `{reply: {id, result?, error?}}` | Its answer. Unanswered after 10 s, the bridge rejects. |
| relay → bridge | `{deliver: {id, event, tabId}}` | An event to dispatch. `id` is the engine's, unique per launch. |
| bridge → relay | `{received: id}` | Sent on receipt, before the listeners run. |
| bridge → relay | `{done: {id, ok, error?}}` | The dispatch finished. |

## Delivered once

What is guaranteed, and what is not:

- Within one relay, the relay page sends a delivery again, on a new
  port, only if the port closed before `received` arrived. A message to a
  port whose worker is gone is dropped, so nothing ran. After `received`
  it is never sent again by the relay.
- Within one worker instance, the bridge keeps the delivery ids it has run
  and ignores a repeat, so a repeat that reaches the same worker runs
  nothing twice.
- The engine retries a delivery at most once, and only when the relay
  answers `ok: false` with `received` not true on the first attempt. It
  then opens a fresh relay and sends the same delivery id again (the id
  is the one the first attempt carried, so the worker instance above
  ignores it if it had already run it). A second failure is logged and the
  click dropped; the relay is replaced so the next click has a live one.
- When the relay's evaluation itself fails (the relay page is gone or its
  call failed), the engine does not retry: whether the worker took the
  delivery is unknown, and a retry could reach a new worker instance that
  has no record of the id. The click is dropped and logged, and the relay
  is replaced.
- A worker that took a delivery and then failed (`received: true`,
  `ok: false`) is never sent it again.

Not guaranteed: a worker instance that stops after it has received a
delivery but before the relay has heard `received` can be replaced by a
new one, which has no record of the id; the listeners could then run
twice. The window is the time between receipt and the `received` message
arriving. No run hit it; every delivery in the Phase 1 runs was taken on
its first attempt.

## Calls (bridge to engine)

| `method` | `args` | `result` | Since |
|----------|--------|----------|-------|
| `tabs.query` | `[queryInfo]` | `[{id, index, windowId, active, url, title}]` | 1 |
| `debugger.attached` | `[debuggee]` | `null` | 1 |
| `debugger.detached` | `[debuggee]` | `null` | 1 |

`debuggee` is Chromium's tab id (a number) for an attach or detach by
`tabId`, or `{"targetId": "<id>"}` for one by `targetId` (the id
`chrome.debugger.getTargets()` lists). The engine maps either form through
its table (`table::TabTable::resolve`): a target that is a Nautilus tab
resolves to it once the tab has reported its own target id, a target that
is no Nautilus tab (a worker, an extension page, the management page)
resolves to nothing and is logged as untracked.

`debugger.attached` and `debugger.detached` report that an extension
attached or detached a `chrome.debugger` session on a tab. The bridge sends them after Chromium has made the change,
and the call (the wrapped `chrome.debugger.attach`/`detach`) resolves
only when the engine has answered. The engine answers once the tab's
state is applied: a tab that is hidden is shown while any extension
debugs it, and hidden again when the last debugger goes (`bridge/debugger.rs`).
So a debugger's first command meets a visible renderer. A report for a
tab the table does not know waits for a table refresh first. A detach
that Chromium makes without the extension (the tab closed, or the user
cancelled the infobar) comes through `chrome.debugger.onDetach`, with
the source it names (`tabId` or `targetId`), and is reported the same way. The
bridge treats any failure or timeout of a report as no answer and lets
the extension's own call go on, so a report never blocks a debugger it
cannot track.

`id` is Chromium's tab id from the tab-id table. In protocol 1 the
engine ignores `queryInfo` and returns every Nautilus tab, `windowId`
is 1 and `index` is the tab's place among the engine's tabs: Phase 5
answers from the shell's strip and filters. The bridge fetches
Chromium's own tab object for each id (`chrome.tabs.get`) and lays
`index`, `windowId`, `active` and `highlighted` over it. Any other
method is answered `{error: "<method> is not supported by Nautilus."}`:
the bridge fails closed. A `tabs.query` is answered when the helper's
list of targets arrives, so a query made while the helper's page is
still loading waits for it.

## Events (engine to bridge)

| `event` | Carries | The bridge does | Since |
|---------|---------|-----------------|-------|
| `action.onClicked` | `tabId` (Chromium's) | `chrome.action.onClicked.dispatch(await chrome.tabs.get(tabId))` | 1 |

A click reaches the engine only when osmena's dispatch finds an action
(`has_action()`: enabled, with an `action_title`, which `on_list` sets for
any action; `extensions.rs`). The engine then sends it to a bridged,
enabled extension whose relay is open; any other extension's click is
not sent. A click for a tab whose Chromium id the table does not know is
dropped with a log line.

## The rewrite on disk

`bridge::install::rewrite_in_place` writes, into an extension's
directory: `nautilus-bridge/{bridge.js, relay.html, relay.js}` (every
time), and the first time `<entry's directory>/nautilus-loader.js`,
`nautilus-bridge/original.json` (`{"service_worker", "module"}`) and
the manifest with `background.service_worker` naming the loader.
Deleting the loader and `nautilus-bridge/` and restoring the entry from
`original.json` undoes it. An extension with no service worker is not
bridged.

`bridge::install::copy_and_rewrite` is the path a bridged extension
takes: it copies the unpacked folder into the profile's
`chromium/extensions/unpacked-<name>-<8 hex>` (`copy_name`: the folder's
name reduced to `[a-z0-9-]`, and eight hex digits of the SHA-256 of the
source's absolute path), replaces an earlier copy of the same folder, and
rewrites the copy. Only the test control's `load` takes this path in
protocol 1. "Load unpacked…" and store installs load their folders
unbridged, so an extension installed that way has no bridge until Phase 5
routes them through it.

## Limits in protocol 1

- `tabs.query` reports every Nautilus tab with `windowId` 1; `queryInfo`
  is ignored.
- Only the methods in the table above are answered; everything else is
  refused, never passed to Chromium.
- Extension pages (the relay page included) receive the bridge's port,
  because the wrapper is in the worker only. Phase 5 puts the bridge into
  extension pages and hides the port there too.
- `chrome.debugger` called from an extension page (a popup, the options
  page, the side panel) is not reported: the bridge is in the worker only.
  Such a debugger's tab is not kept painted until Phase 5 bridges the
  pages.
- An unbridged extension's debugger is not reported. In this phase that
  is every store-installed and every "Load unpacked…" extension; only the
  test control's `load` is bridged, so only the test extension's debugger
  keeps its tab painted.
- A module worker (`"type": "module"`) gets a loader of two static
  imports, which is unit-tested as text only; no module-worker extension
  has run.
- The loader for a classic worker loads the bridge in a `try`: if the
  bridge fails to load, the extension's own worker still runs, unbridged.
