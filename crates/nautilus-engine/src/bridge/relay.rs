//! The CEF side of the bridge: hidden pages the engine evaluates
//! scripts in (each bridged extension's relay page, and the helper
//! extension's page), and the `Bridge` that owns them.
//!
//! A hidden page is a windowless browser nobody paints. The engine
//! reaches it the way `ExtensionManager` reaches `chrome://extensions`:
//! `Runtime.evaluate` through `execute_dev_tools_method`, the answer in
//! `on_dev_tools_method_result`. No render-process code is involved.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use cef::rc::Rc as _;
use cef::{
    Browser, BrowserSettings, CefString, Client, DevToolsMessageObserver, DialogHandler,
    DisplayHandler, Frame, ImplBrowser, ImplBrowserHost, ImplClient, ImplDevToolsMessageObserver,
    ImplDictionaryValue, ImplDisplayHandler, ImplFrame, ImplLifeSpanHandler, ImplLoadHandler,
    ImplRenderHandler, LifeSpanHandler, LoadHandler, LogSeverity, Rect, Registration,
    RenderHandler, WindowInfo, WrapClient, WrapDevToolsMessageObserver, WrapDisplayHandler,
    WrapLifeSpanHandler, WrapLoadHandler, WrapRenderHandler, wrap_client,
    wrap_dev_tools_message_observer, wrap_display_handler, wrap_life_span_handler,
    wrap_load_handler, wrap_render_handler,
};
use osmena_core::TabId;
use serde_json::json;

use super::evaluation;
use super::install::HELPER_ID;
use super::rewrite::BRIDGE_DIR;
use super::table::{TabTable, parse_targets, query_rows, target_id_of};
use crate::delegate::{DelegateState, ViewState};

/// The key of the helper's page among the hidden pages; a relay's key
/// is its extension's id, which this can never be.
const HELPER_KEY: &str = "helper";

#[cfg(target_os = "macos")]
fn no_window() -> cef::sys::cef_window_handle_t {
    std::ptr::null_mut()
}

#[cfg(windows)]
fn no_window() -> cef::sys::cef_window_handle_t {
    cef::sys::HWND(std::ptr::null_mut())
}

/// A page in a windowless browser that is never shown.
struct HiddenPage {
    browser: Browser,
    /// The DevTools observer stays registered while this lives.
    _devtools: Option<Registration>,
    /// Its main frame finished loading.
    ready: Cell<bool>,
}

impl HiddenPage {
    fn open(bridge: &Rc<Bridge>, key: &str, url: &str) -> Option<HiddenPage> {
        let mut client = PageClient::new(
            PageRender::new(),
            PageLife::new(bridge.delegate.clone()),
            PageLoad::new(bridge.clone(), key.to_string()),
            PageDisplay::new(key.to_string()),
            bridge.delegate.dialog_handler(),
        );
        let window_info = WindowInfo::default().set_as_windowless(no_window());
        let settings = BrowserSettings {
            windowless_frame_rate: 1,
            ..BrowserSettings::default()
        };
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&settings),
            None,
            None,
        )?;
        let mut observer = PageDevTools::new(bridge.clone(), key.to_string());
        let devtools = browser
            .host()
            .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));
        log::debug!("bridge: page {key} opening at {url}");
        Some(HiddenPage {
            browser,
            _devtools: devtools,
            ready: Cell::new(false),
        })
    }

    /// Evaluate `expression` (a promise is awaited, the value returned
    /// by value); the DevTools message id the answer will carry.
    fn evaluate(&self, expression: &str) -> Option<i32> {
        let host = self.browser.host()?;
        let mut params = cef::dictionary_value_create()?;
        params.set_string(
            Some(&"expression".into()),
            Some(&CefString::from(expression)),
        );
        params.set_bool(Some(&"awaitPromise".into()), 1);
        params.set_bool(Some(&"returnByValue".into()), 1);
        // Id 0 asks CEF for the next id of this browser; a caller's
        // own id may be raised, so the returned one is what counts.
        let id =
            host.execute_dev_tools_method(0, Some(&"Runtime.evaluate".into()), Some(&mut params));
        (id != 0).then_some(id)
    }

    fn close(&self) {
        if let Some(host) = self.browser.host() {
            host.close_browser(1);
        }
    }
}

