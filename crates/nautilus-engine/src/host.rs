//! The per-window tab host: one windowless CEF browser per tab, painting
//! into a [`Frame`] the window uploads under its toolbar.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cef::{
    Browser, BrowserSettings, CefString, DragOperationsMask, Frame as CefFrame, ImplBrowser,
    ImplBrowserHost, ImplFrame, ImplMediaAccessCallback, KeyEvent, KeyEventType, MouseButtonType,
    MouseEvent, WindowInfo,
};
use dpi::PhysicalSize;
use keyboard_types::{Key, KeyState, NamedKey};
use osmena_core::{
    ButtonAction, EngineEvent, EngineInput, MouseButton, PageCommand, PendingTab, TabId,
};
use osmena_engine::{EngineError, FrameRef, PagePaint, ScreenRect, TabHost as TabHostApi};
use raw_window_handle::RawWindowHandle;
use url::Url;

use crate::delegate::{
    ClipboardImageCallback, DelegateState, Pending, ViewState, apply_tab_settings, client_for,
};
use crate::devtools;
use crate::engine::Engine;
use crate::extensions::ExtensionManager;
use crate::frame::Frame;
use crate::input::{self, KeyEventKind};
use crate::panel::{self, Resized, ViewSizes, dip_size};
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
    /// The sizes of the views in physical pixels: the page area, which
    /// every tab of the window shares, and each side panel's own.
    sizes: RefCell<ViewSizes>,
    /// The tab the window shows, for what a side panel opens.
    shown_tab: Rc<Cell<Option<TabId>>>,
    /// The page area's top-left on screen, logical pixels; shared with
    /// every tab's view state.
    origin: Rc<Cell<(i32, i32)>>,
    /// Mouse buttons currently held, as CEF event flags. The keyboard
    /// modifiers are not kept here: every pointer event brings its own
    /// from the shell, whose window state knows about a modifier
    /// released while the window was not focused.
    button_flags: Cell<u32>,
    last_click: RefCell<Option<LastClick>>,
    /// The fraction of a wheel step not yet sent, per axis (CEF takes
    /// whole steps; see `input::wheel_steps`).
    wheel_carry: Cell<(f64, f64)>,
}

struct LastClick {
    /// The view the press was in: a click continues only in its own view.
    view: TabId,
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
            sizes: RefCell::new(ViewSizes::new((1024, 768))),
            shown_tab: Rc::new(Cell::new(None)),
            origin: Rc::new(Cell::new((0, 0))),
            button_flags: Cell::new(0),
            last_click: RefCell::new(None),
            wheel_carry: Cell::new((0.0, 0.0)),
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

    /// The click count CEF expects for a press in `view`: 1, or one more
    /// than the previous click's if it continues it (`continues_click`).
    fn click_count(&self, view: TabId, button: MouseButton, x: f32, y: f32) -> i32 {
        let now = Instant::now();
        let mut last = self.last_click.borrow_mut();
        let count = match last.as_ref() {
            Some(prev) if continues_click(prev, view, button, x, y, now) => (prev.count % 3) + 1,
            _ => 1,
        };
        *last = Some(LastClick {
            view,
            at: now,
            button,
            x,
            y,
            count,
        });
        count
    }

