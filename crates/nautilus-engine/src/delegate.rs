//! CEF handler callbacks become [`EngineEvent`]s keyed by core tab ids.
//!
//! Every tab has its own `Client` whose handlers share that tab's
//! [`ViewState`], so a callback knows its tab without any id mapping. All
//! callbacks arrive on the main thread from inside `do_message_loop_work`
//! (CEF's UI thread is the thread that initialised it when the message
//! pump is external), so plain `RefCell`s suffice.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use cef::rc::Rc as _;
use cef::{
    BeforeDownloadCallback, Browser, CefString, CefStringList, CefStringUserfree, Client,
    ContextMenuHandler, ContextMenuMediaType, ContextMenuParams, CursorInfo, CursorType,
    DialogHandler, DisplayHandler, DownloadHandler, DownloadImageCallback, DownloadItem,
    DownloadItemCallback, DragData, DragOperationsMask, Errorcode, FileDialogCallback,
    FileDialogMode, FindHandler, Frame as CefFrame, Image, ImplBeforeDownloadCallback,
    ImplBinaryValue, ImplBrowser, ImplBrowserHost, ImplClient, ImplContextMenuHandler,
    ImplContextMenuParams, ImplDialogHandler, ImplDisplayHandler, ImplDownloadHandler,
    ImplDownloadImageCallback, ImplDownloadItem, ImplDragData, ImplFileDialogCallback,
    ImplFindHandler, ImplFrame, ImplImage, ImplKeyboardHandler, ImplLifeSpanHandler,
    ImplLoadHandler, ImplPermissionHandler, ImplRenderHandler, ImplRequest, ImplRequestHandler,
    ImplRunContextMenuCallback, KeyEvent, KeyEventType, KeyboardHandler, LifeSpanHandler,
    LoadHandler, LogSeverity, MediaAccessCallback, MenuModel, PaintElementType, PermissionHandler,
    ProcessId, ProcessMessage, Rect, RenderHandler, Request, RequestHandler,
    RunContextMenuCallback, ScreenInfo, TerminationStatus, TransitionType, WindowOpenDisposition,
    WrapClient, WrapContextMenuHandler, WrapDialogHandler, WrapDisplayHandler, WrapDownloadHandler,
    WrapDownloadImageCallback, WrapFindHandler, WrapKeyboardHandler, WrapLifeSpanHandler,
    WrapLoadHandler, WrapPermissionHandler, WrapRenderHandler, WrapRequestHandler, wrap_client,
    wrap_context_menu_handler, wrap_dialog_handler, wrap_display_handler, wrap_download_handler,
    wrap_download_image_callback, wrap_find_handler, wrap_keyboard_handler, wrap_life_span_handler,
    wrap_load_handler, wrap_permission_handler, wrap_render_handler, wrap_request_handler,
};
use osmena_core::{
    BlockedSites, Cursor, DownloadPolicy, DownloadReport, DownloadState, EditState, EngineEvent,
    ExtensionSource, LoadState, Media, MediaKind, MediaRequest, Misspelling, PageContext,
    PendingTab, RgbaImage, TabId,
};
use url::Url;

use osmena_engine::PixelRect;

use crate::extensions::ExtensionManager;
use crate::frame::Frame;
use crate::input;
use crate::store::StoreRequest;

/// A CEF dirty rectangle as the shell's; `None` if it is not a
/// rectangle of pixels at all (a negative edge or size).
fn pixel_rect(rect: &Rect) -> Option<PixelRect> {
    Some(PixelRect {
        x: u32::try_from(rect.x).ok()?,
        y: u32::try_from(rect.y).ok()?,
        width: u32::try_from(rect.width).ok()?,
        height: u32::try_from(rect.height).ok()?,
    })
}

/// The native cursor handle `on_cursor_change` receives. The `cef` crate's
/// generated handler signature names `CursorHandle` on Windows and Linux
/// but a bare pointer on macOS, and the implementation must match exactly.
#[cfg(target_os = "macos")]
type NativeCursor = *mut u8;
#[cfg(not(target_os = "macos"))]
type NativeCursor = cef::CursorHandle;

/// Shared by every tab of the process: the event queue and the tabs page
/// content asked for.
#[derive(Default)]
pub(crate) struct DelegateState {
    events: RefCell<VecDeque<EngineEvent>>,
    /// Tabs page content asked for (`window.open`, middle-click), held
    /// until the model adopts them.
    pending: RefCell<HashMap<PendingTab, Pending>>,
    next_pending: Cell<u64>,
    /// Browsers created and not yet destroyed (`on_after_created` to
    /// `on_before_close`); CEF may only shut down at zero.
    live_browsers: Cell<usize>,
    /// The tab focused last, which stands as the opener of a tab that
    /// Chromium opens on its own (an extension's `chrome.tabs.create`).
    last_focused: Cell<Option<TabId>>,
    /// That tab's browser, whose visible window owns the dialogs the
    /// engine shows the user.
    focused_browser: RefCell<Option<Browser>>,
    /// The directory the next folder dialog is answered with, whichever
    /// browser Chromium routes it to (the extension manager's `loadUnpacked`
    /// opens one; CEF does not always attribute it to that browser).
    dialog_answer: RefCell<Option<PathBuf>>,
    /// The extension manager, for what store pages ask (`store.rs`); it
    /// holds this state, hence weak.
    extensions: RefCell<Weak<ExtensionManager>>,
    /// Where downloads go, as the shell last said; `None` until it does
    /// (a download before then asks where to save).
    download_policy: RefCell<Option<DownloadPolicy>>,
    /// The callback that cancels, pauses or resumes each download that
    /// is going, by Chromium's id for it.
    downloads: RefCell<HashMap<u32, DownloadItemCallback>>,
    /// The sites the profile blocks, as the shell last said; a
    /// main-frame navigation to one is cancelled in `on_before_browse`.
    blocked: RefCell<BlockedSites>,
    /// The id of the next request for the microphone or the camera.
    next_media_request: Cell<u64>,
    /// Each tab's browser, by the model's id, with CEF's identifier for it
    /// (kept so a page's browser can be matched without asking CEF under
    /// the borrow): for the screenshot `tabs.captureVisibleTab` takes, and
    /// to tell which view a page that calls the bridge is in.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    views: RefCell<HashMap<TabId, (std::os::raw::c_int, Browser)>>,
    /// Each open action popup's browser, by CEF's identifier, with the tab
    /// that was active when it opened (which names its window).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    popups_over: RefCell<HashMap<std::os::raw::c_int, TabId>>,
    /// The bridge's hub; the engine owns it, hence weak.
    #[cfg(any(windows, target_os = "macos"))]
    hub: RefCell<Weak<crate::bridge::hub::Hub>>,
    /// The lane extension pages call the bridge on (`bridge/pages.rs`).
    #[cfg(any(windows, target_os = "macos"))]
    page_lane: RefCell<Option<Rc<crate::bridge::pages::PageLane>>>,
}

impl DelegateState {
    pub(crate) fn drain(&self) -> Vec<EngineEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    pub(crate) fn push(&self, event: EngineEvent) {
        self.events.borrow_mut().push_back(event);
    }

    pub(crate) fn take_pending(&self, pending: PendingTab) -> Option<Pending> {
        self.pending.borrow_mut().remove(&pending)
    }

    pub(crate) fn live_browsers(&self) -> usize {
        self.live_browsers.get()
    }

    pub(crate) fn set_last_focused(&self, tab: TabId, browser: &Browser) {
        self.last_focused.set(Some(tab));
        let displaced = self.focused_browser.borrow_mut().replace(browser.clone());
        drop(displaced);
    }

    /// The tab focused last, which is the tab an extension's action
    /// click is about.
    pub(crate) fn last_focused(&self) -> Option<TabId> {
        self.last_focused.get()
    }

