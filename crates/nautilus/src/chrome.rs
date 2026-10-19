//! The window chrome: tab strip, toolbar, status overlay, and crash panel.
//! Pure functions of the model that return the commands the user issued.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use egui::{
    Align2, Button, Color32, ColorImage, CornerRadius, Frame, Id, Key, Margin, Panel, Pos2, Rect,
    RichText, Sense, Stroke, StrokeKind, TextStyle, TextureHandle, TextureOptions, Ui, Vec2,
};
use keyboard_types::Code;
use nautilus_core::{
    ChromeColor, Command, CommandKind, Favicon, KeyChord, Keymap, LoadState, TabId, Window,
};
use url::{ParseError, Url};

const TAB_TITLE_MAX_CHARS: usize = 24;
const FAVICON_SIZE: f32 = 16.0;
/// Tab geometry, in points.
const TAB_HEIGHT: f32 = 32.0;
const TAB_PADDING_X: f32 = 12.0;
const TAB_CORNER: u8 = 8;
const CLOSE_SIZE: f32 = 18.0;

/// The shades the chrome paints with, all derived from the profile's
/// [`ChromeColor`]: the tab strip is the swatch itself, tabs are veils of
/// white (or black, on a light swatch) over it, and the active tab shares
/// the toolbar's fill, which is the swatch washed most of the way to the
/// page background, so it reads as attached to the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub strip: Color32,
    pub toolbar: Color32,
    pub text_on_strip: Color32,
    pub tab_idle: Color32,
    pub tab_hover: Color32,
    pub tab_selected: Color32,
    pub tab_outline: Color32,
}

impl Palette {
    pub fn new(color: ChromeColor, dark_mode: bool) -> Palette {
        let [r, g, b] = color.rgb();
        let strip = Color32::from_rgb(r, g, b);
        let page = if dark_mode {
            Color32::from_gray(32)
        } else {
            Color32::WHITE
        };
        let light_strip = luminance(strip) > 0.55;
        let veil = if light_strip {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        let veil_alpha = |a: u8| Color32::from_rgba_unmultiplied(veil.r(), veil.g(), veil.b(), a);
        Palette {
            strip,
            toolbar: blend(strip, page, 0.84),
            text_on_strip: if light_strip {
                Color32::from_gray(24)
            } else {
                Color32::WHITE
            },
            tab_idle: veil_alpha(26),
            tab_hover: veil_alpha(56),
            tab_selected: veil_alpha(96),
            tab_outline: veil_alpha(180),
        }
    }
}

/// `a` moved `t` of the way to `b`, per channel.
fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// Relative luminance in 0..1, from gamma-space channels (close enough to
/// pick a text colour).
fn luminance(c: Color32) -> f32 {
    (0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())) / 255.0
}

/// Per-window chrome state that is not part of the browser model: the
/// address bar text the user may be editing, and favicon textures.
#[derive(Default)]
pub struct ChromeState {
    pub address_text: String,
    /// True while the user has edited the address bar since the last
    /// navigation; engine URL changes then leave the text alone.
    pub address_dirty: bool,
    /// Set by `Command::FocusAddressBar`; consumed by the next frame.
    pub focus_address: bool,
    shown_tab: Option<TabId>,
    shown_url: Option<Url>,
    favicons: HashMap<TabId, CachedFavicon>,
    /// Set when a drag of tabs ends; the window takes it and works out
    /// where the tabs were dropped.
    tab_drop: Option<TabDrop>,
    /// Set when the user asks for a menu; the window shows it as a native
    /// popup (`native_menu`) once the frame is painted.
    menu_request: Option<MenuRequest>,
}

impl ChromeState {
    /// The tab drag that ended since the last call, if any.
    pub fn take_tab_drop(&mut self) -> Option<TabDrop> {
        self.tab_drop.take()
    }

    /// The menu the user asked for since the last call, if any.
    pub fn take_menu_request(&mut self) -> Option<MenuRequest> {
        self.menu_request.take()
    }
}

