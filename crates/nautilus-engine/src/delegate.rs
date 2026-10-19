//! CEF handler callbacks become [`EngineEvent`]s keyed by core tab ids.
//!
//! Every tab has its own `Client` whose handlers share that tab's
//! [`ViewState`], so a callback knows its tab without any id mapping. All
//! callbacks arrive on the main thread from inside `do_message_loop_work`
//! (CEF's UI thread is the thread that initialised it when the message
//! pump is external), so plain `RefCell`s suffice.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use cef::rc::Rc as _;
use cef::{
    Browser, CefString, CefStringList, Client, CursorInfo, CursorType, DisplayHandler,
    DownloadImageCallback, Errorcode, Frame as CefFrame, Image, ImplBinaryValue, ImplBrowser,
    ImplBrowserHost, ImplClient, ImplDisplayHandler, ImplDownloadImageCallback, ImplFrame,
    ImplImage, ImplLifeSpanHandler, ImplLoadHandler, ImplRenderHandler, ImplRequestHandler,
    LifeSpanHandler, LoadHandler, LogSeverity, PaintElementType, Rect, RenderHandler,
    RequestHandler, ScreenInfo, TerminationStatus, WrapClient, WrapDisplayHandler,
    WrapDownloadImageCallback, WrapLifeSpanHandler, WrapLoadHandler, WrapRenderHandler,
    WrapRequestHandler, wrap_client, wrap_display_handler, wrap_download_image_callback,
    wrap_life_span_handler, wrap_load_handler, wrap_render_handler, wrap_request_handler,
};
use nautilus_core::{Cursor, EngineEvent, Favicon, LoadState, PendingTab, TabId};
use url::Url;

use crate::frame::Frame;

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

    /// Record that `opener` asked for a new tab at `url` and report it.
    fn request_tab(&self, opener: TabId, url: Url) {
        let pending = PendingTab(self.next_pending.get());
        self.next_pending.set(pending.0 + 1);
        self.pending.borrow_mut().insert(pending, url);
        self.push(EngineEvent::NewTabRequested { opener, pending });
    }
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
}

impl ViewState {
    fn push(&self, event: EngineEvent) {
        self.delegate.push(event);
    }
}

/// Build the client for one tab.
pub(crate) fn client_for(view: Rc<ViewState>) -> Client {
    TabClient::new(
        TabRenderHandler::new(view.clone()),
        TabLifeSpanHandler::new(view.clone()),
        TabLoadHandler::new(view.clone()),
        TabDisplayHandler::new(view.clone()),
        TabRequestHandler::new(view),
    )
}

wrap_client! {
    struct TabClient {
        render: RenderHandler,
        life_span: LifeSpanHandler,
        load: LoadHandler,
        display: DisplayHandler,
        request: RequestHandler,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.render.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life_span.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.load.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.display.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
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
            _dirty_rects: Option<&[Rect]>,
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
                frame.paint_view(width as u32, height as u32, pixels);
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
            _target_disposition: cef::WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
            _popup_features: Option<&cef::PopupFeatures>,
            _window_info: Option<&mut cef::WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut cef::BrowserSettings>,
            _extra_info: Option<&mut Option<cef::DictionaryValue>>,
            _no_javascript_access: Option<&mut ::std::os::raw::c_int>,
        ) -> ::std::os::raw::c_int {
            // A popup becomes an ordinary tab at its URL, opened by the
            // model where it decides (`EngineAction::AdoptTab`). The page
            // gets no handle to it (`window.open` returns null).
            if let Some(url) = target_url.and_then(|u| Url::parse(&u.to_string()).ok()) {
                self.view.delegate.request_tab(self.view.tab, url);
            }
            1
        }

        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            let delegate = &self.view.delegate;
            delegate.live_browsers.set(delegate.live_browsers.get() + 1);
        }

        fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
            if !self.view.closing.get() {
                // The page closed itself (`window.close()`).
                self.view.push(EngineEvent::TabClosed(self.view.tab));
            }
            // Let the close proceed; `on_before_close` follows.
            0
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            let delegate = &self.view.delegate;
            delegate
                .live_browsers
                .set(delegate.live_browsers.get().saturating_sub(1));
        }
    }
}

wrap_load_handler! {
    struct TabLoadHandler {
        view: Rc<ViewState>,
    }

    impl LoadHandler {
        fn on_loading_state_change(
            &self,
            _browser: Option<&mut Browser>,
            is_loading: ::std::os::raw::c_int,
            can_go_back: ::std::os::raw::c_int,
            can_go_forward: ::std::os::raw::c_int,
        ) {
            let tab = self.view.tab;
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
            let uri = format!("data:text/html;base64,{}", base64(page.as_bytes()));
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

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
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
            self.view.push(EngineEvent::TitleChanged {
                tab: self.view.tab,
                title: title.map(CefString::to_string).unwrap_or_default(),
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
            // The first listed icon; Chromium orders them by preference. The
            // clone shares the borrowed list; iterating consumes the clone.
            let Some(first) = icon_urls.clone().into_iter().next() else {
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
            _cursor: cef::CursorHandle,
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
            _target_disposition: cef::WindowOpenDisposition,
            _user_gesture: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int {
            // Middle-click and Ctrl-click on links: a new tab at the URL.
            if let Some(url) = target_url.and_then(|u| Url::parse(&u.to_string()).ok()) {
                self.view.delegate.request_tab(self.view.tab, url);
            }
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
            log::error!("tab crashed: {reason}");
            self.view.push(EngineEvent::TabCrashed {
                tab: self.view.tab,
                reason,
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
            if let Some(favicon) = favicon_from(image) {
                self.view.push(EngineEvent::FaviconChanged {
                    tab: self.view.tab,
                    favicon,
                });
            }
        }
    }
}

/// Chromium's image at scale 1 as straight RGBA8.
fn favicon_from(image: &mut Image) -> Option<Favicon> {
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
    Some(Favicon {
        width: width as u32,
        height: height as u32,
        rgba: rgba.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::{base64, escape_html};

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn html_escaping_covers_the_four_characters() {
        assert_eq!(
            escape_html("<a href=\"x\">&</a>"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;"
        );
    }
}
