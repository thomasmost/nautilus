//! The bridge between Chromium's extension system and Nautilus's tabs.
//!
//! Nautilus's tabs are windowless browsers in no Chromium window, so the
//! extension APIs that walk windows cannot see them. A script loaded
//! ahead of an extension's service worker (`js/bridge.js`) answers
//! those APIs from the engine instead. The engine reaches it through a
//! relay page, one of the extension's own pages, in a hidden browser:
//! `docs/bridge-protocol.md` has the messages.
//!
//! `rewrite`, `table`, `debugger` and `policy` are pure. `relay` holds
//! the CEF side.

pub(crate) mod debugger;
pub(crate) mod groups;
pub(crate) mod install;
pub(crate) mod migrate;
pub(crate) mod policy;
pub(crate) mod rewrite;
pub(crate) mod side_panel;
pub(crate) mod table;

mod differ;
mod ids;
pub(crate) mod pattern;
mod router;
mod shape;
mod snapshot;
mod state;
pub(crate) mod transport;

mod call_ids;
mod hidden_page;
pub(crate) mod hub;
pub(crate) mod pages;
pub(crate) mod relay;

pub(crate) use relay::Bridge;

/// What the router reads about the extension a call comes from: the name
/// the model lists it under, and its commands as the manifest declares
/// them.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExtensionFacts {
    pub name: String,
    pub commands: Vec<osmena_core::ExtensionCommand>,
}

impl ExtensionFacts {
    /// The name the model builds "<name> Session" from: the extension's,
    /// or its id when it is not listed.
    pub(crate) fn name_or<'a>(&'a self, id: &'a str) -> &'a str {
        if self.name.is_empty() { id } else { &self.name }
    }
}

/// The facts of an extension the engine does not list: no name, no
/// commands.
pub(crate) static NO_FACTS: ExtensionFacts = ExtensionFacts {
    name: String::new(),
    commands: Vec::new(),
};

