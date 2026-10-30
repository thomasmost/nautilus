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
    DownloadItemCallback, Errorcode, FileDialogCallback, FileDialogMode, FindHandler,
    Frame as CefFrame, Image, ImplBeforeDownloadCallback, ImplBinaryValue, ImplBrowser,
    ImplBrowserHost, ImplClient, ImplContextMenuHandler, ImplContextMenuParams, ImplDialogHandler,
    ImplDisplayHandler, ImplDownloadHandler, ImplDownloadImageCallback, ImplDownloadItem,
    ImplFileDialogCallback, ImplFindHandler, ImplFrame, ImplImage, ImplKeyboardHandler,
    ImplLifeSpanHandler, ImplLoadHandler, ImplRenderHandler, ImplRequestHandler,
    ImplRunContextMenuCallback, KeyEvent, KeyEventType, KeyboardHandler, LifeSpanHandler,
    LoadHandler, LogSeverity, MenuModel, PaintElementType, ProcessId, ProcessMessage, Rect,
    RenderHandler, RequestHandler, RunContextMenuCallback, ScreenInfo, TerminationStatus,
    TransitionType, WindowOpenDisposition, WrapClient, WrapContextMenuHandler, WrapDialogHandler,
    WrapDisplayHandler, WrapDownloadHandler, WrapDownloadImageCallback, WrapFindHandler,
    WrapKeyboardHandler, WrapLifeSpanHandler, WrapLoadHandler, WrapRenderHandler,
    WrapRequestHandler, wrap_client, wrap_context_menu_handler, wrap_dialog_handler,
    wrap_display_handler, wrap_download_handler, wrap_download_image_callback, wrap_find_handler,
    wrap_keyboard_handler, wrap_life_span_handler, wrap_load_handler, wrap_render_handler,
    wrap_request_handler,
};
use osmena_core::{
    Cursor, EditState, EngineEvent, ExtensionSource, LoadState, Media, MediaKind, Misspelling,
    PageContext, PendingTab, RgbaImage, TabId,
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
    /// Tabs page content asked for (`window.open`, middle-click), by the
    /// URL they should load, held until the model adopts them.
    pending: RefCell<HashMap<PendingTab, Url>>,
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
}

impl DelegateState {
    pub(crate) fn drain(&self) -> Vec<EngineEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    pub(crate) fn push(&self, event: EngineEvent) {
        self.events.borrow_mut().push_back(event);
    }

    pub(crate) fn take_pending(&self, pending: PendingTab) -> Option<Url> {
        self.pending.borrow_mut().remove(&pending)
    }

    pub(crate) fn live_browsers(&self) -> usize {
        self.live_browsers.get()
    }

    pub(crate) fn set_last_focused(&self, tab: TabId, browser: &Browser) {
        self.last_focused.set(Some(tab));
        *self.focused_browser.borrow_mut() = Some(browser.clone());
    }

