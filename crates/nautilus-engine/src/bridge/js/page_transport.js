// The transport for extension pages (popups, pages shown as tabs): the
// render process bound __nautilusBridgeSend(text) on the global object;
// the browser process answers by evaluating __nautilusBridgeDeliver(text)
// in this frame.
(function (root) {
  'use strict';
  var send = root.__nautilusBridgeSend;
  if (typeof send !== 'function' || root.__bridgeCall) { return; }
  delete root.__nautilusBridgeSend;
  var next = 1;
  var waiting = new Map();
  var handlers = [];
  root.__bridgeCall = function (namespace, method, args) {
    return new Promise(function (resolve, reject) {
      var id = next++;
      waiting.set(id, { resolve: resolve, reject: reject });
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
      if (typeof message.error === 'string') {
        entry.reject(new Error(message.error));
      } else {
        entry.resolve(message.ok === undefined ? null : message.ok);
      }
    }
  });
})(self);
