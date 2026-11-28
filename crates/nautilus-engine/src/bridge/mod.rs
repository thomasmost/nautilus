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
const DEBUGGER_JS: &str = include_str!("js/debugger.js");
const SIDE_PANEL_JS: &str = include_str!("js/side_panel.js");
// The refusal (`__nautilusRefuseNative`), the first script of every bridged
// context: what a namespace that cannot be put in is replaced by.
pub(crate) const REFUSE_JS: &str = include_str!("js/refuse.js");
// The page lane's transport script, which `pages::page_script` puts first in an
// extension page's context.
pub(crate) const PAGE_TRANSPORT_JS: &str = include_str!("js/page_transport.js");

/// What is refused when a namespace script throws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refuses {
    /// Every replaced namespace: without the core nothing is put in.
    All,
    /// The namespace the script replaces.
    Only(&'static str),
    /// Nothing: the script replaces no namespace (it wraps members of
    /// Chromium's own `runtime` and `debugger`, which harm nothing).
    Nothing,
}

/// The namespace scripts in the order they run, each with what its failure
/// refuses. `ci/side-panel/bridge_check.mjs` composes the same list
/// (`NAMESPACES` there).
const NAMESPACES: [(&str, Refuses); 8] = [
    (CORE_JS, Refuses::All),
    (TABS_JS, Refuses::Only("tabs")),
    (WINDOWS_JS, Refuses::Only("windows")),
    (RUNTIME_JS, Refuses::Nothing),
    (TAB_GROUPS_JS, Refuses::Only("tabGroups")),
    (COMMANDS_JS, Refuses::Only("commands")),
    (DEBUGGER_JS, Refuses::Nothing),
    (SIDE_PANEL_JS, Refuses::Only("sidePanel")),
];

/// `script` in a guard of its own: a failure is reported through the
/// console, what it would have replaced is refused (`js/refuse.js`), and the
/// scripts after it still run.
fn guarded(script: &str, refuses: Refuses) -> String {
    let refusal = match refuses {
        Refuses::All => "()".to_string(),
        Refuses::Only(name) => format!("(['{name}'])"),
        Refuses::Nothing => {
            return format!(
                "try {{\n{script}\n}} catch (e) {{ console.error('nautilus-bridge: ' + e); }}"
            );
        }
    };
    format!(
        "try {{\n{script}\n}} catch (e) {{ console.error('nautilus-bridge: ' + e); if (typeof self.__nautilusRefuseNative === 'function') {{ self.__nautilusRefuseNative{refusal}; }} }}"
    )
}

