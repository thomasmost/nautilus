// The page's own Chromium tab id. It runs in the deferred install, after the
// namespaces have replaced Chromium's: the core hands this script Chromium's
// `tabs.getCurrent`, once (`takeCurrentTab`, which is gone when taken).
// The id goes to the browser process as a report, and a call of the page
// waits for the report (page_transport.js), so the browser handles the report
// before any call. The wait is bounded: a page that is no tab, an answer that
// fails, or an answer that never comes releases the calls after REPORT_WAIT_MS.
//
// The engine answers the report (`bridge.reported`, with whether this page is
// a side panel's), and in a panel's page says when the extension's other
// contexts have been told the panel's id (`bridge.told`). Until then this
// page's runtime.sendMessage is held (below).
(function (root) {
  'use strict';
  var REPORT_WAIT_MS = 1000;
  // How long a panel's page holds its messages for `bridge.told`.
  var PANEL_TOLD_WAIT_MS = 1000;
  var bridge = root.__nautilusBridge;
  var take = bridge && bridge.takeCurrentTab;
  var report = root.__nautilusBridgeReport;
  delete root.__nautilusBridgeReport;
  var open = false;
  var held = [];
  function release() {
    if (open) { return; }
    open = true;
    var calls = held;
    held = [];
    calls.forEach(function (call) { call(); });
  }
  // A call made now is sent at once once the report is settled, and waits
  // for it otherwise. Calls leave in the order they were made.
  root.__nautilusReportWait = function (call) {
    if (open) { call(); } else { held.push(call); }
  };

  // ---- runtime.sendMessage, held in a page that may be a side panel's.
  // Chromium takes a panel's browser for a tab and names it as sender.tab;
  // the contexts that receive a message take the tab off once the engine has
  // told them the panel's id (runtime.js), which goes from this page's
  // report to the browser, the hub, the relay and the worker's port, while
  // the page's own message goes straight through Chromium and wins. So the
  // message waits at this end: for the engine's answer to the report, under
  // the report's own bound, and then, in a panel's page only, for the
  // engine's word that the id has been told, or PANEL_TOLD_WAIT_MS. It is
  // then sent with the arguments it was given, and its answer (the promise,
  // or the callback with runtime.lastError) is Chromium's own. Once let go
  // the function is Chromium's again, identically: a page that is no
  // panel's is touched only until its report is answered. Only in an
  // extension that can have a side panel. runtime.connect returns its port
  // at once and cannot be held: a port from a panel keeps its sender.tab.
  var runtime = root.chrome && root.chrome.runtime;
  var ownSend = runtime && runtime.sendMessage;
  var ownDescriptor = runtime && Object.getOwnPropertyDescriptor(runtime, 'sendMessage');
  var holding = false;
  var waiting = [];
  var toldTimer = null;
  var answerTimer = null;
  function letGo() {
    if (!holding) { return; }
    holding = false;
    clearTimeout(toldTimer);
    clearTimeout(answerTimer);
    // Restored only while the holder is still what the page has: a page that
    // replaced it during the hold (a library's wrapper) keeps its own value,
    // and the holder, which passes everything on from now, is its target.
    try {
      var current = Object.getOwnPropertyDescriptor(runtime, 'sendMessage');
      if (current && current.value === heldSend) {
        Object.defineProperty(runtime, 'sendMessage', ownDescriptor || { value: ownSend, writable: true, enumerable: true, configurable: true });
      }
    } catch (e) { /* the holder stays, and passes everything on */ }
    var sends = waiting;
    waiting = [];
    sends.forEach(function (send) { send(); });
  }
  // Chromium's signature of runtime.sendMessage, and its refusal by shape.
  // A call the shape refuses throws here at once, as Chromium's does (probed
  // in Chromium 156 with the same TypeError), and is not held. Only the shapes
  // the probe refused are checked: any other argument error is Chromium's,
  // after the hold (bridge-protocol.md, the hold).
  var SIGNATURE_ERROR = 'Error in invocation of runtime.sendMessage(optional string extensionId, any message, optional object options, optional function callback): No matching signature.';
  function refusedShape(args) {
    var n = args.length;
    if (n > 0 && typeof args[n - 1] === 'function') { n -= 1; }
    if (n === 0 || n > 3) { return true; }
    if (n === 2) { return typeof args[0] !== 'string' && (args[1] === null || typeof args[1] !== 'object'); }
    if (n === 3) {
      return typeof args[0] !== 'string' || (args[2] !== undefined && args[2] !== null && typeof args[2] !== 'object');
    }
    return false;
  }
  function heldSend() {
    var args = arguments;
    if (!holding) { return ownSend.apply(runtime, args); }
    if (refusedShape(args)) { throw new TypeError(SIGNATURE_ERROR); }
    if (typeof args[args.length - 1] === 'function') {
      // The callback is Chromium's to call, with its lastError.
      waiting.push(function () {
        try { ownSend.apply(runtime, args); } catch (e) { console.error(e); }
      });
      return undefined;
    }
    return new Promise(function (resolve, reject) {
      waiting.push(function () {
        try {
          var answer = ownSend.apply(runtime, args);
          if (answer && typeof answer.then === 'function') { answer.then(resolve, reject); } else { resolve(answer); }
        } catch (e) {
          reject(e);
        }
      });
    });
  }
  if (root.chrome && root.chrome.sidePanel && typeof ownSend === 'function' && typeof root.__bridgeOnEvent === 'function') {
    try {
      Object.defineProperty(runtime, 'sendMessage', { value: heldSend, writable: true, enumerable: true, configurable: true });
      holding = runtime.sendMessage === heldSend;
    } catch (e) {
      console.error('nautilus-bridge: cannot hold runtime.sendMessage in this page: ' + e);
    }
  }
  if (holding) {
    // The engine's words reach this page as events of the page lane.
    root.__bridgeOnEvent(function (name, args) {
      if (!holding) { return; }
      if (name === 'bridge.told') { letGo(); return; }
      if (name !== 'bridge.reported') { return; }
      clearTimeout(answerTimer);
      if (args && args[0] && args[0].panel === true) {
        if (toldTimer === null) { toldTimer = setTimeout(letGo, PANEL_TOLD_WAIT_MS); }
      } else {
        letGo();
      }
    });
    // An answer that never comes is not waited for past the report's bound.
    answerTimer = setTimeout(letGo, REPORT_WAIT_MS);
    // A page that goes away lets its held messages go first, so they are not
    // lost with it. side_panel.js's window.close calls the same flush before
    // it asks the engine to close the page.
    root.__nautilusHoldFlush = letGo;
    if (typeof root.addEventListener === 'function') { root.addEventListener('pagehide', letGo); }
  }

  var timer = setTimeout(release, REPORT_WAIT_MS);
  // The calls go; with `reported` false nothing was reported, so no answer
  // is coming and the messages go too.
  function settle(reported) {
    clearTimeout(timer);
    release();
    if (reported !== true) { letGo(); }
  }
  if (typeof take !== 'function') { settle(); return; }
  var current;
  // Taken even when there is nobody to report to, so it is never left.
  try { current = take(); } catch (e) { settle(); return; }
  if (!current || typeof report !== 'function') { settle(); return; }
  try {
    Promise.resolve(current).then(function (tab) {
      var reported = false;
      if (tab && typeof tab.id === 'number') {
        try { report(JSON.stringify({ tab: tab.id })); reported = true; }
        catch (e) { console.error('nautilus-bridge: ' + e); }
      }
      settle(reported);
    }, function () { settle(); });
  } catch (e) {
    settle();
  }
})(self);
