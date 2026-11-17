// Nautilus helper. The engine evaluates nautilusHelper.targets() in this
// page: every DevTools target with Chromium's tab id, which the engine
// joins with the target id each of its tabs reports.
window.nautilusHelper = {
  targets: function () {
    return chrome.debugger.getTargets().then(function (targets) {
      return targets.map(function (t) {
        return { id: t.id, tabId: t.tabId === undefined ? null : t.tabId, type: t.type, url: t.url, title: t.title };
      });
    });
  },
};