wrap_client! {
    struct PageClient {
        render: RenderHandler,
        life: LifeSpanHandler,
        load: LoadHandler,
        display: DisplayHandler,
        dialog: DialogHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialog.clone())
        }
    }
}

// A windowless browser must have a render handler that names a view
// rectangle; nothing is done with what it paints.
wrap_render_handler! {
    struct PageRender;

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                rect.x = 0;
                rect.y = 0;
                rect.width = 16;
                rect.height = 16;
            }
        }
    }
}

wrap_life_span_handler! {
    struct PageLife {
        delegate: Rc<DelegateState>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            self.delegate.add_live(1);
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.delegate.add_live(-1);
        }
    }
}

wrap_load_handler! {
    struct PageLoad {
        bridge: Rc<Bridge>,
        key: String,
    }

    impl LoadHandler {
        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: ::std::os::raw::c_int,
        ) {
            if frame.is_some_and(|f| f.is_main() != 0) {
                self.bridge.page_ready(&self.key);
            }
        }
    }
}

wrap_display_handler! {
    struct PageDisplay {
        key: String,
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
            log::debug!(
                "bridge: page {} says: {}",
                self.key,
                message.map(CefString::to_string).unwrap_or_default()
            );
            1
        }
    }
}

wrap_dev_tools_message_observer! {
    struct PageDevTools {
        bridge: Rc<Bridge>,
        key: String,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            self.bridge
                .page_result(&self.key, message_id, success != 0, result.unwrap_or_default());
        }
    }
}

/// What a pending evaluation was for.
enum Pending {
    /// The helper's list of targets.
    Targets,
    /// A relay's `next()`: the worker's requests, when it has some.
    Poll,
    /// An answer handed to a relay; only a failure is of interest.
    Reply,
    /// A delivery to an extension's worker.
    Deliver {
        tab: TabId,
        attempt: u8,
        delivery: u64,
    },
}

/// Work that waits for a fresh table.
enum Job {
    /// Answer call `call` of `extension`'s worker with the tabs.
    Query { extension: String, call: u64 },
    /// Deliver a click on `extension`'s action for `tab`.
    Click {
        extension: String,
        tab: TabId,
        attempt: u8,
    },
}

impl Job {
    fn extension(&self) -> &str {
        match self {
            Job::Query { extension, .. } | Job::Click { extension, .. } => extension,
        }
    }
}

#[derive(Default)]
struct Inner {
    table: TabTable,
    /// The DevTools observers that wait for each tab's target id, kept
    /// until the tab closes.
    probes: HashMap<TabId, Option<Registration>>,
    helper: Option<HiddenPage>,
    /// The relay page of each bridged, enabled extension, by its id.
    relays: HashMap<String, HiddenPage>,
    /// Evaluations sent and not yet answered, by page key and the
    /// DevTools message id of that page's browser.
    pending: HashMap<(String, i32), Pending>,
    /// The helper has been asked for the targets and has not answered.
    refreshing: bool,
    jobs: Vec<Job>,
    next_delivery: u64,
}

/// The engine's end of the bridge. One per engine, owned by the
/// extension manager.
pub(crate) struct Bridge {
    delegate: Rc<DelegateState>,
    inner: RefCell<Inner>,
}

impl Bridge {
    pub(crate) fn new(delegate: Rc<DelegateState>) -> Rc<Bridge> {
        Rc::new(Bridge {
            delegate,
            inner: RefCell::new(Inner {
                next_delivery: 1,
                ..Inner::default()
            }),
        })
    }

    /// A tab's browser exists: ask it for its DevTools target id.
    pub(crate) fn track(self: &Rc<Self>, tab: TabId, browser: &Browser, view: Rc<ViewState>) {
        let Some(host) = browser.host() else {
            return;
        };
        let mut observer = TabProbe::new(self.clone(), tab, view);
        let registration = host.add_dev_tools_message_observer(Some(&mut observer));
        if host.execute_dev_tools_method(0, Some(&"Target.getTargetInfo".into()), None) == 0 {
            log::warn!("bridge: cannot ask {tab:?} for its target id");
        }
        self.inner.borrow_mut().probes.insert(tab, registration);
    }

