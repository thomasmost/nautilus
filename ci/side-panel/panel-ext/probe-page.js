// The test extension's content script: every half second it sends the
// top page's address, width and visibility to the worker, which posts
// them to the harness as `tab` records. The width is the one the page is
// laid out at, so a panel taking its share of the window shows as a
// narrower page.
(function () {
  'use strict';
  if (window !== window.top) return;
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