/// A menu the chrome wants shown, with where to put it (egui points of
/// this window).
#[derive(Clone, Debug, PartialEq)]
pub enum MenuRequest {
    /// The "☰" window menu, below its button.
    Window { position: Pos2 },
    /// A tab's context menu, acting on `targets`, at the pointer.
    Tab { targets: Vec<TabId>, position: Pos2 },
}

/// A drag of tabs that ended: the tabs to move and where the pointer was
/// when the button was released, in egui points of this window (which may
/// be outside the window while the drag held the pointer).
#[derive(Clone, Debug, PartialEq)]
pub struct TabDrop {
    pub tabs: Vec<TabId>,
    pub pointer: Pos2,
}

/// An uploaded favicon texture. The `Arc` is a clone of the model's pixels:
/// holding it keeps that allocation alive, so pointer equality with a new
/// favicon's pixels means the same icon, never a reused address.
struct CachedFavicon {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
    texture: TextureHandle,
}

/// What the chrome needs besides the window it draws.
#[derive(Clone, Copy)]
pub struct ChromeContext<'a> {
    /// The profile's chrome colour.
    pub color: ChromeColor,
    /// The browser-wide engine error to show in the status area, if any.
    pub engine_error: Option<&'a str>,
}

/// Draw the chrome for `window` and return the commands it produced.
pub fn draw(
    ui: &mut Ui,
    window: &Window,
    context: &ChromeContext<'_>,
    state: &mut ChromeState,
    keymap: &Keymap,
) -> Vec<Command> {
    let ChromeContext {
        color,
        engine_error,
    } = *context;
    let mut commands = Vec::new();
    let active = window.active_tab();
    sync_address_text(state, active.id, &active.url);
    state
        .favicons
        .retain(|tab, _| window.index_of(*tab).is_some());
    let palette = Palette::new(color, ui.visuals().dark_mode);

    let strip_frame = Frame::new().fill(palette.strip).inner_margin(Margin {
        left: 8,
        right: 8,
        top: 6,
        bottom: 0,
    });
    Panel::top("tabs").frame(strip_frame).show_inside(ui, |ui| {
        egui::ScrollArea::horizontal()
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    // How many tabs a drag in progress carries, for the overlay.
                    let mut dragging: Option<usize> = None;
                    for tab in &window.tabs {
                        let is_active = tab.id == window.active;
                        let is_selected = window.is_selected(tab.id);
                        let favicon = favicon_texture(ui, state, tab.id, tab.favicon.as_ref())
                            .map(|t| t.id());
                        let title = truncate(tab.display_title(), TAB_TITLE_MAX_CHARS);
                        let (response, close_clicked) =
                            tab_widget(ui, &palette, is_active, is_selected, favicon, &title);
                        if close_clicked {
                            commands.push(Command::CloseTab(tab.id));
                        }
                        // A plain click activates; Ctrl (Cmd on macOS) adds
                        // or removes; Shift extends from the active tab.
                        let modifiers = ui.input(|i| i.modifiers);
                        if response.clicked() {
                            if modifiers.shift {
                                commands.push(Command::SelectTabRange(tab.id));
                            } else if modifiers.command {
                                commands.push(Command::ToggleTabSelection(tab.id));
                            } else if !is_active || window.selected.len() > 1 {
                                commands.push(Command::ActivateTab(tab.id));
                            }
                        }
                        if response.middle_clicked() {
                            commands.push(Command::CloseTab(tab.id));
                        }
                        // The context menu and a drag act on the whole
                        // selection when this tab is part of it; otherwise
                        // on this tab alone, which becomes the active one.
                        let targets = if is_selected {
                            window.selection()
                        } else {
                            vec![tab.id]
                        };
                        if response.secondary_clicked() {
                            if !is_selected {
                                commands.push(Command::ActivateTab(tab.id));
                            }
                            state.menu_request = Some(MenuRequest::Tab {
                                targets: targets.clone(),
                                position: response
                                    .interact_pointer_pos()
                                    .unwrap_or_else(|| response.rect.left_bottom()),
                            });
                        }
                        if response.drag_started() && !is_selected {
                            commands.push(Command::ActivateTab(tab.id));
                        }
                        if response.dragged() {
                            dragging = Some(targets.len());
                        }
                        if response.drag_stopped()
                            && let Some(pointer) = response.interact_pointer_pos()
                        {
                            state.tab_drop = Some(TabDrop {
                                tabs: targets,
                                pointer,
                            });
                        }
                    }
                    if let (Some(count), Some(pointer)) = (dragging, ui.ctx().pointer_latest_pos())
                    {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        egui::Area::new(Id::new("tab-drag-overlay"))
                            .order(egui::Order::Tooltip)
                            .fixed_pos(pointer + Vec2::new(12.0, 12.0))
                            .interactable(false)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::popup(ui.style()).show(ui, |ui| {
                                    ui.label(if count == 1 {
                                        "Move 1 tab".to_string()
                                    } else {
                                        format!("Move {count} tabs")
                                    });
                                });
                            });
                    }
                    ui.add_space(4.0);
                    let on_strip = |text: &str, size: f32| {
                        Button::new(RichText::new(text).color(palette.text_on_strip).size(size))
                            .frame(false)
                            .min_size(Vec2::new(28.0, TAB_HEIGHT))
                    };
                    if ui
                        .add(on_strip("+", 20.0))
                        .on_hover_text(hint(keymap, CommandKind::NewTab, "New tab"))
                        .clicked()
                    {
                        commands.push(Command::NewTab {
                            window: window.id,
                            url: None,
                        });
                    }
                    let menu = ui.add(on_strip("☰", 16.0)).on_hover_text("Menu");
                    if menu.clicked() {
                        state.menu_request = Some(MenuRequest::Window {
                            position: menu.rect.left_bottom(),
                        });
                    }
                });
            });
    });

    let toolbar_frame = Frame::new()
        .fill(palette.toolbar)
        .inner_margin(Margin::symmetric(8, 6));
    Panel::top("toolbar")
        .frame(toolbar_frame)
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(active.can_go_back, Button::new("◀"))
                    .on_hover_text(hint(keymap, CommandKind::Back, "Back"))
                    .clicked()
                {
                    commands.push(Command::Back(active.id));
                }
                if ui
                    .add_enabled(active.can_go_forward, Button::new("▶"))
                    .on_hover_text(hint(keymap, CommandKind::Forward, "Forward"))
                    .clicked()
                {
                    commands.push(Command::Forward(active.id));
                }
                // Reload is always available (a hung load must be recoverable);
                // Stop appears beside it while loading.
                if ui
                    .button("⟳")
                    .on_hover_text(hint(keymap, CommandKind::Reload, "Reload"))
                    .clicked()
                {
                    state.address_dirty = false;
                    commands.push(Command::Reload(active.id));
                }
                if active.load == LoadState::Loading
                    && ui
                        .button("✕")
                        .on_hover_text(hint(keymap, CommandKind::Stop, "Stop"))
                        .clicked()
                {
                    commands.push(Command::Stop(active.id));
                }

                let address_id = Id::new(("address", window.id));
                let field = ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::singleline(&mut state.address_text)
                        .id(address_id)
                        .hint_text("Enter address"),
                );
                if field.changed() {
                    state.address_dirty = true;
                }
                // Select the text on focus, and again on Cmd+L while already
                // focused, so the shortcut always means "replace the address".
                let focus_requested = state.focus_address;
                if state.focus_address {
                    state.focus_address = false;
                    field.request_focus();
                }
                if field.gained_focus() || focus_requested {
                    select_all(ui, address_id, state.address_text.chars().count());
                }
                if field.lost_focus()
                    && ui.input(|i| i.key_pressed(Key::Enter))
                    && let Some(url) = location_input_to_url(&state.address_text)
                {
                    state.address_dirty = false;
                    commands.push(Command::Navigate {
                        tab: active.id,
                        url,
                    });
                }
            });
        });

    // The status text floats over the bottom-left corner of the page as an
    // overlay, so showing or hiding it never resizes the page.
    let status = match engine_error {
        Some(error) => Some(format!("Engine error: {error}")),
        None if !active.status.is_empty() => Some(active.status.clone()),
        None => None,
    };
    if let Some(text) = status {
        egui::Area::new(Id::new(("status", window.id)))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .interactable(engine_error.is_some())
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(text).small());
                        if engine_error.is_some()
                            && ui.small_button("×").on_hover_text("Dismiss").clicked()
                        {
                            commands.push(Command::DismissEngineError);
                        }
                    });
                });
            });
    }

    commands
}