    /// The browser of the last focused tab, for dialogs the user should
    /// see.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn focused_browser(&self) -> Option<Browser> {
        self.focused_browser.borrow().clone()
    }

    pub(crate) fn forget_browser(&self, browser: &Browser) {
        let id = browser.identifier();
        // Each removed value is let go of after its borrow is released.
        let focused = self.focused_browser.borrow().clone();
        if focused.as_ref().is_some_and(|f| f.identifier() == id) {
            let taken = self.focused_browser.borrow_mut().take();
            drop(taken);
        }
        drop(focused);
        let views: Vec<(TabId, (std::os::raw::c_int, Browser))> = {
            let mut views = self.views.borrow_mut();
            let tabs: Vec<TabId> = views
                .iter()
                .filter(|(_, (known, _))| *known == id)
                .map(|(tab, _)| *tab)
                .collect();
            tabs.into_iter()
                .filter_map(|tab| views.remove(&tab).map(|view| (tab, view)))
                .collect()
        };
        drop(views);
        self.popups_over.borrow_mut().remove(&id);
    }

    /// A tab's view was created: its browser is `browser`.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub(crate) fn register_view(&self, tab: TabId, browser: &Browser) {
        let id = browser.identifier();
        let displaced = self.views.borrow_mut().insert(tab, (id, browser.clone()));
        drop(displaced);
    }

    /// A tab's view is gone (`TabHost::close_tab`, or the host dropped).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub(crate) fn unregister_view(&self, tab: TabId) {
        let removed = self.views.borrow_mut().remove(&tab);
        drop(removed);
    }

    /// The browser showing the view `tab`.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub(crate) fn browser_of(&self, tab: TabId) -> Option<Browser> {
        self.views
            .borrow()
            .get(&tab)
            .map(|(_, browser)| browser.clone())
    }

    /// An action popup opened over `over`: its browser, by CEF's id.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub(crate) fn register_popup(&self, browser: &Browser, over: TabId) {
        let id = browser.identifier();
        self.popups_over.borrow_mut().insert(id, over);
    }

    /// Who is asking, for a bridge call from a page in `browser`: the view it
    /// is, the popup it is, or (any other browser) nobody with a window, like
    /// a worker. Used by the extension pages' lane (Task 12).
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn asker_of(&self, browser: &Browser) -> crate::bridge::transport::Asker {
        use crate::bridge::transport::Asker;
        let id = browser.identifier();
        let view = self
            .views
            .borrow()
            .iter()
            .find(|(_, (known, _))| *known == id)
            .map(|(tab, _)| *tab);
        if let Some(tab) = view {
            return Asker::View(tab);
        }
        match self.popups_over.borrow().get(&id) {
            Some(over) => Asker::Popup { over: *over },
            None => Asker::Worker,
        }
    }

    /// The lane extension pages call the bridge on, once the engine has
    /// made it (`engine.rs`). The lane holds no reference back here.
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn set_page_lane(&self, lane: &Rc<crate::bridge::pages::PageLane>) {
        *self.page_lane.borrow_mut() = Some(lane.clone());
    }

    /// The page lane, while the engine is alive.
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn page_lane(&self) -> Option<Rc<crate::bridge::pages::PageLane>> {
        self.page_lane.borrow().clone()
    }

    /// The bridge's hub, once the engine has made it.
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn set_hub(&self, hub: &Rc<crate::bridge::hub::Hub>) {
        *self.hub.borrow_mut() = Rc::downgrade(hub);
    }

    /// The bridge's hub, while the engine is alive.
    #[cfg(any(windows, target_os = "macos"))]
    pub(crate) fn hub(&self) -> Option<Rc<crate::bridge::hub::Hub>> {
        self.hub.borrow().upgrade()
    }

    /// Answer the next folder dialog with `dir` instead of showing it.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn set_extensions(&self, manager: &Rc<ExtensionManager>) {
        *self.extensions.borrow_mut() = Rc::downgrade(manager);
    }

    /// A store page's own button was used: the extension manager does
    /// what the puzzle menu would.
    pub(crate) fn store_request(&self, request: StoreRequest) {
        let Some(manager) = self.extensions.borrow().upgrade() else {
            log::warn!("a store page asked to {request}, but there is no extension manager");
            return;
        };
        log::info!("a store page asked to {request}");
        match request {
            StoreRequest::Install(id) => manager.install(ExtensionSource::Store { id }),
            StoreRequest::Uninstall(id) => manager.uninstall(&id),
            StoreRequest::Enable(id) => manager.set_enabled(&id, true),
            StoreRequest::Disable(id) => manager.set_enabled(&id, false),
        }
    }

    pub(crate) fn answer_next_dialog(&self, dir: PathBuf) {
        *self.dialog_answer.borrow_mut() = Some(dir);
    }

    /// A load's answer is over: whatever it left unused is dropped.
    pub(crate) fn clear_dialog_answer(&self) {
        clear_answer(&self.dialog_answer);
    }

    /// The dialog handler every client of this engine uses: a pending
    /// answer settles the dialog without showing it; otherwise CEF shows
    /// its own, owned by the browser's window.
    pub(crate) fn dialog_handler(self: &Rc<Self>) -> DialogHandler {
        AnswerDialog::new(self.clone())
    }

    /// A URL Chromium wants shown that has no opener tab (an extension
    /// asked for a tab): offer it to the last focused tab's window.
    pub(crate) fn set_download_policy(&self, policy: DownloadPolicy) {
        *self.download_policy.borrow_mut() = Some(policy);
    }

    pub(crate) fn set_blocked_sites(&self, sites: BlockedSites) {
        *self.blocked.borrow_mut() = sites;
    }

    pub(crate) fn download_callback(&self, id: u32) -> Option<DownloadItemCallback> {
        self.downloads.borrow().get(&id).cloned()
    }

    pub(crate) fn request_tab_without_opener(&self, url: Url) -> bool {
        match self.last_focused.get() {
            Some(opener) => {
                self.request_tab(opener, url, false);
                true
            }
            None => false,
        }
    }

    /// A browser other than a tab (the extension manager, a popup) was
    /// created (`+1`) or destroyed (`-1`).
    pub(crate) fn add_live(&self, delta: i64) {
        let count = (self.live_browsers.get() as i64 + delta).max(0);
        self.live_browsers.set(count as usize);
    }

    /// Record that `opener` asked for a new tab at `url` and report it,
    /// `background` when it should open beside the opener without taking
    /// its place.
    pub(crate) fn request_tab(&self, opener: TabId, url: Url, background: bool) {
        self.request(opener, Pending::Url(url), background);
    }

    /// Record that Chromium created `browser` for a popup of `opener`
    /// (`window.open`, a link with a target) and report it as a new tab.
    fn request_popup(
        &self,
        opener: TabId,
        view: Rc<ViewState>,
        browser: Browser,
        background: bool,
    ) {
        self.request(opener, Pending::Popup { view, browser }, background);
    }

    fn request(&self, opener: TabId, tab: Pending, background: bool) {
        let pending = PendingTab(self.next_pending.get());
        self.next_pending.set(pending.0 + 1);
        self.pending.borrow_mut().insert(pending, tab);
        self.push(EngineEvent::NewTabRequested {
            opener,
            pending,
            background,
        });
    }
}

/// A tab page content asked for, until the model adopts it.
pub(crate) enum Pending {
    /// A URL to load in a browser of its own, unrelated to the page that
    /// asked (a middle- or Ctrl-click on a link, a tab Chromium opened
    /// on its own).
    Url(Url),
    /// A popup Chromium created itself, so that it and its opener keep
    /// their handles to each other (`window.open`'s result,
    /// `window.opener`, a window found again by its name).
    Popup {
        view: Rc<ViewState>,
        browser: Browser,
    },
}

impl Pending {
    /// Nothing adopted the tab: a popup's browser is closed.
    pub(crate) fn discard(self) {
        if let Pending::Popup { view, browser } = self {
            view.closing.set(true);
            if let Some(host) = browser.host() {
                host.close_browser(1);
            }
        }
    }
}

/// What a popup's view remembers from `on_before_popup` until Chromium
/// has created its browser.
#[derive(Clone, Copy)]
pub(crate) struct PopupRequest {
    opener: TabId,
    background: bool,
}

/// How Nautilus opens what `on_before_popup` is asked for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PopupPlan {
    /// Cancel Chromium's popup and open a window at its URL
    /// (Shift-click on a link with a target).
    Window,
    /// Let Chromium create the popup, shown as a tab: in front, or
    /// `background` behind its opener.
    Tab { background: bool },
}

pub(crate) fn popup_plan(disposition: WindowOpenDisposition) -> PopupPlan {
    if disposition == WindowOpenDisposition::NEW_WINDOW {
        PopupPlan::Window
    } else {
        PopupPlan::Tab {
            background: disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB,
        }
    }
}

/// Where a tab's context menu was opened: the frame it was opened in and
/// the point, in logical pixels from the view's top left, for the
/// commands that act there (the media toggles, "Inspect").
pub(crate) struct MenuPoint {
    pub frame: Option<CefFrame>,
    pub x: i32,
    pub y: i32,
}

/// A drag the page started on one of its draggable elements
/// (`dragstart`), carried out within the view. Chromium expects the
/// embedder to run the system drag and to feed its own drop target
/// side meanwhile; here no system drag starts. The shell's pointer
/// capture keeps sending the pressed tab every move and the release,
/// and `TabHost::drag_input` turns them into Chromium's drop-target
/// calls, so the page gets the whole `drag`, `dragover`, `drop`,
/// `dragend` cycle and unwinds whatever `dragstart` set up. Nothing
/// leaves the view: a drop on the chrome, on another window or outside
/// the browser ends as a cancelled drag (`dragend` all the same).
pub(crate) struct DragSession {
    /// Chromium's own copy of what is dragged (the lent object is
    /// read-only), fed back to it when the pointer enters the view.
    pub data: DragData,
    /// The operations the source allows (`effectAllowed`).
    pub allowed: DragOperationsMask,
    /// Whether Chromium's drop target has been told the drag is over
    /// the view (`drag_target_drag_enter`) and not since told it left.
    pub entered: bool,
    /// The operation the page accepts where the pointer is, as Chromium
    /// last said (`update_drag_cursor`); none until it does.
    pub operation: DragOperationsMask,
    /// The pointer's offset into the drag image, physical pixels.
    pub hotspot: (i32, i32),
}

/// An event of a view that is not a tab yet, made once it is one.
type HeldEvent = Box<dyn FnOnce(TabId) -> EngineEvent>;

