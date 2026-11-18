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
in the worker only: extension pages have no bridge in protocol 1 (see the
limits below).

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
- **New instance:** the tab lists the old instance made and has not had
  answered are dropped without an answer, since their call ids belong to
  that instance. A debugger report the old instance made and has not had
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
activity. The helper lists an extension's service worker with type `worker`
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
instance is dropped, or applied and not answered (`job_kept`, `reply_due`),
because its call ids belong to a worker that is gone. In the zombie runs no
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
`index`, `windowId`, `active` and `highlighted` over it. (Task 9 removed
that fetch from the bridge: `tabs.query` is now sent by name through
`core.js`, and a forward's `strip` is laid over Chromium's objects there,
by `patchTabs`.) Any other
method is answered `{error: "<method> is not supported by Nautilus."}`:
the bridge fails closed. A `tabs.query` is answered when the helper's
list of targets arrives, so a query made while the helper's page is
still loading waits for it. When the helper's page has been given up, a
query is answered with an error at once.

## Events (engine to bridge)

| `event` | Carries | The bridge does | Since |
|---------|---------|-----------------|-------|
| `action.onClicked` | `tabId` (Chromium's) | `chrome.action.onClicked.dispatch(await <Chromium's chrome.tabs.get>(tabId))` | 1 |

Since Task 9, any other event is handed to every listener that the
namespace scripts registered with `__bridgeOnEvent`, as
`(event, args)`, in the order they were registered. The receipt is posted
before the listeners run. A listener that throws is logged, and the others
still run. The delivery is then acknowledged after all of them ran: `done`
with `ok: true`, or, when any listener threw, `done` with `ok: false` and
the first error's message. The relay reports that as `received` with a
failure, which the engine never retries (`policy::after_delivery`). Chromium's
`tabs.get` for the click is the one `core.js` kept in `native` before it
replaced the namespace, so a click never reaches the engine.

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
- the helper's queued tab lists are answered with an error, and clicks
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

- Whether CEF answers `Target.getTargetInfo` inside the `execute_dev_tools_method` call (the basis of the reservation in B8).
- Whether `ext/phase5.js` is inert when no suite is listening (B15).
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

Calls added to the protocol: the namespaces `tabs` and `windows` with
the methods of the method tables (in the design: `tabs.query`,
`tabs.get`, `tabs.create`, `tabs.update`, `tabs.remove`, and the rest
of the `chrome.tabs` and `chrome.windows` surface the bridge answers,
forwards, or refuses), `tabs.stripOf(id)` (the seven strip fields or
`null`, used for `sender.tab`), and `bridge.listen(eventName)` (sent by
the script on the first `addListener` of an event, so the engine fires
only events someone listens to).

For branch B (Task 12), extension pages have their own lane: a process
message `nautilus-bridge-call` with one string argument
`{"id", "ns", "method", "args"}`, answered by evaluating
`self.__nautilusBridgeDeliver("<json>")` in the frame with `{"id", "ok"}`,
`{"id", "error"}` or `{"event", "args"}`.

