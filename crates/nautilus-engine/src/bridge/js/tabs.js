// chrome.tabs, replaced. The lists are read by a test in bridge/mod.rs:
// keep each on one line.
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  if (!bridge || bridge.installed.indexOf('tabs') !== -1) { return; }
  bridge.define('tabs', {
    bridged: ['query', 'get', 'getCurrent', 'create', 'duplicate', 'remove', 'update', 'move', 'highlight', 'discard', 'group', 'ungroup', 'captureVisibleTab', 'reload', 'goBack', 'goForward', 'getZoom', 'setZoom', 'getZoomSettings', 'setZoomSettings', 'detectLanguage'],
    optional: ['executeScript', 'insertCSS', 'removeCSS'],
    direct: ['sendMessage', 'connect'],
    events: ['onCreated', 'onUpdated', 'onActivated', 'onMoved', 'onRemoved', 'onAttached', 'onDetached', 'onHighlighted', 'onReplaced'],
    nativeEvents: ['onZoomChange']
  });
})(self);
