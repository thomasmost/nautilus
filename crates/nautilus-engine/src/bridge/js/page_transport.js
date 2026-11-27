// The transport for extension pages (popups, pages shown as tabs): the
// render process bound __nautilusBridgeSend(text) on the global object;
// the browser process answers by evaluating __nautilusBridgeDeliver(text)
// in this frame. The refusal (refuse.js) is defined before this runs, so a
// page with no send has it all the same.
(function (root) {
  'use strict';
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
