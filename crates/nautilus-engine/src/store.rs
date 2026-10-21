//! The Chrome Web Store's own install buttons, rerouted to Nautilus.
//!
//! The store's pages call `chrome.webstorePrivate` ("Add to Chrome") and
//! `chrome.management` ("Remove from Chrome", the enable switch), which
//! Chromium answers with its install and uninstall dialogs. Those are
//! modal to the tab's native window, and a windowless browser has none:
//! Chromium dereferences null in `ExtensionInstallPrompt` and the process
//! dies (`docs/upstream-issues.md`). So in the renderer, when a store page's
//! main-frame context is created, [`StoreRenderer`] binds a function on
//! `window` and runs [`SHIM`], which replaces those API methods with ones
//! that call the function and answer the page at once. The function sends
//! a process message to the browser process, where the tab's client turns
//! it into a [`StoreRequest`] for the extension manager (`extensions.rs`):
//! the same download, unpack, and load as the puzzle menu's "Install the
//! extension on this page".
//!
//! Trust: the function exists only in main-frame contexts of the store's
//! origin, and the browser process checks the message's frame again
//! before acting, so no other page can ask for an install.

use std::fmt;

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, Frame, ImplFrame, ImplListValue, ImplProcessMessage,
    ImplRenderProcessHandler, ImplV8Context, ImplV8Handler, ImplV8Value, ProcessId, ProcessMessage,
    RenderProcessHandler, V8Context, V8Handler, V8Propertyattribute, V8Value,
    WrapRenderProcessHandler, WrapV8Handler, wrap_render_process_handler, wrap_v8_handler,
};
use url::Url;

use crate::crx;

/// The function the renderer binds on a store page's `window`.
const REQUEST_FUNCTION: &str = "__nautilusExtensionRequest";

/// The process message it sends: arguments `[kind, id]`.
const REQUEST_MESSAGE: &str = "nautilus-ext-request";

/// Run in a store page's main frame once the function is bound. The
/// replaced methods answer with a callback or a promise, whichever the
/// caller used, as the real ones do.
const SHIM: &str = r#"(function () {
  var request = window.__nautilusExtensionRequest;
  if (typeof request !== "function") { return; }
  delete window.__nautilusExtensionRequest;
  function answer(args, result) {
    var callback = args[args.length - 1];
    if (typeof callback === "function") { callback(result); return undefined; }
    return Promise.resolve(result);
  }
  function replace(object, name, implementation) {
    Object.defineProperty(object, name, { value: implementation, writable: true, configurable: true });
  }
  function patch() {
    var chrome = window.chrome;
    var store = chrome && chrome.webstorePrivate;
    if (!store) { return false; }
    if (store.__nautilus) { return true; }
    replace(store, "beginInstallWithManifest3", function (details) {
      if (details && typeof details.id === "string") { request("install", details.id); }
      return answer(arguments, "user_cancelled");
    });
    replace(store, "install", function (id) {
      if (typeof id === "string") { request("install", id); }
      return answer(arguments, undefined);
    });
    replace(store, "completeInstall", function () { return answer(arguments, undefined); });
    replace(store, "requestExtension", function () { return answer(arguments, "user_cancelled"); });
    var management = chrome.management;
    if (management) {
      replace(management, "uninstall", function (id) {
        if (typeof id === "string") { request("uninstall", id); }
        return answer(arguments, undefined);
      });
      replace(management, "setEnabled", function (id, enabled) {
        if (typeof id === "string") { request(enabled ? "enable" : "disable", id); }
        return answer(arguments, undefined);
      });
    }
    Object.defineProperty(store, "__nautilus", { value: true });
    return true;
  }
  try { if (patch()) { return; } } catch (e) { console.log("nautilus: store shim failed: " + e); }
  var tries = 0;
  var timer = setInterval(function () {
    try { if (patch() || ++tries > 200) { clearInterval(timer); } }
    catch (e) { clearInterval(timer); console.log("nautilus: store shim failed: " + e); }
  }, 50);
})();"#;

/// Whether `url` is a page of the Chrome Web Store (either host it has
/// lived on), over HTTPS.
pub fn is_web_store(url: &Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    match url.host_str() {
        Some("chromewebstore.google.com") => true,
        Some("chrome.google.com") => url
            .path_segments()
            .is_some_and(|mut segments| segments.next() == Some("webstore")),
        _ => false,
    }
}

/// What a store page asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StoreRequest {
    Install(String),
    Uninstall(String),
    Enable(String),
    Disable(String),
}

impl StoreRequest {
    /// The request named `kind` for the extension `id`, if both are
    /// well-formed.
    pub(crate) fn parse(kind: &str, id: &str) -> Option<Self> {
        if !crx::is_extension_id(id) {
            return None;
        }
        let id = id.to_string();
        Some(match kind {
            "install" => StoreRequest::Install(id),
            "uninstall" => StoreRequest::Uninstall(id),
            "enable" => StoreRequest::Enable(id),
            "disable" => StoreRequest::Disable(id),
            _ => return None,
        })
    }

    fn kind(&self) -> &'static str {
        match self {
            StoreRequest::Install(_) => "install",
            StoreRequest::Uninstall(_) => "uninstall",
            StoreRequest::Enable(_) => "enable",
            StoreRequest::Disable(_) => "disable",
        }
    }

    fn id(&self) -> &str {
        match self {
            StoreRequest::Install(id)
            | StoreRequest::Uninstall(id)
            | StoreRequest::Enable(id)
            | StoreRequest::Disable(id) => id,
        }
    }
}

