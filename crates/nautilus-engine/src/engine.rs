//! CEF initialisation, the subprocess entry point, the message-pump bridge
//! to the application's event loop, and the event queue.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
#[cfg(feature = "test-hooks")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use cef::ImplDownloadItemCallback;
use cef::args::Args;
use cef::rc::Rc as _;
use cef::{
    App, Browser, BrowserProcessHandler, CefString, Client, CommandLine, DialogHandler,
    DisplayHandler, Frame as CefFrame, ImplApp, ImplBrowser, ImplBrowserHost,
    ImplBrowserProcessHandler, ImplClient, ImplCommandLine, ImplDisplayHandler, ImplFrame,
    ImplLifeSpanHandler, ImplRequestContext, ImplRequestContextHandler, LifeSpanHandler,
    LogSeverity, RenderProcessHandler, RequestContext, RequestContextHandler,
    RequestContextSettings, Settings, WrapApp, WrapBrowserProcessHandler, WrapClient,
    WrapDisplayHandler, WrapLifeSpanHandler, WrapRequestContextHandler, api_hash, wrap_app,
    wrap_browser_process_handler, wrap_client, wrap_display_handler, wrap_life_span_handler,
    wrap_request_context_handler,
};
use osmena_core::{
    BlockedSites, DownloadPolicy, EngineEvent, EngineInfo, ExtensionReply, ExtensionSource,
    ExtensionSupport, PageTools, PendingTab, Profile,
};
use osmena_engine::raw_window_handle::{DisplayHandle, WindowHandle};
use osmena_engine::{EngineError, GlutinSurface, PhysicalSize, Waker, WindowStrip};
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
    // Chromium's find-in-page (`BrowserHost::find`, the find handler).
    find: true,
    // Chromium's downloads, through the download handler (`delegate.rs`).
    downloads: true,
    // winit reports one wheel notch as one line on Windows; Chromium
    // scrolls about 100 px per notch.
    wheel_line_px: 100.0,
    // Chromium's extension system, driven through the hidden management
    // page (see `extensions.rs`), where the action popups have a window
    // (`popup.rs`: Windows and macOS).
    extensions: if cfg!(any(windows, target_os = "macos")) {
        Some(ExtensionSupport {
            store_name: "Chrome Web Store",
            store_url: "https://chromewebstore.google.com/",
            store_item_id: crate::extensions::web_store_item_id,
            // The docked panel arrives in a later change; until then
            // osmena draws none. The strip is sent regardless: the shell
            // sends it to every engine that runs extensions.
            side_panels: false,
        })
    } else {
        None
    },
    // Downloads, the clipboard, printing, `view-source:` pages, and the
    // developer tools are all Chromium's.
    page_tools: PageTools::ALL,
    // One Chromium runs every profile, each in a request context of its
    // own (`Runtime`).
    profiles_in_process: true,
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
    let mut app = NautilusApp::new(None, Rc::default());
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
    let mut app = NautilusApp::new(None, Rc::default());
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

/// The directory, under Chromium's root, of the profile its global
/// request context is: the default profile's data.
const DEFAULT_PROFILE_DIRECTORY: &str = "Default";

/// How long a profile's request context may take to load before its
/// engine gives up.
const CONTEXT_PATIENCE: Duration = Duration::from_secs(15);

/// Chromium, once per process: build one `Runtime` in `main`, after
/// [`run_subprocess`] returned `None`, and start an [`Engine`] from it for
/// each profile the shell opens. CEF is initialised for the first profile
/// and shut down when the runtime and every engine it started are gone;
/// it cannot be started again after that.
#[derive(Default)]
pub struct Runtime {
    shared: RefCell<Option<Rc<Shared>>>,
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime::default()
    }

    /// Start an engine for `profile`: Chromium itself first, if this is
    /// the first profile of the process.
    ///
    /// Chromium keeps every profile it runs under one directory, the
    /// default profile's `chromium/` (`Profile::shared_engine_dir`): the
    /// default profile's data is that directory's own (`Default/`, as it
    /// always was), and another profile's is `profile-<name>/` inside it
    /// (`Profile::shared_engine_data`), moved there from the profile's
    /// own `chromium/Default/` the first time. A profile's extensions
    /// stay under its own `chromium/extensions/`.
    ///
    /// Each tab's page runs in one of Chromium's renderer processes, so a
    /// renderer that crashes costs that tab its page (reported as an
    /// `EngineEvent::TabCrashed`) and nothing else. The renderer processes
    /// run without Chromium's sandbox for now (see README.md, Known
    /// limitations).
    pub fn start(&self, profile: &Profile, waker: Arc<dyn Waker>) -> Result<Engine, EngineError> {
        let running = self.shared.borrow().clone();
        let shared = match running {
            Some(shared) => shared,
            None => {
                let shared = Rc::new(Shared::initialize(
                    &profile.shared_engine_dir(),
                    waker.clone(),
                )?);
                *self.shared.borrow_mut() = Some(shared.clone());
                shared
            }
        };
        Engine::open(shared, profile, waker)
    }
}

