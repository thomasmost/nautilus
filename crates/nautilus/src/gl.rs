//! The GL context and surface of one native window, through glutin. egui
//! paints the chrome with it, and the page frames Chromium paints
//! off-screen are uploaded to textures on it.

use std::ffi::CString;
use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlProfile, NotCurrentGlContext, PossiblyCurrentContext,
    PossiblyCurrentGlContext, Version,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface as _, Surface, SurfaceAttributesBuilder, WindowSurface};
use nautilus_engine::PhysicalSize;
use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle};

pub struct GlSurface {
    gl: Arc<glow::Context>,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    // Dropped last: the surface and context belong to it.
    _display: Display,
    size: std::cell::Cell<PhysicalSize<u32>>,
}

impl GlSurface {
    /// A double-buffered RGBA8 surface on `window` with a desktop GL 3.3
    /// core context.
    pub fn new(
        display: RawDisplayHandle,
        window: RawWindowHandle,
        size: PhysicalSize<u32>,
    ) -> Result<GlSurface, String> {
        #[cfg(target_os = "windows")]
        let preference = DisplayApiPreference::Wgl(Some(window));
        #[cfg(target_os = "macos")]
        let preference = DisplayApiPreference::Cgl;
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let preference = DisplayApiPreference::Egl;
        // SAFETY: the handles come from a live winit window and event loop.
        let display = unsafe { Display::new(display, preference) }
            .map_err(|e| format!("creating the GL display: {e}"))?;
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .compatible_with_native_window(window)
            .build();
        // SAFETY: as above.
        let config = unsafe { display.find_configs(template) }
            .map_err(|e| format!("finding a GL config: {e}"))?
            .reduce(|best, candidate| {
                // No multisampling: the chrome is 2D, and the page is a
                // texture.
                if candidate.num_samples() < best.num_samples() {
                    candidate
                } else {
                    best
                }
            })
            .ok_or_else(|| "no GL config matches the window".to_string())?;
        let attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
            .with_profile(GlProfile::Core)
            .build(Some(window));
        // SAFETY: as above.
        let context = unsafe { display.create_context(&config, &attributes) }
            .map_err(|e| format!("creating the GL context: {e}"))?;
        let (width, height) = non_zero(size);
        let surface_attributes =
            SurfaceAttributesBuilder::<WindowSurface>::new().build(window, width, height);
        // SAFETY: as above.
        let surface = unsafe { display.create_window_surface(&config, &surface_attributes) }
            .map_err(|e| format!("creating the GL surface: {e}"))?;
        let context = context
            .make_current(&surface)
            .map_err(|e| format!("making the GL context current: {e}"))?;
        // SAFETY: the context is current; the loader resolves GL entry
        // points for it.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                let name = CString::new(name).expect("GL function names have no NUL");
                display.get_proc_address(&name).cast()
            })
        };
        Ok(GlSurface {
            gl: Arc::new(gl),
            surface,
            context,
            _display: display,
            size: std::cell::Cell::new(size),
        })
    }

    pub fn make_current(&self) -> Result<(), String> {
        self.context
            .make_current(&self.surface)
            .map_err(|e| format!("making the GL context current: {e}"))
    }

    /// Call on every window resize, in physical pixels.
    pub fn resize(&self, size: PhysicalSize<u32>) {
        let (width, height) = non_zero(size);
        self.surface.resize(&self.context, width, height);
        self.size.set(size);
    }

    /// The glow handle egui renders through.
    pub fn gl(&self) -> Arc<glow::Context> {
        self.gl.clone()
    }

    /// Make current and bind the window's framebuffer before egui paints.
    pub fn begin_frame(&self) {
        if let Err(e) = self.make_current() {
            log::error!("{e}");
        }
        use glow::HasContext;
        // SAFETY: the context is current.
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            let size = self.size.get();
            self.gl
                .viewport(0, 0, size.width as i32, size.height as i32);
        }
    }

    /// Swap buffers.
    pub fn present(&self) {
        if let Err(e) = self.surface.swap_buffers(&self.context) {
            log::error!("presenting the GL surface: {e}");
        }
    }
}

fn non_zero(size: PhysicalSize<u32>) -> (NonZeroU32, NonZeroU32) {
    (
        NonZeroU32::new(size.width.max(1)).expect("non-zero"),
        NonZeroU32::new(size.height.max(1)).expect("non-zero"),
    )
}

/// A page frame uploaded as a GL texture egui can paint.
pub struct PageTexture {
    texture: glow::Texture,
    pub id: egui::TextureId,
    pub size: (u32, u32),
}

impl PageTexture {
    /// Upload `bgra` (`width` by `height`, tightly packed) into a new
    /// texture and register it with egui's painter.
    pub fn new(
        gl: &glow::Context,
        painter: &mut egui_glow::Painter,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) -> Option<PageTexture> {
        let texture = upload(gl, width, height, bgra)?;
        let id = painter.register_native_texture(texture);
        Some(PageTexture {
            texture,
            id,
            size: (width, height),
        })
    }

    /// Replace the pixels; a size change recreates the texture under the
    /// same egui id.
    pub fn update(
        &mut self,
        gl: &glow::Context,
        painter: &mut egui_glow::Painter,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) {
        use glow::HasContext;
        if (width, height) == self.size {
            // SAFETY: the context is current and the texture is ours.
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    width as i32,
                    height as i32,
                    glow::BGRA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(bgra)),
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            return;
        }
        if let Some(texture) = upload(gl, width, height, bgra) {
            painter.replace_native_texture(self.id, texture);
            // SAFETY: as above; the old texture is no longer referenced.
            unsafe { gl.delete_texture(self.texture) };
            self.texture = texture;
            self.size = (width, height);
        }
    }

    pub fn destroy(self, gl: &glow::Context, painter: &mut egui_glow::Painter) {
        use glow::HasContext;
        painter.free_texture(self.id);
        // SAFETY: the context is current and the texture is ours.
        unsafe { gl.delete_texture(self.texture) };
    }
}

fn upload(gl: &glow::Context, width: u32, height: u32, bgra: &[u8]) -> Option<glow::Texture> {
    use glow::HasContext;
    if width == 0 || height == 0 || bgra.len() != width as usize * height as usize * 4 {
        return None;
    }
    // SAFETY: the context is current; the slice is `width * height` BGRA
    // pixels, as checked above.
    unsafe {
        let texture = match gl.create_texture() {
            Ok(texture) => texture,
            Err(e) => {
                log::error!("creating a page texture: {e}");
                return None;
            }
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width as i32,
            height as i32,
            0,
            glow::BGRA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(bgra)),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
        Some(texture)
    }
}
