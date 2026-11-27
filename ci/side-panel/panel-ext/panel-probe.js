// Reports a panel page's state to the side panel harness every half
// second: the page's name, its width and whether it is visible. A page
// that stops reporting has closed. (A hidden page's timers run once a
// second, so the harness allows for that.) panel_checks.py copies this
// file next to each cookbook sample's panel page, so it uses nothing of
// the test extension.
(function () {
  'use strict';
  const page = location.pathname.slice(1);
  setInterval(() => {
    const body = JSON.stringify({
      kind: 'panel',
      data: { page, width: innerWidth, height: innerHeight, visibility: document.visibilityState },
    });
    fetch('http://127.0.0.1:47616/record', { method: 'POST', mode: 'no-cors', body }).catch(() => {});
  }, 500);
})();
