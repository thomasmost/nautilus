//! Extension pages: action popups and pages shown as tabs (the side
//! panel's page later). The transport is evaluated in each of their frames
//! when its script context is created; the namespaces replace Chromium's
//! at the first change to the document, which the parser makes before any
//! of the page's own scripts run (`deferred_namespaces`). Their calls
//! travel to the browser process as process
//! messages and the answers come back as a script evaluated in the frame.
//!
//! Trust: the render process is not believed about who it is. The
//! browser process takes the extension's id from the frame's own address
//! and drops a message from a frame that is not an extension's page, and
//! delivers nothing to a frame that has navigated elsewhere. A call from a
//! page of an extension the hub does not serve yet is held until it is
//! served, and refused if it never is; it is never answered by Chromium.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Frame, ImplFrame, ImplListValue, ImplProcessMessage, ImplV8Context,
    ImplV8Handler, ImplV8Value, ProcessId, ProcessMessage, V8Context, V8Handler,
    V8Propertyattribute, V8Value, WrapV8Handler, wrap_v8_handler,
};
use serde_json::{Value, json};
use url::Url;

use super::hub::Lane;
use super::install::HELPER_ID;
use super::relay::RELAY_PATH;
use super::transport::{BridgeCall, BridgeTransport, CallId};
use super::{PAGE_TRANSPORT_JS, namespaces_script};

/// The deferred install's observer (`js/page_defer.js`), which
/// `deferred_namespaces` wraps around the namespaces.
const DEFER_JS: &str = include_str!("js/page_defer.js");
use crate::crx;
use crate::delegate::DelegateState;

/// The process message a page's call travels in: one argument, the call
/// as JSON (`{"id", "ns", "method", "args"}`).
pub(crate) const CALL_MESSAGE: &str = "nautilus-bridge-call";

/// The function the render process binds on an extension page's global
/// object; the page transport takes it and deletes it.
const SEND_FUNCTION: &str = "__nautilusBridgeSend";

/// The extension whose page `url` is, if it is one the bridge goes into:
/// any `chrome-extension://<id>/...` address but the relay page (which
/// talks to the engine in its own way) and the helper extension's pages
/// (which are the engine's own, not bridged).
pub(crate) fn extension_of(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if url.scheme() != "chrome-extension" {
        return None;
    }
    let id = url.host_str()?;
    if !crx::is_extension_id(id) || id == HELPER_ID || url.path() == RELAY_PATH {
        return None;
    }
    Some(id.to_string())
}

/// A call as a page sent it.
pub(crate) struct PageCall {
    /// The page's own number for the call.
    pub id: u64,
    pub namespace: String,
    pub method: String,
    pub args: Vec<Value>,
}

pub(crate) fn parse_call(text: &str) -> Option<PageCall> {
    let value: Value = serde_json::from_str(text).ok()?;
    let args = match value.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(args)) => args.clone(),
        Some(_) => return None,
    };
    Some(PageCall {
        id: value.get("id")?.as_u64()?,
        namespace: value.get("ns")?.as_str()?.to_string(),
        method: value.get("method")?.as_str()?.to_string(),
        args,
    })
}

/// What is evaluated in an extension page's context when it is created: the
/// page transport, then the namespaces, deferred to the first change of the
/// document (`deferred_namespaces`). A failure is reported through the
/// console, which the browser process logs.
pub(crate) fn page_script() -> String {
    format!(
        "try {{\n{PAGE_TRANSPORT_JS}\n}} catch (e) {{ console.error('nautilus-bridge: ' + e); }}\n{}",
        deferred_namespaces(&namespaces_script())
    )
}

/// The namespaces as a function run once, at the first change to the
/// page's document (`DEFER_JS`): `chrome` does not exist yet when the
/// context is created, and it does before the page's first script.
pub(crate) fn deferred_namespaces(namespaces: &str) -> String {
    format!(
        "(function (root, install) {{\n{DEFER_JS}\n}})(self, function () {{\ntry {{\n{namespaces}\n}} catch (e) {{ console.error('nautilus-bridge: ' + e); }}\n}});"
    )
}

/// The script that hands `payload` to a page's transport.
fn delivery(payload: &Value) -> String {
    // A JSON string literal is a JavaScript string literal.
    format!(
        "self.__nautilusBridgeDeliver({});",
        Value::String(payload.to_string())
    )
}

/// Render process: put the bridge into an extension page's new context.
/// Called from the render-process handler's `on_context_created`.
pub(crate) fn install(context: &mut V8Context) {
    let Some(global) = context.global() else {
        return;
    };
    let name = CefString::from(SEND_FUNCTION);
    let mut handler = BridgeSend::new();
    let Some(mut function) = cef::v8_value_create_function(Some(&name), Some(&mut handler)) else {
        return;
    };
    global.set_value_bykey(
        Some(&name),
        Some(&mut function),
        V8Propertyattribute::default(),
    );
    // CEF's `eval` does nothing without both out-pointers.
    let (mut retval, mut exception) = (None, None);
    context.eval(
        Some(&CefString::from(page_script().as_str())),
        None,
        0,
        Some(&mut retval),
        Some(&mut exception),
    );
}

