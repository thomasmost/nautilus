importScripts('/phase5.js');
importScripts('lib.js'); // relative: resolves beside the worker's own script

const RUN = Math.random().toString(36).slice(2, 8); // one per worker start
const ROUNDS = 40;

// M8: what the bridge's wrapping of onConnect answers, probed before this
// worker adds any listener of its own (the bridge's own listener must not
// count, and a spy added and removed must come and go).
(function probeListeners() {
  const onConnect = chrome.runtime.onConnect;
  const before = onConnect.hasListeners();
  const spy = () => {};
  onConnect.addListener(spy);
  const withSpy = { hasListeners: onConnect.hasListeners(), hasListener: onConnect.hasListener(spy) };
  onConnect.removeListener(spy);
  const removed = onConnect.hasListeners();
  post('probe', { run: RUN, before, withSpy, removed });
})();

// The extension's own listener, registered at startup as any extension's is.
// It must never see the bridge's port (U3).
chrome.runtime.onConnect.addListener((port) => { post('spy', { run: RUN, port: port.name }); });
chrome.runtime.onInstalled.addListener((d) => { post('event', { run: RUN, name: 'runtime.onInstalled', reason: d.reason }); });
chrome.runtime.onStartup.addListener(() => { post('event', { run: RUN, name: 'runtime.onStartup' }); });
chrome.action.onClicked.addListener((tab) => {
  post('event', { run: RUN, name: 'action.onClicked', tab: slim(tab) });
  // `quietClick` (the stop mode's restart): the click only starts the worker;
  // checks() would attach and detach a debugger on every tab, the held one too.
  config().then((cfg) => (cfg.latency ? latency() : cfg.quietClick ? null : checks()));
});

post('sw-start', { run: RUN, id: chrome.runtime.id, bridge: self.__nautilusTransport || null, script: self.location.pathname });
config().then((cfg) => {
  if (cfg.latencyAtStart) { latency(); }
  if (cfg.provokeReload) { provokeReload(); }
  if (cfg.provokeStop) { provokeStop(); }
  if (cfg.provokeStopCheck) { afterStop(cfg.stopTab); }
});