    fn target_known(&self, tab: TabId, success: bool, result: &[u8]) {
        let parsed: serde_json::Value = serde_json::from_slice(result).unwrap_or_default();
        match target_id_of(&parsed).filter(|_| success) {
            Some(target) => {
                log::debug!("bridge: {tab:?} is DevTools target {target}");
                self.inner.borrow_mut().table.set_target(tab, target);
            }
            None => log::warn!("bridge: {tab:?} gave no target id: {parsed}"),
        }
    }

    /// A tab closed.
    pub(crate) fn untrack(&self, tab: TabId) {
        let mut inner = self.inner.borrow_mut();
        inner.probes.remove(&tab);
        inner.table.forget(tab);
    }

    /// Any tab the table could name, for a test click when no tab has
    /// had the focus yet.
    pub(crate) fn any_tab(&self) -> Option<TabId> {
        self.inner.borrow().probes.keys().min().copied()
    }

    /// The extensions were listed: open the helper's page once the
    /// helper is loaded, a relay for each bridged extension that is
    /// on, and close the relays of those that are gone or off.
    pub(crate) fn sync(self: &Rc<Self>, helper_loaded: bool, bridged: &[String]) {
        let (open_helper, to_open, to_close) = {
            let inner = self.inner.borrow();
            let to_open: Vec<String> = bridged
                .iter()
                .filter(|id| !inner.relays.contains_key(*id))
                .cloned()
                .collect();
            let to_close: Vec<String> = inner
                .relays
                .keys()
                .filter(|id| !bridged.contains(id))
                .cloned()
                .collect();
            (helper_loaded && inner.helper.is_none(), to_open, to_close)
        };
        if open_helper {
            let url = format!("chrome-extension://{HELPER_ID}/helper.html");
            let page = HiddenPage::open(self, HELPER_KEY, &url);
            if page.is_none() {
                log::error!("bridge: Chromium could not create the helper's page");
            }
            self.inner.borrow_mut().helper = page;
        }
        for id in to_close {
            self.close_relay(&id);
        }
        for id in to_open {
            self.open_relay(&id);
        }
    }

    fn open_relay(self: &Rc<Self>, extension: &str) {
        let url = format!("chrome-extension://{extension}/{BRIDGE_DIR}/relay.html");
        match HiddenPage::open(self, extension, &url) {
            Some(page) => {
                self.inner
                    .borrow_mut()
                    .relays
                    .insert(extension.to_string(), page);
            }
            None => log::error!("bridge: Chromium could not create the relay of {extension}"),
        }
    }

    fn close_relay(&self, extension: &str) {
        let mut inner = self.inner.borrow_mut();
        if let Some(page) = inner.relays.remove(extension) {
            page.close();
        }
        inner.pending.retain(|(key, _), _| key != extension);
    }

    /// Close every hidden page; the engine pumps until they are gone.
    pub(crate) fn shutdown(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.probes.clear();
        inner.pending.clear();
        inner.jobs.clear();
        if let Some(page) = inner.helper.take() {
            page.close();
        }
        for (_, page) in inner.relays.drain() {
            page.close();
        }
    }

    fn evaluate(&self, key: &str, expression: &str, pending: Pending) -> bool {
        let mut inner = self.inner.borrow_mut();
        let page = if key == HELPER_KEY {
            inner.helper.as_ref()
        } else {
            inner.relays.get(key)
        };
        let Some(id) = page.and_then(|page| page.evaluate(expression)) else {
            return false;
        };
        inner.pending.insert((key.to_string(), id), pending);
        true
    }

    fn page_is_ready(&self, key: &str) -> Option<bool> {
        let inner = self.inner.borrow();
        let page = if key == HELPER_KEY {
            inner.helper.as_ref()
        } else {
            inner.relays.get(key)
        };
        page.map(|page| page.ready.get())
    }

    /// A hidden page's main frame loaded.
    fn page_ready(self: &Rc<Self>, key: &str) {
        {
            let inner = self.inner.borrow();
            let page = if key == HELPER_KEY {
                inner.helper.as_ref()
            } else {
                inner.relays.get(key)
            };
            let Some(page) = page else {
                return;
            };
            page.ready.set(true);
        }
        log::info!("bridge: page {key} ready");
        if key != HELPER_KEY {
            self.poll(key);
        }
        if !self.inner.borrow().jobs.is_empty() {
            self.refresh();
        }
    }

