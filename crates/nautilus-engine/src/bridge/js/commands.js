// chrome.commands, replaced: getAll is the manifest's commands with the
// shortcuts they declare, and onCommand fires when the browser resolves
// one of them. Only where Chromium defines the namespace, so an extension
// that declares no commands gains none. The lists are read by a test in
// bridge/mod.rs: keep each on one line.
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  if (!bridge || !root.chrome.commands || bridge.installed.indexOf('commands') !== -1) { return; }
  bridge.define('commands', {
    bridged: ['getAll'],
    optional: [],
    direct: [],
    events: ['onCommand'],
    nativeEvents: []
  });
})(self);
