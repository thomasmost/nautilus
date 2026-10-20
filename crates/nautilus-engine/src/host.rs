//! The per-window tab host: one windowless CEF browser per tab, painting
//! into a [`Frame`] the window uploads under its toolbar.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::{
    Browser, BrowserSettings, CefString, Frame as CefFrame, ImplBrowser, ImplBrowserHost,
    ImplFrame, KeyEvent, KeyEventType, MouseButtonType, MouseEvent, WindowInfo,
};
use dpi::PhysicalSize;
use osmena_core::{ButtonAction, EngineInput, MouseButton, PageCommand, PendingTab, TabId};
use osmena_engine::{EngineError, FrameRef, PagePaint, ScreenRect, TabHost as TabHostApi};
use raw_window_handle::RawWindowHandle;
use url::Url;

use crate::delegate::{ClipboardImageCallback, DelegateState, ViewState, client_for};
use crate::devtools;
use crate::engine::Engine;
use crate::extensions::ExtensionManager;
use crate::frame::Frame;
use crate::input::{self, KeyEventKind};
use crate::popup::Popups;

/// Two clicks closer than this in time and space count as a double click
/// (then a triple), as Chromium itself expects to be told.
const MULTI_CLICK_TIME: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f32 = 4.0;

struct TabEntry {
    browser: Browser,
    view: Rc<ViewState>,
}

/// The engine side of one browser window: its tabs and their frames.
pub struct TabHost {
    tabs: HashMap<TabId, TabEntry>,
    delegate: Rc<DelegateState>,
    extensions: Rc<ExtensionManager>,
    /// The extension action popups over this window.
    popups: Popups,
    /// The native window the tabs belong to, for the dialogs and menus
    /// Chromium opens itself.
    parent: cef::sys::cef_window_handle_t,
    scale: Cell<f32>,
    /// The page area in physical pixels; every tab of the window shares it.
    content_size: Cell<(u32, u32)>,
    /// The page area's top-left on screen, logical pixels; shared with
    /// every tab's view state.
    origin: Rc<Cell<(i32, i32)>>,
    /// Keyboard modifier flags from the last key event, for mouse events.
    key_flags: Cell<u32>,
    /// Mouse buttons currently held, as CEF event flags.
    button_flags: Cell<u32>,
    last_click: RefCell<Option<LastClick>>,
    last_pointer: Cell<(i32, i32)>,
}

struct LastClick {
    at: Instant,
    button: MouseButton,
    x: f32,
    y: f32,
    count: i32,
}