    /// Wait for the next requests of `extension`'s worker.
    fn poll(&self, extension: &str) {
        if !self.evaluate(extension, "nautilusRelay.next()", Pending::Poll) {
            log::warn!("bridge: cannot listen to the relay of {extension}");
        }
    }

    /// Ask the helper for the targets; the waiting jobs run when it
    /// answers. With the helper not ready yet they wait for it.
    fn refresh(&self) {
        if self.inner.borrow().refreshing {
            return;
        }
        if self.page_is_ready(HELPER_KEY) != Some(true) {
            log::debug!(
                "bridge: the helper is not ready; {} job(s) wait",
                self.inner.borrow().jobs.len()
            );
            return;
        }
        if self.evaluate(HELPER_KEY, "nautilusHelper.targets()", Pending::Targets) {
            self.inner.borrow_mut().refreshing = true;
        } else {
            log::warn!("bridge: cannot ask the helper for the targets");
        }
    }

    /// `extension`'s action was clicked for `tab`. False when the
    /// extension has no relay (it is not bridged, or is off).
    pub(crate) fn click(&self, extension: &str, tab: TabId) -> bool {
        {
            let mut inner = self.inner.borrow_mut();
            if !inner.relays.contains_key(extension) {
                return false;
            }
            inner.jobs.push(Job::Click {
                extension: extension.to_string(),
                tab,
                attempt: 0,
            });
        }
        self.refresh();
        true
    }

    /// One request of `extension`'s worker, as the relay passed it on.
    fn worker_call(&self, extension: &str, item: &serde_json::Value) {
        let Some(call) = item.get("call") else {
            return;
        };
        let Some(id) = call.get("id").and_then(|v| v.as_u64()) else {
            return;
        };
        let method = call
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        log::debug!("bridge: {extension} calls {method} ({id})");
        if method == "tabs.query" {
            self.inner.borrow_mut().jobs.push(Job::Query {
                extension: extension.to_string(),
                call: id,
            });
            self.refresh();
        } else {
            // Fail closed: a call the engine does not know is refused,
            // never passed to Chromium.
            let answer = json!({"error": format!("{method} is not supported by Nautilus.")});
            self.reply(extension, id, &answer);
        }
    }

    fn reply(&self, extension: &str, call: u64, answer: &serde_json::Value) {
        let expression = format!("nautilusRelay.reply({call}, {answer})");
        if !self.evaluate(extension, &expression, Pending::Reply) {
            log::warn!("bridge: cannot answer call {call} of {extension}");
        }
    }

    /// Run the jobs whose relay is ready, with the table as it now is;
    /// a job whose relay is still loading waits, one whose relay is
    /// gone is dropped.
    fn run_jobs(self: &Rc<Self>) {
        let jobs = std::mem::take(&mut self.inner.borrow_mut().jobs);
        for job in jobs {
            match self.page_is_ready(job.extension()) {
                None => log::warn!("bridge: {} has no relay any more", job.extension()),
                Some(false) => self.inner.borrow_mut().jobs.push(job),
                Some(true) => self.run(job),
            }
        }
    }

    fn run(&self, job: Job) {
        match job {
            Job::Query { extension, call } => {
                let rows = {
                    let inner = self.inner.borrow();
                    query_rows(inner.table.rows(), self.delegate.last_focused())
                };
                self.reply(&extension, call, &json!({"result": rows}));
            }
            Job::Click {
                extension,
                tab,
                attempt,
            } => {
                let (chromium, delivery) = {
                    let mut inner = self.inner.borrow_mut();
                    let delivery = inner.next_delivery;
                    inner.next_delivery += 1;
                    (inner.table.chromium(tab), delivery)
                };
                let Some(chromium) = chromium else {
                    log::warn!(
                        "bridge: no Chromium tab id for {tab:?}; the click on {extension} is dropped"
                    );
                    return;
                };
                let message =
                    json!({"id": delivery, "event": "action.onClicked", "tabId": chromium});
                let expression = format!("nautilusRelay.deliver({message})");
                let pending = Pending::Deliver {
                    tab,
                    attempt,
                    delivery,
                };
                if self.evaluate(&extension, &expression, pending) {
                    log::info!(
                        "bridge: {extension}: delivery {delivery} (action.onClicked, tab {chromium}) sent"
                    );
                } else {
                    log::warn!("bridge: cannot reach the relay of {extension}");
                }
            }
        }
    }

