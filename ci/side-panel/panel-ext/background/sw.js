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
  importScripts('/browser-facts.js');
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

  // This context's `browser` beside `chrome` (browser-facts.js), for the
  // harness to judge.
  const reportBrowser = async (context) =>
    post('browser-facts', { context, facts: self.browserFacts(), query: await self.browserQuery() });

  const opened = [];
  const closed = [];
  const removed = {}; // tab id -> when tabs.onRemoved fired (ms)

  chrome.runtime.onMessage.addListener((message, sender) => {
    if (message && message.report) {
      // A content script's report, with the tab the worker sees as its
      // sender: a content script's sender keeps its tab, with the strip's
      // fields.
      const tab = sender.tab ? { id: sender.tab.id, index: sender.tab.index, windowId: sender.tab.windowId } : null;
      post(message.report.kind, Object.assign({ senderTab: tab }, message.report.data));
    }
    // A panel page says who it is: what the worker sees as its sender.
    if (message && message.whoami) {
      post('sender', {
        page: message.whoami,
        late: message.late === true,
        hasTab: 'tab' in sender,
        tab: sender.tab || null,
        url: sender.url || null,
        frameId: sender.frameId,
      });
    }
  });
  chrome.tabs.onRemoved.addListener((id) => {
    removed[id] = Date.now();
  });

  // The panel page's debugger check (panel.js, debuggerCheck). While the
  // page holds a session on a tab, what this worker can do with that tab:
  // a command through the page's session, and an attach of its own. Then
  // the page waits here (a checkpoint) while the harness reads the log.
  const debuggerSteps = { held: null, done: null };
  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (message && typeof message.debuggerProbe === 'number') {
      const tabId = message.debuggerProbe;
      const said = (promise) => promise.then(() => 'ok', (error) => 'rejected: ' + error.message);
      (async () => {
        const command = await said(chrome.debugger.sendCommand({ tabId }, 'Runtime.evaluate', { expression: '1', returnByValue: true }));
        const attach = await said(chrome.debugger.attach({ tabId }, '1.3'));
        // A second session of this worker's own would be its own to end.
        const detach = attach === 'ok' ? await said(chrome.debugger.detach({ tabId })) : 'not tried';
        sendResponse({ command, attach, detach });
      })();
      return true;
    }
    if (message && typeof message.debuggerHeld === 'number') {
      checkpoint('debugger-held').then(() => sendResponse(true));
      return true;
    }
    if (message && typeof message.debuggerDone === 'number' && debuggerSteps.done) debuggerSteps.done();
    return undefined;
  });

  async function panelSuite() {
    const sp = chrome.sidePanel;
    const self_ = chrome.runtime.id;
    // A worker that asks as it is installed asks before the engine has
    // published its first strip; its call is held until the strip is out
    // and the tabs have Chromium's ids (two seconds at most), so the first
    // answer has both tabs. The loop stays for the bound's sake.
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
    await reportBrowser('worker');
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
    await reportBrowser('worker, after another extension was installed');
    step(
      "after another extension is installed chrome.sidePanel is still the engine's in the worker",
      Object.isFrozen(chrome.sidePanel) && still === 'At least one of `tabId` and `windowId` must be provided',
      still
    );
    chrome.runtime.sendMessage({ command: 'describe' }).catch(() => {});
    await checkpoint('bystander-page');

    // The panel's page attaches chrome.debugger to the tab that is behind
    // the active one, and commands it (panel.js). The harness judges the
    // timings and the engine's log.
    const finished = new Promise((resolve) => { debuggerSteps.done = resolve; });
    chrome.runtime.sendMessage({ command: 'debugger-check', tabId: other.id }).catch(() => {});
    await Promise.race([finished, sleep(40000)]);
    await checkpoint('debugger-done');
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

  // Tab groups and a keyboard command over the bridge, with the calls and
  // the argument shapes an extension makes when it opens its side panel:
  // tabs.group, then tabGroups.update with a title and a colour, get,
  // query, and tabs.ungroup. The harness captures the window while the
  // tab is grouped and sends the command through the test control.
  async function groupsSuite() {
    let tabs = await chrome.tabs.query({});
    for (let i = 0; i < 100 && tabs.length < 2; i += 1) {
      await sleep(200);
      tabs = await chrome.tabs.query({});
    }
    const active = tabs.find((t) => t.active) || tabs[0];
    const window = active.windowId;
    await post('tabs', { id: chrome.runtime.id, active: active.id, window, activeUrl: active.url });
    const tg = chrome.tabGroups;
    step('two tabs', tabs.length === 2, tabs.map((t) => t.url));
    step(
      'the bridge owns chrome.tabGroups and chrome.commands',
      !!self.__nautilusBridge &&
        ['tabs', 'tabGroups', 'commands'].every((name) => self.__nautilusBridge.installed.includes(name)),
      self.__nautilusBridge && self.__nautilusBridge.installed
    );
    step(
      'the constants an extension reads are there',
      tg.TAB_GROUP_ID_NONE === -1 && tg.Color.ORANGE === 'orange' && tg.Color.GREY === 'grey',
      { none: tg.TAB_GROUP_ID_NONE, color: tg.Color }
    );
    const events = [];
    tg.onCreated.addListener((group) => events.push(['tabGroups.onCreated', group]));
    tg.onUpdated.addListener((group) => events.push(['tabGroups.onUpdated', group]));
    tg.onRemoved.addListener((group) => events.push(['tabGroups.onRemoved', group]));
    chrome.tabs.onUpdated.addListener((id, info) => {
      if ('groupId' in info) events.push(['tabs.onUpdated', { id, groupId: info.groupId }]);
    });
    // chrome.commands is not pinned (docs/upstream-issues.md, C19): it is
    // Chromium's own object, left under the property Chromium made, with
    // every member replaced by the bridge's. Read before anything is asked
    // of it.
    const member = (name) => {
      const d = Object.getOwnPropertyDescriptor(chrome.commands, name) || {};
      return { type: typeof d.value, configurable: d.configurable === true, writable: d.writable === true };
    };
    const property = (holder) => {
      const d = Object.getOwnPropertyDescriptor(holder, 'commands') || {};
      return { kind: 'value' in d ? 'data' : 'get' in d ? 'accessor' : 'none', configurable: d.configurable === true };
    };
    const commandsAre = () => ({
      onChrome: property(chrome),
      onBrowser: typeof browser === 'object' && browser ? property(browser) : null,
      same: typeof browser === 'object' && !!browser && browser.commands === chrome.commands,
      getAll: member('getAll'),
      onCommand: member('onCommand'),
      keys: Object.keys(chrome.commands).sort(),
      // The bridge's events are plain objects of its own; Chromium's are not.
      bridgeEvent: Object.getPrototypeOf(chrome.commands.onCommand) !== Object.prototype &&
        typeof chrome.commands.onCommand.dispatch === 'function' && Array.isArray(chrome.commands.onCommand.listeners),
      native: /\[native code\]/.test(Function.prototype.toString.call(chrome.commands.getAll)),
    });
    const unpinned = (facts) =>
      facts.onChrome.configurable && facts.onBrowser && facts.onBrowser.configurable && facts.same &&
      facts.getAll.type === 'function' && !facts.getAll.configurable && !facts.getAll.writable &&
      facts.onCommand.type === 'object' && !facts.onCommand.configurable && !facts.onCommand.writable &&
      facts.bridgeEvent && !facts.native;
    const held = { object: chrome.commands, getAll: chrome.commands.getAll, onCommand: chrome.commands.onCommand };
    step(
      "chrome.commands is Chromium's own object with the bridge's members, not pinned, and browser.commands is the same object",
      unpinned(commandsAre()),
      commandsAre()
    );
    const commands = [];
    chrome.commands.onCommand.addListener((command, tab) => commands.push({ command, tab: tab && tab.id, window: tab && tab.windowId }));
    const clicks = [];
    const clicked = [];
    chrome.action.onClicked.addListener((tab) => {
      clicks.push(tab && tab.id);
      clicked.push(tab);
    });
    const names = () => events.map((e) => e[0]);
    const shape = (title) => ({ id: null, collapsed: false, color: 'grey', title, windowId: window, shared: false });
    const is = (group, id, title) => same(group, { ...shape(title), id });

    // `browser` beside `chrome` (docs/upstream-issues.md, C19): the
    // harness judges that its namespaces are the bridge's too.
    await reportBrowser('worker');
    step('the tab starts in no group', active.groupId === tg.TAB_GROUP_ID_NONE, active.groupId);
    // What a panel opening does first.
    const id = await chrome.tabs.group({ tabIds: [active.id], createProperties: { windowId: window } }).catch((error) => error.message);
    step('tabs.group({tabIds, createProperties: {windowId}}) resolves a group id', Number.isInteger(id) && id > 0, id);
    if (!Number.isInteger(id)) return;
    // Straight after, with no wait: the group is there.
    const made = await tg.get(id).catch((error) => error.message);
    step('tabGroups.get(id) straight after finds the group, named for the extension', is(made, id, 'Nautilus Panel Test Session'), made);
    const updated = await tg
      .update(id, { title: 'Phase 7 check', color: tg.Color.ORANGE, collapsed: false })
      .catch((error) => error.message);
    step('tabGroups.update(id, {title, color, collapsed}) resolves the group, grey', is(updated, id, 'Phase 7 check'), updated);
    const got = await tg.get(id).catch((error) => error.message);
    step('tabGroups.get(id) after the update has the title', is(got, id, 'Phase 7 check'), got);
    const all = await tg.query({}).catch((error) => error.message);
    step('tabGroups.query({}) lists the one group', Array.isArray(all) && all.length === 1 && is(all[0], id, 'Phase 7 check'), all);
    const titled = await tg.query({ title: 'Phase*', windowId: chrome.windows.WINDOW_ID_CURRENT }).catch((error) => error.message);
    step('tabGroups.query({title, windowId}) finds it by pattern', Array.isArray(titled) && titled.length === 1, titled);
    const inGroup = await chrome.tabs.query({ groupId: id });
    step(
      "tabs.query({groupId}) lists the tab, whose groupId is the group's",
      inGroup.length === 1 && inGroup[0].id === active.id && inGroup[0].groupId === id,
      inGroup.map((t) => [t.id, t.groupId])
    );
    step(
      'tabGroups.onCreated fired before the tab was said to be in the group, then onUpdated',
      await until(() => names().includes('tabGroups.onUpdated'), 10) &&
        same(names().slice(0, 3), ['tabGroups.onCreated', 'tabs.onUpdated', 'tabGroups.onUpdated']) &&
        events[1][1].id === active.id &&
        events[1][1].groupId === id &&
        is(events[2][1], id, 'Phase 7 check'),
      events
    );
    const moved = await rejection(tg.move(id, { index: 0 }));
    step('tabGroups.move is refused by the bridge', moved === 'tabGroups.move is not supported.', moved);

    // The harness captures the window here, with the folder in the strip,
    // and sends the command and then the action command.
    const declared = await chrome.commands.getAll().catch((error) => error.message);
    step(
      'commands.getAll() lists the declared command with its shortcut',
      Array.isArray(declared) &&
        declared.length === 1 &&
        declared[0].name === 'probe-command' &&
        declared[0].description === 'The command panel_checks.py groups sends' &&
        typeof declared[0].shortcut === 'string' &&
        declared[0].shortcut.endsWith('9'),
      declared
    );
    await checkpoint('grouped');
    step(
      "the declared command reached commands.onCommand once, with a tab of the window as the strip has it",
      await until(() => commands.length >= 1, 10) &&
        commands.length === 1 &&
        commands[0].command === 'probe-command' &&
        tabs.some((t) => t.id === commands[0].tab) &&
        commands[0].window === window,
      commands
    );
    // The control commands for the tab focused last, which is the active
    // tab when the window has had the keyboard.
    post('note', `the command's tab is ${commands[0] && commands[0].tab}; the active tab is ${active.id}`);
    await checkpoint('action-command');
    step(
      '_execute_action is the click: action.onClicked ran, and no onCommand',
      await until(() => clicks.length >= 1, 10) && clicks.length === 1 && tabs.some((t) => t.id === clicks[0]) && commands.length === 1,
      { clicks, commands }
    );

    // The click's tab is the tab as the strip has it: the window id and the
    // index an extension passes on to sidePanel.open and tabs.group, and
    // the group the tab is in now.
    const hit = clicked[0] || {};
    const known = (await chrome.tabs.query({})).find((t) => t.id === hit.id) || {};
    step(
      "action.onClicked's tab has the strip's windowId, index, active and groupId",
      hit.windowId === window && hit.windowId >= 1 && hit.index === known.index && hit.index >= 0 &&
        hit.active === known.active && hit.groupId === known.groupId && typeof hit.url === 'string',
      { clicked: hit, strip: { id: known.id, index: known.index, windowId: known.windowId, active: known.active, groupId: known.groupId } }
    );
    const reopened = await rejection(chrome.sidePanel.getOptions({ tabId: hit.id }));
    step('and its id and windowId are ones the bridge takes', reopened === null && (await chrome.windows.get(hit.windowId).then((w) => w.id === window, () => false)), reopened);

    const ungrouped = await chrome.tabs.ungroup([active.id]).then((value) => (value === undefined ? 'undefined' : value), (error) => error.message);
    step('tabs.ungroup([tab]) resolves undefined', ungrouped === 'undefined', ungrouped);
    const gone = await rejection(tg.get(id));
    step('the emptied group is gone', gone === `No group with id: ${id}.`, gone);
    const after = await chrome.tabs.get(active.id);
    step('the tab is in no group again', after.groupId === tg.TAB_GROUP_ID_NONE, after.groupId);
    step(
      'the tab left the group before tabGroups.onRemoved fired',
      await until(() => names().includes('tabGroups.onRemoved'), 10) &&
        same(names().slice(3), ['tabs.onUpdated', 'tabGroups.onRemoved']) &&
        events[3][1].groupId === -1 &&
        is(events[4][1], id, 'Phase 7 check'),
      events.slice(3)
    );
    step('tabGroups.query({}) is empty again', same(await tg.query({}), []));
    await checkpoint('ungrouped');

    // A tab joins a group that exists (what an extension does for a tab its
    // assistant opens), and each read is made straight after the call
    // before it resolved, with no wait: it must see that call's change.
    const other = tabs.find((t) => t.id !== active.id);
    const message = (error) => error.message;
    const second = await chrome.tabs.group({ tabIds: [active.id] }).catch(message);
    const joined = await chrome.tabs.group({ tabIds: other.id, groupId: second }).catch(message);
    const both = await chrome.tabs.query({ groupId: second }).catch(message);
    step(
      'tabs.query({groupId}) straight after tabs.group into an existing group lists both tabs',
      Number.isInteger(second) && joined === second && Array.isArray(both) && same(both.map((t) => t.id).sort(), [active.id, other.id].sort()),
      { second, joined, both: Array.isArray(both) ? both.map((t) => [t.id, t.groupId]) : both }
    );
    const renamed = await tg.update(second, { title: 'Second' }).catch(message);
    const read = await tg.get(second).catch(message);
    step(
      'tabGroups.get straight after tabGroups.update has the new title',
      is(renamed, second, 'Second') && is(read, second, 'Second'),
      { renamed, read }
    );
    await checkpoint('joined');
    const out = await chrome.tabs.ungroup(other.id).then(() => 'undefined', message);
    const left = await chrome.tabs.query({ groupId: second }).catch(message);
    const loose = await chrome.tabs.get(other.id).catch(message);
    step(
      'tabs.query({groupId}) and tabs.get straight after tabs.ungroup show the tab out of the group',
      out === 'undefined' && Array.isArray(left) && same(left.map((t) => t.id), [active.id]) && loose.groupId === tg.TAB_GROUP_ID_NONE,
      { out, left: Array.isArray(left) ? left.map((t) => [t.id, t.groupId]) : left, loose: loose && loose.groupId }
    );
    await chrome.tabs.ungroup([active.id]).catch(() => {});

    // Another extension is installed while this worker is alive (the
    // harness loads one at this checkpoint): Chromium then defines every
    // API on `chrome` and `browser` again in this context. `commands` is
    // not pinned, so its property is defined again; it must answer the
    // same object, with the bridge's members still in it, and a shortcut
    // must still reach the listener added before.
    await checkpoint('bystander');
    const still = await chrome.commands.getAll().catch(message);
    step(
      "after another extension is installed chrome.commands is the same object, with the bridge's getAll and onCommand, and still not pinned",
      chrome.commands === held.object && chrome.commands.getAll === held.getAll && chrome.commands.onCommand === held.onCommand &&
        unpinned(commandsAre()) && Array.isArray(still) && still.length === 1 && still[0].name === 'probe-command',
      { facts: commandsAre(), sameObject: chrome.commands === held.object, getAll: still }
    );
    await reportBrowser('worker, after another extension was installed');
    await checkpoint('command-again');
    step(
      'and the declared command still reaches commands.onCommand',
      await until(() => commands.length >= 2, 10) && commands.length === 2 && commands[1].command === 'probe-command',
      commands
    );

    await grantCheck();
    // Awaited: the suite's `done` follows, and must not overtake it.
    await step('no group is left', same(await tg.query({}), []));
  }

  // docs/upstream-issues.md, C19: an API granted to this live worker
  // through chrome.permissions.request. `cookies` sorts between `commands`
  // (not pinned) and `sidePanel` (the first pinned name of this extension),
  // `storage` after it. Neither has a permission warning, so Chromium asks
  // the user nothing. The request needs a user gesture: asked first from
  // here, where there is none, then from grant.html in the handler of a
  // click this worker sends through chrome.debugger. What happened is
  // posted for the harness, which judges it.
  const grant = { ready: null, answer: null };
  chrome.runtime.onMessage.addListener((message) => {
    if (message && message.grantReady && grant.ready) grant.ready();
    if (message && message.grantAnswer && grant.answer) grant.answer(message.grantAnswer);
  });
  async function grantCheck() {
    const what = { permissions: ['cookies', 'storage'] };
    const has = () => ({ cookies: typeof chrome.cookies, storage: typeof chrome.storage });
    const said = (promise) => promise.then((value) => ({ value }), (error) => ({ error: String((error && error.message) || error) }));
    const out = { before: has(), held: await said(chrome.permissions.contains(what)) };
    out.fromWorker = await said(chrome.permissions.request(what));
    out.afterWorker = has();
    if (!(out.fromWorker.value === true)) {
      const ready = new Promise((resolve) => { grant.ready = resolve; });
      const answer = new Promise((resolve) => { grant.answer = resolve; });
      const tab = await chrome.tabs.create({ url: chrome.runtime.getURL('grant.html'), active: true });
      out.page = { tab: tab.id, ready: await Promise.race([ready.then(() => true), sleep(10000).then(() => false)]) };
      const target = { tabId: tab.id };
      out.page.attach = await said(chrome.debugger.attach(target, '1.3'));
      if (!out.page.attach.error) {
        const click = (type) => chrome.debugger.sendCommand(target, 'Input.dispatchMouseEvent', { type, x: 60, y: 60, button: 'left', buttons: type === 'mousePressed' ? 1 : 0, clickCount: 1 });
        out.page.click = await said(click('mousePressed').then(() => click('mouseReleased')));
        out.page.answer = await Promise.race([answer, sleep(10000).then(() => 'no answer in 10 s')]);
        await said(chrome.debugger.detach(target));
      }
      await chrome.tabs.remove(tab.id).catch(() => {});
    }
    // The renderer hears of a grant after the browser has made it.
    await sleep(2000);
    out.contains = await said(chrome.permissions.contains(what));
    out.after = has();
    await post('grant', out);
  }

  const suites = { panel: panelSuite, behavior: behaviorSuite, groups: groupsSuite };

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
