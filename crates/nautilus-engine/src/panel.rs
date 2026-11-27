//! The side panel's view: one more windowless browser of a window, under
//! a `TabId` that is in no strip. The pure half (the page's address, the
//! sizes, what a panel reports, the open panels) is here with its tests;
//! the browser itself is made by `TabHost::create_view` in `host.rs`.

use std::collections::HashMap;

use osmena_core::{EngineEvent, ExtensionRequest, TabId, WindowId};
use url::Url;

/// The panel's width before the shell's first `resize_content`, in
/// logical pixels. The shell sizes the view on the next frame.
#[allow(dead_code)] // Task 6 (`host.rs`) is the first user.
pub(crate) const FIRST_WIDTH_DIP: f32 = 360.0;

/// A side panel path as the model carries it (`SidePanel::page`,
/// `ExtensionRequest::OpenSidePanel::page`): relative to the extension's
/// root, no leading slash, query and fragment kept.
pub(crate) fn normalize_page(path: &str) -> String {
    path.trim().trim_start_matches('/').to_string()
}

/// The address a panel's browser loads: `page` under the extension's
/// root. `None` when `page` leads anywhere else (another scheme or host).
#[allow(dead_code)] // Task 6 (`host.rs` `create_view`) is the first user.
pub(crate) fn page_url(extension: &str, page: &str) -> Option<Url> {
    let root = Url::parse(&format!("chrome-extension://{extension}/")).ok()?;
    let url = root.join(&normalize_page(page)).ok()?;
    (url.scheme() == "chrome-extension" && url.host_str() == Some(extension)).then_some(url)
}

/// A view's size in logical pixels.
#[allow(dead_code)] // Task 6 (`host.rs`) is the first user.
pub(crate) fn dip_size(width: u32, height: u32, scale: f32) -> (i32, i32) {
    (
        ((width as f32 / scale).round() as i32).max(1),
        ((height as f32 / scale).round() as i32).max(1),
    )
}

/// A point of a view on screen, both in logical pixels: what Chromium
/// asks before it places a `<select>` list or reports `screenX`.
pub(crate) fn screen_point(origin: (i32, i32), view: (i32, i32)) -> (i32, i32) {
    (origin.0 + view.0, origin.1 + view.1)
}

/// What a `resize_content` changed.
#[allow(dead_code)] // Task 6 (`host.rs` `resize_content`) is the first user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resized {
    Unchanged,
    /// The page area: every tab of the window takes the size.
    Page,
    /// One panel's view.
    Panel,
}

/// The sizes of a window's views in physical pixels: one for the page
/// area, which every tab shares (a hidden tab must have it when it is
/// shown), and one for each panel view.
#[allow(dead_code)] // Task 6 (`host.rs`) is the first user.
#[derive(Debug)]
pub(crate) struct ViewSizes {
    page: (u32, u32),
    panels: HashMap<TabId, (u32, u32)>,
}

#[allow(dead_code)] // Task 6 (`host.rs`) is the first user.
impl ViewSizes {
    pub(crate) fn new(page: (u32, u32)) -> ViewSizes {
        ViewSizes {
            page,
            panels: HashMap::new(),
        }
    }

    pub(crate) fn page(&self) -> (u32, u32) {
        self.page
    }

    pub(crate) fn is_panel(&self, view: TabId) -> bool {
        self.panels.contains_key(&view)
    }

    /// A panel view joins at the first width and the page's height.
    pub(crate) fn add_panel(&mut self, view: TabId, scale: f32) -> (u32, u32) {
        let size = (
            ((FIRST_WIDTH_DIP * scale).round() as u32).max(1),
            self.page.1,
        );
        self.panels.insert(view, size);
        size
    }

    pub(crate) fn remove_panel(&mut self, view: TabId) {
        self.panels.remove(&view);
    }

    /// The size of `view`: its own for a panel, the page's for a tab.
    pub(crate) fn of(&self, view: TabId) -> (u32, u32) {
        self.panels.get(&view).copied().unwrap_or(self.page)
    }

    pub(crate) fn resize(&mut self, view: TabId, size: (u32, u32)) -> Resized {
        let size = (size.0.max(1), size.1.max(1));
        match self.panels.get_mut(&view) {
            Some(own) if *own == size => Resized::Unchanged,
            Some(own) => {
                *own = size;
                Resized::Panel
            }
            None if self.page == size => Resized::Unchanged,
            None => {
                self.page = size;
                Resized::Page
            }
        }
    }
}

/// Whether a panel's view tells the model `event`. A panel is no tab:
/// its address, title, load state, history, status text, favicon, find
/// results, blocked navigations, media requests, media capture and context
/// menu are nobody's. Its pixels and cursor are the shell's, and its end
/// (the page closed itself, the renderer died) is how the model learns the
/// panel is gone.
pub(crate) fn panel_reports(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::FrameReady(_)
            | EngineEvent::CursorChanged { .. }
            | EngineEvent::TabClosed(_)
            | EngineEvent::TabCrashed { .. }
    )
}