/// What every engine of the process shares: CEF itself, its message
/// pump, and the list of open profiles. Dropping the last reference
/// shuts CEF down.
struct Shared {
    /// Chromium's user-data directory, absolute.
    root: PathBuf,
    /// True while inside `do_message_loop_work`, whose callbacks may ask
    /// for another spin; that request is served on the next call.
    spinning: Cell<bool>,
    pump: Arc<PumpScheduler>,
    /// The event queue of every open profile, for the browsers Chromium
    /// creates on its own.
    delegates: Rc<Delegates>,
}

impl Shared {
    /// Start Chromium with `root` as its user-data directory.
    fn initialize(root: &Path, waker: Arc<dyn Waker>) -> Result<Shared, EngineError> {
        let root = resolved_dir(root)?;
        forget_chromium_session(&root.join(DEFAULT_PROFILE_DIRECTORY));
        #[cfg(target_os = "macos")]
        crate::mac_app::adopt_cef_app_protocol();
        let cache_path = CefString::from(root.to_string_lossy().as_ref());
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
            // Chromium's own log on stderr: warnings, or what
            // NAUTILUS_CEF_LOG asks for ("info", "verbose") when a
            // scripted run needs to see what Chromium is doing.
            log_severity: match std::env::var("NAUTILUS_CEF_LOG").as_deref() {
                Ok("verbose") => LogSeverity::VERBOSE,
                Ok("info") => LogSeverity::INFO,
                _ => LogSeverity::WARNING,
            },
            // Opaque white behind pages that paint nothing.
            background_color: 0xFFFF_FFFF,
            browser_subprocess_path: helper_path()
                .map(|p| CefString::from(p.to_string_lossy().as_ref()))
                .unwrap_or_default(),
            ..Settings::default()
        };
        let pump = Arc::new(PumpScheduler::new(waker));
        let delegates = Rc::new(Delegates::default());
        let mut app = NautilusApp::new(Some(pump.clone()), delegates.clone());
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
        log::info!("Chromium's data is under {}", root.display());
        Ok(Shared {
            root,
            spinning: Cell::new(false),
            pump,
            delegates,
        })
    }

    /// A request context whose profile is the directory `data`, a direct
    /// child of the root, loaded and ready for browsers.
    fn context_at(&self, data: &Path) -> Result<RequestContext, EngineError> {
        let ready = Rc::new(Cell::new(false));
        let mut handler = ContextReady::new(ready.clone());
        let settings = RequestContextSettings {
            cache_path: CefString::from(data.to_string_lossy().as_ref()),
            persist_session_cookies: 1,
            ..RequestContextSettings::default()
        };
        let context = cef::request_context_create_context(Some(&settings), Some(&mut handler))
            .ok_or_else(|| {
                EngineError(format!(
                    "Chromium could not make a profile at {}",
                    data.display()
                ))
            })?;
        // Chromium loads the profile on its message loop, and a browser
        // cannot be created in it before that.
        let was_spinning = self.spinning.replace(true);
        let deadline = Instant::now() + CONTEXT_PATIENCE;
        while !ready.get() && Instant::now() < deadline {
            cef::do_message_loop_work();
            std::thread::sleep(Duration::from_millis(2));
        }
        self.spinning.set(was_spinning);
        if !ready.get() {
            return Err(EngineError(format!(
                "Chromium did not load the profile at {} in {} s",
                data.display(),
                CONTEXT_PATIENCE.as_secs()
            )));
        }
        // A directory Chromium does not accept (one it does not see as
        // inside its root) is answered with a profile kept in memory:
        // the profile's data would seem gone, and nothing would be kept.
        let kept = CefString::from(&context.cache_path()).to_string();
        if kept.is_empty() {
            return Err(EngineError(format!(
                "Chromium would not keep a profile at {} (its log on stderr has the reason)",
                data.display()
            )));
        }
        Ok(context)
    }
}

impl Drop for Shared {
    /// Every engine is gone, and with each of them its browsers (see
    /// `Engine::drop`): CEF may shut down.
    fn drop(&mut self) {
        cef::shutdown();
    }
}

wrap_request_context_handler! {
    struct ContextReady {
        ready: Rc<Cell<bool>>,
    }

    impl RequestContextHandler {
        fn on_request_context_initialized(&self, _request_context: Option<&mut RequestContext>) {
            self.ready.set(true);
        }
    }
}