    /// Drop the entries of side panels whose browser went without the
    /// host asking (`ViewState::retire`): the model already closed those
    /// panels and sends no `CloseView` for them.
    fn sweep(&mut self) {
        let gone: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(_, entry)| entry.view.gone.get())
            .map(|(view, _)| *view)
            .collect();
        for view in gone {
            self.tabs.remove(&view);
            self.sizes.borrow_mut().remove_panel(view);
        }
    }

    /// Input while the tab's page drags something (`ViewState::drag`,
    /// from `start_dragging`): the moves and the release are the drag's,
    /// not the page's mouse events, and go to Chromium's drop-target
    /// side. Returns whether `input` was taken by the drag.
    ///
    /// While the pointer is over the view the moves are drag-overs (an
    /// enter first); off it (over the chrome, outside the window: the
    /// shell's capture keeps sending them) the target is left. The
    /// primary release drops where the pointer is if it is over the
    /// view, and ends the drag either way; Escape cancels it. A primary
    /// press while a session is still open means its release was lost,
    /// so the session is ended first and the press goes on as usual.
    fn drag_input(&self, entry: &TabEntry, host: &cef::BrowserHost, input: &EngineInput) -> bool {
        let scale = self.scale.get();
        let to_dip = |v: f32| (v / scale).round() as i32;
        let (width, height) = entry.view.dip_size.get();
        let inside = |x: i32, y: i32| (0..width).contains(&x) && (0..height).contains(&y);
        match input {
            EngineInput::MouseMove { x, y, modifiers } => {
                let point = (to_dip(*x), to_dip(*y));
                entry.view.last_pointer.set(point);
                let event = MouseEvent {
                    x: point.0,
                    y: point.1,
                    modifiers: input::modifier_flags(*modifiers) | self.button_flags.get(),
                };
                // Chromium's answer (`update_drag_cursor`) arrives on
                // a later message-loop turn, but the borrow is not
                // held across the calls all the same.
                let (data, allowed, entered, hotspot) = {
                    let mut drag = entry.view.drag.borrow_mut();
                    let Some(session) = drag.as_mut() else {
                        return false;
                    };
                    let entered = session.entered;
                    session.entered = inside(point.0, point.1);
                    (
                        Clone::clone(&session.data),
                        session.allowed,
                        entered,
                        session.hotspot,
                    )
                };
                if inside(point.0, point.1) {
                    if !entered {
                        let mut data = data;
                        host.drag_target_drag_enter(Some(&mut data), Some(&event), allowed);
                    }
                    host.drag_target_drag_over(Some(&event), allowed);
                } else if entered {
                    host.drag_target_drag_leave();
                    entry.view.report(|tab| EngineEvent::CursorChanged {
                        tab,
                        cursor: osmena_core::Cursor::NotAllowed,
                    });
                }
                entry
                    .view
                    .frame
                    .borrow_mut()
                    .set_drag_rect(x.round() as i32 - hotspot.0, y.round() as i32 - hotspot.1);
                entry.view.report(EngineEvent::FrameReady);
                true
            }
            EngineInput::MouseLeft => {
                let entered = {
                    let mut drag = entry.view.drag.borrow_mut();
                    let Some(session) = drag.as_mut() else {
                        return false;
                    };
                    std::mem::replace(&mut session.entered, false)
                };
                if entered {
                    host.drag_target_drag_leave();
                }
                true
            }
            EngineInput::MouseButton {
                action: ButtonAction::Up,
                button: MouseButton::Primary,
                x,
                y,
                modifiers,
            } => {
                if entry.view.drag.borrow().is_none() {
                    return false;
                }
                let point = (to_dip(*x), to_dip(*y));
                let event = MouseEvent {
                    x: point.0,
                    y: point.1,
                    modifiers: input::modifier_flags(*modifiers) | self.button_flags.get(),
                };
                self.button_flags
                    .set(self.button_flags.get() & !input::button_flag(MouseButton::Primary));
                self.end_drag(entry, host, inside(point.0, point.1).then_some(event));
                true
            }
            EngineInput::MouseButton {
                action: ButtonAction::Down,
                button: MouseButton::Primary,
                ..
            } => {
                if entry.view.drag.borrow().is_some() {
                    log::debug!("tab {}: a drag outlived its gesture", entry.view.label());
                    self.end_drag(entry, host, None);
                }
                false
            }
            EngineInput::Key(key)
                if key.state == KeyState::Down && key.key == Key::Named(NamedKey::Escape) =>
            {
                if entry.view.drag.borrow().is_none() {
                    return false;
                }
                self.end_drag(entry, host, None);
                true
            }
            _ => false,
        }
    }

    /// End the tab's drag: with a drop at `drop` (the pointer is over
    /// the view, and the page gets `drop` there), or cancelled. The
    /// source gets `dragend` either way, with the operation the target
    /// accepted or none. The order is CEF's: the target's calls before
    /// the source's.
    fn end_drag(&self, entry: &TabEntry, host: &cef::BrowserHost, drop: Option<MouseEvent>) {
        let Some(session) = entry.view.drag.borrow_mut().take() else {
            return;
        };
        let none = DragOperationsMask::default();
        let (x, y, operation) = match drop {
            Some(event) if session.entered => {
                host.drag_target_drop(Some(&event));
                (event.x, event.y, session.operation)
            }
            _ => {
                if session.entered {
                    host.drag_target_drag_leave();
                }
                let (x, y) = entry.view.last_pointer.get();
                (x, y, none)
            }
        };
        host.drag_source_ended_at(x, y, operation);
        host.drag_source_system_drag_ended();
        entry.view.frame.borrow_mut().hide_drag_image();
        log::debug!(
            "tab {}: drag ended at ({x}, {y}), {}",
            entry.view.label(),
            if operation == none {
                "cancelled"
            } else {
                "dropped"
            }
        );
        entry.view.report(EngineEvent::FrameReady);
        let cursor = entry.view.cursor.get();
        entry
            .view
            .report(move |tab| EngineEvent::CursorChanged { tab, cursor });
    }
}