/// A panel whose view exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenPanel {
    pub view: TabId,
    pub extension: String,
    pub page: String,
    pub window: WindowId,
    /// The tab the panel belongs to; `None` for the window's panel.
    pub tab: Option<TabId>,
}

/// What became of a panel, for `chrome.sidePanel.onOpened`/`onClosed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelNotice {
    Opened(OpenPanel),
    Closed(OpenPanel),
}

#[derive(Debug)]
struct Expected {
    token: u64,
    extension: String,
    page: String,
    window: WindowId,
    tab: Option<TabId>,
}

/// The panels the engine has a view for, and the `OpenSidePanel`
/// requests whose view has not been asked for yet. The model allocates
/// the view's id and `create_view` names only the extension and the
/// page, so the request is remembered until `create_view` claims it.
#[derive(Debug, Default)]
pub(crate) struct PanelRegistry {
    expected: Vec<Expected>,
    open: Vec<OpenPanel>,
}

impl PanelRegistry {
    /// The engine is about to report `request` under `token`.
    pub(crate) fn expect(&mut self, token: u64, extension: &str, request: &ExtensionRequest) {
        if let ExtensionRequest::OpenSidePanel { window, tab, page } = request {
            self.expected.push(Expected {
                token,
                extension: extension.to_string(),
                page: page.clone(),
                window: *window,
                tab: *tab,
            });
        }
    }

    /// `create_view` arrived: the oldest expected panel of `extension`
    /// at `page` is `view`.
    pub(crate) fn bind(&mut self, view: TabId, extension: &str, page: &str) -> Option<OpenPanel> {
        let at = self
            .expected
            .iter()
            .position(|e| e.extension == extension && e.page == page)?;
        let expected = self.expected.remove(at);
        let panel = OpenPanel {
            view,
            extension: expected.extension,
            page: expected.page,
            window: expected.window,
            tab: expected.tab,
        };
        self.open.push(panel.clone());
        Some(panel)
    }

    /// The model answered the request under `token`: if no view was
    /// asked for (refused, or the same panel was already there), nothing
    /// is expected any more.
    pub(crate) fn settle(&mut self, token: u64) {
        self.expected.retain(|e| e.token != token);
    }

    pub(crate) fn remove(&mut self, view: TabId) -> Option<OpenPanel> {
        let at = self.open.iter().position(|p| p.view == view)?;
        Some(self.open.remove(at))
    }

    pub(crate) fn get(&self, view: TabId) -> Option<&OpenPanel> {
        self.open.iter().find(|p| p.view == view)
    }

    /// The window hosting the panel whose view is `view`: the "current
    /// window" of a call made from the panel's page.
    #[allow(dead_code)] // Task 7 (`bridge::hub`, the panel's window) is the first user.
    pub(crate) fn window_of(&self, view: TabId) -> Option<WindowId> {
        self.get(view).map(|p| p.window)
    }

    pub(crate) fn all(&self) -> impl Iterator<Item = &OpenPanel> {
        self.open.iter()
    }

