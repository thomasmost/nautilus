# The extension bridge's protocol

Protocol 1 (side-panel plan, Phase 1). What travels between the engine
and the bridge script in an extension's service worker. Nothing outside
`nautilus-engine` sees it. Phases 5 to 7 add calls and events here and
say so in this file; a change that an older `bridge.js` on disk would
misread raises the number in the port's name. Review cycle 3 added the
instance id, the hello, `gone`, `identify` and the give-up (below); review
cycle 4 changed the hello's shape (its sessions carry three states),
removed `alive`, stopped `gone` from asking the worker anything, and added
the worker-target check and the debugger rule (below). Review cycle 5 made a
worker-target change wait for a hello received after the change was observed
(I16, below), and bounded the whole hello, the target list included, at 2 s.
Review cycle 6 replaced that wait with an ask: a worker-target change asks the
relay which worker it reaches, and the answer decides the relay (I17, below);
`identify` rejects in a reloaded extension's zombie page instead of answering
from the hello it cached. They change the message shapes without a new number, because `install`
rewrites `bridge.js` and `relay.js` on every load, so a copy loaded by this
build always carries them. A copy written by an earlier build is not handled
(see the limits).

## The debugger rule (review cycle 4)

Uncertainty never forgets. A tab that an extension's `chrome.debugger`
session debugs stays shown (a hidden tab answers `Input.dispatchMouseEvent`
only after about 5 s: the H5 stall, measured). The engine forgets one of
those debuggers only on positive evidence (`policy::Evidence`,
`policy::forgets`):

1. the worker's detach report (`debugger.detached`, or Chromium's `onDetach`
   for the worker's own debugger), applied as it arrives;
2. a hello from a live worker that names the tab with the state
   `not-attached` (Chromium's "Debugger is not attached" answer, or the tab
   does not exist) (`policy::tabs_to_forget`);
3. the extension is off or gone in the manager's list (`sync`);
4. the tab closed (`untrack`).

A timeout, an unknown probe result, a failed evaluation, an unreachable
worker, a relay that is closed, a relay that is given up on, or any other
error only closes the relay (to be opened again under the opening schedule)
and keeps the debuggers. The engine's two evidence-based forgets (a hello,
and `sync`) both pass through `policy::forgets`; a detach report and a tab's
close are applied as they arrive.

## The parties

