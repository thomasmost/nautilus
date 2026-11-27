// The worker of the side panel's test extension. It asks the harness
// (panel_checks.py) for its suite when it starts; a suite is handed out
// once, to the first worker that asks. Each check posts a `step` record.
// A checkpoint posts `checkpoint` and waits until the harness releases
// it, after it has seen the page or sent the click it waited for.
// Reports from the content script and the pages come in through the
// onMessage listener and are forwarded. Failed fetches are dropped, so
// the file does nothing when no harness is listening.
(function () {
  'use strict';
  const HARNESS = 'http://127.0.0.1:47616';
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const post = (kind, data) =>
    fetch(HARNESS + '/record', { method: 'POST', body: JSON.stringify({ kind, data }) }).catch(() => {});
  const sorted = (value) => {
    if (Array.isArray(value)) return value.map(sorted);
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted(value[key])]));
    }
    return value;
  };
  const same = (a, b) => JSON.stringify(sorted(a)) === JSON.stringify(sorted(b));
  const step = (name, ok, detail) =>
    post('step', { name, ok: ok === true, detail: detail === undefined ? null : detail });
  const rejection = (promise) => promise.then(() => null, (error) => error.message);
  async function until(test, secs) {
    const end = Date.now() + secs * 1000;
    while (Date.now() < end) {
      if (test()) return true;
      await sleep(100);
    }
    return test();
  }
  async function checkpoint(name) {
    await post('checkpoint', { name });
    for (;;) {
      const answer = await fetch(HARNESS + '/go?name=' + encodeURIComponent(name))
        .then((r) => r.text())
        .catch(() => 'wait');
      if (answer === 'go') return;
      await sleep(200);
    }
  }

  // What this context's chrome.sidePanel is: the engine's object (frozen,
  // put in place with defineProperty on `chrome`) or Chromium's own with
  // its members pinned (the script's fallback).
  function describeSidePanel() {
    const sp = chrome.sidePanel;
    const descriptor = Object.getOwnPropertyDescriptor(chrome, 'sidePanel') || {};
    const member = sp ? Object.getOwnPropertyDescriptor(sp, 'open') || {} : {};
    return {
      present: !!sp,
      frozen: !!sp && Object.isFrozen(sp),
      keys: sp ? Object.keys(sp).sort() : [],
      property: {
        data: 'value' in descriptor,
        writable: descriptor.writable === true,
        configurable: descriptor.configurable === true,
        getter: typeof descriptor.get,
      },
      open: { writable: member.writable === true, configurable: member.configurable === true },
      bridge: typeof self.__nautilusBridge,
      installed: self.__nautilusBridge ? self.__nautilusBridge.installed : null,
    };
  }

  const opened = [];
  const closed = [];
  const removed = {}; // tab id -> when tabs.onRemoved fired (ms)

  chrome.runtime.onMessage.addListener((message) => {
    if (message && message.report) post(message.report.kind, message.report.data);
  });
  chrome.tabs.onRemoved.addListener((id) => {
    removed[id] = Date.now();
  });

  async function panelSuite() {
    const sp = chrome.sidePanel;
    const self_ = chrome.runtime.id;
    const tabs = await chrome.tabs.query({});
    const active = tabs.find((t) => t.active) || tabs[0];
    const other = tabs.find((t) => t.id !== active.id);
    const window = active.windowId;
    await post('tabs', {
      id: self_,
      active: active.id,
      other: other && other.id,
      window,
      activeUrl: active.url,
      otherUrl: other && other.url,
    });
    step('two tabs', tabs.length === 2, tabs.map((t) => t.url));
    post('sidepanel-info', { context: 'worker', info: describeSidePanel() });
    step('the bridge owns chrome.sidePanel', Object.isFrozen(sp) && typeof sp.getLayout === 'function');
    sp.onOpened.addListener((info) => opened.push(info));
    sp.onClosed.addListener((info) => closed.push(info));

    // The harness reads the tab's full width here, before any panel.
    await checkpoint('start');

    step('getOptions({}) is the manifest page', same(await sp.getOptions({}), { enabled: true, path: 'panel.html' }));
    step('getLayout', same(await sp.getLayout(), { side: 'right' }));
    await sp.setPanelBehavior({ openPanelOnActionClick: true });
    step('setPanelBehavior then getPanelBehavior', same(await sp.getPanelBehavior(), { openPanelOnActionClick: true }));

    // side-panel.AC5.3
    const refusals = [
      ['AC5.3 open with a missing tab rejects', { tabId: 999999 }, 'No tab with id: 999999.'],
      ['AC5.3 open with a missing window rejects', { windowId: 999999 }, 'No window with id: 999999.'],
      ['AC5.3 open with neither id rejects', {}, 'At least one of `tabId` and `windowId` must be provided'],
      ['AC5.3 open with a tab and a missing window rejects', { tabId: active.id, windowId: 999999 }, 'No window with id: 999999.'],
    ];
    for (const [name, options, message] of refusals) {
      const error = await rejection(sp.open(options));
      step(name, error === message, error);
    }
    step('AC5.3 nothing opened', opened.length === 0, opened);

    // side-panel.AC5.1 (window), AC1.1 and AC1.4: the harness checks the
    // pages; this side checks what the options and contexts say.
    const openedWindow = await rejection(sp.open({ windowId: window }));
    step('AC5.1 open({windowId}) resolves', openedWindow === null, openedWindow);
    await checkpoint('window-panel');
    const contexts = await chrome.runtime.getContexts({ contextTypes: ['SIDE_PANEL'] });
    step(
      'AC5.2 getContexts reports the open panel',
      contexts.length === 1 &&
        contexts[0].documentUrl === `chrome-extension://${self_}/panel.html` &&
        contexts[0].windowId === window &&
        contexts[0].tabId === -1,
      contexts
    );
    post('contexts', await chrome.runtime.getContexts({}).then((all) => all.map((c) => [c.contextType, c.tabId, c.windowId, c.documentUrl])));

    // side-panel.AC5.1 (tab) and AC1.4
    await sp.setOptions({ tabId: other.id, path: 'panel-tab.html', enabled: true });
    step(
      "getOptions({tabId}) is the tab's own",
      same(await sp.getOptions({ tabId: other.id }), { enabled: true, path: 'panel-tab.html', tabId: other.id })
    );
    const openedTab = await rejection(sp.open({ tabId: other.id }));
    step('AC5.1 setOptions({tabId, path, enabled}) then open({tabId}) resolves', openedTab === null, openedTab);
    await checkpoint('tab-panel-hidden');
    await chrome.tabs.update(other.id, { active: true });
    await checkpoint('tab-panel-shown');
    await chrome.tabs.update(active.id, { active: true });
    await checkpoint('window-panel-again');
    const both = await chrome.runtime.getContexts({ contextTypes: ['SIDE_PANEL'] });
    step('getContexts reports both panels', both.length === 2, both);

    // side-panel.AC5.2: setOptions({enabled: false}) closes the tab's panel
    await sp.setOptions({ tabId: other.id, enabled: false });
    step(
      'AC5.2 setOptions({enabled: false}) closes the open panel',
      await until(() => closed.some((i) => i.tabId === other.id && i.path === 'panel-tab.html'), 10),
      closed
    );
    await checkpoint('tab-panel-closed');
    const refused = await rejection(sp.open({ tabId: other.id }));
    step('open for the disabled tab rejects', refused === `No active side panel for tabId: ${other.id}`, refused);

    // side-panel.AC5.2: window.close() in the panel closes it. The page is
    // told to close itself (panel.js obeys this command).
    chrome.runtime.sendMessage({ command: 'window.close' }).catch(() => {});
    await checkpoint('window-closed-by-page');
    step(
      'AC5.2 window.close() in the panel closes it',
      await until(() => closed.some((i) => i.tabId === undefined && i.windowId === window), 10),
      closed
    );
    const none = await chrome.runtime.getContexts({ contextTypes: ['SIDE_PANEL'] });
    step('AC5.2 getContexts reports no panel', none.length === 0, none);

    // close(), and the events
    await sp.open({ windowId: window });
    await checkpoint('window-panel-reopened');
    const closedCall = await sp.close({ windowId: window }).then(() => true, (error) => error.message);
    step('close({windowId}) resolves', closedCall === true, closedCall);
    step(
      'onOpened fired three times and onClosed three',
      await until(() => opened.length === 3 && closed.length === 3, 10),
      { opened, closed }
    );
    step(
      "the tab panel's events name its tab",
      opened.some((i) => i.tabId === other.id && i.path === 'panel-tab.html'),
      opened
    );

    // The action click: the harness sends a `click` command for this
    // extension, and the same decision as the toolbar's button runs.
    await checkpoint('click-open');
    await checkpoint('click-close');
    step(
      'a click on the action opened and closed the window panel',
      await until(() => opened.length === 4 && closed.length === 4, 10),
      { opened: opened.length, closed: closed.length }
    );

    // window.close() in an extension page that is no panel: the script
    // asks the engine first (sidePanel.closeSelf), then closes natively.
    // closer.html times the question and then closes itself.
    const closer = await chrome.tabs.create({ url: chrome.runtime.getURL('closer.html'), active: false });
    post('closer-tab', { id: closer.id });
    await checkpoint('closer');
    const gone = await until(() => removed[closer.id] !== undefined, 10);
    post('closer-gone', { gone, at: removed[closer.id] || null });
    if (!gone) await chrome.tabs.remove(closer.id).catch(() => {});

    // Left open for the steps by hand (side-panel.AC1.2, AC1.3).
    await sp.open({ windowId: window });
    await checkpoint('left-open');
  }

  // The first launch of the behaviour check: only what the cookbook's
  // global sample does, plus reading it back.
  async function behaviorSuite() {
    const sp = chrome.sidePanel;
    post('tabs', { id: chrome.runtime.id });
    post('sidepanel-info', { context: 'worker', info: describeSidePanel() });
    await sp.setPanelBehavior({ openPanelOnActionClick: true });
    step('setPanelBehavior then getPanelBehavior', same(await sp.getPanelBehavior(), { openPanelOnActionClick: true }));
    await checkpoint('behavior-set');
  }

  const suites = { panel: panelSuite, behavior: behaviorSuite };

  fetch(HARNESS + '/suite')
    .then((r) => r.json())
    .then(({ suite: name }) => {
      const suite = suites[name];
      if (!suite) return;
      suite()
        .catch((error) => step('the suite ran to the end', false, String((error && error.message) || error)))
        .then(() => post('done', name));
    })
    .catch(() => {});
})();