/// What one tab's handlers need: the tab id, the geometry CEF asks about,
/// and the frame CEF paints into.
pub(crate) struct ViewState {
    /// The tab this view is, once the model has said: at once for a tab
    /// the shell creates, at adoption for a popup Chromium creates.
    tab: Cell<Option<TabId>>,
    /// What the browser reported before the view was a tab, told to the
    /// model when it becomes one.
    held: RefCell<Vec<HeldEvent>>,
    /// For a popup's view: who opened it, until its browser exists.
    popup: Cell<Option<PopupRequest>>,
    /// The native window the tab belongs to, which a popup of this tab
    /// belongs to as well.
    pub parent: cef::sys::cef_window_handle_t,
    pub delegate: Rc<DelegateState>,
    /// The view size in logical pixels (CSS pixels at zoom 1).
    pub dip_size: Cell<(i32, i32)>,
    pub scale: Cell<f32>,
    /// The screen position of the view's top-left corner, logical pixels,
    /// so CEF can place menus and widgets. Shared with the host, which
    /// updates it as the window moves.
    pub origin: Rc<Cell<(i32, i32)>>,
    pub frame: RefCell<Frame>,
    /// Set when the host asked the browser to close, so `do_close` can
    /// tell a close by the page (`window.close()`) from ours.
    pub closing: Cell<bool>,
    /// Where the page's context menu was last opened, until the shell
    /// says the menu closed.
    pub context_menu: RefCell<Option<MenuPoint>>,
    /// Whether the shell last said this tab has keyboard focus, so the
    /// focus can be told again to a renderer that arrives later
    /// (`on_load_start`).
    pub focused: Cell<bool>,
    /// Whether the window has the tab behind another (`TabHost::hide`
    /// until `TabHost::show`).
    pub hidden: Cell<bool>,
    /// Whether an extension is debugging the tab, as the bridge reports
    /// it (`bridge::debugger`). A hidden renderer answers no input, so a
    /// debugged tab keeps painting even while it is hidden.
    pub debugged: Cell<bool>,
    /// The drag the page is carrying out, while it is.
    pub drag: RefCell<Option<DragSession>>,
    /// The cursor the page last asked for, to put back after a drag
    /// (a drag shows its own, and Chromium does not repeat a cursor it
    /// believes is still shown).
    pub cursor: Cell<Cursor>,
    /// The page's requests for the microphone or the camera that wait
    /// for the user, by the id they were reported under: Chromium's
    /// callback and the permissions it asked for, which an allowing
    /// answer must repeat exactly.
    pub media_requests: RefCell<HashMap<u64, (MediaAccessCallback, u32)>>,
}

impl ViewState {
    /// Whether the tab's renderer should be told it is hidden: it is
    /// behind another tab and no extension is debugging it.
    pub(crate) fn wants_hidden(&self) -> bool {
        self.hidden.get() && !self.debugged.get()
    }

    /// Report an event that names no tab.
    fn push(&self, event: EngineEvent) {
        self.delegate.push(event);
    }

    /// The view of a tab the shell creates, in the native window
    /// `parent`, `dip_size` logical pixels at `scale`.
    pub(crate) fn new(
        tab: TabId,
        delegate: Rc<DelegateState>,
        parent: cef::sys::cef_window_handle_t,
        dip_size: (i32, i32),
        scale: f32,
        origin: Rc<Cell<(i32, i32)>>,
    ) -> ViewState {
        ViewState {
            tab: Cell::new(Some(tab)),
            held: RefCell::new(Vec::new()),
            popup: Cell::new(None),
            parent,
            delegate,
            dip_size: Cell::new(dip_size),
            scale: Cell::new(scale),
            origin,
            frame: RefCell::new(Frame::default()),
            closing: Cell::new(false),
            context_menu: RefCell::new(None),
            focused: Cell::new(false),
            hidden: Cell::new(false),
            debugged: Cell::new(false),
            drag: RefCell::new(None),
            cursor: Cell::new(Cursor::Default),
            media_requests: RefCell::new(HashMap::new()),
        }
    }

    /// The view of a popup `opener` asked for: in the same window, at
    /// the same size, and no tab until the model adopts it.
    fn popup_of(&self, opener: TabId, background: bool) -> ViewState {
        ViewState {
            tab: Cell::new(None),
            popup: Cell::new(Some(PopupRequest { opener, background })),
            ..ViewState::new(
                opener,
                self.delegate.clone(),
                self.parent,
                self.dip_size.get(),
                self.scale.get(),
                self.origin.clone(),
            )
        }
    }

    /// The tab this view is, if it is one yet.
    pub(crate) fn tab(&self) -> Option<TabId> {
        self.tab.get()
    }

    /// The tab's name in a log line.
    pub(crate) fn label(&self) -> String {
        match self.tab.get() {
            Some(tab) => format!("{tab:?}"),
            None => "(a popup not yet adopted)".to_string(),
        }
    }

    /// Report what `event` makes for this view's tab: now, or when the
    /// view becomes a tab.
    pub(crate) fn report(&self, event: impl FnOnce(TabId) -> EngineEvent + 'static) {
        match self.tab.get() {
            Some(tab) => self.delegate.push(event(tab)),
            None => self.held.borrow_mut().push(Box::new(event)),
        }
    }

    /// The model adopted this popup's view as `tab`: it reports as that
    /// tab from now on, starting with what it held back.
    pub(crate) fn adopt(&self, tab: TabId) {
        self.tab.set(Some(tab));
        for event in self.held.take() {
            self.delegate.push(event(tab));
        }
    }

    /// Page content asked for `url` somewhere else: a tab beside this one
    /// that stays behind it (a middle- or Ctrl-click on a link), a new
    /// window (Shift-click), or a tab in front (anything else,
    /// `window.open` included).
    fn open_elsewhere(&self, url: Option<&CefString>, disposition: WindowOpenDisposition) {
        let Some(url) = url.and_then(|u| Url::parse(&u.to_string()).ok()) else {
            return;
        };
        let Some(opener) = self.tab.get() else {
            log::debug!("a popup not yet adopted asked for {url}; ignored");
            return;
        };
        match popup_plan(disposition) {
            PopupPlan::Window => self
                .delegate
                .push(EngineEvent::NewWindowRequested { opener, url }),
            PopupPlan::Tab { background } => self.delegate.request_tab(opener, url, background),
        }
    }
}

/// The settings of a tab's windowless browser.
pub(crate) fn apply_tab_settings(settings: &mut cef::BrowserSettings) {
    settings.windowless_frame_rate = 60;
    // Opaque: the frame is uploaded without blending.
    settings.background_color = 0xFFFF_FFFF;
}

/// Build the client for one tab.
pub(crate) fn client_for(view: Rc<ViewState>) -> Client {
    let dialog = view.delegate.dialog_handler();
    TabClient::new(Rc::new(TabHandlers {
        render: TabRenderHandler::new(view.clone()),
        life_span: TabLifeSpanHandler::new(view.clone()),
        load: TabLoadHandler::new(view.clone()),
        display: TabDisplayHandler::new(view.clone()),
        request: TabRequestHandler::new(view.clone()),
        context_menu: TabContextMenuHandler::new(view.clone()),
        download: TabDownloadHandler::new(view.clone()),
        find: TabFindHandler::new(view.clone()),
        permission: TabPermissionHandler::new(view.clone()),
        keyboard: TabKeyboardHandler::new(),
        dialog,
        view,
    }))
}

/// A tab's handlers, which share its [`ViewState`].
struct TabHandlers {
    render: RenderHandler,
    life_span: LifeSpanHandler,
    load: LoadHandler,
    display: DisplayHandler,
    request: RequestHandler,
    context_menu: ContextMenuHandler,
    download: DownloadHandler,
    find: FindHandler,
    permission: PermissionHandler,
    keyboard: KeyboardHandler,
    dialog: DialogHandler,
    view: Rc<ViewState>,
}

/// Empty the folder a pending load left for the next file dialog.
pub(crate) fn clear_answer(slot: &RefCell<Option<PathBuf>>) {
    slot.borrow_mut().take();
}

#[cfg(test)]
mod dialog_answer_tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::clear_answer;

    #[test]
    fn a_load_that_never_opened_its_dialog_leaves_no_answer_for_the_next_one() {
        // A load queued a folder and failed before `loadUnpacked` opened
        // its dialog; the answer must not reach the next file dialog.
        let slot = RefCell::new(Some(PathBuf::from("/extensions/left-over")));
        clear_answer(&slot);
        assert_eq!(*slot.borrow(), None);
        // Clearing an empty slot is harmless.
        clear_answer(&slot);
        assert_eq!(*slot.borrow(), None);
    }
}

wrap_dialog_handler! {
    struct AnswerDialog {
        delegate: Rc<DelegateState>,
    }

    impl DialogHandler {
        #[allow(clippy::too_many_arguments)]
        fn on_file_dialog(
            &self,
            _browser: Option<&mut Browser>,
            mode: FileDialogMode,
            _title: Option<&CefString>,
            _default_file_path: Option<&CefString>,
            _accept_filters: Option<&mut CefStringList>,
            _accept_extensions: Option<&mut CefStringList>,
            _accept_descriptions: Option<&mut CefStringList>,
            callback: Option<&mut FileDialogCallback>,
        ) -> ::std::os::raw::c_int {
            let answer = self.delegate.dialog_answer.borrow_mut().take();
            log::debug!("file dialog {mode:?}; answer {answer:?}");
            let (Some(dir), Some(callback)) = (answer, callback) else {
                return 0;
            };
            let mut list = CefStringList::new();
            list.append(&dir.to_string_lossy());
            callback.cont(Some(&mut list));
            1
        }
    }
}

