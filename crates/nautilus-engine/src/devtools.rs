//! A tab's developer tools ("Inspect" in the page's context menu): a
//! windowed browser Chromium opens in a top-level window of its own.
//! Closing the tab closes them; the live-browser count keeps shutdown
//! waiting for them like for any browser.

use std::rc::Rc;

use cef::rc::Rc as _;

use cef::{
    Browser, BrowserHost, BrowserSettings, Client, DialogHandler, ImplBrowserHost, ImplClient,
    ImplLifeSpanHandler, LifeSpanHandler, Point, WindowInfo, WrapClient, WrapLifeSpanHandler,
    wrap_client, wrap_life_span_handler,
};

use crate::delegate::DelegateState;

/// Open the developer tools of `host`'s browser, inspecting the element
/// at `point` (logical pixels from the view's top left) if given.
pub(crate) fn open(delegate: &Rc<DelegateState>, host: &BrowserHost, point: Option<(i32, i32)>) {
    let window_info = dev_tools_window();
    let mut client = DevToolsClient::new(
        DevToolsLife::new(delegate.clone()),
        delegate.dialog_handler(),
    );
    let settings = BrowserSettings::default();
    let point = point.map(|(x, y)| Point { x, y });
    host.show_dev_tools(
        Some(&window_info),
        Some(&mut client),
        Some(&settings),
        point.as_ref(),
    );
}

/// A top-level window of Chromium's own, titled for the tools.
#[cfg(windows)]
fn dev_tools_window() -> WindowInfo {
    WindowInfo::default().set_as_popup(cef::sys::HWND(std::ptr::null_mut()), "Developer Tools")
}

#[cfg(not(windows))]
fn dev_tools_window() -> WindowInfo {
    WindowInfo {
        window_name: cef::CefString::from("Developer Tools"),
        bounds: cef::Rect {
            x: 80,
            y: 80,
            width: 1000,
            height: 700,
        },
        ..WindowInfo::default()
    }
}

wrap_client! {
    struct DevToolsClient {
        life: LifeSpanHandler,
        dialog: DialogHandler,
    }

    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.life.clone())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.dialog.clone())
        }
    }
}

wrap_life_span_handler! {
    struct DevToolsLife {
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