/// The event queues of the open profiles.
#[derive(Default)]
struct Delegates(RefCell<Vec<Weak<DelegateState>>>);

impl Delegates {
    fn add(&self, delegate: &Rc<DelegateState>) {
        let mut all = self.0.borrow_mut();
        all.retain(|d| d.strong_count() > 0);
        all.push(Rc::downgrade(delegate));
    }

    fn live(&self) -> Vec<Rc<DelegateState>> {
        self.0.borrow().iter().filter_map(Weak::upgrade).collect()
    }

    /// The profile `browser` was created in: the one whose request
    /// context it has, else the one opened first.
    fn of(&self, browser: Option<&mut Browser>) -> Option<Rc<DelegateState>> {
        let live = self.live();
        let mut context = browser
            .and_then(|b| b.host())
            .and_then(|host| host.request_context());
        if let Some(context) = context.as_mut()
            && let Some(owner) = live.iter().find(|d| d.owns_context(context))
        {
            return Some(owner.clone());
        }
        live.into_iter().next()
    }
}

/// A directory Chromium is given, made if it is not there, as Chromium
/// itself will name it: with symbolic links resolved where it resolves
/// them (`/var` is one on macOS). It takes a profile's directory only
/// when that path starts with its root's, and it reports the paths of
/// extensions resolved. On Windows the path is left absolute and
/// unresolved, as for [`absolute_dir`].
fn resolved_dir(dir: &Path) -> Result<PathBuf, EngineError> {
    let dir = absolute_dir(dir)?;
    if cfg!(windows) {
        return Ok(dir);
    }
    std::fs::canonicalize(&dir)
        .map_err(|e| EngineError(format!("resolving engine directory {}: {e}", dir.display())))
}

/// `dir`, made if it is not there, as an absolute path. Not canonical:
/// on Windows `canonicalize` yields a verbatim `\\?\` path, and
/// Chromium's LevelDB (IndexedDB) then fails to take its lock file in
/// it.
fn absolute_dir(dir: &Path) -> Result<PathBuf, EngineError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| EngineError(format!("creating engine directory {}: {e}", dir.display())))?;
    std::path::absolute(dir)
        .map_err(|e| EngineError(format!("resolving engine directory {}: {e}", dir.display())))
}

/// Where a profile's Chromium data is to be, given where an earlier
/// build (one browser process per profile) left it: `Some((from, to))`
/// when `own` (the profile's `chromium/Default`) is there and `shared`
/// (its `profile-<name>` under the root) is not yet.
fn data_to_adopt(own_exists: bool, shared_exists: bool) -> bool {
    own_exists && !shared_exists
}

/// Move the Chromium data an earlier build kept in the profile's own
/// `chromium/Default` to `data`, where one Chromium for every profile
/// keeps it. A rename: both are under the brand's data directory.
fn adopt_profile_data(engine_dir: &Path, data: &Path) -> Result<(), EngineError> {
    let own = engine_dir.join("Default");
    if !data_to_adopt(own.is_dir(), data.exists()) {
        return Ok(());
    }
    std::fs::rename(&own, data).map_err(|e| {
        EngineError(format!(
            "moving the profile's Chromium data from {} to {}: {e}",
            own.display(),
            data.display()
        ))
    })?;
    log::info!(
        "moved the profile's Chromium data from {} to {}",
        own.display(),
        data.display()
    );
    Ok(())
}

/// One profile's engine: its event queue, its extensions, and, for any
/// profile but the default one, its request context in the process's
/// Chromium. Started by [`Runtime::start`].
pub struct Engine {
    delegate: Rc<DelegateState>,
    extensions: Rc<ExtensionManager>,
    // Last: CEF may only shut down once this profile's browsers are gone.
    shared: Rc<Shared>,
}