wrap_client! {
    struct TabClient {
        handlers: Rc<TabHandlers>,
    }

    impl Client {
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.handlers.dialog.clone())
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.handlers.context_menu.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.handlers.download.clone())
        }

        fn find_handler(&self) -> Option<FindHandler> {
            Some(self.handlers.find.clone())
        }

        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            Some(self.handlers.keyboard.clone())
        }

        fn permission_handler(&self) -> Option<PermissionHandler> {
            Some(self.handlers.permission.clone())
        }

        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.handlers.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.handlers.life_span.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.handlers.load.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.handlers.display.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.handlers.request.clone())
        }

        /// The store shim's request (`store.rs`).
        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            let (Some(frame), Some(message)) = (frame, message) else {
                return 0;
            };
            // A bridge call from an extension page shown as this tab
            // (bridge/pages.rs), or the store shim's request (store.rs).
            #[cfg(any(windows, target_os = "macos"))]
            {
                let delegate = &self.handlers.view.delegate;
                if let (Some(lane), Some(browser)) = (delegate.page_lane(), browser.as_deref())
                    && lane.receive(delegate, browser, frame, message)
                {
                    return 1;
                }
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            let _ = browser;
            let Some(request) = crate::store::request_from_message(frame, message) else {
                return 0;
            };
            self.handlers.view.delegate.store_request(request);
            1
        }
    }
}

/// What a media request asks for, as the model's request: the
/// microphone and the camera of `getUserMedia`. `None` for anything
/// else (capturing the screen or its sound, or nothing at all), which
/// is left to Chromium's default, a refusal.
pub(crate) fn media_request(id: u64, origin: &str, permissions: u32) -> Option<MediaRequest> {
    // The enum itself, not the binding's `get_raw`: that returns the
    // enum's C type, `u32` on macOS and `i32` on Windows, and the
    // handler's bits are `u32` on both.
    use cef::sys::cef_media_access_permission_types_t as Permission;
    let microphone = Permission::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32;
    let camera = Permission::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32;
    if permissions == 0 || permissions & !(microphone | camera) != 0 {
        return None;
    }
    Some(MediaRequest {
        id,
        origin: origin.to_string(),
        microphone: permissions & microphone != 0,
        camera: permissions & camera != 0,
    })
}

wrap_permission_handler! {
    struct TabPermissionHandler {
        view: Rc<ViewState>,
    }

    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut CefFrame>,
            requesting_origin: Option<&CefString>,
            requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(callback) = callback else {
                return 0;
            };
            let origin = requesting_origin
                .map(CefString::to_string)
                .unwrap_or_default();
            let delegate = &self.view.delegate;
            let id = delegate.next_media_request.get();
            let Some(request) = media_request(id, &origin, requested_permissions) else {
                log::info!(
                    "refused media request of {origin} (permissions {requested_permissions:#x})"
                );
                return 0;
            };
            delegate.next_media_request.set(id + 1);
            log::info!(
                "media request {id}: {origin} asks for the {}",
                request.devices()
            );
            self.view
                .media_requests
                .borrow_mut()
                .insert(id, (callback.clone(), requested_permissions));
            self.view
                .report(move |tab| EngineEvent::MediaAccessRequested { tab, request });
            1
        }
    }
}

wrap_render_handler! {
    struct TabRenderHandler {
        view: Rc<ViewState>,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let (width, height) = self.view.dip_size.get();
                rect.x = 0;
                rect.y = 0;
                rect.width = width.max(1);
                rect.height = height.max(1);
            }
        }

        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> ::std::os::raw::c_int {
            let Some(screen_info) = screen_info else {
                return 0;
            };
            screen_info.device_scale_factor = self.view.scale.get();
            screen_info.depth = 24;
            screen_info.depth_per_component = 8;
            screen_info.is_monochrome = 0;
            let (width, height) = self.view.dip_size.get();
            let rect = Rect {
                x: 0,
                y: 0,
                width: width.max(1),
                height: height.max(1),
            };
            screen_info.rect = rect.clone();
            screen_info.available_rect = rect;
            1
        }

        fn screen_point(
            &self,
            _browser: Option<&mut Browser>,
            view_x: ::std::os::raw::c_int,
            view_y: ::std::os::raw::c_int,
            screen_x: Option<&mut ::std::os::raw::c_int>,
            screen_y: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            let (origin_x, origin_y) = self.view.origin.get();
            if let Some(x) = screen_x {
                *x = origin_x + view_x;
            }
            if let Some(y) = screen_y {
                *y = origin_y + view_y;
            }
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: ::std::os::raw::c_int) {
            self.view.frame.borrow_mut().show_popup(show != 0);
            self.view.report(EngineEvent::FrameReady);
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(rect) = rect {
                let scale = self.view.scale.get();
                self.view.frame.borrow_mut().set_popup_rect(
                    (rect.x as f32 * scale).round() as i32,
                    (rect.y as f32 * scale).round() as i32,
                );
            }
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: ::std::os::raw::c_int,
            height: ::std::os::raw::c_int,
        ) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let len = width as usize * height as usize * 4;
            // SAFETY: CEF hands a `width * height` BGRA buffer that stays
            // valid for the duration of this callback.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, len) };
            let mut frame = self.view.frame.borrow_mut();
            if type_ == PaintElementType::POPUP {
                frame.paint_popup(width as u32, height as u32, pixels);
            } else {
                let rects: Vec<PixelRect> = dirty_rects
                    .unwrap_or_default()
                    .iter()
                    .filter_map(pixel_rect)
                    .collect();
                frame.paint_view(width as u32, height as u32, pixels, &rects);
            }
            drop(frame);
            self.view.report(EngineEvent::FrameReady);
        }

        // The page started a drag (`dragstart` on a draggable element,
        // a selection, a link, an image). Answering "not handled"
        // makes CEF end the drag at once without `dragend`, which
        // leaves the page in whatever state `dragstart` put it in
        // (an element faded, hidden, or with `pointer-events: none`,
        // until a `dragend` that never comes). So the drag is handled:
        // kept as a session the host carries out with the pointer
        // events it gets anyway (`DragSession`).
        fn start_dragging(
            &self,
            _browser: Option<&mut Browser>,
            drag_data: Option<&mut DragData>,
            allowed_ops: DragOperationsMask,
            x: ::std::os::raw::c_int,
            y: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let Some(lent) = drag_data else {
                return 0;
            };
            let data = ImplDragData::clone(lent).unwrap_or_else(|| Clone::clone(lent));
            let scale = self.view.scale.get();
            let to_px = |v: i32| (v as f32 * scale).round() as i32;
            let hotspot = data.image_hotspot();
            let hotspot = (to_px(hotspot.x), to_px(hotspot.y));
            let image = (data.has_image() != 0)
                .then(|| data.image())
                .flatten()
                .and_then(|mut image| drag_image_from(&mut image, scale));
            {
                let mut frame = self.view.frame.borrow_mut();
                match image {
                    Some((width, height, bgra)) => {
                        frame.set_drag_image(width, height, &bgra);
                        frame.set_drag_rect(to_px(x) - hotspot.0, to_px(y) - hotspot.1);
                    }
                    None => frame.hide_drag_image(),
                }
            }
            *self.view.drag.borrow_mut() = Some(DragSession {
                data,
                allowed: allowed_ops,
                entered: false,
                operation: DragOperationsMask::default(),
                hotspot,
            });
            log::debug!("tab {}: drag started at ({x}, {y}), allowed {allowed_ops:?}", self.view.label());
            self.view.report(EngineEvent::FrameReady);
            1
        }

        // What the page under the pointer would do with a drop (its
        // `dropEffect`), which is the cursor the drag shows.
        fn update_drag_cursor(&self, _browser: Option<&mut Browser>, operation: DragOperationsMask) {
            if let Some(session) = self.view.drag.borrow_mut().as_mut() {
                session.operation = operation;
            }
            self.view.report(move |tab| EngineEvent::CursorChanged {
                tab,
                cursor: drag_cursor(operation),
            });
        }
    }
}

/// The cursor for a drag over somewhere that would do `operation` with
/// a drop: none means no drop here.
pub(crate) fn drag_cursor(operation: DragOperationsMask) -> Cursor {
    if operation.as_ref().0 == cef::sys::cef_drag_operations_mask_t::DRAG_OPERATION_NONE.0 {
        Cursor::NotAllowed
    } else {
        Cursor::Default
    }
}

/// Chromium's drag image at the view's scale as premultiplied BGRA, with
/// its size in pixels.
fn drag_image_from(image: &mut Image, scale: f32) -> Option<(u32, u32, Vec<u8>)> {
    if image.is_empty() != 0 {
        return None;
    }
    let (mut width, mut height) = (0, 0);
    let bitmap = image.as_bitmap(
        scale,
        cef::ColorType::BGRA_8888,
        cef::AlphaType::PREMULTIPLIED,
        Some(&mut width),
        Some(&mut height),
    )?;
    let size = bitmap.size();
    let mut bgra = vec![0u8; size];
    let copied = bitmap.data(Some(&mut bgra), 0);
    if copied != size || width <= 0 || height <= 0 || size != width as usize * height as usize * 4 {
        log::warn!("drag image bitmap has unexpected size");
        return None;
    }
    Some((width as u32, height as u32, bgra))
}

