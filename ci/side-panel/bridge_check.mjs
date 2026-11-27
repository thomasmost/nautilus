// A check of the bridge scripts against a stand-in for Chromium's chrome
// object. CI runs it in the check job; on a development machine with Node:
//   node ci/side-panel/bridge_check.mjs
// It proves what the Rust tests cannot: that the scripts parse, that a
// replaced namespace has no function of Chromium's left in it, both
// calling styles, runtime.lastError, forwards, and events.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const dir = fileURLToPath(new URL('../../crates/nautilus-engine/src/bridge/js/', import.meta.url));
const read = (name) => readFileSync(dir + name, 'utf8');

function nativeEvent() {
  const fns = [];
  return {
    addListener: (fn) => fns.push(fn),
    removeListener: (fn) => fns.splice(fns.indexOf(fn), 1),
    hasListener: (fn) => fns.includes(fn),
    fire: (...args) => fns.map((fn) => fn(...args)),
    count: () => fns.length,
  };
}

function world({ viaPage, prime = null, sidePanel = true, pinned = false }) {
  const reached = []; // Chromium functions that were really called
  const calls = []; // what the engine was asked
  const answers = new Map(); // "ns.method" -> (args) => value | throws
  let handler = null;
  const messages = nativeEvent(); // Chromium's runtime.onMessage
  const runtime = {
    onMessage: messages,
    id: 'x',
    getContexts: () => { reached.push('runtime.getContexts'); return Promise.resolve([{ contextType: 'BACKGROUND', tabId: -1 }]); },
  };
  const chromium = (name, result) => (...args) => {
    reached.push(name);
    const callback = args.pop();
    callback(result(args));
  };
  const chrome = {
    runtime,
    tabs: {
      TAB_ID_NONE: -1,
      query: chromium('tabs.query', () => []),
      get: chromium('tabs.get', ([id]) => ({ id, index: -1, windowId: -1, active: false, url: 'https://x.example/' })),
      update: chromium('tabs.update', ([id]) => ({ id, index: -1, windowId: -1 })),
      group: chromium('tabs.group', () => 1),
      discard: chromium('tabs.discard', () => undefined),
      create: chromium('tabs.create', () => ({})),
      createSplit: chromium('tabs.createSplit', () => 1),
      sendMessage: (...args) => { reached.push('tabs.sendMessage'); return Promise.resolve(args); },
      connect: () => { reached.push('tabs.connect'); return { name: 'port' }; },
      onCreated: nativeEvent(),
      onZoomChange: nativeEvent(),
      onSelectionChanged: nativeEvent(),
    },
    windows: {
      WINDOW_ID_CURRENT: -2,
      getAll: chromium('windows.getAll', () => []),
      create: chromium('windows.create', () => ({})),
      onFocusChanged: nativeEvent(),
    },
    // Chromium's own sidePanel, none of which may be reached, with one
    // function the bridge does not name.
    sidePanel: {
      open: chromium('sidePanel.open', () => undefined),
      getOptions: chromium('sidePanel.getOptions', () => ({})),
      frobnicate: chromium('sidePanel.frobnicate', () => undefined),
      onOpened: nativeEvent(),
    },
  };
  if (!sidePanel) { delete chrome.sidePanel; }
  if (pinned) {
    // A namespace property Chromium does not let go of.
    Object.defineProperty(chrome, 'sidePanel', { value: chrome.sidePanel, configurable: false, writable: false, enumerable: true });
  }
  const engine = (ns, method, args) => {
    // Through JSON, as on the wire (and so the arrays are this realm's).
    calls.push(JSON.parse(JSON.stringify([ns, method, args])));
    const answer = answers.get(ns + '.' + method);
    try { return Promise.resolve(answer ? answer(args) : null); } catch (e) { return Promise.reject(e); }
  };
  const sandbox = { chrome, console: { error: (...a) => sandbox.errors.push(a.join(' ')), log() {} }, errors: [], Promise, Map, JSON, Object, Array, Error, String, setTimeout, clearTimeout };
  sandbox.self = sandbox;
  vm.createContext(sandbox);
  if (prime) { answers.set('tabs.query', () => prime); }
  if (viaPage) {
    // The page lane: the native function and the delivery function.
    sandbox.__nautilusBridgeSend = (text) => {
      const m = JSON.parse(text);
      engine(m.ns, m.method, m.args).then(
        (ok) => sandbox.__nautilusBridgeDeliver(JSON.stringify({ id: m.id, ok })),
        (e) => sandbox.__nautilusBridgeDeliver(JSON.stringify({ id: m.id, error: e.message })),
      );
    };
    vm.runInContext(read('page_transport.js'), sandbox);
    handler = (name, args) => sandbox.__nautilusBridgeDeliver(JSON.stringify({ event: name, args }));
  } else {
    sandbox.__bridgeCall = engine;
    sandbox.__bridgeOnEvent = (h) => { handler = h; };
    // The worker's marker (bridge.js): the worker primes its tab cache.
    sandbox.__nautilusTransport = { protocol: 1, portHidden: true };
  }
  for (const file of ['core.js', 'tabs.js', 'windows.js', 'runtime.js', 'side_panel.js']) {
    vm.runInContext(read(file), sandbox);
  }
  return { chrome, reached, calls, answers, messages, errors: sandbox.errors, emit: (name, args) => handler(name, args), sandbox };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

for (const viaPage of [false, true]) {
  const w = world({ viaPage });
  const { chrome } = w;

  // Nothing of Chromium's is left in a replaced namespace but the two
  // functions that are meant to be.
  w.answers.set('tabs.query', () => [{ id: 7, index: 0 }]);
  assert.deepEqual(await chrome.tabs.query({}), [{ id: 7, index: 0 }]);
  await assert.rejects(chrome.tabs.createSplit([1, 2]), /tabs\.createSplit is not supported\./);
  w.answers.set('tabs.group', () => { throw new Error('tabs.group is not supported.'); });
  await assert.rejects(chrome.tabs.group({ tabIds: [7] }), /tabs\.group is not supported\./);
  assert.equal(await chrome.tabs.discard(7), undefined);
  await chrome.tabs.create({ url: 'https://x.example/' });
  await chrome.windows.create({});
  await chrome.windows.getAll();
  assert.deepEqual(w.reached, [], 'a bridged call reached Chromium');
  assert.equal(chrome.tabs.connect(7).name, 'port');
  await chrome.tabs.sendMessage(7, 'hi');
  assert.deepEqual(w.reached, ['tabs.connect', 'tabs.sendMessage']);
  assert.equal(chrome.tabs.TAB_ID_NONE, -1);
  assert.equal(chrome.windows.WINDOW_ID_CURRENT, -2);
  assert.equal(typeof chrome.tabs.executeScript, 'undefined');
  // Chromium cannot put its own namespace back.
  assert.throws(() => Object.defineProperty(chrome, 'tabs', { value: {} }));

  // Arguments: the callback is taken off, trailing undefineds dropped.
  w.calls.length = 0;
  await chrome.tabs.reload(undefined, undefined);
  await chrome.tabs.update(7, { active: true });
  assert.deepEqual(w.calls, [['tabs', 'reload', []], ['tabs', 'update', [7, { active: true }]]]);

  // A forward calls Chromium's function and lays the strip over it.
  w.reached.length = 0;
  w.answers.set('tabs.get', () => ({ __forward: { method: 'get', args: [7], strip: { 7: { index: 3, windowId: 1, active: true } } } }));
  assert.deepEqual(await chrome.tabs.get(7), { id: 7, index: 3, windowId: 1, active: true, url: 'https://x.example/' });
  assert.deepEqual(w.reached, ['tabs.get']);

  // A forward names a method the engine never forwards: it is refused, and
  // Chromium's function of that name is not called.
  w.reached.length = 0;
  w.answers.set('tabs.get', () => ({ __forward: { method: 'group', args: [7], strip: {} } }));
  await assert.rejects(chrome.tabs.get(7), /tabs\.group is not supported\./);
  assert.deepEqual(w.reached, [], 'a forward the engine never makes reached Chromium');

  // The callback style, with runtime.lastError on failure.
  const got = await new Promise((resolve) => chrome.tabs.query({}, resolve));
  assert.deepEqual(got, [{ id: 7, index: 0 }]);
  w.answers.set('tabs.remove', () => { throw new Error('No tab with id: 9.'); });
  const seen = await new Promise((resolve) => chrome.tabs.remove(9, () => resolve(chrome.runtime.lastError && chrome.runtime.lastError.message)));
  assert.equal(seen, 'No tab with id: 9.');
  assert.equal('lastError' in chrome.runtime, false);
  await new Promise((resolve) => chrome.tabs.remove(9, resolve));
  assert.ok(w.errors.some((e) => e.includes('Unchecked runtime.lastError: No tab with id: 9.')));
  await assert.rejects(chrome.tabs.remove(9), /No tab with id: 9\./);

  // Events: announced once, delivered to listeners, filters honoured.
  w.calls.length = 0;
  const created = [];
  const listener = (tab) => created.push(tab.id);
  chrome.tabs.onCreated.addListener(listener);
  chrome.tabs.onCreated.addListener(listener);
  chrome.tabs.onCreated.addListener((tab) => created.push('second ' + tab.id));
  await tick();
  assert.deepEqual(w.calls, [['bridge', 'listen', ['tabs.onCreated']]]);
  w.emit('tabs.onCreated', [{ id: 11 }]);
  w.emit('tabs.onRemoved', [11, {}]);
  assert.deepEqual(created, [11, 'second 11']);
  assert.ok(chrome.tabs.onCreated.hasListener(listener));
  chrome.tabs.onCreated.removeListener(listener);
  assert.equal(chrome.tabs.onCreated.hasListener(listener), false);
  const focus = [];
  chrome.windows.onFocusChanged.addListener((id) => focus.push(id));
  chrome.windows.onFocusChanged.addListener((id) => focus.push('never ' + id), { windowTypes: ['popup'] });
  w.emit('windows.onFocusChanged', [-1]);
  assert.deepEqual(focus, [-1]);
  // Chromium's own events the bridge does not fire exist and stay quiet.
  chrome.tabs.onSelectionChanged.addListener(() => assert.fail('never fires'));
  assert.equal(typeof chrome.tabs.onZoomChange.fire, 'function');

  // chrome.sidePanel is replaced whole: its members are the engine's and
  // nothing of Chromium's is left in it.
  const sp = chrome.sidePanel;
  assert.ok(Object.isFrozen(sp));
  assert.deepEqual(
    Object.keys(sp).sort(),
    ['Side', 'close', 'getLayout', 'getOptions', 'getPanelBehavior', 'onClosed', 'onOpened', 'open', 'setOptions', 'setPanelBehavior']
  );
  for (const name of ['open', 'close', 'getOptions', 'setOptions', 'getPanelBehavior', 'setPanelBehavior', 'getLayout']) {
    assert.equal(typeof sp[name], 'function', name);
  }
  assert.equal(sp.Side.RIGHT, 'right');
  // Chromium cannot put its own back. When the set of extensions changes
  // it defines every API on `chrome` again, in every live context
  // (NativeExtensionBindingsSystem::UpdateBindingsForContext, a lazy data
  // property per API): a replaced namespace must refuse that, or the
  // extension is left with Chromium's own from then on.
  for (const name of ['sidePanel', 'tabs', 'windows']) {
    const ours = chrome[name];
    assert.throws(
      () => Object.defineProperty(chrome, name, { value: { chromium: true }, writable: true, enumerable: true, configurable: true }),
      TypeError,
      `Chromium could put its own chrome.${name} back`
    );
    assert.equal(chrome[name], ours, name);
  }
  // Every function asks the engine, and none reaches Chromium.
  w.calls.length = 0;
  w.reached.length = 0;
  w.answers.set('sidePanel.getOptions', () => ({ enabled: true, path: 'panel.html' }));
  w.answers.set('sidePanel.getLayout', () => ({ side: 'right' }));
  assert.equal((await sp.getOptions({ tabId: 7 })).path, 'panel.html');
  assert.equal(await sp.setOptions({ path: 'p.html', enabled: true, tabId: undefined }), undefined);
  assert.equal(await sp.setPanelBehavior({ openPanelOnActionClick: true }), undefined);
  await sp.getPanelBehavior();
  assert.equal((await sp.getLayout()).side, 'right');
  assert.equal(await sp.open({ windowId: 1 }), undefined);
  assert.equal(await sp.close({ tabId: 7 }), undefined);
  assert.deepEqual(w.calls, [
    ['sidePanel', 'getOptions', [{ tabId: 7 }]],
    ['sidePanel', 'setOptions', [{ path: 'p.html', enabled: true }]],
    ['sidePanel', 'setPanelBehavior', [{ openPanelOnActionClick: true }]],
    ['sidePanel', 'getPanelBehavior', []],
    ['sidePanel', 'getLayout', []],
    ['sidePanel', 'open', [{ windowId: 1 }]],
    ['sidePanel', 'close', [{ tabId: 7 }]],
  ]);
  assert.deepEqual(w.reached, [], 'a sidePanel call reached Chromium');
  // The engine's rejection is the promise's; a bad argument never leaves.
  w.answers.set('sidePanel.open', () => { throw new Error('No active side panel for tabId: 7'); });
  await assert.rejects(sp.open({ tabId: 7 }), /No active side panel for tabId: 7/);
  w.calls.length = 0;
  await assert.rejects(sp.open({ bogus: 1 }), /Error in invocation of sidePanel\.open: Unexpected property: 'bogus'\./);
  await assert.rejects(sp.open(), /sidePanel\.open: No matching signature\./);
  await assert.rejects(sp.setOptions({ enabled: 'yes' }), /Error at property 'enabled': Invalid type\./);
  assert.deepEqual(w.calls, []);
  // The callback style.
  w.answers.set('sidePanel.open', () => null);
  assert.equal(await new Promise((resolve) => sp.getLayout((layout) => resolve(layout.side))), 'right');
  // The two events are the core's: the first listener is announced, the
  // core's one handler dispatches them, and the tabs' events still arrive.
  w.calls.length = 0;
  const panels = [];
  sp.onOpened.addListener((info) => panels.push('opened ' + info.path));
  sp.onOpened.addListener((info) => panels.push('again ' + info.path));
  await tick();
  assert.deepEqual(w.calls, [['bridge', 'listen', ['sidePanel.onOpened']]]);
  w.emit('sidePanel.onOpened', [{ windowId: 1, path: 'panel.html' }]);
  w.emit('sidePanel.onClosed', [{ windowId: 1, path: 'panel.html' }]);
  assert.deepEqual(panels, ['opened panel.html', 'again panel.html']);
  sp.onClosed.addListener((info) => panels.push('closed ' + info.path));
  await tick();
  assert.deepEqual(w.calls[1], ['bridge', 'listen', ['sidePanel.onClosed']]);
  w.emit('sidePanel.onClosed', [{ windowId: 1, path: 'panel.html' }]);
  assert.equal(panels[2], 'closed panel.html');
  w.emit('tabs.onCreated', [{ id: 12 }]);
  assert.deepEqual(created.slice(-1), ['second 12'], 'the side panel script took the event handler');

  // runtime.getContexts: Chromium is asked for every context, and the
  // engine gets the filter with Chromium's answer.
  w.calls.length = 0;
  w.reached.length = 0;
  w.answers.set('runtime.getContexts', ([filter, originals]) => originals.concat([{ contextType: 'SIDE_PANEL', windowId: 1 }]));
  const contexts = await chrome.runtime.getContexts({ contextTypes: ['SIDE_PANEL'] });
  assert.deepEqual(contexts.map((c) => c.contextType), ['BACKGROUND', 'SIDE_PANEL']);
  assert.deepEqual(w.reached, ['runtime.getContexts']);
  assert.deepEqual(w.calls, [['runtime', 'getContexts', [{ contextTypes: ['SIDE_PANEL'] }, [{ contextType: 'BACKGROUND', tabId: -1 }]]]]);
  assert.throws(() => chrome.runtime.getContexts('all'), /runtime\.getContexts: No matching signature\./);
  w.reached.length = 0;

  // An extension without the "sidePanel" permission has no chrome.sidePanel.
  assert.equal(world({ viaPage, sidePanel: false }).chrome.sidePanel, undefined);
  // Where Chromium does not let go of the property, its own object is
  // filled with the engine's members, and what the bridge does not name rejects.
  {
    const p = world({ viaPage, pinned: true });
    p.answers.set('sidePanel.getLayout', () => ({ side: 'right' }));
    assert.equal((await p.chrome.sidePanel.getLayout()).side, 'right');
    await p.chrome.sidePanel.open({ windowId: 1 });
    await assert.rejects(p.chrome.sidePanel.frobnicate(), /sidePanel\.frobnicate is not supported\./);
    assert.deepEqual(p.reached, [], 'a pinned sidePanel reached Chromium');
    assert.throws(() => { 'use strict'; p.chrome.sidePanel.open = () => {}; });
  }

  // runtime.onMessage: a message from a tab is delivered at once, synchronously.
  // The strip's fields come from this context's cache: a message from a tab the
  // cache does not hold carries the tab as Chromium gave it, and asks the engine
  // for the tab's strip; the next message from that tab carries the strip's fields.
  w.answers.set('tabs.stripOf', ([id]) => (id === 7 ? { index: 3, windowId: 1, groupId: -1, active: true } : null));
  const senders = [];
  chrome.runtime.onMessage.addListener((message, sender, respond) => { senders.push(sender.tab && sender.tab.windowId); if (message === 'ask') { respond('answer'); } });
  assert.equal(w.messages.count(), 1);
  const responses = [];
  // A listener that answers at once does not keep the channel open.
  assert.deepEqual(w.messages.fire('ask', { tab: { id: 7, windowId: -1 } }, (v) => responses.push(v)), [false]);
  assert.deepEqual(senders, [-1], 'a tab the cache does not hold is given as Chromium gave it');
  assert.deepEqual(responses, ['answer']);
  await tick(); await tick();
  assert.deepEqual(w.messages.fire('ask', { tab: { id: 7, windowId: -1 } }, (v) => responses.push(v)), [false]);
  assert.deepEqual(senders, [-1, 1], 'the next message from the tab carries its strip');
  assert.deepEqual(responses, ['answer', 'answer']);
  // Nobody here answers: the channel is not held (it closes as Chromium closes
  // it), so this context cannot send the sender an undefined that beats an
  // answer from another context (a popup, an offscreen document) that comes later.
  assert.deepEqual(w.messages.fire('other', { tab: { id: 8, windowId: -1 } }, (v) => responses.push(['closed', v])), [false], 'a silent listener held the channel open');
  await tick(); await tick();
  assert.deepEqual(responses, ['answer', 'answer'], 'a silent listener answered the sender');
  // No tab: straight through, synchronously.
  assert.deepEqual(w.messages.fire('ask', {}, (v) => responses.push(v)), [false]);
  assert.equal(responses[2], 'answer');
  // A worker's cache is primed from the tabs it lists: a tab it lists carries
  // its strip from its first message.
  const primed = world({ viaPage: false, prime: [{ id: 9, index: 2, windowId: 4, groupId: -1, active: false, highlighted: false, selected: false, pinned: false }] });
  const primedSenders = [];
  primed.chrome.runtime.onMessage.addListener((message, sender) => { primedSenders.push([sender.tab.index, sender.tab.windowId]); });
  await tick(); await tick();
  primed.messages.fire('hello', { tab: { id: 9, windowId: -1, index: -1 } }, () => {});
  assert.deepEqual(primedSenders, [[2, 4]], 'a primed tab was not given its strip');
  // A tab opened after the prime is learnt from the engine's tabs.onCreated,
  // so its first message carries its strip too; onReplaced moves the entry to
  // the tab's new id, and onRemoved lets it go. The worker listens for these
  // itself, unseen by the extension's own listeners.
  assert.equal(primed.chrome.tabs.onCreated.hasListeners(), false, 'the cache\'s listener is visible to the extension');
  assert.ok(primed.calls.some(([ns, method, args]) => ns === 'bridge' && method === 'listen' && args[0] === 'tabs.onCreated'), 'the worker does not ask for tabs.onCreated');
  primed.emit('tabs.onCreated', [{ id: 11, index: 5, windowId: 4, groupId: -1, active: true, highlighted: true, selected: true, pinned: false }]);
  primed.messages.fire('hello', { tab: { id: 11, windowId: -1, index: -1 } }, () => {});
  assert.deepEqual(primedSenders.at(-1), [5, 4], 'a tab created after the prime was not given its strip on its first message');
  primed.emit('tabs.onCreated', [{ id: 12, index: 6, windowId: 4 }]);
  primed.emit('tabs.onReplaced', [2000000012, 12]);
  primed.messages.fire('hello', { tab: { id: 2000000012, windowId: -1, index: -1 } }, () => {});
  assert.deepEqual(primedSenders.at(-1), [6, 4], 'a replaced tab lost its strip');
  primed.emit('tabs.onRemoved', [11, { windowId: 4, isWindowClosing: false }]);
  primed.messages.fire('hello', { tab: { id: 11, windowId: -1, index: -1 } }, () => {});
  assert.deepEqual(primedSenders.at(-1), [-1, -1], 'a removed tab kept its strip');
  // A page does not listen for them (its calls wait for its own report).
  const paged = world({ viaPage: true });
  paged.chrome.runtime.onMessage.addListener(() => {});
  await tick();
  assert.ok(!paged.calls.some(([ns, method]) => ns === 'bridge' && method === 'listen'), 'a page asked for tab events for its cache');
  // An asynchronous listener keeps the channel.
  chrome.runtime.onMessage.addListener((message) => (message === 'later' ? Promise.resolve('promised') : undefined));
  assert.deepEqual(w.messages.fire('later', {}, (v) => responses.push(v)), [true]);
  await tick();
  assert.equal(responses.at(-1), 'promised');
}
// An extension page's context, as pages.rs composes it: the transport when
// the context is created, the namespaces (page_defer.js around them) at the
// first change of the document. Chromium's `chrome` appears between the two,
// as the render process measures it, so it is set at the change. The
// observer is a stand-in that delivers only while it is connected.
function pageContext({ bindingsAtChange = true, hasDocument = true, answers = true }) {
  const observers = [];
  const errors = [];
  const sent = [];
  const reports = []; // the tab ids the page reported
  const events = []; // 'report' and 'call', in the order they reached the browser
  const reached = []; // Chromium's own functions that were really called
  const chrome = {
    runtime: {
      id: 'abcdefghijklmnopabcdefghijklmnop',
      onConnect: nativeEvent(),
      onMessage: nativeEvent(),
      sendMessage() {},
      connect() { return { name: '', onMessage: nativeEvent(), onDisconnect: nativeEvent(), postMessage() {} }; },
    },
    tabs: {
      TAB_ID_NONE: -1,
      query(...args) {
        reached.push('tabs.query');
        const callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
        if (callback) callback([]);
        return Promise.resolve([]);
      },
      sendMessage() {},
      connect() {},
      onCreated: nativeEvent(),
    },
    windows: { WINDOW_ID_CURRENT: -2 },
  };
  const sandbox = {
    console: { error: (...a) => errors.push(a.map(String).join(' ')), log() {} },
    Promise, Map, Set, JSON, Object, Array, Error, String, Math, Date, setTimeout, clearTimeout,
    // The browser answers each call (an empty answer) unless `answers` is false,
    // so a call nobody waits for is never refused by its bound.
    __nautilusBridgeSend: (text) => {
      const message = JSON.parse(text);
      sent.push(message);
      events.push('call');
      if (answers) { setTimeout(() => sandbox.__nautilusBridgeDeliver(JSON.stringify({ id: message.id, ok: [] })), 0); }
    },
    __nautilusBridgeReport: (text) => { reports.push(JSON.parse(text)); events.push('report'); },
  };
  sandbox.self = sandbox;
  if (hasDocument) sandbox.document = {};
  sandbox.MutationObserver = class {
    constructor(callback) { this.callback = callback; this.connected = true; observers.push(this); }
    observe(target, options) { this.target = target; this.options = options; }
    disconnect() { this.connected = false; }
  };
  vm.createContext(sandbox);
  vm.runInContext(read('page_transport.js'), sandbox);
  const namespaces = ['core.js', 'tabs.js', 'windows.js', 'runtime.js', 'side_panel.js', 'page_report.js'].map(read).join('\n');
  vm.runInContext(
    `(function (root, install) {\n${read('page_defer.js')}\n})(self, function () {\ntry {\n${namespaces}\n} catch (e) { console.error('nautilus-bridge: ' + e); }\n});`,
    sandbox,
  );
  const mutate = () => {
    if (bindingsAtChange) sandbox.chrome = chrome;
    for (const observer of observers) {
      if (observer.connected) observer.callback([{ type: 'childList' }]);
    }
  };
  return { sandbox, observers, errors, sent, reports, events, reached, chrome, mutate };
}

{
  // Before the first change: no namespace is replaced and nothing is asked.
  const p = pageContext({});
  assert.equal(p.sandbox.__nautilusBridge, undefined);
  assert.equal(p.observers.length, 1);
  assert.equal(p.observers[0].target, p.sandbox.document);
  // Copied out of the sandbox's realm first, so the prototypes compare equal.
  assert.deepEqual(JSON.parse(JSON.stringify(p.observers[0].options)), { childList: true, subtree: true });
  assert.deepEqual(p.errors, []);
  // The first change: the namespaces are replaced, and the page's own
  // calls go to the engine, not to Chromium.
  p.mutate();
  assert.equal(p.observers[0].connected, false);
  assert.ok(p.sandbox.__nautilusBridge, 'the namespaces are installed at the first change');
  assert.deepEqual(p.errors, []);
  p.sandbox.chrome.tabs.query({});
  assert.deepEqual(p.reached, []);
  assert.deepEqual(p.sent.map((m) => [m.ns, m.method]), [['tabs', 'query']]);
  // A later change installs nothing again: the observer is gone, and a
  // second install would have failed on the replaced namespaces.
  p.mutate();
  assert.deepEqual(p.errors, []);
  assert.equal(p.sent.length, 1);
}
{
  // No chrome at the first change: the existing message, once, and no
  // namespace replaced (the page's calls stay refused, not native).
  const p = pageContext({ bindingsAtChange: false });
  p.mutate();
  p.mutate();
  assert.deepEqual(p.errors, ['nautilus-bridge: no transport or no chrome in this context']);
  assert.equal(p.sandbox.__nautilusBridge, undefined);
}
{
  // No document at creation: an error, and nothing waits for it.
  const p = pageContext({ hasDocument: false });
  assert.deepEqual(p.errors, ['nautilus-bridge: no document to wait for in this context']);
  assert.equal(p.observers.length, 0);
  p.mutate();
  assert.equal(p.sandbox.__nautilusBridge, undefined);
}
// The worker's runtime port (bridge.js) against a model of Chromium's ports: a
// port whose other end has no listener is closed at once with "Receiving end
// does not exist" (nothing on that end ever sees a message), and a port to a
// listening page delivers both ways. The worker's own first port is opened
// when no page listens yet, as a worker started before the relay page does.
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function portEnd(name) {
  const end = {
    name,
    onMessage: nativeEvent(),
    onDisconnect: nativeEvent(),
    peer: null,
    closed: false,
    postMessage(message) {
      if (end.closed) { throw new Error('Attempting to use a disconnected port object'); }
      const copy = JSON.parse(JSON.stringify(message));
      setTimeout(() => end.peer.receive(copy), 0);
    },
    receive(message) { if (!end.closed) { end.onMessage.fire(message); } },
    disconnect() {
      if (end.closed) { return; }
      end.closed = true;
      end.onDisconnect.fire();
      if (end.peer && !end.peer.closed) { end.peer.disconnect(); }
    },
  };
  return end;
}

function workerWorld() {
  const pages = []; // the onConnect listeners of the extension's pages
  const served = []; // the methods each page was asked
  let lastError;
  const runtime = {
    id: 'x',
    getURL: (path) => `chrome-extension://x/${path}`,
    onConnect: nativeEvent(),
    get lastError() { return lastError; },
    connect({ name }) {
      const worker = portEnd(name);
      const page = portEnd(name);
      worker.peer = page;
      page.peer = worker;
      if (pages.length === 0) {
        setTimeout(() => {
          lastError = { message: 'Could not establish connection. Receiving end does not exist.' };
          worker.disconnect();
          lastError = undefined;
        }, 0);
      } else {
        pages.forEach((listen) => listen(page));
      }
      return worker;
    },
  };
  return { runtime, pages, served };
}

function listeningRelay(served) {
  return (port) => {
    port.onMessage.addListener((message) => {
      if (!message.call) { return; }
      served.push(message.call.method);
      port.postMessage({ reply: { id: message.call.id, instance: message.call.instance, result: 'answer to ' + message.call.method } });
    });
  };
}

function loadWorker(runtime) {
  const sandbox = { chrome: { runtime }, console: { error() {}, log() {} }, Promise, Map, Set, JSON, Object, Array, Error, String, Date, setTimeout, clearTimeout };
  sandbox.self = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(read('bridge.js'), sandbox);
  return sandbox;
}

{
  // A call made before any relay listens waits for one, and each call is
  // answered once by the relay that listens.
  const w = workerWorld();
  const worker = loadWorker(w.runtime);
  const outcome = (p) => p.then((value) => value, (e) => 'rejected: ' + e.message);
  const first = outcome(worker.__bridgeCall('tabs', 'query', [{}]));
  const second = outcome(worker.__bridgeCall('tabs', 'get', [7]));
  await wait(20);
  w.pages.push(listeningRelay(w.served));
  assert.equal(await first, 'answer to tabs.query');
  assert.equal(await second, 'answer to tabs.get');
  assert.deepEqual(w.served.slice().sort(), ['tabs.get', 'tabs.query']);
}
{
  // A call with no relay at all fails with an error within the bridge's
  // bound, and the relay never sees it.
  const w = workerWorld();
  const worker = loadWorker(w.runtime);
  const started = Date.now();
  await assert.rejects(worker.__bridgeCall('tabs', 'query', [{}]), /found no listener/);
  assert.ok(Date.now() - started < 10000, 'the failure is bounded');
  assert.deepEqual(w.served, []);
}
{
  // A relay that listens from the start answers a call as before.
  const w = workerWorld();
  w.pages.push(listeningRelay(w.served));
  const worker = loadWorker(w.runtime);
  assert.equal(await worker.__bridgeCall('tabs', 'query', [{}]), 'answer to tabs.query');
  assert.deepEqual(w.served, ['tabs.query']);
}
{
  // A page's tab is reported before any of its calls, and the calls wait for
  // the report: the browser handles the report first, so a call is answered
  // with the Chromium id the report named.
  const p = pageContext({});
  p.chrome.tabs.getCurrent = () => Promise.resolve({ id: 433, windowId: 1 });
  p.mutate();
  p.chrome.tabs.query({});
  p.chrome.tabs.get(433);
  assert.deepEqual(p.sent, [], 'a call went before the report');
  await tick(); await tick();
  assert.deepEqual(p.reports, [{ tab: 433 }]);
  assert.deepEqual(p.events, ['report', 'call', 'call']);
  assert.deepEqual(p.sent.map((m) => [m.ns, m.method]), [['tabs', 'query'], ['tabs', 'get']]);
  // A later call goes at once: the gate is open.
  p.chrome.tabs.query({});
  assert.equal(p.sent.length, 3);
}
{
  // A page that is no tab (no tab id from Chromium) reports nothing and its
  // calls go at once.
  const p = pageContext({});
  p.chrome.tabs.getCurrent = () => Promise.resolve(undefined);
  p.mutate();
  p.chrome.tabs.query({});
  await tick(); await tick();
  assert.deepEqual(p.reports, []);
  assert.equal(p.sent.length, 1);
}
{
  // Chromium's answer that fails, or never comes: the report is given up
  // after a bound, and the calls then go (a call is never held for good).
  const failed = pageContext({});
  failed.chrome.tabs.getCurrent = () => Promise.reject(new Error('no'));
  failed.mutate();
  failed.chrome.tabs.query({});
  await tick(); await tick();
  assert.equal(failed.sent.length, 1);
  const silent = pageContext({});
  silent.chrome.tabs.getCurrent = () => new Promise(() => {});
  silent.mutate();
  silent.chrome.tabs.query({});
  await wait(20);
  assert.equal(silent.sent.length, 0, 'the bound has not passed');
  await wait(1100);
  assert.equal(silent.sent.length, 1, 'the bound passed, and the call went');
  assert.deepEqual(silent.reports, []);
}
{
  // The worker adopts its bridge port only from this extension's relay page. A
  // port named the same from a content script (it has a tab) is never adopted,
  // and no call goes out on it.
  const w = workerWorld();
  const worker = loadWorker(w.runtime);
  const forged = portEnd('nautilus-bridge-1');
  forged.sender = { id: 'x', tab: { id: 3 }, url: 'https://web.example/' };
  const forgedSent = [];
  forged.postMessage = (message) => { forgedSent.push(message); };
  w.runtime.onConnect.fire(forged);
  w.pages.push(listeningRelay(w.served));
  assert.equal(await worker.__bridgeCall('tabs', 'query', [{}]), 'answer to tabs.query');
  assert.deepEqual(forgedSent, [], 'a call went out on a content script port');
  assert.deepEqual(w.served, ['tabs.query']);
}
{
  // A port from the relay page is adopted, and the worker's call travels on it.
  const w = workerWorld();
  const worker = loadWorker(w.runtime);
  const relayed = portEnd('nautilus-bridge-1');
  // The relay page is hosted in a tab (the engine's hidden browser): it has a sender tab.
  relayed.sender = { id: 'x', tab: { id: 9 }, url: 'chrome-extension://x/nautilus-bridge/relay.html' };
  const relayEnd = portEnd('nautilus-bridge-1');
  relayed.peer = relayEnd;
  relayEnd.peer = relayed;
  listeningRelay(w.served)(relayEnd);
  w.runtime.onConnect.fire(relayed);
  assert.equal(await worker.__bridgeCall('tabs', 'query', [{}]), 'answer to tabs.query');
  assert.deepEqual(w.served, ['tabs.query']);
}
{
  // The relay adopts its worker's port: a port from a content script (with a
  // tab), or from another extension, is ignored, and what it sends never
  // reaches the relay's calls.
  // The rewritten manifest names the loader as the service worker.
  const runtime = {
    id: 'x', lastError: undefined, onConnect: nativeEvent(),
    connect: () => { throw new Error('connect is not expected'); },
    getManifest: () => ({ background: { service_worker: 'background/nautilus-loader.js' } }),
    getURL: (path) => 'chrome-extension://x/' + path,
  };
  const sandbox = { chrome: { runtime }, console: { error() {}, log() {} }, Promise, Map, JSON, Object, Array, Error, String, setTimeout, clearTimeout };
  sandbox.self = sandbox;
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(read('relay.js'), sandbox);
  const forgedFrom = (sender) => {
    const port = portEnd('nautilus-bridge-1');
    port.sender = sender;
    port.peer = portEnd('nautilus-bridge-1');
    port.peer.peer = port;
    return port;
  };
  const content = forgedFrom({ id: 'x', tab: { id: 3 }, url: 'https://web.example/' });
  runtime.onConnect.fire(content);
  content.peer.postMessage({ call: { id: 1, method: 'group', args: [] } });
  const seen = await Promise.race([sandbox.nautilusRelay.next(), wait(30).then(() => 'nothing')]);
  assert.equal(seen, 'nothing', 'a content script port was adopted by the relay');
  const other = forgedFrom({ id: 'y' });
  runtime.onConnect.fire(other);
  other.peer.postMessage({ call: { id: 2, method: 'query', args: [] } });
  assert.equal(await Promise.race([sandbox.nautilusRelay.next(), wait(30).then(() => 'nothing')]), 'nothing');
  // Another page of this extension (a popup, the options page) and another
  // script of it are not the worker: their ports are refused too.
  for (const url of ['chrome-extension://x/popup.html', 'chrome-extension://x/options.html', 'chrome-extension://x/background/other.js']) {
    const page = forgedFrom({ id: 'x', url });
    runtime.onConnect.fire(page);
    page.peer.postMessage({ call: { id: 4, method: 'query', args: [] } });
    assert.equal(await Promise.race([sandbox.nautilusRelay.next(), wait(30).then(() => 'nothing')]), 'nothing', `the relay adopted a port from ${url}`);
  }
  // The worker's own port (from this extension, no tab) is adopted.
  const worker = forgedFrom({ id: 'x', url: 'chrome-extension://x/background/nautilus-loader.js' });
  runtime.onConnect.fire(worker);
  worker.peer.postMessage({ call: { id: 3, method: 'query', args: [] } });
  assert.equal(JSON.stringify(await sandbox.nautilusRelay.next()), JSON.stringify([{ call: { id: 3, method: 'query', args: [] } }]));
}
{
  // A page's call that the browser never answers is refused after the same
  // bound a worker's call has (the call never hangs).
  const p = pageContext({ answers: false });
  const started = Date.now();
  const silent = p.sandbox.__bridgeCall('tabs', 'query', []).then(() => 'answered', (e) => e.message);
  const outcome = await Promise.race([silent, wait(11000).then(() => 'hung')]);
  assert.equal(outcome, 'Nautilus: query was not answered.');
  assert.ok(Date.now() - started >= 9000, 'the bound is the worker\'s, about ten seconds');
}
{
  // Fail closed: a context whose bridge cannot be put in (no transport for
  // core.js, or no document for the deferred install) keeps no callable native
  // tabs or windows: a native tabs.group reaches Chromium and traps it.
  const reached = [];
  const nativeFn = (name) => () => { reached.push(name); };
  const chrome = { tabs: { group: nativeFn('tabs.group'), TAB_ID_NONE: -1 }, windows: { create: nativeFn('windows.create') } };
  const sandbox = { chrome, console: { error() {}, log() {} }, Promise, Map, Set, JSON, Object, Array, Error, String, Proxy };
  sandbox.self = sandbox;
  vm.createContext(sandbox);
  // The page's transport runs first in every extension page (page_script).
  vm.runInContext(read('page_transport.js'), sandbox);
  vm.runInContext(read('core.js'), sandbox);
  await assert.rejects(Promise.resolve().then(() => chrome.tabs.group({ tabIds: [1] })));
  assert.deepEqual(reached, [], 'core.js left a native tabs.group callable');
}
{
  const reached = [];
  const nativeFn = (name) => () => { reached.push(name); };
  const chrome = { tabs: { group: nativeFn('tabs.group') }, windows: { create: nativeFn('windows.create') } };
  const sandbox = { chrome, console: { error() {}, log() {} }, Promise, Map, Set, JSON, Object, Array, Error, String, Proxy };
  sandbox.self = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(read('page_transport.js'), sandbox);
  vm.runInContext(`(function (root, install) {\n${read('page_defer.js')}\n})(self, function () { throw new Error('install ran'); });`, sandbox);
  await assert.rejects(Promise.resolve().then(() => chrome.tabs.group({ tabIds: [1] })));
  await assert.rejects(Promise.resolve().then(() => chrome.windows.create({})));
  assert.deepEqual(reached, [], 'a context with no document left native tabs callable');
}
{
  // A page whose bridge functions could not be bound (no `__nautilusBridgeSend`):
  // the transport defines the refusal and returns, and the refusal, called once,
  // leaves no native tabs or windows callable.
  const reached = [];
  const nativeFn = (name) => () => { reached.push(name); };
  const chrome = { tabs: { group: nativeFn('tabs.group') }, windows: { create: nativeFn('windows.create') } };
  const sandbox = { chrome, console: { error() {}, log() {} }, Promise, Map, Set, JSON, Object, Array, Error, String, Proxy };
  sandbox.self = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(read('page_transport.js'), sandbox);
  vm.runInContext('self.__nautilusRefuseNative();', sandbox);
  await assert.rejects(Promise.resolve().then(() => chrome.tabs.group({ tabIds: [1] })));
  await assert.rejects(Promise.resolve().then(() => chrome.windows.create({})));
  assert.deepEqual(reached, [], 'the refusal left a native tabs or windows call callable');
  assert.equal(sandbox.__bridgeCall, undefined, 'the transport answered without a send');
}
// window.close() in an extension page asks the engine first: a side panel's
// page is closed by the model (the engine answers nothing), any other page
// closes itself (false, or no answer at all).
for (const [answer, closes] of [[() => null, 0], [() => false, 1], [() => { throw new Error('the extension is not bridged.'); }, 1]]) {
  let closed = 0;
  const asked = [];
  const sandbox = { chrome: { runtime: { id: 'x' } }, console: { error() {}, log() {} }, close: () => { closed += 1; } };
  sandbox.self = sandbox;
  sandbox.window = sandbox;
  sandbox.top = sandbox;
  sandbox.__bridgeCall = (ns, method, args) => {
    asked.push([ns, method]);
    try { return Promise.resolve(answer(args)); } catch (e) { return Promise.reject(e); }
  };
  sandbox.__bridgeOnEvent = () => {};
  vm.createContext(sandbox);
  vm.runInContext(read('core.js'), sandbox);
  vm.runInContext(read('side_panel.js'), sandbox);
  vm.runInContext('window.close()', sandbox);
  await tick(); await tick();
  assert.deepEqual(asked, [['sidePanel', 'closeSelf']]);
  assert.equal(closed, closes);
}

console.log('bridge scripts: ok');
