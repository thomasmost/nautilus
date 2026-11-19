// The bridge's core: replaces whole chrome.* namespaces in an extension
// context and routes their calls to the engine. It needs two functions in
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
  // and event objects, for forwarded calls.
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

  // Replace chrome[namespace]. The property is redefined so that Chromium
  // cannot put its own back; where the property cannot be redefined, the
  // namespace object's own members are replaced one by one instead.
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
    var replacement = build(namespace, spec, source);
    try {
      Object.defineProperty(chrome, namespace, {
        value: replacement, enumerable: true, configurable: false, writable: false
      });
    } catch (e) {
      if (!original) { throw e; }
      Object.keys(replacement).forEach(function (key) {
        Object.defineProperty(original, key, {
          value: replacement[key], enumerable: true, configurable: false, writable: false
        });
      });
    }
    bridge.installed.push(namespace);
  }

  var bridge = {
    native: native,
    call: call,
    run: run,
    bridged: bridged,
    define: define,
    BridgeEvent: BridgeEvent,
    installed: []
  };
  Object.defineProperty(root, '__nautilusBridge', { value: bridge });

  onEvent(function (name, args) {
    var event = events.get(name);
    if (event) { event.dispatch(args || []); }
  });
})(self);
