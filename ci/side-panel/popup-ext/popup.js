// The popup's button: window.close(), which the bridge's script replaces
// in every bridged extension page with a question to the engine
// (sidePanel.closeSelf: false for a page that is no side panel) before
// Chromium's own close runs. The line in the browser's log says when the
// button was pressed; the popup should be gone at once.
document.getElementById('close').addEventListener('click', () => {
  console.log(`popup close asked at ${Date.now()}`);
  window.close();
});
