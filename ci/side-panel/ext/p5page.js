// An extension page shown as a tab: what the bridge is to a page.
(async function () {
  var out = {
    installed: (self.__nautilusBridge && self.__nautilusBridge.installed) || null,
    native: typeof chrome.tabs.sendMessage === 'function' && chrome.tabs.TAB_ID_NONE === -1
  };
  try {
    var started = performance.now();
    var me = await chrome.tabs.getCurrent();
    out.getCurrentMs = Math.round(performance.now() - started);
    out.current = me && { id: me.id, index: me.index, windowId: me.windowId, active: me.active };
    out.activeCurrent = (await chrome.tabs.query({ active: true, currentWindow: true })).map(function (t) { return t.id; });
    out.window = (await chrome.windows.getCurrent()).id;
    out.group = await chrome.tabGroups.move(1, { index: 0 }).then(function () { return 'resolved'; }, function (e) { return e.message; });
    var b = self.browser;
    out.browser = !b ? null : {
      tabs: b.tabs === chrome.tabs,
      windows: b.windows === chrome.windows,
      tabGroups: b.tabGroups === chrome.tabGroups,
      current: await b.tabs.getCurrent().then(function (tab) { return tab && tab.id; }, function (e) { return e.message; })
    };
  } catch (e) {
    out.error = String((e && e.message) || e);
  }
  chrome.runtime.sendMessage({ p5page: out });
})();
