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
//! Everything here runs where the action popups have a window
//! (`popup.rs`: Windows and macOS); elsewhere `INFO.extensions` is
//! `None` and the shell shows nothing.

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

/// Which platform's `suggested_key` a command takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) enum KeyPlatform {
    Mac,
    Windows,
    /// No build runs extensions there yet; the manifest's `linux` entry
    /// and its meaning of `Ctrl` are read all the same.
    Linux,
}

impl KeyPlatform {
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub(crate) const CURRENT: KeyPlatform = if cfg!(target_os = "macos") {
        KeyPlatform::Mac
    } else if cfg!(windows) {
        KeyPlatform::Windows
    } else {
        KeyPlatform::Linux
    };

    fn key(self) -> &'static str {
        match self {
            KeyPlatform::Mac => "mac",
            KeyPlatform::Windows => "windows",
            KeyPlatform::Linux => "linux",
        }
    }
}

/// An extension's toolbar action as its manifest declares it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ActionFacts {
    /// The popup page, relative to the extension's root.
    pub popup: Option<String>,
    /// The title, unless it is absent or a `__MSG_name__` key.
    pub title: Option<String>,
}

/// One entry of the manifest's `commands`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandFacts {
    pub name: String,
    pub description: String,
    /// The manifest's suggestion for this platform, as written.
    pub suggested_key: Option<String>,
}

/// What an extension's `manifest.json` says that the engine acts on.
/// Anything missing or of the wrong type is simply absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) struct ManifestFacts {
    /// `action`, `browser_action` or `page_action`, with or without a
    /// popup.
    pub action: Option<ActionFacts>,
    /// The icon to show, relative to the extension's root.
    pub icon_path: Option<String>,
    /// `background.service_worker`.
    pub worker: Option<String>,
    /// `background.type` is `"module"`.
    pub module_worker: bool,
    /// `side_panel.default_path`, trimmed; `None` when absent or empty.
    pub side_panel: Option<String>,
    /// Whether the extension can show a side panel at all: it names a
    /// default page, or holds the `sidePanel` permission and may set a
    /// page later with `chrome.sidePanel.setOptions`.
    pub declares_side_panel: bool,
    /// `commands`, by name, each with this platform's key as written
    /// (`commands::commands_from` parses it).
    pub commands: Vec<CommandFacts>,
}

