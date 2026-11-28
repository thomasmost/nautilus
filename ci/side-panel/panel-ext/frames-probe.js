// Reads what a frame of this extension page can reach (docs/upstream-issues.md,
// C11 and C19; the bridge goes into an extension's pages only at a
// chrome-extension:// address). The page makes an about:blank frame (with no
// src, and with src="about:blank"), a blank frame inside the blank frame, and
// a srcdoc frame, and reports what each frame's `chrome`, `browser` and the
// namespaces are: their types, whether a function is Chromium's own
// ("native") or a script's (the bridge's), and whether the bridge's global is
// there. Nothing is called: this only reads properties, so a path to
// Chromium's own tab code is shown, never taken. Reported as 'frames-probe'.
// closer.html loads this file too, for describeFrame (a window.open popup).
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
  root.describeFrame = describe;

  // A frame appended to `parent` (the document, or a blank frame's body).
  // `attrs`: 'src' (set when given), or 'srcdoc' (set when given), or neither.
  function probe(parent, kindName, attrs) {
    const frame = parent.createElement('iframe');
    frame.setAttribute('data-probe', kindName);
    if (attrs.src !== undefined) frame.setAttribute('src', attrs.src);
    if (attrs.srcdoc !== undefined) frame.srcdoc = attrs.srcdoc;
    parent.body.appendChild(frame);
    return frame;
  }

  function report(data) {
    return fetch('http://127.0.0.1:47616/record', {
      method: 'POST',
      mode: 'no-cors',
      body: JSON.stringify({ kind: 'frames-probe', data }),
    }).catch(() => {});
  }

  // runtime.sendMessage's argument shapes, measured by calling Chromium's own
  // function. The shapes are grouped by what Chromium does with them (the
  // judge reads these groups):
  //   accepted      the forms the signature takes, with the extension's own
  //                 id, and null and undefined in each optional position; each
  //                 message is one of five kinds. Chromium sends each.
  //   readAsId      a string first with more than one argument left: the
  //                 string is an extension id, checked by Chromium, so the
  //                 error (or the send) is Chromium's ("Invalid extension id").
  //   threeArguments  a message that is not a string or null, with options and
  //                 a null or undefined callback: three arguments, the first of
  //                 which is the extension id, so Chromium refuses the shape.
  //   shapes        the refused classes, one shape each (the refusal rule's).
  // The messages are of a type the extension's listeners ignore, and every
  // promise is caught. Reported as 'send-shapes' (after the hold) and as
  // 'hold-shapes' (during it, as the page loads).
  const FORMS = [
    { form: 'message', id: false, options: false, callback: false },
    { form: 'message, callback', id: false, options: false, callback: true },
    { form: 'message, options', id: false, options: true, callback: false },
    { form: 'message, options, callback', id: false, options: true, callback: true },
    { form: 'id, message', id: true, options: false, callback: false },
    { form: 'id, message, callback', id: true, options: false, callback: true },
    { form: 'id, message, options', id: true, options: true, callback: false },
    { form: 'id, message, options, callback', id: true, options: true, callback: true },
  ];
  const MESSAGES = {
    string: 'probe-string',
    number: 42,
    null: null,
    array: ['probe-array'],
    object: { type: 'probe-object' },
  };
  function attempt(args) {
    try {
      const answer = chrome.runtime.sendMessage(...args);
      if (answer && typeof answer.then === 'function') {
        answer.then(() => {}, () => {});
        return 'returned a promise';
      }
      return 'returned ' + typeof answer;
    } catch (error) {
      return 'threw ' + ((error && error.constructor && error.constructor.name) || '?') + ': ' + String((error && error.message) || error);
    }
  }
  function shapeGroups() {
    const own = chrome.runtime.id;
    const badId = 'abcdefghijklmnopabcdefghijklmnop';
    const fn = () => {};
    const groups = { accepted: {}, readAsId: {}, threeArguments: {}, shapes: {} };
    const label = (v) => (v === undefined ? 'undefined' : v === null ? 'null' : typeof v === 'object' ? 'object' : typeof v);
    for (const spec of FORMS) {
      const ids = spec.id ? [own, null, undefined] : [undefined];
      const opts = spec.options ? [{}, null, undefined] : [undefined];
      const cbs = spec.callback ? [fn, null, undefined] : [undefined];
      for (const mName of Object.keys(MESSAGES)) {
        for (const idv of ids) {
          for (const optv of opts) {
            for (const cbv of cbs) {
              const args = [];
              if (spec.id) args.push(idv);
              args.push(MESSAGES[mName]);
              if (spec.options) args.push(optv);
              if (spec.callback) args.push(cbv);
              const key = [spec.form, 'message ' + mName];
              if (spec.id) key.push('id ' + (idv === own ? 'own' : label(idv)));
              if (spec.options) key.push('options ' + label(optv));
              if (spec.callback) key.push('callback ' + (cbv === fn ? 'function' : label(cbv)));
              const hasCallback = cbv === fn;
              const left = hasCallback ? args.length - 1 : args.length;
              const message = MESSAGES[mName];
              let group = 'accepted';
              if (!spec.id && typeof message === 'string' && left >= 2) group = 'readAsId';
              else if (!spec.id && spec.options && !hasCallback && left === 3 && message !== null) group = 'threeArguments';
              groups[group][key.join(' | ')] = args;
            }
          }
        }
      }
    }
    groups.shapes = {
      noArguments: [],
      onlyCallback: [fn],
      numberIdTwoArguments: [42, 'x'],
      objectIdTwoArguments: [{}, 'x'],
      booleanIdTwoArguments: [true, 'x'],
      arrayIdTwoArguments: [['a'], 'x'],
      functionIdTwoArguments: [fn, 'x'],
      objectSecondTwoArguments: [{}, 42],
      stringOptions: [badId, 'x', 'nope'],
      numberOptions: [badId, 'x', 42],
      booleanOptions: [badId, 'x', true],
      numberIdThreeArguments: [42, 'x', {}],
      objectIdThreeArguments: [{}, 'x', {}],
      booleanIdThreeArguments: [true, 'x', {}],
      functionIdThreeArguments: [fn, 'x', {}],
      nullIdNumberOptions: [null, 'x', 42],
      undefinedIdStringOptions: [undefined, 'x', 'nope'],
      fourthNotCallback: [badId, 'x', {}, 'extra'],
      numberOptionsWithNullCallback: [badId, 'x', 42, null],
      tooManyArguments: [badId, 'x', null, fn, 'extra'],
      tooManyWithNullCallback: [badId, 'x', {}, null, null],
      numberOptionsWithCallback: [badId, 'x', 42, fn],
    };
    return groups;
  }
  // Every group, each shape attempted, as { group: { key: answer } }.
  function attemptAll() {
    const groups = shapeGroups();
    const out = {};
    for (const name of Object.keys(groups)) {
      out[name] = {};
      for (const key of Object.keys(groups[name])) out[name][key] = attempt(groups[name][key]);
    }
    return out;
  }

  // During the hold: called as the page loads, before the engine has answered
  // its report, so the calls are the bridge's held ones. Each shape is
  // answered by the bridge's hold (the same rule Chromium's function has). The
  // tagged forms are held and reach the worker (sw.js, 'hold-shape') with
  // their message, in order.
  function holdShapes() {
    const own = chrome.runtime.id;
    const fn = () => {};
    const msg = (tag) => ({ type: 'hold-probe', tag, page: location.pathname.slice(1) });
    const tagged = {
      'message': [msg('message')],
      'message, callback': [msg('message, callback'), fn],
      'id, message': [own, msg('id, message')],
      'id, message, options': [own, msg('id, message, options'), {}],
      'id, message, options, null callback': [own, msg('id, message, options, null callback'), {}, null],
      'id, message, options, callback': [own, msg('id, message, options, callback'), {}, fn],
    };
    const taggedAnswers = {};
    for (const name of Object.keys(tagged)) taggedAnswers[name] = attempt(tagged[name]);
    return Object.assign({ tagged: taggedAnswers }, attemptAll());
  }

  root.framesProbe = function () {
    const page = location.pathname.slice(1);
    const during = holdShapes();
    fetch('http://127.0.0.1:47616/record', {
      method: 'POST',
      mode: 'no-cors',
      body: JSON.stringify({ kind: 'hold-shapes', data: Object.assign({ page }, during) }),
    }).catch(() => {});
    const blank = probe(document, 'about:blank', {});
    const blankSrc = probe(document, 'about:blank-src', { src: 'about:blank' });
    const srcdoc = probe(document, 'srcdoc', { srcdoc: '<!doctype html><p>srcdoc</p>' });
    const frames = { page, parentUrl: location.href, parent: describe(window) };
    setTimeout(() => {
      // A blank frame inside the blank frame: made once the blank frame's
      // document exists, which an about:blank frame has at once.
      const nested = probe(blank.contentDocument, 'nested-blank', {});
      setTimeout(() => {
        const urlOf = (f) => (f.contentDocument ? f.contentDocument.URL : 'no document');
        frames.blankUrl = urlOf(blank);
        frames.blankSrcUrl = urlOf(blankSrc);
        frames.srcdocUrl = urlOf(srcdoc);
        frames.nestedUrl = urlOf(nested);
        frames.blank = describe(blank.contentWindow);
        frames.blankSrc = describe(blankSrc.contentWindow);
        frames.srcdoc = describe(srcdoc.contentWindow);
        frames.nested = describe(nested.contentWindow);
        frames.blankSameAsParent = blank.contentWindow.chrome === window.chrome;
        report(frames);
        setTimeout(() => {
          fetch('http://127.0.0.1:47616/record', {
            method: 'POST',
            mode: 'no-cors',
            body: JSON.stringify({ kind: 'send-shapes', data: Object.assign({ page }, attemptAll()) }),
          }).catch(() => {});
        }, 4000);
        // Removed: an extension's frame is a context of its own in getContexts.
        for (const f of [blank, blankSrc, srcdoc]) f.remove();
      }, 300);
    }, 500);
  };
})(self);