/// The colour the framebuffer is cleared to before egui paints: the panel
/// fill, in the gamma-space components egui_glow's painter expects.
/// The colour the window is cleared to before the chrome paints: the
/// toolbar fill for `color`, so nothing flashes a different shade.
pub fn background(ctx: &egui::Context, color: ChromeColor) -> [f32; 4] {
    Palette::new(color, ctx.global_style().visuals.dark_mode)
        .toolbar
        .to_normalized_gamma_f32()
}

/// One tab: a rounded-top block with the favicon, the title, and a close
/// control, filled per its state. Returns the tab's response (click, drag,
/// context menu) and whether the close control was clicked.
fn tab_widget(
    ui: &mut Ui,
    palette: &Palette,
    is_active: bool,
    is_selected: bool,
    favicon: Option<egui::TextureId>,
    title: &str,
) -> (egui::Response, bool) {
    let font = TextStyle::Body.resolve(ui.style());
    let text_color = if is_active {
        ui.visuals().text_color()
    } else {
        palette.text_on_strip
    };
    let galley = ui
        .painter()
        .layout_no_wrap(title.to_string(), font.clone(), text_color);
    let icon_width = if favicon.is_some() {
        FAVICON_SIZE + 6.0
    } else {
        0.0
    };
    let width = TAB_PADDING_X + icon_width + galley.size().x + 8.0 + CLOSE_SIZE + TAB_PADDING_X;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, TAB_HEIGHT), Sense::click_and_drag());
    let close_rect = Rect::from_center_size(
        Pos2::new(
            rect.right() - TAB_PADDING_X - CLOSE_SIZE / 2.0 + 4.0,
            rect.center().y,
        ),
        Vec2::splat(CLOSE_SIZE),
    );
    // Added after the tab, so it is on top and takes the pointer over it.
    let close = ui.interact(close_rect, response.id.with("close"), Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if is_active {
            palette.toolbar
        } else if is_selected {
            palette.tab_selected
        } else if response.hovered() || response.dragged() {
            palette.tab_hover
        } else {
            palette.tab_idle
        };
        let stroke = if is_selected && !is_active {
            Stroke::new(1.0_f32, palette.tab_outline)
        } else {
            Stroke::NONE
        };
        let corners = CornerRadius {
            nw: TAB_CORNER,
            ne: TAB_CORNER,
            sw: 0,
            se: 0,
        };
        painter.rect(rect, corners, fill, stroke, StrokeKind::Inside);

        let mut x = rect.left() + TAB_PADDING_X;
        if let Some(texture) = favicon {
            let icon = Rect::from_center_size(
                Pos2::new(x + FAVICON_SIZE / 2.0, rect.center().y),
                Vec2::splat(FAVICON_SIZE),
            );
            painter.image(
                texture,
                icon,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            x += icon_width;
        }
        painter.galley(
            Pos2::new(x, rect.center().y - galley.size().y / 2.0),
            galley,
            text_color,
        );

        if close.hovered() {
            let ring = if is_active {
                Color32::from_black_alpha(28)
            } else {
                palette.tab_hover
            };
            painter.circle_filled(close_rect.center(), CLOSE_SIZE / 2.0, ring);
        }
        painter.text(
            close_rect.center(),
            Align2::CENTER_CENTER,
            "×",
            font,
            text_color,
        );
    }
    let close = close.on_hover_text("Close tab");
    (response, close.clicked())
}

