// chrome.debugger in an extension page (a side panel's page, a popup, a
// page in a tab): the engine learns which tabs the page's debugger is on,
// as it learns a worker's (bridge.js, which this follows). A tab a debugger
// is on is kept painted: a hidden renderer answers a DevTools command only
// after about five seconds. An attach is reported after Chromium has made
// it and a detach after it has been made; the caller's own call waits for
// the engine's answer, which comes once the engine has shown the tab, so
// the debugger's first command meets a visible one. The report is best
// effort: one that fails or is not answered does not stop the call.
//
// Pages only. In a worker the transport (bridge.js) has wrapped the same
// functions already, over its own port, and this file does nothing there.
// The wrap is on Chromium's own debugger object, which is one object for
// `chrome` and for `browser` (core.js, `same`).
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  var api = root.chrome && root.chrome.debugger;
  if (!bridge || root.__nautilusTransport || !api || bridge.installed.indexOf('debugger.reports') !== -1) { return; }
  var runtime = root.chrome.runtime;

  // The debuggee is Chromium's tab id, or {targetId} for an attach by a
  // DevTools target id; any other target (an extension's, say) is not
  // reported. The engine maps either one through its table.
  function report(method, target) {
    if (!target) { return Promise.resolve(); }
    var debuggee;
    if (typeof target.tabId === 'number') { debuggee = target.tabId; }
    else if (typeof target.targetId === 'string') { debuggee = { targetId: target.targetId }; }
    else { return Promise.resolve(); }
    return bridge.call('debugger', method, [debuggee]).then(function () {}, function () {});
  }

  // A wrapper of Chromium's call that reports `method` for the target after
  // the call succeeded, with the callback or promise style the caller used.
  function reporting(native, method) {
    return function (target) {
      var args = Array.prototype.slice.call(arguments);
      var last = args[args.length - 1];
      if (typeof last === 'function') {
        args[args.length - 1] = function () {
          var failed = !!(runtime && runtime.lastError);
          var self = this;
          var values = arguments;
          if (failed) { return last.apply(self, values); }
          report(method, target).then(function () { last.apply(self, values); });
        };
        return native.apply(api, args);
      }
      return native.apply(api, args).then(function (result) {
        return report(method, target).then(function () { return result; });
      });
    };
  }

  function define(name, value) {
    try {
      Object.defineProperty(api, name, { value: value, writable: true, configurable: true });
      return api[name] === value;
    } catch (e) {
      return false;
    }
  }

  var nativeAttach = api.attach;
  var nativeDetach = api.detach;
  if (typeof nativeAttach !== 'function' || typeof nativeDetach !== 'function') { return; }
  var wrapped = define('attach', reporting(nativeAttach, 'attached')) && define('detach', reporting(nativeDetach, 'detached'));
  if (!wrapped) {
    console.error('nautilus-bridge: cannot wrap chrome.debugger in this page; a tab it debugs is not kept painted');
  }
  // A detach the page did not make: its tab closed, or the user cancelled
  // the debugger's infobar.
  if (api.onDetach && typeof api.onDetach.addListener === 'function') {
    api.onDetach.addListener(function (source) {
      if (source) { report('detached', source); }
    });
  }
  bridge.installed.push('debugger.reports');
})(self);