wrap_life_span_handler! {
    struct TabLifeSpanHandler {
        view: Rc<ViewState>,
    }

    impl LifeSpanHandler {
        #[allow(clippy::too_many_arguments)]
        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut CefFrame>,
            _popup_id: ::std::os::raw::c_int,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&cef::PopupFeatures>,
            window_info: Option<&mut cef::WindowInfo>,
            client: Option<&mut Option<Client>>,
            settings: Option<&mut cef::BrowserSettings>,
            _extra_info: Option<&mut Option<cef::DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // Chromium creates the popup itself, as one more windowless
            // browser, so the page and its popup keep their handles to
            // each other: `window.open` returns the window,
            // `window.opener` is set, and a later `window.open` or form
            // with the same target name finds it again. A sign-in popup
            // answers its opener through exactly those. The model hears
            // of it as a new tab once the browser exists
            // (`on_after_created`).
            let background = match popup_plan(target_disposition) {
                PopupPlan::Tab { background } => background,
                PopupPlan::Window => {
                    self.view.open_elsewhere(target_url, target_disposition);
                    return 1;
                }
            };
            let (Some(opener), Some(window_info), Some(client)) =
                (self.view.tab(), window_info, client)
            else {
                // A popup of a popup the model has not adopted yet, or
                // nowhere to say how to create it: no popup.
                log::debug!("tab {}: a popup was refused", self.view.label());
                return 1;
            };
            let view = Rc::new(self.view.popup_of(opener, background));
            *window_info = cef::WindowInfo::default().set_as_windowless(self.view.parent);
            if let Some(settings) = settings {
                apply_tab_settings(settings);
            }
            *client = Some(client_for(view));
            0
        }

        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let delegate = &self.view.delegate;
            delegate.live_browsers.set(delegate.live_browsers.get() + 1);
            log::debug!("tab {}: browser created", self.view.label());
            if let Some(request) = self.view.popup.take()
                && let Some(browser) = browser
            {
                delegate.request_popup(
                    request.opener,
                    self.view.clone(),
                    browser.clone(),
                    request.background,
                );
            }
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            if !self.view.closing.get() {
                // The page closed itself (`window.close()`).
                self.view.report(EngineEvent::TabClosed);
            }
            // Let the close proceed; `on_before_close` follows.
            0
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let delegate = &self.view.delegate;
            delegate
                .live_browsers
                .set(delegate.live_browsers.get().saturating_sub(1));
            if let Some(browser) = browser {
                delegate.forget_browser(browser);
            }
        }
    }
}

wrap_load_handler! {
    struct TabLoadHandler {
        view: Rc<ViewState>,
    }

    impl LoadHandler {
        // A navigation to another site commits in a new renderer process,
        // and Chromium hands the new widget the view's focus only if the
        // view reports having it, which a windowless view never does. The
        // page then has no focus: no caret in its fields, grey selection,
        // `document.hasFocus()` false, until the window loses and regains
        // focus. Tell the new widget what the shell last told us.
        fn on_load_start(
            &self,
            browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            _transition_type: TransitionType,
        ) {
            let main_url = frame
                .as_deref()
                .filter(|f| f.is_main() != 0)
                .map(|f| text(f.url()));
            let main = main_url.is_some();
            if let Some(url) = &main_url {
                log::debug!("tab {}: load started at {url}", self.view.label());
            }
            if self.view.focused.get()
                && main
                && let Some(host) = browser.and_then(|b| b.host())
            {
                host.set_focus(1);
            }
        }

        fn on_loading_state_change(
            &self,
            browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            if is_loading == 0
                && let Ok(script) = std::env::var("NAUTILUS_TEST_PAGE_SCRIPT")
                && let Some(frame) = browser.and_then(|b| b.main_frame())
            {
                log::info!("TEST HOOK: running the page script");
                frame.execute_java_script(Some(&CefString::from(script.as_str())), None, 0);
            }
            log::debug!("tab {}: loading = {}", self.view.label(), is_loading != 0);
            self.view.report(move |tab| EngineEvent::LoadStateChanged {
                tab,
                state: if is_loading != 0 {
                    LoadState::Loading
                } else {
                    LoadState::Complete
                },
            });
            self.view.report(move |tab| EngineEvent::HistoryChanged {
                tab,
                can_go_back: can_go_back != 0,
                can_go_forward: can_go_forward != 0,
            });
        }

        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            // A cancelled load (a new navigation, Stop) is not an error.
            if error_code == Errorcode::ABORTED {
                return;
            }
            let Some(frame) = frame else {
                return;
            };
            if frame.is_main() == 0 {
                return;
            }
            let error_text = error_text.map(CefString::to_string).unwrap_or_default();
            let failed_url = failed_url.map(CefString::to_string).unwrap_or_default();
            let page = error_page(&failed_url, &error_text);
            let uri = format!(
                "data:text/html;base64,{}",
                crate::crx::base64(page.as_bytes())
            );
            frame.load_url(Some(&CefString::from(uri.as_str())));
        }
    }
}

/// The page shown for a failed load, in place of Chromium's own (which
/// belongs to the Chrome runtime style, not to windowless browsers).
fn error_page(failed_url: &str, error_text: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Page not available</title>\
         <style>body{{font-family:system-ui,sans-serif;margin:3em;color:#333}}\
         code{{word-break:break-all}}</style></head>\
         <body><h2>This page could not be loaded</h2>\
         <p><code>{}</code></p><p>{}</p></body></html>",
        escape_html(failed_url),
        escape_html(error_text)
    )
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

wrap_display_handler! {
    struct TabDisplayHandler {
        view: Rc<ViewState>,
    }

    impl DisplayHandler {
        fn on_address_change(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            url: Option<&CefString>,
        ) {
            if frame.is_some_and(|f| f.is_main() == 0) {
                return;
            }
            if let Some(url) = url.and_then(|u| Url::parse(&u.to_string()).ok()) {
                self.view.report(move |tab| EngineEvent::UrlChanged {
                    tab,
                    url,
                });
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(CefString::to_string).unwrap_or_default();
            log::debug!("tab {}: title {title:?}", self.view.label());
            self.view.report(move |tab| EngineEvent::TitleChanged {
                tab,
                title,
            });
        }

        fn on_favicon_urlchange(
            &self,
            browser: Option<&mut Browser>,
            icon_urls: Option<&mut CefStringList>,
        ) {
            let (Some(browser), Some(icon_urls)) = (browser, icon_urls) else {
                return;
            };
            let Some(host) = browser.host() else {
                return;
            };
            // The first listed icon; Chromium orders them by preference.
            let Some(first) = strings_of(icon_urls).into_iter().next() else {
                return;
            };
            let mut callback = FaviconCallback::new(self.view.clone());
            host.download_image(
                Some(&CefString::from(first.as_str())),
                1,
                64,
                0,
                Some(&mut callback),
            );
        }

        fn on_status_message(&self, _browser: Option<&mut Browser>, value: Option<&CefString>) {
            let text = value.map(CefString::to_string).unwrap_or_default();
            self.view
                .report(move |tab| EngineEvent::StatusChanged { tab, text });
        }

        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let level = match level {
                LogSeverity::ERROR | LogSeverity::FATAL => log::Level::Error,
                LogSeverity::WARNING => log::Level::Warn,
                LogSeverity::VERBOSE => log::Level::Debug,
                _ => log::Level::Info,
            };
            log::log!(
                target: "nautilus_engine::console",
                level,
                "{} ({}:{line})",
                message.map(CefString::to_string).unwrap_or_default(),
                source.map(CefString::to_string).unwrap_or_default()
            );
            1
        }

        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: NativeCursor,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> ::std::os::raw::c_int {
            let cursor = cursor_from(type_);
            self.view.cursor.set(cursor);
            self.view.report(move |tab| EngineEvent::CursorChanged {
                tab,
                cursor,
            });
            1
        }
    }
}

fn cursor_from(type_: CursorType) -> Cursor {
    match type_ {
        CursorType::HAND => Cursor::Pointer,
        CursorType::IBEAM | CursorType::VERTICALTEXT => Cursor::Text,
        CursorType::CROSS => Cursor::Crosshair,
        CursorType::WAIT => Cursor::Wait,
        CursorType::PROGRESS => Cursor::Progress,
        CursorType::HELP => Cursor::Help,
        CursorType::MOVE | CursorType::MIDDLEPANNING => Cursor::Move,
        CursorType::GRAB => Cursor::Grab,
        CursorType::GRABBING => Cursor::Grabbing,
        CursorType::NOTALLOWED | CursorType::NODROP => Cursor::NotAllowed,
        CursorType::COLUMNRESIZE => Cursor::ColResize,
        CursorType::ROWRESIZE => Cursor::RowResize,
        CursorType::NORTHRESIZE | CursorType::SOUTHRESIZE | CursorType::NORTHSOUTHRESIZE => {
            Cursor::NsResize
        }
        CursorType::EASTRESIZE | CursorType::WESTRESIZE | CursorType::EASTWESTRESIZE => {
            Cursor::EwResize
        }
        CursorType::NORTHEASTRESIZE
        | CursorType::SOUTHWESTRESIZE
        | CursorType::NORTHEASTSOUTHWESTRESIZE => Cursor::NeswResize,
        CursorType::NORTHWESTRESIZE
        | CursorType::SOUTHEASTRESIZE
        | CursorType::NORTHWESTSOUTHEASTRESIZE => Cursor::NwseResize,
        CursorType::ZOOMIN => Cursor::ZoomIn,
        CursorType::ZOOMOUT => Cursor::ZoomOut,
        CursorType::NONE => Cursor::Hidden,
        _ => Cursor::Default,
    }
}

wrap_request_handler! {
    struct TabRequestHandler {
        view: Rc<ViewState>,
    }

    impl RequestHandler {
        /// Every navigation of this tab, before any request goes out.
        /// A main-frame load of a blocked site is cancelled here and
        /// reported, and the model puts the block page up; the blank
        /// page it sits over has no site and passes.
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            request: Option<&mut Request>,
            _user_gesture: ::std::os::raw::c_int,
            is_redirect: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            let main_frame = frame.is_some_and(|f| f.is_main() != 0);
            let Some(request) = request else {
                return 0;
            };
            let url = text(request.url());
            let blocked =
                blocked_navigation(&self.view.delegate.blocked.borrow(), &url, main_frame);
            match blocked {
                Some(url) => {
                    log::info!(
                        "tab {}: blocked navigation to {url}{}",
                        self.view.label(),
                        if is_redirect != 0 { " (a redirect)" } else { "" }
                    );
                    self.view.report(move |tab| EngineEvent::NavigationBlocked {
                        tab,
                        url,
                    });
                    1
                }
                None => 0,
            }
        }

        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut CefFrame>,
            target_url: Option<&CefString>,
            target_disposition: WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            // Middle-click and Ctrl-click on links (a tab behind this
            // one), Shift-click (a window), Ctrl+Shift-click (a tab in
            // front).
            self.view.open_elsewhere(target_url, target_disposition);
            1
        }

        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            status: TerminationStatus,
            error_code: ::std::os::raw::c_int,
            error_string: Option<&CefString>,
        ) {
            let what = match status {
                TerminationStatus::ABNORMAL_TERMINATION => "ended abnormally",
                TerminationStatus::PROCESS_WAS_KILLED => "was killed",
                TerminationStatus::PROCESS_CRASHED => "crashed",
                TerminationStatus::PROCESS_OOM => "ran out of memory",
                TerminationStatus::LAUNCH_FAILED => "could not be started",
                TerminationStatus::INTEGRITY_FAILURE => "failed an integrity check",
                _ => "stopped",
            };
            let detail = error_string.map(CefString::to_string).unwrap_or_default();
            let reason = if detail.is_empty() {
                format!("The page's renderer process {what} (code {error_code}).")
            } else {
                format!("The page's renderer process {what}: {detail} (code {error_code}).")
            };
            log::warn!("tab {}: {reason}", self.view.label());
            log::error!("tab crashed: {reason}");
            self.view.report(move |tab| EngineEvent::TabCrashed {
                tab,
                reason,
            });
        }
    }
}

