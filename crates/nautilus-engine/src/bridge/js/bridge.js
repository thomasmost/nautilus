// Nautilus bridge, protocol 1. Loaded by nautilus-loader.js ahead of the
// extension's own service worker, in the worker's first evaluation. The
// worker script (`worker_script()` in bridge/mod.rs) is this file followed by
// the namespace scripts (core.js and the rest), which replace chrome.tabs,
// chrome.windows and chrome.runtime.onMessage through the two globals below.
//
// It takes deliveries from the relay page (a toolbar click, and the engine's
// events), and makes the calls the namespace scripts send. Everything goes over
// a runtime port named PORT, which the extension's own listeners never see.
(function () {
  'use strict';
  if (self.__nautilusTransport) { return; }
  var PORT = 'nautilus-bridge-1';
  var CALL_TIMEOUT_MS = 10000;
  // A call whose port has no listener on the other end waits this long for a
  // relay to listen, re-sending every RESEND_MS, then fails.
  var RECEIVER_WAIT_MS = 3000;
  var RESEND_MS = 100;
  var runtime = chrome.runtime;

  function define(object, name, value) {
    try {
      Object.defineProperty(object, name, { value: value, writable: true, configurable: true });
      return object[name] === value;
    } catch (e) {
      return false;
    }
  }

  // ---- the extension's onConnect listeners do not see the bridge's port
  var onConnect = runtime.onConnect;
  var nativeAdd = onConnect.addListener.bind(onConnect);
  var nativeRemove = onConnect.removeListener.bind(onConnect);
  var nativeHas = onConnect.hasListener.bind(onConnect);
  var wrappers = new Map();
  var hidden =
    define(onConnect, 'addListener', function (listener) {
      if (typeof listener !== 'function') { return nativeAdd(listener); }
      var wrapper = wrappers.get(listener);
      if (!wrapper) {
        wrapper = function (port) {
          if (port && port.name === PORT) { return undefined; }
          return listener.apply(this, arguments);
        };
        wrappers.set(listener, wrapper);
      }
      return nativeAdd(wrapper);
    }) &&
    define(onConnect, 'removeListener', function (listener) {
      var wrapper = wrappers.get(listener);
      wrappers.delete(listener);
      return nativeRemove(wrapper || listener);
    }) &&
    define(onConnect, 'hasListener', function (listener) {
      if (listener === bridgeListener) { return false; }
      return nativeHas(wrappers.get(listener) || listener);
    }) &&
    define(onConnect, 'hasListeners', function () {
      // Only the extension's own listeners count; the bridge's is always there.
      return wrappers.size > 0;
    });

  // ---- the port to the relay page
  // One id per worker start: the engine tells a restarted worker from this one
  // by it, and an answer carries the id of the call it answers.
  var INSTANCE = Math.random().toString(36).slice(2) + Date.now().toString(36);
  var port = null;
  var calls = new Map(); // id -> {resolve, reject, timer}
  var nextCall = 1;
  var taken = new Set(); // delivery ids this worker instance has run
  // The listeners of the engine's events (`__bridgeOnEvent`), which every
  // delivery other than a click reaches.
  var handlers = [];
  // Resolves once the hello of the port in use has been sent: a call waits
  // for it, so the engine has this instance's identity before any call.
  var ready = Promise.resolve();

  // ---- the worker's debugger sessions, asked by `sendCommand`.
  // `getTargets` lists the tabs; each is probed with `Runtime.evaluate` of `1`,
  // which only a session this extension opened answers. Each answer is one of
  // three states: `attached`; `not-attached`, which is Chromium's own "Debugger
  // is not attached" error (or "No tab with id", a tab that does not exist);
  // and `unknown` for a timeout or any other error. Only `not-attached` is
  // evidence that a session has ended (the engine forgets on it alone). A probe
  // is bounded, so a hung or busy page (or one paused by the debugger) yields
  // `unknown`, which keeps the debugger: harmless. Whether `Runtime.evaluate`
  // answers on a paused or hidden page was not verified (see the protocol doc).
  var PROBE_TIMEOUT_MS = 1000;
  var HELLO_TIMEOUT_MS = 2000;

  function notAttached(error) {
    var text = String((error && error.message) || error || '');
    return /Debugger is not attached|No tab with id/.test(text);
  }

  function probeSession(debuggerApi, tabId) {
    return new Promise(function (resolve) {
      var done = false;
      var timer = setTimeout(function () { finish('unknown'); }, PROBE_TIMEOUT_MS);
      function finish(state) {
        if (done) { return; }
        done = true;
        clearTimeout(timer);
        resolve({ tab: tabId, state: state });
      }
      try {
        debuggerApi.sendCommand({ tabId: tabId }, 'Runtime.evaluate', { expression: '1', returnByValue: true })
          .then(function () { finish('attached'); }, function (e) { finish(notAttached(e) ? 'not-attached' : 'unknown'); });
      } catch (e) {
        finish('unknown');
      }
    });
  }

  // Every tab the target list names, with its state. Never rejects. The whole
  // chain, the list included, is raced against HELLO_TIMEOUT_MS: when it has not
  // settled by then the hello names no session, which forgets nothing. Each probe
  // is bounded by PROBE_TIMEOUT_MS on its own (a probe that has not answered is
  // `unknown`), so a chain of answered probes ends well inside the race.
  function sessionStates() {
    var debuggerApi = chrome.debugger;
    if (!debuggerApi || typeof debuggerApi.getTargets !== 'function') { return Promise.resolve([]); }
    var timer = null;
    var limit = new Promise(function (resolve) { timer = setTimeout(function () { resolve([]); }, HELLO_TIMEOUT_MS); });
    var chain = Promise.resolve().then(function () { return debuggerApi.getTargets(); }).then(function (targets) {
      var tabs = [];
      (targets || []).forEach(function (t) {
        if (typeof t.tabId === 'number' && tabs.indexOf(t.tabId) < 0) { tabs.push(t.tabId); }
      });
      return Promise.all(tabs.map(function (tabId) { return probeSession(debuggerApi, tabId); }));
    });
    return Promise.race([chain, limit]).then(function (answers) {
      clearTimeout(timer);
      return answers;
    }, function () { clearTimeout(timer); return []; });
  }

  function adopt(newPort) {
    port = newPort;
    newPort.onMessage.addListener(function (message) { onMessage(newPort, message); });
    newPort.onDisconnect.addListener(function () {
      if (noListener(runtime.lastError)) {
        // No listener was ever connected, so nothing on the other end saw the
        // calls sent on this port: they are sent again on a port that has one.
        if (port === newPort) { port = null; }
        calls.forEach(function (call) { if (call.on === newPort) { resend(call); } });
        return;
      }
      if (port !== newPort) { return; }
      port = null;
      calls.forEach(function (call, id) {
        clearTimeout(call.timer);
        call.reject(new Error('Nautilus: the browser stopped listening.'));
        calls.delete(id);
      });
    });
    // `ready` never rejects, and settles within HELLO_TIMEOUT_MS, so a call
    // waits for the hello for a bounded time only.
    ready = sessionStates().then(function (sessions) {
      if (port !== newPort) { return; }
      try { newPort.postMessage({ hello: { instance: INSTANCE, sessions: sessions } }); }
      catch (ignored) { /* the port went before the hello */ }
    }, function () {}).catch(function () {});
  }

  // Registered in the first evaluation, so a connect from the relay page
  // starts a stopped worker and lands here. Kept as a named function so
  // hasListener can recognise it.
  // Only the relay page of this extension opens the bridge's port. A content
  // script's port carries the web page's address, and any other context of this
  // extension has another address; the relay page itself may have a sender tab
  // (the engine hosts it in a tab), so a tab is not what is checked.
  var RELAY_URL = runtime.getURL('nautilus-bridge/relay.html');
  function fromRelay(sender) {
    return !!sender && sender.id === runtime.id && sender.url === RELAY_URL;
  }
  var bridgeListener = function (newPort) {
    if (newPort.name === PORT && fromRelay(newPort.sender)) { adopt(newPort); }
  };
  nativeAdd(bridgeListener);

  function ensurePort() {
    if (!port) { adopt(runtime.connect({ name: PORT })); }
    return port;
  }

  // Chromium's error for a port whose other end has no listener: the port is
  // closed at once and nothing on the other end received anything over it.
  function noListener(error) {
    return /Receiving end does not exist|Could not establish connection/.test(String((error && error.message) || error || ''));
  }

  function call(method, args) {
    return new Promise(function (resolve, reject) {
      var id = nextCall++;
      var entry = { id: id, method: method, args: args, resolve: resolve, reject: reject, timer: null, on: null, nobodyUntil: 0 };
      entry.timer = setTimeout(function () {
        calls.delete(id);
        reject(new Error('Nautilus: ' + method + ' was not answered.'));
      }, CALL_TIMEOUT_MS);
      calls.set(id, entry);
      try {
        ensurePort();
      } catch (e) {
        fail(entry, e);
        return;
      }
      send(entry);
    });
  }

  function fail(entry, error) {
    if (!calls.has(entry.id)) { return; }
    calls.delete(entry.id);
    clearTimeout(entry.timer);
    entry.reject(error);
  }

  // Posts a call on the port in use, after that port's hello: a call is always
  // answered under the identity its port announced. The port is read when the
  // call is posted, not when it was made, so a call that waited is never posted
  // on a port that was replaced.
  function send(entry) {
    if (!calls.has(entry.id)) { return; }
    var waited = ready;
    waited.then(function () {
      if (!calls.has(entry.id)) { return; }
      if (ready !== waited) { return send(entry); }
      if (!port) { return resend(entry); }
      entry.on = port;
      try {
        port.postMessage({ call: { id: entry.id, method: entry.method, args: entry.args, instance: INSTANCE } });
      } catch (e) {
        fail(entry, e);
      }
    });
  }

  // A call that found no listener is sent again on a new port, every RESEND_MS,
  // until RECEIVER_WAIT_MS has passed since the first such port; then it fails.
  function resend(entry) {
    if (!calls.has(entry.id)) { return; }
    var now = Date.now();
    if (!entry.nobodyUntil) { entry.nobodyUntil = now + RECEIVER_WAIT_MS; }
    if (now > entry.nobodyUntil) {
      fail(entry, new Error('Nautilus: ' + entry.method + ' found no listener in the browser.'));
      return;
    }
    setTimeout(function () {
      if (!calls.has(entry.id)) { return; }
      try {
        ensurePort();
      } catch (e) {
        fail(entry, e);
        return;
      }
      send(entry);
    }, RESEND_MS);
  }

  function onMessage(from, message) {
    if (!message) { return; }
    if (message.reply) {
      // An answer to another instance's call is not ours to take: that
      // instance may have used the same call id.
      if (message.reply.instance !== INSTANCE) { return; }
      var pending = calls.get(message.reply.id);
      if (!pending) { return; }
      calls.delete(message.reply.id);
      clearTimeout(pending.timer);
      if (message.reply.error !== undefined) { pending.reject(new Error(message.reply.error)); }
      else { pending.resolve(message.reply.result); }
    } else if (message.deliver) {
      take(from, message.deliver);
    }
  }

  // ---- deliveries: acknowledged on receipt, run once
  function take(from, delivery) {
    from.postMessage({ received: delivery.id });
    if (taken.has(delivery.id)) { return; }
    taken.add(delivery.id);
    run(delivery).then(
      function () { from.postMessage({ done: { id: delivery.id, ok: true } }); },
      function (e) {
        try { from.postMessage({ done: { id: delivery.id, ok: false, error: String((e && e.message) || e) } }); }
        catch (ignored) { /* the port went with the page */ }
      }
    );
  }

  // Chromium's own chrome.tabs.get. core.js copies the namespace's original
  // functions (bound) into `native` before it replaces the namespace, so the
  // click reaches Chromium and never the engine. Core runs in this same script,
  // before any delivery can arrive.
  function nativeTabGet(tabId) {
    return new Promise(function (resolve, reject) {
      var core = self.__nautilusBridge;
      var get = core && core.native && core.native.tabs && core.native.tabs.get;
      if (typeof get !== 'function') {
        reject(new Error('chrome.tabs.get is not available in this worker'));
        return;
      }
      get(tabId, function (tab) {
        var error = runtime.lastError;
        if (error) { reject(new Error(error.message)); } else { resolve(tab); }
      });
    });
  }

  function run(delivery) {
    if (delivery.event !== 'action.onClicked') {
      // Every listener of the namespaces gets the event. One that throws is
      // logged, and the others still run. The receipt was posted before this
      // runs, so the engine knows the worker took the delivery: a failure is
      // reported as done with ok false, which the engine reads as received
      // and never retries (policy::after_delivery).
      var failure = null;
      handlers.forEach(function (handler) {
        try {
          handler(delivery.event, delivery.args || []);
        } catch (e) {
          console.error(e);
          if (failure === null) { failure = e; }
        }
      });
      return failure === null ? Promise.resolve() : Promise.reject(failure);
    }
    if (!chrome.action || !chrome.action.onClicked) {
      return Promise.reject(new Error('this extension has no action'));
    }
    // Chromium's own tab object, so the listener gets what Chrome
    // would give it.
    return nativeTabGet(delivery.tabId).then(function (tab) {
      chrome.action.onClicked.dispatch(tab);
    });
  }

  // ---- chrome.debugger: the engine learns which tabs are debugged.
  // An attach is reported after Chromium has made it, and a detach after it
  // has been made; the call waits for the engine's answer, which comes once
  // the engine has shown the tab (a hidden renderer answers no input, so the
  // debugger's first command must meet a visible one). The report is best
  // effort: a report that fails or times out does not stop the extension's
  // own call.
  // The debuggee is Chromium's tab id, or {targetId} for an attach by a
  // DevTools target id; any other target (an extension's, say) is not
  // reported. The engine maps either one through its table.
  function report(method, target) {
    if (!target) { return Promise.resolve(); }
    var debuggee;
    if (typeof target.tabId === 'number') { debuggee = target.tabId; }
    else if (typeof target.targetId === 'string') { debuggee = { targetId: target.targetId }; }
    else { return Promise.resolve(); }
    return call(method, [debuggee]).then(function () {}, function () {});
  }

  // A wrapper of Chromium's call that reports `method` for the target after
  // the call succeeded, with the callback or promise style the caller used.
  function reporting(native, owner, method) {
    return function (target) {
      var args = Array.prototype.slice.call(arguments);
      var last = args[args.length - 1];
      if (typeof last === 'function') {
        args[args.length - 1] = function () {
          var failed = !!runtime.lastError;
          var self = this;
          var values = arguments;
          if (failed) { return last.apply(self, values); }
          report(method, target).then(function () { last.apply(self, values); });
        };
        return native.apply(owner, args);
      }
      return native.apply(owner, args).then(function (result) {
        return report(method, target).then(function () { return result; });
      });
    };
  }

  if (chrome.debugger) {
    var nativeAttach = chrome.debugger.attach;
    var nativeDetach = chrome.debugger.detach;
    define(chrome.debugger, 'attach', reporting(nativeAttach, chrome.debugger, 'debugger.attached'));
    define(chrome.debugger, 'detach', reporting(nativeDetach, chrome.debugger, 'debugger.detached'));
    // A detach the extension did not make: its tab closed, or the user
    // cancelled the debugger's infobar.
    if (chrome.debugger.onDetach) {
      chrome.debugger.onDetach.addListener(function (source) {
        if (source) { report('debugger.detached', source); }
      });
    }
  }

  // The two functions core.js is written against. `__bridgeCall` makes a call
  // of the engine by its namespaced name; `__bridgeOnEvent` adds a listener of
  // the events the engine delivers.
  self.__bridgeCall = function (ns, method, args) {
    return call(ns + '.' + method, args);
  };
  self.__bridgeOnEvent = function (handler) {
    handlers.push(handler);
  };

  // Not `__nautilusBridge`: that name is the namespaces' object, which
  // core.js defines after this script has run.
  self.__nautilusTransport = Object.freeze({ protocol: 1, portHidden: hidden });
})();