    pub(crate) fn of_extension<'a>(
        &'a self,
        extension: &'a str,
    ) -> impl Iterator<Item = &'a OpenPanel> {
        self.open.iter().filter(move |p| p.extension == extension)
    }

    /// `extension`'s panel in that place: `tab`'s own, or the window's.
    pub(crate) fn at(
        &self,
        extension: &str,
        window: WindowId,
        tab: Option<TabId>,
    ) -> Option<&OpenPanel> {
        self.open
            .iter()
            .find(|p| p.extension == extension && p.window == window && p.tab == tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[test]
    fn a_panel_page_is_addressed_under_its_extensions_root() {
        assert_eq!(
            page_url(EXT, "panel.html").unwrap().as_str(),
            format!("chrome-extension://{EXT}/panel.html")
        );
        assert_eq!(
            page_url(EXT, "/ui/panel.html?tab=5#top").unwrap().as_str(),
            format!("chrome-extension://{EXT}/ui/panel.html?tab=5#top")
        );
        assert_eq!(normalize_page(" /panel.html "), "panel.html");
        assert_eq!(normalize_page("//panel.html"), "panel.html");
    }

    #[test]
    fn a_page_that_leads_out_of_the_extension_has_no_address() {
        assert_eq!(page_url(EXT, "https://example.com/"), None);
        assert_eq!(page_url(EXT, "javascript:alert(1)"), None);
        assert_eq!(page_url(EXT, "chrome-extension://other/panel.html"), None);
        // Dots cannot climb out of the root.
        assert_eq!(
            page_url(EXT, "../../panel.html").unwrap().as_str(),
            format!("chrome-extension://{EXT}/panel.html")
        );
    }

    #[test]
    fn logical_sizes_round_and_never_reach_zero() {
        assert_eq!(dip_size(720, 1200, 2.0), (360, 600));
        assert_eq!(dip_size(541, 901, 1.5), (361, 601));
        assert_eq!(dip_size(0, 0, 2.0), (1, 1));
    }

    #[test]
    fn a_views_point_on_screen_is_its_origin_plus_the_point() {
        // The page starts at the window's content corner; the panel
        // starts a page's width to the right, under its header.
        assert_eq!(screen_point((100, 180), (10, 20)), (110, 200));
        assert_eq!(screen_point((100 + 920, 180 + 36), (10, 20)), (1030, 236));
    }

    #[test]
    fn tabs_share_the_page_size_and_a_panel_keeps_its_own() {
        let (tab, other, panel) = (TabId(1), TabId(2), TabId(9));
        let mut sizes = ViewSizes::new((1024, 768));
        assert_eq!(sizes.add_panel(panel, 2.0), (720, 768));
        assert!(sizes.is_panel(panel));
        assert!(!sizes.is_panel(tab));

        // The page narrows for the panel: every tab has the new size,
        // the hidden one included, and the panel is untouched.
        assert_eq!(sizes.resize(tab, (1200, 1400)), Resized::Page);
        assert_eq!(sizes.of(tab), (1200, 1400));
        assert_eq!(sizes.of(other), (1200, 1400));
        assert_eq!(sizes.of(panel), (720, 768));

        // The panel's own size leaves the page alone.
        assert_eq!(sizes.resize(panel, (720, 1328)), Resized::Panel);
        assert_eq!(sizes.of(panel), (720, 1328));
        assert_eq!(sizes.page(), (1200, 1400));

        // The shell says both every frame; an unchanged size is nothing.
        assert_eq!(sizes.resize(tab, (1200, 1400)), Resized::Unchanged);
        assert_eq!(sizes.resize(panel, (720, 1328)), Resized::Unchanged);
        assert_eq!(sizes.resize(panel, (0, 0)), Resized::Panel);
        assert_eq!(sizes.of(panel), (1, 1));

        // Once the panel is gone its id is sized like a tab's.
        sizes.remove_panel(panel);
        assert_eq!(sizes.of(panel), (1200, 1400));
    }

    #[test]
    fn a_panel_reports_its_pixels_its_cursor_and_its_end_and_nothing_else() {
        let view = TabId(9);
        assert!(panel_reports(&EngineEvent::FrameReady(view)));
        assert!(panel_reports(&EngineEvent::TabClosed(view)));
        assert!(panel_reports(&EngineEvent::TabCrashed {
            tab: view,
            reason: "crashed".to_string(),
        }));
        assert!(!panel_reports(&EngineEvent::TitleChanged {
            tab: view,
            title: "Panel".to_string(),
        }));
    }

    fn open(window: u64, tab: Option<u64>, page: &str) -> ExtensionRequest {
        ExtensionRequest::OpenSidePanel {
            window: WindowId(window),
            tab: tab.map(TabId),
            page: page.to_string(),
        }
    }

    #[test]
    fn create_view_claims_the_request_that_asked_for_it() {
        let mut panels = PanelRegistry::default();
        panels.expect(1, EXT, &open(1, None, "panel.html"));
        panels.expect(2, EXT, &open(1, Some(4), "panel.html?tab=4"));
        // Not a panel: nothing is expected for it.
        panels.expect(3, EXT, &ExtensionRequest::CloseTabs(Vec::new()));

        let tab_panel = panels.bind(TabId(21), EXT, "panel.html?tab=4").unwrap();
        assert_eq!(tab_panel.tab, Some(TabId(4)));
        let window_panel = panels.bind(TabId(20), EXT, "panel.html").unwrap();
        assert_eq!(window_panel.tab, None);
        assert_eq!(panels.bind(TabId(22), EXT, "panel.html"), None);

        assert_eq!(panels.window_of(TabId(21)), Some(WindowId(1)));
        assert_eq!(panels.at(EXT, WindowId(1), None).unwrap().view, TabId(20));
        assert_eq!(
            panels.at(EXT, WindowId(1), Some(TabId(4))).unwrap().view,
            TabId(21)
        );
        assert_eq!(panels.at("other", WindowId(1), None), None);
        assert_eq!(panels.of_extension(EXT).count(), 2);

        assert_eq!(panels.remove(TabId(20)), Some(window_panel));
        assert_eq!(panels.remove(TabId(20)), None);
        assert_eq!(panels.window_of(TabId(20)), None);
    }

    #[test]
    fn a_request_the_model_answered_without_a_view_is_forgotten() {
        let mut panels = PanelRegistry::default();
        panels.expect(7, EXT, &open(1, None, "panel.html"));
        // Refused, or the same panel was already open: no `create_view`.
        panels.settle(7);
        assert_eq!(panels.bind(TabId(20), EXT, "panel.html"), None);

        // Answered after its view was made: the panel stays.
        panels.expect(8, EXT, &open(1, None, "panel.html"));
        assert!(panels.bind(TabId(20), EXT, "panel.html").is_some());
        panels.settle(8);
        assert!(panels.get(TabId(20)).is_some());
    }
}