// The bound function: its one string argument becomes a process message
// to the browser process, from the frame that called it.
wrap_v8_handler! {
    struct BridgeSend;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            let Some([Some(text), ..]) = arguments else {
                return 0;
            };
            if text.is_string() == 0 {
                return 0;
            }
            let text = CefString::from(&text.string_value()).to_string();
            let Some(frame) = cef::v8_context_get_current_context().and_then(|c| c.frame()) else {
                return 0;
            };
            let Some(mut message) =
                cef::process_message_create(Some(&CefString::from(CALL_MESSAGE)))
            else {
                return 0;
            };
            if let Some(arguments) = message.argument_list() {
                arguments.set_string(0, Some(&CefString::from(text.as_str())));
            }
            frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
            1
        }
    }
}

/// Browser process: the lane between extension pages and the hub.
///
/// No CEF call is made while one of the `RefCell`s below is borrowed: a
/// frame is asked about with the map out of the borrow, and a frame let
/// go of is dropped after it.
#[derive(Default)]
pub(crate) struct PageLane {
    /// Calls not yet answered: the frame that asked and its own number
    /// for the call.
    calls: RefCell<HashMap<CallId, (Frame, u64)>>,
    last: Cell<CallId>,
    /// The frames of each extension that have called: where its events
    /// go. Only valid frames are kept; no event is kept for a frame that
    /// is not there.
    frames: RefCell<HashMap<String, Vec<Frame>>>,
}

impl PageLane {
    /// A process message arrived from `frame` of `browser`. Returns
    /// whether it was a bridge call (answered or dropped), so the client
    /// tells CEF it was handled.
    pub(crate) fn receive(
        &self,
        delegate: &DelegateState,
        browser: &Browser,
        frame: &Frame,
        message: &ProcessMessage,
    ) -> bool {
        if CefString::from(&message.name()).to_string() != CALL_MESSAGE {
            return false;
        }
        // The extension is the frame's own address, as the browser process
        // holds it, never a field of the message.
        let url = CefString::from(&frame.url()).to_string();
        let Some(extension) = extension_of(&url) else {
            log::warn!("a bridge call from {url}, which is no extension page, was dropped");
            return true;
        };
        let Some(hub) = delegate.hub() else {
            return true;
        };
        // An extension the hub does not serve yet is not dropped here: the
        // call is held there until it is served, and then refused if it
        // never is (`BridgeState::page_call`), so the page always gets a reply.
        let text = message
            .argument_list()
            .filter(|arguments| arguments.size() >= 1)
            .map(|arguments| CefString::from(&arguments.string(0)).to_string());
        let Some(call) = text.as_deref().and_then(parse_call) else {
            return true;
        };
        let id = self.last.get() + 1;
        self.last.set(id);
        // A fresh id: nothing is displaced.
        let _ = self.calls.borrow_mut().insert(id, (frame.clone(), call.id));
        self.remember(&extension, frame);
        hub.call(
            Lane::Page,
            BridgeCall {
                id,
                extension,
                asker: delegate.asker_of(browser),
                namespace: call.namespace,
                method: call.method,
                args: call.args,
            },
        );
        true
    }

    /// The frames of `extension` that are still valid. The list is taken
    /// out of the map while they are asked, and given back pruned; the
    /// invalid ones are let go outside the borrow.
    fn live_frames(&self, extension: &str) -> Vec<Frame> {
        let Some(candidates) = self.frames.borrow_mut().remove(extension) else {
            return Vec::new();
        };
        let (live, gone): (Vec<Frame>, Vec<Frame>) = candidates
            .into_iter()
            .partition(|frame| frame.is_valid() != 0);
        if !live.is_empty() {
            let displaced = self
                .frames
                .borrow_mut()
                .insert(extension.to_string(), live.clone());
            drop(displaced);
        }
        drop(gone);
        live
    }

    fn remember(&self, extension: &str, frame: &Frame) {
        let key = CefString::from(&frame.identifier()).to_string();
        let mut list = self.live_frames(extension);
        if !list
            .iter()
            .any(|known| CefString::from(&known.identifier()).to_string() == key)
        {
            list.push(frame.clone());
        }
        let displaced = self.frames.borrow_mut().insert(extension.to_string(), list);
        drop(displaced);
    }

    /// Let go of every frame and call, before CEF shuts down. Taken out of
    /// the cells first: letting go is a CEF call.
    pub(crate) fn clear(&self) {
        let calls = std::mem::take(&mut *self.calls.borrow_mut());
        let frames = std::mem::take(&mut *self.frames.borrow_mut());
        drop(calls);
        drop(frames);
    }
}

