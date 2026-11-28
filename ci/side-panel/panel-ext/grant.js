// An extension page shown as a tab, for the groups run's question about
// docs/upstream-issues.md, C19: does an API granted to a live context
// through chrome.permissions.request appear in it? The request needs a user
// gesture. The worker gives this page one through chrome.debugger
// (Input.dispatchMouseEvent on the button, which fills the top left of the
// page), and this page asks in the click's handler, then tells the worker.
(() => {
  const what = { permissions: ['cookies', 'storage'] };
  const has = () => ({ cookies: typeof chrome.cookies, storage: typeof chrome.storage });
  document.getElementById('grant').addEventListener('click', async (event) => {
    const before = has();
    const outcome = await chrome.permissions.request(what).then(
      (granted) => ({ granted }),
      (error) => ({ error: String((error && error.message) || error) })
    );
    chrome.runtime.sendMessage({ grantAnswer: { trusted: event.isTrusted, before, after: has(), ...outcome } }).catch(() => {});
  });
  chrome.runtime.sendMessage({ grantReady: true }).catch(() => {});
})();
