//! `vktr acp`: Viktor as an Agent Client Protocol agent over stdio.
//!
//! This is the native Rust port of the TypeScript `viktor-acp` package, which it replaces. An
//! editor (Zed, a JetBrains IDE, any ACP client) runs `vktr acp` as its agent command and talks
//! to Viktor, the AI employee, in its own cloud sandbox.
//!
//! It is deliberately *not* the same thing as `vktr agent stdio`, which serves vktr's local
//! coding agent over the same protocol. Here Viktor does the work in its own cloud sandbox with
//! the team's tools; it reaches the editor's workspace only through the file and terminal
//! capabilities the editor itself offers, and the editor asks the user before any write or
//! command (see `tools`).

#![deny(clippy::indexing_slicing)]

mod agent;
mod client;
mod content;
mod error;
mod mcp;
mod store;
mod tools;

pub use self::agent::{AUTH_METHOD_ID, AgentConfig, UpdateSender, ViktorAcpAgent};
pub use self::client::{VIKTOR_MODEL_ID, ViktorClient, resolve_base_url};
pub use self::content::to_responses_input;
pub use self::error::{ViktorError, ViktorErrorCode};
pub use self::store::SessionStore;
pub use self::tools::{READ_FILE, RUN_COMMAND, WRITE_FILE};

use std::rc::Rc;

use agent_client_protocol::{self as acp, Client as _};

/// Serve the ACP agent over one pair of streams (stdin and stdout in the CLI).
///
/// Returns the future that runs the connection; it completes when the client disconnects. Must
/// be awaited inside a `tokio::task::LocalSet`, because the protocol futures are not `Send`.
pub fn serve(
    client: Rc<ViktorClient>,
    config: AgentConfig,
    outgoing: impl futures::AsyncWrite + Unpin + 'static,
    incoming: impl futures::AsyncRead + Unpin + 'static,
) -> impl std::future::Future<Output = Result<(), acp::Error>> {
    let (updates_tx, mut updates_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent = Rc::new(ViktorAcpAgent::new(client, config, updates_tx));
    let (connection, handle_io) =
        acp::AgentSideConnection::new(Rc::clone(&agent), outgoing, incoming, |fut| {
            tokio::task::spawn_local(fut);
        });
    // The agent calls back into the editor (files, terminals, permission prompts) through the
    // same connection that forwards its session updates.
    let connection = Rc::new(connection);
    agent.set_connection(Rc::clone(&connection));
    // Forward the agent's queued session updates to the client, acking each one so the agent
    // can keep a turn's chunks ordered ahead of its result.
    tokio::task::spawn_local(async move {
        while let Some((notification, ack)) = updates_rx.recv().await {
            if let Err(error) = connection.session_notification(notification).await {
                tracing::warn!(%error, "vktr acp: the client stopped accepting session updates");
                break;
            }
            ack.send(()).ok();
        }
    });
    handle_io
}