/// Evaluate `payload`'s delivery in `frame`, if it is still a page of
/// `extension`.
fn deliver(frame: &Frame, extension: &str, payload: &Value) {
    if frame.is_valid() == 0 {
        return;
    }
    let url = CefString::from(&frame.url()).to_string();
    if extension_of(&url).as_deref() != Some(extension) {
        return;
    }
    frame.execute_java_script(Some(&CefString::from(delivery(payload).as_str())), None, 0);
}

impl BridgeTransport for PageLane {
    fn reply(&self, call: CallId, result: Result<Value, String>) {
        let removed = self.calls.borrow_mut().remove(&call);
        let Some((frame, id)) = removed else {
            return;
        };
        let payload = match result {
            Ok(value) => json!({ "id": id, "ok": value }),
            Err(message) => json!({ "id": id, "error": message }),
        };
        let url = CefString::from(&frame.url()).to_string();
        if let Some(extension) = extension_of(&url) {
            deliver(&frame, &extension, &payload);
        }
    }

    /// At most once, to the pages that have called and are still there;
    /// an event for a page that is gone is dropped, never kept.
    fn emit(&self, extension: &str, event: &str, args: &[Value]) {
        let frames = self.live_frames(extension);
        let payload = json!({ "event": event, "args": args });
        for frame in &frames {
            deliver(frame, extension, &payload);
        }
    }

    /// Pages are awake while they exist.
    fn wake(&self, _extension: &str) {}
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[test]
    fn only_an_extension_s_own_pages_are_bridged() {
        for (url, expected) in [
            (format!("chrome-extension://{ID}/popup.html"), Some(ID)),
            (
                format!("chrome-extension://{ID}/pages/options.html?tab=1#x"),
                Some(ID),
            ),
            (format!("chrome-extension://{ID}/"), Some(ID)),
            (format!("chrome-extension://{ID}{RELAY_PATH}"), None),
            (format!("chrome-extension://{HELPER_ID}/helper.html"), None),
            ("chrome-extension://not-an-id/popup.html".to_string(), None),
            (format!("https://{ID}/popup.html"), None),
            ("https://example.com/".to_string(), None),
            ("about:blank".to_string(), None),
            ("chrome://extensions/".to_string(), None),
            ("not a url".to_string(), None),
        ] {
            assert_eq!(extension_of(&url).as_deref(), expected, "{url}");
        }
    }

    #[test]
    fn a_page_s_call_is_read_from_its_json() {
        let call =
            parse_call(r#"{"id":7,"ns":"tabs","method":"query","args":[{"active":true},null]}"#)
                .unwrap();
        assert_eq!(call.id, 7);
        assert_eq!(call.namespace, "tabs");
        assert_eq!(call.method, "query");
        assert_eq!(call.args, vec![json!({ "active": true }), json!(null)]);
        // No arguments is an empty list.
        assert_eq!(
            parse_call(r#"{"id":1,"ns":"bridge","method":"listen"}"#)
                .unwrap()
                .args,
            Vec::<serde_json::Value>::new()
        );
        for bad in [
            "",
            "[]",
            "{}",
            r#"{"id":"7","ns":"tabs","method":"query"}"#,
            r#"{"id":7,"ns":3,"method":"query"}"#,
            r#"{"id":7,"ns":"tabs"}"#,
            r#"{"id":7,"ns":"tabs","method":"query","args":{}}"#,
        ] {
            assert!(parse_call(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_page_script_defines_the_transport_then_defers_the_namespaces() {
        let script = page_script();
        let transport = script.find("__nautilusBridgeSend").unwrap();
        // The observer's callback (which calls install) is written before
        // the observe call; the namespaces follow both.
        let install = script.find("install();").unwrap();
        let observe = script.find("observer.observe(root.document").unwrap();
        let core = script.find("'__nautilusBridge'").unwrap();
        let tabs = script.find("bridge.define('tabs'").unwrap();
        assert!(transport < install && install < observe && observe < core && core < tabs);
        assert!(script.starts_with("try {"));
        // The namespaces sit inside the function the observer calls.
        assert!(script.find("function () {\ntry {\n").unwrap() < core);
    }

    #[test]
    fn the_namespaces_run_once_at_the_first_change_of_the_document() {
        let script = deferred_namespaces("NAMESPACES");
        assert!(script.starts_with("(function (root, install) {\n"));
        assert!(script.contains("new root.MutationObserver("));
        assert!(
            script.contains("observer.observe(root.document, { childList: true, subtree: true });")
        );
        assert!(script.contains("observer.disconnect();"));
        assert!(script.contains("no document to wait for in this context"));
        assert!(script.ends_with(
            "(self, function () {\ntry {\nNAMESPACES\n} catch (e) { console.error('nautilus-bridge: ' + e); }\n});"
        ));
    }

    #[test]
    fn a_reply_is_one_call_of_the_page_s_delivery_function() {
        assert_eq!(
            delivery(&json!({ "id": 3, "ok": { "title": "a \"quoted\" </script> title" } })),
            r#"self.__nautilusBridgeDeliver("{\"id\":3,\"ok\":{\"title\":\"a \\\"quoted\\\" </script> title\"}}");"#
        );
    }
}
