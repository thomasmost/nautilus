// chrome.tabGroups, replaced. A tab group is one of the browser's tab
// folders: the engine answers reads from the strip and turns writes into
// requests the model decides. move is sent to the engine, which refuses
// it (a folder cannot be moved as one thing); onMoved exists so that
// addListener works, and never fires. Only where Chromium defines the
// namespace, so an extension without the "tabGroups" permission gains
// none. The constants (TAB_GROUP_ID_NONE, Color) are copied by define.
// The lists are read by a test in bridge/mod.rs: keep each on one line.
(function (root) {
  'use strict';
  var bridge = root.__nautilusBridge;
  if (!bridge || !root.chrome.tabGroups || bridge.installed.indexOf('tabGroups') !== -1) { return; }
  bridge.define('tabGroups', {
    bridged: ['get', 'query', 'update', 'move'],
    optional: [],
    direct: [],
    events: ['onCreated', 'onUpdated', 'onMoved', 'onRemoved'],
    nativeEvents: []
  });
})(self);
