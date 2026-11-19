// The page's own Chromium tab id. It runs in the deferred install, after the
// namespaces have replaced Chromium's: the functions they replaced are kept
// in `native` (core.js), so Chromium's `tabs.getCurrent` is still reachable.
// The id goes to the browser process as a report, and a call of the page
// waits for the report (page_transport.js), so the browser handles the report
// before any call. The wait is bounded: a page that is no tab, an answer that
// fails, or an answer that never comes releases the calls after REPORT_WAIT_MS.
(function (root) {
  'use strict';
  var REPORT_WAIT_MS = 1000;
  var native = root.__nautilusBridge && root.__nautilusBridge.native;
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
  var timer = setTimeout(release, REPORT_WAIT_MS);
  function settle() { clearTimeout(timer); release(); }
  var getCurrent = native && native.tabs && native.tabs.getCurrent;
  if (typeof getCurrent !== 'function' || typeof report !== 'function') { settle(); return; }
  try {
    Promise.resolve(getCurrent.call(native.tabs)).then(function (tab) {
      if (tab && typeof tab.id === 'number') {
        try { report(JSON.stringify({ tab: tab.id })); }
        catch (e) { console.error('nautilus-bridge: ' + e); }
      }
      settle();
    }, settle);
  } catch (e) {
    settle();
  }
})(self);