wrap_context_menu_handler! {
    struct TabContextMenuHandler {
        view: Rc<ViewState>,
    }

    impl ContextMenuHandler {
        /// The shell shows the page's context menu (osmena's, shaped
        /// like Chromium's) from `EngineEvent::ContextMenuRequested`;
        /// CEF's own menu is declined. What the user picks comes back as
        /// a `PageCommand`, which the frame's editing methods and the
        /// host's tools carry out, so CEF's callback is cancelled at
        /// once and only the point is kept.
        fn run_context_menu(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            params: Option<&mut ContextMenuParams>,
            _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> ::std::os::raw::c_int {
            let Some(params) = params else {
                return 0;
            };
            if let Some(callback) = callback {
                callback.cancel();
            }
            let (x, y) = (params.xcoord(), params.ycoord());
            *self.view.context_menu.borrow_mut() = Some(MenuPoint {
                frame: frame.map(|f| f.clone()),
                x,
                y,
            });
            let scale = self.view.scale.get();
            let context = page_context(params);
            self.view.report(move |tab| EngineEvent::ContextMenuRequested {
                tab,
                position: Some((x as f32 * scale, y as f32 * scale)),
                context,
            });
            1
        }
    }
}

/// What CEF found under the pointer, in the model's terms.
fn page_context(params: &ContextMenuParams) -> PageContext {
    use cef::sys::cef_context_menu_edit_state_flags_t as Edit;
    use cef::sys::cef_context_menu_media_state_flags_t as MediaFlag;
    use cef::sys::cef_context_menu_type_flags_t as Kind;

    let kinds = params.type_flags().as_ref().0;
    let is = |kind: Kind| kinds & kind.0 != 0;
    let link = is(Kind::CM_TYPEFLAG_LINK)
        .then(|| text(params.link_url()))
        .and_then(|url| Url::parse(&url).ok());
    let media = if is(Kind::CM_TYPEFLAG_MEDIA) {
        let media_type = params.media_type();
        let kind = if media_type == ContextMenuMediaType::IMAGE {
            Some(MediaKind::Image)
        } else if media_type == ContextMenuMediaType::VIDEO {
            Some(MediaKind::Video)
        } else if media_type == ContextMenuMediaType::AUDIO {
            Some(MediaKind::Audio)
        } else {
            None
        };
        let state = params.media_state_flags().as_ref().0;
        let has = |flag: MediaFlag| state & flag.0 != 0;
        kind.zip(Url::parse(&text(params.source_url())).ok())
            .map(|(kind, src)| {
                let toggles = kind != MediaKind::Image;
                Media {
                    kind,
                    src,
                    looping: (toggles && has(MediaFlag::CM_MEDIAFLAG_CAN_LOOP))
                        .then(|| has(MediaFlag::CM_MEDIAFLAG_LOOP)),
                    controls: (toggles && has(MediaFlag::CM_MEDIAFLAG_CAN_TOGGLE_CONTROLS))
                        .then(|| has(MediaFlag::CM_MEDIAFLAG_CONTROLS)),
                }
            })
    } else {
        None
    };
    let selection = is(Kind::CM_TYPEFLAG_SELECTION).then(|| text(params.selection_text()));
    let editable = (params.is_editable() != 0).then(|| {
        let flags = params.edit_state_flags().as_ref().0;
        let can = |flag: Edit| flags & flag.0 != 0;
        EditState {
            can_undo: can(Edit::CM_EDITFLAG_CAN_UNDO),
            can_redo: can(Edit::CM_EDITFLAG_CAN_REDO),
            can_cut: can(Edit::CM_EDITFLAG_CAN_CUT),
            can_copy: can(Edit::CM_EDITFLAG_CAN_COPY),
            can_paste: can(Edit::CM_EDITFLAG_CAN_PASTE),
            can_delete: can(Edit::CM_EDITFLAG_CAN_DELETE),
            can_select_all: can(Edit::CM_EDITFLAG_CAN_SELECT_ALL),
        }
    });
    let word = text(params.misspelled_word());
    let misspelling = (!word.is_empty()).then(|| {
        let mut suggestions = CefStringList::new();
        params.dictionary_suggestions(Some(&mut suggestions));
        Misspelling {
            word,
            suggestions: suggestions.into_iter().map(|s| s.to_string()).collect(),
        }
    });
    PageContext {
        link,
        media,
        selection,
        editable,
        misspelling,
    }
}

/// The navigation `on_before_browse` cancels: a main-frame load of a
/// URL on a blocked site. A subframe passes (nothing is blocked
/// inside a page), and so does anything that is no web page, since
/// `BlockedSites::site_for` gives nothing outside `http` and
/// `https`: `about:`, `data:`, `view-source:`, Chromium's own
/// `chrome:` and `devtools:` pages, extension pages, and text that
/// is no URL. Over plain text so tests need no CEF.
pub(crate) fn blocked_navigation(sites: &BlockedSites, url: &str, main_frame: bool) -> Option<Url> {
    if !main_frame {
        return None;
    }
    let url = Url::parse(url).ok()?;
    sites.blocks(&url)?;
    Some(url)
}

/// A string CEF hands back, as Rust text.
fn text(s: CefStringUserfree) -> String {
    CefString::from(&s).to_string()
}

// The keyboard handler's `os_event` is a Windows `MSG` there and a bare
// pointer (an `NSEvent`) on macOS; the signature must match the
// binding's exactly, so the handler is declared once per platform and
// the work is in `unhandled_key`.
#[cfg(not(target_os = "macos"))]
wrap_keyboard_handler! {
    struct TabKeyboardHandler;

    impl KeyboardHandler {
        fn on_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: Option<&mut cef::sys::MSG>,
        ) -> ::std::os::raw::c_int {
            unhandled_key(browser, event)
        }
    }
}

#[cfg(target_os = "macos")]
wrap_keyboard_handler! {
    struct TabKeyboardHandler;

    impl KeyboardHandler {
        fn on_key_event(
            &self,
            browser: Option<&mut Browser>,
            event: Option<&KeyEvent>,
            _os_event: *mut u8,
        ) -> ::std::os::raw::c_int {
            mac_unhandled_key(browser, event)
        }
    }
}

/// macOS: a key the page did not handle, which `unhandled_key` has had,
/// is answered as handled, always. CEF's next step for it
/// (`HandleKeyboardEvent` in its native platform delegate) offers it to
/// the application's menu bar with `performKeyEquivalent:`, but what it
/// offers is the synthetic `NSEvent` it made from the CEF key event this
/// crate sent, with no modifier flags at all for a plain key, and
/// AppKit's hidden items in every Edit menu, "Emoji & Symbols" (fn-E)
/// and "Start Dictation…" (fn-D), match such an E or D: typing into a
/// page opened the emoji palette, and the dictation prompt, every so
/// often (C7 in `docs/upstream-issues.md`). Nothing is lost by stopping
/// here: the menu bar already had the real event, since AppKit offers
/// every key-down to the menus before the window sees it (that is how
/// Ctrl+Cmd+Space still opens the palette).
#[cfg(target_os = "macos")]
fn mac_unhandled_key(
    browser: Option<&mut Browser>,
    event: Option<&KeyEvent>,
) -> ::std::os::raw::c_int {
    unhandled_key(browser, event);
    1
}

