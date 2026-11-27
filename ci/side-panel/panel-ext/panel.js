// The side panel test pages' script. The pointer and key lines are for
// the checks by hand (side-panel.AC1.2, AC1.3). The suite's command makes
// the page close itself (AC5.2); the page's own questions to the bridge
// are reported once, at load, for the harness to compare.
const pointer = document.getElementById('pointer');
const keys = document.getElementById('keys');

document.addEventListener('mousemove', (event) => {
  pointer.textContent = `${event.clientX}, ${event.clientY} of ${innerWidth} x ${innerHeight}`;
});
document.addEventListener('mouseleave', () => {
  pointer.textContent = '(not over the panel)';
});
document.addEventListener('mousedown', (event) => {
  console.log(`panel mousedown ${event.clientX},${event.clientY} focus=${document.hasFocus()}`);
});
document.addEventListener('keydown', (event) => {
  keys.textContent += (keys.textContent ? ' ' : '') + event.key;
  console.log(`panel keydown ${event.key}`);
});
document.getElementById('close').addEventListener('click', () => window.close());

chrome.runtime.onMessage.addListener((message) => {
  if (message && message.command === 'window.close') window.close();
});

function record(kind, data) {
  return fetch('http://127.0.0.1:47616/record', {
    method: 'POST',
    mode: 'no-cors',
    body: JSON.stringify({ kind, data }),
  }).catch(() => {});
}

(async () => {
  const page = location.pathname.slice(1);
  const sp = chrome.sidePanel;
  const descriptor = Object.getOwnPropertyDescriptor(chrome, 'sidePanel') || {};
  record('sidepanel-info', {
    context: page,
    info: {
      present: !!sp,
      frozen: !!sp && Object.isFrozen(sp),
      keys: sp ? Object.keys(sp).sort() : [],
      property: {
        data: 'value' in descriptor,
        writable: descriptor.writable === true,
        configurable: descriptor.configurable === true,
        getter: typeof descriptor.get,
      },
      bridge: typeof self.__nautilusBridge,
      installed: self.__nautilusBridge ? self.__nautilusBridge.installed : null,
    },
  });
  try {
    const current = await chrome.tabs.query({ active: true, currentWindow: true });
    const self_ = await chrome.tabs.getCurrent();
    const windowIds = current.map((t) => t.windowId);
    const contexts = await chrome.runtime.getContexts({ windowIds, contextTypes: ['SIDE_PANEL'] });
    record('panel-query', {
      page,
      currentTabs: current.map((t) => t.id),
      getCurrent: self_ === undefined,
      windowContexts: contexts.length,
    });
  } catch (error) {
    record('panel-query', { page, error: String((error && error.message) || error) });
  }
  console.log(`panel loaded ${location.pathname} ${innerWidth}x${innerHeight} ${document.visibilityState}`);
})();
