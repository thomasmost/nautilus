// Fail closed. The first script of every bridged context, a page's and a
// worker's: it defines the refusal the others call when a namespace cannot
// be put in (core.js, page_defer.js, the guard around each namespace script).
//
// __nautilusRefuseNative(names) replaces Chromium's namespaces of those names
// by ones whose every call is refused, so no native function of theirs stays
// callable (a native tabs.group ends the browser process). Without `names` it
// is all of tabs, windows, tabGroups, commands and sidePanel. On `chrome` and
// on `browser`, the second global Chromium 156 reaches the same APIs through,
// with one refusing object for both; pinned, as a replaced namespace is, so
// that Chromium does not put its own back when the set of extensions changes.
// tabGroups, commands and sidePanel only where the context has them: an
// extension without the permission gains no namespace.
//
// What is in the way decides how: a property that can be redefined is; one
// that cannot, but can be assigned, is; where neither, every function and
// event of the object it holds is replaced in place; and where one of those
// cannot be replaced either (a frozen object), the global itself (`chrome`,
// `browser`) is replaced by a stand-in that answers the refusal under that
// name and Chromium's own object for every other.
(function (root) {
  'use strict';
  if (Object.prototype.hasOwnProperty.call(root, '__nautilusRefuseNative')) { return; }
  var ALL = ['tabs', 'windows', 'tabGroups', 'commands', 'sidePanel'];
  var GLOBALS = ['chrome', 'browser'];
  // The globals this file replaced: name -> { holder, hidden }.
  var masked = Object.create(null);

  function isObject(value) {
    return !!value && (typeof value === 'object' || typeof value === 'function');
  }

  function place() {
    return typeof root.WorkerGlobalScope === 'function' ? 'worker' : 'page';
  }

  function refusedFunction(name, key) {
    return function () {
      return Promise.reject(new Error(name + '.' + key + ' is not supported in this ' + place() + '.'));
    };
  }

  // An event that takes listeners and never fires, so that an extension's
  // first evaluation does not throw on `onUpdated.addListener`.
  function inertEvent() {
    return {
      addListener: function () {},
      removeListener: function () {},
      hasListener: function () { return false; },
      hasListeners: function () { return false; }
    };
  }

  function refusing(name) {
    var events = Object.create(null);
    return new Proxy({}, {
      get: function (target, key) {
        if (typeof key !== 'string' || key === 'then') { return undefined; }
        if (/^on[A-Z]/.test(key)) {
          if (!events[key]) { events[key] = inertEvent(); }
          return events[key];
        }
        return refusedFunction(name, key);
      }
    });
  }

  // Chromium's object under a global, whether or not the global was replaced.
  function holderOf(globalName) {
    if (masked[globalName]) { return masked[globalName].holder; }
    var holder;
    try { holder = root[globalName]; } catch (e) { return null; }
    return isObject(holder) ? holder : null;
  }

  function read(object, key) {
    try { return { value: object[key] }; } catch (e) { return null; }
  }

  // The global `globalName` answers `refused` under `name` from now on, and
  // its own object for every other name. The stand-in's target is an empty
  // object, since a proxy may not answer anything but the value of a target's
  // property that cannot be changed. Chromium defines nothing on a stand-in
  // (a proxy takes no lazy property), so a context this happens in gains no
  // API later; it is the last resort.
  function mask(globalName, name, refused) {
    var entry = masked[globalName];
    if (entry) { entry.hidden[name] = refused; return; }
    var holder = holderOf(globalName);
    var hidden = Object.create(null);
    hidden[name] = refused;
    var standIn = new Proxy({}, {
      get: function (target, key) { return key in hidden ? hidden[key] : holder[key]; },
      set: function (target, key, value) { return key in hidden ? false : Reflect.set(holder, key, value); },
      has: function (target, key) { return key in hidden || key in holder; },
      deleteProperty: function (target, key) { return key in hidden ? false : Reflect.deleteProperty(holder, key); },
      defineProperty: function (target, key, descriptor) {
        return key in hidden ? false : Reflect.defineProperty(holder, key, descriptor);
      },
      getOwnPropertyDescriptor: function (target, key) {
        var descriptor = key in hidden
          ? { value: hidden[key], writable: false, enumerable: true }
          : Reflect.getOwnPropertyDescriptor(holder, key);
        if (descriptor) { descriptor.configurable = true; }
        return descriptor;
      },
      ownKeys: function () { return Reflect.ownKeys(holder); }
    });
    try {
      Object.defineProperty(root, globalName, { value: standIn, enumerable: true, configurable: true, writable: true });
      masked[globalName] = { holder: holder, hidden: hidden };
    } catch (e) {
      console.error('nautilus-bridge: cannot refuse ' + globalName + '.' + name + ': ' + e);
    }
  }

  function refuseOn(globalName, holder, name, refused) {
    if (masked[globalName] && name in masked[globalName].hidden) { return; }
    try {
      Object.defineProperty(holder, name, { value: refused, enumerable: true, writable: false, configurable: false });
      return;
    } catch (e) { /* the property cannot be redefined */ }
    try { holder[name] = refused; } catch (e) { /* nor assigned */ }
    var held = read(holder, name);
    // A property that cannot be read holds nothing that can be called.
    if (!held || held.value === refused || !isObject(held.value)) { return; }
    var own = held.value;
    var left = false;
    Object.getOwnPropertyNames(own).forEach(function (key) {
      var member = read(own, key);
      if (!member) { return; }
      var standIn;
      if (typeof member.value === 'function') { standIn = refusedFunction(name, key); }
      else if (isObject(member.value) && typeof member.value.addListener === 'function') { standIn = inertEvent(); }
      else { return; }
      try {
        Object.defineProperty(own, key, { value: standIn, enumerable: true, writable: false, configurable: false });
      } catch (e) {
        try { own[key] = standIn; } catch (ignored) { /* nor assigned */ }
      }
      var now = read(own, key);
      if (now && now.value !== standIn) { left = true; }
    });
    if (left) { mask(globalName, name, refused); }
  }

  Object.defineProperty(root, '__nautilusRefuseNative', {
    configurable: true,
    value: function (names) {
      var holders = [];
      GLOBALS.forEach(function (globalName) {
        var holder = holderOf(globalName);
        if (holder && !holders.some(function (known) { return known.holder === holder; })) {
          holders.push({ globalName: globalName, holder: holder });
        }
      });
      (Array.isArray(names) ? names : ALL).forEach(function (name) {
        var has = holders.some(function (known) {
          var held = read(known.holder, name);
          return !held || !!held.value;
        });
        if (name !== 'tabs' && name !== 'windows' && !has) { return; }
        var refused = refusing(name);
        holders.forEach(function (known) { refuseOn(known.globalName, known.holder, name, refused); });
      });
    }
  });
})(self);
