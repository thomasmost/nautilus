# The extension bridge's protocol

Protocol 1 (side-panel plan, Phase 1). What travels between the engine
and the bridge script in an extension's service worker. Nothing outside
`nautilus-engine` sees it. Phases 5 to 7 add calls and events here and
say so in this file; a change that an older `bridge.js` on disk would
misread raises the number in the port's name. Review cycle 3 added the
instance id, the hello, `gone`, `identify`, `alive` and the give-up
(below); they change the message shapes without a new number, because
`install` rewrites `bridge.js` and `relay.js` on every load, so a copy
loaded by this build always carries them. A copy written by an earlier
build is not handled (see the limits).

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
  `policy.rs`, `debugger.rs`, `table.rs`, `rewrite.rs` and `install.rs`).
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
| `nautilusRelay.next()` | `[{call: …}, {hello: …}, {gone: true}, …]`, when the worker has asked something or its identity or port changed | The engine keeps exactly one pending per relay and calls it again after each answer. It stays pending for as long as the worker is quiet. |
| `nautilusRelay.reply(id, {result} \| {error}, instance)` | `true` if posted | The answer to call `id` of worker instance `instance`. The bridge takes only the answers for its own instance. |
| `nautilusRelay.deliver({id, event, tabId})` | `{ok, error?, attempts, received}` | Hand an event to the worker, starting it if stopped. |
| `nautilusRelay.identify()` | `{instance, attached}`, the worker's hello | Which worker instance the relay reaches. It connects, so a stopped worker starts. Rejects when no hello comes within 5 s. |
| `nautilusRelay.alive()` | `true`; rejects when the page's extension context is gone | Checked every 2 s for a relay that holds debuggers. A reloaded extension leaves its pages open; their API calls then fail. |

The page's first evaluation after it loads is a probe:
`typeof nautilusRelay === 'object' || typeof nautilusHelper === 'object'`.
`true` is the page working: its openings start over, and the poll starts
(for the helper, the queued tab lists are asked). Anything else closes the
page, which the next turn may open again (see "Openings and the give-up").

The helper: `nautilusHelper.targets()` resolves with
`[{id, tabId, type, url, title}]` from `chrome.debugger.getTargets()`
(`tabId` null when the target has none). The engine joins the list with
each tab's own target id (`Target.getTargetInfo`), in `table.rs`.

## Relay page and bridge: a runtime port

A `chrome.runtime.connect` port named `nautilus-bridge-1`. Either end
opens it: the relay to deliver or to identify (which starts a stopped
worker), the bridge when the worker was started by something else and
has a call. Each end uses the newest port. An idle port does not keep the
worker alive. The bridge wraps `chrome.runtime.onConnect`'s
`addListener`, `removeListener`, `hasListener` and `hasListeners` so that
the extension's own listeners never receive this port, and `hasListener`
and `hasListeners` do not count the bridge's own listener. The wrapper is
in the worker only: extension pages have no bridge in protocol 1 (see the
limits below).

| Direction | Message | Meaning |
|-----------|---------|---------|
| bridge → relay | `{hello: {instance, attached}}` | Sent first on every port, once the worker's debugger sessions are known. `instance` is this worker start's id; `attached` lists the Chromium tab ids on which this extension's own debugger session still answers (`null` when Chromium did not say). A call waits for the hello, so the engine knows the instance before any call. |
| bridge → relay | `{call: {id, method, args, instance}}` | A bridged API call; `id` counts up per worker start. |
| relay → bridge | `{reply: {id, instance, result?, error?}}` | Its answer. An instance other than the bridge's is ignored. Unanswered after 10 s, the bridge rejects. |
| relay → bridge | `{deliver: {id, event, tabId}}` | An event to dispatch. `id` is the engine's, unique per launch. |
| bridge → relay | `{received: id}` | Sent on receipt, before the listeners run. |
| bridge → relay | `{done: {id, ok, error?}}` | The dispatch finished. |

The relay turns what the worker sends into the engine's `next()` answer:
a `call` as it came, a `hello` as `{hello}`, and the port's end as
`{gone: true}` (the worker stopped, or the extension was reloaded; the
relay learns of it only if the port reports it).

`attached` is decided by asking, not by `chrome.debugger.getTargets()`
alone: `getTargets` marks a tab attached when any client is, the engine's
own DevTools probe included, so each tab it marks is asked with
`sendCommand` (`Runtime.evaluate` of `1`), and only a session this
extension opened answers. Measured in `run.sh stop` (the session is open,
`ServiceWorker.stopAllWorkers` stops the worker, the engine restarts it,
the hello lists the tab): the sessions survive the stop. Measured in
`run.sh reload` (`chrome.runtime.reload()`): the new worker's
`sendCommand` fails with "Debugger is not attached to the tab with id".

## Delivered once

What is guaranteed, and what is not:

- Within one relay, the relay page sends a delivery again, on a new
  port, only if the port closed before `received` arrived. A message to a
  port whose worker is gone is dropped, so nothing ran. After `received`
  it is never sent again by the relay.
- Within one worker instance, the bridge keeps the delivery ids it has run
  and ignores a repeat, so a repeat that reaches the same worker runs
  nothing twice. A new instance has no record of an id, so a delivery it
  receives after a restart runs once there.
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

## Worker instances

