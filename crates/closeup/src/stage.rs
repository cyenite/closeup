use std::sync::Arc;
use std::time::{Duration, Instant};

use closeup_core::{Camera, Rect, Vec2};
use tokio::sync::watch;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use winit::window::{Window, WindowId};

use crate::capture::{Capture, SharedCapture};
use crate::config::Config;
use crate::input::{Action, InputHandle, KeyAccess};
use crate::overlay::{OverlayState, SourceKey};
use crate::render::{Compositor, CursorDraw, Scene};

pub const STAGE_TITLE: &str = "Closeup Stage";

pub enum AppEvent {
    Action(Action),
}

#[derive(Default)]
struct FrameStats {
    drawn: u32,
    skipped: u32,
    captured: u32,
    slowest: Duration,
    since: Option<Instant>,
}

impl FrameStats {
    fn record(&mut self, is_presented: bool, is_captured: bool, took: Duration) {
        let since = *self.since.get_or_insert_with(Instant::now);
        if is_presented {
            self.drawn += 1
        } else {
            self.skipped += 1
        }
        if is_captured {
            self.captured += 1;
        }
        self.slowest = self.slowest.max(took);
        let elapsed = since.elapsed();
        if elapsed >= Duration::from_secs(5) {
            let seconds = elapsed.as_secs_f64();
            log::debug!(
                "stage {:.0} fps, {} skipped, source {:.0} fps, slowest frame {:.1} ms",
                self.drawn as f64 / seconds,
                self.skipped,
                self.captured as f64 / seconds,
                self.slowest.as_secs_f64() * 1000.0
            );
            *self = Self::default();
        }
    }
}

pub struct Stage {
    generation: u64,
    is_paused: bool,
    is_new_source: bool,
    access: KeyAccess,
    modifiers: ModifiersState,
    config: Config,
    camera: Camera,
    stats: FrameStats,
    capture: Capture,
    input: InputHandle,
    next_frame: Instant,
    shared: SharedCapture,
    instance: wgpu::Instance,
    last_tick: Option<Instant>,
    compositor: Option<Compositor>,
    overlay: watch::Sender<OverlayState>,
}

impl Stage {
    pub fn new(
        config: Config,
        capture: Capture,
        input: InputHandle,
        shared: SharedCapture,
        instance: wgpu::Instance,
        overlay: watch::Sender<OverlayState>,
    ) -> Self {
        Self {
            generation: 0,
            is_paused: false,
            is_new_source: true,
            access: KeyAccess::StageOnly,
            modifiers: ModifiersState::empty(),
            camera: Camera::new(config.camera()),
            stats: FrameStats::default(),
            config,
            capture,
            input,
            next_frame: Instant::now(),
            shared,
            instance,
            last_tick: None,
            compositor: None,
            overlay,
        }
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.config.fps.clamp(10, 240) as f64)
    }

    fn handle(&mut self, action: Action) {
        match action {
            Action::Level(level) => self.camera.set_level(level),
            Action::ToggleLock => {
                let is_locked = self.camera.toggle_lock();
                log::info!("pan {}", if is_locked { "locked" } else { "following cursor" });
            }
            Action::PickSource => self.capture.pick_source(),
            Action::Paused(is_paused) => {
                self.is_paused = is_paused;
                log::info!("zoom keys {}", if is_paused { "paused: digits type everywhere" } else { "active" });
            }
            Action::Access(access) => self.access = access,
            Action::TogglePause => {}
        }
    }

    fn draw(&mut self) {
        let now = Instant::now();
        let dt = self.last_tick.map_or(0.0, |last| (now - last).as_secs_f64());
        self.last_tick = Some(now);
        self.next_frame = now + self.frame_interval();
        let Some(compositor) = self.compositor.as_mut() else { return };

        let (frame, cursor, cursor_image, generation, source) = {
            let mut shared = self.shared.lock().unwrap();
            (shared.frame.take(), shared.cursor, shared.cursor_image.clone(), shared.generation, shared.source.clone())
        };
        if generation != self.generation {
            self.generation = generation;
            self.is_new_source = true;
        }
        let is_captured = frame.is_some();
        if let Some(frame) = frame {
            compositor.upload_frame(&frame);
            let size = Vec2::new(frame.width as f64, frame.height as f64);
            if std::mem::take(&mut self.is_new_source) {
                self.camera.reset_source(size);
            } else {
                self.camera.set_source_size(size);
            }
            self.shared.lock().unwrap().spare = Some(frame.pixels);
        }
        if let Some(image) = &cursor_image {
            compositor.upload_cursor(image);
        }
        self.camera.set_cursor(cursor);
        let viewport = self.camera.update(dt);
        let scene = source.as_ref().map(|_| Scene {
            zoom: self.camera.zoom(),
            source: self.camera.source_size(),
            viewport,
            cursor_scale: self.config.cursor_scale,
            cursor: cursor.zip(cursor_image).map(|(position, image)| CursorDraw { position, image }),
        });
        let is_presented = compositor.render(scene.as_ref());
        self.stats.record(is_presented, is_captured, now.elapsed());

        let size = self.camera.source_size();
        let visible = self.camera.visible_source();
        let state = OverlayState {
            zoom: self.camera.zoom(),
            level: self.camera.level(),
            levels: self.config.levels.len(),
            access: self.access,
            generation,
            is_locked: self.camera.is_locked(),
            is_paused: self.is_paused,
            region: source.as_ref().map(|_| Rect::new(visible.origin.unscale(size), visible.size.unscale(size))),
            source: source.as_ref().map(SourceKey::from),
        };
        self.overlay.send_if_modified(|current| {
            let is_changed = *current != state;
            if is_changed {
                *current = state;
            }
            is_changed
        });
    }
}

