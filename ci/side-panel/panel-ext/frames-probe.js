// Reads what a frame of this extension page can reach (docs/upstream-issues.md,
// C11 and C19; the bridge goes into an extension's pages only at a
// chrome-extension:// address). The page makes an about:blank frame and a
// srcdoc frame, and reports what each frame's `chrome`, `browser` and the
// namespaces are: their types, whether a function is Chromium's own
// ("native") or a script's (the bridge's), and whether the bridge's global is
// there. Nothing is called: this only reads properties, so a path to
// Chromium's own tab code is shown, never taken. Reported as 'frames-probe'.
(function (root) {
  'use strict';
  const NAMESPACES = ['tabs', 'tabGroups', 'windows', 'commands', 'sidePanel', 'debugger'];

  function kind(value) {
    if (typeof value !== 'function') return typeof value;
    try {
      return /\[native code\]/.test(Function.prototype.toString.call(value)) ? 'native' : 'script';
    } catch (error) {
      return 'unreadable: ' + String((error && error.message) || error);
    }
  }

  function describe(win) {
    const out = { chrome: 'unreadable' };
    try {
      const c = win.chrome;
      out.chrome = typeof c;
      if (!c || typeof c !== 'object') return out;
      out.bridge = typeof win.__nautilusBridge;
      out.namespaces = {};
      for (const name of NAMESPACES) {
        const ns = c[name];
        out.namespaces[name] = typeof ns;
      }
      out.tabsGroup = kind(c.tabs && c.tabs.group);
      out.tabsQuery = kind(c.tabs && c.tabs.query);
      out.tabGroupsUpdate = kind(c.tabGroups && c.tabGroups.update);
      out.sidePanelOpen = kind(c.sidePanel && c.sidePanel.open);
      out.runtimeSendMessage = kind(c.runtime && c.runtime.sendMessage);
      out.browser = typeof win.browser;
    } catch (error) {
      out.error = String((error && error.message) || error);
    }
    return out;
  }

  function probe(kindName, src) {
    const frame = document.createElement('iframe');
    frame.setAttribute('data-probe', kindName);
    if (src !== null) frame.srcdoc = src;
    document.body.appendChild(frame);
    return frame;
  }

  function report(data) {
    return fetch('http://127.0.0.1:47616/record', {
      method: 'POST',
      mode: 'no-cors',
      body: JSON.stringify({ kind: 'frames-probe', data }),
    }).catch(() => {});
  }

  // What Chromium does with runtime.sendMessage's bad argument shapes: each
  // is refused before anything is sent (no extension id is valid here, and a
  // shape Chromium takes as a message is not among them). Reported as
  // 'send-shapes', after the page's hold has ended.
  function sendShapes() {
    const id = 'abcdefghijklmnopabcdefghijklmnop';
    const cases = {
      noArguments: [],
      numberIdTwoArguments: [42, 'x'],
      objectIdTwoArguments: [{}, 'x'],
      stringOptions: [id, 'x', 'nope'],
      numberOptions: [id, 'x', 42],
      tooManyArguments: [id, 'x', null, () => {}, 'extra'],
      numberOptionsWithCallback: [id, 'x', 42, () => {}],
    };
    const out = {};
    for (const name of Object.keys(cases)) {
      try {
        const answer = chrome.runtime.sendMessage(...cases[name]);
        if (answer && typeof answer.then === 'function') {
          answer.then(() => {}, () => {});
          out[name] = 'returned a promise';
        } else {
          out[name] = 'returned ' + typeof answer;
        }
      } catch (error) {
        out[name] = 'threw ' + ((error && error.constructor && error.constructor.name) || '?') + ': ' + String((error && error.message) || error);
      }
    }
    return out;
  }

  root.framesProbe = function () {
    const page = location.pathname.slice(1);
    const blank = probe('about:blank', null);
    const srcdoc = probe('srcdoc', '<!doctype html><p>srcdoc</p>');
    const frames = { page, parentUrl: location.href, parent: describe(window) };
    setTimeout(() => {
      frames.blankUrl = blank.contentDocument ? blank.contentDocument.URL : 'no document';
      frames.srcdocUrl = srcdoc.contentDocument ? srcdoc.contentDocument.URL : 'no document';
      frames.blank = describe(blank.contentWindow);
      frames.srcdoc = describe(srcdoc.contentWindow);
      frames.blankSameAsParent = blank.contentWindow.chrome === window.chrome;
      report(frames);
      setTimeout(() => {
        fetch('http://127.0.0.1:47616/record', {
          method: 'POST',
          mode: 'no-cors',
          body: JSON.stringify({ kind: 'send-shapes', data: { page, shapes: sendShapes() } }),
        }).catch(() => {});
      }, 4000);
      // Removed: an extension's frame is a context of its own in getContexts.
      blank.remove();
      srcdoc.remove();
    }, 500);
  };
})(self);
