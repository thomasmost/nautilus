//! The settings window: a winit window with egui only (no tabs). It edits
//! `Settings` in place and reports when they should be saved. Phase 2 adds
//! the hotkey and profile editors here.

use egui_glow::EguiGlow;
use nautilus_core::{ChromeColor, CommandKind, Keymap, Settings};
use nautilus_engine::PhysicalSize;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::Window;

use crate::chrome::{self, location_input_to_url};
use crate::gl::GlSurface;
use crate::icon;
use crate::input::{self, KeyRoute};

#[derive(Debug, PartialEq, Eq)]
pub enum SettingsOutcome {
    Nothing,
    /// An edited field was committed (focus left it, or a checkbox
    /// toggled); the caller should persist the settings.
    Changed,
    /// The user closed the window; the caller should drop it, saving first
    /// if `save` (an edit was still uncommitted).
    Close {
        save: bool,
    },
    /// A keymap chord fired while this window had focus. The caller
    /// handles app-level kinds (quit, new window); window kinds close this
    /// window.
    Shortcut(CommandKind),
}

pub struct SettingsWindow {
    /// A text field was edited and has not lost focus yet.
    dirty: bool,
    modifiers: ModifiersState,
    egui: EguiGlow,
    surface: GlSurface,
    // Dropped last, after the GL state that depends on it.
    window: Window,
}

/// What one frame of the form did.
#[derive(Default)]
struct FormActivity {
    /// Some text changed under the cursor.
    edited: bool,
    /// A field lost focus or a checkbox toggled: time to save.
    committed: bool,
}

impl SettingsWindow {
    pub fn open(event_loop: &ActiveEventLoop) -> Result<SettingsWindow, String> {
        let attributes = Window::default_attributes()
            .with_title("Nautilus Settings")
            .with_inner_size(LogicalSize::new(480.0, 320.0))
            .with_min_inner_size(LogicalSize::new(320.0, 200.0))
            .with_window_icon(icon::window_icon());
        let window = event_loop
            .create_window(attributes)
            .map_err(|e| format!("creating settings window: {e}"))?;
        let size = window.inner_size();
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        let display_handle = event_loop
            .display_handle()
            .map_err(|e| format!("display handle: {e}"))?;
        let window_handle = window
            .window_handle()
            .map_err(|e| format!("window handle: {e}"))?;
        let surface = GlSurface::new(display_handle.as_raw(), window_handle.as_raw(), size)?;
        surface.make_current()?;
        let egui = EguiGlow::new(event_loop, surface.gl(), None, None, false);
        Ok(SettingsWindow {
            dirty: false,
            modifiers: ModifiersState::default(),
            egui,
            surface,
            window,
        })
    }

    pub fn winit_id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        self.window.focus_window();
    }

    /// True if an edit has not been saved yet.
    pub fn needs_save(&self) -> bool {
        self.dirty
    }

    pub fn handle_event(
        &mut self,
        event: &WindowEvent,
        settings: &mut Settings,
        keymap: &Keymap,
    ) -> SettingsOutcome {
        match event {
            WindowEvent::CloseRequested => {
                // The fields may still hold a typed value that never lost
                // focus; store its normalised form before the caller saves.
                if self.dirty {
                    normalise_url_setting(&mut settings.homepage);
                    normalise_url_setting(&mut settings.new_tab_url);
                }
                return on_close(self.dirty);
            }
            WindowEvent::Resized(size) => {
                self.surface
                    .resize(PhysicalSize::new(size.width.max(1), size.height.max(1)));
                self.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let activity = self.frame(settings);
                let (dirty, outcome) = after_frame(self.dirty, activity);
                self.dirty = dirty;
                return outcome;
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event: key, .. } => {
                let key_input = crate::keyutils::key_input_from_winit(key, self.modifiers);
                let route = input::route_key(
                    self.egui.egui_ctx.egui_wants_keyboard_input(),
                    key.state == ElementState::Pressed,
                    key.repeat,
                    key_input.modifiers,
                    key_input.code,
                    keymap,
                );
                if let KeyRoute::Command(kind) = route {
                    return SettingsOutcome::Shortcut(kind);
                }
            }
            _ => {}
        }
        let response = self.egui.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        SettingsOutcome::Nothing
    }

    fn frame(&mut self, settings: &mut Settings) -> FormActivity {
        let mut activity = FormActivity::default();
        self.egui.run(&self.window, |ui| {
            activity = draw_settings(ui, settings);
        });
        self.surface.begin_frame();
        // egui_glow paints over whatever the framebuffer holds, so clear it
        // first; the form's panel fills the rest with the same colour.
        let size = self.window.inner_size();
        self.egui.painter.clear(
            [size.width, size.height],
            chrome::background(&self.egui.egui_ctx, settings.chrome_color),
        );
        self.egui.paint(&self.window);
        self.surface.present();
        if self.egui.egui_ctx.has_requested_repaint() {
            self.window.request_redraw();
        }
        activity
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        let _ = self.surface.make_current();
        self.egui.destroy();
    }
}