// The strip reaches the engine after the worker's start: a query in the first
// moments answers from a strip that is not published yet (an empty list; the
// engine's held strip, see docs/bridge-protocol.md). The provocations look for
// a background tab only once both tabs are listed (at most 20 s).
async function listedTabs() {
  const end = Date.now() + 20000;
  for (;;) {
    const list = await chrome.tabs.query({});
    if (list.length >= 2 || Date.now() > end) { return list; }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

// I10 (identity, by a stop): this worker attaches a debugger to the background
// tab and reports it. The driver then stops the worker over DevTools; the
// engine restarts it through the relay to ask who it is, and the restarted
// worker checks whether its own debugger session survived the stop.
async function provokeStop() {
  const tabs = await timed(RUN, 'provoke.tabs.query', () => listedTabs().then((list) => list.map(slim)));
  const background = (tabs.result || []).find((t) => !t.active);
  if (!background) { await post('stop-ready', { run: RUN, error: 'no background tab' }); return; }
  const attach = await timed(RUN, 'provoke.debugger.attach', () => chrome.debugger.attach({ tabId: background.id }, '1.3').then(() => 'attached'), { tabId: background.id });
  await post('stop-ready', { run: RUN, tabId: background.id, attached: attach.ok === true, error: attach.error || null });
}

async function afterStop(tabId) {
  const target = { tabId: tabId };
  // A bridged call first: the worker opens its own port, and its hello tells
  // the engine that a new instance runs (the engine asks for no identity on a
  // `gone` any more, I15). The answer is not needed here.
  await timed(RUN, 'afterstop.tabs.query', () => chrome.tabs.query({}).then((list) => list.length), {});
  const check = await timed(RUN, 'afterstop.debugger.sendCommand', () =>
    chrome.debugger.sendCommand(target, 'Runtime.evaluate', { expression: 'location.href', returnByValue: true }).then((r) => r.result.value), { tabId: tabId });
  await post('after-stop', { run: RUN, tabId: tabId, ok: check.ok === true, error: check.error || null });
}

// I10: the debugger's session is seen from the worker it belongs to. Any
// detach Chromium makes (the reload's included) is posted here, if this
// worker still runs to hear it.
chrome.debugger.onDetach.addListener((source, reason) => {
  post('event', { run: RUN, name: 'debugger.onDetach', source, reason });
});

// I10 provocation: this worker attaches a debugger to the background tab
// (the bridge reports it, and the engine must show the tab), then reloads
// the extension. The driver clears provokeReload when 'reloading' arrives,
// so the worker that the reload starts does not reload again.
async function provokeReload() {
  const tabs = await timed(RUN, 'provoke.tabs.query', () => listedTabs().then((list) => list.map(slim)));
  const background = (tabs.result || []).find((t) => !t.active);
  if (!background) { await post('provoke', { run: RUN, error: 'no background tab: ' + (tabs.error || 'none is inactive') }); return; }
  const target = { tabId: background.id };
  const attach = await timed(RUN, 'provoke.debugger.attach', () => chrome.debugger.attach(target, '1.3').then(() => 'attached'), { tabId: background.id });
  await post('provoke', { run: RUN, tabId: background.id, attached: attach.ok === true, error: attach.error || null });
  await post('reloading', { run: RUN, tabId: background.id });
  chrome.runtime.reload();
}

// tabs.query through the bridge, then Chromium's own id-addressed APIs
// on every id it returned.
async function checks() {
  const query = await timed(RUN, 'tabs.query({})', () => chrome.tabs.query({}).then((tabs) => tabs.map(slim)));
  for (const tab of query.result || []) {
    const about = { tabId: tab.id, url: tab.url };
    const target = { tabId: tab.id };
    await timed(RUN, 'scripting.executeScript', () =>
      chrome.scripting.executeScript({ target, func: () => location.href }).then((r) => r[0].result), about);
    await timed(RUN, 'debugger.attach', () => chrome.debugger.attach(target, '1.3').then(() => 'attached'), about);
    await timed(RUN, 'debugger.Runtime.evaluate', () =>
      chrome.debugger.sendCommand(target, 'Runtime.evaluate', { expression: 'location.href', returnByValue: true }).then((r) => r.result.value), about);
    await timed(RUN, 'debugger.detach', () => chrome.debugger.detach(target).then(() => 'detached'), about);
  }
  // I8: the attach that names the tab by its DevTools target id, once for
  // the first tab, with its detach.
  const first = (query.result || [])[0];
  if (first) {
    const targets = await chrome.debugger.getTargets().catch(() => []);
    const page = targets.find((t) => t.type === 'page' && t.tabId === first.id);
    if (page) {
      const byTarget = { targetId: page.id };
      const about = { tabId: first.id, url: first.url };
      await timed(RUN, 'debugger.attach({targetId})', () => chrome.debugger.attach(byTarget, '1.3').then(() => 'attached'), about);
      await timed(RUN, 'debugger.detach({targetId})', () => chrome.debugger.detach(byTarget).then(() => 'detached'), about);
    }
  }
  await post('checks-done', { run: RUN, tabs: query.result || [], error: query.error || null });
}

// The calls that stalled in the probe, one after another, each timed. The
// debugged tab is the BACKGROUND one (not the window's active tab), so the
// calls go to a tab the renderer is hidden for: the case that stalled.
// Uses no bridge for the target: it comes from chrome.debugger.getTargets().
async function latency() {
  const tabs = await timed(RUN, 'tabs.query({}) for the latency run', () => listedTabs().then((list) => list.map(slim)));
  const background = (tabs.result || []).find((t) => !t.active);
  if (!background) { await post('latency-done', { run: RUN, error: 'no background tab: ' + (tabs.error || 'none is inactive') }); return; }
  const targets = await chrome.debugger.getTargets();
  const page = targets.find((t) => t.type === 'page' && t.tabId === background.id);
  if (!page) { await post('latency-done', { run: RUN, error: 'no page target for the background tab' }); return; }
  const target = { tabId: page.tabId };
  const ms = [];
  let timeouts = 0;
  const step = async (name, fn) => {
    const record = await timed(RUN, name, fn, { latency: true });
    ms.push(record.ms);
    if (!record.ok && /TIMEOUT/.test(record.error)) { timeouts += 1; }
  };
  await step('debugger.attach', () => chrome.debugger.attach(target, '1.3').then(() => 'attached'));
  for (let i = 0; i < ROUNDS && timeouts < 5; i++) {
    await step('Runtime.evaluate', () => chrome.debugger.sendCommand(target, 'Runtime.evaluate', { expression: 'document.title', returnByValue: true }).then((r) => r.result.value));
    await step('Page.captureScreenshot', () => chrome.debugger.sendCommand(target, 'Page.captureScreenshot', { format: 'png' }).then((r) => r.data.length));
    await step('Input.dispatchMouseEvent', () => chrome.debugger.sendCommand(target, 'Input.dispatchMouseEvent', { type: 'mouseMoved', x: 60, y: 60 }).then(() => 'moved'));
    await step('tabs.get', () => chrome.tabs.get(page.tabId).then((t) => t.url));
    await step('runtime.getContexts', () => chrome.runtime.getContexts({}).then((c) => c.length));
  }
  await step('debugger.detach', () => chrome.debugger.detach(target).then(() => 'detached'));
  const sorted = ms.slice().sort((a, b) => a - b);
  await post('latency-done', {
    run: RUN, tabId: page.tabId, count: ms.length, timeouts,
    max: sorted[sorted.length - 1], p95: sorted[Math.floor(sorted.length * 0.95)],
    over1s: ms.filter((m) => m >= 1000).length,
  });
}
