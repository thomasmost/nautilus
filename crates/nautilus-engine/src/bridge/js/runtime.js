// chrome.runtime.onMessage, wrapped so that sender.tab carries the strip's
// fields (index, windowId, groupId, active, highlighted, selected, pinned)
// before the extension's listeners see it. Chromium knows none of them for
// these tabs. One listener is registered with Chromium, and it runs the
// extension's listeners itself, as Chromium would: the first response
// wins; a listener that returns true or a promise keeps the channel open.
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  var runtime = root.chrome && root.chrome.runtime;
  var nativeEvent = runtime && runtime.onMessage;
  if (!bridge || !nativeEvent || bridge.installed.indexOf('runtime.onMessage') !== -1) { return; }
  var listeners = [];

  function deliver(message, sender, sendResponse) {
    var open = false;
    var done = false;
    function respond(value) {
      if (!done) { done = true; sendResponse(value); }
    }
    listeners.slice().forEach(function (fn) {
      var result;
      try { result = fn(message, sender, respond); } catch (e) { console.error(e); return; }
      if (result === true) {
        open = true;
      } else if (result && typeof result.then === 'function') {
        open = true;
        result.then(respond, function (e) { console.error(e); respond(); });
      }
    });
    return { open: open, done: function () { return done; } };
  }

  function onNative(message, sender, sendResponse) {
    var tab = sender && sender.tab;
    if (!tab || typeof tab.id !== 'number') {
      return deliver(message, sender, sendResponse).open;
    }
    bridge.call('tabs', 'stripOf', [tab.id]).then(
      function (strip) { if (strip) { Object.assign(tab, strip); } },
      function () {}
    ).then(function () {
      var state = deliver(message, sender, sendResponse);
      // Nobody answered and nobody will: let the sender go.
      if (!state.open && !state.done()) { sendResponse(); }
    });
    return true;
  }

  var wrapper = {
    addListener: function (fn) {
      if (typeof fn !== 'function' || listeners.indexOf(fn) !== -1) { return; }
      listeners.push(fn);
      if (listeners.length === 1) { nativeEvent.addListener(onNative); }
    },
    removeListener: function (fn) {
      var at = listeners.indexOf(fn);
      if (at === -1) { return; }
      listeners.splice(at, 1);
      if (listeners.length === 0) { nativeEvent.removeListener(onNative); }
    },
    hasListener: function (fn) { return listeners.indexOf(fn) !== -1; },
    hasListeners: function () { return listeners.length > 0; }
  };
  try {
    Object.defineProperty(runtime, 'onMessage', { value: wrapper, enumerable: true, configurable: true });
    bridge.installed.push('runtime.onMessage');
  } catch (e) {
    console.error('nautilus-bridge: cannot wrap runtime.onMessage: ' + e);
  }
})(self);