- **The bridge** (`src/bridge/js/bridge.js`): written into each bridged
  extension as `nautilus-bridge/bridge.js` and loaded ahead of the
  extension's own worker by `nautilus-loader.js`, which the rewritten
  manifest names as `background.service_worker` and which sits beside
  the original entry file. It sets `self.__nautilusTransport =
  {protocol: 1, portHidden}` (`self.__nautilusBridge` is kept free for
  the namespaces' object Phase 5 adds). Since Task 9 it also sets the two
  functions the namespace scripts are written against,
  `self.__bridgeCall(ns, method, args)` (a call named `ns.method`, as
  `call` above) and `self.__bridgeOnEvent(handler)`. The worker's script
  is this file followed by the namespace scripts (`js/core.js`,
  `tabs.js`, `windows.js`, `runtime.js`), joined by `worker_script()`.
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
| `nautilusRelay.identify()` | `{instance, sessions}`, the worker's hello | Which worker instance the relay reaches, and the state of its session on each tab. It connects, so a stopped worker starts: the engine asks only when a relay has just passed its probe and an extension holds a debugger or a queued debugger report needs it (`Bridge::needs_identity`), never on a `gone` (I15). Rejects when no hello comes within 5 s, and then the relay is closed and opened again, its debuggers kept. Rejects at once in a reloaded extension's zombie page (its `chrome.runtime.id` is gone), whatever hello it cached. A worker-target change asks it too, and the answer decides the relay (the ask, below; review cycle 6). |

`alive()` was removed in review cycle 4. The engine asks for a table refresh
every 2 s while a relay is open (`REFRESH_INTERVAL`), and a worker whose
DevTools target changed is found that way (`policy::worker_target`: the
relay is closed and opened again). The guess that `alive()` relied on
(`chrome.runtime.id` and `getManifest` fail in a reloaded extension's page)
was measured in the zombie runs (`review4-zombie-run1.out`, the INFO line
`relay_eval`): in the zombie page `chrome.runtime.id` is `undefined` and
`chrome.runtime.getManifest()` throws `Extension context invalidated.`
(`review4-zombie-run1.out`, `review4-zombie-run2.out`, `review4-zombie-run3.out`;
all three the same). So the guess "fails" was half right: the id does not
throw, it is undefined; `getManifest` throws. Nothing depends on it now.

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
in the worker only; an extension page has its own transport and namespaces
(see the limits below and "The scripted run (phase5.py)").

| Direction | Message | Meaning |
|-----------|---------|---------|
| bridge → relay | `{hello: {instance, sessions}}` | Sent first on every port, once the worker's debugger sessions are known. `instance` is this worker start's id. `sessions` lists, for each Chromium tab the target list names (`chrome.debugger.getTargets`), the state of this extension's own debugger session there: `{tab, state}` with `state` `attached` (the session answers), `not-attached` (Chromium said so: "Debugger is not attached", or "No tab with id"), or `unknown` (a timeout or any other error). A tab the hello does not name has no evidence. A call waits for the hello, for at most about 2 s (`HELLO_TIMEOUT_MS`), so the engine knows the instance before any call. |
| bridge → relay | `{call: {id, method, args, instance}}` | A bridged API call; `id` counts up per worker start. |
| relay → bridge | `{reply: {id, instance, result?, error?}}` | Its answer. An instance other than the bridge's is ignored. Unanswered after 10 s, the bridge rejects. |
| relay → bridge | `{deliver: {id, event, tabId}}` | An event to dispatch. `id` is the engine's, unique per launch. |
| bridge → relay | `{received: id}` | Sent on receipt, before the listeners run. |
| bridge → relay | `{done: {id, ok, error?}}` | The dispatch finished. |

The relay turns what the worker sends into the engine's `next()` answer:
a `call` as it came, a `hello` as `{hello}`, and the port's end as
`{gone: true}` (the worker stopped, or the extension was reloaded; the
relay learns of it only if the port reports it).

**Which port is the bridge's.** The worker adopts the port named
`nautilus-bridge-1` only from its extension's relay page: `sender.id` is the
extension's and `sender.url` is the relay page's address
(`runtime.getURL('nautilus-bridge/relay.html')`). The engine hosts the relay
page in a tab, so its port's sender carries a `tab`; that is not refused. The
relay adopts only its extension's worker's port: `sender.id` is the extension's
and `sender.url` is the address of the worker's script, which is the manifest's
`background.service_worker` (the loader, measured as `background/nautilus-loader.js`)
resolved with `runtime.getURL`. Another page of the extension (a popup, the options
page) and another script of it are refused. A content script's port carries the web
page's address, so it is refused in both places. Measured in the scripted run
(`phase5.py`): the relay's port has a sender tab and is adopted; a check that
refused a sender tab left the first wake undelivered.

**A port with no listener.** The worker may open its own port before the relay
page listens (a worker started before the engine has opened the relay). Chromium
closes such a port at once with "Could not establish connection. Receiving end
does not exist.", and nothing on the other end ever received a message over it.
The bridge does not take that close for a lost relay: a call sent on that port is
sent again on the next port, every 100 ms for up to 3 s from the first close, and
fails after that with "found no listener in the browser" (`bridge.js`,
`noListener`, `resend`). Any other close of the port in use still fails the
pending calls with "the browser stopped listening". A call is posted on the port
in use at the time it is posted, after that port's hello (`send`), so no call is
posted on a port that was replaced.

A session's state is decided by asking, not by `chrome.debugger.getTargets()`
alone: `getTargets` marks a tab attached when any client is, the engine's
own DevTools probe included. So each tab the list names is asked with
`sendCommand` (`Runtime.evaluate` of `1`), and only a session this extension
opened answers. Each probe is raced against 1 s (`PROBE_TIMEOUT_MS`), and the
whole chain, `getTargets` included, against 2 s (`HELLO_TIMEOUT_MS`): when the
chain has not settled by then the hello names no session, which forgets
nothing. A probe that has not answered is `unknown`, which keeps the debugger. The probe's `Runtime.evaluate` is a trivial expression
with no side effect; whether it answers on a page paused by `Debugger.pause`
or on a hidden tab was not verified in this cycle (no run paused a page),
and the timeout makes that harmless (the state is `unknown`, the debugger is
kept). Measured in `run.sh stop` (the session is open,
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
- A call from a worker is sent again only when Chromium closed the port it was
  sent on with "Receiving end does not exist" (see "A port with no listener"):
  no listener received that port, so nothing ran the call. A call that reached a
  listener is not sent again by the bridge.

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

- **Any instance** (the same or another): the hello's `not-attached` tabs
  are forgotten (the rule above); its `attached` and `unknown` tabs, and the
  tabs it does not name, keep their debuggers. A tab forgotten this way is
  hidden again when no debugger is left on it.
- **Same instance:** nothing else is dropped.
- **New instance:** the calls the old instance made are retired
  (`CallIds::retire_instances`): no answer is sent for them, since their call
  ids belong to that instance. The events queued for the old instance are
  dropped (`policy::job_kept`), and logged. A debugger report the old instance made and has not had
  answered is kept and applied when the table resolves its tab (its session
  survives a stop; measured), and its answer is not sent (`policy::reply_due`).
  Clicks are kept (a delivery's id is unique).
- **Unreached** (the relay cannot reach the worker: `identify` failed, or
  its hello has no instance): nothing is forgotten. The relay is closed,
  so the next turn opens a fresh one, and the opening counts as a failed one
  for the schedule (`Opener::unreached`). A silent worker says nothing about
  its sessions.
- **A session for a tab the table cannot name yet** (the hello names a Chromium
  tab id no table row has): kept pending, and applied by the first table join
  that names it (`Bridge::apply_pending`). A debugger attach by the extension
  drops that extension's pending sessions (a dropped one only keeps a debugger).

A worker is asked (`identify`) only when a relay has just passed its probe
and the extension holds a debugger or a debugger report is queued. Its port
reporting `gone` (an idle stop, or a reload) asks nothing: the engine asking
would start the stopped worker, which defeats MV3's idle stop (I15). The new
instance is learned from the hello of the worker's own next port (the
relay's connect, or the worker's call), which arrives unasked. A changed
target does not name the instance; it only decides the relay (below).

**The worker target, a reload, and a restart.** The engine asks the helper
for the target list every 2 s while any relay is open (`REFRESH_INTERVAL`,
`refresh_due`), so a reload is seen within a bounded time with no tab
activity. A tab whose DevTools target id is learned between two refreshes
asks for one on the next turn (`refresh_wanted`, read by `reopen`); no
second timer exists for the table. The helper lists an extension's service worker with type `worker`
(`chrome.debugger.getTargets`: a fact measured in the probe; `service_worker`
is accepted too), with the extension's URL (`policy::worker_target`,
`table::worker_target_of`). A target other than the recorded one is a change
(`WorkerTarget::Changed`, logged "worker target changed from ... to ..."). The
recorded target is kept through an absence (an idle stop), and `policy::WorkerWatch`
(one per extension) counts the changes; a change's number names its ask.

**The ask (review cycle 6, I17).** A change asks the relay that reaches the old
worker which worker instance it reaches now. The ask is the relay's `identify`
(`check_worker_targets`, `ask`), the same evaluation the engine uses for an
extension's debuggers. Only a change with a relay open asks (`policy::asks_on`):
an absence and a first target never ask, so the ask never starts a stopped
worker (I15); a change with no relay open asks nothing, and a relay opened later
says who its worker is when a debugger needs it. The answer decides the relay
(`policy::after_ask`, applied by `decide_ask`):

- **A hello answers:** the relay reaches a live worker, and it is kept. The
  hello is applied as any hello is (a session it says is not attached is
  forgotten, the rest are kept). Nothing in flight on the relay's port is touched.
- **No hello answers** (an error, a throw, no hello within the 5 s of
  `identify`, or a hello with no instance): for the latest change the relay is
  a zombie. It is closed, and the next turn opens a fresh one; the fresh relay's
  own `identify` decides the debuggers. An answer to an earlier change is
  ignored: the latest change's own ask decides (a second change while an ask is
  out is decided by its own answer).
- **A hello that came before the change was observed decides nothing.** The
  hello is not an input any more, and no refresh count is waited for: the
  two-refresh grace of cycles 4 and 5 (`CHANGE_GRACE_REFRESHES`,
  `policy::after_change`) is removed, with its tests.

A zombie cannot answer from memory. After a reload the relay page keeps the
hello of the old worker and gets no port event, so a cached hello would answer
for a worker that is gone. `identify` therefore checks the page's extension
context first and rejects when `chrome.runtime.id` is gone (`relay.js`,
`contextAlive`). The check is node-tested: the committed file resolves with the
stale hello (`review6-identify-zombie-red.txt`, status 1); the tree rejects
(`review6-identify-zombie-green.txt`, status 0).

The sequences are the tests of `policy::tests` (the ask is `policy::asks_on`
and `policy::after_ask` over `WorkerWatch::observe`): a restart kept by its
answer with no close; a reload the detector sees, closed by its failed ask; the
I16 sequence (a restart and a reload in one window: the ask goes to the zombie,
which cannot answer, so it is closed); a restart and then a reload (the first
kept, the second closed); two changes with an ask out (the first's failure is
ignored, the second's decides); a change with no relay open; an absence racing
the ask (no ask, the outstanding one still decides); and a failed ask that closes
once and is not repeated while the target stays.

**Cost and bounds.** A live relay is closed only when its `identify` fails. That
happens in two cases, and each is a wrong close. One: its worker does not say
hello within 5 s of the ask. Two: the ask reaches a relay whose page is still
loading (a relay just opened or reopened, when a refresh sees a target change
before the load ends), where `nautilusRelay` is not defined yet and the
evaluation fails; this case is reasoned from the code and was not seen in a run.
Either costs one reopen, and what is in flight on the relay is failed (below). The ask is made once per change, and it is not retried: a zombie
answers at once, so a retry would repeat the answer, and no run had a worker
that was slow to say hello (not measured). A failed ask closes at most one relay
per change. The reopened relay's identify is the only other ask, and it happens
only when a debugger needs the worker's identity (`needs_identity`, on the probe).

**Measured (review cycle 6 final runs).** In spike run 1 and run 2 and stop run 1
the restart's change was asked and the relay was kept, with no close and no
reopen (`review6-spike-run1-logs/run-2.log`: change at 15:15:56, the ask and
"the relay is kept" at the same second; `review6-spike-run2-logs/run-2.log`:
15:21:53; `review6-stop-run1-logs/run-1.log`: 15:34:18). In both spike runs the
new worker's hello reached the engine before the change was observed (lines 94
and 97 of each `review6-spike-run1-logs/run-2.log` and `review6-spike-run2-logs/run-2.log`), which is the cycle 5 case that was closed after
the grace; the ask keeps the relay. In zombie runs 1 and 2
(`review6-zombie-run1-logs/run-1.log` at 15:13:10,
`review6-zombie-run2-logs/run-1.log` at 15:26:53) the ask failed at the change
("could not say which worker it reaches"), the relay was closed and opened
again within the same second, the reopened relay's hello forgot the tab, and
"hidden again" was logged. The cycle 5 spike close was three to four seconds
after the change (`review5-spike-run2-logs/run-2.log`: 06:27:34 to 06:27:37).
Not measured: a slow start longer than 5 s, a stop and restart repeated in a
loop, a reload whose target id does not change, and Windows.

**The delivery guarantee across a change.** A kept relay interrupts nothing: its
calls and deliveries in flight on its port complete, as the ask cannot close
it. When a zombie relay is closed, whatever is pending on it is dropped. A
delivery sent through it and not acknowledged is logged as unknown and is not
retried (`drop_pending`; `after_delivery` never retries a failed evaluation, so
it is at most once). A click queued but not yet sent waits for the fresh relay
and is delivered once (`job_turn`). A tab list or debugger report of the old
instance is retired (its answer is not sent), and an event queued for it is
dropped (`job_kept`); a debugger report is applied and not answered
(`reply_due`), because its call ids belong to a worker that is gone. In the zombie runs no
delivery was in flight on the closed relay, so the runs show the close and the
hello, not a lost call. A call in flight across a restart window was not
provoked deterministically: the stop run's quiet click is not timed against the
change, so no run puts a `tabs.query` inside the window. The pure parts are
tested (`policy::after_ask`, `asks_on`, `job_kept`, `reply_due`, `after_delivery`).

Earlier builds: cycle 4 closed a relay at the change unless a new instance had
said hello through it, and a restart's hello could cover a later reload (I16).
Cycle 5 counted only a hello received after the change, with a two-refresh grace,
which closed a live relay after every restart whose hello came first (one
reopen, three to four seconds after the change). Cycle 6 asks the relay instead.

## Calls (bridge to engine)

| `method` | `args` | `result` | Since |
|----------|--------|----------|-------|
| any `tabs.*` or `windows.*` function the script bridges, by name (`tabs.query`, `tabs.update`, `windows.get`, …) | the arguments as the extension passed them | the router's answer (`router.rs`): an answer from the engine's strip, a forward to Chromium, a request to the model, or a rejection | 5 |
| `debugger.attached` | `[debuggee]` | `null` | 1 (a worker); 7 (a page) |
| `debugger.detached` | `[debuggee]` | `null` | 1 (a worker); 7 (a page) |

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

**A page's debugger.** An extension page (a side panel's page, a popup,
a page in a tab) makes the same two reports through the page lane:
`js/debugger.js`, one of the namespace scripts, wraps `chrome.debugger`'s
`attach` and `detach` on Chromium's own object (which is `browser`'s
too) and listens to `onDetach`, exactly as the worker's transport does,
and calls `debugger.attached` / `debugger.detached` with the same
`debuggee`. It does nothing in a worker (`__nautilusTransport` is the
mark) and nothing where the page has no `chrome.debugger`. The state
hands the call over as `Effect::Debugger` (after the page's holds, like
every page call), and `Hub::run` gives the debuggee to
`Bridge::page_debugger`, which applies it to the same set
(`bridge/debugger.rs`) with the same show and hide, and only then
answers the call: so a page's `attach` also resolves with its tab
shown. Any other `debugger.<name>` call is refused as unsupported
(Chromium's own functions are never bridged). The set is keyed by
extension, not by context: Chromium keeps one debugger session per
extension and tab (measured in `panel.sh panel`: while a panel's page
holds a session, the worker's `sendCommand` on that tab is answered
through it and the worker's own `attach` is refused as already
attached), so a worker's hello that probes the tab finds the page's
session attached and forgets nothing, and a detach by either context
ends it for both. Both contexts hear `onDetach` and both report it;
the second report changes nothing.

**A painted tab the window does not show redraws nothing.** A tab kept
painted for a debugger paints behind the active one. `on_paint` keeps
its frame and raises `FrameReady` only for a view the shell is showing
(`ViewState::frame_painted`, by `ViewState::hidden`); showing the view
invalidates it (`show_view`), so the first frame after that is
announced.

The call's `method` is split at its first dot into the namespace and the
function, and the hub routes it (`hub.rs`, `router.rs`). `tabs.query`, the
other `tabs` and `windows` functions, and the `bridge.listen` message are
answered by the engine from its strip or its own state, with no CEF call; a
forwarded call goes to Chromium by name with the translated ids, and the
script accepts a forward only for the methods in `router::FORWARDED`; a request
(create, close, move, activate, open) waits for the model's answer; and a
method the router does not know is rejected, never passed to Chromium. A
worker's answer goes back down the relay under the worker instance that made
the call (`CallIds`, which keeps the extension, the instance and the
worker's own call id for each call the hub is answering). Phase 5 answers
from the strip, so a query no longer waits for the helper's list of targets;
the helper's page is needed only for the debugger and for clicks. 

**Bounds.** A worker's call and a page's call are each refused after 10 s if
the engine has not answered (`bridge.js`, `page_transport.js`: "was not
answered"). A page's call of an extension that is not served waits up to 2 s
for it and is then refused ("the extension is not bridged."). Held calls are
kept in order within their own extension, so one extension's held calls never
hold another's. A page call the browser cannot take is answered with an error
when its number can be read; a frame that is not an extension's page gets no
answer. A screenshot fails when its browser is gone or after 10 s.

## Events (engine to bridge)

| `event` | Carries | The bridge does | Since |
|---------|---------|-----------------|-------|
| `action.onClicked` | `tabId` (Chromium's), and since Phase 7 `strip` (the tab's seven strip fields, `View::strip_json`, when the published strip holds the tab) | `chrome.action.onClicked.dispatch(tab)`, `tab` being `await <Chromium's chrome.tabs.get>(tabId)` with `strip` laid over it | 1 |

Any other event is handed to every listener that the
namespace scripts registered with `__bridgeOnEvent`, as
`(event, args)`, in the order they were registered. The receipt is posted
before the listeners run. A listener that throws is logged, and the others
still run. The delivery is then acknowledged after all of them ran: `done`
with `ok: true`, or, when any listener threw, `done` with `ok: false` and
the first error's message. The relay reports that as `received` with a
failure, which the engine never retries (`policy::after_delivery`). Chromium's
`tabs.get` for the click is the one `core.js` kept in `native` before it
replaced the namespace, so a click's tab is Chromium's own object: its
`url` and `title` are there by Chromium's permission rule, not the
bridge's `Grants`. Chromium knows no window and no index for these tabs
(`windowId` -1, `index` -1), and an extension passes the click's tab on
(`sidePanel.open({tabId, windowId})`, `tabs.group({createProperties:
{windowId}})`), so the engine sends the strip's fields with the click
(`relay::click_message`, from `Hub::strip_of`) and the bridge lays them
over the object: `index`, `windowId`, `groupId`, `active`, `highlighted`,
`selected` and `pinned` are the strip's. A click sent while the published
strip did not hold its tab carries no `strip`; the bridge then asks the
engine (`tabs.stripOf`) before it dispatches, and only a tab nobody can
place keeps Chromium's fields. The click itself keeps Phase 1's path
(`Bridge::click`, `Payload::Click`) and its log lines; a keyboard
`_execute_action` takes the same path.

The engine's events are the same messages. It sends `deliver` with
`{id, event, args}` to a service worker only for an event that the worker's
own listeners registered (`tabs.onCreated`, `windows.onFocusChanged`, and the
rest of `differ.rs`'s list), and `bridge.wake` only when a served extension
with a worker is first seen; it starts a stopped worker and nothing else. So a
stopped worker is started only by an event it listens for. An extension page
is sent an event only for a listener its own frame registered, and that
registration ends with the frame (`state.rs`, `page_listeners`;
`pages.rs`, `PageLane::emit_to`; `Hub::tick`). A delivery
is queued as a job (`Job::Emit`) with a bound of 256 queued jobs, and each
has one delivery id, kept across its attempts, so an instance that ran it
ignores a repeat. An event is retried once, on a fresh relay, only when the
relay says the worker never received it (`policy::after_delivery`); a failed
evaluation is not retried. An event that cannot be delivered (its extension
is not on, its page has been given up, its queued event belongs to a worker
instance that was replaced, or the queue sheds it) is logged and dropped,
never delivered later, as an undeliverable click is.

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
Each opening is counted (`Opener::failed`), and the count is cleared by the
page's probe succeeding. A relay whose probe succeeded but whose worker
could not be reached (`identify` failed) is counted too (`Opener::unreached`,
cleared when a hello arrives). The schedule (`policy::next_opening`) counts
the two together. The gap before the second opening is 250 ms, and each later
gap doubles: 500, 1000 and 2000 ms before the fifth opening, which is the last
(`policy::gap_after`, `policy::next_opening`; the openings fall at 0, 0.25,
0.75, 1.75 and 3.75 s, and the sixth is never made). The gap is never below
250 ms, not even after a success, so a page that passes its probe and then
dies is not reopened on every turn (M19). After five failed openings in a row the engine gives up on
that page:

- it logs one warning that says so;
- a relay's debuggers are KEPT (not forgotten: the engine cannot hear the
  worker, but its sessions may still be live, and a debugged hidden tab must
  stay shown; they go by the list, by a tab's close, or by a hello; I14). Its
  queued work is dropped with a log line, and its clicks are refused from
  then on;
- the helper's queued work fails: clicks and events are dropped with a
  log line, and each debugger report is answered without a change; clicks
  are refused.

Nothing opens the page again until a sync sees its extension change (it
came on, went off, or was reloaded: off and on in two lists). Measured:
`run.sh broken` replaces the relay page in the throwaway profile and closes
it; the engine opens it five times, logs the give-up once, opens nothing for
12 s after it, and refuses a click (`review3-final-broken.out`, the cycle 3
build). Before that change the same provocation opened the page 229 times in
60 s with no give-up (`review3-broken-before.out`). Cycle 4 adds a debugged tab:
the worker attaches its debugger first, the give-up is logged with "its
debuggers are kept", and the tab is not hidden (`review4-broken-run1.out`).

## Limits in protocol 1

- Phase 5 answers `tabs.query` from the strip; this line is kept for the
  protocol 1 history. `windowId` and `index` are the strip's, not Chromium's.
- Only the methods in the table above are answered; everything else is
  refused, never passed to Chromium.
- An extension without a service worker is served through its pages alone:
  no worker is woken for it, and its pages get `chrome.tabs` and
  `chrome.windows` like a worker's.
- A host pattern is a host permission: in Manifest V3 only `host_permissions`
  grants one (Chrome ignores a pattern under `permissions`). A host pattern never grants a file page, since this browser grants no file access
  (`pattern.rs` `grants_url`); with the `tabs` permission an extension is given a
  file tab's address and title, and `tabs.query`'s `url` filter finds such a page
  (`pattern.rs` `matches_url`).
- An extension page's `iframe`s (`srcdoc`, `about:blank`) have a `chrome`
  object with no `tabs` or `windows`: measured on the bundle, with a probe page
  in a worker-less test extension, `typeof` is `undefined` for both. If an
  extension page's bridge cannot be put in, its native `tabs` and `windows`,
  and its `tabGroups` and `commands` where it has them, are replaced with
  namespaces that refuse every call (`js/refuse.js`,
  `__nautilusRefuseNative`), so a native `tabs.group` or `tabGroups.update`
  is never called; its `sidePanel` too, and all of them on `browser` as on
  `chrome`. The same holds for one namespace whose install fails, in a
  page and in a worker ("Fail closed", below).
- A message from a tab (`sender.tab`) is delivered synchronously, so Chromium gets
  the answer to its keep-open question at once: the wrapper returns true only when a
  listener of this context kept the channel open (returned true or a promise). A
  context whose listeners do not answer sends nothing, so another context's answer
  wins, and a message no context answers settles as Chromium settles it. The
  `sender.tab` fields come from the context's cache (`js/runtime.js`, `strips`):
  primed from `tabs.query` when a worker starts, kept by the worker's own
  listeners on `tabs.onCreated` (the tab is recorded), `tabs.onReplaced` (its
  entry moves to the new id, and the new id's strip is fetched) and
  `tabs.onRemoved` (the entry goes), and refreshed after each message from the
  engine's `stripOf`. Those listeners are the bridge's (`core.js` `tap`): they ask
  the engine for the events with `bridge.listen`, as an extension's first listener
  does, and the extension's `hasListener` and `hasListeners` never see them. The
  engine sends `tabs.onCreated` when it publishes a tab, under its Chromium id once
  the table knows it, so an extension page shown as a tab is in the worker's cache
  before the page can send its first message. The cost: a bridged worker is sent
  every tab's creation, replacement and removal, whether or not the extension
  listens for them. A page's cache is not primed and has no such listeners (its
  calls wait for its own report), so in a page a tab's first message still carries
  Chromium's own fields (index -1), and the next one the strip.
- `tabs.stripOf(id)` is internal (see the method table); it is not a
  `chrome.tabs` member.
- Extension pages get the bridge (`bridge/pages.rs`): an
  extension's popup, its options page and any page shown as a tab. The
  transport goes in when the page's context is created; the namespaces go in
  at the first change to the page's document, before its first script runs
  (`js/page_defer.js`; "The scripted run (phase5.py)" has the measurements). The relay page and the helper extension's pages
  get none. A page's calls go up through its own lane (the process message
  under "Messages Phase 5 adds"); an event reaches the pages that have
  called, at most once, and an event for a page that is gone is dropped.
- Extension pages still receive the bridge's port (`nautilus-bridge-1`).
  The wrapper that hides it is in the worker's script (`bridge.js`), and the
  page script does not hide it, so a page with its own `runtime.onConnect`
  listener sees the port.
- `chrome.debugger` called from an extension page (a popup, the options
  page, the side panel) is reported as a worker's is, by the page's own
  script (`js/debugger.js`) over the page lane ("A page's debugger",
  under "Calls"). A page's report for a debuggee the table cannot name
  yet is logged and not tracked (a worker's is queued for the next join).
- An unbridged extension's debugger is not reported. From Phase 5 on every
  installed extension is bridged when it is loaded: a store install
  (`unpack_crx`), "Load unpacked…" and the test control's `load` all go
  through `ExtensionManager::load_bridged`, and the first listing after a
  start bridges every unpacked extension under the profile
  (`bridge_into`). An extension that cannot take the bridge is switched
  off, with a message.
- A module worker (`"type": "module"`) gets a loader of two static
  imports, unit-tested as text. One module-worker extension has run
  behind it (the Claude extension, 2026-11-27, on a scratch profile:
  its worker said hello and its `sidePanel`, `tabs` and `tabGroups`
  calls were answered); no scripted run has one.
- The loader for a classic worker loads the bridge in a `try`: if the
  bridge fails to load, the extension's own worker still runs, unbridged.
- The session check is `sendCommand`, which runs `Runtime.evaluate` of `1` in
  each tab the target list names, every time the worker's hello is made (on a
  restart, or when the relay is asked). It is a trivial evaluation the
  extension did not ask for. A tab with no session of the extension is refused
  by Chromium at once ("not attached"); a tab whose probe does not answer within
  1 s is `unknown`, and kept.
- A reload is seen from three places: the list of extensions (`sync`), the
  worker target (a changed DevTools target, within about 2 s, by the periodic
  refresh), and the relay's port event (`gone`, which asks nothing). In the
  cycle 3 runs the list showed the extension off during the reload and `sync`
  forgot its debuggers (`review3-final-reload-*`, `review3-reload-after-attempt3.out`
  to `review3-reload-after-attempt7.out`). The case the list does not show (the relay page survives
  with no port event and the list is not off) was the zombie of cycle 3
  (`review3-reload-before-attempt1.out`, `review3-reload-after-attempt2.out`). Cycle 4
  reaches it on purpose with the list held (`run.sh zombie`): the worker target
  changes, the relay is opened again, and the new hello forgets the tab
  (`review4-zombie-run1.out` to `review4-zombie-run3.out`; the path line "forgotten by
  hello"). Not shown: a reload in which the worker's target id does not change
  (every reload and restart in the kept runs changed it; no run had one that
  did not, so the case is not exercised).
- A copy written by an earlier build is not handled: its `bridge.js` sends
  no hello or instance (cycle 3 and before), which the engine reads as no
  hello. Its hello with the old shape (`attached` instead of `sessions`, cycle
  4) is read by `table::hello_of` as a hello of its instance with no sessions:
  the identity is recorded (so a restart is noticed, and counts as a hello for
  the change rule), and nothing is forgotten, since no tab is named. A copy is
  rewritten on every load by this build, and the copies of a previous engine are
  replaced the next time the extension is loaded.
- A queued debugger report whose tab the table does not know yet is kept and
  applied when the table names the tab (M18), and its answer is not sent if its
  worker instance is gone. Tested (`policy::job_kept`, `policy::reply_due`),
  not seen in a run.
- A hello's session for a tab the table cannot name yet is kept pending and
  applied by the next join that names it (`Bridge::apply_pending`). Tested in
  its parts; not seen in a run.
- `chrome.debugger.getTargets` lists an extension's worker with type `worker`;
  the engine takes the first target of that type (or `service_worker`) whose URL
  is inside the extension. An extension with two such targets would be followed
  by its first only.

## Installing an unpacked extension

`bridge/install.rs` puts the bridge into an extension's directory.

- A folder the user loads is copied into the profile's store as
  `unpacked-<name>-<hash>` (`copy_name`). The copy is built beside its
  place and moved there only once the bridge is in, so a failed load leaves
  no partial copy and keeps the previous one. Links are not followed: each
  is skipped and logged. Dot-files and dot-directories at any depth are not
  copied (`.git`, `.DS_Store`).
- A worker whose entry is a link, is absent, or is under a dot-file or
  dot-directory fails the load, with a message that says which. Nothing is
  installed.
- The bridge's version is 16 hex digits of the SHA-256 of the bridge script.
  The bridged copy records it in `nautilus-bridge.json` beside the manifest
  (the marker). `ensure_bridged` rewrites a directory only when its marker
  names another version. A failed rewrite writes no marker, so the next call
  tries again.
- `bridge_source` is the load's entry point. A marked copy in the store is
  rewritten in place and not copied again, so a newer build's `bridge.js`
  reaches it. Any other folder is copied and bridged as above. Store
  installs and the start-up check reach the bridge through `ensure_bridged`.
- A rewrite of a marked copy reads its manifest first, so a broken manifest
  leaves the copy and its marker as they were. A failure while the bridge
  files are being written could leave some of them new; the next load
  rewrites all of them.
- Replacing a copy renames the old copy aside, moves the staging folder in,
  and deletes the old copy. A failed move renames the old copy back. Staging
  and aside folders carry a per-process number; copies are made one at a time;
  a load removes only the leftovers with exactly its copy's staging or aside
  names (`install.rs`, `replace_dir` and `sweep_leftovers`).
- A user's folder that an earlier build loaded (`bridge/migrate.rs`) is switched off
  while its copy is made, and the copy takes the state the original had once it is
  listed (`migrate::Migrations::listed`). A keyed copy keeps the id. A keyless copy
  has a new id: the original is not removed (Chromium refuses the engine's uninstall
  without a user gesture, and the gesture flag gave no removal; measured), so it stays
  in the list switched off, with its data under the old id, and the user removes it.
  If the copy cannot be made, the original stays switched off, with a message.
  Not measured: an original the user had switched off before the copy is made (the
  harness cannot switch it off first); `migrate.rs` has a pure test that such a copy
  stays off.

## Phase 5 reconciliation

This section records how the Phase 5 plan (tabs and windows, the side
panel) was reconciled with the bridge as Phase 1 built it, and which
adaptations the later tasks carry out. On 2026-11-18 the owner decided
the differences: everything Phase 1's reviews built is kept, and Phase
5's intent is applied around it; an event bound for an extension that
cannot be delivered is dropped, the way a click is.

Rows 1 to 12, in the plan's order, with the code's real names:

1. The worker's script: `bridge.js` `call(method, args)` posts `{call: {id, method, args, instance}}` (the instance was added by Phase 1's reviews); `run(delivery)` handled only `action.onClicked` and rejects any other event; the global is `self.__nautilusTransport`, frozen. Adaptation: Task 9 step 1a adds `__bridgeCall` and `__bridgeOnEvent`, hands non-click deliveries to the handlers; Task 11 step 1a makes the call's instance travel with the call (see row 2 and the list below).
2. `Bridge::reply(extension, instance, call, answer)` (`relay.rs`), not `reply(extension, call, answer)`: the instance is a parameter. Adaptation: Task 11 step 1a. `CallIds` maps a Bridge-local `CallId` to `(extension, instance, worker call id)`; the trait's `reply` takes the triple out of the map and calls `Bridge::reply` with the instance, which keeps Phase 1's signature and `nautilusRelay.reply(id, answer, instance)`.
3. `Bridge::worker_call(extension, item)` answered `tabs.query` with `Job::Query` and `table::query_rows`, routed `debugger.attached` and `debugger.detached` to `debugger_report`, and refused every other method. Adaptation: Task 11 step 1a keeps the debugger branch in front of the hub call (with the instance); every other method goes to `hub.call`; `Job::Query` and `query_rows` are deleted.
4. `table::TabTable` in `Bridge`'s `Inner`, with `chromium(tab)` and `tab(chromium)`. The table is refreshed by the helper's answer (`Pending::Targets`), by a periodic refresh every 2 s while a relay is open, and by `reopen`, `click` and the queued jobs; "only when a job waits" was not accurate. Adaptation: Task 11 step 1a adds `Bridge::chromium_id`; `target_known` sets a flag that the next turn's `reopen` acts on (no CEF call from inside the observer); the `Pending::Targets` arm calls `hub.table_changed()` after `run_jobs`.
5. As assumed: `DelegateState` has `last_focused` and `focused_browser` only, no tab-to-browser map. Adaptation: Task 11 step 2 adds `views`, `register_view`, `unregister_view` and `browser_of`.
6. `Bridge::track(tab, browser, view)` stores a `TabRecord { browser, view, _probe }`, and `TabProbe` takes every DevTools method result of its browser. Adaptation: Task 11 step 1a keeps `track`'s signature and `TabRecord`; a shared cell holds the message id of the probe's `Target.getTargetInfo`, reserved before the CEF call as `reserve`/`settle` do, and `TabProbe` acts only on its own result.
7. `install::rewrite_in_place(dir: &Path, skipped: &[PathBuf]) -> Result<Bridged, String>`: it takes the list of skipped links, which `entry_in_copy` needs. Adaptation: Task 10 keeps the two-argument signature; a directory that is already a marked copy is rewritten with an empty list. `BRIDGE_JS` is the constant in `install.rs`; Task 9 step 7 makes it `pub(crate)` and writes `worker_script()`.
8. As assumed: `bridge.js` has the block headed `// ---- chrome.tabs.query, answered by the engine`, which defines `tabs.query` with `define`. Adaptation: Task 9 step 1a deletes it; the click path keeps Chromium's `tabs.get`, captured before the namespace is replaced.
9. As assumed: no page-side bridge exists and extension pages receive the bridge's port (branch B). Adaptation: Task 12 steps 3 and 4 (branch B): pages get their own lane.
10. The relay page's URL is built from `BRIDGE_DIR` in `Bridge::open_relay`; no constant named the whole path. Adaptation: Task 11 step 1a adds `relay::RELAY_PATH` (`/nautilus-bridge/relay.html`) and uses it in `open_relay`.
11. As assumed: the harness is `ci/side-panel/` with `ext/manifest.json`, a classic worker `ext/background/sw.js` that begins `importScripts('lib.js')`, `driver.py`'s `Control` writing `cmd-N.json`, and `run.sh`; `{"load": dir}` goes to `ExtensionManager::load_bridged`. Adaptation: Task 13 step 1 uses the worker's real name, `background/sw.js`, and writes `importScripts('/phase5.js')` at its top.
12. As assumed: `on_list` builds `Known { id, popup, bridged, enabled }` from `ManifestInfo { facts, icon }`, and the `Extension` literal already has `side_panel: false` and `commands`; `INFO.extensions.side_panels` is `false`. The osmena pin in `Cargo.toml` is `e111e50` (not `a2d3463` or `07323f9` as the plan names it). Adaptation: Task 2 adds only what the compiler reports missing; Tasks 2 and 14 move the pin.

Adaptations carried by later tasks:

- B1, Task 11 step 1a (`worker_call`): keep the `debugger.attached` and `debugger.detached` branch calling `debugger_report` with the instance, before the `hub.call` routing. Without it the debugged tab stays hidden and the debugger stalls.
- B2, Task 11 step 1a (`CallIds`, `Bridge::reply`, the trait's `reply`): `CallIds` stores `(extension, instance, worker call id)`; `Bridge::reply` keeps Phase 1's four-argument signature; a new worker instance's hello drops the old instance's entries (`policy::reply_due` remains the rule for answers).
- B3, Task 11 step 1a: delete every `Job::Query` site and what only served queries (`Job::Query` in `Job`, `Job::extension`, `Job::kind`, `worker_hello`'s partition and its stale-query log, `fail_jobs`, `run`, `worker_call`), `table::query_rows` and its test, and `JobKind::Query` in `policy.rs` with the tests that only use it. Add the `Emit` arm to every exhaustive match that remains.
- B4, Task 11 step 1a and `policy.rs`: a `JobKind::Emit` behaves as `JobKind::Click` in `failed_by_loss` and `overflow`, and `job_kept` does not keep it across an instance change. A tab or window event that cannot be delivered is dropped, logged, and not delivered later.
- B5, Task 11 step 1a (`Pending::Emit`): the retry follows `policy::after_delivery`: a failed evaluation is not retried; a delivery the worker did not receive is retried once through `close_relay` in the callback and `queue(Job::Emit { attempt: 1 })`, with the relay reopened on the next turn. Do not call `open_relay` from a callback.
- B6, Task 11 step 1a (`emit`): push the job with `queue`, so the bound of 256 and `trim` apply.
- B7, Task 11 step 1a (`target_known`): set a flag; `reopen` on the next turn calls `refresh`. No CEF call from inside the observer.
- B8, Task 11 step 1a (`track`, `TabProbe`): reserve the probe's message-id cell before the call, as `reserve` and `settle` do, so a result that arrives inside the call is accepted; the observer ignores any other result.
- B9, Task 9 step 1a (`bridge.js`): capture Chromium's `tabs.get` before the namespace is replaced and use it on the click path; put the `if (delivery.event !== 'action.onClicked')` branch before the click branch and replace the final reject with it; wrap a handler call so a throwing handler cannot escape before `done` is posted.
- B10, Task 9 step 7 (`install.rs`): `BRIDGE_JS` becomes `pub(crate)`, so that `worker_script()` can join it with the namespaces.
- B12 and B13 and B18, Task 10 (`install.rs`): keep Phase 1's hardened copy (staging to `<name>.partial` and renaming into place; links skipped and recorded; `refuse_nested`; `entry_in_copy`; no copy left behind on failure) with all of its tests. Task 10 adds the marker and the copy without dot-files around it: the copy function returns the copy's path and the skipped list; `rewrite_in_place(dir, skipped)` keeps two arguments; a marked copy is rewritten with an empty list. The step 1 shim is not used to replace these tests.
- B14, Task 13 step 3 (`phase5.py`): its launch fails a run on a non-zero or signal exit, on a launch still up past the grace, and on the log markers `run.sh` checks (`panicked at`, `fatal runtime error`, `already borrowed`, `already mutably borrowed`, `browser(s) still open at shutdown`).
- B15, Task 13 step 1 (`ext/phase5.js`): before the Phase 5 run, show by running Phase 1's `run.sh` modes that `phase5.js` is inert when `phase5.py` is not listening.

Not determined at reconciliation:

- Whether CEF answers `Target.getTargetInfo` inside the `execute_dev_tools_method` call (the basis of the reservation in B8). It no longer matters: since 2026-11-19 the probe and the relay send under ids of their own, recorded before the call, and the reservation (`reserve`, `settle`) is gone.
- Whether `ext/phase5.js` is inert when no suite is listening (B15): settled by the scripted run, see "The scripted run (phase5.py)" below. Its results are the same as before, with one change the runs showed: its listeners let a delivered event start a stopped worker.
- Whether the Phase 5 code compiles against the bridge: settled by the compiler in Tasks 9 to 12, not by reading.

## Messages Phase 5 adds

Protocol 1 (Phase 1) carries the calls and events below; Phase 5 adds
the following, all through the same channel. The worker's calls now
carry `instance` (`{call: {id, method, args, instance}}`), and the
engine's reply carries the instance the call came from
(`nautilusRelay.reply(id, answer, instance)`): a reply for another
instance is ignored by the worker, so the engine answers each call
with the instance that made it. A delivery of `Job::Emit` is at most
once: the relay sends `{deliver: {id, event, args}}`, and an event the
worker does not take is dropped, as a click is.

The transport, as Phase 5 defines it (`bridge/transport.rs`):

```rust
pub(crate) type CallId = u64;

pub(crate) enum Asker { Worker, View(TabId), Popup { over: TabId } }

pub(crate) struct BridgeCall {
    /// The lane's id for the call; the hub replaces it with its own and keeps
    /// the lane's id (`Hub::routes`). For a worker call the lane's id is the
    /// Bridge-local id from `CallIds` (Task 11, Step 1a).
    pub id: CallId,
    pub extension: String,
    pub asker: Asker,
    pub namespace: String,
    pub method: String,
    pub args: Vec<serde_json::Value>,
}

pub(crate) trait BridgeTransport {
    /// Settle the promise of `call`: `Ok(value)` resolves it (JSON `null`
    /// is `undefined`), `Err(message)` rejects it with that message.
    fn reply(&self, call: CallId, result: Result<serde_json::Value, String>);
    /// Run the listeners of `event` (for example "tabs.onCreated") in the
    /// contexts of `extension` this transport serves, starting its worker
    /// if it is stopped and delivering exactly once.
    fn emit(&self, extension: &str, event: &str, args: &[serde_json::Value]);
    /// Start `extension`'s worker if it is stopped; deliver nothing.
    fn wake(&self, extension: &str);
}
```

In the script, `__bridgeCall(namespace, method, args) → Promise`
(rejecting with an `Error` whose message is the engine's) and
`__bridgeOnEvent(handler)` with `handler(eventName, args)`.

**The worker's call ids.** The `CallId` on this trait is the Bridge's
own: `Bridge` maps it to `(extension, instance, worker call id)`
(`CallIds`) and gives the hub that id in `BridgeCall.id`. A worker
numbers its calls itself, so two workers can both have a call 7.

**A forward** is an ordinary resolved value with one reserved key, so
the transport needs no third kind of reply:
`{"__forward": {"method": "get", "args": [123], "strip": {"123": {…seven fields…}}}}`.
The script calls Chromium's `method` of the same namespace with `args`
and lays `strip` over the tab objects in the result.

Calls added to the protocol: the namespaces `tabs` and `windows`, with
the methods of the two tables below; `tabs.stripOf(id)` (the seven strip
fields or `null`, used for `sender.tab`); and `bridge.listen(eventName)`
(sent by the script on the first `addListener` of an event, so the engine
fires only events someone listens to).

**Method tables, as built** (`bridge/router.rs`). `S` answered from the
snapshot; `R` becomes an `ExtensionRequest`; `F` forwarded to Chromium by
name with translated ids; `D` called on Chromium directly by the script;
`X` rejected. A tab id the strip lacks rejects `No tab with id: N.`; a
window id, `No window with id: N.`; `N` is always the id the extension
used. The router checks ids before it emits a request, and where the model
still refuses one (the tab went meanwhile) `router::refusal` puts the
extension's id back in place of osmena's `TabId`. `F` on a tab with a
synthetic id rejects `The tab with id N is not loaded.` (there is no
browser to ask).

`chrome.tabs`:

| Method | | How |
| --- | --- | --- |
| `query(info)` | S | every key of the reference; an unknown key rejects `Unexpected property: 'k'.`. `title` and `url` filters match only tabs the extension may see. `windowType` matches `"normal"` only. |
| `get(id)` | F / S | live: forward `get(id)`; synthetic: the snapshot's object |
| `getCurrent()` | F / S | `View(tab)` in a strip: as `get`; otherwise `undefined` |
| `create(props)` | R | `OpenTab { window, url, active }`: `windowId` (or none: the model's focused window), `url` resolved against the extension's origin and checked as Chromium checks it (default `about:blank`), `active` (default true; `selected` is its old name). `index`, `pinned`, `openerTabId` are ignored: a new tab goes where osmena puts new tabs. Reply `Tab(id)`; resolves with the tab object once the snapshot holding it is published. |
| `duplicate(id)` | R | `OpenTab` in the tab's window at the tab's URL, active; history is not copied |
| `remove(id \| ids)` | R | `CloseTabs(ids)`; resolves `undefined` once the published strip no longer shows them, so a query straight after does not list them |
| `update(id?, props)` | R / F / S | `active`, `highlighted` or `selected` true on a tab that is not active: `ActivateTab`, then (once the tab is active and live) forward `update` with the remaining keys, or `get` when none remain. Otherwise forward `update` with the remaining keys. `pinned`, and those three keys, are never forwarded. A `url` on a synthetic id without activation rejects as not loaded. No id: the current window's active tab. |
| `move(id \| ids, {index, windowId?})` | R | one `MoveTab { tab, index }` per tab, `index` being the tab's final index in the strip (osmena's rule), so the tabs land at `index`, `index + 1`, … (`-1`: the end; past the end: the end); resolves with the tab object or objects after the next publish. Another window's id rejects `Moving a tab to another window is not supported.` |
| `highlight({tabs, windowId?})` | R / S | activates the tab at the first index (`ActivateTab`); resolves with the populated window. `No tab at index: N.` |
| `discard(id?)` | S | resolves `undefined` ("not discarded"), after checking the id. Never forwarded. |
| `group(options)`, `ungroup(id \| ids)` | R | `GroupTabs`, `UngroupTabs`: see "Messages Phase 7 adds". Never forwarded: Chromium's `tabs.group` ends the browser process on these tabs. |
| `captureVisibleTab(windowId?, options?)` | engine | needs `<all_urls>` in the manifest; the window's active tab must be an `http`, `https`, `ftp` or `data` page, or one of the extension's own pages (any other, a file page included, is refused with a message that names its URL); `Page.captureScreenshot` on the window's active tab's browser; resolves `data:image/<format>;base64,…` |
| `stripOf(id)` | S | internal, and not a `chrome.tabs` member: the seven strip fields of tab `id`, or `null` (or `{panel: true}` for the id of a side panel's page of the calling extension, "Messages Phase 6 adds"). `runtime.js` asks for it to lay the strip over `sender.tab`. Any bridged caller can reach it through the transport, and it gives nothing `get` does not (no URL, no title). |
| `reload`, `goBack`, `goForward`, `getZoom`, `setZoom`, `getZoomSettings`, `setZoomSettings`, `detectLanguage` (tab id optional) | F | the id translated, or filled in with the current window's active tab (Chromium has no current window to fall back on) |
| `executeScript`, `insertCSS`, `removeCSS` (Manifest V2 only) | F | the same, defined only where Chromium defines them |
| `sendMessage`, `connect` | D | Chromium's own functions, bound |
| anything else Chromium defines | X | `tabs.<name> is not supported.` |

`setZoom` lasts until the tab's next navigation: osmena re-applies the
site's zoom then (its zoom convention).

`chrome.windows`:

| Method | | How |
| --- | --- | --- |
| `get(id, opts?)`, `getCurrent(opts?)`, `getLastFocused(opts?)`, `getAll(opts?)` | S | `populate` adds `tabs`. `getAll` with `windowTypes` lacking `"normal"` is `[]`. Objects: `id`, `focused`, `incognito: false`, `alwaysOnTop: false`, `type: "normal"`, `state: "normal"`; no bounds. |
| `create(data?)` | R | `OpenWindow { url }` with the first URL; each further URL an `OpenTab` into the window the reply names; resolves with the populated window. `incognito: true` rejects `Incognito mode is disabled.`; `tabId` rejects as unsupported; `type` and bounds are ignored (the window is an ordinary one). |
| `update(id, info)` | S | resolves with the window, changing nothing (no request exists for focus, state or bounds) |
| `remove(id)` | R | `CloseTabs` of every tab in it (a window whose last tab closes, closes: osmena's `an_extension_closing_every_tab_of_a_window_closes_the_window`). Limit: the only window's last tab goes through osmena's quit guard, so the browser quits, or, while downloads are going, the quit prompt shows, the tab stays, and the promise resolves at `SETTLE_LIMIT` with the window still there |
| anything else | X | `windows.<name> is not supported.` |

`onBoundsChanged` exists and never fires. A listener whose
`filters.windowTypes` lacks `"normal"` is never called.

**Events, in the order one publish yields them** (`bridge/differ.rs`,
`differ::diff`; the argument lists are those the script passes to the
listeners):

1. `tabs.onReplaced(added, removed)` for each tab whose id changed and that extensions already knew.
2. `windows.onCreated(window)` for each new window, in window order.
3. `tabs.onRemoved(id, {windowId, isWindowClosing})` for each tab gone from the model, by old window then old index; `isWindowClosing` when its window is gone too.
4. `tabs.onDetached(id, {oldWindowId, oldPosition})` for a tab now in another window.
5. `tabs.onCreated(tab)` for each new tab, by window then index.
6. `tabs.onAttached(id, {newWindowId, newPosition})`.
7. `tabs.onMoved(id, {windowId, fromIndex, toIndex})` per window: one event when taking a single tab out of both orders makes them equal (a drag), otherwise one per tab that has to be lifted, left to right.
8. `tabs.onActivated({tabId, windowId})` then `tabs.onHighlighted({windowId, tabIds: [tabId]})` for each window whose active tab changed (a new window's first tab included).
9. `tabs.onUpdated(id, changeInfo, tab)` for each tab present before and after whose `url`, `title`, `status`, `groupId` or `discarded` changed, with exactly those keys (`url` and `title` only for an extension that may see them; an event whose `changeInfo` would be empty is not sent).
10. `windows.onRemoved(id)`.
11. `windows.onFocusChanged(id)` when the focused window changed (`-1`: none).

The tab-group events of the same publish go around these: `tabGroups.onCreated` ahead of step 1, `tabGroups.onUpdated` and `tabGroups.onRemoved` after step 11 ("Messages Phase 7 adds").

The model moves a tab between windows by closing it and making a new one,
so steps 4 and 6 do not occur today; the differ still handles a `TabId`
that changes window.

An event goes to an extension only when one of its contexts has added a
listener for it this run (`bridge.listen`). The engine wakes an extension's
worker once when it first learns of the extension (`state::set_extensions`),
so top-level listeners are registered. A delivered event starts a stopped
worker that listens for it (see "The scripted run", below).

**The page's own tab (the report).** An extension page shown as a tab reports
its own Chromium tab id, so the table has a row for it as it has for a web page
(`sender.tab` in a worker then carries the strip's fields). The page's bridge
reads `chrome.tabs.getCurrent()` from Chromium's own function, which `core.js`
keeps in its closure when it replaces the namespaces and hands over once
(`__nautilusBridge.takeCurrentTab`, deleted when taken); `js/page_report.js`
runs in the deferred install after the replacement and takes it before the
page's first script. Nothing else of Chromium's replaced namespaces is on
`__nautilusBridge` (its members are `call`, `tap`, `define`, `pin`,
`BridgeEvent` and `installed`), which every script of a bridged context
can read: the worker's click takes Chromium's `tabs.get` for itself when
`bridge.js` loads, and `bridge_check.mjs` walks the object in both kinds
of context. The report is one process message,
`nautilus-bridge-tab` (`pages::REPORT_MESSAGE`), whose one string argument is
`{"tab": n}` (`pages::report_of`): an object with only that key, holding a
positive integer below `SYNTHETIC_BASE`. It carries no view, tab or extension.

The browser process decides the view. `pages::report_source` accepts a report
only from the main frame of a bridged extension's page (`extension_of`, which
refuses the relay page, the helper's pages and Chromium's own extensions,
`CHROMIUM_OWN`: the PDF viewer) whose browser is a tab view
(`Asker::View`, so not a popup, a worker, or a view that is no tab). A refused
report is logged once with its reason and ignored.

`TabTable::report` (`bridge/table.rs`) records or refuses the id (`Report`):
- a view with no DevTools target yet, or whose target is not a page of the
  extension the report came from, is refused;
- a view that already has an id from any source keeps it: the same id is
  `Same`, another id is refused, so a page's report never overwrites a row the
  helper's listing gave;
- an id the listing names for another target is refused, and so is an id another
  tab has;
- otherwise the report is recorded and the rows are joined again.

A recorded report is a row only while its view's target is still a page of the
extension it was made from (`prune_reports`), so it does not outlive that page:
it ends when the view navigates to a web page or another extension, and when the
view closes (`forget`). Then the helper's listing joins the web page as before. A
recorded report calls `table_changed` at once (`Hub::report_tab`), so the strip
publishes the id: a tab that was published with a synthetic id takes its
Chromium id by the existing path, and an extension sees `tabs.onReplaced` with the
new and the old id (the state test "the table catches up").

The page's calls wait for the report. `js/page_transport.js` holds each call
until `__nautilusReportWait` releases it; `page_report.js` releases the calls
after the report is sent, or after `REPORT_WAIT_MS` (1 s), whichever comes first.
The report is sent before the first call on the same ordered channel, so the
browser handles it first, and a call is never held for longer than that bound. A
page that is no tab, an answer that fails, or an answer that never comes sends
no report and releases its calls after the bound.

Known limit: a page could report the id of a tab the helper has not listed yet.
The helper's listing wins once it names that id for another target (the report's
row is skipped, and new reports for it are refused).

**Id spaces** (`bridge/ids.rs`):

- **Tab ids.** An extension sees **Chromium's tab id** for a tab that has a browser and is not deferred, so every id-addressed Chromium API accepts it unchanged. A tab with no Chromium id (a deferred tab; a live tab whose id the table could not learn within two seconds) has a **synthetic id**: the first free integer at or above `2_000_000_000` (`ids::SYNTHETIC_BASE`), never one the table holds, stable for that tab until it closes. A deferred tab that is being warmed has a browser, but stays on its synthetic id: the warm's browser is temporary.
- **When a tab's id changes** (a deferred tab is shown; a late table entry arrives) the bridge fires `chrome.tabs.onReplaced(addedTabId, removedTabId)`. The old id **keeps resolving** in every bridged and forwarded `chrome.tabs` call for as long as the tab lives (the router translates it), so an extension that ignores `onReplaced` still works through `chrome.tabs`; only APIs the bridge does not see (`scripting`, `debugger`) need the new id, which `tabs.get(oldId)` returns.
- **A new live tab is held back until its id is known.** The engine keeps the snapshot it was given and publishes it (answers queries from it, diffs it for events) only once the table has an id for every tab that is not deferred, or two seconds have passed (`state::PUBLISH_WAIT`). So `onCreated` carries the real id in the ordinary case, and `tabs.create` resolves after that publish with the tab object extensions will go on seeing.
- **A worker's tab calls wait for the real ids too (2026-11-27).** A worker's call was never held, so a worker that started early was answered from whatever was published: no tabs before the first strip, and synthetic ids for live tabs when the strip had been published before the helper's first listing (a browser that starts and then has an extension installed: the strip goes out after two seconds, the helper lists a moment later). An extension that kept such an id and handed it to Chromium's own `debugger.attach`, `scripting.executeScript` or `tabs.sendMessage` got "No tab with id" (the cause of `run.sh pump`'s occasional "no page target for the background tab"). `BridgeState::worker_call` now holds a worker's call that reads or names tabs (`hands_out_tab_ids`: every `chrome.tabs` function but the bridge's own `stripOf`, and `windows.get`, `getAll`, `getCurrent` and `getLastFocused` asked for with `populate: true`) while the ids are pending (`ids_pending`): nothing is published yet, or a live tab of the published strip still has a synthetic id and either the helper's first listing has not been joined (`table_joined`, called by the relay for a listing that parsed) or a strip is waiting for its ids. The held calls are answered in arrival order (`Hold::Ids`, in `settle`), each after `PUBLISH_WAIT` at most with what there is; a call that waited the bound out for a listing that never came ends the waiting for the calls after it. Every other worker call is answered at once, and a page's calls are not queued behind a worker's held one.
- **Window ids** are osmena's: `windowId = WindowId.0`. The model counts from 1, so an id is never 0, `WINDOW_ID_NONE` (-1) or `WINDOW_ID_CURRENT` (-2). Chromium's own window ids never reach an extension (the whole `windows` namespace is replaced, and forwarded tab objects have `windowId` overwritten) and no window id is ever forwarded to Chromium, so the two spaces cannot meet.
- **Group ids** are osmena's: `groupId = FolderId.0` (from 1), `-1` (`TAB_GROUP_ID_NONE`) for a tab in no folder. `ids::group_id` maps a folder to its id and `groups::folder_of` an id back to its folder ("Messages Phase 7 adds").

`tabs.js` also keeps Chromium's `onZoomChange` as a native event, which the
engine does not fire.

Extension pages have their own lane (`bridge/pages.rs`): a process
message `nautilus-bridge-call` with one string argument
`{"id", "ns", "method", "args"}`, answered by evaluating
`self.__nautilusBridgeDeliver("<json>")` in the frame with `{"id", "ok"}`,
`{"id", "error"}` or `{"event", "args"}`. The browser process takes the
extension from the calling frame's own address, never from the message,
and drops a call from a frame that is not an extension's page. A page's call
for a tab that the published strip does not hold yet is held until the strip
publishes, or for `PUBLISH_WAIT` (2 s); a call of an extension the hub does not
serve yet is held until it is served, or for the same bound, and then refused
with `the extension is not bridged.`. A page's call always gets a reply.


## Messages Phase 6 adds

Phase 6 (`chrome.sidePanel`) adds calls and two events to the same
channel, on both lanes, and no new kind of message. The script is
`js/side_panel.js`, the last of `namespaces_script()`.

**Calls.** The namespace `sidePanel`, and one method of `runtime`:

| Call | Arguments | Answer |
| --- | --- | --- |
| `sidePanel.getOptions` | `[{tabId?}]` | `{enabled?, path?, tabId?}`: the tab's own options (then with `tabId`), else the default ones, else the manifest's |
| `sidePanel.setOptions` | `[{tabId?, path?, enabled?}]` | `null`; an open panel whose place is no longer active closes, one whose page changed shows the new page (requests of the engine's own, below) |
| `sidePanel.getPanelBehavior` | `[]` | `{openPanelOnActionClick}` |
| `sidePanel.setPanelBehavior` | `[{openPanelOnActionClick?}]` | `null`; written to `<profile's Chromium directory>/side-panel-behavior.json` when it changed |
| `sidePanel.open` | `[{tabId?, windowId?}]` | `ExtensionRequest::OpenSidePanel`; `null` once the model is done, or Chromium's rejection for the case |
| `sidePanel.close` | `[{tabId?, windowId?}]` | `ExtensionRequest::CloseSidePanel` when this extension's panel is open there, else `null` at once |
| `sidePanel.getLayout` | `[]` | `{side: "right"}` |
| `sidePanel.closeSelf` | `[]` | internal, and not a `chrome.sidePanel` member: what the script's `window.close` asks. From a side panel's own page it is `CloseSidePanel` for that panel (answered `null` when the model is done); from any other page it is `false`, and the script then calls Chromium's `window.close` (as it does when the call fails, or is not answered within 500 ms). |
| `runtime.openPanels` | `[]` | internal: the extension's open panels now, `[{view, page, windowId}]`. The script asks it at the moment `getContexts` is called, before it asks Chromium, and hands the answer back as the third argument below |
| `runtime.getContexts` | `[filter, originals, panels]` | the contexts: `originals` is Chromium's own answer for `{}` and `panels` the answer to `openPanels`; the engine leaves out the bridge's own relay page (Chromium lists it as a `TAB` of the extension's), retypes or adds an entry as `SIDE_PANEL` in its window for each panel in `panels` and applies `filter`. Without a third argument that is a list (a worker script written before 2026-11-27, until its extension is rewritten) the panels are the ones open when this call arrives |

**`getContexts` answers for the moment it was called.** Chromium's own
answer takes a turn of the event loop or more, and until 2026-11-27 the
engine was asked only when it had come back, so the engine's part could
describe a later moment than the call: an extension that asks for its
contexts, calls `sidePanel.open` and then reads the answer to learn
whether its panel was open (the Claude extension does, to decide whether
to group the tab) could be told of the panel it had just opened. The
panels are now read first (`openPanels`, sent as the call is made and
ahead of any later call on the same lane) and travel back to the engine
with Chromium's answer. They are the calling extension's own word about
its own answer, so `side_panel::panels_of` reads them and nothing else
depends on them.

**Only an extension with a side panel pays for this** (review of Phase
6, I1). The script takes Chromium's `chrome.sidePanel` in the context as
the `sidePanel` permission. Without it, nothing named `sidePanel` is
installed, `window.close` stays Chromium's own function (the same
object), and `runtime.getContexts` is Chromium's own answer to the
caller's own filter with the relay page filtered out in the script: the
engine is never asked. With it, neither call waits on the engine for
long: `getContexts` falls back to Chromium's `originals`, less the relay
page and with the `ContextFilter` applied by the script (the rule of
`side_panel::passes`), when the engine rejects either of its two
questions or has not answered one within 1 s (`GET_CONTEXTS_WAIT`, each;
the first runs while Chromium answers), and `window.close` closes the page
natively on a rejection at once and after 500 ms without an answer
(`CLOSE_WAIT`). In the fallback an open panel's entry keeps the type
Chromium gave it. Known: a panel's own page that calls `getContexts`
while its first calls are still held (`Hold::Tab`, up to 2 s) gets the
fallback.

**The permission.** A `sidePanel` call of an extension whose manifest
does not hold the `sidePanel` permission is rejected (`chrome.sidePanel
is not available: the extension's manifest does not hold the "sidePanel"
permission.`), whichever context made it: the script installs nothing
for such an extension, but any context can call the bridge directly.
`sidePanel.closeSelf` and `runtime.getContexts` are answered for every
extension. The manifest's `side_panel.default_path` without the
permission is no panel, as in Chromium, which parses the key for any
Manifest V3 extension and makes a panel only for one with the
permission (`SidePanelStore::listed`, `ManifestFacts::declares_side_panel`).
`optional_permissions` are not counted. A tab's own options
(`setOptions({tabId})`) are dropped when the published strip no longer
holds the tab (`SidePanelStore::keep_tabs`). `setOptions` that gives a
tab its first own options while the window's panel is shown on that tab
changes nothing on screen, as in Chromium 156. The behaviour file is
written to `side-panel-behavior.json.tmp` and renamed into place.

Any other `sidePanel` method rejects
`chrome.sidePanel.<name> is not supported in this browser.`; any other
`runtime` method is not bridged at all (the script replaces only
`getContexts`, and only where Chromium defines it). The script checks
the arguments before it calls, so a malformed call rejects with a
`TypeError` (`Error in invocation of sidePanel.<name>: ...`) and never
reaches the engine. No user gesture is asked for.

**Events**, each with one argument, `{windowId, path, tabId?}` (`tabId`
for a tab's own panel only): `sidePanel.onOpened` when a panel's view was
made, `sidePanel.onClosed` when it went. They are `BridgeEvent`s of
`js/core.js`, so a context announces its first listener with
`bridge.listen` like any other event, and the engine sends one only to
the contexts of the panel's extension that announced it: the worker
(`Effect::Emit`, which starts a stopped worker that listens) and each
listening page (`Effect::EmitPage`). `BridgeState::emit` decides; a
stopped worker that does not listen is not started.

**Who answers.** These calls are not rows of `router::route`.
`BridgeState::call` hands them back as `Effect::SidePanel(call)`, after
the `bridge` namespace and for a page's call after its holds
(`page_call`, `settle`), so the order of a page's calls and the
`the extension is not bridged.` refusal apply to them as to `tabs`.
`Hub::run` gives the effect to `Hub::side_panel_call`, which answers from
two stores in `DelegateState`: the options and behaviours
(`bridge::side_panel::SidePanelStore`) and the open panels
(`panel::PanelRegistry`). The answer becomes effects through
`BridgeState::outcome_effects`, the same function the router's outcomes
go through; a request is answered with `Then::Undefined` (`null` on
`Done`, the refusal's text on `Refused`).

**Requests of the engine's own.** A panel that `setOptions` closes or
re-pages, and a panel an action click opens or closes, are
`ExtensionRequest`s no call waits for (`Effect::EngineRequest`,
`DelegateState::request_of_engine`). Their token has bit 63 set
(`ENGINE_TOKEN`); the bridge's own tokens count up from 1 and never reach
it. `ExtensionManager::extension_reply` passes every reply to
`DelegateState::note_reply` first, which settles what the panel registry
expected of the request and ends a reply to an engine token there.

**The current window of a panel's page.** It is the window the engine
opened the panel in, by the panel registry (`PanelRegistry::window_of`):
the hub asks it for a `sidePanel` call, and gives the state every open
panel's window before a call is routed (`Hub::sync_panels`,
`BridgeState::set_panel_windows`), so `tabs` and `windows` calls, which
are the router's, answer `currentWindow`, `windows.getCurrent` and
`WINDOW_ID_CURRENT` from it too (`View::current_window`). The strip
names only the panel a window shows, and only once the shell has
described it, which is after a panel's page can make its first call: so
the strip is asked second. A panel whose window the strip no longer
holds falls back to the window focused last.

**A panel's page and the tab report.** A panel's page runs
`js/page_report.js` like every bridged page. Its view is no tab and has
no row in the table (`TabHost::create_view` never calls `track_tab`), so
`PageLane::report` never hands a report from a registered panel's view to
the table (`pages::panel_report`). The page's calls still wait for the
report's bound (1 s at most), and a call from a panel's view is held
like a call from a tab the strip does not hold while a strip is pending
(`Hold::Tab`, 2 s at most).

**A panel's page is no `sender.tab`.** Chromium takes the panel's
windowless browser for a tab, so a message from the panel's page reaches
its extension with `sender.tab = {id, index: -1, windowId: -1, ...}`,
where Chrome gives a side panel's sender no `tab`. The id in the panel
page's report is therefore kept, beside the tab table and never in it
(`table::PanelIds`, held by `BridgeState`: the view, its extension, the
id), when the report comes from the main frame of a page of the panel's
own extension, the view is an open panel, and the id is no strip tab's
(`BridgeState::panel_reported`). It goes when the panel closes
(`set_panel_windows`). The scripts learn the ids three ways, all in
`js/runtime.js` and only in a context that has `chrome.sidePanel`:

| | |
| --- | --- |
| `bridge.panels` (call, `[]`) | the Chromium tab ids of the calling extension's open panels, asked once as the context starts |
| `bridge.panels` (event, `[ids]`) | every id of the extension's panels, sent when one is learnt to the contexts of that extension that announced the event (`bridge.listen`, a tap of `js/core.js`); a stopped worker that listens is started by it, as by any event. Nothing is sent when a panel closes: Chromium does not give the id out again in the run, and the next list leaves it out |
| `tabs.stripOf(id)` answering `{panel: true}` | for an id that is a panel's of the calling extension, in place of the `null` an unknown tab gets: what a context that was never told learns after the first message |

`onNative` gives a message whose `sender.tab.id` is one of those ids to
the extension's listeners without `sender.tab` (deleted, or a copy of
the sender without it where the property does not let go). An id is
told only to the panel's own extension, and nothing is inferred from an
id being unknown to the context: a content script's tab the cache has
not learnt keeps its `sender.tab`.

**A panel's first message waits at the sending end (2026-11-27).** A
message is delivered at once where it arrives, and the panel's report
travels page to browser to hub to relay to the worker's port while the
page's own `runtime.sendMessage` goes straight through Chromium, so a
message the page sent as it loaded used to arrive first, with the tab.
It is now held in the page (`js/page_report.js`), in every page of an
extension that has `chrome.sidePanel`:

| to the page, as an event of the page lane | |
| --- | --- |
| `bridge.reported` (`[{panel}]`) | the engine's answer to the page's report of its tab id: whether the page is a side panel's. `PageLane::report` answers every report; only the main frame of a panel's own page is told `true` (`BridgeState::panel_reported`) |
| `bridge.told` (`[]`) | to a panel's page: the extension's other contexts have been told its id. Sent at once when no worker has to be told (none listens for `bridge.panels`, or the id is one they have), and otherwise when the worker has taken that delivery (`Effect::EmitTold`, `BridgeTransport::emit_told`, `Bridge::delivered`, `Hub::told`, `BridgeState::told`), or when the delivery is given up. Pages that listen are sent the ids before it, unacknowledged |

`runtime.sendMessage` (one function for `chrome.runtime` and
`browser.runtime`) is replaced by a holder until the page knows: it
waits for `bridge.reported` under the report's own bound
(`REPORT_WAIT_MS`, 1 s from the script's start; a page with no tab to
report, a popup, is let go as soon as Chromium has said so), and in a
panel's page then for `bridge.told`, for at most 1 s more
(`PANEL_TOLD_WAIT_MS`). Then each held message is sent with the
arguments it was given, in order, and its answer is Chromium's own: the
promise settles as Chromium's does, a callback is Chromium's to call
with `runtime.lastError`. From then on the property holds Chromium's
function again, identically. The state forgets a wait nobody reported
on after `PUBLISH_WAIT`. Seen in `panel.sh panel`: the at-load message
of each of the run's panel pages reached the worker without
`sender.tab`, the word coming within the same second as the report.

What remains. (1) The receiving context has just started (a worker a
later message itself woke, a popup that opens) and its own question
(`bridge.panels`) has not been answered: that message carries the tab,
and the next does not (the answer to `stripOf` marks the id). (2) A
worker that does not take the delivery within the second (a slow
start): the page sends at its bound. (3) `runtime.connect` returns its
port at once and cannot be held, and a port (`runtime.onConnect`'s
`port.sender`) is not wrapped by the bridge at all, for tabs or panels:
its `sender.tab` is Chromium's. A held `sendMessage` whose arguments
Chromium refuses rejects (or logs, with a callback) where the unheld
call throws.

**The action click.** `ExtensionManager::click` is the one decision
(`side_panel::decide_action_click`), in Chromium's order
(`ExtensionActionRunner::RunAction`): the panel when the extension asked
for it with `setPanelBehavior` and the tab has an active panel
(`Hub::side_panel_click`), even when the action has a popup; else the
popup if the action has one; else `action.onClicked` through the bridge
as before. The toolbar
(`TabHost::open_extension_action`) and the test control's `click` both
take it; the control's log line says `sent to the bridge`, `not sent`,
`opened or closed its side panel`, or `the action has a popup, which the
control does not open`.


## Messages Phase 7 adds

Phase 7 adds tab groups (`chrome.tabGroups`, `tabs.group`,
`tabs.ungroup`) and keyboard commands (`chrome.commands`) to the same
channel, on both lanes, and no new kind of message. They are ordinary
namespaces by the recipe below: `js/tab_groups.js` and `js/commands.js`
each call `define`, their calls are rows of `router::route`, and their
events are a `StripEvent` (groups) and `BridgeState::emit` (a command,
which is no difference of the strip). Each script replaces its namespace
only where Chromium defines it, so an extension without the `tabGroups`
permission, or with no `commands` in its manifest, gains none.

**A tab group is a folder** (`bridge/groups.rs`, pure). `groupId` is the
folder's number (`FolderId.0`, from 1; unique across windows), `-1` for
none; `groups::folder_of` maps an id back. The object is
`{id, collapsed, color, title, windowId, shared}`: `collapsed` is "not
expanded", `title` the folder's name, `color` always `"grey"` (folders
have no colour), `shared` always `false`. A folder with no tab is no
group.

| Call | | How |
| --- | --- | --- |
| `tabGroups.get(id)` | S | the group's object; `No group with id: N.` |
| `tabGroups.query(info?)` | S | every group, in strip order, filtered by `windowId` (`-2`: the asker's current window), `collapsed`, `title` (Chromium's pattern: `*` any run, `?` any one character, case matters), `color` (only `"grey"` matches) and `shared` (only `false` matches) |
| `tabGroups.update(id, {title?, collapsed?, color?})` | R / S | `UpdateFolder { folder, name, expanded }`; resolves with the group as it will be, once the strip shows the change. A blank `title` resolves as `"<extension name> Session"`, the name the model gives. `color` is accepted and ignored; with `color` alone nothing is asked and the group is answered as it is. |
| `tabGroups.move(id, props)` | X | `tabGroups.move is not supported.`: the model has no request that moves a folder. The script sends it to the engine, which refuses it. |
| `tabs.group({tabIds, groupId?, createProperties?})` | R | `GroupTabs { tabs, folder }`; resolves with the group's id (a number). `tabIds` is one id or a list (each tab once; a former id of a tab still names it). Without `groupId` the model makes a folder named `"<extension name> Session"`, expanded, where the first tab is. `createProperties.windowId` naming the tabs' own window (or `-2`) is accepted. Rejections, in Chromium's order and words where it has them: `Cannot specify 'createProperties' along with a 'groupId'.`, `No group with id: N.`, `No window with id: N.`, `No tab with id: N.`; and `Tabs can only be grouped within their own window.` for tabs of two windows, or a group or window that is not the tabs' own (Chromium would move the tabs; the model has no such request). |
| `tabs.ungroup(id \| ids)` | R | `UngroupTabs(tabs)`; resolves `undefined` once the strip shows the tabs in no folder. A folder emptied by it goes. |
| `commands.getAll()` | S | `[{name, description, shortcut}]`, by name: the commands the manifest declares, each with the shortcut it suggests for this platform as Chrome writes it (`⌘E`, `⇧⌘U` on macOS; `Ctrl+Shift+Y` elsewhere), `""` when it suggests none or one Chrome would not accept. `_execute_action` is listed. |
| anything else Chromium defines in either | X | `<namespace>.<name> is not supported.` |

`tabGroups.TAB_GROUP_ID_NONE` and `tabGroups.Color` are Chromium's own
values, copied by `define`.

**When a group call resolves** (`router::settle`, `Then::Group`). Each
resolves on the published strip that shows its own change, so a read
made straight after the promise sees it; a strip published since the
reply is not enough, since the engine republishes the strip it has when
the tab-id table changes. `tabs.group` resolves once every tab the call
named (that the strip still holds) is in the folder the model's reply
names, in a new group and in one that existed
(`GroupThen::GroupId { tabs }`), so `tabGroups.get(id)` and
`tabs.query({groupId})` straight after find the group and all its tabs.
`tabGroups.update` resolves once the strip's folder has the title and
the collapsed state that were asked for (`GroupThen::Updated`; a request
for what the folder already has resolves with the reply); a blank title
is the model's to word, so with nothing else asked it resolves on a
strip published after the reply, or `SETTLE_QUIET` after it.
`tabs.ungroup` resolves once every named tab is in no folder
(`GroupThen::Ungrouped`). Each resolves at `SETTLE_LIMIT` all the same if
the strip never shows it. A refusal of the model's rejects at once, in
the extension's ids.

**The group events** (`groups::group_events`, as `StripEvent::Group`),
each with the group's object as its one argument, whoever made the
change (an extension or the user):

- `tabGroups.onCreated` for a folder that holds a tab now and held none
  before: ahead of every tab event of the same publish, so the group
  exists before a tab's `tabs.onUpdated` names it.
- `tabGroups.onUpdated` for a group whose title or collapsed state
  changed, and `tabGroups.onRemoved` (the group as it last was) for a
  folder that holds no tab any more: after the tab events, so the tab's
  `tabs.onUpdated` with `groupId: -1`, or its `tabs.onRemoved`, comes
  first.
- A group whose tabs are now in another window is removed there and
  created here (both ahead of the tab events).
- `tabGroups.onMoved` exists and never fires.

A tab's own change of group is still step 9 above, `tabs.onUpdated(id,
{groupId}, tab)`: a tab whose address and group change in one publish
gets one event with both keys.

**A command** (`chrome.commands.onCommand`). The shell resolves a key to
an extension's command after its own keymap and calls
`TabHost::extension_command(tab, extension, name)` with the window's
active tab. `_execute_action`, `_execute_browser_action` and
`_execute_page_action` are the toolbar's click (`ExtensionManager::click`,
"The action click" above; a popup hangs from the top right corner of the
page area) and fire no event. Any other name is `BridgeState::command`:
the event `commands.onCommand` with `[name, tab]`, the tab as that
extension may see it (`View::tab_json`; no tab when the strip does not
hold it), sent by `BridgeState::emit` to the contexts of the extension
that announced a listener: the worker (started if it was stopped) and
each listening page. `Hub::command` logs `extension <id>: command "<name>"
for TabId(n)`, with `: no context listens for it` when nothing was sent.
Like every event sent through `emit`, one queued for a worker instance
that is replaced before it is sent is dropped.

`commands.getAll` reports the declared shortcut whether or not the
shell's keymap leaves the chord to the extension: the engine is not
given the keymap. A chord the user's own keymap binds is the keymap's,
and is still listed here.

**The test control** (a build with `test-hooks`):
`{"extension_command": {"extension": "<id>", "name": "<command>"}}`
does what `TabHost::extension_command` does, for the tab focused last,
short of opening a popup. Its log line is `TEST HOOK: command N:
extension_command "<name>" of <id> for Some(TabId(n)): ` and then
`handed to the bridge`, or for an action command the click's own words
(`sent to the bridge`, `not sent`, ...), or `not sent (no tab to command
for)`.

**Fail closed.** No call of these namespaces reaches Chromium: the
scripts leave no function of Chromium's in a replaced namespace
(`define`), the router rejects what it does not name, and a page whose
bridge cannot be put in has its `tabGroups`, `commands` and `sidePanel` refused with
its `tabs` and `windows` (`__nautilusRefuseNative`), on `chrome` and on
`browser`: every replacement goes through `pin` (below), which covers
both globals.

The refusal is `js/refuse.js`, the first script of every bridged context,
a worker's as a page's. Each namespace is installed in a guard of its own,
twice over: the core's `install` (what `define` and `sidePanel` call)
catches a `pin` that throws, and `namespaces_script` (`bridge/mod.rs`)
wraps each script in a `try` whose `catch` refuses that script's
namespace (the core's: all five). So one namespace that cannot be put in
is refused on both globals and the ones after it are installed all the
same; before 2026-11-27 they ran in one `try` and a failure left every
later namespace Chromium's. The refusal takes what it finds: a property
that can be redefined is; one that can only be assigned is; where neither,
every function and event of the object it holds is replaced in place
(which also finishes an object `pin` left half replaced); and where a
member cannot be replaced either (a frozen object), the global itself,
`chrome` or `browser`, is replaced by a stand-in that answers the refusal
under that name and Chromium's object for every other. That last resort
is not seen in Chromium, whose API properties can be redefined; a context
it happens in gains no API afterwards, since Chromium defines nothing on
a stand-in. A refused namespace's events take listeners and never fire,
so an extension's first evaluation does not throw on
`onUpdated.addListener`. The worker's transport is guarded too: if it
throws, the core finds no transport and refuses all five. The tests are
`every_function_of_a_replaced_namespace_is_ours` and
`tab_groups_and_commands_are_replaced_whole_and_refused_when_they_cannot_be`
(`bridge/mod.rs`), `group_calls_and_the_command_listing_are_routed`
(`bridge/router.rs`) and the group and command parts of
`ci/side-panel/bridge_check.mjs`. `phase5.py`'s two fail-closed checks
call `tabGroups.move`, the one function of these namespaces the engine
still refuses.


## How a later phase adds a namespace

**A namespace is replaced on both globals, in one place.** Chromium 156
gives an extension context `browser` beside `chrome`, with every API
set on both (`docs/upstream-issues.md`, C19). `__nautilusBridge.pin(name,
replacement)` in `js/core.js` is the only code that puts a replacement
in: it defines it on `chrome` and the same object on `browser` (where
the context has one), not configurable and not writable, and where a
property cannot be redefined it replaces every member of the object the
property holds (the replacement's members, a rejection for any other
function, an inert event for any other event). `define` calls it, and
so does the hand-written `sidePanel`; no namespace script sets a
property of `chrome` or `browser` itself (a test holds them to that).
So `browser.tabs === chrome.tabs` in a bridged context, and likewise
`windows`, `tabGroups`, `commands` and `sidePanel`; a context without
`browser` gains none. `commands` is the one namespace not pinned
(`defineInPlace`, C19): the property stays Chromium's on both globals
and Chromium's own object, one for both, has its members replaced in
place, so Chromium's update of a live context's APIs does not stop at
`commands`; where a member cannot be replaced it is pinned like the
rest. What the bridge wraps without replacing
(`runtime.getContexts`, `runtime.onMessage`, the worker's
`runtime.onConnect`, `debugger.attach` and `detach`) it wraps on
Chromium's own object, which is one object for both globals;
`core.js`'s `same` gives `browser` `chrome`'s object should the two
ever differ. The fail-closed refusal (`__nautilusRefuseNative`,
`js/refuse.js`) covers both globals and `sidePanel`, with one refusing
object per namespace, pinned.

1. A file under `bridge/js/` calls `__nautilusBridge.define('<name>', { bridged, optional, direct, events, nativeEvents })` (the comment on `define` in `js/core.js` says what each list does; `js/tabs.js` and `js/windows.js` are the examples). `bridged` names the functions the engine answers, `optional` the same where Chromium defines them, `direct` Chromium's own functions, called bound, `events` the events the engine fires, `nativeEvents` Chromium's events, kept.
2. Its name is added to `namespaces_script()` in `bridge/mod.rs`, so the file's text follows the others in a worker's and a page's script.
3. Its rows go in `router::route` (`bridge/router.rs`): one arm per namespace, one per method, each answered as S, R, F, D or X in the method tables above.
4. Its events are `StripEvent` variants (`bridge/differ.rs`) produced by `diff`, with a `render` arm giving the event name and its argument list.
5. Its names are covered by the test `every_function_of_a_replaced_namespace_is_ours` (`bridge/mod.rs`), which checks that every bridged or optional method of the `tabs`, `windows`, `tabGroups` and `commands` scripts reaches the router (`tabGroups.move` excepted, which the router refuses on purpose). A new script's constant joins that test's loop, its `'use strict'` list, and the file lists of `ci/side-panel/bridge_check.mjs`, in the order of `namespaces_script()`; it goes before `js/side_panel.js`, which a test keeps last.

**`sidePanel` is written by hand, and is the exception.** `js/side_panel.js` does not use `define`, its calls are not rows of `router::route`, and its events are not `StripEvent`s. The reasons, one per step above: its functions check their arguments as Chromium's do and fail with a `TypeError` naming the function before anything is sent, and its object is frozen whole, neither of which `define` does (what `define` would have given, the callback style with `runtime.lastError` set, it does not have: a callback gets no argument on failure and the error is logged as unchecked); its answers come from stores in `DelegateState` that a call changes (`SidePanelStore`, `PanelRegistry`), which the router, a pure function of the call and the published strip, cannot hold, so the hub answers them (`Effect::SidePanel`, "Messages Phase 6 adds"); and a panel opening or closing is no difference between two strips, so the events come from the panel registry's notices through `BridgeState::emit`. What it keeps of the recipe: its events are `__nautilusBridge.BridgeEvent`s, so they are announced and dispatched like every other; it is fail-closed (a member Chromium defines and the script does not name is not in the frozen object, or rejects where Chromium does not let go of the property; a method the engine does not know rejects); and it has its own test, `every_side_panel_function_is_ours`, with its part of `ci/side-panel/bridge_check.mjs`. A namespace whose answers are a function of the strip follows the five steps, not this.

## The scripted run (phase5.py)

What the run established, on the macOS bundle built from this tree (2026-11-18).
Each figure is from the kept files under `evidence/phase-5/` (not committed);
the names are the run's `.out` files, and `-logs/` directories hold the logs.

**Review 1 (2026-11-19, `31a7dd7`).** Three passes of `phase5.py` each exit 0
with 72 PASS and no FAIL (`review1-phase5-pass1..3.txt`). The regression modes
each exit 0: `spike` 54 PASS (`review1-run-spike.txt`), `pump` 7, `reload` 7,
`zombie` 21, `stop` 17, `idle` 9, `broken` 14, none failing (`review1-run-*.txt`).
Between the review's first commits and the port fix, the scripted run failed
"every event the engine sent was taken by the worker" in its first pass
(`review1-failing-run-wake-lines.txt`): the relay page's port has a sender tab,
and a check that refused it lost the first wake. The probes that measured the
two senders are `review1-port-probe-failing-run2.txt` and
`review1-relay-sender-measure-run.txt`. The page probe that measured
`chrome.tabs` in a worker-less extension's page and its iframes is
`review1-probe-measure.txt`, with its script under `review1-probe/`.

**The regression modes (B15).** Each earlier mode, on the build with
`ext/phase5.js` imported by the worker and `webNavigation` in the manifest:

| Mode | Before the check change (first build) | Final (committed tree) |
|---|---|---|
| `spike` | 0 (`task13-regress-spike.out.status`) | 0 (`task13-final-spike.out.status`) |
| `pump` | 0 (`task13-regress-pump.out.status`) | 0 (`task13-final-pump.out.status`) |
| `zombie` | 0 (`task13-regress-zombie.out.status`) | 0 (`task13-final-zombie.out.status`) |
| `broken` | 0 (`task13-regress-broken.out.status`) | 0 (`task13-final-broken.out.status`) |
| `reload` | not run before the change | 0 (`task13-final-reload.out.status`) |
| `stop` | 1 (`task13-regress-stop.out.status`), then 1 (`task13-regress-stop2.out.status`, the provocation race below), then 0 (`task13-regress-stop3.out.status`) | 0 (`task13-final-stop.out.status`) |
| `idle` | not run before the change | 0 (`task13-final-idle.out.status`) |

The control: `stop` with the two-line change reverted (no `importScripts`,
no `webNavigation`) passed (`task13-control-noimport-stop.out.status`, 0).
So the failure of the first `stop` came from `phase5.js`, not from the Phase 5
code.

Two things the runs showed, and what was done:

- **A listening worker's stop is not quiet (the plan's emit contract).** The
  test worker now listens for tab and window events. In
  `task13-regress-stop-logs/run-1.log` the stop is at 19:47:46 (line 106); a
  `windows.onFocusChanged` delivery reached the stopped worker at 19:47:50
  (line 117), which started it ("runs a new worker instance", line 149). The
  plan's `emit` starts a stopped worker for a delivery, so this is the contract,
  not a defect. The I15 check of `stop` was asked for "no restart in 90 s" and
  failed. It now says the contract: a restart or an identity request in the
  quiet window must follow a delivery the engine sent (`driver.py`
  `started_by_delivery`, tested first). The stop run on the final tree had such
  a delivery in its window (`INFO deliveries sent in the 90 s quiet window:
  ['windows.onFocusChanged']`). Accepted by the owner on 2026-11-18 for now, to
  be revisited: an idle worker that listens for events is started by each
  delivered event (the stop check's `started_by_delivery` rule).
- **A query before the first strip is published answers an empty list** (as it
  stood on 2026-11-18; since 2026-11-27 a worker's tab call waits for the first
  strip and for the tabs' real ids, two seconds at most: "Id spaces", above). The
  worker's first `tabs.query` came while the first strip was held for the
  table (`task13-regress-stop2-logs/run-1.log` line 69: "(3 waiting for the
  table)", then the query at line 90), and answered `[]` with `ok: true`
  (`task13-regress-stop2-logs/results.jsonl` line 4). The engine answers from
  the published snapshot (the plan's wording), and before the first publish
  that snapshot is empty. The test worker now waits until both tabs are listed
  (`sw.js` `listedTabs`, commit `3fc4ef2`). Accepted by the owner on 2026-11-18
  for now, to be revisited: the engine is not changed, so a query that waits
  for the first publish is not built.

**The scripted run (`ci/side-panel/phase5.py`) and the page's install.** Before
the install was deferred, the run stopped at an extension page opened as a tab:
the page was not bridged, and the browser process trapped. What was measured
(evidence under `evidence/phase-5/`, not committed):

- **The page's `chrome` appears after its context is created.** `pages::install`
  ran in the render process's `on_context_created`, and there `chrome` was
  `undefined`: before the transport, `chrome=undefined` and
  `__nautilusBridgeSend` a function; after the page's script was evaluated,
  `chrome=undefined` and `__bridgeCall` a function. The page's own first script
  saw `chrome` as an object (`task13b-e4b-probe-logs/main.log` lines 292 and
  294; `task13b-e1-probe-logs/main.log` line 298). `core.js` then failed closed
  with `nautilus-bridge: no transport or no chrome in this context (:56)`, so no
  namespace was replaced and the page kept Chromium's own APIs. The order is
  Chromium's: its extension bindings are created after CEF's callback returns.
  The Chromium call site was not identified.
- **The deferred install.** The transport still goes in at context creation. The
  namespaces run once, from a `MutationObserver` on the document that is
  registered at creation and disconnects at its first delivery (`js/page_defer.js`,
  `pages::deferred_namespaces`). The parser delivers the first change before it
  runs the page's first script, and by then `chrome` exists. Measured on the
  bundle with console probes (`task13c-v1-head-logs` to `task13c-v5-inline-logs`):
  the callback runs before a classic script in `<head>` (`task13c-v1-head-logs/main.log`
  line 320, then the script's line 323, `cb=ran chrome=object`), before a script
  that is the first element (`task13c-v2-first-logs/main.log`), before a module
  script (`task13c-v3-module-logs/main.log` line 307), and in a page with no
  script (`task13c-v4-noscript-logs/main.log` line 287). An inline script is
  refused by the extension's content security policy
  (`task13c-v5-inline-logs/main.log` lines 296 and 297), so a page's inline code
  never runs. If the document is absent at creation, one error is logged and
  nothing is installed. If `chrome` is absent at the change, `core.js` logs its
  message once.
- **The trap is Chromium's native `tabs.group`, reached only from an unbridged
  page.** `chrome.tabs.getCurrent()` in an unbridged extension page returns the
  page's Alloy tab id, and `chrome.tabs.group` on that id traps the browser
  process inside Chromium. The last step marker before the trap is "group call"
  (`task13b-e2-steps-logs/main.log`); an unknown id returns `No tab with id`
  (`task13b-e3-group-logs/main.log`). The trap also happens with
  `pages::install` never called (`task13b-e6-noinstall-logs`, exit -5), and does
  not happen for a page with no script (`task13b-e5-plain-logs`), so it is not
  caused by the bridge's code. A bridged page cannot make the call: the engine
  refuses `tabs.group` and `ungroup` for a bridged caller (`bridge/router.rs`).
  Upstream entry C11 in `docs/upstream-issues.md` records it.
- **The page-call holds (commit `ef21a61`).** A page's call for a tab that the
  published strip does not hold yet is held until the strip publishes, or for
  `PUBLISH_WAIT` (2 s, `bridge/state.rs`); a call of an extension the hub does not
  serve yet is held until it is served, or for the same bound, and then refused
  with `the extension is not bridged.`. Held calls are answered in arrival order
  (`BridgeState::page_call`, `Hold`, `settle`). The page lane no longer drops
  such a call (`pages.rs` `receive`), so a page always gets a reply. A worker's
  calls are unchanged.
- **The run after the page-call holds.** On the final tree (commit `ef21a61`; the
  docs commit changes no code): pass 1 exit 1, 66 PASS, 3 FAIL
  (`task13d-final-p5-run1.out`); pass 2 exit 1, 66 PASS, 3 FAIL
  (`task13d-final-p5-run2.out`); the same three lines in both. Before that, on the
  same code's working tree, one pass failed 5 (`task13d-p5-run1.out`, with
  `AC3.3` and `AC3.4`) and another 3 (`task13d-p5-run2.out`). The browser does not
  trap and exits cleanly in all of them, and the page is bridged. The failures that remain:
  - `sender.tab carries the strip's fields` (failed in every pass before the page's
    report). The page's own tab had no Chromium id in the table: the helper lists
    its target as `type other` with no tab id (`task13e-e2-target-lines.txt`), and
    `table::join` takes only `page` targets with a tab id. Fixed by the page's
    report (see "The page's own tab (the report)"); passes after it are below.
  - `restore-list` and `restore-show`: fixed in this cycle (below, "The restored
    worker's port"); both pass in the final runs.
  - `AC3.3` (a created tab still `loading`) and `AC3.4` (two ids above
    `SYNTHETIC_BASE`, 2 000 000 000, so the strip published them without
    Chromium's ids) failed once in the working-tree passes and not in the final
    ones. The strip's publish wait ended before the table named those tabs.
    Not explained.
- **The restored worker's port (fixed, measured).** The restore launch's
  worker starts before the engine has its relay page listening. Its first call
  (the restore suite's `tabs.query({})`, which the suite starts at once) opens the
  worker's own port, and Chromium closes that port with "Could not establish
  connection. Receiving end does not exist.", since nothing listens yet. The
  bridge read the close as the port in use closing: it cleared `port` and
  rejected every pending call with "the browser stopped listening", so the
  suite's first call failed. The relay page was still opening in the same second
  (`task13e-e1-logs/restore.log` lines 37 and 44). The trace is
  `task13e-e1-restore-trace.txt`: the first call is sent on the worker's own
  port `p1`; `disconnect p1 current=true err=Could not establish connection.
  Receiving end does not exist.`; then `adopt p2` with no call sent on `p2`
  before the failure. The earlier reading, "the first call never reaches the
  engine and fails about 30 s later", is not what this trace shows: the failure
  is at the first close.
  The fix (`bridge.js`, `noListener`, `send`, `resend`): a call whose port closed
  with that error, and which no listener received, is sent again on the next
  port every `RESEND_MS` (100 ms) for up to `RECEIVER_WAIT_MS` (3 s) from the
  first such close, then fails with "found no listener in the browser". A call is
  posted on the port in use when it is posted, after that port's hello, so it is
  never posted on a port that was replaced. Measured after the fix:
  `restore-list` and `restore-show` pass in both final passes
  (`task13e-final-p5-run1.out`, `task13e-final-p5-run2.out`). The node check of
  the port (`ci/side-panel/bridge_check.mjs`, the last block) was red first
  (`task13e-port-red.txt`: the first call rejected with "the browser stopped
  listening") and is green (`task13e-port-green.txt`). The harness's "the suite
  finished" check passed on a `failed:` record before this cycle; it does not now
  (`done` in `phase5.py`, `task13e-done-red.txt` and `task13e-done-green.txt`).
  The cause of the worker's early start is not measured. The test extension
  listens for `runtime.onStartup`, which may be why Chromium wakes it at the
  profile's start; that is a guess.
- **`Failed to create API on Chrome object`** appears in the restore launch only:
  six times in the restored extension page's context and twice at the management
  page's first `Init` (`task13e-e1-logs/restore.log` lines 20 to 30); none in
  `task13e-e1-logs/main.log`. The log does not name the API. The bridge's page
  transport is installed in both launches' pages, so the transport alone does not
  explain the difference. Not identified and not the cause of a failure here.
- **The sender's tab (fixed by the page's report).** The join for an extension
  page's tab had no Chromium id to use: the helper's listing gave none
  (`type other`, no tab id). Option (a) was built, and the page reports its own
  id (see "The page's own tab (the report)"). Option (b), the helper listing
  `chrome.tabs.query` by URL, was not built: it needs the helper to hold the
  `tabs` permission and a join by URL that is ambiguous when two tabs show the
  same page. Option (c), leaving the fields off, was the state before this change.
- **A page's `getCurrent` answered `null` once (not in the final passes).** In the
  run `task13e-e2-p5.out` (the same code as the restore fix, with a temporary
  log), `chrome.tabs.getCurrent()` in the page answered nothing, so the page's
  own `tabs.group` call threw and three page checks failed ("the page is bridged
  before its scripts run", "a page's current window", "a page's tabs.group
  rejects too"). The page lane's answer for a view is `null` when the published
  snapshot does not locate the page's tab (`router.rs`, `getCurrent`), and
  `page_call` holds a call only while a strip publish is pending (`state.rs`).
  So a page call that arrives after its tab exists, with no publish pending and
  the snapshot not yet holding the tab, is answered `null`. This is a reading of
  the code and matches the run; it is not reproduced on demand. The same three
  checks failed in two earlier runs too (`task13d-e9-p5.out`, `task13d-diag-p5.out`)
  and passed in the final runs of `task13d` and in both passes of this cycle. In
  the kept logs, three of the full passes since the page-call holds show it
  (`task13d-diag-p5.out`, `task13d-e9-p5.out`, `task13e-e2-p5.out`); the failure
  is intermittent. The report (above) puts the page's id in the table before its
  first call, so the snapshot locates the tab; no `null` came in the six final passes.
- **The regression modes** on the same tree, each judged by its exit status
  (`task13d-final-mode-*.out.status`): `spike` 0 (54 PASS), `pump` 0 (7 PASS), `reload` 0
  (7 PASS), `zombie` 0 (21 PASS), `stop` 0 (17 PASS), `idle` 0 (9 PASS), and
  `broken` 0 (14 PASS), none with a FAIL line.

Passing checks (before the deferred install and after it): every tab listed in strip order
with `windowId`, `index`, `active`, `groupId`; `{active, currentWindow}` is the
window's active tab; `windows.getAll`, `getCurrent`, `getLastFocused`;
`tabs.create` resolves with the tab that exists afterwards; `update({active})`
activates it; `move` puts it first; `scripting.executeScript`,
`tabs.sendMessage`, `tabs.update({url})` with `webNavigation`, and
`chrome.debugger` (`Runtime.evaluate`, `Page.captureScreenshot`,
`Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`) on an id `tabs.query`
returned; `captureVisibleTab` answers a JPEG data URL; `tabs.group` and
`tabs.discard` answer as the protocol states; `tabs.remove` of an unknown id and
`chrome.tabs.get` with a callback reject in Chromium's words; `duplicate` and
`remove`; `remove` closes the tab; the relay page answers `bridge: undefined`
(`bridge: relay <id> answers the relay page: …`, `task13-p5-run2-logs/main.log`);
a web page (`https://example.com/`) has no `__bridgeCall`, `__nautilusBridgeSend`
or `__nautilusBridge` in its main world.

The event deliveries before the stop are each sent once and taken once:
`event tabs.onCreated (delivery 2) sent` and `… taken by the worker`
(`task13-p5-run2-logs/main.log` lines 130 and 135), the same for `tabs.onUpdated`,
`tabs.onActivated`, `tabs.onHighlighted`, `tabs.onMoved`, `tabs.onRemoved`,
`windows.onFocusChanged`. The requests reach the model: `OpenTab` (line 117,
and token 5 `CloseTabs`), `ActivateTab(TabId(3))` (line 159), `MoveTab` (line
173), `CloseTabs([TabId(4)])` (line 259).

**The run after the restored worker's port (2026-11-18).** On the tree with the
port fix and the `done` check (the code commit of this cycle): `phase5.py` pass 1
exit 1, 71 PASS, 1 FAIL (`task13e-final-p5-run1.out`); pass 2 exit 1, 71 PASS,
1 FAIL (`task13e-final-p5-run2.out`). The one FAIL in both is `sender.tab carries
the strip's fields` (above). The restore checks pass in both. The regression
modes, each judged by its exit status (`task13e-final-mode-*.out.status`): `spike`
0 (54 PASS), `pump` 0 (7 PASS), `reload` 0 (7 PASS), `zombie` 0 (21 PASS), `stop`
0 (17 PASS), `idle` 0 (9 PASS), and `broken` 0 (14 PASS), none with a FAIL line.
One pass before the final two (`task13e-e2-p5.out`, with the port fix and a
temporary log of the helper's targets, since reverted) had 68 PASS and 4 FAIL:
the three page checks above and `sender.tab`.

**The run after the page's report (2026-11-18).** The baseline first, on the
tree before the report (`evidence/phase-5/task13f-before-p5-run1.out`, `-run2.out`,
exit 1 both, 71 PASS and 1 FAIL, `sender.tab`): the page's first call
(`getCurrent`, timed in the page with `performance.now`) answered in 1993 ms and
2017 ms, the publish wait. On the tree with the report: six passes of
`phase5.py`, each exit 0 with 72 PASS and 0 FAIL (`task13f-final-p5-run1.out`
to `-run6.out`). The page's first call answered in 154, 1, 8, 3, 1 and 5 ms
(the `INFO` line of each pass). The regression modes, each exit 0:
`spike` (54 PASS), `pump` (7), `reload` (7), `zombie` (21), `stop` (17), `idle`
(9), `broken` (14), none with a FAIL line (`task13f-final-mode-*.out.status`).
The log of a pass shows the report and the row
(`bridge: ...'s page reports TabId(5) is Chromium tab 751615618`, then the table row,
before the strip's first publish of that tab; `task13f-final-run6-logs/main.log`
lines 295 to 307).
No late report (after a synthetic publish) was provoked in these runs; that path
is the existing one in `state.rs` (`table_changed` replaces the id, with
`tabs.onReplaced`).

**Still open.** Whether CEF answers `Target.getTargetInfo` inside the call (B8)
is not determined by these runs. The owner's toolbar click, real popups and
Windows were not run. The helper extension's page is not bridged: not observed,
since the run has no way to evaluate in it.


## Phase 5 handoff (2026-11-18)

Built: `chrome.tabs` and `chrome.windows` over the bridge (`bridge/ids.rs`,
`snapshot.rs`, `pattern.rs`, `shape.rs`, `router.rs`, `differ.rs`, `state.rs`,
`hub.rs`, `pages.rs`, `transport.rs`, `debugger.rs`, `table.rs`; `install.rs`
extended with the marker and the dot-file-free copy; `js/core.js`, `tabs.js`,
`windows.js`, `runtime.js`, `page_transport.js`, `page_defer.js`). Branch taken
for extension pages: B, the process-message lane (`bridge/pages.rs`, the
`nautilus-bridge-call` message). Adaptations: the reconciliation's rows 1 to 4,
6, 7, 9 to 12 carry the changes listed under "Adaptations carried by later
tasks"; rows 5 and 8 held as assumed.

Decisions taken in the build:
- Extension pages are bridged by branch B; the transport goes in at context
  creation and the namespaces at the document's first change (the deferred
  install, `js/page_defer.js`), because Chromium defines `chrome` only after the
  context is created.
- A page's call for a tab the published strip does not hold yet is held until
  the strip publishes or for `PUBLISH_WAIT`; a call of an extension the hub does
  not serve yet is held until it is served or for the same bound, then refused
  with `the extension is not bridged.`
- A worker's call that finds no listener is sent again on a new port every
  100 ms for up to 3 s from the first close, then fails with `found no listener
  in the browser` (`bridge.js` `resend`).
- An extension page shown as a tab reports its own Chromium tab id, once, from
  its first document; the browser accepts it only from that page's main frame
  and only for its own tab view, and a page's calls leave after the report or a
  1 s bound (`js/page_report.js`, `pages::report_source`, `TabTable::report`).
  The owner chose this over the helper listing and over leaving the gap open.
- Owner's acceptance of 2026-11-18, for now, to be revisited: a delivered tab or
  window event starts a stopped worker that listens for it; `tabs.query` from a
  worker before the first strip publish answers an empty list.

Verified by unit tests: the id spaces, the router's method table, the differ's
order, the publish hold, `tabs.create` resolving after the publish, and the
marker and copy in `install.rs`. The last recorded gate (the page's report, 2026-11-18): `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
(227 and 2 passing), `cargo build --workspace --all-targets`,
`node ci/side-panel/bridge_check.mjs` and `cargo xtask bundle-mac`, all exit 0
(`evidence/phase-5/task13f-gate-*`).all exit 0.

Verified in the running browser (`ci/side-panel/phase5.py`, macOS, 2026-11-18,
an ordinary bundle, the test extension loaded through `NAUTILUS_TEST_CONTROL`):
six passes on the final tree, each exit 0 with 72 PASS and 0 FAIL
(`evidence/phase-5/task13f-final-p5-run1.out` to `-run6.out`). The regression
modes, each judged by its exit status, all exit 0: `spike` (54 PASS), `pump` (7),
`reload` (7), `zombie` (21), `stop` (17), `idle` (9), `broken` (14), none with a
FAIL line (`task13f-final-mode-*.out.status`). The earlier passes, before the
report, are in `task13e-*` (71 PASS and 1 FAIL, `sender.tab`).
- Window counts over the run: not recorded by the script's output.
- "Chromium asked for a default client" count: 0.
- Whether Chromium's bindings were on `chrome` when the page bridge went in
  (the `native` flag): not recorded by the output.
- `captureVisibleTab` answers a JPEG data URL (PASS); its refusal text as a real
  extension sees it: not exercised.

Not verified:
- An action popup's calls: needs a click on the toolbar button, which cannot be
  scripted on this Mac. Open: pin the test extension's action if it has a popup,
  open it, and check that `chrome.tabs.query({active: true, currentWindow: true})`
  in the popup's console returns the tab under it.
- Events for changes the user makes by hand (closing a tab with the mouse,
  dragging a tab, a second window): the differ is unit-tested and the scripted run
  covers the same events caused through the API.
- Manifest V2 extensions (callback-only contexts, `tabs.executeScript`).
- Real clicks on the toolbar; anything on Windows (nothing was run there
  locally; CI builds and tests on Windows).
- `sender.tab` for an extension page shown as a tab carried the strip's fields
  only after the page's report (see "The page's own tab (the report)"); it passes
  in all six final passes. Before the report it lacked them (`windowId` and
  `index` were -1).
- Intermittent, not explained: the page's own `getCurrent` answering `null` in
  some earlier runs (three page checks; no `null` in the six final passes), `AC3.3`
  and `AC3.4` failing once in two earlier passes, and `Failed to create API on
  Chrome object` in the restore launch (not identified).
- Whether CEF answers `Target.getTargetInfo` inside the probe's call (the basis
  of the reservation in the reconciliation's B8). It no longer matters: since
  2026-11-19 the probe sends under its own id and there is no reservation.

For the next phase: the protocol and the method tables above are the contract;
a namespace is added as "How a later phase adds a namespace" describes. The
`sender.tab` decision was taken by the owner: option (a), the page's report
(`Messages Phase 5 adds`). The release (a merge to `main`) needs the
owner's yes; the draft pull requests are osmena #2 and nautilus #1.

## Measured in review cycle 2 (Phase 5)

- **Content scripts run.** On the bundle, a manifest `content_scripts` entry for
  `http://127.0.0.1:8765/*` ran on the matching page and its `fetch` reached a local
  server (`evidence/phase-5/review2-n6-*`, the probe in the session scratchpad). The
  first attempt in the scripted run used an `https` page and a `fetch` to an `http`
  address; that is the likely reason it never reported, not verified.
- **Messages to a silent listener.** From an isolated-world sender on a tab (the
  `content-silent` step of `phase5.py`), the wrapped worker listener settles in about
  1 ms with an undefined answer (`evidence/phase-5/review2-n3-after-phase5-run1.txt`);
  before the wrapper was synchronous it held the channel for the whole 5 s
  (`review2-n3-before-phase5-run2.txt`). The unwrapped baseline could not be measured:
  the only listener outside the bridge is the helper extension's, which has none, so the
  expectation is this measured outcome, not a measured Chromium one.
- **The sender's strip from a page's first message.** Before the worker's cache
  followed the tabs, an extension page's first message reached the worker with
  `index: -1` and `windowId: -1` in each of five passes
  (`evidence/phase-5/review2-final-phase5-run1.txt` to `-run5.txt`); after it, the
  check passed in three of three (`review2-n7-phase5-run1.txt` to `-run3.txt`).
- **Ids.** Chromium's tab ids are session ids that can lie anywhere in the 32-bit range,
  above the synthetic base (`2 000 000 000`). A page's report of its id is accepted for
  any positive 32-bit value; a value a live tab already holds, synthetic ids included,
  is refused by `TabTable::report`.