    /// The browser of the last focused tab, for dialogs the user should
    /// see.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn focused_browser(&self) -> Option<Browser> {
        self.focused_browser.borrow().clone()
    }

    pub(crate) fn forget_browser(&self, browser: &Browser) {
        let mut focused = self.focused_browser.borrow_mut();
        if focused
            .as_ref()
            .is_some_and(|f| f.identifier() == browser.identifier())
        {
            *focused = None;
        }
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

    /// The dialog handler every client of this engine uses: a pending
    /// answer settles the dialog without showing it; otherwise CEF shows
    /// its own, owned by the browser's window.
    pub(crate) fn dialog_handler(self: &Rc<Self>) -> DialogHandler {
        AnswerDialog::new(self.clone())
    }

    /// A URL Chromium wants shown that has no opener tab (an extension
    /// asked for a tab): offer it to the last focused tab's window.
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
        let pending = PendingTab(self.next_pending.get());
        self.next_pending.set(pending.0 + 1);
        self.pending.borrow_mut().insert(pending, url);
        self.push(EngineEvent::NewTabRequested {
            opener,
            pending,
            background,
        });
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

/// What one tab's handlers need: the tab id, the geometry CEF asks about,
/// and the frame CEF paints into.
pub(crate) struct ViewState {
    pub tab: TabId,
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
}

impl ViewState {
    fn push(&self, event: EngineEvent) {
        self.delegate.push(event);
    }

    /// Page content asked for `url` somewhere else: a tab beside this one
    /// that stays behind it (a middle- or Ctrl-click on a link), a new
    /// window (Shift-click), or a tab in front (anything else,
    /// `window.open` included).
    fn open_elsewhere(&self, url: Option<&CefString>, disposition: WindowOpenDisposition) {
        let Some(url) = url.and_then(|u| Url::parse(&u.to_string()).ok()) else {
            return;
        };
        if disposition == WindowOpenDisposition::NEW_WINDOW {
            self.push(EngineEvent::NewWindowRequested {
                opener: self.tab,
                url,
            });
        } else {
            let background = disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB;
            self.delegate.request_tab(self.tab, url, background);
        }
    }
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
    keyboard: KeyboardHandler,
    dialog: DialogHandler,
    view: Rc<ViewState>,
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
            _browser: Option<&mut Browser>,
            frame: Option<&mut CefFrame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> ::std::os::raw::c_int {
            let (Some(frame), Some(message)) = (frame, message) else {
                return 0;
            };
            let Some(request) = crate::store::request_from_message(frame, message) else {
                return 0;
            };
            self.handlers.view.delegate.store_request(request);
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
            self.view.push(EngineEvent::FrameReady(self.view.tab));
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
            self.view.push(EngineEvent::FrameReady(self.view.tab));
        }
    }
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
            _window_info: Option<&mut cef::WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut cef::BrowserSettings>,
            _extra_info: Option<&mut Option<cef::DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // A popup becomes an ordinary tab (or window) at its URL,
            // opened by the model where it decides
            // (`EngineAction::AdoptTab`). The page gets no handle to it
            // (`window.open` returns null).
            self.view.open_elsewhere(target_url, target_disposition);
            1
        }

        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            let delegate = &self.view.delegate;
            delegate.live_browsers.set(delegate.live_browsers.get() + 1);
            log::debug!("tab {:?}: browser created", self.view.tab);
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            if !self.view.closing.get() {
                // The page closed itself (`window.close()`).
                self.view.push(EngineEvent::TabClosed(self.view.tab));
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
            let main = frame.is_some_and(|f| f.is_main() != 0);
            if main {
                log::debug!("tab {:?}: load started", self.view.tab);
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
            let tab = self.view.tab;
            log::debug!("tab {tab:?}: loading = {}", is_loading != 0);
            self.view.push(EngineEvent::LoadStateChanged {
                tab,
                state: if is_loading != 0 {
                    LoadState::Loading
                } else {
                    LoadState::Complete
                },
            });
            self.view.push(EngineEvent::HistoryChanged {
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
                self.view.push(EngineEvent::UrlChanged {
                    tab: self.view.tab,
                    url,
                });
            }
        }

        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(CefString::to_string).unwrap_or_default();
            log::debug!("tab {:?}: title {title:?}", self.view.tab);
            self.view.push(EngineEvent::TitleChanged {
                tab: self.view.tab,
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
            self.view.push(EngineEvent::StatusChanged {
                tab: self.view.tab,
                text: value.map(CefString::to_string).unwrap_or_default(),
            });
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
            self.view.push(EngineEvent::CursorChanged {
                tab: self.view.tab,
                cursor: cursor_from(type_),
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
            log::warn!("tab {:?}: {reason}", self.view.tab);
            log::error!("tab crashed: {reason}");
            self.view.push(EngineEvent::TabCrashed {
                tab: self.view.tab,
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
            self.view.push(EngineEvent::ContextMenuRequested {
                tab: self.view.tab,
                position: Some((x as f32 * scale, y as f32 * scale)),
                context: page_context(params),
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
            unhandled_key(browser, event)
        }
    }
}

/// A key the page did not handle. On macOS the editing chords (Cmd+Z,
/// Shift+Cmd+Z, Cmd+X, Cmd+C, Cmd+V, Shift+Cmd+V, Cmd+A) are the
/// application's Edit menu's, not the renderer's: Blink leaves them to
/// the browser, which Chromium serves from its menu. Nautilus has no
/// application menu, so they are carried out here on the focused frame,
/// after the page had its chance (a page that handles the key itself is
/// not second-guessed). Windows and Linux renderers handle these chords
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
            self.view.push(EngineEvent::FindResult {
                tab: self.view.tab,
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

        /// Every download (a link to a file, "Save … as") asks where to
        /// save, in the system's dialog, owned by the tab's window.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            if let Some(item) = download_item {
                // What is being saved and why it is a download, for the log.
                log::info!(
                    "download {:?} ({}, {:?}) from {}",
                    suggested_name.map(CefString::to_string).unwrap_or_default(),
                    CefString::from(&item.mime_type()),
                    CefString::from(&item.content_disposition()).to_string(),
                    CefString::from(&item.url())
                );
            }
            let Some(callback) = callback else {
                return 0;
            };
            callback.cont(None, 1);
            1
        }

        /// Progress and the outcome show in the status overlay.
        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            download_item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = download_item else {
                return;
            };
            // The chosen path once the dialog is answered, else the
            // suggested name, else the URL's last segment (a download
            // that fails before the dialog has neither).
            let last_segment = |text: String| {
                text.rsplit(['/', '\\'])
                    .find(|part| !part.is_empty())
                    .map(|part| part.split('?').next().unwrap_or(part).to_string())
                    .unwrap_or_default()
            };
            let mut name = last_segment(text(item.full_path()));
            if name.is_empty() {
                name = text(item.suggested_file_name());
            }
            if name.is_empty() {
                name = last_segment(text(item.url()));
            }
            let status = if item.is_complete() != 0 {
                format!("Saved {name}")
            } else if item.is_canceled() != 0 {
                return;
            } else if item.is_interrupted() != 0 {
                format!("Could not save {name}")
            } else if item.is_in_progress() != 0 {
                let percent = item.percent_complete();
                if percent >= 0 {
                    format!("Saving {name}\u{2026} {percent}%")
                } else {
                    format!("Saving {name}\u{2026}")
                }
            } else {
                return;
            };
            self.view.push(EngineEvent::StatusChanged {
                tab: self.view.tab,
                text: status,
            });
        }
    }
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
                self.view.push(EngineEvent::FaviconChanged {
                    tab: self.view.tab,
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
    use super::{EditCommand, edit_command, escape_html};
    #[cfg(not(target_os = "macos"))]
    use cef::CefStringList;

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
}
