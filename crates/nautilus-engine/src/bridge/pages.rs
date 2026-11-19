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

use osmena_core::TabId;

use super::hub::Lane;
use super::ids::SYNTHETIC_BASE;
use super::install::HELPER_ID;
use super::relay::RELAY_PATH;
use super::transport::{Asker, BridgeCall, CallId};
use super::{PAGE_TRANSPORT_JS, namespaces_script};

/// The deferred install's observer (`js/page_defer.js`), which
/// `deferred_namespaces` wraps around the namespaces.
const DEFER_JS: &str = include_str!("js/page_defer.js");

/// The page's own tab id, reported after the namespaces (`page_report.js`).
const REPORT_JS: &str = include_str!("js/page_report.js");

/// The function the render process binds for a report; the page's report
/// script takes it and deletes it.
const REPORT_FUNCTION: &str = "__nautilusBridgeReport";
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

/// The process message a page's own Chromium tab id travels in
/// (`js/page_report.js`).
pub(crate) const REPORT_MESSAGE: &str = "nautilus-bridge-tab";

/// The Chromium tab id a page reports: an object whose only key is `tab`,
/// holding a positive integer below `SYNTHETIC_BASE`.
pub(crate) fn report_of(text: &str) -> Option<i64> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    let tab = object.get("tab")?.as_i64()?;
    (0 < tab && tab < SYNTHETIC_BASE).then_some(tab)
}

/// Whether a report may come from a frame, and for which view and extension:
/// the main frame of a bridged extension's page that is shown as a tab. The
/// view is the browser's own (`Asker::View`), never something the page named.
pub(crate) fn report_source(main: bool, url: &str, view: Option<TabId>) -> Option<(String, TabId)> {
    if !main {
        return None;
    }
    let extension = extension_of(url)?;
    Some((extension, view?))
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
        deferred_namespaces(&format!("{}\n{REPORT_JS}", namespaces_script()))
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

/// What an extension page gets when its bridge cannot be bound: the transport
/// script, which defines the refusal of the page's native `tabs` and `windows`
/// and then returns (no send is bound), and the refusal itself. The namespaces
/// are not put in, so the page's calls stay refused.
pub(crate) fn refusal_script() -> String {
    format!(
        "try {{\n{PAGE_TRANSPORT_JS}\n}} catch (e) {{ console.error('nautilus-bridge: ' + e); }}\ntry {{ self.__nautilusRefuseNative(); }} catch (e) {{ console.error('nautilus-bridge: ' + e); }}"
    )
}

/// Takes the call `call` out of `calls` and returns it when its frame is still
/// a page of the extension the call was made for: the frame's current address
/// (`url_of`) decides, against the stored extension. The call is taken either
/// way, so an answer is given at most once.
fn take_answer<F>(
    calls: &mut HashMap<CallId, (F, u64, String)>,
    call: CallId,
    url_of: impl FnOnce(&F) -> String,
) -> Option<(F, u64, String)> {
    let (frame, id, extension) = calls.remove(&call)?;
    let url = url_of(&frame);
    deliverable(&url, &extension).then_some((frame, id, extension))
}

/// Render process: put the bridge into an extension page's new context.
/// Called from the render-process handler's `on_context_created`. When the
/// bound functions cannot be made, the page gets `refusal_script` instead, so
/// the page's native `tabs` and `windows` are refused either way.
pub(crate) fn install(context: &mut V8Context) {
    let script = if bind_functions(context) {
        page_script()
    } else {
        refusal_script()
    };
    // CEF's `eval` does nothing without both out-pointers.
    let (mut retval, mut exception) = (None, None);
    context.eval(
        Some(&CefString::from(script.as_str())),
        None,
        0,
        Some(&mut retval),
        Some(&mut exception),
    );
}

/// Binds the two functions the page's transport calls on the global object.
/// False when the global object or either function cannot be made.
fn bind_functions(context: &mut V8Context) -> bool {
    let Some(global) = context.global() else {
        return false;
    };
    let name = CefString::from(SEND_FUNCTION);
    let mut handler = BridgeSend::new();
    let Some(mut function) = cef::v8_value_create_function(Some(&name), Some(&mut handler)) else {
        return false;
    };
    global.set_value_bykey(
        Some(&name),
        Some(&mut function),
        V8Propertyattribute::default(),
    );
    let report_name = CefString::from(REPORT_FUNCTION);
    let mut report_handler = BridgeReport::new();
    let Some(mut report_function) =
        cef::v8_value_create_function(Some(&report_name), Some(&mut report_handler))
    else {
        return false;
    };
    global.set_value_bykey(
        Some(&report_name),
        Some(&mut report_function),
        V8Propertyattribute::default(),
    );
    true
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
            send_to_browser(CALL_MESSAGE, arguments)
        }
    }
}

