use std::cell::Cell;
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use closeup_core::Vec2;
use pipewire as pw;
use pw::spa;
use pw::spa::buffer::meta::{MetaCursor, MetaVideoCrop};
use pw::spa::param::ParamType;
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Fraction, Id, Rectangle, SpaTypes};
use pw::stream::StreamState;

use super::{CursorImage, Frame, PixelFormat, SharedCapture};

static CURSOR_SERIAL: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct Negotiated {
    format: Option<PixelFormat>,
    info: VideoInfoRaw,
}

/// Streams `node` into `state` until told to quit. `on_lost` runs once if
/// the source itself goes away (e.g. the shared window was closed).
pub fn run(
    node: u32,
    fd: OwnedFd,
    state: SharedCapture,
    quit: pw::channel::Receiver<()>,
    on_lost: Box<dyn Fn() + Send>,
) -> Result<()> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_fd_rc(fd, None)?;
    let stopper = main_loop.clone();
    let _quit = quit.attach(main_loop.loop_(), move |_| stopper.quit());

    let stream = pw::stream::StreamRc::new(
        core,
        "closeup-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let ender = main_loop.clone();
    let has_ended = Rc::new(Cell::new(false));
    let _listener = stream
        .add_local_listener_with_user_data(Negotiated::default())
        .state_changed(move |_, _, old, new| {
            log::debug!("stream state {old:?} -> {new:?}");
            let is_lost = matches!(new, StreamState::Error(_))
                || (matches!(new, StreamState::Unconnected) && !matches!(old, StreamState::Unconnected));
            if is_lost && !has_ended.replace(true) {
                on_lost();
                ender.quit();
            }
        })
        .param_changed(|stream, negotiated, id, param| {
            let Some(param) = param else { return };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let Ok((MediaType::Video, MediaSubtype::Raw)) = spa::param::format_utils::parse_format(param) else {
                return;
            };
            if negotiated.info.parse(param).is_err() {
                return;
            }
            negotiated.format = pixel_format(negotiated.info.format());
            log::info!(
                "negotiated {:?} {}x{}",
                negotiated.info.format(),
                negotiated.info.size().width,
                negotiated.info.size().height
            );
            let metas = [
                meta_param(spa::sys::SPA_META_VideoCrop, fixed(region_size())),
                meta_param(
                    spa::sys::SPA_META_Cursor,
                    ChoiceValue::Int(Choice(
                        ChoiceFlags::empty(),
                        ChoiceEnum::Range {
                            default: cursor_meta_size(64),
                            min: cursor_meta_size(1),
                            max: cursor_meta_size(512),
                        },
                    )),
                ),
            ];
            let mut pods: Vec<&Pod> = metas.iter().filter_map(|bytes| Pod::from_bytes(bytes)).collect();
            if let Err(error) = stream.update_params(&mut pods) {
                log::warn!("could not request cursor metadata: {error}");
            }
        })
        .process(move |stream, negotiated| {
            let mut newest = None;
            while let Some(buffer) = stream.dequeue_buffer() {
                newest = Some(buffer);
            }
            let Some(mut buffer) = newest else { return };
            let crop = buffer
                .find_meta::<MetaVideoCrop>()
                .map(|crop| crop.meta_region())
                .filter(|region| region.is_valid() && region.size().width > 0)
                .map(|region| (region.position(), region.size()));
            let (origin_x, origin_y) = crop.map_or((0, 0), |(position, _)| (position.x, position.y));
            let cursor = buffer.find_meta::<MetaCursor>().map(|meta| read_cursor(meta, origin_x, origin_y));

            let spare = {
                let mut shared = state.lock().unwrap();
                if let Some((position, image)) = cursor {
                    shared.cursor = position;
                    if image.is_some() {
                        shared.cursor_image = image;
                    }
                }
                shared.spare.take()
            };
            let Some(format) = negotiated.format else { return };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else { return };
            let chunk = data.chunk();
            let (size, offset, stride) = (chunk.size() as usize, chunk.offset() as usize, chunk.stride());
            if size == 0 || stride <= 0 {
                return;
            }
            let full = negotiated.info.size();
            let area = crop.map_or(Rectangle { width: full.width, height: full.height }, |(_, size)| size);
            let Some(bytes) = data.data() else { return };
            let Some(source) = bytes.get(offset..offset + size) else { return };
            let stride = stride as usize;
            let row = area.width as usize * 4;
            let mut pixels = spare.unwrap_or_default();
            pixels.clear();
            pixels.reserve(row * area.height as usize);
            for line in 0..area.height as usize {
                let start = (origin_y.max(0) as usize + line) * stride + origin_x.max(0) as usize * 4;
                match source.get(start..start + row) {
                    Some(slice) => pixels.extend_from_slice(slice),
                    None => return,
                }
            }
            let mut shared = state.lock().unwrap();
            let previous = shared.frame.replace(Frame {
                width: area.width,
                height: area.height,
                stride: row as u32,
                pixels,
                format,
            });
            if let Some(previous) = previous {
                shared.spare = Some(previous.pixels);
            }
        })
        .register()?;

    let format = format_param().context("serialize format")?;
    let mut params = [Pod::from_bytes(&format).context("format pod")?];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;
    main_loop.run();
    Ok(())
}

