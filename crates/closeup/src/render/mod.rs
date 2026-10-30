mod renderer;

use std::sync::Arc;

use anyhow::{Context, Result};
use winit::window::Window;

use crate::capture::{CursorImage, Frame};
pub use renderer::{CursorDraw, Renderer, Scene};

/// The Stage window: a [`Renderer`] presenting to the window's surface.
pub struct Compositor {
    window: Arc<Window>,
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl Compositor {
    pub fn new(instance: &wgpu::Instance, window: Arc<Window>) -> Result<Self> {
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no GPU adapter can present to the stage window")?;
        let (device, queue) = pollster::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor { label: Some("closeup"), ..Default::default() }),
        )?;

        let capabilities = surface.get_capabilities(&adapter);
        // Captured pixels are already sRGB-encoded bytes; an 8-bit UNORM
        // surface passes them through untouched, while sRGB or float
        // surfaces would re-encode them and wash the picture out.
        let format = [wgpu::TextureFormat::Bgra8Unorm, wgpu::TextureFormat::Rgba8Unorm]
            .into_iter()
            .find(|format| capabilities.formats.contains(format))
            .context("the stage window offers no 8-bit UNORM surface format")?;
        let present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|mode| capabilities.present_modes.contains(mode))
            .unwrap_or(wgpu::PresentMode::Fifo);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        log::info!("stage surface {format:?}, {present_mode:?} on {}", adapter.get_info().name);
        Ok(Self { window, renderer: Renderer::new(device, queue, format), surface, config })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(self.renderer.device(), &self.config);
    }

    pub fn upload_frame(&mut self, frame: &Frame) {
        self.renderer.upload_frame(frame);
    }

    pub fn upload_cursor(&mut self, image: &CursorImage) {
        self.renderer.upload_cursor(image);
    }

    /// Returns whether a frame reached the screen.
    pub fn render(&mut self, scene: Option<&Scene>) -> bool {
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                self.surface.configure(self.renderer.device(), &self.config);
                texture
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(self.renderer.device(), &self.config);
                return false;
            }
            _ => return false,
        };
        let view = texture.texture.create_view(&Default::default());
        let mut encoder = self.renderer.device().create_command_encoder(&Default::default());
        self.renderer.encode(&mut encoder, &view, (self.config.width, self.config.height), scene);
        self.renderer.queue().submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.renderer.queue().present(texture);
        true
    }
}
