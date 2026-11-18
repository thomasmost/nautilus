//! The bridge's CEF-facing half: it carries out what `BridgeState`
//! decides. Calls arrive from two lanes (the relay, for service workers;
//! the page lane, for extension pages), answers go back down the lane a
//! call came on, events go down both, requests become `EngineEvent`s,
//! and a screenshot is taken through the tab's own DevTools agent.
//!
//! Nothing here holds a `RefCell` borrow while it performs an effect: every
//! effect runs after the state that decided it is released, because an
//! effect can reach CEF, and CEF can call back into the hub (an evaluation
//! answered inside its call reaches `page_result`, which calls `call`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Instant;

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, DevToolsMessageObserver, ImplBrowser, ImplBrowserHost,
    ImplDevToolsMessageObserver, ImplDictionaryValue, Registration, WrapDevToolsMessageObserver,
    wrap_dev_tools_message_observer,
};
use osmena_core::{EngineEvent, ExtensionReply, TabId};
use osmena_engine::WindowStrip;
use serde_json::Value;

use super::pattern::Grants;
use super::snapshot::Snapshot;
use super::state::{BridgeState, Effect};
use super::transport::{BridgeCall, BridgeTransport, CallId};
use crate::delegate::DelegateState;

/// Which way a call came, and so which way its answer goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    /// The relay: a service worker.
    Worker,
    /// A process message from an extension page (Task 12's page lane).
    // Constructed only by the page lane, which Task 12 adds.
    #[allow(dead_code)]
    Page,
}

/// The first DevTools message id the hub uses on a tab's browser; other
/// users of that channel stay far below it.
const CAPTURE_IDS: i32 = 1_000_000;
const CAPTURE_FAILED: &str = "Failed to capture tab: the browser returned no image.";

/// A screenshot under way.
struct Capture {
    call: CallId,
    format: String,
    /// Keeps the observer registered until the result is in.
    observer: Registration,
}

pub(crate) struct Hub {
    state: RefCell<BridgeState>,
    worker: Rc<dyn BridgeTransport>,
    pages: Option<Rc<dyn BridgeTransport>>,
    /// Calls not yet answered: the hub's id to the lane and the lane's id.
    routes: RefCell<HashMap<CallId, (Lane, CallId)>>,
    last_call: Cell<CallId>,
    delegate: Weak<DelegateState>,
    /// The tab-id table: Chromium's id for a tab, when it is known.
    table: Box<dyn Fn(TabId) -> Option<i64>>,
    /// Asks for another `tick` in a moment.
    nudge: Box<dyn Fn()>,
    captures: RefCell<HashMap<i32, Capture>>,
    last_capture: Cell<i32>,
    /// Observers whose result has arrived, let go on the next tick rather
    /// than inside their own callback.
    spent: RefCell<Vec<Registration>>,
}

impl Hub {
    pub(crate) fn new(
        delegate: &Rc<DelegateState>,
        worker: Rc<dyn BridgeTransport>,
        pages: Option<Rc<dyn BridgeTransport>>,
        table: Box<dyn Fn(TabId) -> Option<i64>>,
        nudge: Box<dyn Fn()>,
    ) -> Rc<Hub> {
        Rc::new(Hub {
            state: RefCell::new(BridgeState::default()),
            worker,
            pages,
            routes: RefCell::new(HashMap::new()),
            last_call: Cell::new(0),
            delegate: Rc::downgrade(delegate),
            table,
            nudge,
            captures: RefCell::new(HashMap::new()),
            last_capture: Cell::new(CAPTURE_IDS),
            spent: RefCell::new(Vec::new()),
        })
    }

    /// A bridged function was called. `call.id` is the lane's own id for
    /// it.
    pub(crate) fn call(self: &Rc<Self>, lane: Lane, mut call: BridgeCall) {
        let id = self.last_call.get() + 1;
        self.last_call.set(id);
        self.routes.borrow_mut().insert(id, (lane, call.id));
        call.id = id;
        log::debug!(
            "bridge: {} calls {}.{} ({lane:?}, {:?})",
            call.extension,
            call.namespace,
            call.method,
            call.asker
        );
        let effects = self.state.borrow_mut().call(call);
        self.run(effects);
    }

