//! CEF initialisation, the subprocess entry point, the message-pump bridge
//! to the application's event loop, and the event queue.

use std::cell::Cell;
use std::fmt;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, BrowserProcessHandler, CefString, CommandLine, ImplApp, ImplBrowserProcessHandler,
    ImplCommandLine, LogSeverity, Settings, WrapApp, WrapBrowserProcessHandler, api_hash, wrap_app,
    wrap_browser_process_handler,
};
use nautilus_core::{EngineEvent, PendingTab};

use crate::delegate::DelegateState;

/// Wakes the application's event loop from any thread. CEF calls this
/// when it has work for the main thread; the app then calls
/// [`Engine::spin`].
pub trait Waker: Send + Sync + 'static {
    fn wake(&self);
}

#[derive(Debug)]
pub struct EngineError(pub String);

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

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
    let mut app = NautilusApp::new(None);
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
    let mut app = NautilusApp::new(None);
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
        // verbatim `\?\` path, and Chromium's LevelDB (IndexedDB) then
        // fails to take its lock file in it.
        let engine_dir = std::path::absolute(engine_dir).map_err(|e| {
            EngineError(format!(
                "resolving engine directory {}: {e}",
                engine_dir.display()
            ))
        })?;
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
        let pump = Arc::new(PumpScheduler::new(waker));
        let mut app = NautilusApp::new(Some(pump));
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
        Ok(Engine {
            delegate: Rc::new(DelegateState::default()),
            spinning: Cell::new(false),
        })
    }

    /// Run the work CEF has queued for the main thread: it delivers the
    /// handler callbacks (which become [`EngineEvent`]s) and paints. Call
    /// after every window event and every [`Waker::wake`].
    pub fn spin(&self) {
        if self.spinning.replace(true) {
            return;
        }
        cef::do_message_loop_work();
        self.spinning.set(false);
    }

    /// Take every event produced since the last call.
    pub fn drain_events(&self) -> Vec<EngineEvent> {
        self.delegate.drain()
    }

    /// Forget a tab that page content asked for but that no window adopted.
    pub fn discard_pending(&self, pending: PendingTab) {
        if self.delegate.take_pending(pending).is_none() {
            log::debug!("no pending tab {pending:?} to discard");
        }
    }

    pub(crate) fn delegate(&self) -> &Rc<DelegateState> {
        &self.delegate
    }
}

impl Drop for Engine {
    /// Every browser must be closed before CEF shuts down. The tab hosts
    /// drop before the engine and ask their browsers to close; closing
    /// completes on the message loop, so pump it until the last one is
    /// gone (or give up after a moment).
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.delegate.live_browsers() > 0 && Instant::now() < deadline {
            cef::do_message_loop_work();
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.delegate.live_browsers() > 0 {
            log::warn!(
                "{} browser(s) still open at shutdown",
                self.delegate.live_browsers()
            );
        }
        cef::shutdown();
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
            #[cfg(target_os = "macos")]
            command_line.append_switch(Some(&"use-mock-keychain".into()));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            let pump = self.pump.clone()?;
            if !pump.started.swap(true, Ordering::AcqRel) {
                pump.start_timer_thread();
            }
            Some(NautilusBrowserProcessHandler::new(pump))
        }
    }
}

wrap_browser_process_handler! {
    struct NautilusBrowserProcessHandler {
        pump: Arc<PumpScheduler>,
    }

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.pump.schedule(delay_ms);
        }
    }
}
