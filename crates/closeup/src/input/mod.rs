mod focus;
mod router;
mod session;

use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, Device, EventSummary, EventType, KeyCode};
use tokio::sync::watch;

use focus::FocusProbe;
pub use router::{Action, KeyAccess};
use router::{Context as KeyContext, KeyRouter, Verdict};

const VIRTUAL_NAME: &str = "Closeup passthrough keyboard";
const RESCAN_INTERVAL: Duration = Duration::from_secs(2);
const HIGHEST_KEY: u16 = 248;

pub type ActionSink = Arc<dyn Fn(Action) + Send + Sync>;

/// Shared controls for the keyboard service, also used by the shell's
/// shortcuts and the Stage window.
#[derive(Clone)]
pub struct InputHandle {
    sink: ActionSink,
    paused: watch::Sender<bool>,
    is_live: watch::Sender<bool>,
    has_shell_shortcuts: Arc<AtomicBool>,
}

impl InputHandle {
    pub fn toggle_pause(&self) {
        let mut now = false;
        self.paused.send_modify(|paused| {
            *paused = !*paused;
            now = *paused;
        });
        (self.sink)(Action::Paused(now));
    }

    /// Once the shell owns the Ctrl+Alt shortcuts, the keyboard service
    /// must stop acting on them too, or every toggle would fire twice.
    pub fn set_shell_shortcuts(&self, is_owned: bool) {
        self.has_shell_shortcuts.store(is_owned, Ordering::Relaxed);
    }

    pub fn dispatch(&self, action: Action) {
        match action {
            Action::TogglePause => self.toggle_pause(),
            action => (self.sink)(action),
        }
    }
}

/// Reads every keyboard through evdev. Each keyboard is grabbed and every
/// event is replayed through a uinput keyboard, except bare digits pressed
/// while the presenter is not typing: those drive the zoom instead.
pub fn spawn(sink: ActionSink) -> InputHandle {
    let handle = InputHandle {
        sink,
        paused: watch::channel(false).0,
        is_live: watch::channel(true).0,
        has_shell_shortcuts: Arc::new(AtomicBool::new(false)),
    };
    let input = handle.clone();
    thread::Builder::new()
        .name("input".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => return log::error!("input runtime: {error}"),
            };
            runtime.block_on(run(input));
        })
        .expect("spawn input thread");
    handle
}

async fn run(input: InputHandle) {
    let passthrough = passthrough_keyboard().map(|device| Arc::new(Mutex::new(device)));
    if passthrough.is_none() {
        log::warn!("/dev/uinput is not writable: bare digits stay with apps, Ctrl+Alt shortcuts still zoom");
    }
    tokio::spawn(session::watch(input.is_live.clone()));
    let focus = Arc::new(FocusProbe::connect().await);
    let active: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
    let mut access = None;
    let mut has_explained = false;
    loop {
        let scan = scan_keyboards(&active.lock().unwrap());
        for (path, keyboard) in scan.found {
            let name = keyboard.name().unwrap_or("keyboard").to_owned();
            log::info!("listening to {name}");
            active.lock().unwrap().insert(path.clone());
            let task = Keyboard { input: input.clone(), focus: focus.clone(), passthrough: passthrough.clone() };
            let active = active.clone();
            tokio::spawn(async move {
                if let Err(error) = task.run(keyboard).await {
                    log::info!("stopped listening to {name}: {error:#}");
                }
                active.lock().unwrap().remove(&path);
            });
        }
        let has_keyboards = !active.lock().unwrap().is_empty();
        let now = match (has_keyboards, &passthrough) {
            (true, Some(_)) => KeyAccess::Full,
            (true, None) => KeyAccess::ChordsOnly,
            (false, _) => KeyAccess::StageOnly,
        };
        if access != Some(now) {
            access = Some(now);
            input.dispatch(Action::Access(now));
        }
        if !has_keyboards && scan.is_denied && !has_explained {
            log::error!("cannot read keyboards in /dev/input: {}", permission_hint());
            has_explained = true;
        }
        tokio::time::sleep(RESCAN_INTERVAL).await;
    }
}

/// Declares every key, not just those of the keyboards present at startup,
/// so a keyboard plugged in later is replayed faithfully.
fn passthrough_keyboard() -> Option<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    (1..=HIGHEST_KEY).for_each(|code| keys.insert(KeyCode::new(code)));
    VirtualDevice::builder().ok()?.name(VIRTUAL_NAME).with_keys(&keys).ok()?.build().ok()
}

struct Scan {
    is_denied: bool,
    found: Vec<(PathBuf, Device)>,
}

fn scan_keyboards(known: &HashSet<PathBuf>) -> Scan {
    let mut scan = Scan { is_denied: false, found: Vec::new() };
    let Ok(entries) = std::fs::read_dir("/dev/input") else { return scan };
    for path in entries.flatten().map(|entry| entry.path()) {
        let is_event_node =
            path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("event"));
        if !is_event_node || known.contains(&path) {
            continue;
        }
        match Device::open(&path) {
            Ok(device) if device.name() != Some(VIRTUAL_NAME) && is_keyboard(&device) => {
                scan.found.push((path, device))
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::PermissionDenied => scan.is_denied = true,
            Err(_) => {}
        }
    }
    scan
}

