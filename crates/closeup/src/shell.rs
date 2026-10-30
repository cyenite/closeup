//! The D-Bus API of the GNOME Shell extension in `gnome-extension/`.

use crate::input::Action;

/// Bumped whenever the interface changes. GNOME on Wayland only loads new
/// extension code at login, so a mismatch means "log out and back in".
pub const PROTOCOL_VERSION: u32 = 2;

#[zbus::proxy(
    interface = "app.closeup.Overlay",
    default_service = "org.gnome.Shell",
    default_path = "/app/closeup/Overlay"
)]
pub trait Extension {
    fn version(&self) -> zbus::Result<u32>;
    fn set_source(&self, kind: &str, x: i32, y: i32, width: i32, height: i32) -> zbus::Result<()>;
    #[allow(clippy::too_many_arguments)]
    fn show(&self, x: f64, y: f64, width: f64, height: f64, zoom: f64, level: u32, is_locked: bool)
    -> zbus::Result<()>;
    fn hide(&self) -> zbus::Result<()>;
    fn set_controls(&self, is_paused: bool, access: &str, levels: u32) -> zbus::Result<()>;
    fn is_typing(&self) -> zbus::Result<bool>;
    fn expect_picker(&self, is_expected: bool) -> zbus::Result<()>;
    fn grab_shortcuts(&self, levels: u32) -> zbus::Result<bool>;
    #[zbus(signal)]
    fn shortcut_activated(&self, shortcut: String) -> zbus::Result<()>;
}

pub async fn connect() -> zbus::Result<ExtensionProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    ExtensionProxy::new(&connection).await
}

/// A Ctrl+Alt shortcut the shell owns and reports back by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Level(usize),
    Lock,
    Pause,
    Source,
}

impl Shortcut {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "lock" => Some(Self::Lock),
            "pause" => Some(Self::Pause),
            "source" => Some(Self::Source),
            _ => name.strip_prefix("level-")?.parse().ok().filter(|level| (1..=9).contains(level)).map(Self::Level),
        }
    }

    pub fn action(self) -> Action {
        match self {
            Self::Level(level) => Action::Level(level),
            Self::Lock => Action::ToggleLock,
            Self::Pause => Action::TogglePause,
            Self::Source => Action::PickSource,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_shortcut_name() {
        assert_eq!(Shortcut::parse("level-1"), Some(Shortcut::Level(1)));
        assert_eq!(Shortcut::parse("level-9"), Some(Shortcut::Level(9)));
        assert_eq!(Shortcut::parse("lock"), Some(Shortcut::Lock));
        assert_eq!(Shortcut::parse("pause"), Some(Shortcut::Pause));
        assert_eq!(Shortcut::parse("source"), Some(Shortcut::Source));
    }

    #[test]
    fn rejects_unknown_names() {
        for name in ["level-0", "level-10", "level-x", "zoom", ""] {
            assert_eq!(Shortcut::parse(name), None, "{name}");
        }
    }
}