impl Engine {
    fn open(
        shared: Rc<Shared>,
        profile: &Profile,
        waker: Arc<dyn Waker>,
    ) -> Result<Engine, EngineError> {
        // Resolved like the root: Chromium names an extension's directory
        // with its links resolved, and the extension manager tells its own
        // copies (under this directory) from a user's folder by the path.
        let engine_dir = resolved_dir(&profile.engine_dir)?;
        let delegate = Rc::new(DelegateState::default());
        if let Some(data) = profile.shared_engine_data() {
            // Under the root as Chromium names it, which the profile's
            // own idea of the shared directory must be.
            let theirs = resolved_dir(&profile.shared_engine_dir())?;
            let name = data.file_name().filter(|_| theirs == shared.root);
            let Some(name) = name else {
                return Err(EngineError(format!(
                    "profile {:?} keeps its Chromium data at {}, outside this browser's {}",
                    profile.name,
                    data.display(),
                    shared.root.display()
                )));
            };
            let data = shared.root.join(name);
            adopt_profile_data(&engine_dir, &data)?;
            forget_chromium_session(&data);
            delegate.set_request_context(Some(shared.context_at(&data)?));
            log::info!(
                "profile {:?}: Chromium data at {}",
                profile.name,
                data.display()
            );
        }
        shared.delegates.add(&delegate);
        // The bridge asks for another turn of the pump while it has
        // something to wait for.
        let nudge = {
            let pump = shared.pump.clone();
            Box::new(move || pump.schedule(50))
        };
        let extensions = ExtensionManager::start(
            delegate.clone(),
            &engine_dir,
            UnpackQueue::new(waker),
            nudge,
        );
        delegate.set_extensions(&extensions);
        Ok(Engine {
            delegate,
            extensions,
            shared,
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
        if self.shared.spinning.replace(true) {
            return;
        }
        // Store downloads unpacked on worker threads are loaded here, on
        // the UI thread.
        self.extensions.poll_unpacked();
        // A scripted run's next command, if NAUTILUS_TEST_CONTROL is set
        // (a build with the `test-hooks` feature only).
        #[cfg(feature = "test-hooks")]
        self.extensions.poll_control();
        // The bridge's relay pages that closed underneath are opened again
        // here, never from a CEF callback.
        self.extensions.poll_bridge();
        let started = Instant::now();
        cef::do_message_loop_work();
        // Strips held for a tab's id, and answers held for a strip.
        self.extensions.tick_bridge();
        self.shared.pump.after_spin(started.elapsed());
        self.shared.spinning.set(false);
    }

    fn drain_events(&self) -> Vec<EngineEvent> {
        self.delegate.drain()
    }

    fn discard_pending(&self, pending: PendingTab) {
        match self.delegate.take_pending(pending) {
            Some(tab) => tab.discard(),
            None => log::debug!("no pending tab {pending:?} to discard"),
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

    fn configure_downloads(&self, policy: DownloadPolicy) {
        self.delegate.set_download_policy(policy);
    }

    fn configure_blocked_sites(&self, sites: BlockedSites) {
        self.delegate.set_blocked_sites(sites);
    }

    /// The strip changed: extensions' queries are answered from it, and its
    /// differences become their tab and window events.
    fn strip_changed(&self, windows: &[WindowStrip<'_>]) {
        self.extensions.strip_changed(windows);
    }

    /// The model's answer to an extension's request.
    fn extension_reply(&self, token: u64, reply: ExtensionReply) {
        self.extensions.extension_reply(token, reply);
    }

    fn cancel_download(&self, id: u32) {
        if let Some(callback) = self.delegate.download_callback(id) {
            callback.cancel();
        }
    }

    fn pause_download(&self, id: u32) {
        if let Some(callback) = self.delegate.download_callback(id) {
            callback.pause();
        }
    }

    fn resume_download(&self, id: u32) {
        if let Some(callback) = self.delegate.download_callback(id) {
            callback.resume();
        }
    }
}

impl Drop for Engine {
    /// Every browser of the profile must be closed before its request
    /// context goes, and every browser of every profile before CEF shuts
    /// down (`Shared::drop`). The tab hosts drop before the engine and
    /// ask their browsers (and popups) to close; the extension manager's
    /// goes here; closing completes on the message loop, so pump it
    /// until the last one is gone (or give up after a moment).
    fn drop(&mut self) {
        self.extensions.release_bridge();
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
        self.delegate.set_request_context(None);
    }
}

/// macOS: the helper executable Chromium runs its processes from, when
/// this is the browser inside its app bundle (`cargo xtask bundle-mac`):
/// `Contents/Frameworks/<Name> Helper.app/Contents/MacOS/<Name> Helper`
/// beside the framework, where `<Name>` is this executable's file name.
/// That is the path Chromium derives itself from the bundle; naming it
/// keeps the layout explicit and the `(Renderer)`, `(GPU)`, `(Plugin)`,
/// and `(Alerts)` variants are derived from it. `None` (the default) when
/// the helper is not there, and on other platforms, where the browser
/// executable runs the helpers itself.
#[cfg(target_os = "macos")]
fn helper_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_name()?.to_str()?;
    let helper = format!("{name} Helper");
    let path = exe
        .parent()?
        .join("../Frameworks")
        .join(format!("{helper}.app"))
        .join("Contents/MacOS")
        .join(&helper);
    if path.is_file() {
        Some(path)
    } else {
        log::warn!(
            "no helper app at {}: Chromium's processes cannot start outside the app bundle",
            path.display()
        );
        None
    }
}

#[cfg(not(target_os = "macos"))]
fn helper_path() -> Option<std::path::PathBuf> {
    None
}

/// Chromium saves the extension manager's window (a Chrome-style browser
/// is a Chrome window to its session service) and, if the browser did not
/// exit cleanly, restores it as a visible Chromium window at the next
/// launch. Nautilus never wants Chromium's session restore (the shell has
/// its own), so the saved session goes before Chromium loads the profile
/// whose directory is `profile_dir`.
fn forget_chromium_session(profile_dir: &Path) {
    let sessions = profile_dir.join("Sessions");
    if !sessions.is_dir() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(&sessions) {
        log::warn!("cannot remove {}: {e}", sessions.display());
    }
}

/// Turns CEF's "do work in `delay` ms" requests, which arrive on any
/// thread, into wakes of the application's event loop: a zero-delay
/// request wakes at once; a delayed one replaces the pending one and is
/// clamped to the interval; after every spin the timer is armed at the
/// interval when nothing is pending, so CEF's work runs even when CEF
/// does not ask for it.
struct PumpScheduler {
    waker: Arc<dyn Waker>,
    deadline: Mutex<Option<Instant>>,
    changed: Condvar,
    started: AtomicBool,
    interval: Option<Duration>,
    stats: PumpStats,
}

/// The longest the pump goes without running CEF's work, when the
/// fallback is on: cefclient's `kMaxTimerDelay`, 30 times a second.
const MAX_PUMP_INTERVAL: Duration = Duration::from_millis(1000 / 30);

/// The fallback is on: CEF asks for work through
/// `on_schedule_message_pump_work` but does not promise to ask for all
/// of it ("The application must also schedule CefDoMessageLoopWork()
/// calls when no callback is received", `cef_browser_process_handler.h`),
/// and without the tick extension calls stalled for seconds (C14 in
/// `docs/upstream-issues.md`). In a build with the `test-hooks` feature,
/// `NAUTILUS_PUMP_TICK_MS=0` switches it off to reproduce that.
const DEFAULT_PUMP_INTERVAL: Option<Duration> = Some(MAX_PUMP_INTERVAL);

/// The pump's interval: `NAUTILUS_PUMP_TICK_MS` in milliseconds when a
/// scripted run sets it (0 switches the fallback off), else `default`.
#[cfg(feature = "test-hooks")]
fn pump_interval(setting: Option<&str>, default: Option<Duration>) -> Option<Duration> {
    match setting.map(str::trim).map(str::parse::<u64>) {
        Some(Ok(0)) => None,
        Some(Ok(ms)) => Some(Duration::from_millis(ms)),
        _ => default,
    }
}

/// When the timer should next fire after CEF asked for work in `delay`,
/// and whether the request was dropped. With an interval, the request
/// replaces the pending one (CEF's contract: "any currently pending
/// scheduled call should be cancelled") and is clamped to the
/// interval. Without one, as this code did before: the earliest of the
/// two is kept, so a later request behind an earlier one is lost once
/// the earlier one fires.
fn next_deadline(
    now: Instant,
    pending: Option<Instant>,
    delay: Duration,
    interval: Option<Duration>,
) -> (Option<Instant>, bool) {
    match interval {
        Some(interval) => (Some(now + delay.min(interval)), false),
        None => match pending {
            Some(current) if current <= now + delay => (pending, true),
            _ => (Some(now + delay), false),
        },
    }
}

/// The timer's deadline once a spin has ended. Something already pending
/// is left alone; with nothing pending and the fallback on (an interval),
/// the timer arms at `now + interval`; without an interval nothing arms.
fn armed_after_spin(
    pending: Option<Instant>,
    now: Instant,
    interval: Option<Duration>,
) -> Option<Instant> {
    match (pending, interval) {
        (Some(_), _) => pending,
        (None, Some(interval)) => Some(now + interval),
        (None, None) => None,
    }
}

/// Counters that say why CEF's work is not being run, read by the
/// watch thread (`NAUTILUS_PUMP_LOG`). Times are milliseconds since
/// the scheduler was made. Only a build with the `test-hooks` feature
/// counts; any other build has the empty `PumpStats` below.
#[cfg(feature = "test-hooks")]
struct PumpStats {
    started_at: Instant,
    asked_now: AtomicU64,
    asked_later: AtomicU64,
    dropped_later: AtomicU64,
    wakes: AtomicU64,
    spins: AtomicU64,
    last_ask_ms: AtomicU64,
    last_spin_end_ms: AtomicU64,
    /// `asked_now + asked_later` as it stood when the last spin ended.
    asked_at_spin_end: AtomicU64,
    longest_spin_us: AtomicU64,
    /// The furthest behind its deadline the timer thread has fired.
    latest_timer_ms: AtomicU64,
}

#[cfg(feature = "test-hooks")]
impl PumpStats {
    fn new() -> PumpStats {
        PumpStats {
            started_at: Instant::now(),
            asked_now: AtomicU64::new(0),
            asked_later: AtomicU64::new(0),
            dropped_later: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
            spins: AtomicU64::new(0),
            last_ask_ms: AtomicU64::new(0),
            last_spin_end_ms: AtomicU64::new(0),
            asked_at_spin_end: AtomicU64::new(0),
            longest_spin_us: AtomicU64::new(0),
            latest_timer_ms: AtomicU64::new(0),
        }
    }

    fn now_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    /// CEF asked for work, at once (`now`) or after a delay.
    fn asked(&self, now: bool) {
        self.last_ask_ms.store(self.now_ms(), Ordering::Relaxed);
        let counter = if now {
            &self.asked_now
        } else {
            &self.asked_later
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// A delayed request was lost behind an earlier one.
    fn dropped(&self) {
        self.dropped_later.fetch_add(1, Ordering::Relaxed);
    }

    fn woke(&self) {
        self.wakes.fetch_add(1, Ordering::Relaxed);
    }

    fn spin_ended(&self, took: Duration) {
        self.spins.fetch_add(1, Ordering::Relaxed);
        self.last_spin_end_ms
            .store(self.now_ms(), Ordering::Relaxed);
        self.asked_at_spin_end.store(
            self.asked_now.load(Ordering::Relaxed) + self.asked_later.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.longest_spin_us
            .fetch_max(took.as_micros() as u64, Ordering::Relaxed);
    }

    /// The timer thread fired `late` after its deadline.
    fn timer_fired(&self, late: Duration) {
        self.latest_timer_ms
            .fetch_max(late.as_millis() as u64, Ordering::Relaxed);
    }
}

/// A build without the `test-hooks` feature counts nothing.
#[cfg(not(feature = "test-hooks"))]
struct PumpStats;

#[cfg(not(feature = "test-hooks"))]
impl PumpStats {
    fn new() -> PumpStats {
        PumpStats
    }

    fn asked(&self, _now: bool) {}

    fn dropped(&self) {}

    fn woke(&self) {}

    fn spin_ended(&self, _took: Duration) {}

    fn timer_fired(&self, _late: Duration) {}
}

impl PumpScheduler {
    fn new(waker: Arc<dyn Waker>) -> PumpScheduler {
        #[cfg(feature = "test-hooks")]
        let interval = pump_interval(
            std::env::var("NAUTILUS_PUMP_TICK_MS").ok().as_deref(),
            DEFAULT_PUMP_INTERVAL,
        );
        #[cfg(not(feature = "test-hooks"))]
        let interval = DEFAULT_PUMP_INTERVAL;
        log::info!("pump: fallback interval {interval:?}");
        PumpScheduler {
            waker,
            deadline: Mutex::new(None),
            changed: Condvar::new(),
            started: AtomicBool::new(false),
            interval,
            stats: PumpStats::new(),
        }
    }

    fn wake(&self) {
        self.stats.woke();
        self.waker.wake();
    }

    /// A spin ended after `took`. With the fallback on, the timer is
    /// armed at the interval when nothing else is pending: CEF does not
    /// promise to ask for every piece of work.
    fn after_spin(&self, took: Duration) {
        self.stats.spin_ended(took);
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let was_pending = deadline.is_some();
        let armed = armed_after_spin(*deadline, Instant::now(), self.interval);
        *deadline = armed;
        if !was_pending && armed.is_some() {
            self.changed.notify_one();
        }
    }

    /// `NAUTILUS_PUMP_LOG`: once a second while no spin has run for
    /// half a second, say what the pump last heard. An idle browser
    /// prints these too; they mean a stall only while something is
    /// waiting on Chromium (the scripted run's timestamps say when).
    #[cfg(feature = "test-hooks")]
    fn watch(&self) {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let stats = &self.stats;
            let now = stats.now_ms();
            let idle = now.saturating_sub(load(&stats.last_spin_end_ms));
            if idle < 500 {
                continue;
            }
            let asked = load(&stats.asked_now) + load(&stats.asked_later);
            let pending = self
                .deadline
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some();
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            log::info!(
                target: "nautilus_engine::pump",
                "at {unix_ms}: no spin for {idle} ms; CEF asked {} time(s) since it ended \
                 (last {} ms ago; {} now, {} later, {} later dropped in all); \
                 {} wake(s) for {} spin(s); timer pending: {pending}; \
                 longest spin {} us; timer at most {} ms late",
                asked - load(&stats.asked_at_spin_end),
                now.saturating_sub(load(&stats.last_ask_ms)),
                load(&stats.asked_now),
                load(&stats.asked_later),
                load(&stats.dropped_later),
                load(&stats.wakes),
                load(&stats.spins),
                load(&stats.longest_spin_us),
                load(&stats.latest_timer_ms),
            );
        }
    }

    fn start_timer_thread(self: &Arc<Self>) {
        let scheduler = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("CefPumpTimer".to_string())
            .spawn(move || scheduler.run());
        #[cfg(feature = "test-hooks")]
        if std::env::var_os("NAUTILUS_PUMP_LOG").is_some() {
            let scheduler = Arc::clone(self);
            let _ = std::thread::Builder::new()
                .name("CefPumpWatch".to_string())
                .spawn(move || scheduler.watch());
        }
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
                        self.stats.timer_fired(now - at);
                        self.wake();
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
        self.stats.asked(delay_ms <= 0);
        if delay_ms <= 0 {
            self.wake();
            return;
        }
        let mut deadline = self
            .deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (next, dropped) = next_deadline(
            Instant::now(),
            *deadline,
            Duration::from_millis(delay_ms as u64),
            self.interval,
        );
        if dropped {
            self.stats.dropped();
        }
        if next != *deadline {
            *deadline = next;
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
        // The open profiles' event queues, for the browsers Chromium
        // creates on its own.
        delegates: Rc<Delegates>,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
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
            // The default profile is Chromium's global request context,
            // and CEF gives that context the profile Chromium would
            // start with, which is the one a Chrome-style window last
            // used (`profile.last_used` in `Local State`), not
            // `Default`. The extension manager's window is such a
            // window, so after another profile had been open the
            // default profile's tabs were created in that profile's
            // cookies, storage and extensions (C17 in
            // `docs/upstream-issues.md`). The switch outranks the
            // preference.
            command_line.append_switch_with_value(
                Some(&"profile-directory".into()),
                Some(&DEFAULT_PROFILE_DIRECTORY.into()),
            );
            // A page that marks the dragged element in its `dragstart`
            // handler the common way, with `pointer-events: none`,
            // could not be dragged at all: after dispatching
            // `dragstart`, Blink hit-tests the drag origin again and
            // cancels the drag when the hit no longer lands on the
            // drag source (`DragController::StartDrag`), which that
            // very property guarantees. Chromium's own fix generates
            // the drag overlay before `dragstart` and skips the second
            // hit test; the feature is stable in Chromium but not yet
            // enabled in CEF 154's build (C9 in
            // `docs/upstream-issues.md`), so turn it on here. Drop
            // this when a CEF bump has it on by default.
            command_line.append_switch_with_value(
                Some(&"enable-blink-features".into()),
                Some(&"GenerateDragOverlayBeforeDragStart".into()),
            );
            #[cfg(target_os = "macos")]
            command_line.append_switch(Some(&"use-mock-keychain".into()));
            // Chromium switches for a scripted run (CI's smoke test on a
            // runner without a GPU, a verbose Chromium log): a
            // comma-separated list of `name` or `name=value`, without the
            // dashes. Browser process only; Chromium hands its children
            // the switches they need.
            if process_type.is_none_or(|t| t.to_string().is_empty())
                && let Ok(switches) = std::env::var("NAUTILUS_CHROMIUM_SWITCHES")
            {
                for switch in switches.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    match switch.split_once('=') {
                        Some((name, value)) => command_line.append_switch_with_value(
                            Some(&name.into()),
                            Some(&value.into()),
                        ),
                        None => command_line.append_switch(Some(&switch.into())),
                    }
                    log::info!("Chromium switch from the environment: --{switch}");
                }
            }
        }

        /// Every renderer gets the store shim (`store.rs`).
        fn render_process_handler(&self) -> Option<RenderProcessHandler> {
            Some(crate::store::StoreRenderer::new())
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            let pump = self.pump.clone()?;
            if !pump.started.swap(true, Ordering::AcqRel) {
                pump.start_timer_thread();
            }
            Some(NautilusBrowserProcessHandler::new(pump, self.delegates.clone()))
        }
    }
}

wrap_browser_process_handler! {
    struct NautilusBrowserProcessHandler {
        pump: Arc<PumpScheduler>,
        delegates: Rc<Delegates>,
    }

    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.pump.schedule(delay_ms);
        }

        /// The client of a browser Chromium creates on its own (an
        /// extension's `chrome.tabs.create`, a link the extension
        /// manager's page opens): a browser Nautilus cannot show, so its
        /// URL becomes a tab of the last focused window of its profile
        /// and it closes. Which profile is known once the browser
        /// exists (`StrayLife::on_after_created`).
        fn default_client(&self) -> Option<Client> {
            let first = self.delegates.live().into_iter().next()?;
            log::debug!("Chromium asked for a client for a browser of its own");
            let owner = Rc::new(RefCell::new(None));
            Some(StrayClient::new(
                StrayLife::new(self.delegates.clone(), owner.clone()),
                StrayDisplay::new(owner, Cell::new(false)),
                first.dialog_handler(),
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
        delegates: Rc<Delegates>,
        // The profile the browser turned out to be in, shared with the
        // display handler.
        owner: Rc<RefCell<Option<Rc<DelegateState>>>>,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let owner = self.delegates.of(browser);
            if let Some(owner) = &owner {
                owner.add_live(1);
            }
            *self.owner.borrow_mut() = owner;
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if let Some(owner) = self.owner.borrow_mut().take() {
                owner.add_live(-1);
            }
        }
    }
}

wrap_display_handler! {
    struct StrayDisplay {
        owner: Rc<RefCell<Option<Rc<DelegateState>>>>,
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
            let owner = self.owner.borrow().clone();
            if !owner.is_some_and(|owner| owner.request_tab_without_opener(url)) {
                log::warn!("no window to show it in");
            }
            if let Some(host) = browser.and_then(|b| b.host()) {
                host.close_browser(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    #[cfg(feature = "test-hooks")]
    use super::pump_interval;
    use super::{armed_after_spin, data_to_adopt, next_deadline};

    const TICK: Option<Duration> = Some(Duration::from_millis(33));

    #[test]
    fn a_profiles_own_chromium_data_is_moved_once_and_never_over_what_is_there() {
        // An earlier build's data, nothing under the root yet: moved.
        assert!(data_to_adopt(true, false));
        // Moved already, or a profile made by this build.
        assert!(!data_to_adopt(false, true));
        assert!(!data_to_adopt(false, false));
        // Both there (an older build ran again in between): what the
        // profile has been using since stays.
        assert!(!data_to_adopt(true, true));
    }

    #[test]
    fn after_a_spin_with_nothing_pending_the_timer_arms_at_the_interval() {
        let now = Instant::now();
        assert_eq!(
            armed_after_spin(None, now, TICK),
            Some(now + Duration::from_millis(33))
        );
    }

    #[test]
    fn after_a_spin_with_something_pending_the_pending_deadline_is_left_alone() {
        let now = Instant::now();
        let earlier = Some(now + Duration::from_millis(5));
        let later = Some(now + Duration::from_millis(20));
        assert_eq!(armed_after_spin(earlier, now, TICK), earlier);
        assert_eq!(armed_after_spin(later, now, TICK), later);
    }

    #[test]
    fn without_an_interval_a_spin_arms_nothing() {
        let now = Instant::now();
        assert_eq!(armed_after_spin(None, now, None), None);
        let pending = Some(now + Duration::from_millis(5));
        assert_eq!(armed_after_spin(pending, now, None), pending);
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn the_pumps_interval_comes_from_the_setting_or_the_default() {
        assert_eq!(pump_interval(None, TICK), TICK);
        assert_eq!(
            pump_interval(Some("0"), TICK),
            None,
            "0 switches the fallback off"
        );
        assert_eq!(
            pump_interval(Some("50"), None),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            pump_interval(Some(" 16 "), TICK),
            Some(Duration::from_millis(16))
        );
        assert_eq!(
            pump_interval(Some("fast"), TICK),
            TICK,
            "nonsense leaves the default"
        );
    }

    #[test]
    fn with_an_interval_a_later_request_replaces_the_pending_one_and_is_clamped() {
        let now = Instant::now();
        let pending = Some(now + Duration::from_millis(5));
        // Replaced, though the pending one was earlier: CEF's contract.
        assert_eq!(
            next_deadline(now, pending, Duration::from_millis(20), TICK),
            (Some(now + Duration::from_millis(20)), false)
        );
        // Clamped to the interval.
        assert_eq!(
            next_deadline(now, None, Duration::from_secs(60), TICK),
            (Some(now + Duration::from_millis(33)), false)
        );
    }

    #[test]
    fn without_an_interval_the_earliest_request_is_kept_and_a_later_one_is_counted_as_dropped() {
        let now = Instant::now();
        let pending = Some(now + Duration::from_millis(5));
        assert_eq!(
            next_deadline(now, pending, Duration::from_millis(20), None),
            (pending, true)
        );
        assert_eq!(
            next_deadline(now, pending, Duration::from_millis(2), None),
            (Some(now + Duration::from_millis(2)), false)
        );
        assert_eq!(
            next_deadline(now, None, Duration::from_secs(60), None),
            (Some(now + Duration::from_secs(60)), false)
        );
    }
}
