use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::shell::{self, ExtensionProxy};

/// Answers "does a text field have keyboard focus?" by asking the shell
/// extension, which sees every input-method focus change. A slow or
/// missing shell must never stall the keyboard, hence the short timeout
/// and the last known answer as fallback.
pub struct FocusProbe {
    last: AtomicBool,
    proxy: Option<ExtensionProxy<'static>>,
}

impl FocusProbe {
    const TIMEOUT: Duration = Duration::from_millis(50);

    pub async fn connect() -> Self {
        let proxy = match shell::connect().await {
            Ok(proxy) => Some(proxy),
            Err(error) => {
                log::warn!("text-field detection off, no session bus: {error}");
                None
            }
        };
        Self { last: AtomicBool::new(false), proxy }
    }

    pub async fn is_text_focused(&self) -> bool {
        let Some(proxy) = &self.proxy else { return false };
        match tokio::time::timeout(Self::TIMEOUT, proxy.is_typing()).await {
            Ok(Ok(is_typing)) => {
                self.last.store(is_typing, Ordering::Relaxed);
                is_typing
            }
            Ok(Err(_)) => false,
            Err(_) => self.last.load(Ordering::Relaxed),
        }
    }
}