// The same for the tab report: one string, sent in its own message.
wrap_v8_handler! {
    struct BridgeReport;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            send_to_browser(REPORT_MESSAGE, arguments)
        }
    }
}

/// Sends the one string argument of a bound function to the browser process
/// as a process message named `name`, from the frame that called it.
fn send_to_browser(name: &str, arguments: Option<&[Option<V8Value>]>) -> ::std::os::raw::c_int {
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
    let Some(mut message) = cef::process_message_create(Some(&CefString::from(name))) else {
        return 0;
    };
    if let Some(arguments) = message.argument_list() {
        arguments.set_string(0, Some(&CefString::from(text.as_str())));
    }
    frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
    1
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
    calls: RefCell<HashMap<CallId, (Frame, u64, String)>>,
    last: Cell<CallId>,
    /// The frames that have called, by the key the page context carries
    /// (`BridgeCall::page`): where their events go. A frame that is gone is
    /// dropped by `gone`, and no event is kept for it.
    pages: RefCell<HashMap<u64, PageFrame>>,
    /// The key of each frame, by its identifier, so a frame keeps its key.
    keys: RefCell<HashMap<String, u64>>,
    last_key: Cell<u64>,
}

/// A frame that has called, and the extension whose page it is.
struct PageFrame {
    extension: String,
    frame: Frame,
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
        let name = CefString::from(&message.name()).to_string();
        if name == REPORT_MESSAGE {
            return self.report(delegate, browser, frame, message);
        }
        if name != CALL_MESSAGE {
            return false;
        }
        // The extension is the frame's own address, as the browser process
        // holds it, never a field of the message.
        let url = CefString::from(&frame.url()).to_string();
        let Some(extension) = extension_of(&url) else {
            log::warn!("a bridge call from {url}, which is no extension page, was dropped");
            return true;
        };
        // An extension the hub does not serve yet is not dropped here: the
        // call is held there until it is served, and then refused if it
        // never is (`BridgeState::page_call`), so the page always gets a reply.
        let text = message
            .argument_list()
            .filter(|arguments| arguments.size() >= 1)
            .map(|arguments| CefString::from(&arguments.string(0)).to_string());
        // A call the browser cannot take is answered with an error when its
        // number can be read: this frame is an extension's page, so the answer
        // goes to it and to no other frame.
        let Some(hub) = delegate.hub() else {
            refuse(
                frame,
                &extension,
                text.as_deref(),
                "the bridge is not running.",
            );
            return true;
        };
        let Some(call) = text.as_deref().and_then(parse_call) else {
            refuse(frame, &extension, text.as_deref(), "the call is malformed.");
            return true;
        };
        let id = self.last.get() + 1;
        self.last.set(id);
        // A fresh id: nothing is displaced.
        let _ = self
            .calls
            .borrow_mut()
            .insert(id, (frame.clone(), call.id, extension.clone()));
        let page = self.key_of(frame, &extension);
        hub.call(
            Lane::Page,
            BridgeCall {
                id,
                extension,
                asker: delegate.asker_of(browser),
                namespace: call.namespace,
                method: call.method,
                args: call.args,
                page: Some(page),
            },
        );
        true
    }

    /// A page reported its own Chromium tab id. The view is the browser's: the
    /// frame must be the main frame of a bridged extension's page, and the
    /// browser's view for its browser must be a tab (`report_source`). A
    /// report that is not accepted is logged once and ignored.
    fn report(
        &self,
        delegate: &DelegateState,
        browser: &Browser,
        frame: &Frame,
        message: &ProcessMessage,
    ) -> bool {
        let url = CefString::from(&frame.url()).to_string();
        let view = match delegate.asker_of(browser) {
            Asker::View(tab) => Some(tab),
            Asker::Worker | Asker::Popup { .. } => None,
        };
        let text = message
            .argument_list()
            .filter(|arguments| arguments.size() >= 1)
            .map(|arguments| CefString::from(&arguments.string(0)).to_string());
        let Some(chromium) = text.as_deref().and_then(report_of) else {
            log::warn!("a tab report from {url} was dropped: it is not a tab id");
            return true;
        };
        let Some((extension, tab)) = report_source(frame.is_main() != 0, &url, view) else {
            log::warn!(
                "a tab report from {url} was dropped: it is not the main frame of a bridged extension's page shown as a tab"
            );
            return true;
        };
        if let Some(hub) = delegate.hub() {
            hub.report_tab(tab, chromium, &extension);
        }
        true
    }

    /// The key of `frame`, as a page of `extension`: the frame's own key,
    /// made on its first call. The frame is kept until `gone` finds it
    /// invalid.
    fn key_of(&self, frame: &Frame, extension: &str) -> u64 {
        let identifier = CefString::from(&frame.identifier()).to_string();
        let key = *self.keys.borrow_mut().entry(identifier).or_insert_with(|| {
            let key = self.last_key.get() + 1;
            self.last_key.set(key);
            key
        });
        let displaced = self.pages.borrow_mut().insert(
            key,
            PageFrame {
                extension: extension.to_string(),
                frame: frame.clone(),
            },
        );
        drop(displaced);
        key
    }

    /// The keys of the frames that are gone, which are let go. The map is
    /// taken out of its cell while the frames are asked, since asking is a
    /// CEF call.
    pub(crate) fn gone(&self) -> Vec<u64> {
        let all = std::mem::take(&mut *self.pages.borrow_mut());
        let (live, gone): (HashMap<u64, PageFrame>, HashMap<u64, PageFrame>) = all
            .into_iter()
            .partition(|(_, page)| page.frame.is_valid() != 0);
        self.pages.borrow_mut().extend(live);
        let keys: Vec<u64> = gone.keys().copied().collect();
        self.keys.borrow_mut().retain(|_, key| !keys.contains(key));
        drop(gone);
        keys
    }

    /// Let go of every frame and call, before CEF shuts down. Taken out of
    /// the cells first: letting go is a CEF call.
    pub(crate) fn clear(&self) {
        let calls = std::mem::take(&mut *self.calls.borrow_mut());
        let pages = std::mem::take(&mut *self.pages.borrow_mut());
        let keys = std::mem::take(&mut *self.keys.borrow_mut());
        drop(calls);
        drop(pages);
        drop(keys);
    }

    /// Answer the page call `call`: the page's own number, in the frame
    /// that asked, if that frame is still a page of the extension that asked.
    pub(crate) fn reply(&self, call: CallId, result: Result<Value, String>) {
        let answer = take_answer(&mut self.calls.borrow_mut(), call, |frame| {
            CefString::from(&frame.url()).to_string()
        });
        let Some((frame, id, extension)) = answer else {
            return;
        };
        let payload = match result {
            Ok(value) => json!({ "id": id, "ok": value }),
            Err(message) => json!({ "id": id, "error": message }),
        };
        deliver(&frame, &extension, &payload);
    }

    /// Deliver `event` to the page `page`, a frame of `extension` that has
    /// called and is still there. An event for a frame that is gone is
    /// dropped, never kept.
    pub(crate) fn emit_to(&self, page: u64, extension: &str, event: &str, args: &[Value]) {
        let frame = self
            .pages
            .borrow()
            .get(&page)
            .filter(|known| known.extension == extension)
            .map(|known| known.frame.clone());
        if let Some(frame) = frame {
            deliver(&frame, extension, &json!({ "event": event, "args": args }));
        }
    }
}

