//! The popup of an extension's toolbar action: a windowed (not
//! off-screen) Alloy-style browser in a borderless top-level window owned
//! by the browser window, placed under the toolbar button and sized to
//! its page. The page reports its size through a console message; the
//! popup closes on Escape, when the page closes itself, and when the
//! shell says so (the window took focus back, moved, or resized).
//!
//! The window is the platform's: a `WS_POPUP` window CEF creates on
//! Windows, a borderless child `NSWindow` of the browser window on macOS
//! (`platform`). Elsewhere nothing opens, like the extension manager.

#[cfg(any(windows, target_os = "macos"))]
pub(crate) use native::Popups;

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use stub::Popups;

#[cfg(not(any(windows, target_os = "macos")))]
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

#[cfg(any(windows, target_os = "macos"))]
mod native {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use cef::rc::Rc as _;
    use cef::{
        Browser, BrowserSettings, CefString, Client, DisplayHandler, Frame, ImplBrowser,
        ImplBrowserHost, ImplClient, ImplDisplayHandler, ImplFrame, ImplKeyboardHandler,
        ImplLifeSpanHandler, ImplLoadHandler, KeyEvent, KeyEventType, KeyboardHandler,
        LifeSpanHandler, LoadHandler, LogSeverity, WrapClient, WrapDisplayHandler,
        WrapKeyboardHandler, WrapLifeSpanHandler, WrapLoadHandler, wrap_client,
        wrap_display_handler, wrap_keyboard_handler, wrap_life_span_handler, wrap_load_handler,
    };
    use osmena_engine::ScreenRect;

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
        window: platform::PopupWindow,
    }

    #[derive(Default)]
    struct Inner {
        open: Option<Open>,
        /// Windows whose browser was told to close and has not reported
        /// `on_before_close` yet; each goes when its browser does.
        closing: Vec<(String, platform::PopupWindow)>,
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
            let size = (MIN_SIZE * scale) as i32;
            let bounds = place(anchor, size, size);
            let Some((window_info, window)) = platform::create(self.owner, bounds, scale) else {
                log::error!("could not make a window for the popup of extension {id}");
                return;
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
                window.close();
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
                window,
            });
        }

        pub(crate) fn close_all(&self) {
            let open = self.inner.borrow_mut().open.take();
            if let Some(open) = open {
                open.window.hide();
                self.inner
                    .borrow_mut()
                    .closing
                    .push((open.id.clone(), open.window));
                if let Some(host) = open.browser.host() {
                    host.close_browser(1);
                }
            }
        }
    }

    impl Drop for Popups {
        fn drop(&mut self) {
            self.close_all();
            let closing = std::mem::take(&mut self.inner.borrow_mut().closing);
            for (_, window) in closing {
                window.close();
            }
        }
    }

    /// Where a popup of `width` by `height` goes: under the anchor, its
    /// right edge on the anchor's right edge, never off the left of the
    /// screen.
    fn place(anchor: ScreenRect, width: i32, height: i32) -> ScreenRect {
        ScreenRect {
            x: (anchor.x + anchor.width - width).max(0),
            y: anchor.y + anchor.height + GAP,
            width,
            height,
        }
    }

    /// The page asked for `css_width` by `css_height`: resize the window.
    fn resize(open: &Open, css_width: f32, css_height: f32) {
        let width = (css_width.clamp(MIN_SIZE, MAX_WIDTH) * open.scale).round() as i32;
        let height = (css_height.clamp(MIN_SIZE, MAX_HEIGHT) * open.scale).round() as i32;
        let bounds = place(open.anchor, width, height);
        platform::resize(&open.window, &open.browser, bounds, open.scale);
    }

    /// Escape closes the popup, as it does in other browsers.
    fn escape_closes(browser: Option<&mut Browser>, event: Option<&KeyEvent>) -> i32 {
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
                if inner.open.as_ref().is_some_and(|open| open.id == self.id)
                    && let Some(open) = inner.open.take()
                {
                    open.window.close();
                }
                if let Some(at) = inner.closing.iter().position(|(id, _)| *id == self.id) {
                    let (_, window) = inner.closing.remove(at);
                    window.close();
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

    // The keyboard handler's `os_event` is a Windows `MSG` there and a
    // bare pointer (an `NSEvent`) on macOS; the signature must match the
    // binding's exactly, so the handler is declared once per platform.
    #[cfg(windows)]
    wrap_keyboard_handler! {
        struct PopupKeys;

        impl KeyboardHandler {
            fn on_pre_key_event(
                &self,
                browser: Option<&mut Browser>,
                event: Option<&KeyEvent>,
                _os_event: Option<&mut cef::sys::MSG>,
                _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
            ) -> ::std::os::raw::c_int {
                escape_closes(browser, event)
            }
        }
    }

    #[cfg(target_os = "macos")]
    wrap_keyboard_handler! {
        struct PopupKeys;

        impl KeyboardHandler {
            fn on_pre_key_event(
                &self,
                browser: Option<&mut Browser>,
                event: Option<&KeyEvent>,
                _os_event: *mut u8,
                _is_keyboard_shortcut: Option<&mut ::std::os::raw::c_int>,
            ) -> ::std::os::raw::c_int {
                escape_closes(browser, event)
            }
        }
    }

    /// Windows: CEF creates the popup as a `WS_POPUP` window owned by
    /// the browser window and destroys it with the browser; nothing is
    /// held here.
    #[cfg(windows)]
    mod platform {
        use cef::{Browser, ImplBrowser, ImplBrowserHost, Rect, RuntimeStyle, WindowInfo};
        use osmena_engine::ScreenRect;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
            WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
        };

        pub(super) struct PopupWindow;

        impl PopupWindow {
            pub(super) fn hide(&self) {}

            pub(super) fn close(self) {}
        }

        /// The `WindowInfo` for a popup at `bounds` (physical screen
        /// pixels), owned by the browser window `owner`.
        pub(super) fn create(
            owner: cef::sys::cef_window_handle_t,
            bounds: ScreenRect,
            _scale: f32,
        ) -> Option<(WindowInfo, PopupWindow)> {
            let window_info = WindowInfo {
                style: WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_VISIBLE,
                ex_style: WS_EX_TOOLWINDOW,
                parent_window: owner,
                bounds: Rect {
                    x: bounds.x,
                    y: bounds.y,
                    width: bounds.width,
                    height: bounds.height,
                },
                windowless_rendering_enabled: 0,
                runtime_style: RuntimeStyle::ALLOY,
                ..WindowInfo::default()
            };
            Some((window_info, PopupWindow))
        }

        /// Move the popup's window to `bounds` (physical screen pixels).
        pub(super) fn resize(
            _window: &PopupWindow,
            browser: &Browser,
            bounds: ScreenRect,
            _scale: f32,
        ) {
            let Some(host) = browser.host() else {
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
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    /// macOS: a borderless `NSWindow` of our own, a child of the browser
    /// window (so it moves with it and stays above it), holding CEF's
    /// browser view as its content. A plain borderless window refuses to
    /// become key, and the popup's page needs the keyboard, so the window
    /// is a subclass that accepts it.
    #[cfg(target_os = "macos")]
    mod platform {
        use cef::{Browser, Rect, RuntimeStyle, WindowInfo};
        use objc2::rc::Retained;
        use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
        use objc2_app_kit::{
            NSBackingStoreType, NSPopUpMenuWindowLevel, NSScreen, NSView, NSWindow,
            NSWindowOrderingMode, NSWindowStyleMask,
        };
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        use osmena_engine::ScreenRect;

        define_class!(
            // SAFETY: NSWindow has no subclassing requirements beyond
            // being used on the main thread, and the type has no Drop.
            #[unsafe(super(NSWindow))]
            #[thread_kind = MainThreadOnly]
            #[name = "NautilusPopupWindow"]
            struct KeyableWindow;

            impl KeyableWindow {
                #[unsafe(method(canBecomeKeyWindow))]
                fn can_become_key_window(&self) -> bool {
                    true
                }
            }
        );

        pub(super) struct PopupWindow {
            window: Retained<KeyableWindow>,
            owner: Retained<NSWindow>,
        }

        impl PopupWindow {
            pub(super) fn hide(&self) {
                self.window.orderOut(None);
            }

            pub(super) fn close(self) {
                self.owner.removeChildWindow(&self.window);
                self.window.orderOut(None);
                self.window.close();
            }
        }

        /// A window at `bounds` (physical screen pixels, from the top
        /// left) under the browser window whose content view is `owner`,
        /// and the `WindowInfo` that puts CEF's view into it.
        pub(super) fn create(
            owner: cef::sys::cef_window_handle_t,
            bounds: ScreenRect,
            scale: f32,
        ) -> Option<(WindowInfo, PopupWindow)> {
            let mtm = MainThreadMarker::new()?;
            if owner.is_null() {
                return None;
            }
            // SAFETY: the handle is the live content view of the browser
            // window, given to the tab host by the shell.
            let owner_view: &NSView = unsafe { &*owner.cast::<NSView>() };
            let owner_window = owner_view.window()?;
            let frame = screen_frame(bounds, scale, mtm);
            let window = KeyableWindow::alloc(mtm).set_ivars(());
            // SAFETY: a designated initialiser of NSWindow with plain
            // arguments; the window is released by its `Retained`, not
            // by `close` (see `setReleasedWhenClosed`).
            let window: Retained<KeyableWindow> = unsafe {
                msg_send![
                    super(window),
                    initWithContentRect: frame,
                    styleMask: NSWindowStyleMask::Borderless,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            // SAFETY: the window is ours and not yet shown.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setLevel(NSPopUpMenuWindowLevel);
            window.setHasShadow(true);
            window.setMovable(false);
            // SAFETY: both windows are live and on the main thread.
            unsafe { owner_window.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
            window.makeKeyAndOrderFront(None);
            let content = window.contentView()?;
            let size = content.bounds().size;
            let window_info = WindowInfo {
                parent_view: Retained::as_ptr(&content).cast_mut().cast(),
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: size.width as i32,
                    height: size.height as i32,
                },
                windowless_rendering_enabled: 0,
                runtime_style: RuntimeStyle::ALLOY,
                ..WindowInfo::default()
            };
            Some((
                window_info,
                PopupWindow {
                    window,
                    owner: owner_window,
                },
            ))
        }

        /// Move the window to `bounds` (physical screen pixels); CEF's
        /// view follows the content view (it autoresizes with it).
        pub(super) fn resize(
            window: &PopupWindow,
            _browser: &Browser,
            bounds: ScreenRect,
            scale: f32,
        ) {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            window
                .window
                .setFrame_display(screen_frame(bounds, scale, mtm), true);
        }

        /// `bounds` (physical pixels from the top left of the main
        /// display, the shell's and winit's convention) as an AppKit
        /// frame (points from the bottom left of the main display).
        fn screen_frame(bounds: ScreenRect, scale: f32, mtm: MainThreadMarker) -> NSRect {
            let scale = f64::from(scale.max(0.01));
            let main_height = NSScreen::screens(mtm)
                .firstObject()
                .map(|screen| screen.frame().size.height)
                .unwrap_or(0.0);
            let width = f64::from(bounds.width) / scale;
            let height = f64::from(bounds.height) / scale;
            let x = f64::from(bounds.x) / scale;
            let top = f64::from(bounds.y) / scale;
            NSRect::new(
                NSPoint::new(x, main_height - top - height),
                NSSize::new(width, height),
            )
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
            let placed = place(anchor, 300, 200);
            assert_eq!(
                (placed.x, placed.y, placed.width, placed.height),
                (226, 68, 300, 200)
            );
            let near_left = ScreenRect { x: 10, ..anchor };
            let placed = place(near_left, 300, 200);
            assert_eq!((placed.x, placed.y), (0, 68));
        }
    }
}
