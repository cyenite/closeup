use futures_util::StreamExt;
use tokio::sync::watch;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/session/auto"
)]
trait Session {
    #[zbus(property)]
    fn active(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
}

/// Keeps `is_live` true only while this login session is in the foreground
/// and unlocked. The keyboard grab spans the whole seat, so without this a
/// locked screen or another user's session would have its keys routed
/// through us.
pub async fn watch(is_live: watch::Sender<bool>) {
    let proxy = match zbus::Connection::system().await {
        Ok(connection) => SessionProxy::new(&connection).await,
        Err(error) => Err(error),
    };
    let proxy = match proxy {
        Ok(proxy) => proxy,
        Err(error) => {
            log::warn!("cannot follow the login session ({error}); keyboards stay grabbed while locked");
            return;
        }
    };
    let mut active = proxy.receive_active_changed().await;
    let mut locked = proxy.receive_locked_hint_changed().await;
    loop {
        let is_active = proxy.active().await.unwrap_or(true);
        let is_locked = proxy.locked_hint().await.unwrap_or(false);
        let now = is_active && !is_locked;
        is_live.send_if_modified(|current| {
            let is_changed = *current != now;
            if is_changed {
                *current = now;
                log::info!(
                    "{}",
                    if now {
                        "session active: zoom keys on"
                    } else {
                        "session locked or switched away: keyboards released"
                    }
                );
            }
            is_changed
        });
        tokio::select! {
            Some(_) = active.next() => {}
            Some(_) = locked.next() => {}
            else => return,
        }
    }
}