/// The number a page gave a call, read even when the rest of the call is not
/// well formed.
pub(crate) fn call_number(text: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(text).ok()?;
    value.get("id")?.as_u64()
}

/// Answer the page's call `text` with `message`, when its number can be read
/// (`call_number`); a call with no readable number has nothing to answer.
fn refuse(frame: &Frame, extension: &str, text: Option<&str>, message: &str) {
    let Some(id) = text.and_then(call_number) else {
        return;
    };
    deliver(frame, extension, &json!({ "id": id, "error": message }));
}

/// Whether a frame at `url` may be given a message for `extension`: it is
/// still a page of that extension.
pub(crate) fn deliverable(url: &str, extension: &str) -> bool {
    extension_of(url).as_deref() == Some(extension)
}

/// Evaluate `payload`'s delivery in `frame`, if it is still a page of
/// `extension`.
fn deliver(frame: &Frame, extension: &str, payload: &Value) {
    if frame.is_valid() == 0 {
        return;
    }
    let url = CefString::from(&frame.url()).to_string();
    if !deliverable(&url, extension) {
        return;
    }
    frame.execute_java_script(Some(&CefString::from(delivery(payload).as_str())), None, 0);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[test]
    fn a_reply_is_taken_only_for_the_frame_still_on_the_extension_that_asked() {
        const OTHER: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";
        let page = format!("chrome-extension://{ID}/page.html");
        let mut calls: HashMap<CallId, (String, u64, String)> = HashMap::new();
        calls.insert(1, (page.clone(), 7, ID.to_string()));
        assert_eq!(
            take_answer(&mut calls, 1, |frame| frame.clone()),
            Some((page, 7, ID.to_string()))
        );
        assert!(calls.is_empty());
        // The frame navigated to another extension's page: no reply, and the call is gone.
        calls.insert(
            2,
            (
                format!("chrome-extension://{OTHER}/page.html"),
                8,
                ID.to_string(),
            ),
        );
        assert_eq!(take_answer(&mut calls, 2, |frame| frame.clone()), None);
        assert!(calls.is_empty());
        // The frame navigated to the web.
        calls.insert(3, ("https://example.com/".to_string(), 9, ID.to_string()));
        assert_eq!(take_answer(&mut calls, 3, |frame| frame.clone()), None);
        assert!(calls.is_empty());
        // A call that is not pending has no answer.
        assert_eq!(take_answer(&mut calls, 4, |frame| frame.clone()), None);
    }

    #[test]
    fn a_page_whose_bridge_cannot_be_bound_refuses_native_tabs_and_windows() {
        let script = refusal_script();
        assert!(script.contains(PAGE_TRANSPORT_JS));
        assert!(script.contains("self.__nautilusRefuseNative()"));
        assert!(!script.contains(&namespaces_script()));
    }

    #[test]
    fn a_reply_goes_only_to_the_frame_still_on_the_extension_that_called() {
        const OTHER: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";
        assert!(deliverable(
            &format!("chrome-extension://{ID}/page.html"),
            ID
        ));
        // The frame navigated to another extension's page, or to the web.
        assert!(!deliverable(
            &format!("chrome-extension://{OTHER}/page.html"),
            ID
        ));
        assert!(!deliverable("https://example.com/", ID));
    }

    #[test]
    fn a_call_number_is_read_from_a_call_that_is_not_well_formed() {
        assert_eq!(call_number(r#"{"id":7,"ns":3}"#), Some(7));
        assert_eq!(call_number(r#"{"ns":"tabs"}"#), None);
        assert_eq!(call_number("not json"), None);
        assert_eq!(call_number(r#"{"id":"7"}"#), None);
    }

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

#[cfg(test)]
mod report_tests {
    use super::*;

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";

    fn page_url() -> String {
        format!("chrome-extension://{ID}/p5page.html")
    }

    #[test]
    fn a_report_is_an_object_with_only_a_positive_tab_id_below_the_synthetic_range() {
        assert_eq!(report_of(r#"{"tab":5}"#), Some(5));
        assert_eq!(report_of(r#"{"tab":1999999999}"#), Some(1_999_999_999));
        assert_eq!(report_of(r#"{"tab":0}"#), None);
        assert_eq!(report_of(r#"{"tab":-3}"#), None);
        assert_eq!(report_of(r#"{"tab":2000000000}"#), None);
        assert_eq!(report_of(r#"{"tab":"5"}"#), None);
        assert_eq!(report_of(r#"{"tab":5.5}"#), None);
        assert_eq!(report_of(r#"{"tab":5,"view":7}"#), None, "only the id");
        assert_eq!(report_of("{}"), None);
        assert_eq!(report_of("5"), None);
        assert_eq!(report_of("not json"), None);
    }

    #[test]
    fn only_the_main_frame_of_a_bridged_extension_page_shown_as_a_tab_may_report() {
        let view = Some(TabId(5));
        assert_eq!(
            report_source(true, &page_url(), view),
            Some((ID.to_string(), TabId(5)))
        );
        assert_eq!(report_source(false, &page_url(), view), None, "a subframe");
        assert_eq!(
            report_source(
                true,
                &format!("chrome-extension://{ID}/nautilus-bridge/relay.html"),
                view
            ),
            None,
            "the relay page"
        );
        assert_eq!(
            report_source(
                true,
                &format!("chrome-extension://{HELPER_ID}/helper.html"),
                view
            ),
            None,
            "the helper's page"
        );
        assert_eq!(
            report_source(true, "https://example.com/", view),
            None,
            "a web page"
        );
        assert_eq!(
            report_source(true, &page_url(), None),
            None,
            "a view that is no tab"
        );
    }
}