/// The replacement of every bridged namespace, for a context whose
/// transport has already defined `__bridgeCall` and `__bridgeOnEvent` and
/// which has the refusal (`REFUSE_JS`). Each script runs in its own guard:
/// one that fails is refused and the others are still installed.
pub(crate) fn namespaces_script() -> String {
    NAMESPACES
        .iter()
        .map(|(script, refuses)| guarded(script, *refuses))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The whole script of a bridged service worker, as it is written to
/// `nautilus-bridge/bridge.js`: the refusal, the transport, then the
/// namespaces. The transport is guarded too: without it the core finds no
/// transport and refuses every replaced namespace, where an unguarded
/// failure would end the script and leave them all Chromium's.
pub(crate) fn worker_script() -> String {
    [
        guarded(REFUSE_JS, Refuses::Nothing),
        guarded(install::BRIDGE_JS, Refuses::Nothing),
        namespaces_script(),
    ]
    .join("\n")
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
        COMMANDS_JS, CORE_JS, DEBUGGER_JS, NO_FACTS, RUNTIME_JS, SIDE_PANEL_JS, TAB_GROUPS_JS,
        TABS_JS, WINDOWS_JS, namespaces_script, worker_script,
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

    // What is left here of the tests that read the scripts' text
    // (2026-11-27). A script's behaviour is checked where the script runs,
    // in `ci/side-panel/bridge_check.mjs`: that a replaced namespace has
    // nothing of Chromium's left, the pin, both globals, the refusal, the
    // events, the debugger's reports. A test that only asserted a script
    // contains some text proved none of that and is gone. The ones kept
    // each hold a script to something on the Rust side, which the node
    // check cannot see: the names a script sends against the router and
    // the side panel's routes, the lists of Chromium's own functions a
    // script lets through, the strip's fields, and the composition.

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
        // Kept: the node check cannot ask the router. Every name a script
        // sends to the engine has a row there.
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
        // Kept: the node check shows that the functions its stand-in has
        // do not reach Chromium, which says nothing of a name its stand-in
        // lacks. These lists are every function and event of Chromium's
        // own that a script lets through: `tabs.sendMessage` and
        // `tabs.connect`, which walk no tab strip, the zoom event, and
        // nothing of `windows`, `tabGroups` or `commands`.
        assert_eq!(list(TABS_JS, "direct"), ["sendMessage", "connect"]);
        assert_eq!(list(TABS_JS, "nativeEvents"), ["onZoomChange"]);
        for script in [WINDOWS_JS, TAB_GROUPS_JS, COMMANDS_JS] {
            assert!(list(script, "direct").is_empty());
            assert!(list(script, "nativeEvents").is_empty());
        }
        for script in [TAB_GROUPS_JS, COMMANDS_JS] {
            assert!(list(script, "optional").is_empty());
        }
    }

    #[test]
    fn every_side_panel_function_is_one_the_engine_routes() {
        // Kept: the node check holds `chrome.sidePanel` to exactly these
        // members (its `Object.keys` assertion); this holds the engine to
        // routing each of them, and to refusing a name that is not one.
        for method in [
            "open",
            "close",
            "getOptions",
            "setOptions",
            "getPanelBehavior",
            "setPanelBehavior",
            "getLayout",
        ] {
            assert!(side_panel::is_method(method), "{method}");
        }
        assert!(!side_panel::is_method("frobnicate"));
    }

    #[test]
    fn the_strip_fields_a_worker_remembers_are_the_ones_the_engine_sends() {
        // Kept: the two halves of one contract are in two languages.
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
    fn the_worker_script_is_the_refusal_the_transport_then_the_namespaces() {
        // Kept, with the test below: the order is the engine's to compose.
        let script = worker_script();
        let at = |part: &str| script.find(part).expect("the script is there");
        assert!(at(super::REFUSE_JS) < at(super::install::BRIDGE_JS));
        assert!(at(super::install::BRIDGE_JS) < at(CORE_JS));
        assert!(script.ends_with(&namespaces_script()));
    }

    // A text test, kept: what this composes is what runs in an extension,
    // and `ci/side-panel/bridge_check.mjs`, which checks the behaviour,
    // composes the scripts itself from the same list. Only this holds the
    // two lists together.
    #[test]
    fn each_namespace_script_runs_in_its_own_guard_and_its_failure_is_refused() {
        use super::Refuses;

        let composed = namespaces_script();
        let mut rest = composed.as_str();
        for (script, refusal) in [
            (CORE_JS, Some("self.__nautilusRefuseNative();")),
            (TABS_JS, Some("self.__nautilusRefuseNative(['tabs']);")),
            (
                WINDOWS_JS,
                Some("self.__nautilusRefuseNative(['windows']);"),
            ),
            (RUNTIME_JS, None),
            (
                TAB_GROUPS_JS,
                Some("self.__nautilusRefuseNative(['tabGroups']);"),
            ),
            (
                COMMANDS_JS,
                Some("self.__nautilusRefuseNative(['commands']);"),
            ),
            (DEBUGGER_JS, None),
            (
                SIDE_PANEL_JS,
                Some("self.__nautilusRefuseNative(['sidePanel']);"),
            ),
        ] {
            let opening = format!(
                "try {{\n{script}\n}} catch (e) {{ console.error('nautilus-bridge: ' + e);"
            );
            assert!(rest.starts_with(&opening), "a script is out of its place");
            rest = &rest[opening.len()..];
            let end = rest.find(" }\ntry {\n").map_or(rest.len(), |at| at + 3);
            let (guard, after) = rest.split_at(end);
            match refusal {
                Some(call) => assert!(guard.contains(call), "{guard}"),
                None => assert!(!guard.contains("__nautilusRefuseNative"), "{guard}"),
            }
            rest = after;
        }
        assert!(rest.is_empty(), "a script nobody listed: {rest}");
        assert_eq!(
            super::guarded("x();", Refuses::Nothing),
            "try {\nx();\n} catch (e) { console.error('nautilus-bridge: ' + e); }"
        );
    }
}