impl groups::GroupView {
    /// The strip as the group calls and the group events read it, with
    /// window and tab ids as extensions know them.
    pub(crate) fn of(snapshot: &snapshot::Snapshot, ids: &ids::IdSpace) -> groups::GroupView {
        groups::GroupView {
            windows: snapshot
                .windows
                .iter()
                .map(|window| groups::GroupWindow {
                    id: ids::window_id(window.id),
                    folders: window.folders.clone(),
                    tabs: window
                        .tabs
                        .iter()
                        .map(|tab| groups::GroupTab {
                            tab: tab.id,
                            id: ids.ext(tab.id),
                            folder: tab.folder,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

const CORE_JS: &str = include_str!("js/core.js");
const TABS_JS: &str = include_str!("js/tabs.js");
const WINDOWS_JS: &str = include_str!("js/windows.js");
const RUNTIME_JS: &str = include_str!("js/runtime.js");
const TAB_GROUPS_JS: &str = include_str!("js/tab_groups.js");
const COMMANDS_JS: &str = include_str!("js/commands.js");
const SIDE_PANEL_JS: &str = include_str!("js/side_panel.js");
// The page lane's transport script, which `pages::page_script` puts first in an
// extension page's context.
pub(crate) const PAGE_TRANSPORT_JS: &str = include_str!("js/page_transport.js");

/// The replacement of every bridged namespace, for a context whose
/// transport has already defined `__bridgeCall` and `__bridgeOnEvent`.
pub(crate) fn namespaces_script() -> String {
    [
        CORE_JS,
        TABS_JS,
        WINDOWS_JS,
        RUNTIME_JS,
        TAB_GROUPS_JS,
        COMMANDS_JS,
        SIDE_PANEL_JS,
    ]
    .join("\n")
}

/// The whole script of a bridged service worker, as it is written to
/// `nautilus-bridge/bridge.js`: the transport, then the namespaces.
pub(crate) fn worker_script() -> String {
    [install::BRIDGE_JS, &namespaces_script()].join("\n")
}

/// The value a `Runtime.evaluate` result carries, or why there is
/// none: the call failed, or the script threw.
pub(crate) fn evaluation(success: bool, result: &[u8]) -> Result<serde_json::Value, String> {
    let parsed: serde_json::Value = serde_json::from_slice(result).unwrap_or_default();
    if !success {
        return Err(parsed
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("the DevTools call failed")
            .to_string());
    }
    if let Some(exception) = parsed.get("exceptionDetails") {
        return Err(exception
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exception.get("text").and_then(|t| t.as_str()))
            .unwrap_or("the script threw")
            .to_string());
    }
    Ok(parsed
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::evaluation;

    #[test]
    fn an_evaluation_yields_its_value_by_value() {
        let result = br#"{"result":{"type":"object","value":{"ok":true,"attempts":1}}}"#;
        assert_eq!(
            evaluation(true, result),
            Ok(json!({"ok": true, "attempts": 1}))
        );
        // `undefined` has no value.
        assert_eq!(
            evaluation(true, br#"{"result":{"type":"undefined"}}"#),
            Ok(json!(null))
        );
    }

    #[test]
    fn a_thrown_script_and_a_failed_call_are_errors() {
        let thrown = br#"{"result":{"type":"object"},"exceptionDetails":{"text":"Uncaught","exception":{"description":"ReferenceError: nautilusRelay is not defined"}}}"#;
        assert_eq!(
            evaluation(true, thrown),
            Err("ReferenceError: nautilusRelay is not defined".to_string())
        );
        assert_eq!(
            evaluation(
                false,
                br#"{"code":-32000,"message":"Execution context was destroyed."}"#
            ),
            Err("Execution context was destroyed.".to_string())
        );
        assert_eq!(
            evaluation(false, b"not json"),
            Err("the DevTools call failed".to_string())
        );
    }

    use serde_json::Value;

    use super::ids::IdSpace;
    use super::pattern::Grants;
    use super::router::{self, Outcome};
    use super::shape::View;
    use super::side_panel;
    use super::snapshot::Snapshot;
    use super::transport::{Asker, BridgeCall};
    use super::{
        COMMANDS_JS, CORE_JS, NO_FACTS, PAGE_TRANSPORT_JS, RUNTIME_JS, SIDE_PANEL_JS,
        TAB_GROUPS_JS, TABS_JS, WINDOWS_JS, namespaces_script, worker_script,
    };

    /// The names in `key: ['a', 'b']` of a namespace script.
    fn list(script: &str, key: &str) -> Vec<String> {
        let opening = format!("{key}: [");
        let start = script.find(&opening).expect("the list is there") + opening.len();
        let end = start + script[start..].find(']').expect("the list ends");
        script[start..end]
            .split(',')
            .map(|name| name.trim().trim_matches('\'').to_string())
            .filter(|name| !name.is_empty())
            .collect()
    }

    #[test]
    fn every_function_of_a_replaced_namespace_is_ours() {
        let snapshot = Snapshot::default();
        let ids = IdSpace::default();
        let grants = Grants::default();
        let view = View {
            snapshot: &snapshot,
            ids: &ids,
            grants: &grants,
            last_focused: None,
            panels: &std::collections::HashMap::new(),
            facts: &NO_FACTS,
        };
        for (namespace, script) in [
            ("tabs", TABS_JS),
            ("windows", WINDOWS_JS),
            ("tabGroups", TAB_GROUPS_JS),
            ("commands", COMMANDS_JS),
        ] {
            for method in list(script, "bridged")
                .into_iter()
                .chain(list(script, "optional"))
            {
                let call = BridgeCall {
                    id: 1,
                    extension: "abcdefghijklmnopabcdefghijklmnop".to_string(),
                    asker: Asker::Worker,
                    namespace: namespace.to_string(),
                    method: method.clone(),
                    args: Vec::<Value>::new(),
                    page: None,
                };
                let unrouted = router::route(&call, &view)
                    == Outcome::Reject(router::unsupported(namespace, &method));
                // The one name the script sends that the router refuses
                // on purpose: the model has no request that moves a folder.
                assert_eq!(
                    unrouted,
                    (namespace, method.as_str()) == ("tabGroups", "move"),
                    "{namespace}.{method}"
                );
            }
        }
        // Only these two are Chromium's own functions, and neither walks
        // a tab strip.
        assert_eq!(list(TABS_JS, "direct"), ["sendMessage", "connect"]);
        assert!(list(WINDOWS_JS, "direct").is_empty());
        // Whatever else Chromium defines is replaced by a rejection, and
        // whatever events it has that the engine does not fire are inert.
        assert!(CORE_JS.contains("target[key] = unsupported(namespace, key);"));
        assert!(CORE_JS.contains("target[key] = new BridgeEvent(null);"));
        assert!(CORE_JS.contains("configurable: false, writable: false"));
        // One place puts a replacement in, for `chrome` and for the
        // `browser` global beside it: the core's `pin`, which `define`
        // and the hand-written sidePanel both go through. No namespace
        // script sets a property of `chrome` itself, so none can cover
        // one global and miss the other.
        assert!(CORE_JS.contains("var other = root.browser;"));
        assert!(CORE_JS.contains("[chrome, twin].forEach(function (holder) {"));
        assert!(CORE_JS.contains("pin(namespace, build(namespace, spec, source));"));
        assert!(SIDE_PANEL_JS.contains("bridge.pin('sidePanel', Object.freeze(api));"));
        for script in [
            TABS_JS,
            WINDOWS_JS,
            TAB_GROUPS_JS,
            COMMANDS_JS,
            SIDE_PANEL_JS,
        ] {
            assert!(!script.contains("Object.defineProperty(chrome,"));
            assert!(!script.contains("root.browser"));
        }
        for (namespace, script) in [
            ("tabs", TABS_JS),
            ("windows", WINDOWS_JS),
            ("tabGroups", TAB_GROUPS_JS),
            ("commands", COMMANDS_JS),
        ] {
            assert!(script.contains(&format!("bridge.define('{namespace}', {{")));
        }
        // What the bridge wraps on Chromium's own runtime and debugger
        // objects holds for `browser` because the object is one.
        assert!(CORE_JS.contains("same('runtime');"));
        assert!(CORE_JS.contains("same('debugger');"));
        for script in [
            CORE_JS,
            TABS_JS,
            WINDOWS_JS,
            RUNTIME_JS,
            TAB_GROUPS_JS,
            COMMANDS_JS,
            SIDE_PANEL_JS,
            PAGE_TRANSPORT_JS,
        ] {
            assert!(script.contains("'use strict';"));
        }
    }

    #[test]
    fn tab_groups_and_commands_are_replaced_whole_and_refused_when_they_cannot_be() {
        assert_eq!(
            list(TAB_GROUPS_JS, "bridged"),
            ["get", "query", "update", "move"]
        );
        assert_eq!(
            list(TAB_GROUPS_JS, "events"),
            ["onCreated", "onUpdated", "onMoved", "onRemoved"]
        );
        assert_eq!(list(COMMANDS_JS, "bridged"), ["getAll"]);
        assert_eq!(list(COMMANDS_JS, "events"), ["onCommand"]);
        // Nothing of either is Chromium's own function.
        for script in [TAB_GROUPS_JS, COMMANDS_JS] {
            assert!(list(script, "direct").is_empty());
            assert!(list(script, "optional").is_empty());
            assert!(list(script, "nativeEvents").is_empty());
        }
        // tabs.group and tabs.ungroup are the engine's too.
        for method in ["group", "ungroup"] {
            assert!(list(TABS_JS, "bridged").contains(&method.to_string()));
        }
        // Each is replaced only where Chromium defines it.
        assert!(TAB_GROUPS_JS.contains("!root.chrome.tabGroups"));
        assert!(COMMANDS_JS.contains("!root.chrome.commands"));
        // A page whose bridge cannot be put in keeps none of them, the
        // side panel included, on `chrome` or on `browser`.
        assert!(
            PAGE_TRANSPORT_JS
                .contains("['tabs', 'windows', 'tabGroups', 'commands', 'sidePanel'].forEach")
        );
        assert!(PAGE_TRANSPORT_JS.contains("[root.chrome, root.browser].filter"));
        // Both come before the side panel's script, which is last.
        let script = namespaces_script();
        let at = |part: &str| script.find(part).expect("the script is there");
        assert!(at(TABS_JS) < at(TAB_GROUPS_JS));
        assert!(at(TAB_GROUPS_JS) < at(COMMANDS_JS));
        assert!(at(COMMANDS_JS) < at(SIDE_PANEL_JS));
    }

    #[test]
    fn every_side_panel_function_is_ours() {
        // The members the script declares as the engine's, and nothing else.
        assert_eq!(
            list(SIDE_PANEL_JS, "bridged"),
            [
                "open",
                "close",
                "getOptions",
                "setOptions",
                "getPanelBehavior",
                "setPanelBehavior",
                "getLayout"
            ]
        );
        assert_eq!(list(SIDE_PANEL_JS, "events"), ["onOpened", "onClosed"]);
        for method in list(SIDE_PANEL_JS, "bridged") {
            assert!(side_panel::is_method(&method), "{method}");
            // Each is defined on the object the script installs.
            assert!(
                SIDE_PANEL_JS.contains(&format!("      {method}: callbackable(")),
                "{method}"
            );
        }
        // A name the script does not declare is not one of the engine's:
        // `route` rejects it as unsupported.
        assert!(!side_panel::is_method("frobnicate"));
        assert!(SIDE_PANEL_JS.contains("Object.freeze(api)"));
        assert!(SIDE_PANEL_JS.contains("bridge.pin('sidePanel', Object.freeze(api));"));
        // Its events are the core's, announced and dispatched there: the
        // script registers no event handler of its own.
        for event in list(SIDE_PANEL_JS, "events") {
            assert!(
                SIDE_PANEL_JS.contains(&format!(
                    "{event}: new bridge.BridgeEvent('sidePanel.{event}')"
                )),
                "{event}"
            );
        }
        assert!(!SIDE_PANEL_JS.contains("__bridgeOnEvent("));
        // It is the last of the namespaces, after the core it needs.
        assert!(namespaces_script().ends_with(SIDE_PANEL_JS));
    }

    #[test]
    fn the_strip_fields_a_worker_remembers_are_the_ones_the_engine_sends() {
        // `js/runtime.js` keeps these fields of each tab for `sender.tab`;
        // `View::strip_json` is what the engine answers `stripOf` with.
        let opening = "var FIELDS = [";
        let start = RUNTIME_JS.find(opening).expect("the list is there") + opening.len();
        let end = start + RUNTIME_JS[start..].find(']').expect("the list ends");
        let mut remembered: Vec<String> = RUNTIME_JS[start..end]
            .split(',')
            .map(|name| name.trim().trim_matches('\'').to_string())
            .collect();
        remembered.sort();

        use super::snapshot::tests::{snapshot, tab, window};
        let snapshot = snapshot(vec![window(1, true, vec![tab(1, "a*")])]);
        let ids = IdSpace::default();
        let grants = Grants::default();
        let view = View {
            snapshot: &snapshot,
            ids: &ids,
            grants: &grants,
            last_focused: None,
            panels: &std::collections::HashMap::new(),
            facts: &NO_FACTS,
        };
        let strip = view.strip_json(&snapshot.windows[0], 0);
        let mut sent: Vec<String> = strip
            .as_object()
            .expect("the strip is an object")
            .keys()
            .cloned()
            .collect();
        sent.sort();
        assert_eq!(remembered, sent);
    }

    #[test]
    fn the_worker_script_is_the_transport_then_the_namespaces() {
        let script = worker_script();
        assert!(script.starts_with(super::install::BRIDGE_JS));
        assert!(script.ends_with(&namespaces_script()));
        // The two globals the namespaces are written against, and no
        // tabs.query of the transport's own.
        assert!(super::install::BRIDGE_JS.contains("self.__bridgeCall = function"));
        assert!(super::install::BRIDGE_JS.contains("self.__bridgeOnEvent = function"));
        assert!(!super::install::BRIDGE_JS.contains("define(chrome.tabs, 'query'"));
    }
}
