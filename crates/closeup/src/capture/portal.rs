use std::os::fd::OwnedFd;

use anyhow::{Context, Result};
use ashpd::WindowIdentifier;
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};

/// Asks the shell extension to raise the picker when it appears. Optional:
/// without the extension the parent window alone has to do.
async fn expect_picker(is_expected: bool) {
    if let Ok(proxy) = crate::shell::connect().await {
        let _ = proxy.expect_picker(is_expected).await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Monitor,
    Window,
    Virtual,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Monitor => "monitor",
            Self::Window => "window",
            Self::Virtual => "virtual",
        }
    }
}

/// What the portal tells us about the chosen source, in logical
/// (compositor) coordinates. Window sources come without a position.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub kind: SourceKind,
    pub size: Option<(i32, i32)>,
    pub position: Option<(i32, i32)>,
}

pub struct OpenedSource {
    pub fd: OwnedFd,
    pub node_id: u32,
    pub info: SourceInfo,
    pub session: Session<Screencast>,
    pub restore_token: Option<String>,
}

pub async fn open(restore_token: Option<String>, parent: Option<&WindowIdentifier>) -> Result<OpenedSource> {
    let proxy = Screencast::new().await.context("screencast portal unavailable")?;
    let session = proxy.create_session(Default::default()).await?;
    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Metadata)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_restore_token(restore_token.as_deref())
                .set_persist_mode(PersistMode::ExplicitlyRevoked),
        )
        .await?
        .response()?;
    expect_picker(true).await;
    let started = proxy.start(&session, parent, Default::default()).await;
    expect_picker(false).await;
    let streams = started?.response()?;
    let stream = streams.streams().first().context("no source selected")?.clone();
    let fd = proxy.open_pipe_wire_remote(&session, Default::default()).await?;
    let kind = match stream.source_type() {
        Some(SourceType::Window) => SourceKind::Window,
        Some(SourceType::Virtual) => SourceKind::Virtual,
        _ => SourceKind::Monitor,
    };
    log::info!(
        "streaming {} source (node {}, logical size {:?} at {:?})",
        kind.label(),
        stream.pipe_wire_node_id(),
        stream.size(),
        stream.position()
    );
    Ok(OpenedSource {
        fd,
        node_id: stream.pipe_wire_node_id(),
        info: SourceInfo { kind, size: stream.size(), position: stream.position() },
        session,
        restore_token: streams.restore_token().map(str::to_owned),
    })
}
