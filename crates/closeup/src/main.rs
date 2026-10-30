use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{ArgAction, Parser};
use closeup::capture::{Capture, SharedCapture};
use closeup::config::{self, Config};
use closeup::stage::{AppEvent, Stage};
use closeup::{doctor, input, overlay};
use winit::event_loop::EventLoop;

/// Live zoom for screen sharing: share the "Closeup Stage" window in your
/// meeting, then press 1-9 to zoom the audience's view toward your cursor.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Check permissions, the GNOME Shell extension, the portal and the GPU, then exit.
    #[arg(long)]
    doctor: bool,
    /// Forget the remembered source and open the picker.
    #[arg(long)]
    pick: bool,
    /// More log detail; repeat for library logs too.
    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let filter = match cli.verbose {
        0 => "warn,closeup=info,sctk_adwaita=off",
        1 => "warn,closeup=debug,sctk_adwaita=off",
        _ => "debug",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(filter)).init();
    if cli.doctor {
        return doctor::run();
    }
    if cli.pick {
        let _ = std::fs::remove_file(config::restore_token_path());
    }
    match run(Config::load()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(config: Config) -> Result<()> {
    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let proxy = Mutex::new(event_loop.create_proxy());
    let input = input::spawn(Arc::new(move |action| {
        let _ = proxy.lock().unwrap().send_event(AppEvent::Action(action));
    }));
    let shared = SharedCapture::default();
    let capture = Capture::spawn(shared.clone(), config::restore_token_path());
    let overlay = overlay::spawn(input.clone(), config.levels.len());
    let display = Box::new(event_loop.owned_display_handle());
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(display));

    let mut stage = Stage::new(config, capture, input, shared, instance, overlay);
    event_loop.run_app(&mut stage)?;
    Ok(())
}
