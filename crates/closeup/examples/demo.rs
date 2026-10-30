//! Renders the README demo with the real camera and compositor: a scripted
//! presenter moves the cursor over `slide.png` and presses zoom keys, and
//! every Stage frame is written out along with what the overlay outlines.
//!
//! `cargo run --release --example demo -- target/demo` (see scripts/demo/).

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use closeup::capture::{CursorImage, Frame, PixelFormat};
use closeup::render::{CursorDraw, Renderer, Scene};
use closeup_core::{Camera, CameraConfig, Vec2};

const FPS: f64 = 25.0;
const OUTPUT: (u32, u32) = (1280, 720);
const DURATION: f64 = 9.2;

/// Where the presenter's cursor is at each moment (slide pixels); it eases
/// between these points.
const PATH: [(f64, f64, f64); 11] = [
    (0.0, 1200.0, 700.0),
    (0.6, 1200.0, 700.0),
    (1.4, 1650.0, 380.0),
    (2.0, 1650.0, 380.0),
    (3.2, 2010.0, 470.0),
    (3.6, 2010.0, 470.0),
    (4.4, 1900.0, 1090.0),
    (5.0, 1900.0, 1090.0),
    (6.0, 2150.0, 1130.0),
    (6.8, 2150.0, 1130.0),
    (7.6, 650.0, 1090.0),
];

const KEYS: [(f64, usize); 4] = [(1.6, 3), (4.7, 5), (6.6, 2), (8.2, 1)];

struct Readback {
    buffer: wgpu::Buffer,
    texture: wgpu::Texture,
    bytes_per_row: u32,
}

fn main() -> Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/demo".into()));
    let frames_dir = out.join("frames");
    std::fs::create_dir_all(&frames_dir)?;
    let slide = load_rgba(&out.join("slide.png")).context("run scripts/demo/assets.py first")?;
    let cursor = load_rgba(&out.join("cursor.png"))?;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let mut renderer = Renderer::new(device.clone(), queue.clone(), wgpu::TextureFormat::Rgba8Unorm);
    let readback = readback(&device);

    let source = Vec2::new(slide.width as f64, slide.height as f64);
    renderer.upload_frame(&slide);
    let image = Arc::new(CursorImage {
        width: cursor.width,
        height: cursor.height,
        serial: 1,
        hotspot: Vec2::new(2.0, 2.0),
        pixels: premultiplied(cursor.pixels),
    });
    renderer.upload_cursor(&image);

    let mut camera = Camera::new(CameraConfig::default());
    camera.reset_source(source);
    let mut timeline = BufWriter::new(File::create(out.join("timeline.jsonl"))?);
    let mut last_key = None;
    let frames = (DURATION * FPS) as usize;
    for index in 0..frames {
        let time = index as f64 / FPS;
        let cursor = cursor_at(time);
        camera.set_cursor(Some(cursor));
        for (at, level) in KEYS {
            if (time - at).abs() < 0.5 / FPS {
                camera.set_level(level);
                last_key = Some((at, level));
            }
        }
        let viewport = camera.update(1.0 / FPS);
        let scene = Scene {
            zoom: camera.zoom(),
            source,
            viewport,
            cursor_scale: 1.0,
            cursor: Some(CursorDraw { position: cursor, image: image.clone() }),
        };
        let pixels = render(&renderer, &readback, &scene)?;
        write_png(&frames_dir.join(format!("{index:04}.png")), &pixels)?;

        let visible = camera.visible_source();
        let key = last_key.filter(|(at, _)| time - at < 0.9).map(|(_, level)| level);
        let line = serde_json::json!({
            "frame": index,
            "zoom": camera.zoom(),
            "level": camera.level(),
            "key": key,
            "cursor": [cursor.x / source.x, cursor.y / source.y],
            "region": [visible.origin.x / source.x, visible.origin.y / source.y,
                       visible.size.x / source.x, visible.size.y / source.y],
        });
        writeln!(timeline, "{line}")?;
    }
    println!("rendered {frames} frames to {}", frames_dir.display());
    Ok(())
}

fn cursor_at(time: f64) -> Vec2 {
    let next = PATH.iter().position(|(at, _, _)| *at > time).unwrap_or(PATH.len());
    if next == 0 || next == PATH.len() {
        let (_, x, y) = PATH[next.saturating_sub(1).min(PATH.len() - 1)];
        return Vec2::new(x, y);
    }
    let (start, x0, y0) = PATH[next - 1];
    let (end, x1, y1) = PATH[next];
    let progress = ((time - start) / (end - start)).clamp(0.0, 1.0);
    let eased = progress * progress * (3.0 - 2.0 * progress);
    Vec2::new(x0 + (x1 - x0) * eased, y0 + (y1 - y0) * eased)
}

fn readback(device: &wgpu::Device) -> Readback {
    let bytes_per_row = (OUTPUT.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    Readback {
        buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bytes_per_row * OUTPUT.1) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        texture: device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage"),
            size: wgpu::Extent3d { width: OUTPUT.0, height: OUTPUT.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }),
        bytes_per_row,
    }
}

fn render(renderer: &Renderer, readback: &Readback, scene: &Scene) -> Result<Vec<u8>> {
    let device = renderer.device();
    let view = readback.texture.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.encode(&mut encoder, &view, OUTPUT, Some(scene));
    encoder.copy_texture_to_buffer(
        readback.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback.buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(readback.bytes_per_row),
                rows_per_image: Some(OUTPUT.1),
            },
        },
        wgpu::Extent3d { width: OUTPUT.0, height: OUTPUT.1, depth_or_array_layers: 1 },
    );
    renderer.queue().submit([encoder.finish()]);
    let slice = readback.buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.expect("map readback buffer"));
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let row = (OUTPUT.0 * 4) as usize;
    let pixels = {
        let mapped = slice.get_mapped_range()?;
        mapped.chunks(readback.bytes_per_row as usize).flat_map(|line| line[..row].to_vec()).collect()
    };
    readback.buffer.unmap();
    Ok(pixels)
}

fn load_rgba(path: &Path) -> Result<Frame> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size().context("image too large")?];
    let info = reader.next_frame(&mut pixels)?;
    pixels.truncate(info.buffer_size());
    Ok(Frame { width: info.width, height: info.height, stride: info.width * 4, pixels, format: PixelFormat::Rgba })
}

/// The compositor blends premultiplied alpha, as cursor bitmaps from the
/// compositor already are; PNG alpha is straight.
fn premultiplied(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u16;
        for channel in &mut pixel[..3] {
            *channel = (*channel as u16 * alpha / 255) as u8;
        }
    }
    pixels
}

fn write_png(path: &Path, pixels: &[u8]) -> Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), OUTPUT.0, OUTPUT.1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}