impl fmt::Display for StoreRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind(), self.id())
    }
}

/// Browser process: the request a tab's process message carries, if it is
/// the store shim's message from the main frame of a store page.
pub(crate) fn request_from_message(
    frame: &Frame,
    message: &ProcessMessage,
) -> Option<StoreRequest> {
    if CefString::from(&message.name()).to_string() != REQUEST_MESSAGE || frame.is_main() == 0 {
        return None;
    }
    let url = Url::parse(&CefString::from(&frame.url()).to_string()).ok()?;
    if !is_web_store(&url) {
        return None;
    }
    let arguments = message.argument_list()?;
    if arguments.size() < 2 {
        return None;
    }
    StoreRequest::parse(
        &CefString::from(&arguments.string(0)).to_string(),
        &CefString::from(&arguments.string(1)).to_string(),
    )
}

// Render process: binds the request function and runs the shim in every
// main-frame context of a store page.
wrap_render_process_handler! {
    pub(crate) struct StoreRenderer;

    impl RenderProcessHandler {
        fn on_context_created(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            context: Option<&mut V8Context>,
        ) {
            let (Some(frame), Some(context)) = (frame, context) else {
                return;
            };
            let url = CefString::from(&frame.url()).to_string();
            {
                let (mut r, mut e) = (None, None);
                context.eval(Some(&CefString::from(format!("console.log('nautilus-diag context main={} url={url}')", frame.is_main()).as_str())), None, 0, Some(&mut r), Some(&mut e));
            }
            if frame.is_main() == 0 {
                return;
            }
            if !Url::parse(&url).is_ok_and(|url| is_web_store(&url)) {
                return;
            }
            let Some(global) = context.global() else {
                return;
            };
            let name = CefString::from(REQUEST_FUNCTION);
            let mut handler = StoreRequestFunction::new();
            let Some(mut function) =
                cef::v8_value_create_function(Some(&name), Some(&mut handler))
            else {
                return;
            };
            global.set_value_bykey(Some(&name), Some(&mut function), V8Propertyattribute::default());
            // CEF's `eval` does nothing without both out-pointers.
            let (mut retval, mut exception) = (None, None);
            context.eval(
                Some(&CefString::from(SHIM)),
                None,
                0,
                Some(&mut retval),
                Some(&mut exception),
            );
        }
    }
}

// The bound function: `(kind, id)` becomes a process message to the
// browser process.
wrap_v8_handler! {
    struct StoreRequestFunction;

    impl V8Handler {
        fn execute(
            &self,
            _name: Option<&CefString>,
            _object: Option<&mut V8Value>,
            arguments: Option<&[Option<V8Value>]>,
            _retval: Option<&mut Option<V8Value>>,
            _exception: Option<&mut CefString>,
        ) -> ::std::os::raw::c_int {
            let Some([Some(kind), Some(id), ..]) = arguments else {
                return 0;
            };
            if kind.is_string() == 0 || id.is_string() == 0 {
                return 0;
            }
            let kind = CefString::from(&kind.string_value()).to_string();
            let id = CefString::from(&id.string_value()).to_string();
            let Some(request) = StoreRequest::parse(&kind, &id) else {
                return 0;
            };
            let Some(frame) = cef::v8_context_get_current_context().and_then(|c| c.frame()) else {
                return 0;
            };
            let Some(mut message) =
                cef::process_message_create(Some(&CefString::from(REQUEST_MESSAGE)))
            else {
                return 0;
            };
            if let Some(arguments) = message.argument_list() {
                arguments.set_string(0, Some(&CefString::from(request.kind())));
                arguments.set_string(1, Some(&CefString::from(request.id())));
            }
            frame.send_process_message(ProcessId::BROWSER, Some(&mut message));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "ghmbeldphafepmbegfdlkpapadhbakde";

    #[test]
    fn store_pages_are_recognised() {
        for url in [
            "https://chromewebstore.google.com/".to_string(),
            format!("https://chromewebstore.google.com/detail/proton-pass/{ID}"),
            format!("https://chrome.google.com/webstore/detail/x/{ID}"),
        ] {
            assert!(is_web_store(&Url::parse(&url).unwrap()), "{url}");
        }
        for url in [
            "http://chromewebstore.google.com/",
            "https://chrome.google.com/",
            "https://chromewebstore.google.com.evil.example/",
            "https://example.com/chromewebstore.google.com/",
        ] {
            assert!(!is_web_store(&Url::parse(url).unwrap()), "{url}");
        }
    }

    #[test]
    fn requests_need_a_known_kind_and_a_real_id() {
        assert_eq!(
            StoreRequest::parse("install", ID),
            Some(StoreRequest::Install(ID.to_string()))
        );
        assert_eq!(
            StoreRequest::parse("disable", ID),
            Some(StoreRequest::Disable(ID.to_string()))
        );
        assert_eq!(StoreRequest::parse("install", "not-an-id"), None);
        assert_eq!(StoreRequest::parse("format", ID), None);
        assert_eq!(
            StoreRequest::Uninstall(ID.to_string()).to_string(),
            format!("uninstall {ID}")
        );
    }

    #[test]
    fn the_shim_replaces_every_dialog_showing_method() {
        for method in [
            "beginInstallWithManifest3",
            "\"install\"",
            "completeInstall",
            "requestExtension",
            "uninstall",
            "setEnabled",
        ] {
            assert!(SHIM.contains(method), "{method}");
        }
        assert!(SHIM.contains(REQUEST_FUNCTION));
    }
}
