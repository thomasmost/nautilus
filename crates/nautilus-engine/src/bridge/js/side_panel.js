// chrome.sidePanel, answered by the engine, and the side panel's part of
// chrome.runtime.getContexts. Nothing here reaches Chromium's own
// sidePanel implementation, which knows no window of this browser.
//
// It needs the bridge's core (js/core.js): `__nautilusBridge.call(namespace,
// method, args)`, a promise of the engine's answer that rejects with an
// Error carrying the engine's message, and `__nautilusBridge.BridgeEvent`,
// which announces its first listener to the engine (`bridge.listen`) and
// is dispatched by the core's one event handler.
//
// The namespace is written by hand and not with `define`: its functions
// check their arguments as Chromium's do (a TypeError naming the function),
// and its object is frozen whole (docs/bridge-protocol.md, "sidePanel is
// written by hand").
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  var chrome = root.chrome;
  if (!bridge || !chrome || bridge.installed.indexOf('runtime.getContexts') !== -1) { return; }
  bridge.installed.push('runtime.getContexts');

  // The members the engine answers, read by a test in bridge/mod.rs
  // (keep each list on one line): bridged: ['open', 'close', 'getOptions', 'setOptions', 'getPanelBehavior', 'setPanelBehavior', 'getLayout'], events: ['onOpened', 'onClosed'].

  // `promise`, or a rejection once `ms` have passed without it settling:
  // nothing below waits on the engine for longer than it says.
  function bounded(promise, ms) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('The engine did not answer in time.')), ms);
      promise.then(
        (value) => { clearTimeout(timer); resolve(value); },
        (error) => { clearTimeout(timer); reject(error); }
      );
    });
  }

  // An extension without the "sidePanel" permission has no
  // chrome.sidePanel, here as in Chromium. It gets nothing of this file but the relay page
  // left out of its getContexts: its `window.close` and everything else
  // stay Chromium's own, and the engine is never asked.
  const native = chrome.sidePanel;
  if (native) {
    const call = (method, args) => bridge.call('sidePanel', method, args);

    const fail = (method, why) => {
      throw new TypeError(`Error in invocation of sidePanel.${method}: ${why}`);
    };

    // The options object of a call, checked field by field against
    // `fields` (name -> typeof, with 'integer' for whole numbers).
    function checked(method, options, fields, required) {
      if (options === undefined || options === null) {
        if (required) fail(method, 'No matching signature.');
        return {};
      }
      if (typeof options !== 'object' || Array.isArray(options)) {
        fail(method, 'No matching signature.');
      }
      const out = {};
      for (const [name, value] of Object.entries(options)) {
        const type = fields[name];
        if (!type) fail(method, `Unexpected property: '${name}'.`);
        if (value === undefined) continue;
        const fits = type === 'integer' ? Number.isInteger(value) : typeof value === type;
        if (!fits) fail(method, `Error at property '${name}': Invalid type.`);
        out[name] = value;
      }
      return out;
    }

    // Promise for callers without a callback; with one, the callback
    // gets the value, and an error is logged as Chromium logs an
    // unchecked lastError.
    function callbackable(fn) {
      return function (...args) {
        const callback = typeof args[args.length - 1] === 'function' ? args.pop() : null;
        const promise = fn(...args);
        if (!callback) return promise;
        promise.then(
          (value) => callback(value),
          (error) => {
            console.error(`Unchecked runtime.lastError: ${error.message}`);
            callback();
          }
        );
        return undefined;
      };
    }

    // The engine answers `null` where Chromium answers nothing.
    const nothing = () => undefined;
    const place = { tabId: 'integer', windowId: 'integer' };

    const api = {
      Side: { LEFT: 'left', RIGHT: 'right' },
      getOptions: callbackable(async (options) =>
        call('getOptions', [checked('getOptions', options, { tabId: 'integer' }, true)])
      ),
      setOptions: callbackable(async (options) => {
        const fields = { tabId: 'integer', path: 'string', enabled: 'boolean' };
        return call('setOptions', [checked('setOptions', options, fields, true)]).then(nothing);
      }),
      getPanelBehavior: callbackable(async () => call('getPanelBehavior', [])),
      setPanelBehavior: callbackable(async (behavior) => {
        const fields = { openPanelOnActionClick: 'boolean' };
        return call('setPanelBehavior', [checked('setPanelBehavior', behavior, fields, true)]).then(nothing);
      }),
      // No user gesture is asked for: a call the bridge carries has
      // none to show (docs/upstream-issues.md, "`sidePanel.open`
      // cannot be held to a user gesture").
      open: callbackable(async (options) =>
        call('open', [checked('open', options, place, true)]).then(nothing)
      ),
      close: callbackable(async (options) =>
        call('close', [checked('close', options, place, true)]).then(nothing)
      ),
      getLayout: callbackable(async () => call('getLayout', [])),
      onOpened: new bridge.BridgeEvent('sidePanel.onOpened'),
      onClosed: new bridge.BridgeEvent('sidePanel.onClosed'),
    };

    // Put in place by the core's `pin`, as `define` puts a namespace: on
    // `chrome` and on `browser`, not configurable (Chromium defines every
    // API again in each live context when the set of extensions changes,
    // and a configurable property is then its own once more, for good).
    // Where a property cannot be redefined, Chromium's own object stays
    // with every member of it replaced, and a function of Chromium's that
    // is not one of the engine's rejects; where that cannot be done either,
    // the namespace is refused.
    if (bridge.pin('sidePanel', Object.freeze(api))) { bridge.installed.push('sidePanel'); }
  }

  // chrome.runtime.getContexts. The bridge's own relay page, which Chromium
  // lists as a tab of the extension's, is the browser's and is always left
  // out. For an extension without a side panel that is all: Chromium is
  // asked with the caller's filter and the engine not at all. For one with
  // a side panel Chromium is asked for everything and the engine applies
  // the filter, since an open panel's entry changes type on the way
  // (SIDE_PANEL, in its window). The engine is asked twice: for its open
  // panels at once, as the call is made and before Chromium is asked
  // (`openPanels`), and then, with Chromium's answer and those panels, for
  // the contexts. So the answer is of the moment of the call: an extension
  // that asks, opens its panel and then reads the answer does not find the
  // panel it opened afterwards in it. An engine that rejects either
  // question, or has not answered one within GET_CONTEXTS_WAIT, is not
  // waited for, and the answer is then Chromium's own with the filter
  // applied here (`passes`, the same rule as `side_panel::passes`), which
  // is what the call was before the bridge.
  const GET_CONTEXTS_WAIT = 1000;
  const runtime = chrome.runtime;
  if (runtime && typeof runtime.getContexts === 'function') {
    const nativeGetContexts = runtime.getContexts.bind(runtime);
    const relay = 'chrome-extension://' + runtime.id + '/nautilus-bridge/relay.html';
    const notRelay = (context) => !context || context.documentUrl !== relay;
    const LISTS = [['contextIds', 'contextId'], ['contextTypes', 'contextType'], ['documentIds', 'documentId'], ['documentOrigins', 'documentOrigin'], ['documentUrls', 'documentUrl'], ['frameIds', 'frameId'], ['tabIds', 'tabId'], ['windowIds', 'windowId']];
    const passes = (context, filter) =>
      LISTS.every(([list, field]) => !Array.isArray(filter[list]) || filter[list].indexOf(context[field]) !== -1) &&
      (typeof filter.incognito !== 'boolean' || context.incognito === filter.incognito);
    const getContexts = function (filter, callback) {
      let promise;
      if (!native) {
        // Chromium checks the filter itself, and throws as it always did.
        promise = nativeGetContexts(filter).then((contexts) => contexts.filter(notRelay));
      } else {
        if (filter === null || typeof filter !== 'object' || Array.isArray(filter)) {
          throw new TypeError('Error in invocation of runtime.getContexts: No matching signature.');
        }
        // Settled either way, so a failure waits to be read below and is
        // never an unhandled rejection.
        const panels = bounded(bridge.call('runtime', 'openPanels', []), GET_CONTEXTS_WAIT).then(
          (open) => ({ open }),
          (error) => ({ error })
        );
        promise = nativeGetContexts({}).then((originals) =>
          panels
            .then((now) => {
              if (now.error) throw now.error;
              return bounded(bridge.call('runtime', 'getContexts', [filter, originals, now.open]), GET_CONTEXTS_WAIT);
            })
            .catch(() => originals.filter(notRelay).filter((context) => passes(context, filter)))
        );
      }
      if (typeof callback !== 'function') return promise;
      promise.then(
        (contexts) => callback(contexts),
        (error) => {
          console.error(`Unchecked runtime.lastError: ${error.message}`);
          callback();
        }
      );
      return undefined;
    };
    Object.defineProperty(runtime, 'getContexts', {
      value: getContexts,
      configurable: true,
      enumerable: true,
      writable: true,
    });
  }

  // window.close() in a page of an extension that has a side panel: a
  // panel's page closes its panel through the model, so the page area gets
  // its width back; any other page (the engine answers false) closes as it
  // always did, and so does every page when the engine rejects or has not
  // answered within CLOSE_WAIT. An extension without a side panel keeps
  // Chromium's own function, untouched.
  const CLOSE_WAIT = 500;
  if (native && typeof window !== 'undefined' && typeof window.close === 'function' && window === window.top) {
    const nativeClose = window.close.bind(window);
    window.close = function () {
      bounded(bridge.call('sidePanel', 'closeSelf', []), CLOSE_WAIT).then(
        (closed) => {
          if (closed === false) nativeClose();
        },
        () => nativeClose()
      );
    };
  }
})(self);
