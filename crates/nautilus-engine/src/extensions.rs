//! Chromium's extension system, driven from the outside.
//!
//! CEF exposes no extension API of its own, but the Chrome bootstrap it
//! runs on carries Chromium's whole extension system, and `chrome://
//! extensions` (which only a Chrome-style browser may load) talks to it
//! through `chrome.developerPrivate`. So the engine keeps one Chrome-style
//! browser on that page, in a Views window that is never shown (a
//! Chrome-style browser given a native parent instead opens a Chromium
//! window of its own), and runs scripts in it: list the extensions (and
//! again whenever the
//! system reports a change), switch developer mode on, load an unpacked
//! directory, enable, disable, uninstall. Results come back as console
//! messages prefixed `nautilus-ext:` that the display handler turns into
//! [`EngineEvent`]s.
//!
//! Installing from the store downloads the CRX with a CEF URL request,
//! unpacks it under the profile's `chromium/extensions/<id>/`, writes
//! the publisher's key into its manifest (so the unpacked copy keeps the
//! store id), then loads it as an unpacked directory: `loadUnpacked`
//! opens a folder dialog, which the dialog handler answers with that
//! path. Chromium remembers unpacked extensions across launches, so
//! nothing is needed at startup beyond the listing.
//!
//! Everything here is Windows only until the popups exist for macOS;
//! elsewhere `INFO.extensions` is `None` and the shell shows nothing.

use url::Url;

/// The id on a Chrome Web Store item page:
/// `https://chromewebstore.google.com/detail/<name>/<id>` (the old
/// `chrome.google.com/webstore/detail/<name>/<id>` too).
pub fn web_store_item_id(url: &Url) -> Option<String> {
    let mut segments = url.path_segments()?;
    let store = match url.host_str()? {
        "chromewebstore.google.com" => true,
        "chrome.google.com" => segments.next() == Some("webstore"),
        _ => false,
    };
    if !store || segments.next() != Some("detail") {
        return None;
    }
    segments
        .map(str::to_string)
        .find(|segment| crate::crx::is_extension_id(segment))
}

