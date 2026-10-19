//! CEF initialisation, the subprocess entry point, the message-pump bridge
//! to the application's event loop, and the event queue.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, Browser, BrowserProcessHandler, CefString, Client, CommandLine, DialogHandler,
    DisplayHandler, Frame as CefFrame, ImplApp, ImplBrowser, ImplBrowserHost,
    ImplBrowserProcessHandler, ImplClient, ImplCommandLine, ImplDisplayHandler, ImplFrame,
    ImplLifeSpanHandler, LifeSpanHandler, LogSeverity, Settings, WrapApp,
    WrapBrowserProcessHandler, WrapClient, WrapDisplayHandler, WrapLifeSpanHandler, api_hash,
    wrap_app, wrap_browser_process_handler, wrap_client, wrap_display_handler,
    wrap_life_span_handler,
};
use osmena_core::{EngineEvent, EngineInfo, ExtensionSource, ExtensionSupport, PendingTab};
use osmena_engine::raw_window_handle::{DisplayHandle, WindowHandle};
use osmena_engine::{EngineError, GlutinSurface, PhysicalSize, Waker};
use url::Url;

use crate::delegate::DelegateState;
use crate::extensions::{ExtensionManager, UnpackQueue};
use crate::host::TabHost;

/// What the shell needs to know about this engine. The version is the
/// Chromium major of the `cef` crate pinned in the workspace `Cargo.toml`.
pub static INFO: EngineInfo = EngineInfo {
    name: "Chromium",
    version: "154",
    data_dir: "chromium",
    supports_stop: true,
    // winit reports one wheel notch as one line on Windows; Chromium
    // scrolls about 100 px per notch.
    wheel_line_px: 100.0,
    // Chromium's extension system, driven through the hidden management
    // page (see `extensions.rs`); not wired up on macOS yet.
    extensions: if cfg!(windows) {
        Some(ExtensionSupport {
            store_name: "Chrome Web Store",
            store_url: "https://chromewebstore.google.com/",
            store_item_id: crate::extensions::web_store_item_id,
        })
    } else {
        None
    },
};

/// Run this process as one of Chromium's helper processes if it is one.
///
/// Chromium starts its renderer, GPU, network, and utility processes by
/// running the browser's executable again with a `--type=<kind>` argument
/// (and many more). This must be the first thing `main` does: before
/// parsing the command line, logging, or opening windows. It returns
/// `Some(exit code)` when this process was a helper and has finished its
/// work, in which case `main` must exit with that code, and `None` in the
/// browser process, which then goes on to build an [`Engine`].
pub fn run_subprocess() -> Option<i32> {
    load_framework(false);
    let _ = api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let mut app = NautilusApp::new(None, Rc::new(RefCell::new(None)));
    let code = cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    (code >= 0).then_some(code)
}

/// Run this process as a Chromium helper process and return its exit
/// code. For the dedicated helper executable (`nautilus-helper`), which on
/// macOS lives in a helper app bundle inside the browser's bundle.
pub fn run_helper() -> i32 {
    load_framework(true);
    let _ = api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = Args::new();
    let mut app = NautilusApp::new(None, Rc::new(RefCell::new(None)));
    cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    )
}

/// macOS: CEF is a framework loaded at run time from the app bundle
/// (`../Frameworks` from the browser executable, `../../..` from a helper).
/// The loader stays alive for the life of the process. Elsewhere libcef is
/// a linked DLL and there is nothing to do.
#[cfg(target_os = "macos")]
fn load_framework(helper: bool) {
    use std::sync::OnceLock;
    static LOADER: OnceLock<cef::library_loader::LibraryLoader> = OnceLock::new();
    LOADER.get_or_init(|| {
        let exe = std::env::current_exe().expect("the executable path is known");
        let loader = cef::library_loader::LibraryLoader::new(&exe, helper);
        assert!(
            loader.load(),
            "cannot load the Chromium Embedded Framework from the app bundle"
        );
        loader
    });
}

