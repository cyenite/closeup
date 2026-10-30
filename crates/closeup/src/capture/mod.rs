mod portal;
mod stream;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::Result;
use ashpd::WindowIdentifier;
use closeup_core::Vec2;

pub use portal::SourceInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra,
    Rgba,
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
    pub format: PixelFormat,
}

pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    pub serial: u64,
    pub hotspot: Vec2,
    pub pixels: Vec<u8>,
}

#[derive(Default)]
pub struct CaptureState {
    pub generation: u64,
    pub frame: Option<Frame>,
    pub cursor: Option<Vec2>,
    pub spare: Option<Vec<u8>>,
    pub source: Option<SourceInfo>,
    pub cursor_image: Option<Arc<CursorImage>>,
}

pub type SharedCapture = Arc<Mutex<CaptureState>>;

enum Command {
    Start(Option<WindowIdentifier>),
    Pick,
}

pub struct Capture {
    commands: Sender<Command>,
}

impl Capture {
    pub fn spawn(state: SharedCapture, token_path: PathBuf) -> Self {
        let (commands, requests) = mpsc::channel::<Command>();
        let retry = commands.clone();
        thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                if let Err(error) = run(state, token_path, requests, retry) {
                    log::error!("capture stopped: {error:#}");
                }
            })
            .expect("spawn capture thread");
        Self { commands }
    }

    /// Starts capturing once the Stage window exists, so the picker can be
    /// parented to it; an unparented picker opens behind other windows.
    pub fn start(&self, parent: Option<WindowIdentifier>) {
        let _ = self.commands.send(Command::Start(parent));
    }

    /// Opens the system source picker; the current source keeps streaming
    /// until the new one is chosen, so the meeting never sees a gap.
    pub fn pick_source(&self) {
        let _ = self.commands.send(Command::Pick);
    }
}

struct ActiveStream {
    stopping: Arc<AtomicBool>,
    quit: pipewire::channel::Sender<()>,
    thread: JoinHandle<()>,
}

impl ActiveStream {
    /// Flags the stop as deliberate first, so tearing the stream down is
    /// not mistaken for the source going away.
    fn stop(self) {
        self.stopping.store(true, Ordering::Relaxed);
        let _ = self.quit.send(());
        let _ = self.thread.join();
    }
}

fn run(
    state: SharedCapture,
    token_path: PathBuf,
    requests: mpsc::Receiver<Command>,
    retry: Sender<Command>,
) -> Result<()> {
    let parent = loop {
        match requests.recv() {
            Ok(Command::Start(parent)) => break parent,
            Ok(Command::Pick) => continue,
            Err(_) => return Ok(()),
        }
    };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let mut token = std::fs::read_to_string(&token_path).ok().filter(|token| !token.is_empty());
    let mut active: Option<ActiveStream> = None;
    let mut session = None;
    loop {
        match runtime.block_on(portal::open(token.take(), parent.as_ref())) {
            Ok(opened) => {
                if let Some(saved) = &opened.restore_token {
                    if let Some(parent) = token_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&token_path, saved);
                }
                if let Some(previous) = active.take() {
                    previous.stop();
                }
                if let Some(previous) = session.replace(opened.session) {
                    let _ = runtime.block_on(previous.close());
                }
                {
                    let mut shared = state.lock().unwrap();
                    shared.generation += 1;
                    shared.frame = None;
                    shared.cursor = None;
                    shared.source = Some(opened.info);
                }
                let (quit, receiver) = pipewire::channel::channel();
                let stopping = Arc::new(AtomicBool::new(false));
                let on_lost = lost_source_handler(state.clone(), stopping.clone(), retry.clone());
                let shared = state.clone();
                let node = opened.node_id;
                let fd = opened.fd;
                let thread = thread::Builder::new().name("pipewire".into()).spawn(move || {
                    if let Err(error) = stream::run(node, fd, shared, receiver, on_lost) {
                        log::error!("pipewire stream failed: {error:#}");
                    }
                })?;
                active = Some(ActiveStream { stopping, quit, thread });
            }
            Err(error) => log::warn!("source selection cancelled or failed: {error:#}"),
        }
        if !matches!(requests.recv(), Ok(Command::Pick)) {
            break;
        }
    }
    if let Some(previous) = active.take() {
        previous.stop();
    }
    Ok(())
}

/// When the shared window closes, the Stage goes dark instead of freezing
/// on its last frame, and the picker opens so the presenter can carry on.
fn lost_source_handler(
    state: SharedCapture,
    stopping: Arc<AtomicBool>,
    retry: Sender<Command>,
) -> Box<dyn Fn() + Send> {
    Box::new(move || {
        if stopping.load(Ordering::Relaxed) {
            return;
        }
        log::warn!("the shared source went away; opening the picker");
        {
            let mut shared = state.lock().unwrap();
            shared.generation += 1;
            shared.frame = None;
            shared.cursor = None;
            shared.source = None;
        }
        let _ = retry.send(Command::Pick);
    })
}