/// The save rule after one frame of the form. An edit marks the form dirty;
/// a committed field (or toggled checkbox) while dirty is a change that the
/// caller persists, which clears the dirty flag. Returns the new dirty flag
/// and what the window reports.
fn after_frame(dirty: bool, activity: FormActivity) -> (bool, SettingsOutcome) {
    let dirty = dirty || activity.edited;
    if activity.committed && dirty {
        (false, SettingsOutcome::Changed)
    } else {
        (dirty, SettingsOutcome::Nothing)
    }
}

/// Closing reports the uncommitted edit, if any, so the caller saves it.
fn on_close(dirty: bool) -> SettingsOutcome {
    SettingsOutcome::Close { save: dirty }
}

/// Stores a URL setting in normalised form when its text parses, so
/// `example.com` becomes `https://example.com/`. Text that does not parse is
/// left as typed. Returns true if the stored text changed.
fn normalise_url_setting(text: &mut String) -> bool {
    let Some(url) = location_input_to_url(text.as_str()) else {
        return false;
    };
    if url.as_str() == text.as_str() {
        return false;
    }
    *text = url.to_string();
    true
}

/// A note under a URL field whose text does not parse. It shows once the
/// field is not being edited, so it does not flicker while typing.
fn url_hint(ui: &mut egui::Ui, text: &str, focused: bool) {
    if !focused && location_input_to_url(text).is_none() {
        let color = ui.visuals().error_fg_color;
        ui.label(egui::RichText::new("Not a URL").small().color(color));
    }
}

