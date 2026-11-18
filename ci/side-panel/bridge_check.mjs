// A check of the bridge scripts against a stand-in for Chromium's chrome
// object, for a development machine with Node (it is not part of CI):
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

function world({ viaPage }) {
  const reached = []; // Chromium functions that were really called
  const calls = []; // what the engine was asked
  const answers = new Map(); // "ns.method" -> (args) => value | throws
  let handler = null;
  const messages = nativeEvent(); // Chromium's runtime.onMessage
  const runtime = { onMessage: messages, id: 'x' };
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
  };
  const engine = (ns, method, args) => {
    // Through JSON, as on the wire (and so the arrays are this realm's).
    calls.push(JSON.parse(JSON.stringify([ns, method, args])));
    const answer = answers.get(ns + '.' + method);
    try { return Promise.resolve(answer ? answer(args) : null); } catch (e) { return Promise.reject(e); }
  };
  const sandbox = { chrome, console: { error: (...a) => sandbox.errors.push(a.join(' ')), log() {} }, errors: [], Promise, Map, JSON, Object, Array, Error, String };
  sandbox.self = sandbox;
  vm.createContext(sandbox);
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
  }
  for (const file of ['core.js', 'tabs.js', 'windows.js', 'runtime.js']) {
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

  // runtime.onMessage: sender.tab gets the strip's fields first.
  w.answers.set('tabs.stripOf', ([id]) => (id === 7 ? { index: 3, windowId: 1, groupId: -1, active: true } : null));
  const senders = [];
  chrome.runtime.onMessage.addListener((message, sender, respond) => { senders.push(sender.tab && sender.tab.windowId); if (message === 'ask') { respond('answer'); } });
  assert.equal(w.messages.count(), 1);
  const responses = [];
  assert.deepEqual(w.messages.fire('ask', { tab: { id: 7, windowId: -1 } }, (v) => responses.push(v)), [true]);
  await tick(); await tick();
  assert.deepEqual(senders, [1]);
  assert.deepEqual(responses, ['answer']);
  // Nobody answers: the sender is let go with undefined.
  w.messages.fire('other', { tab: { id: 8, windowId: -1 } }, (v) => responses.push(['closed', v]));
  await tick(); await tick();
  assert.deepEqual(responses[1], ['closed', undefined]);
  // No tab: straight through, synchronously.
  assert.deepEqual(w.messages.fire('ask', {}, (v) => responses.push(v)), [false]);
  assert.equal(responses[2], 'answer');
  // An asynchronous listener keeps the channel.
  chrome.runtime.onMessage.addListener((message) => (message === 'later' ? Promise.resolve('promised') : undefined));
  assert.deepEqual(w.messages.fire('later', {}, (v) => responses.push(v)), [true]);
  await tick();
  assert.equal(responses[3], 'promised');
}
console.log('bridge scripts: ok');
