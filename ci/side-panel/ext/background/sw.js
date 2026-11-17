importScripts('lib.js'); // relative: resolves beside the worker's own script

const RUN = Math.random().toString(36).slice(2, 8); // one per worker start
const ROUNDS = 40;

// The extension's own listeners, registered at startup as any
// extension's are. The first must never see the bridge's port.
chrome.runtime.onConnect.addListener((port) => { post('spy', { run: RUN, port: port.name }); });
chrome.runtime.onInstalled.addListener((d) => { post('event', { run: RUN, name: 'runtime.onInstalled', reason: d.reason }); });
chrome.runtime.onStartup.addListener(() => { post('event', { run: RUN, name: 'runtime.onStartup' }); });
chrome.action.onClicked.addListener((tab) => {
  post('event', { run: RUN, name: 'action.onClicked', tab: slim(tab) });
  config().then((cfg) => (cfg.latency ? latency() : checks()));
});

post('sw-start', { run: RUN, id: chrome.runtime.id, bridge: self.__nautilusTransport || null, script: self.location.pathname });
config().then((cfg) => { if (cfg.latencyAtStart) { latency(); } });

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
  await post('checks-done', { run: RUN, tabs: query.result || [], error: query.error || null });
}

// The calls that stalled in the probe, one after another, each timed.
// Uses no bridge: the tab comes from chrome.debugger.getTargets().
async function latency() {
  const targets = await chrome.debugger.getTargets();
  const page = targets.find((t) => t.type === 'page' && t.tabId !== undefined && /^https?:/.test(t.url));
  if (!page) { await post('latency-done', { run: RUN, error: 'no page target' }); return; }
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