/// Replace the page area with an error panel. Returns true if the user
/// asked to reload.
pub fn draw_crash_panel(ui: &mut Ui, rect: Rect, reason: &str) -> bool {
    let mut reload = false;
    // The page is not blitted while crashed, so the panel fills its own
    // area; nothing else paints there.
    ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("This tab crashed");
            ui.label(reason);
            ui.add_space(12.0);
            if ui.button("Reload").clicked() {
                reload = true;
            }
        });
    });
    reload
}

/// Turn address-bar text into a URL. Bare domains get `https://`. `localhost`,
/// bare IPv4 addresses, and `host:port` get `http://`. `javascript:` URLs
/// are refused. Returns `None` for text that is not a URL (Phase 1 has no
/// search engine).
pub fn location_input_to_url(input: &str) -> Option<Url> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    match Url::parse(text) {
        Ok(url) if url.scheme() == "javascript" => None,
        // "localhost:8080" and "example.com:8080" parse as scheme + path.
        Ok(url)
            if url.host_str().is_none()
                && (url.scheme() == "localhost" || url.scheme().contains('.')) =>
        {
            with_default_scheme(text)
        }
        Ok(url) => Some(url),
        Err(ParseError::RelativeUrlWithoutBase) if looks_like_host(text) => {
            with_default_scheme(text)
        }
        Err(_) => None,
    }
}

