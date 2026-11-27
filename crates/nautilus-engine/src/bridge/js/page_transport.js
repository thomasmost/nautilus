// The transport for extension pages (popups, pages shown as tabs): the
// render process bound __nautilusBridgeSend(text) on the global object;
// the browser process answers by evaluating __nautilusBridgeDeliver(text)
// in this frame.
(function (root) {
  'use strict';
  // Fail closed, for the scripts that put the namespaces in (core.js and
  // page_defer.js) when they cannot: the page's Chromium tabs, windows,
  // tabGroups, commands and sidePanel are replaced by namespaces whose every
  // call is refused, so no native function of theirs stays callable (a
  // native tabs.group traps the browser). On `chrome` and on `browser`, the
  // second global Chromium 156 reaches the same APIs through, with one
  // refusing object for both; pinned, as a replaced namespace is, so that
  // Chromium does not put its own back when the set of extensions changes.
  // tabGroups, commands and sidePanel only where the page has them: an
  // extension without the permission gains no namespace. Defined before the
  // transport's own check, which returns when there is no send.
  if (!Object.prototype.hasOwnProperty.call(root, '__nautilusRefuseNative')) {
    Object.defineProperty(root, '__nautilusRefuseNative', {
      configurable: true,
      value: function () {
        var holders = [root.chrome, root.browser].filter(function (holder, at, all) {
          return !!holder && (typeof holder === 'object' || typeof holder === 'function') && all.indexOf(holder) === at;
        });
        ['tabs', 'windows', 'tabGroups', 'commands', 'sidePanel'].forEach(function (name) {
          var has = holders.some(function (holder) { return !!holder[name]; });
          if (name !== 'tabs' && name !== 'windows' && !has) { return; }
          var refused = new Proxy({}, {
            get: function (target, key) {
              if (typeof key !== 'string' || key === 'then') { return undefined; }
              return function () {
                return Promise.reject(new Error(name + '.' + key + ' is not supported in this page.'));
              };
            }
          });
          holders.forEach(function (holder) {
            try {
              Object.defineProperty(holder, name, { value: refused, enumerable: true, writable: false, configurable: false });
            } catch (e) {
              console.error('nautilus-bridge: cannot refuse ' + name + ': ' + e);
            }
          });
        });
      }
    });
  }
  var send = root.__nautilusBridgeSend;
  if (typeof send !== 'function' || root.__bridgeCall) { return; }
  delete root.__nautilusBridgeSend;
  // The same bound as the worker's calls (bridge.js): a call the browser never
  // answers is refused, so no page call hangs.
  var CALL_TIMEOUT_MS = 10000;
  var next = 1;
  var waiting = new Map();
  var handlers = [];
  root.__bridgeCall = function (namespace, method, args) {
    return new Promise(function (resolve, reject) {
      var id = next++;
      var entry = { resolve: resolve, reject: reject, timer: null };
      entry.timer = setTimeout(function () {
        if (waiting.get(id) === entry) { waiting.delete(id); }
        reject(new Error('Nautilus: ' + method + ' was not answered.'));
      }, CALL_TIMEOUT_MS);
      waiting.set(id, entry);
      var text = JSON.stringify({ id: id, ns: namespace, method: method, args: args });
      // A call leaves once the page's own tab is reported (page_report.js).
      var wait = root.__nautilusReportWait;
      if (typeof wait === 'function') { wait(function () { send(text); }); } else { send(text); }
    });
  };
  root.__bridgeOnEvent = function (handler) { handlers.push(handler); };
  Object.defineProperty(root, '__nautilusBridgeDeliver', {
    value: function (text) {
      var message = JSON.parse(text);
      if (message.event) {
        handlers.forEach(function (handler) { handler(message.event, message.args); });
        return;
      }
      var entry = waiting.get(message.id);
      if (!entry) { return; }
      waiting.delete(message.id);
      clearTimeout(entry.timer);
      if (typeof message.error === 'string') {
        entry.reject(new Error(message.error));
      } else {
        entry.resolve(message.ok === undefined ? null : message.ok);
      }
    }
  });
})(self);
