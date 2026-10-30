use std::thread;
use std::time::Duration;

use closeup_core::Rect;
use futures_util::StreamExt;
use tokio::sync::watch;

use crate::capture::SourceInfo;
use crate::input::{InputHandle, KeyAccess};
use crate::shell::{self, ExtensionProxy, PROTOCOL_VERSION, Shortcut};

const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Everything the presenter's overlay needs; `region` is normalized to the
/// source (0..1) so the shell can place it without knowing pixel scales.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayState {
    pub zoom: f64,
    pub level: usize,
    pub levels: usize,
    pub generation: u64,
    pub access: KeyAccess,
    pub is_locked: bool,
    pub is_paused: bool,
    pub region: Option<Rect>,
    pub source: Option<SourceKey>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceKey {
    pub kind: &'static str,
    pub size: (i32, i32),
    pub position: (i32, i32),
}

impl From<&SourceInfo> for SourceKey {
    fn from(info: &SourceInfo) -> Self {
        Self {
            kind: info.kind.label(),
            size: info.size.unwrap_or((-1, -1)),
            position: info.position.unwrap_or((-1, -1)),
        }
    }
}

/// Runs the app's side of the GNOME Shell extension: streams the overlay
/// state, hands the Ctrl+Alt shortcuts to the shell and relays them back.
pub fn spawn(input: InputHandle, levels: usize) -> watch::Sender<OverlayState> {
    let (sender, receiver) = watch::channel(OverlayState::default());
    thread::Builder::new()
        .name("shell".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
                return;
            };
            runtime.block_on(run(receiver, input, levels));
        })
        .expect("spawn shell thread");
    sender
}

async fn run(mut states: watch::Receiver<OverlayState>, input: InputHandle, levels: usize) {
    let proxy = match shell::connect().await {
        Ok(proxy) => proxy,
        Err(error) => return log::warn!("overlay and shell shortcuts disabled, no session bus: {error}"),
    };
    let mut shortcuts = match proxy.receive_shortcut_activated().await {
        Ok(stream) => Some(stream),
        Err(error) => {
            log::warn!("cannot listen for shell shortcuts: {error}");
            None
        }
    };
    let mut sent: Option<OverlayState> = None;
    let mut is_attached = false;
    let mut has_warned = false;
    loop {
        if !is_attached {
            is_attached = attach(&proxy, &input, levels, &mut has_warned).await;
            if is_attached {
                sent = None;
                states.mark_changed();
            }
        }
        tokio::select! {
            changed = states.changed() => {
                if changed.is_err() {
                    break;
                }
                if !is_attached {
                    continue;
                }
                let state = states.borrow_and_update().clone();
                match push(&proxy, sent.as_ref(), &state).await {
                    Ok(()) => sent = Some(state),
                    Err(error) => {
                        log::warn!("lost the GNOME Shell extension ({error}); retrying");
                        input.set_shell_shortcuts(false);
                        is_attached = false;
                    }
                }
            }
            Some(signal) = next_shortcut(&mut shortcuts) => {
                let shortcut = signal.args().ok().and_then(|args| Shortcut::parse(args.shortcut()));
                if let Some(shortcut) = shortcut {
                    input.dispatch(shortcut.action());
                }
            }
            _ = tokio::time::sleep(RETRY_INTERVAL), if !is_attached => {}
        }
    }
}

async fn next_shortcut(stream: &mut Option<shell::ShortcutActivatedStream>) -> Option<shell::ShortcutActivated> {
    match stream {
        Some(stream) => stream.next().await,
        None => std::future::pending().await,
    }
}

/// Checks the extension speaks our protocol, then registers the shortcuts.
/// Warnings are logged once per outage, not on every retry.
async fn attach(proxy: &ExtensionProxy<'_>, input: &InputHandle, levels: usize, has_warned: &mut bool) -> bool {
    let problem = match proxy.version().await {
        Ok(PROTOCOL_VERSION) => None,
        Ok(version) => Some(format!(
            "the loaded GNOME Shell extension speaks protocol {version}, this app needs {PROTOCOL_VERSION}; log out and back in to load the installed one"
        )),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" => {
            Some("the loaded GNOME Shell extension is outdated; log out and back in to load the installed one".into())
        }
        Err(error) => Some(format!(
            "GNOME Shell extension not reachable ({error}); run setup/install.sh and enable closeup@closeup.app"
        )),
    };
    if let Some(problem) = problem {
        if !*has_warned {
            log::warn!("{problem}");
            *has_warned = true;
        }
        return false;
    }
    *has_warned = false;
    match proxy.grab_shortcuts(levels.min(9) as u32).await {
        Ok(true) => {
            log::info!("Ctrl+Alt shortcuts registered with GNOME Shell");
            input.set_shell_shortcuts(true);
        }
        Ok(false) => {
            log::warn!("GNOME Shell refused the Ctrl+Alt shortcuts (another app holds them); using keyboard access");
            input.set_shell_shortcuts(false);
        }
        Err(error) => {
            log::warn!("could not register shell shortcuts: {error}");
            return false;
        }
    }
    true
}

/// Sends only what changed since the last successful push; `None` means
/// the extension has never heard from us, so everything goes out.
async fn push(proxy: &ExtensionProxy<'_>, sent: Option<&OverlayState>, state: &OverlayState) -> zbus::Result<()> {
    let is_changed = |differs: fn(&OverlayState, &OverlayState) -> bool| sent.is_none_or(|sent| differs(sent, state));
    if is_changed(|sent, state| sent.source != state.source || sent.generation != state.generation)
        && let Some(source) = &state.source
    {
        let (x, y) = source.position;
        let (width, height) = source.size;
        proxy.set_source(source.kind, x, y, width, height).await?;
    }
    if is_changed(|sent, state| {
        sent.is_paused != state.is_paused || sent.access != state.access || sent.levels != state.levels
    }) {
        proxy.set_controls(state.is_paused, state.access.label(), state.levels as u32).await?;
    }
    if !is_changed(|sent, state| {
        sent.region != state.region
            || sent.zoom != state.zoom
            || sent.level != state.level
            || sent.is_locked != state.is_locked
    }) {
        return Ok(());
    }
    match state.region {
        Some(region) => {
            let level = state.level as u32;
            let (origin, size) = (region.origin, region.size);
            proxy.show(origin.x, origin.y, size.x, size.y, state.zoom, level, state.is_locked).await
        }
        None => proxy.hide().await,
    }
}