/// `text` with the scheme `default_scheme` picks for it, parsed.
fn with_default_scheme(text: &str) -> Option<Url> {
    Url::parse(&format!("{}://{text}", default_scheme(text))).ok()
}

/// `http` for `localhost`, bare IPv4 addresses, and `host:port`; `https` for
/// everything else.
fn default_scheme(text: &str) -> &'static str {
    let (host, port) = split_port(authority(text));
    if host == "localhost" || host.parse::<Ipv4Addr>().is_ok() || port.is_some() {
        "http"
    } else {
        "https"
    }
}

fn looks_like_host(text: &str) -> bool {
    let (host, _) = split_port(authority(text));
    !text.contains(char::is_whitespace)
        && !text.starts_with('.')
        && (host == "localhost" || host.contains('.'))
}

/// The part of address-bar text before any path, query, or fragment, such as
/// `host` or `host:port`.
fn authority(text: &str) -> &str {
    text.split(['/', '?', '#']).next().unwrap_or(text)
}

/// Splits `host:port` when the part after the last colon is all digits.
fn split_port(authority: &str) -> (&str, Option<&str>) {
    match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            (host, Some(port))
        }
        _ => (authority, None),
    }
}

fn sync_address_text(state: &mut ChromeState, tab: TabId, url: &Url) {
    let tab_changed = state.shown_tab != Some(tab);
    let url_changed = state.shown_url.as_ref() != Some(url);
    if tab_changed || (url_changed && !state.address_dirty) {
        state.address_text = url.to_string();
        state.address_dirty = false;
    }
    state.shown_tab = Some(tab);
    state.shown_url = Some(url.clone());
}

fn select_all(ui: &Ui, id: Id, len: usize) {
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    if let Some(mut edit) = TextEditState::load(ui.ctx(), id) {
        edit.cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(len))));
        edit.store(ui.ctx(), id);
    }
}

