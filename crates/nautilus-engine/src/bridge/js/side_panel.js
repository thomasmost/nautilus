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
  if (!bridge || !chrome || bridge.installed.indexOf('sidePanel') !== -1) { return; }
  bridge.installed.push('sidePanel');

  // The members the engine answers, read by a test in bridge/mod.rs
  // (keep each list on one line): bridged: ['open', 'close', 'getOptions', 'setOptions', 'getPanelBehavior', 'setPanelBehavior', 'getLayout'], events: ['onOpened', 'onClosed'].

  // An extension without the "sidePanel" permission has no
  // chrome.sidePanel, here as in Chromium.
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

    // Not configurable, as `define` leaves a namespace (js/core.js):
    // Chromium defines every API on `chrome` again in each live context
    // when the set of extensions changes, and a configurable property is
    // then its own once more, for good.
    try {
      Object.defineProperty(chrome, 'sidePanel', {
        value: Object.freeze(api),
        configurable: false,
        enumerable: true,
        writable: false,
      });
    } catch (e) {
      // The property cannot be redefined: Chromium's own object stays,
      // with every member of it replaced. A function of Chromium's that
      // is not one of the engine's rejects, as in a `define`d namespace.
      const members = Object.assign({}, api);
      for (const key of Object.keys(native)) {
        if (typeof native[key] === 'function' && !(key in members)) {
          members[key] = () => Promise.reject(new Error(`sidePanel.${key} is not supported.`));
        }
      }
      for (const key of Object.keys(members)) {
        Object.defineProperty(native, key, {
          value: members[key],
          configurable: false,
          enumerable: true,
          writable: false,
        });
      }
    }
  }

  // chrome.runtime.getContexts: Chromium's answer, with each open side
  // panel reported as a SIDE_PANEL context in its window. Chromium is
  // asked for everything and the engine applies the filter, since the
  // panel's entry changes type on the way.
  const runtime = chrome.runtime;
  if (runtime && typeof runtime.getContexts === 'function') {
    const nativeGetContexts = runtime.getContexts.bind(runtime);
    const getContexts = function (filter, callback) {
      if (filter === null || typeof filter !== 'object' || Array.isArray(filter)) {
        throw new TypeError('Error in invocation of runtime.getContexts: No matching signature.');
      }
      const promise = nativeGetContexts({}).then((originals) =>
        bridge.call('runtime', 'getContexts', [filter, originals])
      );
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

  // window.close() in an extension page: a side panel's page closes
  // its panel through the model, so the page area gets its width back;
  // any other page (the engine answers false) closes as it always did.
  if (typeof window !== 'undefined' && typeof window.close === 'function' && window === window.top) {
    const nativeClose = window.close.bind(window);
    window.close = function () {
      bridge.call('sidePanel', 'closeSelf', []).then(
        (closed) => {
          if (closed === false) nativeClose();
        },
        () => nativeClose()
      );
    };
  }
})(self);
