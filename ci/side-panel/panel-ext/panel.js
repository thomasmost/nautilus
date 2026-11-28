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
  // What a panel opens is a tab of its window, and the panel gets no
  // handle to it.
  if (message && message.command === 'window.open') {
    const opened = window.open('https://example.net/?from-panel');
    record('panel-opened', { page: location.pathname.slice(1), handle: opened === null ? 'null' : typeof opened });
  }
});

function record(kind, data) {
  return fetch('http://127.0.0.1:47616/record', {
    method: 'POST',
    mode: 'no-cors',
    body: JSON.stringify({ kind, data }),
  }).catch(() => {});
}

// What this page's chrome.sidePanel is: the engine's object (frozen, a
// property of `chrome` that cannot be redefined) or Chromium's own.
function describe(context) {
  self.browserQuery().then((query) => record('browser-facts', { context, facts: self.browserFacts(), query }));
  const sp = chrome.sidePanel;
  const descriptor = Object.getOwnPropertyDescriptor(chrome, 'sidePanel') || {};
  record('sidepanel-info', {
    context,
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
}

chrome.runtime.onMessage.addListener((message) => {
  if (message && message.command === 'describe') describe(location.pathname.slice(1) + ', after another extension was installed');
});

// A debugger held by this page on a tab behind another (the worker asks
// for it): attach, a few DevTools commands each timed, what the worker can
// do with the same tab meanwhile, detach. The engine must have shown the
// tab by the time attach resolves, or each command takes seconds.
async function debuggerCheck(tabId) {
  const page = location.pathname.slice(1);
  const out = { page, tabId, installed: self.__nautilusBridge ? self.__nautilusBridge.installed : null, commands: [] };
  const timed = async (name, call) => {
    const started = performance.now();
    const outcome = await call().then(() => 'ok', (error) => 'rejected: ' + error.message);
    out.commands.push({ name, ms: Math.round(performance.now() - started), outcome });
  };
  try {
    await timed('attach', () => chrome.debugger.attach({ tabId }, '1.3'));
    for (let i = 0; i < 3; i += 1) {
      await timed('Input.dispatchMouseEvent', () =>
        chrome.debugger.sendCommand({ tabId }, 'Input.dispatchMouseEvent', { type: 'mouseMoved', x: 20 + i, y: 20 })
      );
    }
    for (let i = 0; i < 3; i += 1) {
      await timed('Runtime.evaluate', () =>
        chrome.debugger.sendCommand({ tabId }, 'Runtime.evaluate', { expression: '1 + 1', returnByValue: true })
      );
    }
    // The same tab from the worker, while this page holds the session.
    out.worker = await chrome.runtime.sendMessage({ debuggerProbe: tabId }).catch((error) => 'no answer: ' + error.message);
    await record('debugger-check', Object.assign({ phase: 'attached' }, out));
    // The harness reads the log while the session is held, then lets go.
    await chrome.runtime.sendMessage({ debuggerHeld: tabId }).catch(() => {});
    await timed('detach', () => chrome.debugger.detach({ tabId }));
  } catch (error) {
    out.error = String((error && error.message) || error);
  }
  await record('debugger-check', Object.assign({ phase: 'done' }, out));
  chrome.runtime.sendMessage({ debuggerDone: tabId }).catch(() => {});
}

chrome.runtime.onMessage.addListener((message) => {
  if (message && message.command === 'debugger-check') debuggerCheck(message.tabId);
});

// Attach and stay attached: the worker then closes this panel, so the page
// goes without a detach, and asks Chromium whether the session went with it.
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (!message || message.command !== 'debugger-attach-and-stay') return undefined;
  chrome.debugger.attach({ tabId: message.tabId }, '1.3').then(
    () => sendResponse('ok'),
    (error) => sendResponse('rejected: ' + error.message)
  );
  return true;
});

// A web page framed in this page (panel.html only): the suite's own page
// at the origin the manifest's externally_connectable names. It sends this
// extension external messages as it loads (ci/side-panel/external.html), and
// asks this page for the tab beside the panel, which this page reads through
// chrome.tabs.
const external = document.getElementById('external');
if (external) {
  window.addEventListener('message', async (event) => {
    if (event.source !== external.contentWindow || !event.data || event.data.type !== 'external-get-tab') return;
    let answer;
    try {
      const [active] = await chrome.tabs.query({ active: true, currentWindow: true });
      const tab = await chrome.tabs.get(active.id);
      answer = { ok: true, id: tab.id, same: tab.id === active.id, url: tab.url };
    } catch (error) {
      answer = { ok: false, error: String((error && error.message) || error) };
    }
    event.source.postMessage({ type: 'external-tab', answer }, event.origin);
  });
  external.src = `http://localhost:47616/external.html?where=panel&ext=${chrome.runtime.id}`;
}

(async () => {
  const page = location.pathname.slice(1);
  self.framesProbe();
  describe(page);
  chrome.runtime.sendMessage({ whoami: page }).catch(() => {});
  // Again, once the engine has had this page's report and told the worker
  // that this page's Chromium tab id is a panel's: the message the harness
  // judges (the first one races the report, and is only noted).
  setTimeout(() => chrome.runtime.sendMessage({ whoami: page, late: true }).catch(() => {}), 2500);
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
