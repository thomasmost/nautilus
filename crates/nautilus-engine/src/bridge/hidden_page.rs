//! A hidden page: a windowless browser nobody paints, in which the engine
//! evaluates scripts (each bridged extension's relay page, and the helper
//! extension's page). Its CEF handlers report to the `Bridge` that owns it,
//! each naming the page by its key and generation.

use std::cell::Cell;
use std::rc::Rc;

use cef::rc::Rc as _;
use cef::{
    Browser, BrowserSettings, CefString, Client, DevToolsMessageObserver, DialogHandler,
    DisplayHandler, Frame, ImplBrowser, ImplBrowserHost, ImplClient, ImplDevToolsMessageObserver,
    ImplDisplayHandler, ImplFrame, ImplLifeSpanHandler, ImplLoadHandler, ImplRenderHandler,
    ImplRequestHandler, LifeSpanHandler, LoadHandler, LogSeverity, Rect, Registration,
    RenderHandler, RequestHandler, TerminationStatus, WindowInfo, WrapClient,
    WrapDevToolsMessageObserver, WrapDisplayHandler, WrapLifeSpanHandler, WrapLoadHandler,
    WrapRenderHandler, WrapRequestHandler, wrap_client, wrap_dev_tools_message_observer,
    wrap_display_handler, wrap_life_span_handler, wrap_load_handler, wrap_render_handler,
    wrap_request_handler,
};

use super::relay::Bridge;
use crate::delegate::DelegateState;

#[cfg(target_os = "macos")]
fn no_window() -> cef::sys::cef_window_handle_t {
    std::ptr::null_mut()
}

#[cfg(windows)]
fn no_window() -> cef::sys::cef_window_handle_t {
    cef::sys::HWND(std::ptr::null_mut())
}

/// A page in a windowless browser that is never shown.
pub(super) struct HiddenPage {
    pub(super) browser: Browser,
    /// Tells this page from an earlier one under the same key: a callback
    /// from a page that is gone must not change its successor.
    pub(super) generation: u64,
    /// The DevTools observer stays registered while this lives.
    _devtools: Option<Registration>,
    /// Its main frame finished loading.
    pub(super) ready: Cell<bool>,
}

impl HiddenPage {
    pub(super) fn open(
        bridge: &Rc<Bridge>,
        key: &str,
        url: &str,
        generation: u64,
    ) -> Option<HiddenPage> {
        let mut client = PageClient::new(
            PageRender::new(),
            PageLife::new(
                bridge.delegate.clone(),
                bridge.clone(),
                key.to_string(),
                generation,
            ),
            PageLoad::new(bridge.clone(), key.to_string(), generation),
            PageDisplay::new(key.to_string()),
            bridge.delegate.dialog_handler(),
            PageRequest::new(bridge.clone(), key.to_string(), generation),
        );
        let window_info = WindowInfo::default().set_as_windowless(no_window());
        let settings = BrowserSettings {
            windowless_frame_rate: 1,
            ..BrowserSettings::default()
        };
        let mut context = bridge.delegate.request_context();
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&settings),
            None,
            context.as_mut(),
        )?;
        let mut observer = PageDevTools::new(bridge.clone(), key.to_string(), generation);
        let devtools = browser
            .host()
            .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));
        log::debug!("bridge: page {key} opening at {url}");
        // A view nobody sees is captured like any other, and counts
        // against a popup widget's painting (C15).
        crate::popup_pause::track_page(&browser);
        Some(HiddenPage {
            browser,
            generation,
            _devtools: devtools,
            ready: Cell::new(false),
        })
    }

    /// Evaluate `expression` (a promise is awaited, the value returned
    /// by value) as DevTools message `id`; the id the answer will carry.
    pub(super) fn evaluate(browser: &Browser, id: i32, expression: &str) -> Option<i32> {
        // CEF keeps `id` when it is not below this browser's next id, and
        // otherwise uses that next id (`Bridge::evaluate` has the rule).
        crate::devtools::evaluate(&browser.host()?, id, expression)
    }

    pub(super) fn close(&self) {
        if let Some(host) = self.browser.host() {
            host.close_browser(1);
        }
    }
}

impl Drop for HiddenPage {
    fn drop(&mut self) {
        crate::popup_pause::forget_page(self.browser.identifier());
    }
}

wrap_client! {
    struct PageClient {
        render: RenderHandler,
        life: LifeSpanHandler,
        load: LoadHandler,
        display: DisplayHandler,
        dialog: DialogHandler,
        request: RequestHandler,
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

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.request.clone())
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
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl LifeSpanHandler {
        fn on_after_created(&self, _browser: Option<&mut Browser>) {
            self.delegate.add_live(1);
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            self.delegate.add_live(-1);
            // Closed by the engine, the page is already gone and this is
            // a no-op; closed underneath (an extension reload), it goes.
            self.bridge.page_gone(&self.key, self.generation, "it closed", false);
        }
    }
}

// A renderer that terminates takes the page's script with it; the browser
// itself stays until it is closed, so the page is closed too.
wrap_request_handler! {
    struct PageRequest {
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl RequestHandler {
        fn on_render_process_terminated(
            &self,
            _browser: Option<&mut Browser>,
            _status: TerminationStatus,
            _error_code: ::std::os::raw::c_int,
            _error_string: Option<&CefString>,
        ) {
            self.bridge
                .page_gone(&self.key, self.generation, "its renderer terminated", true);
        }
    }
}

wrap_load_handler! {
    struct PageLoad {
        bridge: Rc<Bridge>,
        key: String,
        generation: u64,
    }

    impl LoadHandler {
        fn on_load_end(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            _http_status_code: ::std::os::raw::c_int,
        ) {
            if frame.is_some_and(|f| f.is_main() != 0) {
                self.bridge.page_ready(&self.key, self.generation);
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
        generation: u64,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int,
            result: Option<&[u8]>,
        ) {
            self.bridge.page_result(
                &self.key,
                self.generation,
                message_id,
                success != 0,
                result.unwrap_or_default(),
            );
        }

        /// The page's DevTools session ended: the results still pending
        /// will never arrive (CEF's header says so), so they are dropped
        /// and what was waiting for them is asked again.
        fn on_dev_tools_agent_detached(&self, _browser: Option<&mut Browser>) {
            self.bridge.page_detached(&self.key, self.generation);
        }
    }
}