/// Store downloads are unpacked on a worker thread: writing an
/// extension's files takes seconds, and seconds spent inside a CEF
/// callback (the download's completion) leave CEF's external message
/// pump without further work requests, so nothing moves afterwards. The
/// outcome comes back here, and the engine hands it to the manager on
/// the next spin.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct UnpackQueue {
    outcomes: std::sync::Mutex<Vec<(String, Result<std::path::PathBuf, String>)>>,
    waker: std::sync::Arc<dyn osmena_engine::Waker>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl UnpackQueue {
    pub(crate) fn new(waker: std::sync::Arc<dyn osmena_engine::Waker>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(UnpackQueue {
            outcomes: std::sync::Mutex::new(Vec::new()),
            waker,
        })
    }

    fn push(&self, id: String, outcome: Result<std::path::PathBuf, String>) {
        self.outcomes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((id, outcome));
        self.waker.wake();
    }

    fn take(&self) -> Vec<(String, Result<std::path::PathBuf, String>)> {
        std::mem::take(
            &mut *self
                .outcomes
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

#[cfg(windows)]
pub(crate) use windows::ExtensionManager;

#[cfg(not(windows))]
pub(crate) use stub::ExtensionManager;

#[cfg(not(windows))]
mod stub {
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::Arc;

    use osmena_core::ExtensionSource;

    use super::UnpackQueue;
    use crate::delegate::DelegateState;

    /// Extensions are not wired up on this platform (see the module docs).
    pub(crate) struct ExtensionManager;

    impl ExtensionManager {
        pub(crate) fn start(
            _delegate: Rc<DelegateState>,
            _engine_dir: &Path,
            _unpacked: Arc<UnpackQueue>,
        ) -> Rc<Self> {
            Rc::new(ExtensionManager)
        }

        pub(crate) fn poll_unpacked(&self) {}

        pub(crate) fn settle_deadline(&self) -> Option<std::time::Instant> {
            None
        }

        pub(crate) fn install(&self, _source: ExtensionSource) {}

        pub(crate) fn set_enabled(&self, _id: &str, _enabled: bool) {}

        pub(crate) fn uninstall(&self, _id: &str) {}

        pub(crate) fn popup_url(&self, _id: &str) -> Option<String> {
            None
        }

        pub(crate) fn shutdown(&self) {}
    }
}

#[cfg(windows)]
mod windows {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cef::rc::Rc as _;
    use cef::{
        Browser, BrowserSettings, BrowserView, BrowserViewDelegate, CefString, CefStringList,
        ChromeToolbarType, Client, DevToolsMessageObserver, DialogHandler, DisplayHandler,
        FileDialogMode, Frame, ImplBrowser, ImplBrowserHost, ImplBrowserViewDelegate, ImplClient,
        ImplDevToolsMessageObserver, ImplDictionaryValue, ImplDisplayHandler, ImplFrame,
        ImplLifeSpanHandler, ImplLoadHandler, ImplPanel, ImplPanelDelegate, ImplRequest,
        ImplResponse, ImplRunFileDialogCallback, ImplUrlrequest, ImplUrlrequestClient,
        ImplViewDelegate, ImplWindow, ImplWindowDelegate, LifeSpanHandler, LoadHandler,
        LogSeverity, Rect, Registration, RunFileDialogCallback, RuntimeStyle, Urlrequest,
        UrlrequestClient, UrlrequestStatus, View, Window as CefWindow, WindowDelegate,
        WrapBrowserViewDelegate, WrapClient, WrapDevToolsMessageObserver, WrapDisplayHandler,
        WrapLifeSpanHandler, WrapLoadHandler, WrapRunFileDialogCallback, WrapUrlrequestClient,
        WrapWindowDelegate, wrap_browser_view_delegate, wrap_client,
        wrap_dev_tools_message_observer, wrap_display_handler, wrap_life_span_handler,
        wrap_load_handler, wrap_run_file_dialog_callback, wrap_urlrequest_client,
        wrap_window_delegate,
    };
    use osmena_core::{EngineEvent, Extension, ExtensionSource, Favicon};

    use super::UnpackQueue;
    use crate::crx;
    use crate::delegate::DelegateState;

    /// The console message the page's change listener logs.
    const CHANGED: &str = "nautilus-ext:changed";

    /// Chromium's update service, which redirects to the CRX of an
    /// extension; `prodversion` must be a Chromium version the store
    /// serves (any recent one does).
    const CRX_URL: &str = "https://clients2.google.com/service/update2/crx?\
         response=redirect&prodversion=154.0.0.0&acceptformat=crx3&x=id%3D{id}%26uc";

    // The scripts, each an expression yielding a promise of a plain
    // object, evaluated through the DevTools protocol (`Runtime.evaluate`
    // with `awaitPromise`), which is reliable for this browser where
    // `CefFrame::ExecuteJavaScript` is not, and returns the result.

    /// Developer mode on (unpacked loads need it) and a listener that
    /// reports changes through the console.
    const INIT_SCRIPT: &str = r#"new Promise(function(done){
  if (!window.__nautilusInit) {
    window.__nautilusInit = true;
    chrome.developerPrivate.onItemStateChanged.addListener(function(){ console.log("nautilus-ext:changed"); });
  }
  chrome.developerPrivate.updateProfileConfiguration({inDeveloperMode:true}, function(){
    var err = chrome.runtime.lastError;
    done({error: err ? err.message : null});
  });
})"#;

    const LIST_SCRIPT: &str = r#"new Promise(function(done){
  chrome.developerPrivate.getExtensionsInfo({includeDisabled:true, includeTerminated:true}, function(items){
    var err = chrome.runtime.lastError;
    done({error: err ? err.message : null, items:(items||[]).map(function(e){
      return {id:e.id, name:e.name, version:e.version, description:e.description,
              state:e.state, location:e.location, path:e.path, type:e.type};
    })});
  });
})"#;

    /// Load an unpacked extension; the folder dialog it opens is answered
    /// by the engine's dialog handler.
    const LOAD_SCRIPT: &str = r#"new Promise(function(done){
  chrome.developerPrivate.loadUnpacked({failQuietly:true, populateError:true}, function(r){
    var err = chrome.runtime.lastError;
    done({error:(r && r.error) || null, path:(r && r.path) || null, message:err ? err.message : null});
  });
})"#;

    const ENABLE_SCRIPT: &str = r#"new Promise(function(done){
  chrome.developerPrivate.updateExtensionConfiguration({extensionId:__ID__, enabled:__ENABLED__}, function(){
    var err = chrome.runtime.lastError;
    done({error: err ? err.message : null});
  });
})"#;

    const UNINSTALL_SCRIPT: &str = r#"new Promise(function(done){
  chrome.management.uninstall(__ID__, {showConfirmDialog:false}, function(){
    var err = chrome.runtime.lastError;
    done({error: err ? err.message : null});
  });
})"#;

    /// What a script was for, to handle its result.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Request {
        Init,
        List,
        Load,
        Enable,
        Uninstall { id: String },
    }

    /// What the manager remembers of an extension beyond the model.
    struct Known {
        id: String,
        /// The popup page of its toolbar action, relative to its root.
        popup: Option<String>,
    }

    struct Inner {
        browser: Option<Browser>,
        /// The DevTools observer stays registered while this lives.
        devtools: Option<Registration>,
        /// The page loaded and the init script is on its way.
        ready: bool,
        /// Scripts asked for before the page was ready.
        queued: Vec<(String, Request)>,
        next_message_id: i32,
        /// Scripts evaluated and not yet answered, by message id.
        pending: HashMap<i32, Request>,
        /// A change arrived while a listing was pending: list again.
        list_again: bool,
        known: Vec<Known>,
        /// Store downloads in flight (CEF needs the request kept alive).
        downloads: Vec<Urlrequest>,
        /// The never-shown Views window holding the browser.
        window: Option<CefWindow>,
    }

    /// The management browser (never shown) and what it learnt. One per
    /// engine.
    pub(crate) struct ExtensionManager {
        delegate: Rc<DelegateState>,
        /// `<engine_dir>/extensions`, where store downloads are unpacked.
        store_dir: PathBuf,
        /// Where the unpacking threads report.
        unpacked: Arc<UnpackQueue>,
        /// When an extension was last installed, switched, or removed:
        /// Chromium writes its preferences up to ten seconds later, and
        /// nothing in CEF's shutdown flushes them (every exit counts as a
        /// crash to Chromium), so the engine pumps that long before it
        /// shuts down.
        last_change: Cell<Option<Instant>>,
        inner: RefCell<Inner>,
    }

    /// Chromium's preference write delay (`ImportantFileWriter`), plus a
    /// little.
    const PREFS_SETTLE: Duration = Duration::from_secs(11);

    impl ExtensionManager {
        /// Create the management browser in a window that is never
        /// shown; it reports the extensions once it is up.
        pub(crate) fn start(
            delegate: Rc<DelegateState>,
            engine_dir: &Path,
            unpacked: Arc<UnpackQueue>,
        ) -> Rc<Self> {
            let manager = Rc::new(ExtensionManager {
                delegate,
                store_dir: engine_dir.join("extensions"),
                unpacked,
                last_change: Cell::new(None),
                inner: RefCell::new(Inner {
                    browser: None,
                    devtools: None,
                    ready: false,
                    queued: Vec::new(),
                    next_message_id: 1,
                    pending: HashMap::new(),
                    list_again: false,
                    known: Vec::new(),
                    downloads: Vec::new(),
                    window: None,
                }),
            });
            let mut client = ManagerClient::new(
                ManagerLoad::new(manager.clone()),
                ManagerDisplay::new(manager.clone()),
                manager.delegate.dialog_handler(),
                ManagerLife::new(manager.clone()),
            );
            let mut view_delegate = ManagerViewDelegate::new();
            let browser_view = cef::browser_view_create(
                Some(&mut client),
                Some(&CefString::from("chrome://extensions")),
                Some(&BrowserSettings::default()),
                None,
                None,
                Some(&mut view_delegate),
            );
            let Some(browser_view) = browser_view else {
                log::error!("Chromium could not create the extension manager browser");
                return manager;
            };
            let mut window_delegate = ManagerWindowDelegate::new(browser_view);
            match cef::window_create_top_level(Some(&mut window_delegate)) {
                Some(window) => manager.inner.borrow_mut().window = Some(window),
                None => log::error!("Chromium could not create the extension manager window"),
            }
            manager
        }

        /// Evaluate `script` in the management page, now or once it is
        /// ready; the result comes back to `on_result` as `request`.
        fn run(&self, script: String, request: Request) {
            let mut inner = self.inner.borrow_mut();
            if !inner.ready {
                log::debug!("extension manager not ready; queued {request:?}");
                inner.queued.push((script, request));
                return;
            }
            let Some(host) = inner.browser.as_ref().and_then(|b| b.host()) else {
                log::warn!("extension manager has no browser for {request:?}");
                return;
            };
            let Some(mut params) = cef::dictionary_value_create() else {
                return;
            };
            params.set_string(
                Some(&"expression".into()),
                Some(&CefString::from(script.as_str())),
            );
            params.set_bool(Some(&"awaitPromise".into()), 1);
            params.set_bool(Some(&"returnByValue".into()), 1);
            let id = inner.next_message_id;
            inner.next_message_id += 1;
            log::debug!("extension manager evaluates {request:?} as message {id}");
            inner.pending.insert(id, request);
            host.execute_dev_tools_method(id, Some(&"Runtime.evaluate".into()), Some(&mut params));
        }

        fn list(&self) {
            {
                let mut inner = self.inner.borrow_mut();
                if inner.pending.values().any(|r| *r == Request::List) {
                    inner.list_again = true;
                    return;
                }
            }
            self.run(LIST_SCRIPT.to_string(), Request::List);
        }

        /// The page said something changed.
        fn on_change(&self) {
            self.list();
        }

        fn report_error(&self, message: String) {
            log::error!("{message}");
            self.delegate.push(EngineEvent::EngineError(message));
        }

        pub(crate) fn install(self: &Rc<Self>, source: ExtensionSource) {
            match source {
                ExtensionSource::Directory(path) => self.load_unpacked(path),
                ExtensionSource::AskForDirectory => self.ask_for_directory(),
                ExtensionSource::Store { id } => self.install_from_store(&id),
            }
        }

        /// Load the extension in `dir`: `loadUnpacked` opens a folder
        /// dialog, which the engine's dialog handler answers with `dir`.
        fn load_unpacked(&self, dir: PathBuf) {
            self.delegate.answer_next_dialog(dir);
            self.run(LOAD_SCRIPT.to_string(), Request::Load);
        }

        /// Ask the user for the directory, with a dialog owned by the
        /// focused tab's window (the manager's own window is never shown,
        /// and a dialog owned by it would be invisible and modal), then
        /// load it.
        fn ask_for_directory(self: &Rc<Self>) {
            let Some(host) = self.delegate.focused_browser().and_then(|b| b.host()) else {
                self.report_error("no window to ask for the extension's directory".to_string());
                return;
            };
            let mut callback = DirectoryPicked::new(self.clone());
            host.run_file_dialog(
                FileDialogMode::OPEN_FOLDER,
                Some(&"Load unpacked extension".into()),
                None,
                None,
                Some(&mut callback),
            );
        }

        fn install_from_store(self: &Rc<Self>, id: &str) {
            if !crx::is_extension_id(id) {
                self.report_error(format!("{id:?} is not an extension id"));
                return;
            }
            let Some(mut request) = cef::request_create() else {
                self.report_error("cannot create the download request".to_string());
                return;
            };
            let url = CRX_URL.replace("{id}", id);
            request.set_url(Some(&CefString::from(url.as_str())));
            let mut client =
                CrxDownload::new(self.clone(), id.to_string(), RefCell::new(Vec::new()));
            match cef::urlrequest_create(Some(&mut request), Some(&mut client), None) {
                Some(download) => self.inner.borrow_mut().downloads.push(download),
                None => self.report_error("cannot start the extension download".to_string()),
            }
        }

        /// The store served `bytes` for `id`: unpack it on a worker thread
        /// (see [`UnpackQueue`]), then load it from [`Self::poll_unpacked`].
        fn store_download_finished(&self, id: &str, bytes: Vec<u8>) {
            let dir = self.store_dir.join(id);
            log::debug!("downloaded {} bytes for extension {id}", bytes.len());
            let queue = self.unpacked.clone();
            let name = id.to_string();
            let spawned = std::thread::Builder::new()
                .name("NautilusUnpack".to_string())
                .spawn(move || {
                    let outcome = unpack_crx(&name, &bytes, &dir).map(|()| dir);
                    queue.push(name, outcome);
                });
            if let Err(e) = spawned {
                self.report_error(format!("cannot start unpacking extension {id}: {e}"));
            }
        }

        /// Until when the engine should keep pumping before shutting down
        /// so that Chromium's delayed preference write (which records an
        /// install, switch, or removal) has happened; `None` when nothing
        /// is pending.
        pub(crate) fn settle_deadline(&self) -> Option<Instant> {
            let deadline = self.last_change.get()? + PREFS_SETTLE;
            (deadline > Instant::now()).then_some(deadline)
        }

        /// Load what the unpacking threads finished. Called by the engine
        /// on every spin.
        pub(crate) fn poll_unpacked(&self) {
            for (id, outcome) in self.unpacked.take() {
                match outcome {
                    Ok(dir) => self.load_unpacked(dir),
                    Err(e) => self.report_error(format!("cannot install extension {id}: {e}")),
                }
            }
        }

        pub(crate) fn set_enabled(&self, id: &str, enabled: bool) {
            self.run(
                ENABLE_SCRIPT
                    .replace("__ID__", &js_string(id))
                    .replace("__ENABLED__", if enabled { "true" } else { "false" }),
                Request::Enable,
            );
        }

        pub(crate) fn uninstall(&self, id: &str) {
            self.run(
                UNINSTALL_SCRIPT.replace("__ID__", &js_string(id)),
                Request::Uninstall { id: id.to_string() },
            );
        }

        /// The URL of `id`'s action popup, if it has one.
        pub(crate) fn popup_url(&self, id: &str) -> Option<String> {
            let inner = self.inner.borrow();
            let known = inner.known.iter().find(|k| k.id == id)?;
            let popup = known.popup.as_ref()?;
            Some(format!(
                "chrome-extension://{id}/{}",
                popup.trim_start_matches('/')
            ))
        }

        /// Close the management window and its browser; the engine then
        /// pumps until they are gone.
        pub(crate) fn shutdown(&self) {
            let mut inner = self.inner.borrow_mut();
            inner.ready = false;
            inner.queued.clear();
            inner.pending.clear();
            inner.downloads.clear();
            inner.devtools = None;
            if let Some(host) = inner.browser.take().and_then(|b| b.host()) {
                host.close_browser(1);
            }
            if let Some(window) = inner.window.take() {
                window.close();
            }
        }

        /// The page loaded: hook the DevTools channel up, initialise,
        /// then run what waited.
        fn on_page_ready(self: &Rc<Self>) {
            let queued = {
                let mut inner = self.inner.borrow_mut();
                if inner.devtools.is_none()
                    && let Some(host) = inner.browser.as_ref().and_then(|b| b.host())
                {
                    let mut observer = ManagerDevTools::new(self.clone());
                    inner.devtools = host.add_dev_tools_message_observer(Some(&mut observer));
                }
                inner.ready = true;
                std::mem::take(&mut inner.queued)
            };
            self.run(INIT_SCRIPT.to_string(), Request::Init);
            for (script, request) in queued {
                self.run(script, request);
            }
        }

        /// A `Runtime.evaluate` answered.
        fn on_result(self: &Rc<Self>, message_id: i32, success: bool, result: &[u8]) {
            let Some(request) = self.inner.borrow_mut().pending.remove(&message_id) else {
                return;
            };
            let parsed: serde_json::Value = serde_json::from_slice(result).unwrap_or_default();
            let value = if success {
                if let Some(exception) = parsed.get("exceptionDetails") {
                    let text = exception
                        .get("exception")
                        .and_then(|e| e.get("description"))
                        .and_then(|d| d.as_str())
                        .unwrap_or("script error");
                    self.report_error(format!("extension manager script failed: {text}"));
                    return;
                }
                parsed
                    .get("result")
                    .and_then(|r| r.get("value"))
                    .cloned()
                    .unwrap_or_default()
            } else {
                let text = parsed
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error");
                self.report_error(format!("extension manager call failed: {text}"));
                return;
            };
            let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
            let error = text("error");
            log::debug!("extension manager {request:?} answered; error {error:?}");
            match request {
                Request::Init => {
                    if let Some(error) = error {
                        self.report_error(format!(
                            "extension manager could not turn developer mode on: {error}"
                        ));
                    }
                    self.list();
                }
                Request::List => {
                    match error {
                        Some(error) => {
                            self.report_error(format!("cannot list the extensions: {error}"));
                        }
                        None => {
                            let items = value
                                .get("items")
                                .and_then(|v| v.as_array())
                                .cloned()
                                .unwrap_or_default();
                            self.on_list(&items);
                        }
                    }
                    if std::mem::take(&mut self.inner.borrow_mut().list_again) {
                        self.list();
                    }
                }
                Request::Load => {
                    self.last_change.set(Some(Instant::now()));
                    if let Some(error) = value.get("error").filter(|e| e.is_object()) {
                        let what = error
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("unknown error");
                        let path = error.get("path").and_then(|p| p.as_str()).unwrap_or("");
                        self.report_error(format!("cannot load the extension in {path}: {what}"));
                    } else if let Some(message) = text("message") {
                        // A cancelled folder dialog reports through here
                        // too, and is not an error.
                        if !message.to_lowercase().contains("cancel") {
                            self.report_error(format!("cannot load the extension: {message}"));
                        }
                    }
                    self.list();
                }
                Request::Enable => {
                    self.last_change.set(Some(Instant::now()));
                    if let Some(error) = error {
                        self.report_error(format!("cannot change the extension: {error}"));
                    }
                    self.list();
                }
                Request::Uninstall { id } => {
                    self.last_change.set(Some(Instant::now()));
                    match error {
                        Some(error) => {
                            self.report_error(format!("cannot remove the extension: {error}"));
                        }
                        None => {
                            // A store download of it is ours to delete.
                            let dir = self.store_dir.join(&id);
                            if crx::is_extension_id(&id)
                                && dir.is_dir()
                                && let Err(e) = std::fs::remove_dir_all(&dir)
                            {
                                log::warn!("cannot delete {}: {e}", dir.display());
                            }
                        }
                    }
                    self.list();
                }
            }
        }

        /// The current list: remember the popups, report the model's view.
        fn on_list(&self, items: &[serde_json::Value]) {
            let mut known = Vec::new();
            let mut extensions = Vec::new();
            for item in items {
                let text = |key: &str| {
                    item.get(key)
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string()
                };
                let kind = text("type");
                if !kind.is_empty() && kind != "EXTENSION" {
                    continue;
                }
                let id = text("id");
                if id.is_empty() {
                    continue;
                }
                let path = text("path");
                let manifest = if path.is_empty() {
                    ManifestInfo::default()
                } else {
                    read_manifest(Path::new(&path))
                };
                let name = text("name");
                known.push(Known {
                    id: id.clone(),
                    popup: manifest.popup.clone(),
                });
                extensions.push(Extension {
                    id,
                    name: name.clone(),
                    version: text("version"),
                    description: text("description"),
                    enabled: text("state") == "ENABLED",
                    icon: manifest.icon,
                    action_title: manifest
                        .popup
                        .is_some()
                        .then(|| manifest.action_title.unwrap_or(name)),
                });
            }
            log::info!(
                "{} extension(s) installed: {}",
                extensions.len(),
                extensions
                    .iter()
                    .map(|e| format!(
                        "{} ({}, {})",
                        e.name,
                        e.id,
                        if e.enabled { "on" } else { "off" }
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            self.inner.borrow_mut().known = known;
            self.delegate
                .push(EngineEvent::ExtensionsChanged(extensions));
        }
    }

    /// A JavaScript string literal.
    fn js_string(text: &str) -> String {
        serde_json::Value::String(text.to_string()).to_string()
    }

    /// Unpack a store CRX into `dir` (replacing what is there) and write
    /// the publisher's key into its manifest so the unpacked copy keeps
    /// the store id.
    fn unpack_crx(id: &str, bytes: &[u8], dir: &Path) -> Result<(), String> {
        let crx = crx::parse(bytes).map_err(|e| e.to_string())?;
        if crx.id != id {
            return Err(format!("the store served extension {} instead", crx.id));
        }
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| format!("clearing {}: {e}", dir.display()))?;
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(crx.archive))
            .map_err(|e| format!("the extension archive is unreadable: {e}"))?;
        archive
            .extract(dir)
            .map_err(|e| format!("unpacking into {}: {e}", dir.display()))?;
        let manifest_path = dir.join("manifest.json");
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("the extension has no manifest: {e}"))?;
        let mut manifest: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("the manifest is not JSON: {e}"))?;
        let Some(object) = manifest.as_object_mut() else {
            return Err("the manifest is not a JSON object".to_string());
        };
        object.insert(
            "key".to_string(),
            serde_json::Value::String(crx::base64(&crx.public_key)),
        );
        let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        std::fs::write(&manifest_path, text).map_err(|e| format!("writing the manifest: {e}"))?;
        Ok(())
    }

    /// What the manifest adds to the listing.
    #[derive(Default)]
    struct ManifestInfo {
        popup: Option<String>,
        action_title: Option<String>,
        icon: Option<Favicon>,
    }

    /// The action and icon from an extension's `manifest.json`. Anything
    /// missing or unreadable is simply absent.
    fn read_manifest(dir: &Path) -> ManifestInfo {
        let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
            return ManifestInfo::default();
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
            return ManifestInfo::default();
        };
        let action = ["action", "browser_action", "page_action"]
            .iter()
            .find_map(|key| manifest.get(*key))
            .filter(|a| a.is_object());
        let string = |v: Option<&serde_json::Value>| {
            v.and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let popup = string(action.and_then(|a| a.get("default_popup")));
        // A localised title is a `__MSG_name__` key; the name serves then.
        let action_title = string(action.and_then(|a| a.get("default_title")))
            .filter(|t| !t.starts_with("__MSG_"));
        let icon_path = largest_icon(manifest.get("icons"))
            .or_else(|| largest_icon(action.and_then(|a| a.get("default_icon"))))
            .or_else(|| string(action.and_then(|a| a.get("default_icon"))));
        let icon = icon_path.and_then(|p| decode_png(&dir.join(p.trim_start_matches('/'))));
        ManifestInfo {
            popup,
            action_title,
            icon,
        }
    }

    /// The path of the largest icon of a `{"16": "a.png", ...}` map that
    /// is no bigger than 128 (bigger ones are splash art), or the
    /// largest of all otherwise.
    fn largest_icon(icons: Option<&serde_json::Value>) -> Option<String> {
        let map = icons?.as_object()?;
        let mut sizes: Vec<(u32, &str)> = map
            .iter()
            .filter_map(|(size, path)| Some((size.parse().ok()?, path.as_str()?)))
            .collect();
        sizes.sort_unstable();
        sizes
            .iter()
            .rev()
            .find(|(size, _)| *size <= 128)
            .or(sizes.last())
            .map(|(_, path)| path.to_string())
    }

    /// Decode a PNG file to straight RGBA8, whatever its colour type.
    fn decode_png(path: &Path) -> Option<Favicon> {
        let bytes = std::fs::read(path).ok()?;
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().ok()?;
        let mut buffer = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buffer).ok()?;
        buffer.truncate(info.buffer_size());
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => buffer,
            png::ColorType::Rgb => buffer
                .chunks_exact(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::GrayscaleAlpha => buffer
                .chunks_exact(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::Indexed => return None,
        };
        if rgba.len() != info.width as usize * info.height as usize * 4 {
            return None;
        }
        Some(Favicon {
            width: info.width,
            height: info.height,
            rgba: Arc::from(rgba),
        })
    }

    wrap_browser_view_delegate! {
        struct ManagerViewDelegate {}

        impl ViewDelegate {}

        impl BrowserViewDelegate {
            /// Chrome style: only it may load `chrome://extensions`.
            fn browser_runtime_style(&self) -> RuntimeStyle {
                RuntimeStyle::CHROME
            }

            fn chrome_toolbar_type(
                &self,
                _browser_view: Option<&mut BrowserView>,
            ) -> ChromeToolbarType {
                ChromeToolbarType::NONE
            }
        }
    }

    wrap_window_delegate! {
        struct ManagerWindowDelegate {
            browser_view: BrowserView,
        }

        impl ViewDelegate {}

        impl PanelDelegate {}

        impl WindowDelegate {
            /// The browser goes into the window, which is never shown.
            fn on_window_created(&self, window: Option<&mut CefWindow>) {
                if let Some(window) = window {
                    window.add_child_view(Some(&mut View::from(&self.browser_view)));
                }
            }

            fn initial_bounds(&self, _window: Option<&mut CefWindow>) -> Rect {
                Rect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 600,
                }
            }

            fn is_frameless(&self, _window: Option<&mut CefWindow>) -> ::std::os::raw::c_int {
                1
            }
        }
    }
    wrap_client! {
        struct ManagerClient {
            load: LoadHandler,
            display: DisplayHandler,
            dialog: DialogHandler,
            life: LifeSpanHandler,
        }

        impl Client {
            fn load_handler(&self) -> Option<LoadHandler> {
                Some(self.load.clone())
            }

            fn display_handler(&self) -> Option<DisplayHandler> {
                Some(self.display.clone())
            }

            fn dialog_handler(&self) -> Option<DialogHandler> {
                Some(self.dialog.clone())
            }

            fn life_span_handler(&self) -> Option<LifeSpanHandler> {
                Some(self.life.clone())
            }
        }
    }

    wrap_life_span_handler! {
        struct ManagerLife {
            manager: Rc<ExtensionManager>,
        }

        impl LifeSpanHandler {
            fn on_after_created(&self, browser: Option<&mut Browser>) {
                self.manager.delegate.add_live(1);
                self.manager.inner.borrow_mut().browser = browser.map(|b| b.clone());
            }

            fn on_before_close(&self, _browser: Option<&mut Browser>) {
                self.manager.delegate.add_live(-1);
                let mut inner = self.manager.inner.borrow_mut();
                inner.browser = None;
                inner.devtools = None;
                inner.ready = false;
                inner.pending.clear();
            }
        }
    }

    wrap_load_handler! {
        struct ManagerLoad {
            manager: Rc<ExtensionManager>,
        }

        impl LoadHandler {
            fn on_load_end(
                &self,
                _browser: Option<&mut Browser>,
                frame: Option<&mut Frame>,
                _http_status_code: ::std::os::raw::c_int,
            ) {
                if frame.is_some_and(|f| f.is_main() != 0) {
                    self.manager.on_page_ready();
                }
            }
        }
    }

    wrap_display_handler! {
        struct ManagerDisplay {
            manager: Rc<ExtensionManager>,
        }

        impl DisplayHandler {
            fn on_console_message(
                &self,
                _browser: Option<&mut Browser>,
                _level: LogSeverity,
                message: Option<&CefString>,
                _source: Option<&CefString>,
                _line: ::std::os::raw::c_int,
            ) -> ::std::os::raw::c_int {
                let message = message.map(CefString::to_string).unwrap_or_default();
                if message == CHANGED {
                    self.manager.on_change();
                } else {
                    log::debug!("extension manager page: {message}");
                }
                1
            }
        }
    }

    wrap_dev_tools_message_observer! {
        struct ManagerDevTools {
            manager: Rc<ExtensionManager>,
        }

        impl DevToolsMessageObserver {
            fn on_dev_tools_method_result(
                &self,
                _browser: Option<&mut Browser>,
                message_id: ::std::os::raw::c_int,
                success: ::std::os::raw::c_int,
                result: Option<&[u8]>,
            ) {
                self.manager
                    .on_result(message_id, success != 0, result.unwrap_or_default());
            }
        }
    }

    wrap_run_file_dialog_callback! {
        struct DirectoryPicked {
            manager: Rc<ExtensionManager>,
        }

        impl RunFileDialogCallback {
            fn on_file_dialog_dismissed(&self, file_paths: Option<&mut CefStringList>) {
                let picked = file_paths
                    .and_then(|paths| paths.clone().into_iter().next())
                    .filter(|path| !path.is_empty());
                if let Some(path) = picked {
                    self.manager.load_unpacked(PathBuf::from(path));
                }
            }
        }
    }

    wrap_urlrequest_client! {
        struct CrxDownload {
            manager: Rc<ExtensionManager>,
            id: String,
            bytes: RefCell<Vec<u8>>,
        }

        impl UrlrequestClient {
            fn on_download_data(
                &self,
                _request: Option<&mut Urlrequest>,
                data: *const u8,
                data_length: usize,
            ) {
                if data.is_null() || data_length == 0 {
                    return;
                }
                // SAFETY: CEF hands a buffer of `data_length` bytes that
                // stays valid for the duration of this callback.
                let chunk = unsafe { std::slice::from_raw_parts(data, data_length) };
                self.bytes.borrow_mut().extend_from_slice(chunk);
            }

            fn on_request_complete(&self, request: Option<&mut Urlrequest>) {
                log::debug!(
                    "download complete; on UI thread: {}, on IO thread: {}",
                    cef::currently_on(cef::ThreadId::UI),
                    cef::currently_on(cef::ThreadId::IO)
                );
                let Some(request) = request else {
                    return;
                };
                let status = request.response().map(|r| r.status()).unwrap_or(0);
                let bytes = std::mem::take(&mut *self.bytes.borrow_mut());
                let manager = &self.manager;
                manager.inner.borrow_mut().downloads.clear();
                if request.request_status() != UrlrequestStatus::SUCCESS || status != 200 {
                    let why = if status == 204 || status == 404 {
                        "the store has no such extension".to_string()
                    } else if status != 0 {
                        format!("the store answered HTTP {status}")
                    } else {
                        format!("the download failed ({:?})", request.request_error())
                    };
                    manager.report_error(format!(
                        "cannot download extension {}: {why}",
                        self.id
                    ));
                    return;
                }
                manager.store_download_finished(&self.id, bytes);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::web_store_item_id;
    use url::Url;

    #[test]
    fn store_item_pages_yield_the_id() {
        let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
        for page in [
            format!("https://chromewebstore.google.com/detail/ublock-origin/{id}"),
            format!("https://chromewebstore.google.com/detail/ublock-origin/{id}?hl=en"),
            format!("https://chromewebstore.google.com/detail/{id}"),
            format!("https://chrome.google.com/webstore/detail/ublock-origin/{id}"),
        ] {
            assert_eq!(
                web_store_item_id(&Url::parse(&page).unwrap()).as_deref(),
                Some(id),
                "{page}"
            );
        }
    }

    #[test]
    fn other_pages_do_not() {
        for page in [
            "https://chromewebstore.google.com/",
            "https://chromewebstore.google.com/category/extensions",
            "https://chromewebstore.google.com/detail/not-an-id",
            "https://example.com/detail/cjpalhdlnbpafiamejdnhcphjbkeiagm",
            "https://chrome.google.com/detail/cjpalhdlnbpafiamejdnhcphjbkeiagm",
        ] {
            assert_eq!(
                web_store_item_id(&Url::parse(page).unwrap()),
                None,
                "{page}"
            );
        }
    }
}
