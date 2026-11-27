// What this context's `browser` global is beside `chrome` (Chromium 156
// gives an extension context both; docs/upstream-issues.md, C19). Loaded by
// the worker (importScripts), the panel pages (a script tag) and the content
// script (the manifest), so the three report the same facts. Nothing here
// changes anything: it reads properties, and `browserQuery` makes one
// `tabs.query` through each global.
(function (root) {
  'use strict';
  const NAMESPACES = ['tabs', 'windows', 'tabGroups', 'commands', 'sidePanel', 'debugger'];

  function descriptor(object, name) {
    const d = object ? Object.getOwnPropertyDescriptor(object, name) : undefined;
    if (!d) return null;
    return {
      kind: 'value' in d ? 'data' : 'accessor',
      configurable: d.configurable === true,
      writable: d.writable === true,
      enumerable: d.enumerable === true,
    };
  }

  root.browserFacts = function () {
    const c = root.chrome;
    const b = root.browser;
    const facts = {
      browser: typeof b,
      browserIsChrome: b === c,
      bridge: typeof root.__nautilusBridge,
      namespaces: {},
      runtime: null,
    };
    if (!b || typeof b !== 'object' || !c) return facts;
    for (const name of NAMESPACES) {
      facts.namespaces[name] = {
        chrome: typeof c[name],
        browser: typeof b[name],
        same: b[name] === c[name],
        onBrowser: descriptor(b, name),
        onChrome: descriptor(c, name),
      };
    }
    const cr = c.runtime;
    const br = b.runtime;
    const member = (name) => (cr && br ? { chrome: typeof cr[name], browser: typeof br[name], same: cr[name] === br[name] } : null);
    facts.runtime = {
      same: cr === br,
      onBrowser: descriptor(b, 'runtime'),
      getContexts: member('getContexts'),
      onConnect: member('onConnect'),
      onMessage: member('onMessage'),
      sendMessage: member('sendMessage'),
      connect: member('connect'),
      onConnectAddListener:
        cr && br && cr.onConnect && br.onConnect ? cr.onConnect.addListener === br.onConnect.addListener : null,
    };
    const cd = c.debugger;
    const bd = b.debugger;
    facts.debuggerAttachSame = cd && bd ? cd.attach === bd.attach : null;
    return facts;
  };

  // One tabs.query through each global: the tab ids each answers with, or
  // the error. The bridge answers with the strip's tabs (an index from 0, a
  // window id from 1); Chromium's own knows no tab of this browser.
  root.browserQuery = async function () {
    const ask = async (api) => {
      if (!api || !api.tabs || typeof api.tabs.query !== 'function') return 'no tabs.query';
      try {
        const tabs = await api.tabs.query({});
        return tabs.map((t) => [t.id, t.index, t.windowId]);
      } catch (error) {
        return 'rejected: ' + String((error && error.message) || error);
      }
    };
    return { chrome: await ask(root.chrome), browser: await ask(root.browser) };
  };
})(self);
