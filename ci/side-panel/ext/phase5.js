// The tabs-and-windows checks of the side panel work. They run once per
// browser launch, when ci/side-panel/phase5.py is listening and names a
// suite; with nobody listening this file only registers listeners.
(function () {
  'use strict';
  var SERVER = 'http://127.0.0.1:47615';
  var events = [];
  var navigations = [];

  function note(name) {
    return function () {
      events.push({ name: name, args: Array.prototype.slice.call(arguments) });
    };
  }
  // Listeners go on at the top level, as an extension's do.
  ['onCreated', 'onUpdated', 'onActivated', 'onMoved', 'onRemoved', 'onReplaced', 'onHighlighted'].forEach(function (e) {
    chrome.tabs[e].addListener(note('tabs.' + e));
  });
  ['onCreated', 'onRemoved', 'onFocusChanged'].forEach(function (e) {
    chrome.windows[e].addListener(note('windows.' + e));
  });
  chrome.webNavigation.onCompleted.addListener(function (d) {
    if (d.frameId === 0) { navigations.push({ tabId: d.tabId, url: d.url }); }
  });
  chrome.runtime.onStartup.addListener(function () {});
  chrome.runtime.onMessage.addListener(function (message, sender, respond) {
    if (message && message.p5page) {
      post('page', { result: message.p5page, senderTab: sender.tab });
      respond('ok');
    }
  });

  function post(kind, data) {
    return fetch(SERVER + '/r', { method: 'POST', body: JSON.stringify({ kind: kind, at: Date.now(), data: data }) }).catch(function () {});
  }
  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
  async function until(probe, ms) {
    var end = Date.now() + ms;
    for (;;) {
      var value = await probe();
      if (value) { return value; }
      if (Date.now() > end) { throw new Error('timed out after ' + ms + ' ms'); }
      await sleep(200);
    }
  }
  function slim(t) {
    return t && { id: t.id, index: t.index, windowId: t.windowId, groupId: t.groupId, active: t.active, status: t.status, discarded: t.discarded, url: t.url, title: t.title };
  }
  function message(e) { return String((e && e.message) || e); }
  async function step(name, body) {
    var out;
    try { out = { step: name, ok: true, value: await body() }; }
    catch (e) { out = { step: name, ok: false, error: message(e) }; }
    await post('step', out);
    if (!out.ok) { throw new Error(name + ': ' + out.error); }
    return out.value;
  }

  async function main() {
    var tabs = await until(async function () {
      var all = await chrome.tabs.query({});
      return all.length >= 2 ? all : null;
    }, 40000);
    await step('query-all', async function () { return tabs.map(slim); });
    await step('query-active-current', async function () {
      return (await chrome.tabs.query({ active: true, currentWindow: true })).map(slim);
    });
    // A web page (tabs[0] is https://example.com/) has no bridge in its main
    // world; the id lets the driver ask the relay page what it holds.
    await step('web-page', async function () {
      var page = (await chrome.scripting.executeScript({
        target: { tabId: tabs[0].id },
        world: 'MAIN',
        func: function () {
          return { call: typeof self.__bridgeCall, send: typeof self.__nautilusBridgeSend, bridge: typeof self.__nautilusBridge };
        }
      }))[0].result;
      return { page: page, id: chrome.runtime.id, url: tabs[0].url };
    });
    await step('windows', async function () {
      return {
        all: await chrome.windows.getAll({ populate: true }),
        current: (await chrome.windows.getCurrent()).id,
        lastFocused: (await chrome.windows.getLastFocused()).id
      };
    });
    events.length = 0;
    var created = await step('create', async function () {
      var tab = await chrome.tabs.create({ url: 'https://example.net/?p5', active: false });
      var all = await chrome.tabs.query({});
      return { tab: slim(tab), got: slim(await chrome.tabs.get(tab.id)), count: all.length, last: slim(all[all.length - 1]) };
    });
    var id = created.tab.id;
    await until(async function () { return (await chrome.tabs.get(id)).status === 'complete'; }, 30000);
    await step('activate', async function () {
      var tab = await chrome.tabs.update(id, { active: true });
      return { tab: slim(tab), active: (await chrome.tabs.query({ active: true, currentWindow: true })).map(slim) };
    });
    await step('move', async function () {
      var tab = await chrome.tabs.move(id, { index: 0 });
      return { tab: slim(tab), order: (await chrome.tabs.query({ windowId: tab.windowId })).map(function (t) { return t.id; }) };
    });
    await step('by-id', async function () {
      var out = {};
      out.executeScript = (await chrome.scripting.executeScript({
        target: { tabId: id },
        func: function () {
          chrome.runtime.onMessage.addListener(function (m, s, r) { if (m === 'p5-ping') { r('pong ' + location.host); } });
          return document.title;
        }
      }))[0].result;
      out.sendMessage = await chrome.tabs.sendMessage(id, 'p5-ping');
      navigations.length = 0;
      out.update = slim(await chrome.tabs.update(id, { url: 'https://example.org/?p5-nav' }));
      out.navigation = await until(async function () {
        return navigations.find(function (n) { return n.tabId === id && n.url.indexOf('p5-nav') !== -1; }) || null;
      }, 30000);
      await chrome.debugger.attach({ tabId: id }, '1.3');
      try {
        out.evaluate = (await chrome.debugger.sendCommand({ tabId: id }, 'Runtime.evaluate', { expression: '6 * 7', returnByValue: true })).result.value;
        out.screenshot = (await chrome.debugger.sendCommand({ tabId: id }, 'Page.captureScreenshot', { format: 'png' })).data.length;
        await chrome.debugger.sendCommand({ tabId: id }, 'Input.dispatchMouseEvent', { type: 'mouseMoved', x: 40, y: 40 });
        await chrome.debugger.sendCommand({ tabId: id }, 'Input.dispatchKeyEvent', { type: 'keyDown', key: 'a', code: 'KeyA', windowsVirtualKeyCode: 65 });
        await chrome.debugger.sendCommand({ tabId: id }, 'Input.dispatchKeyEvent', { type: 'keyUp', key: 'a', code: 'KeyA', windowsVirtualKeyCode: 65 });
        out.input = 'dispatched';
      } finally {
        await chrome.debugger.detach({ tabId: id });
      }
      out.capture = (await chrome.tabs.captureVisibleTab()).slice(0, 23);
      return out;
    });
    await step('content-silent', async function () {
      // A content script's message (sent from the page's isolated world, as a
      // content script sends it) that the worker's listener does not answer. It
      // must settle at once: closed by Chromium, not held to a garbage collection.
      var result = await chrome.scripting.executeScript({
        target: { tabId: id },
        world: 'ISOLATED',
        func: function () {
          return new Promise(function (resolve) {
            var started = performance.now();
            var timer = setTimeout(function () {
              resolve({ outcome: 'hung', ms: Math.round(performance.now() - started), detail: null });
            }, 5000);
            Promise.resolve(chrome.runtime.sendMessage({ n3: 'silent' })).then(function (answer) {
              clearTimeout(timer);
              resolve({ outcome: 'answered', ms: Math.round(performance.now() - started), detail: answer === undefined ? null : answer });
            }, function (error) {
              clearTimeout(timer);
              resolve({ outcome: 'closed', ms: Math.round(performance.now() - started), detail: String((error && error.message) || error) });
            });
          });
        }
      });
      return result[0].result;
    });
    await step('fail-closed', async function () {
      var out = {};
      out.group = await chrome.tabGroups.move(1, { index: 0 }).then(function () { return 'resolved'; }, message);
      out.discard = await chrome.tabs.discard(id).then(function (v) { return v === undefined ? 'undefined' : 'a tab'; }, message);
      out.unknown = await chrome.tabs.remove(999999).then(function () { return 'resolved'; }, message);
      out.callback = await new Promise(function (resolve) {
        chrome.tabs.get(999999, function () { resolve(chrome.runtime.lastError && chrome.runtime.lastError.message); });
      });
      var copy = await chrome.tabs.duplicate(id);
      out.duplicate = slim(copy);
      out.countAfterDuplicate = (await chrome.tabs.query({})).length;
      await chrome.tabs.remove(copy.id);
      out.countAfterRemove = (await chrome.tabs.query({})).length;
      out.getRemoved = await chrome.tabs.get(copy.id).then(function () { return 'resolved'; }, message);
      // `browser`, the second global beside `chrome`: its bridged
      // namespaces are chrome's own objects, so a call through it is the
      // bridge's too (docs/upstream-issues.md, C19).
      var b = self.browser;
      out.browser = !b ? null : {
        tabs: b.tabs === chrome.tabs,
        windows: b.windows === chrome.windows,
        tabGroups: b.tabGroups === chrome.tabGroups,
        query: await b.tabs.query({}).then(function (tabs) { return tabs.length; }, message)
      };
      return out;
    });
    await step('remove', async function () {
      await chrome.tabs.remove(id);
      return { get: await chrome.tabs.get(id).then(function () { return 'resolved'; }, message), count: (await chrome.tabs.query({})).length };
    });
    await step('page', async function () {
      var tab = await chrome.tabs.create({ url: 'p5page.html' });
      await sleep(3000);
      return slim(tab);
    });
    await step('windows-create', async function () {
      var before = (await chrome.windows.getAll()).length;
      var made = await chrome.windows.create({ url: 'https://example.com/?p5-window' });
      await post('mark', 'window-open');
      var during = (await chrome.windows.getAll()).length;
      await sleep(1500);
      await chrome.windows.remove(made.id);
      var after = await until(async function () {
        var n = (await chrome.windows.getAll()).length;
        return n === before ? n : null;
      }, 15000);
      await sleep(1000);
      await post('mark', 'window-closed');
      return { id: made.id, tabs: (made.tabs || []).map(slim), before: before, during: during, after: after };
    });
    await sleep(1000);
    await step('events', async function () { return events.slice(); });
    await post('done', 'main');
  }

  async function restore() {
    var tabs = await until(async function () {
      var all = await chrome.tabs.query({});
      return all.some(function (t) { return t.discarded; }) ? all : null;
    }, 40000);
    await step('restore-list', async function () { return tabs.map(slim); });
    var deferred = tabs.filter(function (t) { return t.discarded; })[0];
    events.length = 0;
    await step('restore-show', async function () {
      var shown = await chrome.tabs.update(deferred.id, { active: true });
      var replaced = await until(async function () {
        return events.find(function (e) { return e.name === 'tabs.onReplaced' && e.args[1] === deferred.id; }) || null;
      }, 20000);
      var byOld = await chrome.tabs.get(deferred.id);
      var href = (await chrome.scripting.executeScript({ target: { tabId: shown.id }, func: function () { return location.href; } }))[0].result;
      return { was: slim(deferred), shown: slim(shown), replaced: replaced.args, byOld: slim(byOld), href: href };
    });
    await post('done', 'restore');
  }

  fetch(SERVER + '/cfg').then(function (r) { return r.json(); }).then(function (cfg) {
    var run = cfg.suite === 'main' ? main : cfg.suite === 'restore' ? restore : null;
    if (run) { run().catch(function (e) { post('done', 'failed: ' + message(e)); }); }
  }).catch(function () {});
})();