    /// `Engine::strip_changed`.
    pub(crate) fn strip_changed(self: &Rc<Self>, windows: &[WindowStrip<'_>]) {
        let snapshot = Snapshot::copy_of(windows);
        let effects = self
            .state
            .borrow_mut()
            .strip_changed(snapshot, &*self.table, Instant::now());
        self.run(effects);
    }

    /// `Engine::extension_reply`.
    pub(crate) fn extension_reply(self: &Rc<Self>, token: u64, reply: ExtensionReply) {
        let effects = self
            .state
            .borrow_mut()
            .extension_reply(token, reply, Instant::now());
        self.run(effects);
    }

    /// The table gained or lost a tab's Chromium id.
    pub(crate) fn table_changed(self: &Rc<Self>) {
        let effects = self
            .state
            .borrow_mut()
            .table_changed(&*self.table, Instant::now());
        self.run(effects);
    }

    /// The enabled extensions that carry the bridge, with their grants.
    pub(crate) fn set_extensions(self: &Rc<Self>, extensions: Vec<(String, Grants)>) {
        let effects = self.state.borrow_mut().set_extensions(extensions);
        self.run(effects);
    }

    /// Called on every spin of the engine.
    pub(crate) fn tick(self: &Rc<Self>) {
        self.spent.borrow_mut().clear();
        let effects = {
            let mut state = self.state.borrow_mut();
            if !state.needs_tick() {
                return;
            }
            state.tick(&*self.table, Instant::now())
        };
        self.run(effects);
    }

    /// Let go of everything that holds a CEF object, before CEF shuts
    /// down.
    pub(crate) fn shutdown(&self) {
        self.captures.borrow_mut().clear();
        self.spent.borrow_mut().clear();
        self.routes.borrow_mut().clear();
    }

    /// Carry out `effects`. No borrow of the hub's state is held here.
    fn run(self: &Rc<Self>, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Reply(call, result) => self.reply(call, result),
                Effect::Emit {
                    extension,
                    event,
                    args,
                } => {
                    log::debug!("bridge: {event} to {extension}");
                    self.worker.emit(&extension, event, &args);
                    if let Some(pages) = &self.pages {
                        pages.emit(&extension, event, &args);
                    }
                }
                Effect::Wake(extension) => self.worker.wake(&extension),
                Effect::Request {
                    token,
                    extension,
                    request,
                } => {
                    if let Some(delegate) = self.delegate.upgrade() {
                        log::debug!(
                            "bridge: {extension} asks the browser for {request:?} (token {token})"
                        );
                        delegate.push(EngineEvent::ExtensionRequest {
                            token,
                            extension,
                            request,
                        });
                    }
                }
                Effect::Capture {
                    call,
                    tab,
                    format,
                    quality,
                } => {
                    self.capture(call, tab, format, quality);
                }
            }
        }
        let wants_tick = self.state.borrow().needs_tick();
        if wants_tick {
            (self.nudge)();
        }
    }

    fn reply(&self, call: CallId, result: Result<Value, String>) {
        let removed = self.routes.borrow_mut().remove(&call);
        let Some((lane, local)) = removed else {
            return;
        };
        if let Err(message) = &result {
            log::debug!("bridge: call {call} rejects: {message}");
        }
        match lane {
            Lane::Worker => self.worker.reply(local, result),
            Lane::Page => {
                if let Some(pages) = &self.pages {
                    pages.reply(local, result);
                }
            }
        }
    }

    /// `tabs.captureVisibleTab`: `Page.captureScreenshot` on the tab's own
    /// browser, whose result comes back to `captured`.
    fn capture(self: &Rc<Self>, call: CallId, tab: TabId, format: String, quality: i64) {
        let host = self
            .delegate
            .upgrade()
            .and_then(|delegate| delegate.browser_of(tab))
            .and_then(|browser| browser.host());
        let (Some(host), Some(mut params)) = (host, cef::dictionary_value_create()) else {
            self.reply(call, Err(CAPTURE_FAILED.to_string()));
            return;
        };
        params.set_string(
            Some(&"format".into()),
            Some(&CefString::from(format.as_str())),
        );
        if format == "jpeg" {
            params.set_int(Some(&"quality".into()), quality as i32);
        }
        let mut observer = CaptureObserver::new(Rc::downgrade(self));
        let Some(observer) = host.add_dev_tools_message_observer(Some(&mut observer)) else {
            self.reply(call, Err(CAPTURE_FAILED.to_string()));
            return;
        };
        let id = self.last_capture.get() + 1;
        self.last_capture.set(id);
        self.captures.borrow_mut().insert(
            id,
            Capture {
                call,
                format,
                observer,
            },
        );
        if host.execute_dev_tools_method(
            id,
            Some(&"Page.captureScreenshot".into()),
            Some(&mut params),
        ) == 0
        {
            self.captured(id, false, &[]);
        }
    }

    fn captured(&self, id: i32, success: bool, result: &[u8]) {
        let removed = self.captures.borrow_mut().remove(&id);
        let Some(capture) = removed else {
            return;
        };
        let data = success
            .then(|| serde_json::from_slice::<Value>(result).ok())
            .flatten()
            .and_then(|value| {
                value
                    .get("data")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let result = match data {
            Some(data) => Ok(Value::String(format!(
                "data:image/{};base64,{data}",
                capture.format
            ))),
            None => Err(CAPTURE_FAILED.to_string()),
        };
        self.spent.borrow_mut().push(capture.observer);
        self.reply(capture.call, result);
    }
}

wrap_dev_tools_message_observer! {
    struct CaptureObserver {
        hub: Weak<Hub>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            if let Some(hub) = self.hub.upgrade() {
                hub.captured(message_id, success != 0, result.unwrap_or_default());
            }
        }
    }
}
