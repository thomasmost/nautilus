// The page's own Chromium tab id. It runs in the deferred install, after the
// namespaces have replaced Chromium's: the core hands this script Chromium's
// `tabs.getCurrent`, once (`takeCurrentTab`, which is gone when taken).
// The id goes to the browser process as a report, and a call of the page
// waits for the report (page_transport.js), so the browser handles the report
// before any call. The wait is bounded: a page that is no tab, an answer that
// fails, or an answer that never comes releases the calls after REPORT_WAIT_MS.
(function (root) {
  'use strict';
  var REPORT_WAIT_MS = 1000;
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
  var timer = setTimeout(release, REPORT_WAIT_MS);
  function settle() { clearTimeout(timer); release(); }
  if (typeof take !== 'function') { settle(); return; }
  var current;
  // Taken even when there is nobody to report to, so it is never left.
  try { current = take(); } catch (e) { settle(); return; }
  if (!current || typeof report !== 'function') { settle(); return; }
  try {
    Promise.resolve(current).then(function (tab) {
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