fn favicon_texture<'a>(
    ui: &Ui,
    state: &'a mut ChromeState,
    tab: TabId,
    favicon: Option<&Favicon>,
) -> Option<&'a TextureHandle> {
    let favicon = favicon?;
    let fresh = state
        .favicons
        .get(&tab)
        .is_some_and(|cached| cached.is_for(favicon));
    if !fresh {
        let image = ColorImage::from_rgba_unmultiplied(
            [favicon.width as usize, favicon.height as usize],
            &favicon.rgba,
        );
        let texture =
            ui.ctx()
                .load_texture(format!("favicon-{}", tab.0), image, TextureOptions::LINEAR);
        state.favicons.insert(
            tab,
            CachedFavicon {
                width: favicon.width,
                height: favicon.height,
                rgba: favicon.rgba.clone(),
                texture,
            },
        );
    }
    state.favicons.get(&tab).map(|cached| &cached.texture)
}

impl CachedFavicon {
    /// True if this texture was uploaded from `favicon`'s pixels. The model
    /// replaces the `Arc` whenever the engine reports a new icon, so the
    /// pointer comparison is exact and costs nothing per frame.
    fn is_for(&self, favicon: &Favicon) -> bool {
        self.width == favicon.width
            && self.height == favicon.height
            && Arc::ptr_eq(&self.rgba, &favicon.rgba)
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

/// A label with its shortcut, like "New tab (Cmd+T)".
fn hint(keymap: &Keymap, kind: CommandKind, label: &str) -> String {
    match keymap.chord_for(kind) {
        Some(chord) => format!("{label} ({})", describe_chord(chord)),
        None => label.to_string(),
    }
}

/// A readable name for a chord: modifiers in Ctrl, Alt, Shift, Cmd order,
/// then the key.
fn describe_chord(chord: KeyChord) -> String {
    use keyboard_types::Modifiers;
    let mut parts: Vec<String> = Vec::new();
    if chord.modifiers.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if chord.modifiers.contains(Modifiers::ALT) {
        parts.push(
            if cfg!(target_os = "macos") {
                "Option"
            } else {
                "Alt"
            }
            .to_string(),
        );
    }
    if chord.modifiers.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if chord.modifiers.contains(Modifiers::META) {
        parts.push(
            if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Win"
            }
            .to_string(),
        );
    }
    parts.push(key_name(chord.code));
    parts.join("+")
}

fn key_name(code: Code) -> String {
    let name = match code {
        Code::BracketLeft => "[",
        Code::BracketRight => "]",
        Code::Comma => ",",
        Code::Period => ".",
        Code::ArrowLeft => "Left",
        Code::ArrowRight => "Right",
        Code::ArrowUp => "Up",
        Code::ArrowDown => "Down",
        Code::Escape => "Esc",
        other => {
            let debug = format!("{other:?}");
            return debug
                .strip_prefix("Key")
                .or_else(|| debug.strip_prefix("Digit"))
                .unwrap_or(&debug)
                .to_string();
        }
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_types::Modifiers;

    #[test]
    fn full_urls_pass_through() {
        assert_eq!(
            location_input_to_url("https://example.com/a?b=c")
                .unwrap()
                .as_str(),
            "https://example.com/a?b=c"
        );
        assert_eq!(
            location_input_to_url("about:blank").unwrap().as_str(),
            "about:blank"
        );
    }

    #[test]
    fn bare_domains_get_https() {
        assert_eq!(
            location_input_to_url("example.com").unwrap().as_str(),
            "https://example.com/"
        );
        assert_eq!(
            location_input_to_url("  www.example.com/path ")
                .unwrap()
                .as_str(),
            "https://www.example.com/path"
        );
    }

    #[test]
    fn localhost_and_host_port_get_http() {
        assert_eq!(
            location_input_to_url("localhost").unwrap().as_str(),
            "http://localhost/"
        );
        assert_eq!(
            location_input_to_url("localhost:8080").unwrap().as_str(),
            "http://localhost:8080/"
        );
        assert_eq!(
            location_input_to_url("example.com:8080").unwrap().as_str(),
            "http://example.com:8080/"
        );
    }

    #[test]
    fn non_urls_are_rejected() {
        assert_eq!(location_input_to_url(""), None);
        assert_eq!(location_input_to_url("hello world"), None);
        assert_eq!(location_input_to_url("justaword"), None);
        assert_eq!(location_input_to_url(".hidden"), None);
    }

    #[test]
    fn localhost_with_a_path_gets_http() {
        assert_eq!(
            location_input_to_url("localhost/path").unwrap().as_str(),
            "http://localhost/path"
        );
        assert_eq!(
            location_input_to_url("localhost:8080/path?q=1")
                .unwrap()
                .as_str(),
            "http://localhost:8080/path?q=1"
        );
    }

    #[test]
    fn a_domain_that_starts_with_localhost_gets_https() {
        assert_eq!(
            location_input_to_url("localhostfoo.com").unwrap().as_str(),
            "https://localhostfoo.com/"
        );
    }

    #[test]
    fn bare_ipv4_addresses_get_http_with_or_without_port_and_path() {
        assert_eq!(
            location_input_to_url("192.168.1.1").unwrap().as_str(),
            "http://192.168.1.1/"
        );
        assert_eq!(
            location_input_to_url("192.168.1.1:8080").unwrap().as_str(),
            "http://192.168.1.1:8080/"
        );
        assert_eq!(
            location_input_to_url("10.0.0.1/admin").unwrap().as_str(),
            "http://10.0.0.1/admin"
        );
    }

    #[test]
    fn javascript_urls_are_refused() {
        assert_eq!(location_input_to_url("javascript:alert(1)"), None);
        assert_eq!(location_input_to_url("  JavaScript:void(0) "), None);
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn edited_address_text_survives_an_engine_url_change() {
        let mut state = ChromeState::default();
        sync_address_text(&mut state, TabId(1), &url("https://a.test/"));
        state.address_text = "typed by the user".into();
        state.address_dirty = true;
        sync_address_text(&mut state, TabId(1), &url("https://a.test/moved"));
        assert_eq!(state.address_text, "typed by the user");
        assert!(state.address_dirty);
    }

    #[test]
    fn switching_tabs_resets_edited_text_to_the_new_tab_url() {
        let mut state = ChromeState::default();
        sync_address_text(&mut state, TabId(1), &url("https://a.test/"));
        state.address_text = "typed by the user".into();
        state.address_dirty = true;
        sync_address_text(&mut state, TabId(2), &url("https://b.test/"));
        assert_eq!(state.address_text, "https://b.test/");
        assert!(!state.address_dirty);
    }

    #[test]
    fn clean_address_text_follows_the_url() {
        let mut state = ChromeState::default();
        sync_address_text(&mut state, TabId(1), &url("https://a.test/"));
        assert!(!state.address_dirty);
        sync_address_text(&mut state, TabId(1), &url("https://a.test/next"));
        assert_eq!(state.address_text, "https://a.test/next");
        assert!(!state.address_dirty);
    }

    #[test]
    fn truncate_adds_ellipsis_only_when_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a longer title here", 8), "a longer…");
    }

    #[test]
    fn chord_labels_are_readable() {
        assert_eq!(
            describe_chord(KeyChord::new(Modifiers::CONTROL, Code::KeyT)),
            "Ctrl+T"
        );
        assert_eq!(
            describe_chord(KeyChord::new(
                Modifiers::CONTROL | Modifiers::SHIFT,
                Code::BracketRight
            )),
            "Ctrl+Shift+]"
        );
        assert_eq!(
            describe_chord(KeyChord::new(Modifiers::CONTROL, Code::Comma)),
            "Ctrl+,"
        );
        assert_eq!(
            describe_chord(KeyChord::new(Modifiers::CONTROL, Code::Digit1)),
            "Ctrl+1"
        );
        assert_eq!(
            describe_chord(KeyChord::new(Modifiers::empty(), Code::F5)),
            "F5"
        );
    }
}