pub fn readable_keyboards() -> usize {
    scan_keyboards(&HashSet::new()).found.len()
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        [KeyCode::KEY_A, KeyCode::KEY_Z, KeyCode::KEY_1, KeyCode::KEY_SPACE].iter().all(|key| keys.contains(*key))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupState {
    Missing,
    NotMember,
    Stale,
    Active,
}

pub fn input_group_state() -> GroupState {
    let user = std::env::var("USER").unwrap_or_default();
    let groups = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let Some(fields) =
        groups.lines().map(|line| line.split(':').collect::<Vec<_>>()).find(|fields| fields[0] == "input")
    else {
        return GroupState::Missing;
    };
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let is_active = fields.get(2).is_some_and(|gid| {
        status
            .lines()
            .find_map(|line| line.strip_prefix("Groups:"))
            .is_some_and(|active| active.split_whitespace().any(|group| group == *gid))
    });
    let is_member = fields.get(3).is_some_and(|members| members.split(',').any(|member| member == user));
    match (is_member, is_active) {
        (_, true) => GroupState::Active,
        (true, false) => GroupState::Stale,
        (false, false) => GroupState::NotMember,
    }
}

/// Group changes only apply to sessions started after them, which is easy
/// to miss right after running the setup script.
pub fn permission_hint() -> &'static str {
    match input_group_state() {
        GroupState::Missing => "this system has no 'input' group",
        GroupState::NotMember => "run setup/install.sh to join the 'input' group, then log out and back in",
        GroupState::Stale if is_lingering() => {
            "you are in the 'input' group, but lingering keeps your systemd user manager (and its old groups) alive across logouts; reboot"
        }
        GroupState::Stale => {
            "you are in the 'input' group, but this login session started before that; log out and back in"
        }
        GroupState::Active => "the 'input' group is active but no keyboard is readable; check `ls -l /dev/input`",
    }
}

/// With lingering on, the user manager outlives logouts, and every new
/// session inherits the groups it had when it first started.
pub fn is_lingering() -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    Path::new("/var/lib/systemd/linger").join(user).exists()
}

pub fn can_write_uinput() -> bool {
    std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok()
}

struct Keyboard {
    input: InputHandle,
    focus: Arc<FocusProbe>,
    passthrough: Option<Arc<Mutex<VirtualDevice>>>,
}

impl Keyboard {
    async fn run(self, device: Device) -> Result<()> {
        let mut stream = device.into_event_stream().context("open event stream")?;
        let mut changes = self.input.paused.subscribe();
        let mut liveness = self.input.is_live.subscribe();
        let mut router = KeyRouter::default();
        let mut is_grabbed = false;
        loop {
            let is_live = *liveness.borrow();
            let wants_grab = is_live && !*changes.borrow() && self.passthrough.is_some();
            // Switching only with no keys down keeps press/release pairs on
            // one device; libinput drops releases it never saw pressed,
            // which would leave a key stuck. At startup the router has not
            // seen anything yet (the Enter that launched us may still be
            // down), so ask the kernel too.
            let device = stream.device_mut();
            let is_physically_idle = device.get_key_state().is_ok_and(|keys| keys.iter().next().is_none());
            if wants_grab != is_grabbed && router.is_idle() && is_physically_idle {
                let result = if wants_grab { device.grab() } else { device.ungrab() };
                match result {
                    Ok(()) => is_grabbed = wants_grab,
                    Err(error) => log::warn!("could not change keyboard grab: {error}"),
                }
            }
            tokio::select! {
                changed = changes.changed() => changed?,
                changed = liveness.changed() => changed?,
                event = stream.next_event() => {
                    let event = event?;
                    if let EventSummary::Key(_, key, value) = event.destructure() {
                        let is_text_focused =
                            router.needs_focus_check(key, value, is_grabbed) && self.focus.is_text_focused().await;
                        let context = KeyContext {
                            now: Instant::now(),
                            is_grabbed,
                            is_text_focused,
                            has_shell_shortcuts: self.input.has_shell_shortcuts.load(Ordering::Relaxed),
                        };
                        let (verdict, action) = router.handle(key, value, context);
                        if let Some(action) = action.filter(|_| is_live) {
                            self.input.dispatch(action);
                        }
                        if is_grabbed && verdict == Verdict::Forward {
                            self.forward(event);
                        }
                    } else if is_grabbed && !matches!(event.event_type(), EventType::SYNCHRONIZATION | EventType::MISC) {
                        self.forward(event);
                    }
                }
            }
        }
    }

    fn forward(&self, event: evdev::InputEvent) {
        if let Some(passthrough) = &self.passthrough
            && let Err(error) = passthrough.lock().unwrap().emit(&[event])
        {
            log::warn!("passthrough failed: {error}");
        }
    }
}
