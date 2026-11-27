// The test extension's content script: every half second it sends the
// top page's address, width and visibility to the worker, which posts
// them to the harness as `tab` records. The width is the one the page is
// laid out at, so a panel taking its share of the window shows as a
// narrower page.
(function () {
  'use strict';
  if (window !== window.top) return;
  // Once: what this isolated world has of `browser` and of the namespaces
  // the bridge replaces in the extension's own contexts (none is expected).
  chrome.runtime
    .sendMessage({ report: { kind: 'browser-facts', data: { context: 'content script', facts: self.browserFacts(), query: null } } })
    .catch(() => {});
  setInterval(() => {
    chrome.runtime
      .sendMessage({
        report: {
          kind: 'tab',
          data: { url: location.href, width: innerWidth, visibility: document.visibilityState },
        },
      })
      .catch(() => {});
  }, 500);
})();
