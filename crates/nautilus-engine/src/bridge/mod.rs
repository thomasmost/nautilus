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

const CORE_JS: &str = include_str!("js/core.js");
const TABS_JS: &str = include_str!("js/tabs.js");
const WINDOWS_JS: &str = include_str!("js/windows.js");
const RUNTIME_JS: &str = include_str!("js/runtime.js");
// The page lane's transport script, which `pages::page_script` puts first in an
// extension page's context.
pub(crate) const PAGE_TRANSPORT_JS: &str = include_str!("js/page_transport.js");

/// The replacement of every bridged namespace, for a context whose
/// transport has already defined `__bridgeCall` and `__bridgeOnEvent`.
pub(crate) fn namespaces_script() -> String {
    [CORE_JS, TABS_JS, WINDOWS_JS, RUNTIME_JS].join("\n")
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
    use super::snapshot::Snapshot;
    use super::transport::{Asker, BridgeCall};
    use super::{
        CORE_JS, PAGE_TRANSPORT_JS, RUNTIME_JS, TABS_JS, WINDOWS_JS, namespaces_script,
        worker_script,
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
        };
        // Names the script sends to the engine that the router does not
        // serve yet: the tab-groups change empties this list.
        let later = ["group", "ungroup"];
        for (namespace, script) in [("tabs", TABS_JS), ("windows", WINDOWS_JS)] {
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
                assert_eq!(
                    unrouted,
                    namespace == "tabs" && later.contains(&method.as_str()),
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
        for script in [CORE_JS, TABS_JS, WINDOWS_JS, RUNTIME_JS, PAGE_TRANSPORT_JS] {
            assert!(script.contains("'use strict';"));
        }
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