/// A key the page did not handle. On macOS the editing chords (Cmd+Z,
/// Shift+Cmd+Z, Cmd+X, Cmd+C, Cmd+V, Shift+Cmd+V, Cmd+A) are the
/// application's Edit menu's, not the renderer's: Blink leaves them to
/// the browser, which Chromium serves from its menu. osmena's menu bar
/// normally takes them before the page sees them; a chord that reaches
/// the page anyway is carried out here on the focused frame, after the
/// page had its chance (a page that handles the key itself is not
/// second-guessed). Windows and Linux renderers handle these chords
/// themselves, so nothing happens there.
fn unhandled_key(browser: Option<&mut Browser>, event: Option<&KeyEvent>) -> ::std::os::raw::c_int {
    let Some(event) = event else {
        return 0;
    };
    let Some(command) = mac_edit_command(event) else {
        return 0;
    };
    let Some(frame) = browser.and_then(|b| b.focused_frame()) else {
        return 0;
    };
    match command {
        EditCommand::Undo => frame.undo(),
        EditCommand::Redo => frame.redo(),
        EditCommand::Cut => frame.cut(),
        EditCommand::Copy => frame.copy(),
        EditCommand::Paste => frame.paste(),
        EditCommand::PastePlain => frame.paste_and_match_style(),
        EditCommand::SelectAll => frame.select_all(),
    }
    1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    PastePlain,
    SelectAll,
}

/// The macOS editing command a Cmd chord stands for, if it is one and
/// this is macOS (elsewhere the renderer handled it already, or it is
/// not a shortcut).
fn mac_edit_command(event: &KeyEvent) -> Option<EditCommand> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    if event.type_ != KeyEventType::RAWKEYDOWN && event.type_ != KeyEventType::KEYDOWN {
        return None;
    }
    edit_command(event.modifiers, event.windows_key_code)
}

/// The editing command for a Cmd chord: `modifiers` are CEF event
/// flags, `key` the Windows virtual-key code CEF derives on every
/// platform.
fn edit_command(modifiers: u32, key: i32) -> Option<EditCommand> {
    let command = modifiers & input::FLAG_COMMAND != 0;
    let other = modifiers & (input::FLAG_CONTROL | input::FLAG_ALT) != 0;
    if !command || other {
        return None;
    }
    let shift = modifiers & input::FLAG_SHIFT != 0;
    match (key, shift) {
        (0x5A, false) => Some(EditCommand::Undo),
        (0x5A, true) => Some(EditCommand::Redo),
        (0x58, false) => Some(EditCommand::Cut),
        (0x43, false) => Some(EditCommand::Copy),
        (0x56, false) => Some(EditCommand::Paste),
        (0x56, true) => Some(EditCommand::PastePlain),
        (0x41, false) => Some(EditCommand::SelectAll),
        _ => None,
    }
}

wrap_find_handler! {
    struct TabFindHandler {
        view: Rc<ViewState>,
    }

    impl FindHandler {
        /// Where the search stands: Chromium reports as it counts, the
        /// count growing, then once more with `final_update`.
        fn on_find_result(
            &self,
            _browser: Option<&mut Browser>,
            _identifier: ::std::os::raw::c_int,
            count: ::std::os::raw::c_int,
            _selection_rect: Option<&Rect>,
            active_match_ordinal: ::std::os::raw::c_int,
            final_update: ::std::os::raw::c_int,
        ) {
            self.view.report(move |tab| EngineEvent::FindResult {
                tab,
                current: u32::try_from(active_match_ordinal).unwrap_or(0),
                total: u32::try_from(count).unwrap_or(0),
                final_update: final_update != 0,
            });
        }
    }
}

wrap_download_handler! {
    struct TabDownloadHandler {
        view: Rc<ViewState>,
    }

    impl DownloadHandler {
        fn can_download(
            &self,
            _browser: Option<&mut Browser>,
            _url: Option<&CefString>,
            _request_method: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }

        /// Every download (a link to a file, "Save … as") goes to the
        /// directory the shell named, under its suggested name (made
        /// unique), or asks where to save when the shell said so (or
        /// said nothing yet).
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let suggested = suggested_name.map(CefString::to_string).unwrap_or_default();
            if let Some(item) = download_item {
                // What is being saved and why it is a download, for the log.
                log::info!(
                    "download {:?} ({}, {:?}) from {}",
                    suggested,
                    CefString::from(&item.mime_type()),
                    CefString::from(&item.content_disposition()).to_string(),
                    CefString::from(&item.url())
                );
            }
            let Some(callback) = callback else {
                return 0;
            };
            let policy = self.view.delegate.download_policy.borrow().clone();
            match policy {
                Some(policy) => {
                    let name = if suggested.is_empty() {
                        "download".to_string()
                    } else {
                        suggested
                    };
                    let path = unique_path(&policy.directory, &name);
                    callback.cont(
                        Some(&CefString::from(path.to_string_lossy().as_ref())),
                        i32::from(policy.ask),
                    );
                }
                None => callback.cont(None, 1),
            }
            1
        }

        /// Every change goes to the model as a report; the callback that
        /// cancels, pauses or resumes is kept while the download goes.
        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else {
                return;
            };
            let id = item.id();
            // The chosen path once known, else the suggested name, else
            // the URL's last segment.
            let last_segment = |text: String| {
                text.rsplit(['/', '\\'])
                    .find(|part| !part.is_empty())
                    .map(|part| part.split('?').next().unwrap_or(part).to_string())
                    .unwrap_or_default()
            };
            let full_path = text(item.full_path());
            let mut name = last_segment(full_path.clone());
            if name.is_empty() {
                name = text(item.suggested_file_name());
            }
            // The address the user asked for, not the signed one a
            // redirect landed on, so a retry works after it expires.
            let mut url_text = text(item.original_url());
            if url_text.is_empty() {
                url_text = text(item.url());
            }
            if name.is_empty() {
                name = last_segment(url_text.clone());
            }
            let state = if item.is_complete() != 0 {
                DownloadState::Complete
            } else if item.is_canceled() != 0 {
                DownloadState::Cancelled
            } else if item.is_interrupted() != 0 {
                DownloadState::Interrupted(interrupt_text(item.interrupt_reason()))
            } else if item.is_paused() != 0 {
                DownloadState::Paused
            } else {
                DownloadState::InProgress
            };
            {
                let mut downloads = self.view.delegate.downloads.borrow_mut();
                match (&state, callback) {
                    (DownloadState::InProgress | DownloadState::Paused, Some(callback)) => {
                        downloads.insert(id, callback.clone());
                    }
                    _ => {
                        downloads.remove(&id);
                    }
                }
            }
            let Ok(url) = Url::parse(&url_text) else {
                return;
            };
            let total = u64::try_from(item.total_bytes()).ok().filter(|t| *t > 0);
            self.view.push(EngineEvent::DownloadUpdated {
                id,
                report: DownloadReport {
                    name,
                    url,
                    path: (!full_path.is_empty()).then(|| PathBuf::from(full_path)),
                    received: u64::try_from(item.received_bytes()).unwrap_or(0),
                    total,
                    speed: u64::try_from(item.current_speed()).unwrap_or(0),
                    state,
                },
            });
        }
    }
}

/// `dir/name`, or `dir/name (2).ext` and so on when the name is taken.
fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (name.to_string(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some number is free")
}

/// Chromium's interrupt reason as a short phrase ("file access denied"):
/// the variant's name (`DownloadInterruptReason(CEF_DOWNLOAD_INTERRUPT_
/// REASON_FILE_ACCESS_DENIED)` in its `Debug` form) without the prefix.
fn interrupt_text(reason: cef::DownloadInterruptReason) -> String {
    let raw = format!("{reason:?}");
    let inner = raw
        .split_once('(')
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .unwrap_or(&raw);
    let name = inner.trim_start_matches("CEF_DOWNLOAD_INTERRUPT_REASON_");
    name.to_ascii_lowercase().replace('_', " ")
}

wrap_download_image_callback! {
    struct FaviconCallback {
        view: Rc<ViewState>,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            _image_url: Option<&CefString>,
            _http_status_code: ::std::os::raw::c_int,
            image: Option<&mut Image>,
        ) {
            let Some(image) = image else {
                return;
            };
            if let Some(favicon) = image_from(image) {
                self.view.report(move |tab| EngineEvent::FaviconChanged {
                    tab,
                    favicon,
                });
            }
        }
    }
}

// The image the user asked to copy ("Copy image"), once Chromium has
// it: the shell puts it on the clipboard.
wrap_download_image_callback! {
    pub(crate) struct ClipboardImageCallback {
        view: Rc<ViewState>,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            image_url: Option<&CefString>,
            _http_status_code: ::std::os::raw::c_int,
            image: Option<&mut Image>,
        ) {
            match image.and_then(image_from) {
                Some(image) => self.view.push(EngineEvent::ClipboardImage(image)),
                None => log::warn!(
                    "cannot copy the image at {}",
                    image_url.map(CefString::to_string).unwrap_or_default()
                ),
            }
        }
    }
}

/// The strings of a list CEF lent to a handler, read through the list's
/// raw handle. The obvious `list.clone().into_iter()` yields nothing:
/// the cef crate (154.5.0) clones a borrowed list by copying the
/// opaque struct behind its pointer, and the copy has no handle to
/// read from (`docs/upstream-issues.md`).
pub(crate) fn strings_of(list: &mut CefStringList) -> Vec<String> {
    let handle: *mut cef::sys::_cef_string_list_t = list.into();
    if handle.is_null() {
        return Vec::new();
    }
    // SAFETY: `handle` is the live list CEF passed to the handler, and
    // `value` is a zeroed string the list fills with its own copy,
    // which `cef_string_utf16_clear` releases once read.
    unsafe {
        let count = cef::sys::cef_string_list_size(handle);
        (0..count)
            .filter_map(|index| {
                let mut value: cef::sys::cef_string_utf16_t = std::mem::zeroed();
                if cef::sys::cef_string_list_value(handle, index, &mut value) == 0 {
                    return None;
                }
                let string = CefString::from(std::ptr::from_ref(&value)).to_string();
                cef::sys::cef_string_utf16_clear(&mut value);
                Some(string)
            })
            .collect()
    }
}

