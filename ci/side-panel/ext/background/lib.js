// Shared by the test extension's worker: reporting to the driver.
const DRIVER = 'http://127.0.0.1:47613';
const CALL_TIMEOUT_MS = 20000;

function post(kind, data) {
  return fetch(DRIVER + '/r', { method: 'POST', body: JSON.stringify({ t: Date.now(), kind, data }) }).catch(() => {});
}

function config() {
  return fetch(DRIVER + '/cfg').then((r) => r.json()).catch(() => ({}));
}

function slim(tab) {
  return tab && { id: tab.id, windowId: tab.windowId, index: tab.index, active: tab.active, url: tab.url, title: tab.title };
}

// Run one API call, report how long it took and what it gave.
async function timed(run, name, fn, extra) {
  const started = Date.now();
  const record = Object.assign({ run, name }, extra || {});
  try {
    record.result = await Promise.race([
      Promise.resolve().then(fn),
      new Promise((_, reject) => setTimeout(() => reject(new Error('TIMEOUT after ' + CALL_TIMEOUT_MS + ' ms')), CALL_TIMEOUT_MS)),
    ]);
    record.ok = true;
  } catch (e) {
    record.ok = false;
    record.error = String((e && e.message) || e);
  }
  record.ms = Date.now() - started;
  await post('call', record);
  return record;
}