fn pixel_format(format: VideoFormat) -> Option<PixelFormat> {
    match format {
        VideoFormat::BGRx | VideoFormat::BGRA => Some(PixelFormat::Bgra),
        VideoFormat::RGBx | VideoFormat::RGBA => Some(PixelFormat::Rgba),
        _ => None,
    }
}

fn read_cursor(meta: &MetaCursor, origin_x: i32, origin_y: i32) -> (Option<Vec2>, Option<Arc<CursorImage>>) {
    if !meta.is_valid() {
        return (None, None);
    }
    let point = meta.position();
    let position = Vec2::new((point.x - origin_x) as f64, (point.y - origin_y) as f64);
    let image = meta.bitmap().filter(|bitmap| bitmap.is_valid()).and_then(|bitmap| {
        let size = bitmap.size();
        let swap = match bitmap.format() {
            VideoFormat::RGBA | VideoFormat::RGBx => false,
            VideoFormat::BGRA | VideoFormat::BGRx => true,
            _ => return None,
        };
        let data = bitmap.bitmap_data()?;
        let stride = bitmap.stride().max(0) as usize;
        let row = size.width as usize * 4;
        let mut pixels = Vec::with_capacity(row * size.height as usize);
        for line in 0..size.height as usize {
            pixels.extend_from_slice(data.get(line * stride..line * stride + row)?);
        }
        if swap {
            pixels.as_chunks_mut::<4>().0.iter_mut().for_each(|pixel| pixel.swap(0, 2));
        }
        let hotspot = meta.hotspot();
        Some(Arc::new(CursorImage {
            width: size.width,
            height: size.height,
            serial: CURSOR_SERIAL.fetch_add(1, Ordering::Relaxed),
            hotspot: Vec2::new(hotspot.x as f64, hotspot.y as f64),
            pixels,
        }))
    });
    (Some(position), image)
}

fn region_size() -> i32 {
    std::mem::size_of::<spa::sys::spa_meta_region>() as i32
}

fn cursor_meta_size(edge: i32) -> i32 {
    (std::mem::size_of::<spa::sys::spa_meta_cursor>() + std::mem::size_of::<spa::sys::spa_meta_bitmap>()) as i32
        + edge * edge * 4
}

fn fixed(value: i32) -> ChoiceValue {
    ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::None(value)))
}

fn serialize(object: Object) -> Option<Vec<u8>> {
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

fn meta_param(meta: u32, size: ChoiceValue) -> Vec<u8> {
    serialize(Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(spa::sys::SPA_PARAM_META_type, Value::Id(Id(meta))),
            Property::new(spa::sys::SPA_PARAM_META_size, Value::Choice(size)),
        ],
    })
    .unwrap_or_default()
}

fn format_param() -> Option<Vec<u8>> {
    serialize(pw::spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pw::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pw::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pw::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        pw::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 60, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 240, denom: 1 }
        ),
    ))
}