/// Chromium's image at scale 1 as straight RGBA8.
fn image_from(image: &mut Image) -> Option<RgbaImage> {
    if image.is_empty() != 0 {
        return None;
    }
    let (mut width, mut height) = (0, 0);
    let bitmap = image.as_bitmap(
        1.0,
        cef::ColorType::RGBA_8888,
        cef::AlphaType::POSTMULTIPLIED,
        Some(&mut width),
        Some(&mut height),
    )?;
    let size = bitmap.size();
    let mut rgba = vec![0u8; size];
    let copied = bitmap.data(Some(&mut rgba), 0);
    if copied != size || width <= 0 || height <= 0 || size != width as usize * height as usize * 4 {
        log::warn!("favicon bitmap has unexpected size");
        return None;
    }
    Some(RgbaImage {
        width: width as u32,
        height: height as u32,
        rgba: rgba.into(),
    })
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "macos"))]
    use super::strings_of;
    use super::{
        DelegateState, EditCommand, PopupPlan, ViewState, edit_command, escape_html, media_request,
        popup_plan,
    };
    use cef::WindowOpenDisposition;
    use osmena_core::{EngineEvent, TabId};
    use std::cell::Cell;
    use std::rc::Rc;

    fn view(delegate: &Rc<DelegateState>, tab: TabId) -> ViewState {
        // SAFETY: a window handle is a pointer (or a pointer-sized id) on
        // every platform, and all-zero is its "no window"; nothing here
        // hands it to Chromium.
        let parent = unsafe { std::mem::zeroed() };
        ViewState::new(
            tab,
            delegate.clone(),
            parent,
            (800, 600),
            2.0,
            Rc::new(Cell::new((10, 20))),
        )
    }

    #[test]
    fn a_popup_is_a_tab_chromium_creates_unless_it_asks_for_a_window() {
        assert_eq!(
            popup_plan(WindowOpenDisposition::NEW_POPUP),
            PopupPlan::Tab { background: false }
        );
        assert_eq!(
            popup_plan(WindowOpenDisposition::NEW_FOREGROUND_TAB),
            PopupPlan::Tab { background: false }
        );
        assert_eq!(
            popup_plan(WindowOpenDisposition::NEW_BACKGROUND_TAB),
            PopupPlan::Tab { background: true }
        );
        assert_eq!(
            popup_plan(WindowOpenDisposition::NEW_WINDOW),
            PopupPlan::Window
        );
    }

    #[test]
    fn a_tabs_view_reports_at_once() {
        let delegate = Rc::new(DelegateState::default());
        let view = view(&delegate, TabId(4));
        view.report(EngineEvent::FrameReady);
        assert_eq!(delegate.drain(), [EngineEvent::FrameReady(TabId(4))]);
    }

    #[test]
    fn a_popups_view_holds_its_events_until_it_is_adopted_as_a_tab() {
        let delegate = Rc::new(DelegateState::default());
        let opener = view(&delegate, TabId(4));
        let popup = opener.popup_of(TabId(4), false);
        assert_eq!(popup.tab(), None);
        assert_eq!(popup.dip_size.get(), (800, 600));
        assert!(Rc::ptr_eq(&popup.origin, &opener.origin));

        // What the popup's browser says before the model knows it.
        popup.report(|tab| EngineEvent::TitleChanged {
            tab,
            title: "Sign in".to_string(),
        });
        popup.report(EngineEvent::TabClosed);
        assert!(delegate.drain().is_empty(), "no tab to report them for yet");

        popup.adopt(TabId(9));
        assert_eq!(popup.tab(), Some(TabId(9)));
        assert_eq!(
            delegate.drain(),
            [
                EngineEvent::TitleChanged {
                    tab: TabId(9),
                    title: "Sign in".to_string(),
                },
                EngineEvent::TabClosed(TabId(9)),
            ]
        );
        popup.report(EngineEvent::FrameReady);
        assert_eq!(delegate.drain(), [EngineEvent::FrameReady(TabId(9))]);
    }
    #[cfg(not(target_os = "macos"))]
    use cef::CefStringList;

    /// Chromium's bits: 1 the microphone, 2 the camera, 4 and 8 the
    /// screen's sound and picture.
    #[test]
    fn a_media_request_is_the_microphone_and_the_camera_or_nothing() {
        let mic = media_request(3, "https://a.example", 1).unwrap();
        assert_eq!((mic.id, mic.origin.as_str()), (3, "https://a.example"));
        assert!(mic.microphone && !mic.camera);
        let camera = media_request(4, "https://a.example", 2).unwrap();
        assert!(!camera.microphone && camera.camera);
        let both = media_request(5, "https://a.example", 3).unwrap();
        assert!(both.microphone && both.camera);
        // Screen capture, alone or with a device, and an empty request
        // are not asked about: Chromium's default refuses them.
        for other in [0, 4, 8, 12, 5, 10] {
            assert_eq!(media_request(6, "https://a.example", other), None);
        }
    }

    /// Calls into libcef, which on macOS is loaded at run time from the
    /// app bundle (`LibraryLoader` in `engine.rs`): a test binary there
    /// has no library to call and crashes, so the test runs elsewhere.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_lent_string_list_is_read_through_its_handle() {
        let mut list = CefStringList::new();
        assert!(list.append("https://a.test/icon.png"));
        assert!(list.append("https://a.test/icon.svg"));
        assert_eq!(
            strings_of(&mut list),
            ["https://a.test/icon.png", "https://a.test/icon.svg"]
        );
        assert!(strings_of(&mut CefStringList::new()).is_empty());
    }
    use crate::input::{FLAG_ALT, FLAG_COMMAND, FLAG_CONTROL, FLAG_SHIFT};

    #[test]
    fn html_escaping_covers_the_four_characters() {
        assert_eq!(
            escape_html("<a href=\"x\">&</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn cmd_chords_name_the_edit_menu_commands() {
        assert_eq!(edit_command(FLAG_COMMAND, 0x43), Some(EditCommand::Copy));
        assert_eq!(edit_command(FLAG_COMMAND, 0x56), Some(EditCommand::Paste));
        assert_eq!(
            edit_command(FLAG_COMMAND | FLAG_SHIFT, 0x56),
            Some(EditCommand::PastePlain)
        );
        assert_eq!(edit_command(FLAG_COMMAND, 0x58), Some(EditCommand::Cut));
        assert_eq!(
            edit_command(FLAG_COMMAND, 0x41),
            Some(EditCommand::SelectAll)
        );
        assert_eq!(edit_command(FLAG_COMMAND, 0x5A), Some(EditCommand::Undo));
        assert_eq!(
            edit_command(FLAG_COMMAND | FLAG_SHIFT, 0x5A),
            Some(EditCommand::Redo)
        );
    }

    #[test]
    fn other_chords_are_left_alone() {
        // Ctrl+C is the renderer's own on Windows and Linux.
        assert_eq!(edit_command(FLAG_CONTROL, 0x43), None);
        // Cmd+Option+C and Cmd+Ctrl+C mean something else.
        assert_eq!(edit_command(FLAG_COMMAND | FLAG_ALT, 0x43), None);
        assert_eq!(edit_command(FLAG_COMMAND | FLAG_CONTROL, 0x43), None);
        // Shift+Cmd+C is not a shortcut here.
        assert_eq!(edit_command(FLAG_COMMAND | FLAG_SHIFT, 0x43), None);
        assert_eq!(edit_command(FLAG_COMMAND, 0x42), None);
        assert_eq!(edit_command(0, 0x43), None);
    }

    use super::blocked_navigation;
    use osmena_core::BlockedSites;

    fn blocking(site: &str) -> BlockedSites {
        let mut b = BlockedSites::default();
        b.add(site);
        b
    }

    #[test]
    fn a_main_frame_load_of_a_blocked_site_or_its_subdomain_is_cancelled() {
        let b = blocking("example.com");
        let hit = blocked_navigation(&b, "https://example.com/page", true).unwrap();
        assert_eq!(hit.as_str(), "https://example.com/page");
        assert!(blocked_navigation(&b, "https://www.example.com/", true).is_some());
        assert!(blocked_navigation(&b, "http://m.example.com:8080/x?y", true).is_some());
        assert!(blocked_navigation(&b, "https://notexample.com/", true).is_none());
        assert!(blocked_navigation(&b, "https://example.org/", true).is_none());
    }

    #[test]
    fn subframes_and_pages_without_a_site_pass() {
        let mut b = blocking("example.com");
        assert!(blocked_navigation(&b, "https://example.com/", false).is_none());
        // Even with the engine pages' "hosts" in the list, they pass:
        // only web pages have sites.
        b.add("extensions");
        b.add("devtools");
        b.add("abcdefghijklmnop");
        for url in [
            "about:blank",
            "data:text/html,<a href='https://example.com/'>x</a>",
            "view-source:https://example.com/",
            "chrome://extensions/",
            "devtools://devtools/bundled/inspector.html",
            "chrome-extension://abcdefghijklmnop/popup.html",
            "not a url",
        ] {
            assert!(blocked_navigation(&b, url, true).is_none(), "{url}");
        }
    }
}