Each worker start has an instance id (`bridge.js`, `INSTANCE`). The
engine learns it from the hello, and every call and debugger report
carries it. The engine's rules (`policy.rs`, `relay.rs` `worker_hello`):

- **Same instance:** nothing is dropped. The tabs the engine shows for
  the extension's debuggers are kept where the hello's `attached` list
  still has them (or where it does not say).
- **New instance:** the tab lists and debugger reports the old instance
  made and has not had answered are dropped without an answer, since their
  call ids belong to that instance. Clicks are kept (a delivery's id is
  unique). The tabs the extension debugs are forgotten where its sessions
  no longer answer, and each one with no debugger left is hidden again.
- **Unreached** (the relay cannot reach the worker: `identify` or `alive`
  failed): the extension's queued tab lists and debugger reports are
  dropped, and every tab it debugs is forgotten and hidden again. The
  relay is closed, so the next turn opens a fresh one.

A worker is asked (`identify`) when its relay's port reports `gone` or
its relay opens again, and only when the extension holds a debugger or a
debugger report is queued: a stopped worker is not started just to be
asked. A relay that holds debuggers is checked with `alive` every 2 s.

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
attached or detached a `chrome.debugger` session on a tab. The bridge sends
them after Chromium has made the change, and the call (the wrapped
`chrome.debugger.attach`/`detach`) resolves only when the engine has
answered. The engine answers once the tab's state is applied: a tab that
is hidden is shown while any extension debugs it, and hidden again when
the last debugger goes (`bridge/debugger.rs`). So a debugger's first
command meets a visible renderer. A report for a tab the table does not
know waits for a table refresh first. A detach that Chromium makes
without the extension (the tab closed, or the user cancelled the
infobar) comes through `chrome.debugger.onDetach`, with the source it
names (`tabId` or `targetId`), and is reported the same way. The bridge
treats any failure or timeout of a report as no answer and lets the
extension's own call go on, so a report never blocks a debugger it
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
still loading waits for it. When the helper's page has been given up, a
query is answered with an error at once.

## Events (engine to bridge)

| `event` | Carries | The bridge does | Since |
|---------|---------|-----------------|-------|
| `action.onClicked` | `tabId` (Chromium's) | `chrome.action.onClicked.dispatch(await chrome.tabs.get(tabId))` | 1 |

A click reaches the engine only when osmena's dispatch finds an action
(`has_action()`: enabled, with an `action_title`, which `on_list` sets for
any action; `extensions.rs`). The engine then sends it to a bridged,
enabled extension whose relay is open; any other extension's click is
not sent. A click for a tab whose Chromium id the table does not know is
dropped with a log line. A click for an extension whose relay was given up
is refused (`click` returns false), as is any click while the helper's
page is given up.

## Openings and the give-up

A hidden page (a relay, or the helper) is opened by the engine's turn
when it is wanted and not open (`Bridge::reopen`), never from a callback.
Each opening is counted, and the count is cleared by the page's probe.
The gap before the next opening is 250 ms after the first failed one,
doubling to a cap of 8 s (`policy::next_opening`). After five openings
without a successful probe the engine gives up on that page:

- it logs one warning that says so;
- a relay's debuggers are forgotten (their tabs are shown again, since
  the engine can no longer hear the worker), its queued work is dropped
  with a log line, and its clicks are refused from then on;
- the helper's queued tab lists are answered with an error, and clicks
  are refused.

Nothing opens the page again until a sync sees its extension change (it
came on, went off, or was reloaded: off and on in two lists). Measured:
`run.sh broken` replaces the relay page in the throwaway profile and closes
it; the engine opens it five times, logs the give-up once, opens nothing for
12 s after it, and refuses a click (`review3-broken-after`). Before the change the
same provocation opened the page 229 times in 60 s with no give-up
(`review3-broken-before`).

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
- The session check is `sendCommand`, which runs `Runtime.evaluate` of `1`
  in each tab Chromium marks attached, every time the worker's hello is
  made (on a restart, or when the relay is asked). It is a trivial
  evaluation the extension did not ask for. A tab the extension has no
  session on is refused by Chromium, and the hello leaves it out.
- A reload is judged from what the engine can see: the relay's port event
  (`gone`), the `alive` check, and the list of extensions (`sync`). In the
  measured runs the list showed the extension off during the reload and
  `sync` forgot its debuggers, and the session was gone (`review3-final-reload-*`,
  `review3-reload-after-attempt3` to `-attempt7`). The one case that is not
  shown handled is a reload in which the list does not show the extension off
  and the relay page stays open with no port event: the first attempt
  before the fix (`review3-reload-before-attempt1`) and the build before the
  `alive` check (`review3-reload-after-attempt2`, which failed) left the tab
  painted with its session gone. The `alive` check was added for that case,
  and the failure path it takes was run under a forced failure
  (`review3-control-alive-forced-fail`); no run on the final build reached
  the case, so whether `alive` fails on a real reload is not shown.
- A copy written by an earlier build is not handled: its `bridge.js` sends
  no hello or instance, and its `relay.js` has no `alive`. A copy is
  rewritten on every load by this build, and the copies of a previous
  engine are replaced the next time the extension is loaded.
- A queued debugger report whose tab the table does not know, and whose
  worker restarts before the table knows the tab, is dropped with a log
  line (the worker's own call is not answered). Not provoked in a run: the
  case is tested (`job_belongs`), not seen.
