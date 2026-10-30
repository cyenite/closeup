use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use closeup_core::{Rect, Vec2};

use crate::capture::{CursorImage, Frame, PixelFormat};

const BACKDROP: wgpu::Color = wgpu::Color { r: 0.07, g: 0.07, b: 0.08, a: 1.0 };

#[derive(Clone, Copy)]
#[repr(u32)]
enum DrawMode {
    Frame = 0,
    Cursor = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    dest: [f32; 4],
    source: [f32; 4],
    texel: [f32; 2],
    mode: u32,
    opacity: f32,
}

pub struct CursorDraw {
    pub position: Vec2,
    pub image: Arc<CursorImage>,
}

pub struct Scene {
    pub zoom: f64,
    pub source: Vec2,
    pub viewport: Rect,
    pub cursor_scale: f64,
    pub cursor: Option<CursorDraw>,
}

struct Layer {
    size: (u32, u32),
    uniforms: wgpu::Buffer,
    format: wgpu::TextureFormat,
    texture: Option<wgpu::Texture>,
    bind_group: Option<wgpu::BindGroup>,
}

impl Layer {
    fn new(device: &wgpu::Device, label: &str) -> Self {
        Self {
            size: (0, 0),
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            format: wgpu::TextureFormat::Rgba8Unorm,
            texture: None,
            bind_group: None,
        }
    }
}

/// Draws a captured frame through the camera viewport, plus the cursor,
/// into any texture view: the Stage window's surface, or an offscreen
/// texture for headless rendering.
pub struct Renderer {
    queue: wgpu::Queue,
    device: wgpu::Device,
    frame: Layer,
    cursor: Layer,
    cursor_serial: u64,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("compose.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compose"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compose"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compose"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            frame: Layer::new(&device, "frame"),
            cursor: Layer::new(&device, "cursor"),
            cursor_serial: 0,
            queue,
            device,
            sampler,
            layout,
            pipeline,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn upload_frame(&mut self, frame: &Frame) {
        let format = match frame.format {
            PixelFormat::Bgra => wgpu::TextureFormat::Bgra8Unorm,
            PixelFormat::Rgba => wgpu::TextureFormat::Rgba8Unorm,
        };
        let size = (frame.width, frame.height);
        upload(
            &self.device,
            &self.queue,
            &self.layout,
            &self.sampler,
            &mut self.frame,
            size,
            format,
            frame.stride,
            &frame.pixels,
        );
    }

    pub fn upload_cursor(&mut self, image: &CursorImage) {
        if image.serial == self.cursor_serial {
            return;
        }
        self.cursor_serial = image.serial;
        let size = (image.width, image.height);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        upload(
            &self.device,
            &self.queue,
            &self.layout,
            &self.sampler,
            &mut self.cursor,
            size,
            format,
            image.width * 4,
            &image.pixels,
        );
    }

    /// Records the draw into `encoder`; `target` is the view's size in pixels.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target: (u32, u32),
        scene: Option<&Scene>,
    ) {
        let draws = scene.map(|scene| self.prepare(scene, target)).unwrap_or_default();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compose"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(BACKDROP), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        for layer in draws {
            let layer = match layer {
                DrawMode::Frame => &self.frame,
                DrawMode::Cursor => &self.cursor,
            };
            if let Some(bind_group) = &layer.bind_group {
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }

    /// The stage is always 16:9; a differently shaped window gets bars
    /// rather than a stretched picture.
    fn output_rect(target: (u32, u32)) -> Rect {
        let window = Vec2::new(target.0 as f64, target.1 as f64);
        let aspect = 16.0 / 9.0;
        let size = if window.x / window.y > aspect {
            Vec2::new(window.y * aspect, window.y)
        } else {
            Vec2::new(window.x, window.x / aspect)
        };
        let rect = Rect::from_center(window / 2.0, size);
        Rect::new(rect.origin.unscale(window), rect.size.unscale(window))
    }

    fn prepare(&self, scene: &Scene, target: (u32, u32)) -> Vec<DrawMode> {
        let mut draws = Vec::new();
        if self.frame.bind_group.is_none() {
            return draws;
        }
        let dest = Self::output_rect(target);
        let viewport = scene.viewport;
        let texel = [1.0 / self.frame.size.0 as f32, 1.0 / self.frame.size.1 as f32];
        let source = Rect::new(viewport.origin.unscale(scene.source), viewport.size.unscale(scene.source));
        self.write_params(&self.frame, DrawMode::Frame, dest, source, texel);
        draws.push(DrawMode::Frame);

        let Some(cursor) = &scene.cursor else { return draws };
        if self.cursor.bind_group.is_none() || !viewport.contains(cursor.position) {
            return draws;
        }
        let point = dest.origin + (cursor.position - viewport.origin).unscale(viewport.size).scale(dest.size);
        let unit = dest.size.unscale(viewport.size * scene.zoom) * scene.cursor_scale;
        let image = Vec2::new(cursor.image.width as f64, cursor.image.height as f64);
        let rect = Rect::new(point - cursor.image.hotspot.scale(unit), image.scale(unit));
        let whole = Rect::new(Vec2::ZERO, Vec2::new(1.0, 1.0));
        self.write_params(&self.cursor, DrawMode::Cursor, rect, whole, [0.0, 0.0]);
        draws.push(DrawMode::Cursor);
        draws
    }

    fn write_params(&self, layer: &Layer, mode: DrawMode, dest: Rect, source: Rect, texel: [f32; 2]) {
        let params =
            Params { dest: rect_array(dest), source: rect_array(source), texel, mode: mode as u32, opacity: 1.0 };
        self.queue.write_buffer(&layer.uniforms, 0, bytemuck::bytes_of(&params));
    }
}

fn rect_array(rect: Rect) -> [f32; 4] {
    [rect.origin.x as f32, rect.origin.y as f32, rect.size.x as f32, rect.size.y as f32]
}

#[allow(clippy::too_many_arguments)]
fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    layer: &mut Layer,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    stride: u32,
    pixels: &[u8],
) {
    if size.0 == 0 || size.1 == 0 {
        return;
    }
    let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
    if layer.texture.is_none() || layer.size != size || layer.format != format {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        layer.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: layer.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        }));
        layer.texture = Some(texture);
        layer.size = size;
        layer.format = format;
    }
    let Some(texture) = &layer.texture else { return };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(size.1) },
        extent,
    );
}
