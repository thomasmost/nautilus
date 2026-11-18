// Nautilus bridge, protocol 1. Loaded by nautilus-loader.js ahead of the
// extension's own service worker, in the worker's first evaluation.
//
// Phase 1: it takes deliveries from the relay page (a toolbar click),
// and answers chrome.tabs.query from the engine. Everything goes over a
// runtime port named PORT, which the extension's own listeners never see.
(function () {
  'use strict';
  if (self.__nautilusTransport) { return; }
  var PORT = 'nautilus-bridge-1';
  var CALL_TIMEOUT_MS = 10000;
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
  var port = null;
  var calls = new Map(); // id -> {resolve, reject, timer}
  var nextCall = 1;
  var taken = new Set(); // delivery ids this worker instance has run

  function adopt(newPort) {
    port = newPort;
    newPort.onMessage.addListener(function (message) { onMessage(newPort, message); });
    newPort.onDisconnect.addListener(function () {
      void runtime.lastError;
      if (port !== newPort) { return; }
      port = null;
      calls.forEach(function (call, id) {
        clearTimeout(call.timer);
        call.reject(new Error('Nautilus: the browser stopped listening.'));
        calls.delete(id);
      });
    });
  }

  // Registered in the first evaluation, so a connect from the relay page
  // starts a stopped worker and lands here. Kept as a named function so
  // hasListener can recognise it.
  var bridgeListener = function (newPort) {
    if (newPort.name === PORT) { adopt(newPort); }
  };
  nativeAdd(bridgeListener);

  function ensurePort() {
    if (!port) { adopt(runtime.connect({ name: PORT })); }
    return port;
  }

  function call(method, args) {
    return new Promise(function (resolve, reject) {
      var id = nextCall++;
      var timer = setTimeout(function () {
        calls.delete(id);
        reject(new Error('Nautilus: ' + method + ' was not answered.'));
      }, CALL_TIMEOUT_MS);
      calls.set(id, { resolve: resolve, reject: reject, timer: timer });
      try {
        ensurePort().postMessage({ call: { id: id, method: method, args: args } });
      } catch (e) {
        clearTimeout(timer);
        calls.delete(id);
        reject(e);
      }
    });
  }

  function onMessage(from, message) {
    if (!message) { return; }
    if (message.reply) {
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

  function run(delivery) {
    if (delivery.event === 'action.onClicked') {
      if (!chrome.action || !chrome.action.onClicked) {
        return Promise.reject(new Error('this extension has no action'));
      }
      // Chromium's own tab object, so the listener gets what Chrome
      // would give it.
      return chrome.tabs.get(delivery.tabId).then(function (tab) {
        chrome.action.onClicked.dispatch(tab);
      });
    }
    return Promise.reject(new Error('unknown event ' + delivery.event));
  }

  // ---- chrome.debugger: the engine learns which tabs are debugged.
  // An attach is reported after Chromium has made it, and a detach after it
  // has been made; the call waits for the engine's answer, which comes once
  // the engine has shown the tab (a hidden renderer answers no input, so the
  // debugger's first command must meet a visible one). The report is best
  // effort: a report that fails or times out does not stop the extension's
  // own call.
  function report(method, target) {
    if (!target || typeof target.tabId !== 'number') { return Promise.resolve(); }
    return call(method, [target.tabId]).then(function () {}, function () {});
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
        if (source) { report('debugger.detached', { tabId: source.tabId }); }
      });
    }
  }

  // ---- chrome.tabs.query, answered by the engine
  if (chrome.tabs) {
    var nativeGet = chrome.tabs.get.bind(chrome.tabs);
    define(chrome.tabs, 'query', function (queryInfo, callback) {
      var answer = call('tabs.query', [queryInfo || {}]).then(function (rows) {
        return Promise.all(rows.map(function (row) {
          return nativeGet(row.id).then(function (tab) {
            tab.index = row.index;
            tab.windowId = row.windowId;
            tab.active = row.active;
            tab.highlighted = row.active;
            return tab;
          });
        }));
      });
      if (typeof callback === 'function') {
        answer.then(callback, function (e) { console.error(e); callback([]); });
        return undefined;
      }
      return answer;
    });
  }

  // Not `__nautilusBridge`: that name is the namespaces' object, which
  // Phase 5's core.js defines after this script has run.
  self.__nautilusTransport = Object.freeze({ protocol: 1, portHidden: hidden });
})();