impl ApplicationHandler<AppEvent> for Stage {
    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause
            && let Some(compositor) = &self.compositor
        {
            compositor.window().request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.compositor.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(STAGE_TITLE)
            .with_name("closeup-stage", "closeup-stage")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                log::error!("cannot open the stage window: {error}");
                return event_loop.exit();
            }
        };
        self.capture.start(window_identifier(&window));
        match Compositor::new(&self.instance, window) {
            Ok(compositor) => {
                compositor.window().request_redraw();
                self.compositor = Some(compositor);
            }
            Err(error) => {
                log::error!("cannot start the GPU compositor: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Action(action) => self.handle(action),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(compositor) = self.compositor.as_mut() {
                    compositor.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(action) = stage_shortcut(code, self.modifiers)
                {
                    self.input.dispatch(action);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
    }
}

/// Exports the Stage window (xdg-foreign on Wayland, the XID on X11) so the
/// portal can parent its picker to it.
fn window_identifier(window: &Window) -> Option<ashpd::WindowIdentifier> {
    let window_handle = window.window_handle().ok()?.as_raw();
    let display_handle = window.display_handle().ok()?.as_raw();
    let identifier =
        pollster::block_on(ashpd::WindowIdentifier::from_raw_handle(&window_handle, Some(&display_handle)));
    if identifier.is_none() {
        log::warn!("could not export the Stage window; the picker may open behind other windows");
    }
    identifier
}

/// Keys typed into the Stage window itself: the fallback that works with
/// no keyboard access and no shell extension at all.
fn stage_shortcut(code: KeyCode, modifiers: ModifiersState) -> Option<Action> {
    let is_chord = modifiers.control_key() && modifiers.alt_key();
    let is_bare = modifiers.is_empty();
    if modifiers.shift_key() || modifiers.super_key() || !(is_chord || is_bare) {
        return None;
    }
    let level = match code {
        KeyCode::KeyP if is_chord => return Some(Action::TogglePause),
        KeyCode::KeyS if is_chord => return Some(Action::PickSource),
        KeyCode::Digit0 | KeyCode::Numpad0 => return Some(Action::ToggleLock),
        KeyCode::Digit1 | KeyCode::Numpad1 => 1,
        KeyCode::Digit2 | KeyCode::Numpad2 => 2,
        KeyCode::Digit3 | KeyCode::Numpad3 => 3,
        KeyCode::Digit4 | KeyCode::Numpad4 => 4,
        KeyCode::Digit5 | KeyCode::Numpad5 => 5,
        KeyCode::Digit6 | KeyCode::Numpad6 => 6,
        KeyCode::Digit7 | KeyCode::Numpad7 => 7,
        KeyCode::Digit8 | KeyCode::Numpad8 => 8,
        KeyCode::Digit9 | KeyCode::Numpad9 => 9,
        _ => return None,
    };
    Some(Action::Level(level))
}
