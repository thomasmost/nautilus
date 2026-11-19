// chrome.windows, replaced. The lists are read by a test in
// bridge/mod.rs: keep each on one line.
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  if (!bridge || bridge.installed.indexOf('windows') !== -1) { return; }
  bridge.define('windows', {
    bridged: ['get', 'getAll', 'getCurrent', 'getLastFocused', 'create', 'update', 'remove'],
    optional: [],
    direct: [],
    events: ['onCreated', 'onRemoved', 'onFocusChanged'],
    nativeEvents: []
  });
})(self);
