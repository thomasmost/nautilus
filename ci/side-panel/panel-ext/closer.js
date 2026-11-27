// An extension page that is no side panel, shown as a tab. The bridge's
// script replaces window.close in it, as in an action popup's page, with
// a question the engine answers first (sidePanel.closeSelf: false for a
// page that is no panel) before Chromium's own close runs. This page asks
// the same question twice, timed, reports the answers, and then calls
// window.close() itself; the worker reports when its tab went.
(async () => {
  const record = (kind, data) =>
    fetch('http://127.0.0.1:47616/record', {
      method: 'POST',
      mode: 'no-cors',
      body: JSON.stringify({ kind, data }),
    }).catch(() => {});
  const bridge = self.__nautilusBridge;
  const asks = [];
  if (bridge && typeof bridge.call === 'function') {
    for (let i = 0; i < 2; i += 1) {
      const start = performance.now();
      const answer = await bridge.call('sidePanel', 'closeSelf', []).then(
        (value) => ({ value }),
        (error) => ({ error: String((error && error.message) || error) })
      );
      asks.push({ ms: Math.round(performance.now() - start), ...answer });
    }
  }
  await record('closer', {
    bridge: typeof bridge,
    native: /\[native code\]/.test(Function.prototype.toString.call(window.close)),
    asks,
    closing: Date.now(),
  });
  window.close();
})();