#[cfg(not(target_os = "macos"))]
fn load_framework(_helper: bool) {}

/// One CEF instance. CEF is process-global, so build exactly one `Engine`
/// per process, after [`run_subprocess`] returned `None`.
pub struct Engine {
    delegate: Rc<DelegateState>,
    extensions: Rc<ExtensionManager>,
    /// True while inside `do_message_loop_work`, whose callbacks may ask
    /// for another spin; that request is served on the next call.
    spinning: Cell<bool>,
}

impl Engine {
    /// Start Chromium with its cookies, local storage, and HTTP cache under
    /// `engine_dir` (the profile's `chromium/` directory).
    ///
    /// Each tab's page runs in one of Chromium's renderer processes, so a
    /// renderer that crashes costs that tab its page (reported as an
    /// `EngineEvent::TabCrashed`) and nothing else. The renderer processes
    /// run without Chromium's sandbox for now (see README.md, Known
    /// limitations).
    pub fn new(engine_dir: &Path, waker: Arc<dyn Waker>) -> Result<Engine, EngineError> {
        std::fs::create_dir_all(engine_dir).map_err(|e| {
            EngineError(format!(
                "creating engine directory {}: {e}",
                engine_dir.display()
            ))
        })?;
        // Absolute, but not canonical: on Windows `canonicalize` yields a
        // verbatim `\\?\` path, and Chromium's LevelDB (IndexedDB) then
        // fails to take its lock file in it.
        let engine_dir = std::path::absolute(engine_dir).map_err(|e| {
            EngineError(format!(
                "resolving engine directory {}: {e}",
                engine_dir.display()
            ))
        })?;
        forget_chromium_session(&engine_dir);
        let cache_path = CefString::from(engine_dir.to_string_lossy().as_ref());
        let settings = Settings {
            no_sandbox: 1,
            // Tabs paint off-screen into frames the window uploads.
            windowless_rendering_enabled: 1,
            // CEF's work runs from `Engine::spin` on the app's event loop
            // instead of a loop of its own; CEF asks for a spin through
            // `on_schedule_message_pump_work`.
            external_message_pump: 1,
            root_cache_path: cache_path.clone(),
            cache_path,
            persist_session_cookies: 1,
            log_severity: LogSeverity::WARNING,
            // Opaque white behind pages that paint nothing.
            background_color: 0xFFFF_FFFF,
            ..Settings::default()
        };
        let pump = Arc::new(PumpScheduler::new(waker.clone()));
        // Filled in once the engine exists, for the browsers Chromium
        // creates on its own.
        let shared_delegate = Rc::new(RefCell::new(None));
        let mut app = NautilusApp::new(Some(pump), shared_delegate.clone());
        let args = Args::new();
        if cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ) != 1
        {
            return Err(EngineError(
                "Chromium failed to initialise (the CEF log on stderr has the reason)".to_string(),
            ));
        }
        let delegate = Rc::new(DelegateState::default());
        *shared_delegate.borrow_mut() = Some(delegate.clone());
        let extensions =
            ExtensionManager::start(delegate.clone(), &engine_dir, UnpackQueue::new(waker));
        Ok(Engine {
            delegate,
            extensions,
            spinning: Cell::new(false),
        })
    }

    pub(crate) fn delegate(&self) -> &Rc<DelegateState> {
        &self.delegate
    }

    pub(crate) fn extensions(&self) -> &Rc<ExtensionManager> {
        &self.extensions
    }
}

impl osmena_engine::Engine for Engine {
    /// Chromium paints into memory, so the chrome's GL context is the
    /// shell's own.
    type Surface = GlutinSurface;
    type Host = TabHost;

    /// Run the work CEF has queued for the main thread: it delivers the
    /// handler callbacks (which become [`EngineEvent`]s) and paints.
    fn spin(&self) {
        if self.spinning.replace(true) {
            return;
        }
        // Store downloads unpacked on worker threads are loaded here, on
        // the UI thread.
        self.extensions.poll_unpacked();
        cef::do_message_loop_work();
        self.spinning.set(false);
    }

