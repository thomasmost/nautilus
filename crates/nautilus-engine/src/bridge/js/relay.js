// Nautilus relay, protocol 1. One of the extension's own pages, loaded by
// the engine in a hidden browser. The engine calls window.nautilusRelay
// through DevTools (Runtime.evaluate with awaitPromise); this page talks
// to the extension's service worker over a runtime port, and connecting
// that port is what starts a stopped worker.
(function () {
  'use strict';
  var PORT = 'nautilus-bridge-1';
  var DELIVERY_TIMEOUT_MS = 10000;
  var IDENTIFY_TIMEOUT_MS = 5000;
  var port = null;
  var outbox = [];   // what the worker asked, until the engine takes it
  var waiter = null; // the engine's pending next()
  var deliveries = new Map(); // id -> {received, done, lost}
  var known = null;  // the hello of the worker on the current port
  var identifying = []; // identify() calls waiting for that hello

  function flush() {
    if (waiter && outbox.length) {
      var resolve = waiter;
      waiter = null;
      resolve(outbox.splice(0));
    }
  }

  function adopt(newPort) {
    port = newPort;
    known = null;
    newPort.onMessage.addListener(function (message) {
      if (!message) { return; }
      if (message.hello) {
        // The worker's identity, sent first on every port: the engine learns
        // of a restarted worker from it, whether or not the worker calls.
        known = message.hello;
        var waiting = identifying.splice(0);
        waiting.forEach(function (done) { done(known); });
        outbox.push({ hello: known }); flush();
        return;
      }
      if (message.call) { outbox.push({ call: message.call }); flush(); return; }
      var id = message.received !== undefined ? message.received : (message.done && message.done.id);
      var delivery = deliveries.get(id);
      if (!delivery) { return; }
      if (message.done) { delivery.done(message.done); } else { delivery.received(); }
    });
    newPort.onDisconnect.addListener(function () {
      void chrome.runtime.lastError;
      if (port === newPort) {
        port = null;
        known = null;
        // The worker went (it stopped, or the extension was reloaded): the
        // engine hears it, and asks which worker it reaches if it matters.
        outbox.push({ gone: true }); flush();
      }
      deliveries.forEach(function (delivery) { delivery.lost(newPort); });
    });
  }

  // The worker opens the port itself when it was started by something
  // else and needs the engine.
  chrome.runtime.onConnect.addListener(function (newPort) {
    if (newPort.name === PORT) { adopt(newPort); }
  });

  function connect() {
    if (!port) { adopt(chrome.runtime.connect({ name: PORT })); }
    return port;
  }

  window.nautilusRelay = {
    // Resolves with the worker's next requests: [{call: {id, method, args}}].
    next: function () {
      return new Promise(function (resolve) { waiter = resolve; flush(); });
    },

    // Which worker instance this relay reaches: {instance, attached} (the
    // worker's hello). A stopped worker is started by the connect. Rejects
    // when the worker does not say within IDENTIFY_TIMEOUT_MS.
    identify: function () {
      return new Promise(function (resolve, reject) {
        if (known) { resolve(known); return; }
        var timer = setTimeout(function () {
          identifying = identifying.filter(function (w) { return w !== done; });
          reject(new Error('the worker did not say who it is'));
        }, IDENTIFY_TIMEOUT_MS);
        function done(hello) { clearTimeout(timer); resolve(hello); }
        identifying.push(done);
        try {
          connect();
        } catch (e) {
          clearTimeout(timer);
          identifying = identifying.filter(function (w) { return w !== done; });
          reject(e);
        }
      });
    },

    // Whether this page's extension context is still live. A reload of the
    // extension leaves the page open with no port event, and its API calls
    // throw; the evaluation fails then, and the engine closes the page.
    alive: function () {
      if (!chrome.runtime || !chrome.runtime.id) { throw new Error('the extension context is invalidated'); }
      return !!chrome.runtime.getManifest();
    },

    // The engine's answer to call `id` of worker instance `instance`:
    // {result} or {error}. The worker refuses an instance other than its own.
    reply: function (id, answer, instance) {
      try {
        connect().postMessage({ reply: { id: id, instance: instance, result: answer.result, error: answer.error } });
        return true;
      } catch (e) {
        return false;
      }
    },

    // Hand {id, event, tabId} to the worker, starting it if it is
    // stopped. Sent again once, on a fresh port, only if the port went
    // before the worker acknowledged receipt: a delivery the worker has
    // acknowledged is never sent twice.
    deliver: function (delivery) {
      return new Promise(function (resolve) {
        var attempts = 0;
        var received = false;
        var sentOn = null;
        var timer = setTimeout(function () {
          finish({ ok: false, error: 'the worker did not finish in time' });
        }, DELIVERY_TIMEOUT_MS);
        function finish(outcome) {
          clearTimeout(timer);
          deliveries.delete(delivery.id);
          outcome.attempts = attempts;
          outcome.received = received;
          resolve(outcome);
        }
        function send() {
          attempts += 1;
          try {
            sentOn = connect();
            sentOn.postMessage({ deliver: delivery });
          } catch (e) {
            finish({ ok: false, error: String((e && e.message) || e) });
          }
        }
        deliveries.set(delivery.id, {
          received: function () { received = true; },
          done: function (done) { finish({ ok: !!done.ok, error: done.error }); },
          lost: function (lostPort) {
            if (lostPort !== sentOn) { return; }
            if (received) { finish({ ok: false, error: 'the worker stopped before it finished' }); }
            else if (attempts < 2) { send(); }
            else { finish({ ok: false, error: 'the worker did not take the delivery' }); }
          },
        });
        send();
      });
    },
  };
})();