/// Read `manifest` (the parsed `manifest.json`).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn manifest_facts(manifest: &serde_json::Value, platform: KeyPlatform) -> ManifestFacts {
    let string = |v: Option<&serde_json::Value>| {
        v.and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let action = ["action", "browser_action", "page_action"]
        .iter()
        .find_map(|key| manifest.get(*key))
        .filter(|a| a.is_object());
    let icon_path = largest_icon(manifest.get("icons"))
        .or_else(|| largest_icon(action.and_then(|a| a.get("default_icon"))))
        .or_else(|| string(action.and_then(|a| a.get("default_icon"))));
    let background = manifest.get("background");
    let mut commands: Vec<CommandFacts> = manifest
        .get("commands")
        .and_then(|c| c.as_object())
        .map(|map| {
            map.iter()
                .map(|(name, command)| {
                    let keys = command.get("suggested_key");
                    CommandFacts {
                        name: name.clone(),
                        description: string(command.get("description")).unwrap_or_default(),
                        // A bare string is the key on every platform.
                        suggested_key: string(keys)
                            .or_else(|| string(keys.and_then(|k| k.get(platform.key()))))
                            .or_else(|| string(keys.and_then(|k| k.get("default")))),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    let side_panel = string(
        manifest
            .get("side_panel")
            .and_then(|s| s.get("default_path")),
    );
    let side_panel_permitted = manifest
        .get("permissions")
        .and_then(|p| p.as_array())
        .is_some_and(|list| list.iter().any(|p| p.as_str() == Some("sidePanel")));
    ManifestFacts {
        action: action.map(|a| ActionFacts {
            popup: string(a.get("default_popup")),
            // A localised title is a `__MSG_name__` key; the name
            // serves then.
            title: string(a.get("default_title")).filter(|t| !t.starts_with("__MSG_")),
        }),
        icon_path,
        worker: string(background.and_then(|b| b.get("service_worker"))),
        module_worker: background
            .and_then(|b| b.get("type"))
            .and_then(|t| t.as_str())
            == Some("module"),
        declares_side_panel: side_panel_permitted || side_panel.is_some(),
        side_panel,
        commands,
    }
}

/// The path of the largest icon of a `{"16": "a.png", ...}` map that
/// is no bigger than 128 (bigger ones are splash art), or the
/// largest of all otherwise.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
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

/// Store downloads are unpacked on a worker thread: writing an
/// extension's files takes seconds, and seconds spent inside a CEF
/// callback (the download's completion) leave CEF's external message
/// pump without further work requests, so nothing moves afterwards. The
/// outcome comes back here, and the engine hands it to the manager on
/// the next spin.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) struct UnpackQueue {
    outcomes: std::sync::Mutex<Vec<(String, Result<std::path::PathBuf, String>)>>,
    waker: std::sync::Arc<dyn osmena_engine::Waker>,
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
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

#[cfg(any(windows, target_os = "macos"))]
pub(crate) use native::ExtensionManager;

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use stub::ExtensionManager;

#[cfg(not(any(windows, target_os = "macos")))]
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
            _nudge: Box<dyn Fn()>,
        ) -> Rc<Self> {
            Rc::new(ExtensionManager)
        }

        pub(crate) fn poll_unpacked(&self) {}

        pub(crate) fn settle_deadline(&self) -> Option<std::time::Instant> {
            None
        }

        pub(crate) fn is_closed(&self) -> bool {
            true
        }

        pub(crate) fn install(&self, _source: ExtensionSource) {}

        pub(crate) fn set_enabled(&self, _id: &str, _enabled: bool) {}

        pub(crate) fn uninstall(&self, _id: &str) {}

        pub(crate) fn shutdown(&self) {}

        pub(crate) fn track_tab(
            &self,
            _tab: osmena_core::TabId,
            _browser: &cef::Browser,
            _view: std::rc::Rc<crate::delegate::ViewState>,
        ) {
        }

        pub(crate) fn untrack_tab(&self, _tab: osmena_core::TabId) {}

        #[cfg(feature = "test-hooks")]
        pub(crate) fn poll_control(&self) {}

        pub(crate) fn poll_bridge(&self) {}

        pub(crate) fn tick_bridge(&self) {}

        pub(crate) fn release_bridge(&self) {}

        pub(crate) fn strip_changed(&self, _windows: &[osmena_engine::WindowStrip<'_>]) {}

        pub(crate) fn extension_reply(&self, _token: u64, _reply: osmena_core::ExtensionReply) {}

        pub(crate) fn action_clicked(&self, _id: &str, _tab: osmena_core::TabId) -> Option<String> {
            None
        }

        pub(crate) fn command(&self, _id: &str, _name: &str, _tab: osmena_core::TabId) {}
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod native {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cef::rc::Rc as _;
    use cef::{
        Browser, BrowserSettings, BrowserView, BrowserViewDelegate, CefString, CefStringList,
        ChromeToolbarType, Client, DevToolsMessageObserver, DialogHandler, DisplayHandler,
        FileDialogMode, Frame, ImplBrowser, ImplBrowserHost, ImplBrowserViewDelegate, ImplClient,
        ImplDevToolsMessageObserver, ImplDisplayHandler, ImplFrame, ImplLifeSpanHandler,
        ImplLoadHandler, ImplPanel, ImplPanelDelegate, ImplRequest, ImplResponse,
        ImplRunFileDialogCallback, ImplUrlrequest, ImplUrlrequestClient, ImplViewDelegate,
        ImplWindow, ImplWindowDelegate, LifeSpanHandler, LoadHandler, LogSeverity, Rect,
        Registration, RunFileDialogCallback, RuntimeStyle, Urlrequest, UrlrequestClient,
        UrlrequestStatus, View, Window as CefWindow, WindowDelegate, WrapBrowserViewDelegate,
        WrapClient, WrapDevToolsMessageObserver, WrapDisplayHandler, WrapLifeSpanHandler,
        WrapLoadHandler, WrapRunFileDialogCallback, WrapUrlrequestClient, WrapWindowDelegate,
        wrap_browser_view_delegate, wrap_client, wrap_dev_tools_message_observer,
        wrap_display_handler, wrap_life_span_handler, wrap_load_handler,
        wrap_run_file_dialog_callback, wrap_urlrequest_client, wrap_window_delegate,
    };
    use osmena_core::{EngineEvent, Extension, ExtensionCommand, ExtensionSource, Favicon, TabId};

    use super::UnpackQueue;
    use crate::bridge::pattern::Grants;
    use crate::bridge::{self, Bridge};
    use crate::crx;
    use crate::delegate::DelegateState;

    /// The console message the page's change listener logs.
    const CHANGED: &str = "nautilus-ext:changed";

    /// Chromium's update service, which redirects to the CRX of an
    /// extension; `prodversion` must be a Chromium version the store
    /// serves (any recent one does).
    const CRX_URL: &str = "https://clients2.google.com/service/update2/crx?\
         response=redirect&prodversion=156.0.0.0&acceptformat=crx3&x=id%3D{id}%26uc";

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
  chrome.management.setEnabled(__ID__, __ENABLED__, function(){
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

    /// A scripted run's command directory (`NAUTILUS_TEST_CONTROL`).
    #[cfg(feature = "test-hooks")]
    struct Control {
        dir: PathBuf,
        /// The number of the next `cmd-N.json` to carry out.
        next: u32,
    }

    /// What a scripted run holds in the manager. Only a build with the
    /// `test-hooks` feature has it.
    #[cfg(feature = "test-hooks")]
    #[derive(Default)]
    struct Hooks {
        /// The scripted run's command directory, if one was named.
        control: Option<Control>,
        /// A scripted run's hold on the extension list (`hold_list`): until
        /// this instant, a listing's answer is not read (sync sees no list).
        list_hold: Option<Instant>,
        /// A listing's answer came during the hold; it is read again when the
        /// hold ends, so that the list the engine acts on is the one after it.
        list_held: bool,
    }

    /// The scripted run's control directory, if `NAUTILUS_TEST_CONTROL`
    /// names one that may be used. A directory that fails the check is
    /// logged and the hook stays off.
    #[cfg(feature = "test-hooks")]
    fn test_control(engine_dir: &Path) -> Option<Control> {
        let dir = PathBuf::from(std::env::var_os("NAUTILUS_TEST_CONTROL")?);
        if let Some(reason) = crate::test_control::control_refusal(&dir, engine_dir) {
            log::warn!(
                "TEST HOOK refused: {} ({reason}); the hook stays off",
                dir.display()
            );
            return None;
        }
        log::info!("TEST HOOK: commands are read from {}", dir.display());
        Some(Control { dir, next: 1 })
    }

    /// The file under the profile's Chromium directory that keeps which
    /// extensions open their side panel on an action click
    /// (`chrome.sidePanel.setPanelBehavior`), as Chromium keeps it in its
    /// preferences.
    const SIDE_PANEL_BEHAVIOR_FILE: &str = "side-panel-behavior.json";

    /// What a click on an extension's toolbar action came to
    /// (`ExtensionManager::click`).
    #[derive(Debug, PartialEq)]
    enum Clicked {
        /// The action has a popup at this address; the caller opens it.
        Popup(String),
        /// The engine asked the model to open or close the side panel.
        Panel,
        /// The click was handed to the bridge for `action.onClicked`.
        Delivered,
        /// The bridge did not take the click (its relay is given up).
        Refused,
        /// No popup, no panel, and the extension is not bridged.
        NotBridged,
    }

    /// What the test control logs for a click; `None` when there was no
    /// tab to click for. A click that reached nothing says `not sent`, and
    /// one handed to the bridge `sent to the bridge`: the scripted runs
    /// read those two phrases (`ci/side-panel/driver.py`).
    #[cfg(feature = "test-hooks")]
    fn click_report(clicked: Option<&Clicked>) -> &'static str {
        match clicked {
            None => "not sent (no tab to click for)",
            Some(Clicked::Popup(_)) => "the action has a popup, which the control does not open",
            Some(Clicked::Panel) => "opened or closed its side panel",
            Some(Clicked::Delivered) => "sent to the bridge",
            Some(Clicked::Refused) | Some(Clicked::NotBridged) => "not sent",
        }
    }

    #[cfg(all(test, feature = "test-hooks"))]
    mod click_tests {
        use super::{Clicked, click_report};

        #[test]
        fn the_controls_click_line_keeps_the_phrases_the_scripted_runs_read() {
            // `run.sh broken` requires `not sent` for a click the bridge
            // refused; the other modes look for `sent to the bridge`.
            assert_eq!(
                click_report(Some(&Clicked::Delivered)),
                "sent to the bridge"
            );
            for refused in [Clicked::Refused, Clicked::NotBridged] {
                assert_eq!(click_report(Some(&refused)), "not sent");
            }
            assert!(click_report(None).contains("not sent"));
            assert_eq!(
                click_report(Some(&Clicked::Panel)),
                "opened or closed its side panel"
            );
            let popup = Clicked::Popup("chrome-extension://x/popup.html".into());
            assert!(click_report(Some(&popup)).contains("popup"));
            // Neither new phrase can be taken for one of the old two.
            for other in [Clicked::Panel, popup] {
                assert!(!click_report(Some(&other)).contains("sent"));
            }
        }
    }

    /// The test control's `extension_command`: the extension and the
    /// command a scripted run wants pressed, as
    /// `{"extension_command": {"extension": "<id>", "name": "<command>"}}`.
    #[cfg(feature = "test-hooks")]
    fn control_extension_command(command: &serde_json::Value) -> Option<(&str, &str)> {
        let body = command.get("extension_command")?;
        let extension = body.get("extension")?.as_str()?;
        let name = body.get("name")?.as_str()?;
        (crx::is_extension_id(extension) && !name.is_empty() && !name.contains(char::is_whitespace))
            .then_some((extension, name))
    }

    #[cfg(all(test, feature = "test-hooks"))]
    mod command_tests {
        use serde_json::json;

        use super::control_extension_command;

        #[test]
        fn the_test_hook_command_names_an_extension_and_a_command() {
            let id = "cjpalhdlnbpafiamejdnhcphjbkeiagm";
            assert_eq!(
                control_extension_command(
                    &json!({"extension_command": {"extension": id, "name": "toggle-panel"}})
                ),
                Some((id, "toggle-panel"))
            );
            assert_eq!(
                control_extension_command(
                    &json!({"extension_command": {"extension": id, "name": "_execute_action"}})
                ),
                Some((id, "_execute_action"))
            );
            for command in [
                json!({"click": id}),
                json!({"extension_command": id}),
                json!({"extension_command": {"extension": id}}),
                json!({"extension_command": {"name": "toggle"}}),
                json!({"extension_command": {"extension": id, "name": ""}}),
                json!({"extension_command": {"extension": id, "name": "two words"}}),
                json!({"extension_command": {"extension": "not-an-id", "name": "toggle"}}),
                json!({"extension_command": {"extension": id, "name": 3}}),
            ] {
                assert_eq!(control_extension_command(&command), None, "{command}");
            }
        }
    }

    /// What the manager remembers of an extension beyond the model.
    struct Known {
        id: String,
        /// The popup page of its toolbar action, relative to its root.
        popup: Option<String>,
        /// Its worker is wrapped by the bridge's loader.
        bridged: bool,
        enabled: bool,
        /// Where Chromium loads it from.
        path: PathBuf,
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
        /// The window exists (created, not yet destroyed). CEF may only
        /// shut down once it is gone.
        window_alive: bool,
        /// Unpacked loads waiting their turn: `loadUnpacked`'s folder
        /// dialog is answered from one slot, so one load runs at a time.
        loads: VecDeque<PathBuf>,
        loading: bool,
        /// The helper extension was missing from a listing and its load
        /// has been asked for.
        helper_requested: bool,
        /// A scripted run's control and its hold on the list.
        #[cfg(feature = "test-hooks")]
        hooks: Hooks,
        /// Extensions whose directory was already looked at for the bridge
        /// this run (`on_list`), so each is bridged once per run.
        checked: HashSet<String>,
        /// The user-folder extensions being moved into the profile's copies
        /// (`bridge::migrate`).
        migrations: bridge::migrate::Migrations,
    }

    /// The management browser (never shown) and what it learnt. One per
    /// engine.
    pub(crate) struct ExtensionManager {
        delegate: Rc<DelegateState>,
        /// `<engine_dir>/extensions`, where store downloads are unpacked.
        store_dir: PathBuf,
        /// Where the unpacking threads report.
        unpacked: Arc<UnpackQueue>,
        /// The engine's end of the extension bridge: the relay pages and
        /// the tab-id table.
        bridge: Rc<Bridge>,
        /// The bridge's hub, which answers extensions' calls from the
        /// strip. Its worker lane is `bridge`; extension pages have a lane
        /// of their own, which the delegate holds too.
        hub: Rc<bridge::hub::Hub>,
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
            nudge: Box<dyn Fn()>,
        ) -> Rc<Self> {
            #[cfg(not(feature = "test-hooks"))]
            for hook in [
                "NAUTILUS_TEST_CONTROL",
                "NAUTILUS_TEST_PAGE_SCRIPT",
                "NAUTILUS_PUMP_LOG",
                "NAUTILUS_PUMP_TICK_MS",
            ] {
                if std::env::var_os(hook).is_some() {
                    log::warn!(
                        "{hook} is set, but this build has no test hooks: build with `--features test-hooks`"
                    );
                }
            }
            let store_dir = engine_dir.join("extensions");
            if let Err(e) = bridge::install::write_helper(&store_dir) {
                log::error!("cannot write the helper extension: {e}");
            }
            // The panel behaviours of earlier runs, before the first listing
            // or call can ask for them.
            delegate.load_side_panel_behaviors(engine_dir.join(SIDE_PANEL_BEHAVIOR_FILE));
            let bridge = Bridge::new(delegate.clone());
            let hub = {
                let table = {
                    let bridge = bridge.clone();
                    Box::new(move |tab: TabId| bridge.chromium_id(tab))
                };
                let reporter: bridge::hub::Reporter = {
                    let bridge = bridge.clone();
                    Box::new(move |tab, chromium, extension| {
                        bridge.report_tab(tab, chromium, extension)
                    })
                };
                let page_lane = Rc::new(bridge::pages::PageLane::default());
                delegate.set_page_lane(&page_lane);
                let hub = bridge::hub::Hub::new(
                    &delegate,
                    bridge.clone() as Rc<dyn bridge::transport::BridgeTransport>,
                    Some(page_lane),
                    table,
                    reporter,
                    nudge,
                );
                delegate.set_hub(&hub);
                hub
            };
            let manager = Rc::new(ExtensionManager {
                bridge,
                hub,
                delegate,
                store_dir,
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
                    window_alive: false,
                    loads: VecDeque::new(),
                    loading: false,
                    helper_requested: false,
                    #[cfg(feature = "test-hooks")]
                    hooks: Hooks {
                        control: test_control(engine_dir),
                        ..Hooks::default()
                    },
                    checked: HashSet::new(),
                    migrations: Default::default(),
                }),
            });
            let mut client = ManagerClient::new(
                ManagerLoad::new(manager.clone()),
                ManagerDisplay::new(manager.clone()),
                manager.delegate.dialog_handler(),
                ManagerLife::new(manager.clone()),
            );
            let mut view_delegate = ManagerViewDelegate::new();
            let mut context = manager.delegate.request_context();
            let browser_view = cef::browser_view_create(
                Some(&mut client),
                Some(&CefString::from("chrome://extensions")),
                Some(&BrowserSettings::default()),
                None,
                context.as_mut(),
                Some(&mut view_delegate),
            );
            let Some(browser_view) = browser_view else {
                log::error!("Chromium could not create the extension manager browser");
                return manager;
            };
            let mut window_delegate = ManagerWindowDelegate::new(browser_view, manager.clone());
            match cef::window_create_top_level(Some(&mut window_delegate)) {
                Some(window) => manager.inner.borrow_mut().window = Some(window),
                None => log::error!("Chromium could not create the extension manager window"),
            }
            manager
        }

        /// True once the management browser and its window are gone (or
        /// were never made), so CEF may shut down.
        pub(crate) fn is_closed(&self) -> bool {
            let inner = self.inner.borrow();
            inner.browser.is_none() && !inner.window_alive
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
            // The entry is recorded before the call, under an id of the
            // manager's own: CEF can answer inside the call.
            let id = inner.next_message_id;
            inner.next_message_id += 1;
            log::debug!("extension manager evaluates {request:?} as message {id}");
            inner.pending.insert(id, request);
            drop(inner);
            if crate::devtools::evaluate(&host, id, &script).is_none() {
                let request = self.inner.borrow_mut().pending.remove(&id);
                log::warn!("extension manager could not evaluate {request:?}");
            }
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
                ExtensionSource::Directory(path) => self.load_bridged(path),
                ExtensionSource::AskForDirectory => self.ask_for_directory(),
                ExtensionSource::Store { id } => self.install_from_store(&id),
            }
        }

        /// Load the extension in `dir`, after any load already going:
        /// `loadUnpacked` opens a folder dialog, which the engine's dialog
        /// handler answers with `dir`.
        fn load_unpacked(&self, dir: PathBuf) {
            self.inner.borrow_mut().loads.push_back(dir);
            self.next_load();
        }

        fn next_load(&self) {
            let dir = {
                let mut inner = self.inner.borrow_mut();
                if inner.loading {
                    return;
                }
                let Some(dir) = inner.loads.pop_front() else {
                    return;
                };
                inner.loading = true;
                dir
            };
            self.delegate.answer_next_dialog(dir);
            self.run(LOAD_SCRIPT.to_string(), Request::Load);
        }

        /// Load the extension the user keeps in `source`: copy it under
        /// the profile, put the bridge into the copy, load the copy. The
        /// user's folder is never written to; loading the same folder
        /// again replaces the copy, so Chromium reloads it. A directory
        /// already under the profile is bridged and loaded where it is
        /// (`install::bridge_source`). The copying runs on a worker thread,
        /// like unpacking, and `poll_unpacked` loads what it finished. "Load
        /// unpacked…", `ExtensionSource::Directory` and the test control's
        /// `load` command all come here.
        pub(crate) fn load_bridged(&self, source: PathBuf) {
            let store_dir = self.store_dir.clone();
            let queue = self.unpacked.clone();
            let name = source.display().to_string();
            let label = name.clone();
            let spawned = std::thread::Builder::new()
                .name("NautilusUnpack".to_string())
                .spawn(move || {
                    let version = bridge::install::version_of(&crate::bridge::worker_script());
                    let outcome = bridge::install::bridge_source(&source, &store_dir, &version)
                        .map(|(copy, bridged)| {
                            log::info!(
                                "extension at {} copied to {} (bridged: {bridged:?})",
                                source.display(),
                                copy.display()
                            );
                            copy
                        });
                    queue.push(name, outcome);
                });
            if let Err(e) = spawned {
                self.report_error(format!(
                    "cannot start copying the extension in {label}: {e}"
                ));
            }
        }

        /// Open the bridge's relays and helper page that are wanted and not
        /// open (`Bridge::reopen`). The engine calls this on its turn.
        pub(crate) fn poll_bridge(&self) {
            self.bridge.reopen();
        }

        /// Called after CEF's work on every turn: strips held for a tab's
        /// id, answers held for a strip, and the side panels that opened
        /// or closed since the last turn, for the extensions that listen.
        pub(crate) fn tick_bridge(&self) {
            self.hub.tick();
            self.hub.send_panel_notices();
        }

        /// `Engine::strip_changed`: extensions' queries are answered from
        /// the strip, and its differences become their events.
        pub(crate) fn strip_changed(&self, windows: &[osmena_engine::WindowStrip<'_>]) {
            self.hub.strip_changed(windows);
        }

        /// `Engine::extension_reply`: the model's answer to a request.
        pub(crate) fn extension_reply(&self, token: u64, reply: osmena_core::ExtensionReply) {
            // Every reply settles what the panel registry expected of its
            // request; a reply to a request of the engine's own ends here.
            if self.delegate.note_reply(token, &reply) {
                return;
            }
            self.hub.extension_reply(token, reply);
        }

        /// Let go of what the hub and the page lane hold of CEF, before
        /// the engine pumps towards shutdown.
        pub(crate) fn release_bridge(&self) {
            self.hub.shutdown();
            if let Some(lane) = self.delegate.page_lane() {
                lane.clear();
            }
        }

        pub(crate) fn track_tab(
            &self,
            tab: TabId,
            browser: &cef::Browser,
            view: Rc<crate::delegate::ViewState>,
        ) {
            self.bridge.track(tab, browser, view);
        }

        pub(crate) fn untrack_tab(&self, tab: TabId) {
            self.bridge.untrack(tab);
        }

        /// A click on `id`'s toolbar action at `tab`. Returns the popup's
        /// address when the action has a popup, which the caller opens
        /// under the button; everything else is carried out here.
        pub(crate) fn action_clicked(&self, id: &str, tab: TabId) -> Option<String> {
            match self.click(id, tab) {
                Clicked::Popup(url) => Some(url),
                Clicked::Panel | Clicked::Delivered => None,
                Clicked::Refused => {
                    log::warn!("extension {id}: its action click was not delivered");
                    None
                }
                Clicked::NotBridged => {
                    log::warn!(
                        "extension {id} has no popup and no bridge: its action does nothing yet"
                    );
                    None
                }
            }
        }

        /// `id`'s keyboard command `name`, for `tab`: its
        /// `commands.onCommand` listeners. An action command is a click
        /// (`action_clicked`) and never comes here.
        pub(crate) fn command(&self, id: &str, name: &str, tab: TabId) {
            self.hub.command(id, name, tab);
        }

        /// The one decision for a click on `id`'s toolbar action at `tab`
        /// (`decide_action_click`), carried out but for the popup, which
        /// only a caller with the button's place can open. The toolbar
        /// (`action_clicked`) and the test control take this same path.
        fn click(&self, id: &str, tab: TabId) -> Clicked {
            use bridge::side_panel::{ActionClick, decide_action_click};
            let bridged = self
                .inner
                .borrow()
                .known
                .iter()
                .any(|k| k.id == id && k.bridged && k.enabled);
            let popup = self.popup_url(id);
            let panel = if popup.is_none() {
                self.hub.side_panel_click(id, tab)
            } else {
                None
            };
            match decide_action_click(popup, panel, bridged) {
                ActionClick::Popup(url) => Clicked::Popup(url),
                ActionClick::Panel(request) => {
                    self.delegate.request_of_engine(id, request);
                    Clicked::Panel
                }
                ActionClick::OnClicked => {
                    if self.bridge.click(id, tab) {
                        Clicked::Delivered
                    } else {
                        Clicked::Refused
                    }
                }
                ActionClick::NotBridged => Clicked::NotBridged,
            }
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
            for (name, outcome) in self.unpacked.take() {
                let migration = self.inner.borrow_mut().migrations.finish(&name, &outcome);
                match outcome {
                    Ok(dir) => self.load_unpacked(match migration {
                        Some(bridge::migrate::Next::Load(copy)) => copy,
                        _ => dir,
                    }),
                    Err(e) => match migration {
                        Some(bridge::migrate::Next::Disable(original)) => {
                            self.set_enabled(&original, false);
                            self.report_error(format!(
                                "{name} was switched off: its copy in the profile could not be made ({e})"
                            ));
                        }
                        _ => self.report_error(format!("cannot install extension {name}: {e}")),
                    },
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
        fn popup_url(&self, id: &str) -> Option<String> {
            let inner = self.inner.borrow();
            let known = inner.known.iter().find(|k| k.id == id)?;
            let popup = known.popup.as_ref()?;
            Some(format!(
                "chrome-extension://{id}/{}",
                popup.trim_start_matches('/')
            ))
        }

        /// Carry out the next command of a scripted run, if one is
        /// waiting. Called by the engine on every spin; does nothing
        /// unless `NAUTILUS_TEST_CONTROL` was set.
        #[cfg(feature = "test-hooks")]
        pub(crate) fn poll_control(self: &Rc<Self>) {
            // A hold that ended with a listing held reads the list again, so the
            // engine acts on the list as it is after the hold.
            let again = {
                let mut inner = self.inner.borrow_mut();
                match inner.hooks.list_hold {
                    Some(until) if Instant::now() >= until => {
                        inner.hooks.list_hold = None;
                        std::mem::take(&mut inner.hooks.list_held)
                    }
                    _ => false,
                }
            };
            if again {
                log::info!("TEST HOOK: the list hold ended; the extension list is read again");
                self.list();
            }
            let (number, text) = {
                let mut inner = self.inner.borrow_mut();
                let Some(control) = inner.hooks.control.as_mut() else {
                    return;
                };
                let path = control.dir.join(format!("cmd-{}.json", control.next));
                let Ok(text) = std::fs::read_to_string(&path) else {
                    return;
                };
                control.next += 1;
                (control.next - 1, text)
            };
            let command: serde_json::Value = match serde_json::from_str(&text) {
                Ok(command) => command,
                Err(e) => {
                    log::warn!("TEST HOOK: command {number} is not JSON: {e}");
                    return;
                }
            };
            if let Some(dir) = command.get("load").and_then(|v| v.as_str()) {
                log::info!("TEST HOOK: command {number}: load {dir}");
                self.load_bridged(PathBuf::from(dir));
            } else if let Some(id) = command.get("click").and_then(|v| v.as_str()) {
                let tab = self
                    .delegate
                    .last_focused()
                    .or_else(|| self.bridge.any_tab());
                let clicked = tab.map(|tab| self.click(id, tab));
                log::info!(
                    "TEST HOOK: command {number}: click on {id} for {tab:?}: {}",
                    click_report(clicked.as_ref())
                );
            } else if let Some(message) = command.get("devtools").filter(|m| m.is_object()) {
                let mut message = message.clone();
                message["id"] = serde_json::json!(9000 + number);
                log::info!(
                    "TEST HOOK: command {number}: devtools {}",
                    message
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or("?")
                );
                let bytes = message.to_string().into_bytes();
                let host = self.inner.borrow().browser.as_ref().and_then(|b| b.host());
                if host.is_none_or(|host| host.send_dev_tools_message(Some(&bytes)) == 0) {
                    log::warn!("TEST HOOK: command {number} could not be sent");
                }
            } else if command.get("table").is_some() {
                log::info!("TEST HOOK: command {number}: table");
                self.bridge.log_table();
            } else if let Some(seconds) = command.get("hold_list").and_then(|v| v.as_u64()) {
                // The extension list is not read for this long, so the
                // engine's sync sees no change while the extension reloads.
                log::info!("TEST HOOK: command {number}: hold_list {seconds} s");
                self.inner.borrow_mut().hooks.list_hold =
                    Some(Instant::now() + std::time::Duration::from_secs(seconds));
            } else if let Some(relay) = command.get("relay_eval") {
                // The relay page's own answer to an expression, logged.
                let extension = relay
                    .get("extension")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let label = relay
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("the expression");
                let expression = relay
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                log::info!("TEST HOOK: command {number}: relay_eval {label} on {extension}");
                self.bridge.inspect_relay(extension, label, expression);
            } else if let Some((extension, name)) = control_extension_command(&command) {
                // A shortcut cannot be pressed from a script. An action
                // command is the toolbar's click, decided as the `click`
                // command decides it; any other goes to the extension's
                // `commands.onCommand` listeners.
                let tab = self
                    .delegate
                    .last_focused()
                    .or_else(|| self.bridge.any_tab());
                let outcome = match tab {
                    None => "not sent (no tab to command for)",
                    Some(tab) if crate::commands::is_action_command(name) => {
                        click_report(Some(&self.click(extension, tab)))
                    }
                    Some(tab) => {
                        self.command(extension, name, tab);
                        "handed to the bridge"
                    }
                };
                log::info!(
                    "TEST HOOK: command {number}: extension_command {name:?} of {extension} for {tab:?}: {outcome}"
                );
            } else {
                log::warn!("TEST HOOK: command {number} is not understood: {command}");
            }
        }

        /// Append a DevTools message of the management browser to the
        /// scripted run's `devtools.log`.
        #[cfg(feature = "test-hooks")]
        fn control_log(&self, message: &[u8]) {
            use std::io::Write;
            let inner = self.inner.borrow();
            let Some(control) = inner.hooks.control.as_ref() else {
                return;
            };
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(control.dir.join("devtools.log"));
            if let Ok(mut file) = file {
                let _ = file.write_all(message);
                let _ = file.write_all(b"\n");
            }
        }

        /// Close the management window and its browser; the engine then
        /// pumps until they are gone.
        pub(crate) fn shutdown(&self) {
            log::debug!("extension manager shutting down");
            self.bridge.shutdown();
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

        /// A `Runtime.evaluate` answered. Whatever a load's answer was,
        /// the next queued load may start.
        fn on_result(self: &Rc<Self>, message_id: i32, success: bool, result: &[u8]) {
            let was_load = self.inner.borrow().pending.get(&message_id) == Some(&Request::Load);
            self.handle_result(message_id, success, result);
            if was_load {
                // Answered or failed, the load is over: an answer it never
                // used must not settle the next file dialog of any page.
                self.delegate.clear_dialog_answer();
                self.inner.borrow_mut().loading = false;
                self.next_load();
            }
        }

        fn handle_result(self: &Rc<Self>, message_id: i32, success: bool, result: &[u8]) {
            let Some(request) = self.inner.borrow_mut().pending.remove(&message_id) else {
                return;
            };
            let value = match bridge::evaluation(success, result) {
                Ok(value) => value,
                Err(e) => {
                    self.report_error(format!("extension manager script failed: {e}"));
                    return;
                }
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
                            self.delegate.side_panel_uninstalled(&id);
                            // A store download of it is ours to delete.
                            let dir = self.store_dir.join(&id);
                            if crx::is_extension_id(&id)
                                && dir.is_dir()
                                && let Err(e) = std::fs::remove_dir_all(&dir)
                            {
                                log::warn!("cannot delete {}: {e}", dir.display());
                            }
                            // So is the profile's copy of a folder the user loaded.
                            let copy = self
                                .inner
                                .borrow()
                                .known
                                .iter()
                                .find(|known| known.id == id)
                                .map(|known| known.path.clone());
                            if let Some(copy) = copy
                                && copy != self.store_dir
                                && copy.starts_with(&self.store_dir)
                                && copy.is_dir()
                                && let Err(e) = std::fs::remove_dir_all(&copy)
                            {
                                log::warn!("cannot delete {}: {e}", copy.display());
                            }
                        }
                    }
                    self.list();
                }
            }
        }

        /// Whether a scripted run's hold (`hold_list`) is on: the listing is
        /// then not read, so sync sees no change while the hold lasts.
        #[cfg(feature = "test-hooks")]
        fn list_is_held(&self) -> bool {
            let mut inner = self.inner.borrow_mut();
            let held = inner
                .hooks
                .list_hold
                .is_some_and(|until| Instant::now() < until);
            if held && !std::mem::replace(&mut inner.hooks.list_held, true) {
                log::info!("TEST HOOK: an extension list read is held until the hold ends");
            }
            held
        }

        /// The current list: remember the popups, report the model's view.
        fn on_list(&self, items: &[serde_json::Value]) {
            #[cfg(feature = "test-hooks")]
            if self.list_is_held() {
                return;
            }
            let mut known = Vec::new();
            let mut listing = Vec::new();
            let mut extensions = Vec::new();
            let mut helper_loaded = false;
            let mut granted: Vec<(String, Grants, bool)> = Vec::new();
            let mut facts = Vec::new();
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
                if id == bridge::install::HELPER_ID {
                    helper_loaded = true;
                    continue;
                }
                let path = text("path");
                let manifest = if path.is_empty() {
                    ManifestInfo::default()
                } else {
                    read_manifest(Path::new(&path))
                };
                let name = text("name");
                let dir = PathBuf::from(&path);
                let enabled = text("state") == "ENABLED";
                if text("location") == "UNPACKED" && !path.is_empty() {
                    let first_look = self.inner.borrow_mut().checked.insert(id.clone());
                    if !dir.starts_with(&self.store_dir) {
                        // Loaded by an earlier build from the user's own folder,
                        // which is never written to: run it from a copy instead.
                        // The original stays until its copy is made and listed
                        // (`bridge::migrate`), and is switched off if the copy
                        // cannot be made.
                        if first_look {
                            let source = dir.display().to_string();
                            self.inner
                                .borrow_mut()
                                .migrations
                                .start(&source, &id, enabled);
                            // Off while its copy is made and listed: the original runs
                            // unbridged, with Chromium's own tab functions, until then.
                            // It stays off afterwards: the engine cannot remove it.
                            if enabled {
                                self.set_enabled(&id, false);
                            }
                            self.load_bridged(dir);
                        }
                        continue;
                    }
                    let settled = self.inner.borrow_mut().migrations.listed(&dir);
                    if let Some(on) = settled {
                        // The copy takes the state the user left the original in.
                        if enabled != on {
                            self.set_enabled(&id, on);
                        }
                    }
                    if first_look {
                        match bridge_into(&dir) {
                            Ok(true) => {
                                log::info!("put the bridge into extension {id}; reloading it");
                                self.run(
                                    RELOAD_SCRIPT.replace("__ID__", &js_string(&id)),
                                    Request::Enable,
                                );
                            }
                            Ok(false) => {}
                            // An extension that cannot carry the bridge must not
                            // run: two of Chromium's own tab functions crash the
                            // browser.
                            Err(e) => {
                                self.report_error(format!(
                                    "{name} was switched off: the bridge could not be put into it ({e})"
                                ));
                                if enabled {
                                    self.set_enabled(&id, false);
                                }
                                continue;
                            }
                        }
                    }
                }
                let bridged = manifest
                    .facts
                    .worker
                    .as_deref()
                    .is_some_and(bridge::rewrite::is_loader);
                // Every enabled extension is served, with or without a
                // worker: store.rs bridges each extension page, and the
                // worker (the loader) is the only thing woken.
                if enabled {
                    granted.push((id.clone(), manifest.grants.clone(), bridged));
                    facts.push((
                        id.clone(),
                        bridge::ExtensionFacts {
                            name: name.clone(),
                            commands: manifest.commands.clone(),
                        },
                    ));
                }
                for command in &manifest.commands {
                    log::debug!(
                        "extension {id}: command {:?} on {}",
                        command.name,
                        command
                            .chord
                            .map(|chord| chord.to_string())
                            .unwrap_or_else(|| "no key".to_string())
                    );
                }
                let action = manifest.facts.action.as_ref();
                listing.push(crate::bridge::side_panel::Listed {
                    id: id.clone(),
                    enabled,
                    side_panel_path: manifest.facts.side_panel.clone(),
                });
                known.push(Known {
                    id: id.clone(),
                    popup: action.and_then(|a| a.popup.clone()),
                    bridged,
                    enabled,
                    path: dir,
                });
                extensions.push(Extension {
                    id,
                    name: name.clone(),
                    version: text("version"),
                    description: text("description"),
                    enabled,
                    icon: manifest.icon,
                    // Any toolbar action has a title, with or without a
                    // popup: the host decides what a click does.
                    action_title: action.map(|a| a.title.clone().unwrap_or(name)),
                    side_panel: manifest.facts.declares_side_panel,
                    // As the manifest has them; the shell words a command
                    // that has no description.
                    commands: manifest.commands,
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
            // The store follows the list: an enabled extension has its
            // manifest's panel page, one that is off or gone loses its
            // options and has its open panels closed.
            self.delegate.side_panels.borrow_mut().listed(&listing);
            let closes =
                crate::bridge::side_panel::orphaned(&self.delegate.panels.borrow(), &listing);
            for (extension, request) in closes {
                self.delegate.request_of_engine(&extension, request);
            }
            let bridged: Vec<String> = known
                .iter()
                .filter(|k| k.bridged && k.enabled)
                .map(|k| k.id.clone())
                .collect();
            // The helper and its hidden page are needed only while a
            // bridged extension is on: nothing else talks to them.
            let load_helper = {
                let mut inner = self.inner.borrow_mut();
                inner.known = known;
                !bridged.is_empty()
                    && !helper_loaded
                    && !std::mem::replace(&mut inner.helper_requested, true)
            };
            if load_helper {
                log::info!("loading the helper extension");
                self.load_unpacked(bridge::install::helper_dir(&self.store_dir));
            }
            self.bridge.sync(helper_loaded, &bridged);
            if let Some(hub) = self.delegate.hub() {
                hub.set_extensions(granted);
                hub.set_facts(facts);
            }
            self.delegate
                .push(EngineEvent::ExtensionsChanged(extensions));
        }
    }

    /// A JavaScript string literal.
    fn js_string(text: &str) -> String {
        serde_json::Value::String(text.to_string()).to_string()
    }

    /// Reload an extension after its files changed on disk.
    const RELOAD_SCRIPT: &str = r#"new Promise(function(done){
  chrome.developerPrivate.reload(__ID__, {failQuietly:true}, function(){
    var err = chrome.runtime.lastError;
    done({error: err ? err.message : null});
  });
})"#;

    /// Put the bridge into `dir` if it is not there at the current version
    /// (`install::ensure_bridged`: the marker names the version of the bridge
    /// script). Returns whether the directory changed. A directory without a
    /// worker is marked too, so it is not looked at again until the script
    /// changes.
    fn bridge_into(dir: &Path) -> Result<bool, String> {
        let script = crate::bridge::worker_script();
        bridge::install::ensure_bridged(dir, &bridge::install::version_of(&script), |at| {
            bridge::install::rewrite_in_place(at, &[]).map(|_| ())
        })
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
        // The bridge goes in before Chromium ever loads the directory.
        bridge_into(dir)?;
        Ok(())
    }

    /// What the manifest adds to the listing.
    #[derive(Default)]
    struct ManifestInfo {
        facts: super::ManifestFacts,
        icon: Option<Favicon>,
        /// What the bridge lets the extension's contexts see.
        grants: Grants,
        /// The commands it declares, with this platform's chords.
        commands: Vec<ExtensionCommand>,
    }

    /// The manifest's facts and the decoded icon of the extension in
    /// `dir`. Anything missing or unreadable is simply absent.
    fn read_manifest(dir: &Path) -> ManifestInfo {
        let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
            return ManifestInfo::default();
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
            return ManifestInfo::default();
        };
        let facts = super::manifest_facts(&manifest, super::KeyPlatform::CURRENT);
        let icon = facts
            .icon_path
            .as_deref()
            .and_then(|p| decode_png(&dir.join(p.trim_start_matches('/'))));
        // A localised description is a key into the default locale's
        // messages; the extension's name needs none of this, Chromium
        // lists it localised already.
        let locale = manifest
            .get("default_locale")
            .and_then(|locale| locale.as_str())
            .map(str::trim)
            .filter(|locale| !locale.is_empty());
        let messages = locale.and_then(|locale| {
            let path = dir.join("_locales").join(locale).join("messages.json");
            let text = std::fs::read_to_string(path).ok()?;
            serde_json::from_str::<serde_json::Value>(&text).ok()
        });
        let commands = crate::commands::commands_from(
            &facts.commands,
            messages.as_ref(),
            super::KeyPlatform::CURRENT,
        );
        ManifestInfo {
            facts,
            icon,
            grants: Grants::from_manifest(&manifest),
            commands,
        }
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
            manager: Rc<ExtensionManager>,
        }

        impl ViewDelegate {}

        impl PanelDelegate {}

        impl WindowDelegate {
            /// The browser goes into the window, which is never shown.
            fn on_window_created(&self, window: Option<&mut CefWindow>) {
                self.manager.inner.borrow_mut().window_alive = true;
                if let Some(window) = window {
                    window.add_child_view(Some(&mut View::from(&self.browser_view)));
                }
            }

            /// The window may close whenever asked. (The binding's default
            /// answers 0, which keeps the window open for ever and then
            /// crashes CEF's shutdown.)
            fn can_close(&self, _window: Option<&mut CefWindow>) -> ::std::os::raw::c_int {
                1
            }

            fn on_window_destroyed(&self, _window: Option<&mut CefWindow>) {
                log::debug!("extension manager window destroyed");
                let mut inner = self.manager.inner.borrow_mut();
                inner.window_alive = false;
                inner.window = None;
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
                log::debug!("extension manager: browser created");
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
                    log::debug!("extension manager: page loaded");
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
            // Every message, for a scripted run's log (a build with the
            // `test-hooks` feature). 0: not handled, so the method results
            // and events are still delivered.
            fn on_dev_tools_message(
                &self,
                _browser: Option<&mut Browser>,
                message: Option<&[u8]>,
            ) -> ::std::os::raw::c_int {
                #[cfg(feature = "test-hooks")]
                if let Some(message) = message {
                    self.manager.control_log(message);
                }
                #[cfg(not(feature = "test-hooks"))]
                let _ = message;
                0
            }

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
                    self.manager.load_bridged(PathBuf::from(path));
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
    use super::{CommandFacts, KeyPlatform, manifest_facts, web_store_item_id};
    use url::Url;

    fn facts(json: &str, platform: KeyPlatform) -> super::ManifestFacts {
        manifest_facts(&serde_json::from_str(json).unwrap(), platform)
    }

    #[test]
    fn an_action_without_a_popup_is_still_an_action() {
        let f = facts(
            r#"{"action": {"default_title": " Open panel "}}"#,
            KeyPlatform::Mac,
        );
        let action = f.action.unwrap();
        assert_eq!(action.popup, None);
        assert_eq!(action.title.as_deref(), Some("Open panel"));
        // An empty action object is an action too; no key at all is none.
        assert!(
            facts(r#"{"action": {}}"#, KeyPlatform::Mac)
                .action
                .is_some()
        );
        assert!(facts(r#"{"name": "x"}"#, KeyPlatform::Mac).action.is_none());
        assert!(
            facts(r#"{"action": "yes"}"#, KeyPlatform::Mac)
                .action
                .is_none()
        );
    }

    #[test]
    fn the_older_action_keys_and_a_localised_title_are_read_as_before() {
        let f = facts(
            r#"{"browser_action": {"default_popup": "/p.html", "default_title": "__MSG_title__"}}"#,
            KeyPlatform::Windows,
        );
        let action = f.action.unwrap();
        assert_eq!(action.popup.as_deref(), Some("/p.html"));
        assert_eq!(action.title, None);
        assert!(
            facts(r#"{"page_action": {}}"#, KeyPlatform::Mac)
                .action
                .is_some()
        );
    }

    #[test]
    fn the_icon_is_the_largest_up_to_128_then_the_actions() {
        let f = facts(
            r#"{"icons": {"16": "a.png", "128": "b.png", "512": "c.png"}}"#,
            KeyPlatform::Mac,
        );
        assert_eq!(f.icon_path.as_deref(), Some("b.png"));
        let f = facts(
            r#"{"action": {"default_icon": {"32": "d.png"}}}"#,
            KeyPlatform::Mac,
        );
        assert_eq!(f.icon_path.as_deref(), Some("d.png"));
        let f = facts(r#"{"action": {"default_icon": "e.png"}}"#, KeyPlatform::Mac);
        assert_eq!(f.icon_path.as_deref(), Some("e.png"));
    }

    #[test]
    fn the_worker_entry_and_its_kind_are_read() {
        let f = facts(
            r#"{"background": {"service_worker": "bg/sw.js", "type": "module"}}"#,
            KeyPlatform::Mac,
        );
        assert_eq!(f.worker.as_deref(), Some("bg/sw.js"));
        assert!(f.module_worker);
        let f = facts(
            r#"{"background": {"service_worker": "sw.js"}}"#,
            KeyPlatform::Mac,
        );
        assert!(!f.module_worker);
        // A Manifest V2 background page is no worker.
        let f = facts(r#"{"background": {"scripts": ["a.js"]}}"#, KeyPlatform::Mac);
        assert_eq!(f.worker, None);
    }

    #[test]
    fn the_side_panel_and_the_commands_are_read_with_this_platforms_key() {
        let json = r#"{
            "side_panel": {"default_path": "panel.html"},
            "commands": {
                "toggle": {
                    "suggested_key": {"default": "Ctrl+E", "mac": "Command+E"},
                    "description": "Toggle the panel"
                },
                "_execute_action": {"suggested_key": {"default": "Ctrl+Shift+U"}},
                "bare": {}
            }
        }"#;
        let mac = facts(json, KeyPlatform::Mac);
        assert_eq!(mac.side_panel.as_deref(), Some("panel.html"));
        assert_eq!(
            mac.commands,
            vec![
                CommandFacts {
                    name: "_execute_action".to_string(),
                    description: String::new(),
                    suggested_key: Some("Ctrl+Shift+U".to_string()),
                },
                CommandFacts {
                    name: "bare".to_string(),
                    description: String::new(),
                    suggested_key: None,
                },
                CommandFacts {
                    name: "toggle".to_string(),
                    description: "Toggle the panel".to_string(),
                    suggested_key: Some("Command+E".to_string()),
                },
            ]
        );
        let windows = facts(json, KeyPlatform::Windows);
        assert_eq!(windows.commands[2].suggested_key.as_deref(), Some("Ctrl+E"));
        // A bare string is the key on every platform.
        assert_eq!(
            facts(
                r#"{"commands": {"p": {"suggested_key": "Alt+Shift+P"}}}"#,
                KeyPlatform::Mac
            )
            .commands[0]
                .suggested_key
                .as_deref(),
            Some("Alt+Shift+P")
        );
    }

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

#[cfg(test)]
mod side_panel_manifest_tests {
    use super::{KeyPlatform, ManifestFacts, manifest_facts};
    use serde_json::json;

    fn facts(manifest: &serde_json::Value) -> ManifestFacts {
        manifest_facts(manifest, KeyPlatform::CURRENT)
    }

    #[test]
    fn the_default_path_is_read_as_written() {
        let manifest = json!({
            "permissions": ["sidePanel"],
            "side_panel": { "default_path": " sidepanel.html " }
        });
        assert_eq!(
            facts(&manifest).side_panel.as_deref(),
            Some("sidepanel.html")
        );
        assert!(facts(&manifest).declares_side_panel);
    }

    #[test]
    fn a_permission_without_a_page_still_declares_a_panel() {
        // cookbook.sidepanel-site-specific: the page comes from setOptions.
        let manifest = json!({ "permissions": ["sidePanel", "tabs"] });
        assert_eq!(facts(&manifest).side_panel, None);
        assert!(facts(&manifest).declares_side_panel);
    }

    #[test]
    fn anything_else_is_no_panel() {
        for manifest in [
            json!({}),
            json!({ "permissions": ["tabs"] }),
            json!({ "side_panel": {} }),
            json!({ "side_panel": { "default_path": "" } }),
            json!({ "side_panel": { "default_path": 3 } }),
            json!({ "side_panel": "sidepanel.html" }),
        ] {
            assert_eq!(facts(&manifest).side_panel, None, "{manifest}");
            assert!(!facts(&manifest).declares_side_panel, "{manifest}");
        }
    }
}
