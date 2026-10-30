use std::process::ExitCode;

use ashpd::desktop::screencast::{CursorMode, Screencast};

use crate::config;
use crate::input::{self, GroupState};
use crate::shell::{self, PROTOCOL_VERSION};

const EXTENSION_UUID: &str = "closeup@closeup.app";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "  ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
        }
    }
}

struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self { name, status, detail: detail.into() }
    }
}

/// `closeup --doctor`: checks every prerequisite and says exactly what to
/// do about each problem.
pub fn run() -> ExitCode {
    let checks = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime.block_on(collect()),
        Err(error) => vec![Check::new("runtime", Status::Fail, error.to_string())],
    };
    for check in &checks {
        println!("[{}] {:<16} {}", check.status.label(), check.name, check.detail);
    }
    if checks.iter().any(|check| check.status == Status::Fail) {
        println!("\nSome checks failed; fix those first.");
        ExitCode::FAILURE
    } else {
        println!("\nReady.");
        ExitCode::SUCCESS
    }
}

async fn collect() -> Vec<Check> {
    vec![session(), keyboards(), passthrough(), extension().await, portal().await, gpu().await, saved_source()]
}

fn session() -> Check {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let detail = format!("{session} on {desktop}");
    if !desktop.to_lowercase().contains("gnome") {
        return Check::new(
            "session",
            Status::Warn,
            format!("{detail}: the overlay and typing detection need GNOME Shell"),
        );
    }
    if session != "wayland" {
        return Check::new("session", Status::Warn, format!("{detail}: built and tested for Wayland"));
    }
    Check::new("session", Status::Pass, detail)
}

fn keyboards() -> Check {
    match input::input_group_state() {
        GroupState::Active => match input::readable_keyboards() {
            0 => Check::new("keyboards", Status::Fail, input::permission_hint()),
            count => Check::new("keyboards", Status::Pass, format!("{count} readable; bare digits can zoom")),
        },
        _ => Check::new("keyboards", Status::Fail, input::permission_hint()),
    }
}

fn passthrough() -> Check {
    if input::can_write_uinput() {
        Check::new("uinput", Status::Pass, "writable; other keys reach apps untouched")
    } else {
        Check::new(
            "uinput",
            Status::Warn,
            "/dev/uinput not writable: bare digits can't be swallowed (run setup/install.sh)",
        )
    }
}

async fn extension() -> Check {
    let is_installed = dirs::data_dir()
        .map(|data| data.join("gnome-shell/extensions").join(EXTENSION_UUID))
        .is_some_and(|path| path.exists());
    let version = match shell::connect().await {
        Ok(proxy) => proxy.version().await,
        Err(error) => Err(error),
    };
    match version {
        Ok(PROTOCOL_VERSION) => Check::new("shell extension", Status::Pass, format!("protocol {PROTOCOL_VERSION}")),
        Ok(other) => Check::new(
            "shell extension",
            Status::Fail,
            format!("protocol {other} loaded, {PROTOCOL_VERSION} needed: log out and back in"),
        ),
        Err(_) if is_installed => Check::new(
            "shell extension",
            Status::Fail,
            format!(
                "installed but not running the current version: `gnome-extensions enable {EXTENSION_UUID}`, then log out and back in"
            ),
        ),
        Err(_) => Check::new("shell extension", Status::Fail, "not installed: run setup/install.sh"),
    }
}

async fn portal() -> Check {
    let proxy = match Screencast::new().await {
        Ok(proxy) => proxy,
        Err(error) => return Check::new("screencast", Status::Fail, format!("portal unavailable: {error}")),
    };
    match proxy.available_cursor_modes().await {
        Ok(modes) if modes.contains(CursorMode::Metadata) => {
            Check::new("screencast", Status::Pass, format!("portal v{}, cursor metadata", proxy.version()))
        }
        Ok(_) => Check::new("screencast", Status::Warn, "no cursor metadata: the cursor won't stay sharp when zoomed"),
        Err(error) => Check::new("screencast", Status::Warn, format!("cursor modes unknown: {error}")),
    }
}

async fn gpu() -> Check {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    match instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await {
        Ok(adapter) => {
            let info = adapter.get_info();
            Check::new("gpu", Status::Pass, format!("{} ({:?})", info.name, info.backend))
        }
        Err(error) => Check::new("gpu", Status::Fail, format!("no usable adapter: {error}")),
    }
}

fn saved_source() -> Check {
    if config::restore_token_path().exists() {
        Check::new("saved source", Status::Pass, "remembered; `closeup --pick` chooses again")
    } else {
        Check::new("saved source", Status::Pass, "none yet; the picker opens on start")
    }
}