impl TabHost {
    /// A host for tabs shown in the native window `parent`, at the
    /// window's scale factor.
    pub fn new(
        engine: &Engine,
        parent: RawWindowHandle,
        scale: f32,
    ) -> Result<TabHost, EngineError> {
        let parent = native_handle(parent)?;
        Ok(TabHost {
            tabs: HashMap::new(),
            delegate: engine.delegate().clone(),
            extensions: engine.extensions().clone(),
            popups: Popups::new(engine.delegate().clone(), parent),
            parent,
            scale: Cell::new(scale),
            content_size: Cell::new((1024, 768)),
            origin: Rc::new(Cell::new((0, 0))),
            key_flags: Cell::new(0),
            button_flags: Cell::new(0),
            last_click: RefCell::new(None),
            last_pointer: Cell::new((0, 0)),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    fn with(&self, tab: TabId, f: impl FnOnce(&TabEntry)) {
        match self.tabs.get(&tab) {
            Some(entry) => f(entry),
            None => log::warn!("no engine tab for {tab:?}"),
        }
    }

    fn with_host(&self, tab: TabId, f: impl FnOnce(&cef::BrowserHost)) {
        self.with(tab, |entry| {
            if let Some(host) = entry.browser.host() {
                f(&host);
            }
        });
    }

    /// Borrow the tab's latest pixels.
    pub fn with_frame<R>(&self, tab: TabId, f: impl FnOnce(&mut Frame) -> R) -> Option<R> {
        let entry = self.tabs.get(&tab)?;
        Some(f(&mut entry.view.frame.borrow_mut()))
    }

    /// The click count CEF expects for a press: 1, or one more than the
    /// previous click's if it was the same button, close in time and
    /// space.
    fn click_count(&self, button: MouseButton, x: f32, y: f32) -> i32 {
        let now = Instant::now();
        let mut last = self.last_click.borrow_mut();
        let count = match last.as_ref() {
            Some(prev)
                if prev.button == button
                    && now.duration_since(prev.at) <= MULTI_CLICK_TIME
                    && (prev.x - x).abs() <= MULTI_CLICK_DISTANCE
                    && (prev.y - y).abs() <= MULTI_CLICK_DISTANCE =>
            {
                (prev.count % 3) + 1
            }
            _ => 1,
        };
        *last = Some(LastClick {
            at: now,
            button,
            x,
            y,
            count,
        });
        count
    }
}

impl TabHostApi for TabHost {
    /// Create a tab loading `url`.
    fn create_tab(&mut self, tab: TabId, url: Url) {
        let (width, height) = self.content_size.get();
        let scale = self.scale.get();
        let view = Rc::new(ViewState {
            tab,
            delegate: self.delegate.clone(),
            dip_size: Cell::new(dip_size(width, height, scale)),
            scale: Cell::new(scale),
            origin: self.origin.clone(),
            frame: RefCell::new(Frame::default()),
            closing: Cell::new(false),
            context_menu: RefCell::new(None),
        });
        let mut client = client_for(view.clone());
        let window_info = WindowInfo::default().set_as_windowless(self.parent);
        let settings = BrowserSettings {
            windowless_frame_rate: 60,
            // Opaque: the frame is uploaded without blending.
            background_color: 0xFFFF_FFFF,
            ..BrowserSettings::default()
        };
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from(url.as_str())),
            Some(&settings),
            None,
            None,
        );
        match browser {
            Some(browser) => {
                self.tabs.insert(tab, TabEntry { browser, view });
            }
            None => log::error!("Chromium could not create a browser for {tab:?} at {url}"),
        }
    }

    /// Create the tab page content asked for. Returns false if the engine
    /// no longer holds the request.
    fn adopt_tab(&mut self, pending: PendingTab, tab: TabId) -> bool {
        let Some(url) = self.delegate.take_pending(pending) else {
            return false;
        };
        self.create_tab(tab, url);
        true
    }

    /// Close the tab's browser, and its developer tools if they are
    /// open. It finishes closing on the message loop.
    fn close_tab(&mut self, tab: TabId) {
        if let Some(entry) = self.tabs.remove(&tab) {
            entry.view.closing.set(true);
            if let Some(host) = entry.browser.host() {
                if host.has_dev_tools() != 0 {
                    host.close_dev_tools();
                }
                host.close_browser(1);
            }
        }
    }

    fn has_tab(&self, tab: TabId) -> bool {
        self.tabs.contains_key(&tab)
    }

    fn navigate(&self, tab: TabId, url: Url) {
        self.with(tab, |entry| {
            if let Some(frame) = entry.browser.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        });
    }

    fn back(&self, tab: TabId) {
        self.with(tab, |entry| {
            if entry.browser.can_go_back() != 0 {
                entry.browser.go_back();
            }
        });
    }

    fn forward(&self, tab: TabId) {
        self.with(tab, |entry| {
            if entry.browser.can_go_forward() != 0 {
                entry.browser.go_forward();
            }
        });
    }

    fn reload(&self, tab: TabId) {
        self.with(tab, |entry| entry.browser.reload());
    }

    fn stop(&self, tab: TabId) {
        self.with(tab, |entry| entry.browser.stop_load());
    }

    /// The tab is the window's visible tab: paint it.
    fn show(&self, tab: TabId) {
        self.with_host(tab, |host| {
            host.was_hidden(0);
            host.invalidate(cef::PaintElementType::VIEW);
        });
    }

    /// The tab is behind another: stop painting it.
    fn hide(&self, tab: TabId) {
        self.with_host(tab, |host| host.was_hidden(1));
    }

    fn set_focused(&self, tab: TabId, focused: bool) {
        if focused && let Some(entry) = self.tabs.get(&tab) {
            self.delegate.set_last_focused(tab, &entry.browser);
        }
        self.with_host(tab, |host| host.set_focus(i32::from(focused)));
    }

    /// Resize the page area, if it changed. All tabs in a window share it.
    fn resize_content(&self, tab: TabId, size: PhysicalSize<u32>) {
        let size = (size.width.max(1), size.height.max(1));
        if self.content_size.replace(size) == size {
            return;
        }
        let scale = self.scale.get();
        let dip = dip_size(size.0, size.1, scale);
        for entry in self.tabs.values() {
            entry.view.dip_size.set(dip);
        }
        // Only the shown tab needs telling now; a hidden tab picks the
        // size up through `view_rect` when it is shown and repainted.
        self.with_host(tab, |host| host.was_resized());
    }

    /// Where the page area sits on screen (logical pixels), so Chromium
    /// can place its menus and widgets.
    fn set_content_origin(&self, x: i32, y: i32) {
        self.origin.set((x, y));
    }

    fn set_hidpi(&self, scale: f32) {
        self.scale.set(scale);
        let (width, height) = self.content_size.get();
        let dip = dip_size(width, height, scale);
        for entry in self.tabs.values() {
            entry.view.scale.set(scale);
            entry.view.dip_size.set(dip);
            if let Some(host) = entry.browser.host() {
                host.notify_screen_info_changed();
                host.was_resized();
            }
        }
    }

    fn input(&self, tab: TabId, input: EngineInput) {
        let Some(entry) = self.tabs.get(&tab) else {
            log::warn!("no engine tab for {tab:?}");
            return;
        };
        let Some(host) = entry.browser.host() else {
            return;
        };
        let scale = self.scale.get();
        let to_dip = |v: f32| (v / scale).round() as i32;
        let mouse = |x: f32, y: f32, flags: u32| MouseEvent {
            x: to_dip(x),
            y: to_dip(y),
            modifiers: flags,
        };
        let flags = self.key_flags.get() | self.button_flags.get();
        match input {
            EngineInput::MouseMove { x, y } => {
                self.last_pointer.set((to_dip(x), to_dip(y)));
                host.send_mouse_move_event(Some(&mouse(x, y, flags)), 0);
            }
            EngineInput::MouseLeft => {
                let (x, y) = self.last_pointer.get();
                let event = MouseEvent {
                    x,
                    y,
                    modifiers: flags,
                };
                host.send_mouse_move_event(Some(&event), 1);
            }
            EngineInput::MouseButton {
                action,
                button,
                x,
                y,
            } => {
                let Some(kind) = button_type(button) else {
                    return;
                };
                let (count, flags) = match action {
                    ButtonAction::Down => {
                        let count = self.click_count(button, x, y);
                        self.button_flags
                            .set(self.button_flags.get() | input::button_flag(button));
                        (count, self.key_flags.get() | self.button_flags.get())
                    }
                    ButtonAction::Up => {
                        // The flags of a release still name the button.
                        let flags = self.key_flags.get() | self.button_flags.get();
                        self.button_flags
                            .set(self.button_flags.get() & !input::button_flag(button));
                        let count = self
                            .last_click
                            .borrow()
                            .as_ref()
                            .map_or(1, |last| last.count);
                        (count, flags)
                    }
                };
                host.send_mouse_click_event(
                    Some(&mouse(x, y, flags)),
                    kind,
                    i32::from(action == ButtonAction::Up),
                    count,
                );
            }
            EngineInput::Wheel { dx, dy, x, y } => {
                host.send_mouse_wheel_event(
                    Some(&mouse(x, y, flags)),
                    (dx / f64::from(scale)).round() as i32,
                    (dy / f64::from(scale)).round() as i32,
                );
            }
            EngineInput::Key(key) => {
                self.key_flags.set(input::modifier_flags(key.modifiers));
                for spec in input::key_events(&key) {
                    let event = KeyEvent {
                        type_: match spec.kind {
                            KeyEventKind::RawKeyDown => KeyEventType::RAWKEYDOWN,
                            KeyEventKind::KeyUp => KeyEventType::KEYUP,
                            KeyEventKind::Char => KeyEventType::CHAR,
                        },
                        modifiers: spec.modifiers,
                        windows_key_code: spec.windows_key_code,
                        native_key_code: spec.native_key_code,
                        is_system_key: 0,
                        character: spec.character,
                        unmodified_character: spec.unmodified_character,
                        focus_on_editable_field: 0,
                        ..KeyEvent::default()
                    };
                    host.send_key_event(Some(&event));
                }
            }
            EngineInput::ImeStart => {}
            EngineInput::ImePreedit(text) => {
                host.ime_set_composition(Some(&CefString::from(text.as_str())), None, None, None);
            }
            EngineInput::ImeCommit(text) => {
                host.ime_commit_text(Some(&CefString::from(text.as_str())), None, 0);
            }
            EngineInput::ImeDismissed => host.ime_cancel_composition(),
        }
    }

    /// Open the popup page of `extension`'s action under its button.
    fn open_extension_action(&self, _tab: TabId, extension: &str, anchor: ScreenRect) {
        match self.extensions.popup_url(extension) {
            Some(url) => self.popups.open(extension, &url, anchor, self.scale.get()),
            None => log::warn!("extension {extension} has no popup to open"),
        }
    }

    fn close_extension_popups(&self) {
        self.popups.close_all();
    }

    /// Carry out a choice from the page's context menu. The editing
    /// commands go to the frame the menu was opened in; the rest are the
    /// browser host's tools, at the point the menu was opened.
    fn page_command(&self, tab: TabId, command: PageCommand) {
        let Some(entry) = self.tabs.get(&tab) else {
            log::warn!("no engine tab for {tab:?}");
            return;
        };
        let Some(host) = entry.browser.host() else {
            return;
        };
        let menu = entry.view.context_menu.borrow();
        let point = menu.as_ref().map(|m| (m.x, m.y));
        let Some(frame) = menu
            .as_ref()
            .and_then(|m| m.frame.clone())
            .or_else(|| entry.browser.main_frame())
        else {
            return;
        };
        let cef_text = |s: &str| CefString::from(s);
        match command {
            PageCommand::Undo => frame.undo(),
            PageCommand::Redo => frame.redo(),
            PageCommand::Cut => frame.cut(),
            PageCommand::Copy => frame.copy(),
            PageCommand::Paste => frame.paste(),
            PageCommand::PasteAsPlainText => frame.paste_and_match_style(),
            PageCommand::Delete => frame.del(),
            PageCommand::SelectAll => frame.select_all(),
            PageCommand::ReplaceMisspelling(word) => {
                host.replace_misspelling(Some(&cef_text(&word)));
            }
            PageCommand::AddToDictionary(word) => {
                host.add_word_to_dictionary(Some(&cef_text(&word)));
            }
            PageCommand::Save(url) => host.start_download(Some(&cef_text(url.as_str()))),
            PageCommand::CopyImage(url) => {
                let mut callback = ClipboardImageCallback::new(entry.view.clone());
                host.download_image(Some(&cef_text(url.as_str())), 0, 0, 0, Some(&mut callback));
            }
            PageCommand::SetMediaLoop(on) => set_media_property(&frame, point, "loop", on),
            PageCommand::SetMediaControls(on) => {
                set_media_property(&frame, point, "controls", on);
            }
            PageCommand::Print => host.print(),
            PageCommand::ViewSource => {
                // Chromium renders `view-source:` pages in any browser;
                // CEF's own `ViewSource` would open a text editor instead.
                let url = CefString::from(&frame.url()).to_string();
                match Url::parse(&format!("view-source:{url}")) {
                    Ok(source) => self.delegate.request_tab(tab, source, false),
                    Err(e) => log::warn!("cannot view the source of {url}: {e}"),
                }
            }
            PageCommand::Inspect => devtools::open(&self.delegate, &host, point),
        }
    }

    fn context_menu_closed(&self, tab: TabId) {
        if let Some(entry) = self.tabs.get(&tab) {
            entry.view.context_menu.borrow_mut().take();
        }
    }

    /// The tab's latest frame, if it painted one yet.
    fn paint_page(&self, tab: TabId, sink: &mut dyn FnMut(PagePaint<'_>)) {
        let Some(entry) = self.tabs.get(&tab) else {
            return;
        };
        let mut frame = entry.view.frame.borrow_mut();
        if frame.is_empty() {
            return;
        }
        let (width, height) = frame.size();
        let fresh = frame.take_dirty();
        let pixels = frame.pixels();
        sink(PagePaint::Frame(FrameRef {
            width,
            height,
            bgra: &pixels,
            fresh,
        }));
    }
}

impl Drop for TabHost {
    fn drop(&mut self) {
        let tabs: Vec<TabId> = self.tabs.keys().copied().collect();
        for tab in tabs {
            self.close_tab(tab);
        }
    }
}

/// Set `property` (`loop`, `controls`) of the media element at `point`
/// (logical pixels from the view's top left) in `frame`. Chromium does
/// this for its own menu inside the renderer; CEF exposes no way in, so
/// the page's script does it. The point is the view's; a media element
/// in a sub-frame whose origin differs is missed.
fn set_media_property(frame: &CefFrame, point: Option<(i32, i32)>, property: &str, on: bool) {
    let Some((x, y)) = point else {
        return;
    };
    let script = format!(
        "(function(x,y){{var e=document.elementFromPoint(x,y);\
         while(e&&!(e instanceof HTMLMediaElement))e=e.parentElement;\
         if(e)e.{property}={on};}})({x},{y})"
    );
    frame.execute_java_script(Some(&CefString::from(script.as_str())), None, 0);
}

fn dip_size(width: u32, height: u32, scale: f32) -> (i32, i32) {
    (
        ((width as f32 / scale).round() as i32).max(1),
        ((height as f32 / scale).round() as i32).max(1),
    )
}

fn button_type(button: MouseButton) -> Option<MouseButtonType> {
    match button {
        MouseButton::Primary => Some(MouseButtonType::LEFT),
        MouseButton::Secondary => Some(MouseButtonType::RIGHT),
        MouseButton::Auxiliary => Some(MouseButtonType::MIDDLE),
        MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => None,
    }
}

#[cfg(target_os = "windows")]
fn native_handle(handle: RawWindowHandle) -> Result<cef::sys::cef_window_handle_t, EngineError> {
    match handle {
        RawWindowHandle::Win32(h) => Ok(cef::sys::HWND(h.hwnd.get() as *mut _)),
        other => Err(EngineError(format!(
            "unsupported window handle for Chromium: {other:?}"
        ))),
    }
}

#[cfg(target_os = "macos")]
fn native_handle(handle: RawWindowHandle) -> Result<cef::sys::cef_window_handle_t, EngineError> {
    match handle {
        RawWindowHandle::AppKit(h) => Ok(h.ns_view.as_ptr().cast()),
        other => Err(EngineError(format!(
            "unsupported window handle for Chromium: {other:?}"
        ))),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn native_handle(_handle: RawWindowHandle) -> Result<cef::sys::cef_window_handle_t, EngineError> {
    Err(EngineError(
        "Nautilus runs on Windows and macOS only".to_string(),
    ))
}
