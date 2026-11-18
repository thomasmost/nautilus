// The body of the deferred install of an extension page's bridge. It runs
// inside a function of two parameters: `root`, the global object, and
// `install`, the function that replaces the namespaces. The render process
// creates the page's context before Chromium defines `chrome` in it, so
// `install` waits for the first change to the document: the parser delivers
// that change before it runs the page's first script, and by then `chrome`
// exists. The observer disconnects at its first delivery, so `install` runs
// once.
if (!root.document || typeof root.MutationObserver !== 'function') {
  console.error('nautilus-bridge: no document to wait for in this context');
  return;
}
var observer = new root.MutationObserver(function () {
  observer.disconnect();
  install();
});
observer.observe(root.document, { childList: true, subtree: true });
