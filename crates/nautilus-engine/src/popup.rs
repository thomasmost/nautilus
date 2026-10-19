//! The popup of an extension's toolbar action: a windowed (not
//! off-screen) Alloy-style browser in a borderless top-level window owned
//! by the browser window, placed under the toolbar button and sized to
//! its page. The page reports its size through a console message; the
//! popup closes on Escape, when the page closes itself, and when the
//! shell says so (the window took focus back, moved, or resized).
//!
//! Windows only, like the extension manager; elsewhere nothing opens.

#[cfg(windows)]
pub(crate) use windows::Popups;

#[cfg(not(windows))]
pub(crate) use stub::Popups;

#[cfg(not(windows))]
mod stub {
    use std::rc::Rc;

    use osmena_engine::ScreenRect;

    use crate::delegate::DelegateState;

    pub(crate) struct Popups;

    impl Popups {
        pub(crate) fn new(
            _delegate: Rc<DelegateState>,
            _owner: cef::sys::cef_window_handle_t,
        ) -> Popups {
            Popups
        }

        pub(crate) fn open(&self, _id: &str, _url: &str, _anchor: ScreenRect, _scale: f32) {
            log::warn!("extension popups are not available on this platform");
        }

        pub(crate) fn close_all(&self) {}
    }
}

#[cfg(windows)]
mod windows {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use cef::rc::Rc as _;
    use cef::{
        Browser, BrowserSettings, CefString, Client, DisplayHandler, Frame, ImplBrowser,
        ImplBrowserHost, ImplClient, ImplDisplayHandler, ImplFrame, ImplKeyboardHandler,
        ImplLifeSpanHandler, ImplLoadHandler, KeyEvent, KeyEventType, KeyboardHandler,
        LifeSpanHandler, LoadHandler, LogSeverity, Rect, RuntimeStyle, WindowInfo, WrapClient,
        WrapDisplayHandler, WrapKeyboardHandler, WrapLifeSpanHandler, WrapLoadHandler, wrap_client,
        wrap_display_handler, wrap_keyboard_handler, wrap_life_span_handler, wrap_load_handler,
    };
    use osmena_engine::ScreenRect;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
        WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    use crate::delegate::DelegateState;

    /// Chromium's limits for an action popup, in CSS pixels.
    const MIN_SIZE: f32 = 25.0;
    const MAX_WIDTH: f32 = 800.0;
    const MAX_HEIGHT: f32 = 600.0;
    /// Space between the button and the popup.
    const GAP: i32 = 2;
    /// Clicking the button while its popup is open closes it: the click
    /// first takes the focus back (which closes the popup) and then
    /// arrives as a request to open it, which is ignored this soon after.
    const REOPEN_DEBOUNCE: Duration = Duration::from_millis(400);

    const PREFIX: &str = "nautilus-popup-size:";