    /// An evaluation in a hidden page answered.
    fn page_result(self: &Rc<Self>, key: &str, id: i32, success: bool, result: &[u8]) {
        let Some(pending) = self
            .inner
            .borrow_mut()
            .pending
            .remove(&(key.to_string(), id))
        else {
            return;
        };
        let value = evaluation(success, result);
        match pending {
            Pending::Targets => {
                {
                    let mut inner = self.inner.borrow_mut();
                    inner.refreshing = false;
                    match &value {
                        Ok(value) => {
                            let targets = parse_targets(value);
                            inner.table.join(&targets);
                            log::info!(
                                "bridge: table has {} tab(s) of {} target(s)",
                                inner.table.rows().len(),
                                targets.len()
                            );
                        }
                        Err(e) => log::warn!("bridge: the helper could not list the targets: {e}"),
                    }
                }
                self.run_jobs();
            }
            Pending::Poll => match value {
                Ok(items) => {
                    for item in items.as_array().into_iter().flatten() {
                        self.worker_call(key, item);
                    }
                    self.poll(key);
                }
                // The page went (the extension was reloaded or removed);
                // listening starts again when a relay is next ready.
                Err(e) => log::warn!("bridge: the relay of {key} stopped listening: {e}"),
            },
            Pending::Reply => {
                if let Err(e) = value {
                    log::warn!("bridge: an answer to {key} was lost: {e}");
                }
            }
            Pending::Deliver {
                tab,
                attempt,
                delivery,
            } => {
                let outcome = value.unwrap_or_else(|e| json!({"ok": false, "error": e}));
                if outcome.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    log::info!(
                        "bridge: {key}: delivery {delivery} taken by the worker after {} attempt(s)",
                        outcome
                            .get("attempts")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0)
                    );
                } else if attempt == 0
                    && outcome.get("received").and_then(|v| v.as_bool()) != Some(true)
                {
                    // The relay or the worker is gone: a fresh relay,
                    // one more try. A delivery the worker acknowledged
                    // is never sent again.
                    log::warn!(
                        "bridge: {key}: delivery {delivery} failed ({outcome}); trying once more"
                    );
                    self.close_relay(key);
                    self.open_relay(key);
                    self.inner.borrow_mut().jobs.push(Job::Click {
                        extension: key.to_string(),
                        tab,
                        attempt: 1,
                    });
                } else {
                    log::warn!("bridge: {key}: delivery {delivery} dropped: {outcome}");
                }
            }
        }
    }

    /// The table, for a scripted run's log.
    pub(crate) fn log_table(&self) {
        let inner = self.inner.borrow();
        for row in inner.table.rows() {
            log::info!(
                "bridge: table: {:?} is Chromium tab {} at {}",
                row.tab,
                row.chromium,
                row.url
            );
        }
        log::info!("bridge: table: {} row(s)", inner.table.rows().len());
    }
}

wrap_dev_tools_message_observer! {
    struct TabProbe {
        bridge: Rc<Bridge>,
        tab: TabId,
        view: Rc<ViewState>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            _message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            self.bridge
                .target_known(self.tab, success != 0, result.unwrap_or_default());
        }

        /// A debugger is attached to the tab (the extension's
        /// `chrome.debugger`, or DevTools). Its renderer must keep running
        /// while it is: a hidden one answers no input and no screenshot,
        /// and the debugger waits for the answer (about 5 s each in the
        /// probe, Task 11's H5). The tab is painted again at once.
        fn on_dev_tools_agent_attached(&self, browser: Option<&mut Browser>) {
            self.view.dev_tools_attached.set(true);
            if let Some(host) = browser.and_then(|b| b.host()) {
                host.was_hidden(0);
            }
        }

        /// The debugger went: a tab that is behind another is hidden again.
        fn on_dev_tools_agent_detached(&self, browser: Option<&mut Browser>) {
            self.view.dev_tools_attached.set(false);
            if self.view.hidden.get()
                && let Some(host) = browser.and_then(|b| b.host())
            {
                host.was_hidden(1);
            }
        }
    }
}
