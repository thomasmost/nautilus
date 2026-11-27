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

  chrome.runtime.onMessage.addListener((message, sender) => {
    if (message && message.report) post(message.report.kind, message.report.data);
    // A panel page says who it is: what the worker sees as its sender.
    if (message && message.whoami) {
      post('sender', { page: message.whoami, tab: sender.tab || null, url: sender.url || null, frameId: sender.frameId });
    }
  });
  chrome.tabs.onRemoved.addListener((id) => {
    removed[id] = Date.now();
  });

  async function panelSuite() {
    const sp = chrome.sidePanel;
    const self_ = chrome.runtime.id;
    // A worker that asks as it is installed asks before the engine has
    // published its first strip, and is answered with no tabs: wait for
    // the two.
    let tabs = await chrome.tabs.query({});
    const firstCount = tabs.length;
    for (let i = 0; i < 100 && tabs.length < 2; i += 1) {
      await sleep(200);
      tabs = await chrome.tabs.query({});
    }
    post('note', `the first tabs.query answered ${firstCount} tab(s)`);
    // The tabs loaded before the extension was installed, so they have no
    // content script: reloaded, they do. Its reports also keep this
    // worker running through the checkpoints.
    for (const tab of tabs) await chrome.tabs.reload(tab.id);
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
    step(
      "getContexts({}) lists the worker and the panel, and not the bridge's relay page",
      await chrome.runtime.getContexts({}).then(
        (all) => same(all.map((c) => c.contextType).sort(), ['BACKGROUND', 'SIDE_PANEL']) && !all.some((c) => (c.documentUrl || '').includes('nautilus-bridge'))
      )
    );
    post('contexts', await chrome.runtime.getContexts({}).then((all) => all.map((c) => [c.contextType, c.tabId, c.windowId, c.documentUrl, c.contextId])));

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

    await extras(sp, window, active, other);

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

  // Beyond the acceptance lines: a new page for an open panel, what a
  // panel opens, a tab closed with its panel, a second window's panel and
  // that window closed with it. It starts and ends with no panel open.
  async function extras(sp, window, active, other) {
    const seen = () => ({ opened: opened.length, closed: closed.length });
    const types = { contextTypes: ['SIDE_PANEL'] };

    // setOptions({path}) on an open panel shows the new page.
    await sp.open({ windowId: window });
    await checkpoint('extra-open');

    // Another extension is installed while this worker and the panel's
    // page are alive (the harness loads one at this checkpoint). Chromium
    // then defines every API on `chrome` again in each live context; the
    // engine's chrome.sidePanel must stay. Chromium's own open() wants a
    // user gesture; the engine's says what is wrong with the call.
    await checkpoint('bystander');
    const still = await rejection(chrome.sidePanel.open({}));
    post('sidepanel-info', { context: 'worker, after another extension was installed', info: describeSidePanel() });
    step(
      "after another extension is installed chrome.sidePanel is still the engine's in the worker",
      Object.isFrozen(chrome.sidePanel) && still === 'At least one of `tabId` and `windowId` must be provided',
      still
    );
    chrome.runtime.sendMessage({ command: 'describe' }).catch(() => {});
    await checkpoint('bystander-page');
    let before = seen();
    await sp.setOptions({ path: 'panel-tab.html' });
    step(
      'setOptions({path}) on the open window panel closes the old page and opens the new',
      await until(
        () =>
          closed.length === before.closed + 1 &&
          opened.length === before.opened + 1 &&
          closed[closed.length - 1].path === 'panel.html' &&
          opened[opened.length - 1].path === 'panel-tab.html' &&
          opened[opened.length - 1].tabId === undefined,
        10
      ),
      { opened: opened.slice(before.opened), closed: closed.slice(before.closed) }
    );
    await checkpoint('path-changed');
    await sp.setOptions({ path: 'panel.html' });
    await checkpoint('path-restored');

    // window.open in a panel's page is a tab of the window, and the panel stays.
    const created = [];
    const onCreated = (tab) => created.push(tab);
    chrome.tabs.onCreated.addListener(onCreated);
    chrome.runtime.sendMessage({ command: 'window.open' }).catch(() => {});
    // The tab is announced blank and loads the address after.
    let listed = [];
    const fromPanel = () => listed.find((t) => (t.url || '').includes('from-panel'));
    for (let i = 0; i < 50 && !fromPanel(); i += 1) {
      await sleep(200);
      listed = await chrome.tabs.query({});
    }
    const opener = listed.find((t) => t.id === active.id);
    step(
      'window.open in the panel opens one tab, beside the tab the window shows',
      created.length === 1 && !!fromPanel() && fromPanel().id === created[0].id && !!opener && fromPanel().index === opener.index + 1,
      { created: created.map((t) => [t.id, t.index, t.url]), listed: listed.map((t) => [t.id, t.index, t.active, t.url]) }
    );
    await checkpoint('panel-opened-tab');
    chrome.tabs.onCreated.removeListener(onCreated);
    for (const tab of listed) {
      if (tab.id !== active.id && tab.id !== other.id) await chrome.tabs.remove(tab.id).catch(() => {});
    }
    await chrome.tabs.update(active.id, { active: true });

    // A tab closed with its own panel open.
    const extra = await chrome.tabs.create({ url: 'https://example.net/?own-panel', active: false });
    await sp.setOptions({ tabId: extra.id, path: 'panel-tab.html', enabled: true });
    before = seen();
    const ownOpen = await rejection(sp.open({ tabId: extra.id }));
    step(
      "a new tab's own panel opens",
      ownOpen === null && (await until(() => opened.length === before.opened + 1 && opened[opened.length - 1].tabId === extra.id, 10)),
      { ownOpen, opened: opened.slice(before.opened) }
    );
    await chrome.tabs.remove(extra.id);
    step(
      'closing a tab closes its own panel, and onClosed names the tab',
      await until(() => closed.length === before.closed + 1, 10) &&
        closed[closed.length - 1].tabId === extra.id &&
        closed[closed.length - 1].path === 'panel-tab.html',
      closed.slice(before.closed)
    );
    await checkpoint('tab-closed-with-panel');
    const afterTab = await chrome.runtime.getContexts(types);
    step('getContexts reports only the window panel after it', afterTab.length === 1 && afterTab[0].windowId === window, afterTab);

    // A second window: its own panel, and the window closed with it open.
    const second = await chrome.windows.create({ url: 'https://example.net/?second-window' });
    const secondTab = second.tabs && second.tabs[0];
    await post('second-window', { window: second.id, tab: secondTab && secondTab.id });
    before = seen();
    const secondOpen = await rejection(sp.open({ windowId: second.id }));
    step(
      "open({windowId}) for a second window opens that window's panel",
      secondOpen === null &&
        (await until(() => opened.length === before.opened + 1 && opened[opened.length - 1].windowId === second.id, 10)),
      { secondOpen, opened: opened.slice(before.opened) }
    );
    await checkpoint('second-window');
    const two = await chrome.runtime.getContexts(types);
    step(
      'getContexts reports a panel in each window',
      two.length === 2 && same(two.map((c) => c.windowId).sort(), [window, second.id].sort()),
      two
    );
    const inSecond = await chrome.runtime.getContexts({ contextTypes: ['SIDE_PANEL'], windowIds: [second.id] });
    step('getContexts({windowIds}) narrows to the second window', inSecond.length === 1 && inSecond[0].windowId === second.id, inSecond);
    await chrome.windows.remove(second.id);
    step(
      'closing a window closes its panel, and onClosed names the window',
      await until(() => closed.length === before.closed + 1, 10) && closed[closed.length - 1].windowId === second.id,
      closed.slice(before.closed)
    );
    const one = await chrome.runtime.getContexts(types);
    step('getContexts reports the first window panel alone again', one.length === 1 && one[0].windowId === window, one);
    await checkpoint('second-window-closed');

    await sp.close({ windowId: window });
    step('no panel is left open', await until(() => opened.length === closed.length, 10), seen());
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