    /// Reports the page's preferred size, now and whenever it changes:
    /// the root's max-content width (what the content asks for, whatever
    /// the window is) and the body's bottom edge.
    const SIZE_SCRIPT: &str = r#"(function(){
  if (window.__nautilusSized) return;
  window.__nautilusSized = true;
  var html = document.documentElement, last = "";
  function measure(){
    var body = document.body;
    var prev = html.style.width;
    html.style.width = "max-content";
    var w = Math.ceil(html.getBoundingClientRect().width);
    html.style.width = prev;
    var h = 0;
    if (body) {
      var r = body.getBoundingClientRect();
      h = Math.ceil(r.bottom + (parseFloat(getComputedStyle(body).marginBottom) || 0));
    }
    if (!(h > 0)) h = Math.ceil(html.scrollHeight);
    var msg = w + "," + h;
    if (msg !== last) { last = msg; console.log("nautilus-popup-size:" + msg); }
  }
  measure();
  if (window.ResizeObserver) {
    var ro = new ResizeObserver(function(){ measure(); });
    ro.observe(html);
    if (document.body) ro.observe(document.body);
  }
  new MutationObserver(function(records){
    var ours = records.every(function(r){ return r.target === html && r.attributeName === "style"; });
    if (!ours) measure();
  }).observe(html, {subtree:true, childList:true, attributes:true, characterData:true});
})();"#;

    struct Open {
        id: String,
        browser: Browser,
        anchor: ScreenRect,
        scale: f32,
    }

    #[derive(Default)]
    struct Inner {
        open: Option<Open>,
        /// The popup that closed last, and when, for the reopen debounce.
        closed: Option<(String, Instant)>,
    }

    /// The popups of one browser window: at most one open at a time.
    pub(crate) struct Popups {
        delegate: Rc<DelegateState>,
        owner: cef::sys::cef_window_handle_t,
        inner: Rc<RefCell<Inner>>,
    }

    impl Popups {
        pub(crate) fn new(
            delegate: Rc<DelegateState>,
            owner: cef::sys::cef_window_handle_t,
        ) -> Popups {
            Popups {
                delegate,
                owner,
                inner: Rc::new(RefCell::new(Inner::default())),
            }
        }

        /// Open `url` as the popup of extension `id`, under `anchor`
        /// (physical screen pixels) at the window's `scale`.
        pub(crate) fn open(&self, id: &str, url: &str, anchor: ScreenRect, scale: f32) {
            {
                let inner = self.inner.borrow();
                if let Some((closed_id, at)) = &inner.closed
                    && closed_id == id
                    && at.elapsed() < REOPEN_DEBOUNCE
                {
                    return;
                }
            }
            self.close_all();
            // Tiny until the page says how big it wants to be; the
            // window is owned by the browser window, so it stays above it
            // and off the taskbar.
            let (width, height) = ((MIN_SIZE * scale) as i32, (MIN_SIZE * scale) as i32);
            let (x, y) = place(anchor, width, height);
            let window_info = WindowInfo {
                style: WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_VISIBLE,
                ex_style: WS_EX_TOOLWINDOW,
                parent_window: self.owner,
                bounds: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                windowless_rendering_enabled: 0,
                runtime_style: RuntimeStyle::ALLOY,
                ..WindowInfo::default()
            };
            let mut client = PopupClient::new(
                PopupLife::new(self.delegate.clone(), self.inner.clone(), id.to_string()),
                PopupLoad::new(),
                PopupDisplay::new(self.inner.clone()),
                PopupKeys::new(),
            );
            let settings = BrowserSettings {
                background_color: 0xFFFF_FFFF,
                ..BrowserSettings::default()
            };
            let browser = cef::browser_host_create_browser_sync(
                Some(&window_info),
                Some(&mut client),
                Some(&CefString::from(url)),
                Some(&settings),
                None,
                None,
            );
            let Some(browser) = browser else {
                log::error!("Chromium could not open the popup of extension {id}");
                return;
            };
            log::info!("opened the popup of extension {id} at {url}");
            if let Some(host) = browser.host() {
                host.set_focus(1);
            }
            self.inner.borrow_mut().open = Some(Open {
                id: id.to_string(),
                browser,
                anchor,
                scale,
            });
        }

        pub(crate) fn close_all(&self) {
            let open = self.inner.borrow_mut().open.take();
            if let Some(open) = open
                && let Some(host) = open.browser.host()
            {
                host.close_browser(1);
            }
        }
    }

    impl Drop for Popups {
        fn drop(&mut self) {
            self.close_all();
        }
    }

    /// Where a popup of `width` by `height` goes: under the anchor, its
    /// right edge on the anchor's right edge, never off the left of the
    /// screen.
    fn place(anchor: ScreenRect, width: i32, height: i32) -> (i32, i32) {
        let _ = height;
        let x = (anchor.x + anchor.width - width).max(0);
        let y = anchor.y + anchor.height + GAP;
        (x, y)
    }

    /// The page asked for `css_width` by `css_height`: resize the window.
    fn resize(open: &Open, css_width: f32, css_height: f32) {
        let width = (css_width.clamp(MIN_SIZE, MAX_WIDTH) * open.scale).round() as i32;
        let height = (css_height.clamp(MIN_SIZE, MAX_HEIGHT) * open.scale).round() as i32;
        let (x, y) = place(open.anchor, width, height);
        let Some(host) = open.browser.host() else {
            return;
        };
        let hwnd = host.window_handle().0;
        if hwnd.is_null() {
            return;
        }
        // SAFETY: the handle is the live window CEF created for this
        // browser; CEF handles the resize of its content from WM_SIZE.
        unsafe {
            SetWindowPos(
                hwnd.cast(),
                std::ptr::null_mut(),
                x,
                y,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    wrap_client! {
        struct PopupClient {
            life: LifeSpanHandler,
            load: LoadHandler,
            display: DisplayHandler,
            keys: KeyboardHandler,
        }

        impl Client {
            fn life_span_handler(&self) -> Option<LifeSpanHandler> {
                Some(self.life.clone())
            }

            fn load_handler(&self) -> Option<LoadHandler> {
                Some(self.load.clone())
            }

            fn display_handler(&self) -> Option<DisplayHandler> {
                Some(self.display.clone())
            }

            fn keyboard_handler(&self) -> Option<KeyboardHandler> {
                Some(self.keys.clone())
            }
        }
    }

    wrap_life_span_handler! {
        struct PopupLife {
            delegate: Rc<DelegateState>,
            inner: Rc<RefCell<Inner>>,
            id: String,
        }

        impl LifeSpanHandler {
            fn on_after_created(&self, _browser: Option<&mut Browser>) {
                self.delegate.add_live(1);
            }

            fn do_close(&self, _browser: Option<&mut Browser>) -> ::std::os::raw::c_int {
                // Ours or the page's `window.close()`: let it close.
                0
            }

            fn on_before_close(&self, _browser: Option<&mut Browser>) {
                self.delegate.add_live(-1);
                let mut inner = self.inner.borrow_mut();
                if inner.open.as_ref().is_some_and(|open| open.id == self.id) {
                    inner.open = None;
                }
                inner.closed = Some((self.id.clone(), Instant::now()));
            }
        }
    }

    wrap_load_handler! {
        struct PopupLoad;

        impl LoadHandler {
            fn on_load_end(
                &self,
                _browser: Option<&mut Browser>,
                frame: Option<&mut Frame>,
                _http_status_code: ::std::os::raw::c_int,
            ) {
                if let Some(frame) = frame
                    && frame.is_main() != 0
                {
                    frame.execute_java_script(Some(&CefString::from(SIZE_SCRIPT)), None, 0);
                }
            }
        }
    }

    wrap_display_handler! {
        struct PopupDisplay {
            inner: Rc<RefCell<Inner>>,
        }

        impl DisplayHandler {
            fn on_console_message(
                &self,
                _browser: Option<&mut Browser>,
                level: LogSeverity,
                message: Option<&CefString>,
                source: Option<&CefString>,
                line: ::std::os::raw::c_int,
            ) -> ::std::os::raw::c_int {
                let message = message.map(CefString::to_string).unwrap_or_default();
                if let Some(size) = message.strip_prefix(PREFIX) {
                    if let Some((w, h)) = size.split_once(',')
                        && let (Ok(w), Ok(h)) = (w.trim().parse::<f32>(), h.trim().parse::<f32>())
                        && let Some(open) = self.inner.borrow().open.as_ref()
                    {
                        resize(open, w, h);
                    }
                    return 1;
                }
                let level = match level {
                    LogSeverity::ERROR | LogSeverity::FATAL => log::Level::Error,
                    LogSeverity::WARNING => log::Level::Warn,
                    _ => log::Level::Debug,
                };
                log::log!(
                    target: "nautilus_engine::console",
                    level,
                    "{message} ({}:{line})",
                    source.map(CefString::to_string).unwrap_or_default()
                );
                1
            }
        }
    }

    wrap_keyboard_handler! {
        struct PopupKeys;

        impl KeyboardHandler {
            /// Escape closes the popup, as it does in other browsers.
            fn on_pre_key_event(
                &self,
                browser: Option<&mut Browser>,
                event: Option<&KeyEvent>,
                _os_event: Option<&mut cef::sys::MSG>,
                _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
            ) -> ::std::os::raw::c_int {
                const VK_ESCAPE: ::std::os::raw::c_int = 0x1B;
                let escape = event.is_some_and(|e| {
                    e.type_ == KeyEventType::RAWKEYDOWN && e.windows_key_code == VK_ESCAPE
                });
                if !escape {
                    return 0;
                }
                if let Some(host) = browser.and_then(|b| b.host()) {
                    host.close_browser(1);
                }
                1
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn popups_hang_under_the_anchor_right_aligned_and_on_screen() {
            let anchor = ScreenRect {
                x: 500,
                y: 40,
                width: 26,
                height: 26,
            };
            assert_eq!(place(anchor, 300, 200), (226, 68));
            let near_left = ScreenRect { x: 10, ..anchor };
            assert_eq!(place(near_left, 300, 200), (0, 68));
        }
    }
}
