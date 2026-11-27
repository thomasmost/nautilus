// The bridge's core: replaces whole chrome.* namespaces in an extension
// context, on `chrome` and on the `browser` global beside it, and routes
// their calls to the engine. It needs two functions in
// scope, which the transport for this kind of context defines first:
//   __bridgeCall(namespace, method, args) -> Promise (rejects with an Error
//     whose message is the engine's)
//   __bridgeOnEvent(handler), handler(eventName, args)
// Everything here fails closed: a function Chromium defines in a replaced
// namespace and this file does not name rejects as unsupported.
(function (root) {
  'use strict';
  if (root.__nautilusBridge) { return; }
  var call = root.__bridgeCall;
  var onEvent = root.__bridgeOnEvent;
  var chrome = root.chrome;
  if (typeof call !== 'function' || typeof onEvent !== 'function' || !chrome) {
    console.error('nautilus-bridge: no transport or no chrome in this context');
    if (typeof root.__nautilusRefuseNative === 'function') { root.__nautilusRefuseNative(); }
    return;
  }
  // Chromium's own namespaces, kept as plain objects of bound functions
  // and event objects, for forwarded calls. They stay in this closure:
  // nothing that reaches them is put on `__nautilusBridge`, which every
  // script of the context can read (Chromium's bound tabs.group ends the
  // browser process).
  var native = {};
  var events = new Map();
  // The methods the engine answers with a forward (`__forward`), in every
  // namespace. The Rust router holds the same list (`router::FORWARDED`).
  var FORWARDED = ['get', 'update', 'setZoom', 'reload', 'goBack', 'goForward', 'getZoom', 'getZoomSettings', 'setZoomSettings', 'detectLanguage', 'executeScript', 'insertCSS', 'removeCSS'];

  function isEvent(value) {
    return !!value && typeof value === 'object' && typeof value.addListener === 'function';
  }

  function BridgeEvent(name) {
    this.name = name;
    this.listeners = [];
    this.announced = false;
    if (name) { events.set(name, this); }
  }
  BridgeEvent.prototype.addListener = function (fn, filters) {
    if (typeof fn !== 'function' || this.hasListener(fn)) { return; }
    this.listeners.push({ fn: fn, filters: filters });
    if (this.name && !this.announced) {
      this.announced = true;
      call('bridge', 'listen', [this.name]).catch(function () {});
    }
  };
  BridgeEvent.prototype.removeListener = function (fn) {
    this.listeners = this.listeners.filter(function (l) { return l.fn !== fn; });
  };
  BridgeEvent.prototype.hasListener = function (fn) {
    return this.listeners.some(function (l) { return l.fn === fn; });
  };
  BridgeEvent.prototype.hasListeners = function () {
    return this.listeners.length > 0;
  };
  // Every window here is a normal one, so a filter that leaves "normal"
  // out leaves everything out.
  BridgeEvent.prototype.accepts = function (filters) {
    var types = filters && filters.windowTypes;
    return !Array.isArray(types) || types.indexOf('normal') !== -1;
  };
  BridgeEvent.prototype.dispatch = function (args) {
    var self = this;
    this.listeners.slice().forEach(function (l) {
      if (!self.accepts(l.filters)) { return; }
      try { l.fn.apply(undefined, args); } catch (e) { console.error(e); }
    });
  };

  // The arguments of a call without its callback and without trailing
  // undefineds (which JSON would turn into nulls).
  function split(args) {
    var list = Array.prototype.slice.call(args);
    var callback = typeof list[list.length - 1] === 'function' ? list.pop() : null;
    while (list.length && list[list.length - 1] === undefined) { list.pop(); }
    return { list: list, callback: callback };
  }

  // Run `callback` with chrome.runtime.lastError set to `error`, as
  // Chromium does for a failed call made with a callback.
  function withLastError(error, callback) {
    var runtime = chrome.runtime;
    var message = String((error && error.message) || error);
    var previous = Object.getOwnPropertyDescriptor(runtime, 'lastError');
    var checked = false;
    Object.defineProperty(runtime, 'lastError', {
      configurable: true,
      enumerable: true,
      get: function () { checked = true; return { message: message }; }
    });
    try {
      callback();
    } finally {
      delete runtime.lastError;
      if (previous) { Object.defineProperty(runtime, 'lastError', previous); }
      if (!checked) { console.error('Unchecked runtime.lastError: ' + message); }
    }
  }

  // A promise, or the callback style when the caller passed one.
  function settle(promise, callback) {
    if (!callback) { return promise; }
    promise.then(
      function (value) { callback(value); },
      function (error) { withLastError(error, callback); }
    );
    return undefined;
  }

  // Chromium's own function, by callback so it works in Manifest V2
  // contexts too (which have no promises).
  function callNative(namespace, method, args) {
    return new Promise(function (resolve, reject) {
      var fn = native[namespace] && native[namespace][method];
      if (typeof fn !== 'function') {
        reject(new Error(namespace + '.' + method + ' is not supported.'));
        return;
      }
      try {
        fn.apply(undefined, args.concat([function (value) {
          var error = chrome.runtime.lastError;
          if (error) { reject(new Error(error.message)); } else { resolve(value); }
        }]));
      } catch (e) {
        reject(e);
      }
    });
  }

  // Lay the strip's fields over Chromium's tab objects, by tab id.
  function patchTabs(value, strip) {
    if (Array.isArray(value)) {
      value.forEach(function (item) { patchTabs(item, strip); });
    } else if (value && typeof value === 'object' && typeof value.id === 'number' && strip[value.id]) {
      Object.assign(value, strip[value.id]);
    }
    return value;
  }

  function run(namespace, method, list) {
    return call(namespace, method, list).then(function (answer) {
      var forward = answer && typeof answer === 'object' && answer.__forward;
      if (forward) {
        // Only the methods the engine forwards (`router::FORWARDED`, which a
        // test keeps equal to this list): an answer naming any other method
        // comes from something that is not the engine, and Chromium's function
        // is not called for it.
        if (FORWARDED.indexOf(forward.method) < 0) {
          throw new Error(namespace + '.' + forward.method + ' is not supported.');
        }
        return callNative(namespace, forward.method, forward.args).then(function (result) {
          return patchTabs(result, forward.strip || {});
        });
      }
      return answer === null ? undefined : answer;
    });
  }

  function bridged(namespace, method) {
    return function () {
      var parts = split(arguments);
      return settle(run(namespace, method, parts.list), parts.callback);
    };
  }

  function unsupported(namespace, method) {
    return function () {
      var parts = split(arguments);
      var error = new Error(namespace + '.' + method + ' is not supported.');
      return settle(Promise.reject(error), parts.callback);
    };
  }

  // Build the replacement for one namespace. `spec`:
  //   bridged   functions the engine answers
  //   optional  the same, defined only where Chromium defines them
  //   direct    Chromium's own functions, bound (they return something
  //             synchronously, or address a tab Chromium knows)
  //   events    events the engine fires
  //   nativeEvents  Chromium's own events, kept
  // Constants and enums are copied; every other function rejects as
  // unsupported and every other event never fires.
  function build(namespace, spec, source) {
    var target = {};
    Object.keys(source).forEach(function (key) {
      var value = source[key];
      if (typeof value === 'function') {
        target[key] = unsupported(namespace, key);
      } else if (isEvent(value)) {
        target[key] = new BridgeEvent(null);
      } else {
        target[key] = value;
      }
    });
    (spec.bridged || []).forEach(function (m) { target[m] = bridged(namespace, m); });
    (spec.optional || []).forEach(function (m) {
      if (typeof source[m] === 'function') { target[m] = bridged(namespace, m); }
    });
    (spec.direct || []).forEach(function (m) {
      if (typeof source[m] === 'function') { target[m] = source[m]; }
    });
    (spec.events || []).forEach(function (e) { target[e] = new BridgeEvent(namespace + '.' + e); });
    (spec.nativeEvents || []).forEach(function (e) {
      if (isEvent(source[e])) { target[e] = source[e]; }
    });
    return target;
  }

  // The second global of an extension context. Chromium 156 sets every API
  // on `chrome` and again on `browser`, each a lazy property that answers
  // the context's one cached object for that API
  // (NativeExtensionBindingsSystem::UpdateBindingsForContext, GetAPIHelper):
  // so before the bridge runs `browser.tabs` is `chrome.tabs`, and after a
  // replacement of `chrome.tabs` alone it would still be Chromium's. Null
  // where the context has none.
  var twin = (function () {
    var other = root.browser;
    return other && (typeof other === 'object' || typeof other === 'function') && other !== chrome ? other : null;
  })();

  // Chromium's own object under `name` is one object on both globals, and
  // what the bridge wraps of it (runtime's getContexts, onMessage and
  // onConnect, the debugger's attach and detach) it wraps on that object. A
  // context where `browser` has another object would keep an unwrapped one:
  // it is given chrome's.
  function same(name) {
    if (!twin || !chrome[name] || twin[name] === chrome[name]) { return; }
    console.error('nautilus-bridge: browser.' + name + ' is not chrome.' + name + ' in this context; made the same');
    try {
      Object.defineProperty(twin, name, { value: chrome[name], enumerable: true, configurable: true, writable: true });
    } catch (e) {
      console.error('nautilus-bridge: cannot make browser.' + name + ' chrome.' + name + ': ' + e);
    }
  }
  same('runtime');
  same('debugger');

  // Every member of `own`, an object of Chromium's that its global does not
  // let go of, replaced in place: the replacement's members, a rejection for
  // any other function and an inert event for any other event.
  function replaceMembers(namespace, own, replacement) {
    var members = {};
    Object.keys(own).forEach(function (key) {
      if (typeof own[key] === 'function') { members[key] = unsupported(namespace, key); }
      else if (isEvent(own[key])) { members[key] = new BridgeEvent(null); }
    });
    Object.keys(replacement).forEach(function (key) { members[key] = replacement[key]; });
    Object.keys(members).forEach(function (key) {
      Object.defineProperty(own, key, {
        value: members[key], enumerable: true, configurable: false, writable: false
      });
    });
  }

  // Put `replacement` in place of Chromium's `namespace`, on `chrome` and on
  // `browser`. Each property is redefined so that Chromium
  // cannot put its own back (it defines every API again on both globals when
  // the set of extensions changes); where a property cannot be redefined,
  // the object it holds has its members replaced one by one instead.
  // `browser` gets the very object `chrome` ends up with. It throws where
  // neither can be done; `install` is what calls it.
  function pin(namespace, replacement) {
    [chrome, twin].forEach(function (holder) {
      if (!holder) { return; }
      var held = holder === chrome ? replacement : chrome[namespace];
      try {
        Object.defineProperty(holder, namespace, {
          value: held, enumerable: true, configurable: false, writable: false
        });
      } catch (e) {
        var own = holder[namespace];
        if (own === held) { return; }
        if (!own || (typeof own !== 'object' && typeof own !== 'function')) { throw e; }
        replaceMembers(namespace, own, replacement);
      }
    });
  }

  // The one place a namespace is replaced, for `define` and for the
  // hand-written sidePanel: `pin`, in a guard of its own. A namespace that
  // cannot be put in is refused on both globals (refuse.js), which also
  // finishes an object `replaceMembers` left half replaced, and the
  // namespaces after it are installed all the same. Returns whether the
  // replacement is in.
  function install(namespace, replacement) {
    try {
      pin(namespace, replacement);
      return true;
    } catch (e) {
      console.error('nautilus-bridge: cannot install ' + namespace + ', which is refused: ' + e);
      delete native[namespace];
      if (typeof root.__nautilusRefuseNative === 'function') { root.__nautilusRefuseNative([namespace]); }
      return false;
    }
  }

  // Replace the namespace on `chrome` and on `browser` (`install`), keeping
  // Chromium's own functions, bound, for the calls the engine forwards.
  function define(namespace, spec) {
    var original = chrome[namespace];
    var source = {};
    if (original) {
      Object.keys(original).forEach(function (key) {
        var value = original[key];
        source[key] = typeof value === 'function' ? value.bind(original) : value;
      });
    }
    native[namespace] = source;
    if (install(namespace, build(namespace, spec, source))) { bridge.installed.push(namespace); }
  }

  // The bridge's own listeners, by event name: run before the extension's,
  // and never seen by them (`hasListener`, `hasListeners`). Tapping an event
  // asks the engine for it, as an extension's first listener does.
  var taps = new Map();
  function tap(name, fn) {
    if (!taps.has(name)) {
      taps.set(name, []);
      call('bridge', 'listen', [name]).catch(function () {});
    }
    taps.get(name).push(fn);
  }

  // What the namespace scripts need, and nothing that reaches a function
  // of Chromium's: `define` binds what it finds under `chrome` when it is
  // called, which after this evaluation is the bridge's own.
  var bridge = {
    call: call,
    tap: tap,
    define: define,
    pin: install,
    BridgeEvent: BridgeEvent,
    installed: []
  };
  // A page's own Chromium tab, for its report (page_report.js, the one
  // reader outside this file of anything of Chromium's): handed over once,
  // and gone from the object when it is taken, which is before the page's
  // first script. A worker has no tab of its own and gets none.
  if (!root.__nautilusTransport) {
    Object.defineProperty(bridge, 'takeCurrentTab', {
      configurable: true,
      value: function () {
        delete bridge.takeCurrentTab;
        var getCurrent = native.tabs && native.tabs.getCurrent;
        if (typeof getCurrent !== 'function') { return null; }
        // Asked as it always was, without a callback: Chromium's promise.
        return Promise.resolve(getCurrent());
      }
    });
  }
  Object.defineProperty(root, '__nautilusBridge', { value: bridge });

  onEvent(function (name, args) {
    (taps.get(name) || []).forEach(function (fn) {
      try { fn.apply(undefined, args || []); } catch (e) { console.error(e); }
    });
    var event = events.get(name);
    if (event) { event.dispatch(args || []); }
  });
})(self);