/// The settings form, on the panel fill so the window background is never
/// left to the framebuffer.
fn draw_settings(ui: &mut egui::Ui, settings: &mut Settings) -> FormActivity {
    let mut activity = FormActivity::default();
    egui::CentralPanel::default().show_inside(ui, |ui| {
        ui.heading("Settings");
        ui.add_space(8.0);
        egui::Grid::new("settings-grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Homepage");
                ui.vertical(|ui| {
                    let homepage = ui.add(
                        egui::TextEdit::singleline(&mut settings.homepage).desired_width(300.0),
                    );
                    activity.edited |= homepage.changed();
                    if homepage.lost_focus() {
                        activity.committed = true;
                        activity.edited |= normalise_url_setting(&mut settings.homepage);
                    }
                    url_hint(ui, &settings.homepage, homepage.has_focus());
                });
                ui.end_row();

                ui.label("New tab URL");
                ui.vertical(|ui| {
                    let new_tab = ui.add(
                        egui::TextEdit::singleline(&mut settings.new_tab_url).desired_width(300.0),
                    );
                    activity.edited |= new_tab.changed();
                    if new_tab.lost_focus() {
                        activity.committed = true;
                        activity.edited |= normalise_url_setting(&mut settings.new_tab_url);
                    }
                    url_hint(ui, &settings.new_tab_url, new_tab.has_focus());
                });
                ui.end_row();

                ui.label("Chrome colour");
                ui.horizontal(|ui| {
                    for color in ChromeColor::ALL {
                        let [r, g, b] = color.rgb();
                        let selected = settings.chrome_color == color;
                        let stroke = if selected {
                            egui::Stroke::new(2.0_f32, ui.visuals().strong_text_color())
                        } else {
                            egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(40))
                        };
                        let swatch = ui
                            .add(
                                egui::Button::new("")
                                    .fill(egui::Color32::from_rgb(r, g, b))
                                    .stroke(stroke)
                                    .corner_radius(6.0)
                                    .min_size(egui::Vec2::splat(26.0)),
                            )
                            .on_hover_text(color.label());
                        if swatch.clicked() && !selected {
                            settings.chrome_color = color;
                            activity.edited = true;
                            activity.committed = true;
                        }
                    }
                });
                ui.end_row();

                ui.label("On launch");
                let restore = ui.checkbox(
                    &mut settings.restore_windows_on_launch,
                    "Restore previous windows (not yet implemented)",
                );
                if restore.changed() {
                    activity.edited = true;
                    activity.committed = true;
                }
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("Keyboard shortcuts and profiles arrive in Phase 2.")
                .small()
                .weak(),
        );
    });
    activity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited() -> FormActivity {
        FormActivity {
            edited: true,
            ..Default::default()
        }
    }

    fn committed() -> FormActivity {
        FormActivity {
            committed: true,
            ..Default::default()
        }
    }

    fn toggled() -> FormActivity {
        FormActivity {
            edited: true,
            committed: true,
        }
    }

    #[test]
    fn an_edit_without_a_blur_marks_dirty_and_reports_nothing() {
        assert_eq!(
            after_frame(false, edited()),
            (true, SettingsOutcome::Nothing)
        );
    }

    #[test]
    fn a_blur_without_an_edit_reports_nothing() {
        assert_eq!(
            after_frame(false, committed()),
            (false, SettingsOutcome::Nothing)
        );
    }

    #[test]
    fn a_toggle_reports_a_change_at_once() {
        assert_eq!(
            after_frame(false, toggled()),
            (false, SettingsOutcome::Changed)
        );
    }

    #[test]
    fn an_edit_then_a_blur_reports_a_change_and_clears_dirty() {
        let (dirty, outcome) = after_frame(false, edited());
        assert_eq!(outcome, SettingsOutcome::Nothing);
        assert_eq!(
            after_frame(dirty, committed()),
            (false, SettingsOutcome::Changed)
        );
    }

    #[test]
    fn an_edit_then_a_close_saves() {
        let (dirty, _) = after_frame(false, edited());
        assert_eq!(on_close(dirty), SettingsOutcome::Close { save: true });
    }

    #[test]
    fn a_close_while_clean_does_not_save() {
        assert_eq!(on_close(false), SettingsOutcome::Close { save: false });
    }

    #[test]
    fn a_close_after_a_committed_change_does_not_save_again() {
        let (dirty, outcome) = after_frame(false, toggled());
        assert_eq!(outcome, SettingsOutcome::Changed);
        assert_eq!(on_close(dirty), SettingsOutcome::Close { save: false });
    }

    #[test]
    fn a_bare_domain_is_stored_as_a_full_https_url() {
        let mut text = "example.com".to_string();
        assert!(normalise_url_setting(&mut text));
        assert_eq!(text, "https://example.com/");
    }

    #[test]
    fn surrounding_space_is_trimmed_from_a_stored_url() {
        let mut text = "  www.example.com/path ".to_string();
        assert!(normalise_url_setting(&mut text));
        assert_eq!(text, "https://www.example.com/path");
    }

    #[test]
    fn a_valid_url_is_left_unchanged() {
        let mut text = "https://example.com/a".to_string();
        assert!(!normalise_url_setting(&mut text));
        assert_eq!(text, "https://example.com/a");
    }

    #[test]
    fn text_that_is_not_a_url_is_kept_as_typed() {
        for raw in ["not a url", "javascript:alert(1)", ""] {
            let mut text = raw.to_string();
            assert!(!normalise_url_setting(&mut text), "{raw:?}");
            assert_eq!(text, raw);
            assert!(location_input_to_url(&text).is_none(), "{raw:?} is flagged");
        }
    }
}