    fn drain_events(&self) -> Vec<EngineEvent> {
        self.delegate.drain()
    }

    fn discard_pending(&self, pending: PendingTab) {
        if self.delegate.take_pending(pending).is_none() {
            log::debug!("no pending tab {pending:?} to discard");
        }
    }

    fn open_surface(
        &self,
        display: DisplayHandle<'_>,
        window: WindowHandle<'_>,
        size: PhysicalSize<u32>,
    ) -> Result<GlutinSurface, EngineError> {
        GlutinSurface::new(display, window, size)
    }

    fn open_host(
        &self,
        _surface: &GlutinSurface,
        window: WindowHandle<'_>,
        scale: f32,
    ) -> Result<TabHost, EngineError> {
        TabHost::new(self, window.as_raw(), scale)
    }

    fn install_extension(&self, source: ExtensionSource) {
        self.extensions.install(source);
    }

    fn set_extension_enabled(&self, extension: &str, enabled: bool) {
        self.extensions.set_enabled(extension, enabled);
    }

    fn uninstall_extension(&self, extension: &str) {
        self.extensions.uninstall(extension);
    }
}

impl Drop for Engine {
    /// Every browser must be closed before CEF shuts down. The tab hosts
    /// drop before the engine and ask their browsers (and popups) to
    /// close; the extension manager's goes here; closing completes on the
    /// message loop, so pump it until the last one is gone (or give up
    /// after a moment).
    fn drop(&mut self) {
        // A recent extension change is still only in memory; see
        // `ExtensionManager::settle_deadline`.
        if let Some(settle) = self.extensions.settle_deadline() {
            log::info!("waiting for Chromium to record the extension change");
            while Instant::now() < settle {
                cef::do_message_loop_work();
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.extensions.shutdown();
        let deadline = Instant::now() + Duration::from_secs(3);
        while (self.delegate.live_browsers() > 0 || !self.extensions.is_closed())
            && Instant::now() < deadline
        {
            cef::do_message_loop_work();
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.delegate.live_browsers() > 0 || !self.extensions.is_closed() {
            log::warn!(
                "{} browser(s) still open at shutdown (extension manager closed: {})",
                self.delegate.live_browsers(),
                self.extensions.is_closed()
            );
        }
        cef::shutdown();
    }
}

/// Chromium saves the extension manager's window (a Chrome-style browser
/// is a Chrome window to its session service) and, if the browser did not
/// exit cleanly, restores it as a visible Chromium window at the next
/// launch. Nautilus never wants Chromium's session restore (the shell has
/// its own), so the saved session goes before Chromium starts.
fn forget_chromium_session(engine_dir: &Path) {
    let sessions = engine_dir.join("Default").join("Sessions");
    if !sessions.is_dir() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(&sessions) {
        log::warn!("cannot remove {}: {e}", sessions.display());
    }
}

/// Turns CEF's "do work in `delay` ms" requests, which arrive on any
/// thread, into wakes of the application's event loop: immediately for a
/// zero delay, otherwise from a timer thread at the earliest requested
/// time.
struct PumpScheduler {
    waker: Arc<dyn Waker>,
    deadline: Mutex<Option<Instant>>,
    changed: Condvar,
    started: AtomicBool,
}

impl PumpScheduler {
    fn new(waker: Arc<dyn Waker>) -> PumpScheduler {
        PumpScheduler {
            waker,
            deadline: Mutex::new(None),
            changed: Condvar::new(),
            started: AtomicBool::new(false),
        }
    }

    fn start_timer_thread(self: &Arc<Self>) {
        let scheduler = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("CefPumpTimer".to_string())
            .spawn(move || scheduler.run());
    }

    fn run(&self) {
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match *deadline {
                None => {
                    deadline = self
                        .changed
                        .wait(deadline)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                Some(at) => {
                    let now = Instant::now();
                    if at <= now {
                        *deadline = None;
                        self.waker.wake();
                    } else {
                        deadline = self
                            .changed
                            .wait_timeout(deadline, at - now)
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .0;
                    }
                }
            }
        }
    }

    fn schedule(&self, delay_ms: i64) {
        if delay_ms <= 0 {
            self.waker.wake();
            return;
        }
        let at = Instant::now() + Duration::from_millis(delay_ms as u64);
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if deadline.is_none_or(|current| at < current) {
            *deadline = Some(at);
            self.changed.notify_one();
        }
    }
}

// The application handler: the same type serves `execute_process` (every
// process) and `initialize` (the browser process), where it carries the
// pump scheduler.
wrap_app! {
    struct NautilusApp {
        pump: Option<Arc<PumpScheduler>>,
        // The event queue, once the engine exists, for the browsers
        // Chromium creates on its own.
        delegate: Rc<RefCell<Option<Rc<DelegateState>>>>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(command_line) = command_line else {
                return;
            };
            // No Chrome-style startup chrome: Nautilus draws its own.
            command_line.append_switch(Some(&"no-startup-window".into()));
            command_line.append_switch(Some(&"noerrdialogs".into()));
            command_line.append_switch(Some(&"hide-crash-restore-bubble".into()));
            command_line.append_switch(Some(&"disable-session-crashed-bubble".into()));
            // The GPU process crashes repeatedly (and Chromium then falls
            // back to software rendering for the session) when a windowed
            // Chrome-style browser (the extension manager's) exists next
            // to the windowless tabs with DirectComposition on; the tabs
            // paint into memory and never use it.
            command_line.append_switch(Some(&"disable-direct-composition".into()));
            #[cfg(target_os = "macos")]
            command_line.append_switch(Some(&"use-mock-keychain".into()));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            let pump = self.pump.clone()?;
            if !pump.started.swap(true, Ordering::AcqRel) {
                pump.start_timer_thread();
            }
            Some(NautilusBrowserProcessHandler::new(pump, self.delegate.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct NautilusBrowserProcessHandler {
        pump: Arc<PumpScheduler>,
        delegate: Rc<RefCell<Option<Rc<DelegateState>>>>,
    }

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.pump.schedule(delay_ms);
        }

        /// The client of a browser Chromium creates on its own (an
        /// extension's `chrome.tabs.create`, a link the extension
        /// manager's page opens): a browser Nautilus cannot show, so its
        /// URL becomes a tab of the last focused window and it closes.
        fn default_client(&self) -> Option<Client> {
            let delegate = self.delegate.borrow().clone()?;
            log::debug!("Chromium asked for a client for a browser of its own");
            Some(StrayClient::new(
                StrayLife::new(delegate.clone()),
                StrayDisplay::new(delegate.clone(), Cell::new(false)),
                delegate.dialog_handler(),
            ))
        }
    }
}

wrap_client! {
    struct StrayClient {
        life: LifeSpanHandler,
        display: DisplayHandler,
        dialog: DialogHandler,
    }

    impl Client {
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialog.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }
    }
}

wrap_life_span_handler! {
    struct StrayLife {
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

wrap_display_handler! {
    struct StrayDisplay {
        delegate: Rc<DelegateState>,
        handled: Cell<bool>,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            url: Option<&CefString>,
        ) {
            if self.handled.get() || frame.is_some_and(|f| f.is_main() == 0) {
                return;
            }
            let Some(url) = url.and_then(|u| Url::parse(&u.to_string()).ok()) else {
                return;
            };
            if url.as_str() == "about:blank" {
                return;
            }
            self.handled.set(true);
            log::info!("Chromium opened {url} on its own; moving it to a tab");
            if !self.delegate.request_tab_without_opener(url) {
                log::warn!("no window to show it in");
            }
            if let Some(host) = browser.and_then(|b| b.host()) {
                host.close_browser(1);
            }
        }
    }
}
