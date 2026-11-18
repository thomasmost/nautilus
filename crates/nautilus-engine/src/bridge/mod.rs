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

#![allow(dead_code)]

pub(crate) mod debugger;
pub(crate) mod install;
pub(crate) mod policy;
pub(crate) mod rewrite;
pub(crate) mod table;

mod differ;
mod ids;
pub(crate) mod pattern;
mod router;
mod shape;
mod snapshot;
mod state;
pub(crate) mod transport;

#[cfg(any(windows, target_os = "macos"))]
pub(crate) mod relay;

#[cfg(any(windows, target_os = "macos"))]
pub(crate) use relay::Bridge;

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
}
