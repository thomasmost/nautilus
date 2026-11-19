// chrome.runtime.onMessage, wrapped so that sender.tab carries the strip's
// fields (index, windowId, groupId, active, highlighted, selected, pinned)
// before the extension's listeners see it. Chromium knows none of them for
// these tabs. One listener is registered with Chromium, and it runs the
// extension's listeners itself, as Chromium would: the first response wins; a
// listener that returns true or a promise keeps the channel open. The wrapper
// is synchronous, so it returns exactly what Chromium needs: true only when a
// listener of this context kept the channel open, and false otherwise, which
// lets Chromium close a channel that no listener of this context holds.
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  var runtime = root.chrome && root.chrome.runtime;
  var nativeEvent = runtime && runtime.onMessage;
  if (!bridge || !nativeEvent || bridge.installed.indexOf('runtime.onMessage') !== -1) { return; }
  var listeners = [];

  // The strip's fields by tab id, as the engine last reported them. A message
  // is delivered at once, so the fields come from here; a tab this context does
  // not hold yet carries Chromium's own fields for its first message, and the
  // engine's strip is fetched after each message, for the next one.
  var FIELDS = ['index', 'windowId', 'groupId', 'active', 'highlighted', 'selected', 'pinned'];
  var strips = Object.create(null);

  function remember(tab) {
    if (!tab || typeof tab.id !== 'number') { return; }
    var strip = {};
    FIELDS.forEach(function (field) { if (field in tab) { strip[field] = tab[field]; } });
    strips[tab.id] = strip;
  }

  function fetchStrip(id) {
    bridge.call('tabs', 'stripOf', [id]).then(function (strip) {
      if (strip) { remember(Object.assign({ id: id }, strip)); }
    }, function () {});
  }

  // A worker lists the tabs now, so the tabs that exist when it starts carry
  // their fields from their first message, and it follows the tabs the engine
  // opens, renames and closes after that, so a tab opened later (an extension
  // page among them) carries its fields from its first message too. The engine
  // sends tabs.onCreated when the tab is published, with its Chromium id once
  // the table knows it, which is before the tab's page can send. (A page's
  // calls wait for its own tab to be reported, so a page does none of this.)
  if (root.__nautilusTransport) {
    bridge.call('tabs', 'query', [{}]).then(function (tabs) {
      (Array.isArray(tabs) ? tabs : []).forEach(remember);
    }, function () {});
    bridge.tap('tabs.onCreated', remember);
    bridge.tap('tabs.onReplaced', function (added, removed) {
      if (strips[removed]) { strips[added] = strips[removed]; delete strips[removed]; }
      fetchStrip(added);
    });
    bridge.tap('tabs.onRemoved', function (id) { delete strips[id]; });
  }

  // Delivers to this context's listeners; returns whether one kept the channel
  // open (returned true or a promise).
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
    return open;
  }

  function onNative(message, sender, sendResponse) {
    var tab = sender && sender.tab;
    if (!tab || typeof tab.id !== 'number') {
      return deliver(message, sender, sendResponse);
    }
    var strip = strips[tab.id];
    if (strip) { Object.assign(tab, strip); }
    fetchStrip(tab.id);
    return deliver(message, sender, sendResponse);
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