impl TabHostApi for TabHost {
    /// Create a tab loading `url`.
    fn create_tab(&mut self, tab: TabId, url: Url) {
        self.sweep();
        let (width, height) = self.sizes.borrow().page();
        let scale = self.scale.get();
        let view = Rc::new(ViewState::new(
            tab,
            self.delegate.clone(),
            self.parent,
            dip_size(width, height, scale),
            scale,
            self.origin.clone(),
        ));
        let mut client = client_for(view.clone());
        let window_info = WindowInfo::default().set_as_windowless(self.parent);
        let mut settings = BrowserSettings::default();
        apply_tab_settings(&mut settings);
        let mut context = self.delegate.request_context();
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from(url.as_str())),
            Some(&settings),
            None,
            context.as_mut(),
        );
        match browser {
            Some(browser) => {
                self.delegate.register_view(tab, &browser);
                crate::popup_pause::track(&view, &browser);
                self.extensions.track_tab(tab, &browser, view.clone());
                self.tabs.insert(tab, TabEntry { browser, view });
            }
            None => log::error!("Chromium could not create a browser for {tab:?} at {url}"),
        }
    }

    /// Make the tab page content asked for: a browser at its URL, or the
    /// popup Chromium already created, which becomes `tab`. Returns false
    /// if the engine no longer holds the request.
    fn adopt_tab(&mut self, pending: PendingTab, tab: TabId) -> bool {
        match self.delegate.take_pending(pending) {
            None => false,
            Some(Pending::Url(url)) => {
                self.create_tab(tab, url);
                true
            }
            Some(Pending::Popup { view, browser }) => {
                // The popup was sized like its opener when it was asked
                // for; the window may have changed since.
                let (width, height) = self.sizes.borrow().page();
                let scale = self.scale.get();
                view.scale.set(scale);
                view.dip_size.set(dip_size(width, height, scale));
                if let Some(host) = browser.host() {
                    host.notify_screen_info_changed();
                    host.was_resized();
                }
                view.adopt(tab);
                self.delegate.register_view(tab, &browser);
                crate::popup_pause::track(&view, &browser);
                self.extensions.track_tab(tab, &browser, view.clone());
                self.tabs.insert(tab, TabEntry { browser, view });
                true
            }
        }
    }

    /// Close the tab's browser, and its developer tools if they are
    /// open. It finishes closing on the message loop.
    fn close_tab(&mut self, tab: TabId) {
        if let Some(entry) = self.tabs.remove(&tab) {
            // A closed tab is no longer the one a panel opens beside;
            // `show` sets the next one when the window shows a tab.
            if self.shown_tab.get() == Some(tab) {
                self.shown_tab.set(None);
            }
            self.delegate.unregister_view(tab);
            self.extensions.untrack_tab(tab);
            entry.view.closing.set(true);
            // A button pressed in this tab is released nowhere (the
            // shell delivers a release only to the tab that got the
            // press), so forget the buttons rather than stamp every
            // later move with one; a drag goes with its page.
            self.button_flags.set(0);
            entry.view.drag.borrow_mut().take();
            if let Some(host) = entry.browser.host() {
                if host.has_dev_tools() != 0 {
                    host.close_dev_tools();
                }
                host.close_browser(1);
            }
        }
    }

    /// The view of a side panel: a windowless browser at `extension`'s
    /// `page`, under the id the model chose. Every other method of the
    /// host finds it under that id like a tab. It starts hidden; the shell
    /// shows it when it is the panel on screen.
    ///
    /// If the page has no address or Chromium makes no browser, the model
    /// is told the view closed (`TabClosed`), which closes the panel there.
    fn create_view(&mut self, view: TabId, extension: &str, page: &str) {
        self.sweep();
        let Some(url) = panel::page_url(extension, page) else {
            log::warn!("extension {extension} has no side panel page at {page:?}");
            self.delegate.push(EngineEvent::TabClosed(view));
            return;
        };
        let scale = self.scale.get();
        let (width, height) = self.sizes.borrow_mut().add_panel(view, scale);
        let state = Rc::new(ViewState::panel(
            view,
            self.delegate.clone(),
            self.parent,
            dip_size(width, height, scale),
            scale,
            // Its own corner on screen, the page's until the shell says
            // (`set_view_origin`).
            Rc::new(Cell::new(self.origin.get())),
            self.shown_tab.clone(),
        ));
        let mut client = client_for(state.clone());
        let window_info = WindowInfo::default().set_as_windowless(self.parent);
        let mut settings = BrowserSettings::default();
        apply_tab_settings(&mut settings);
        let mut context = self.delegate.request_context();
        let browser = cef::browser_host_create_browser_sync(
            Some(&window_info),
            Some(&mut client),
            Some(&CefString::from(url.as_str())),
            Some(&settings),
            None,
            context.as_mut(),
        );
        let Some(browser) = browser else {
            log::error!("Chromium could not create a browser for side panel {view:?} at {url}");
            self.sizes.borrow_mut().remove_panel(view);
            self.delegate.push(EngineEvent::TabClosed(view));
            return;
        };
        if let Some(host) = browser.host() {
            host.was_hidden(1);
        }
        if !self.delegate.panel_opened(view, extension, page) {
            log::warn!("side panel {view:?} of {extension} was not asked for by the engine");
        }
        log::info!("side panel {view:?}: {url}");
        // The page's calls name their view through `DelegateState::views`
        // (Phase 5's `asker_of`); without this the panel's calls arrive as
        // a worker's.
        self.delegate.register_view(view, &browser);
        // A panel paints, so it is on the list of views a popup widget
        // pauses (`popup_pause`). It is not a tab of the extensions'
        // table: no `track_tab` (D7).
        crate::popup_pause::track(&state, &browser);
        self.tabs.insert(
            view,
            TabEntry {
                browser,
                view: state,
            },
        );
    }

    /// The panel is shown nowhere any more: close its browser.
    fn close_view(&mut self, view: TabId) {
        self.sweep();
        self.sizes.borrow_mut().remove_panel(view);
        self.delegate.panel_gone(view);
        self.close_tab(view);
    }

    /// Where a side panel's view sits on screen (logical pixels, as
    /// `set_content_origin`), so its `<select>` lists and `screenX` are
    /// the panel's and not the page's.
    fn set_view_origin(&mut self, view: TabId, x: i32, y: i32) {
        if let Some(entry) = self.tabs.get(&view)
            && entry.view.is_panel()
        {
            entry.view.origin.set((x, y));
        }
    }

    fn has_tab(&self, tab: TabId) -> bool {
        self.tabs
            .get(&tab)
            .is_some_and(|entry| !entry.view.gone.get())
    }

    fn navigate(&self, tab: TabId, url: Url) {
        self.with(tab, |entry| {
            if let Some(frame) = entry.browser.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        });
    }

    /// `location.replace` swaps the current entry for the blank page;
    /// `load_url` would add one and leave the blocked page behind Back.
    fn replace_with_blank(&self, tab: TabId) {
        self.with(tab, |entry| {
            if let Some(frame) = entry.browser.main_frame() {
                frame.execute_java_script(
                    Some(&CefString::from("location.replace('about:blank')")),
                    None,
                    0,
                );
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

    /// The tab is the window's visible tab: paint it, unless another
    /// tab's popup widget is holding the others (`popup_pause`), which
    /// lets it go when the widget closes.
    fn show(&self, tab: TabId) {
        let Some(entry) = self.tabs.get(&tab) else {
            return;
        };
        // A tab that is shown is the window's tab, which is what a side
        // panel opens tabs beside.
        if !entry.view.is_panel() {
            self.shown_tab.set(Some(tab));
        }
        entry.view.hidden.set(false);
        let hidden = entry.view.wants_hidden();
        self.with_host(tab, |host| {
            if hidden {
                host.was_hidden(1);
            } else {
                crate::delegate::show_view(host);
            }
        });
    }

    /// The tab is behind another: stop painting it, unless an extension is
    /// debugging it (a hidden renderer answers no input, and the debugger
    /// waits on that answer; `bridge::debugger` decides).
    fn hide(&self, tab: TabId) {
        let Some(entry) = self.tabs.get(&tab) else {
            return;
        };
        entry.view.hidden.set(true);
        let hidden = entry.view.wants_hidden();
        log::debug!("tab {tab:?}: hidden, painting stopped = {hidden}");
        self.with_host(tab, |host| host.was_hidden(i32::from(hidden)));
    }

    fn set_focused(&self, tab: TabId, focused: bool) {
        log::debug!("tab {tab:?}: focus = {focused}");
        if let Some(entry) = self.tabs.get(&tab) {
            entry.view.focused.set(focused);
            // The last focused *tab* stands as the opener of tabs Chromium
            // opens on its own; a side panel is none.
            if focused && !entry.view.is_panel() {
                self.delegate.set_last_focused(tab, &entry.browser);
            }
        }
        self.with_host(tab, |host| host.set_focus(i32::from(focused)));
    }

    /// Resize a view, if its size changed. A tab's size is the page area's,
    /// which all tabs of the window share; a side panel's view has its own.
    fn resize_content(&self, tab: TabId, size: PhysicalSize<u32>) {
        let resized = self
            .sizes
            .borrow_mut()
            .resize(tab, (size.width, size.height));
        let scale = self.scale.get();
        let (width, height) = self.sizes.borrow().of(tab);
        let dip = dip_size(width, height, scale);
        match resized {
            Resized::Unchanged => return,
            Resized::Page => {
                // Every tab, hidden ones included: a hidden tab picks the
                // size up through `view_rect` when it is shown.
                for entry in self.tabs.values().filter(|e| !e.view.is_panel()) {
                    entry.view.dip_size.set(dip);
                }
            }
            Resized::Panel => {
                if let Some(entry) = self.tabs.get(&tab) {
                    entry.view.dip_size.set(dip);
                }
            }
        }
        // Only the named view needs telling now.
        self.with_host(tab, |host| host.was_resized());
    }

    /// Where the page area sits on screen (logical pixels), so Chromium
    /// can place its menus and widgets.
    fn set_content_origin(&self, x: i32, y: i32) {
        self.origin.set((x, y));
    }

    fn set_hidpi(&self, scale: f32) {
        self.scale.set(scale);
        let sizes = self.sizes.borrow();
        for (id, entry) in &self.tabs {
            let (width, height) = sizes.of(*id);
            entry.view.scale.set(scale);
            entry.view.dip_size.set(dip_size(width, height, scale));
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
        if entry.view.drag.borrow().is_some() && self.drag_input(entry, &host, &input) {
            return;
        }
        let scale = self.scale.get();
        let to_dip = |v: f32| (v / scale).round() as i32;
        let mouse = |x: f32, y: f32, flags: u32| MouseEvent {
            x: to_dip(x),
            y: to_dip(y),
            modifiers: flags,
        };
        let buttons = self.button_flags.get();
        match input {
            EngineInput::MouseMove { x, y, modifiers } => {
                entry.view.last_pointer.set((to_dip(x), to_dip(y)));
                let flags = input::modifier_flags(modifiers) | buttons;
                host.send_mouse_move_event(Some(&mouse(x, y, flags)), 0);
            }
            EngineInput::MouseLeft => {
                let (x, y) = entry.view.last_pointer.get();
                let event = MouseEvent {
                    x,
                    y,
                    modifiers: buttons,
                };
                host.send_mouse_move_event(Some(&event), 1);
            }
            EngineInput::MouseButton {
                action,
                button,
                x,
                y,
                modifiers,
            } => {
                let Some(kind) = button_type(button) else {
                    return;
                };
                let keys = input::modifier_flags(modifiers);
                let (count, flags) = match action {
                    ButtonAction::Down => {
                        let count = self.click_count(tab, button, x, y);
                        self.button_flags
                            .set(self.button_flags.get() | input::button_flag(button));
                        (count, keys | self.button_flags.get())
                    }
                    ButtonAction::Up => {
                        // The flags of a release still name the button.
                        let flags = keys | self.button_flags.get();
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
            EngineInput::Wheel {
                dx,
                dy,
                x,
                y,
                modifiers,
            } => {
                let flags = input::modifier_flags(modifiers) | buttons;
                let (carry_x, carry_y) = self.wheel_carry.get();
                let (steps_x, carry_x) = input::wheel_steps(dx / f64::from(scale), carry_x);
                let (steps_y, carry_y) = input::wheel_steps(dy / f64::from(scale), carry_y);
                self.wheel_carry.set((carry_x, carry_y));
                if steps_x == 0 && steps_y == 0 {
                    return;
                }
                host.send_mouse_wheel_event(Some(&mouse(x, y, flags)), steps_x, steps_y);
            }
            EngineInput::Key(key) => {
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
                // "Replace nothing" is CEF's invalid range, said out loud:
                // with no range at all the call inserted nothing.
                let nothing = cef::Range {
                    from: u32::MAX,
                    to: u32::MAX,
                };
                host.ime_commit_text(Some(&CefString::from(text.as_str())), Some(&nothing), 0);
            }
            EngineInput::ImeDismissed => host.ime_cancel_composition(),
        }
    }

    /// A click on `extension`'s toolbar action: its popup under the
    /// button if it has one, otherwise the click itself, delivered
    /// to the extension's worker for `tab` through the bridge.
    fn open_extension_action(&self, tab: TabId, extension: &str, anchor: ScreenRect) {
        match self.extensions.popup_url(extension) {
            Some(url) => self
                .popups
                .open(extension, &url, anchor, self.scale.get(), tab),
            None => {
                if !self.extensions.click_action(extension, tab) {
                    log::warn!(
                        "extension {extension} has no popup and no bridge: its action does nothing yet"
                    );
                }
            }
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

    /// The user's answer to a page's request for the microphone or the
    /// camera: all that was asked for, or nothing. Chromium ignores an
    /// answer to a request the page has since abandoned.
    fn answer_media_access(&self, tab: TabId, request: u64, allow: bool) {
        let Some(entry) = self.tabs.get(&tab) else {
            return;
        };
        let held = entry.view.media_requests.borrow_mut().remove(&request);
        let Some((callback, permissions)) = held else {
            log::debug!("media request {request} is no longer held");
            return;
        };
        if allow {
            callback.cont(permissions);
        } else {
            callback.cancel();
        }
    }

    /// Chromium's find-in-page: it highlights every match, selects and
    /// scrolls to the active one, and reports through the find handler.
    fn find(&self, tab: TabId, text: &str, forward: bool, match_case: bool, find_next: bool) {
        self.with_host(tab, |host| {
            host.find(
                Some(&CefString::from(text)),
                i32::from(forward),
                i32::from(match_case),
                i32::from(find_next),
            );
        });
    }

    fn stop_finding(&self, tab: TabId, clear_selection: bool) {
        self.with_host(tab, |host| host.stop_finding(i32::from(clear_selection)));
    }

    fn start_download(&self, tab: TabId, url: Url) {
        self.with_host(tab, |host| {
            host.start_download(Some(&CefString::from(url.as_str())));
        });
    }

    /// Chromium's zoom level is logarithmic: each unit is a factor of
    /// 1.2, so 120% is level 1 and 100% level 0.
    fn set_zoom(&self, tab: TabId, percent: u16) {
        let level = (f64::from(percent) / 100.0).ln() / 1.2f64.ln();
        self.with_host(tab, |host| host.set_zoom_level(level));
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
        let dirty = frame.dirty_rects();
        let pixels = frame.pixels();
        sink(PagePaint::Frame(FrameRef {
            width,
            height,
            bgra: &pixels,
            fresh,
            dirty,
        }));
    }
}

impl Drop for TabHost {
    /// A window that closes takes its panels with it.
    fn drop(&mut self) {
        let views: Vec<TabId> = self.tabs.keys().copied().collect();
        for view in views {
            self.delegate.panel_gone(view);
            self.close_tab(view);
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

/// Whether a press of `button` at (`x`, `y`) in `view`, at `now`, continues
/// the click `prev`: the same view, button, close in time and space.
fn continues_click(
    prev: &LastClick,
    view: TabId,
    button: MouseButton,
    x: f32,
    y: f32,
    now: Instant,
) -> bool {
    prev.view == view
        && prev.button == button
        && now.duration_since(prev.at) <= MULTI_CLICK_TIME
        && (prev.x - x).abs() <= MULTI_CLICK_DISTANCE
        && (prev.y - y).abs() <= MULTI_CLICK_DISTANCE
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

#[cfg(test)]
mod tests {
    use super::*;

    fn press(view: u64, at: Instant, x: f32, y: f32) -> LastClick {
        LastClick {
            view: TabId(view),
            at,
            button: MouseButton::Primary,
            x,
            y,
            count: 1,
        }
    }

    #[test]
    fn a_press_in_the_same_view_continues_the_click() {
        let start = Instant::now();
        let prev = press(1, start, 10.0, 10.0);
        let later = start + Duration::from_millis(100);
        assert!(continues_click(
            &prev,
            TabId(1),
            MouseButton::Primary,
            10.0,
            10.0,
            later
        ));
    }

    #[test]
    fn a_press_at_the_same_point_of_another_view_starts_a_new_click() {
        let start = Instant::now();
        let prev = press(1, start, 10.0, 10.0);
        let later = start + Duration::from_millis(100);
        assert!(!continues_click(
            &prev,
            TabId(2),
            MouseButton::Primary,
            10.0,
            10.0,
            later
        ));
    }

    #[test]
    fn a_press_late_far_or_with_another_button_starts_a_new_click() {
        let start = Instant::now();
        let prev = press(1, start, 10.0, 10.0);
        let late = start + MULTI_CLICK_TIME + Duration::from_millis(1);
        assert!(!continues_click(
            &prev,
            TabId(1),
            MouseButton::Primary,
            10.0,
            10.0,
            late
        ));
        let far = 10.0 + MULTI_CLICK_DISTANCE + 1.0;
        assert!(!continues_click(
            &prev,
            TabId(1),
            MouseButton::Primary,
            far,
            10.0,
            start
        ));
        assert!(!continues_click(
            &prev,
            TabId(1),
            MouseButton::Secondary,
            10.0,
            10.0,
            start
        ));
    }
}
