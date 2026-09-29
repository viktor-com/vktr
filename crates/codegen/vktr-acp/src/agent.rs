//! Viktor as an Agent Client Protocol agent.
//!
//! One ACP session is one Viktor thread: the first prompt starts a thread, later prompts continue
//! it through `previous_response_id`, so Viktor keeps its sandbox state and context for the whole
//! editor session. The mapping is stored on disk, so `session/load` and `session/resume` bring a
//! session back on the same thread after the agent restarts.
//!
//! Viktor works in its own cloud sandbox. When the editor offers file-system or terminal
//! capabilities, they are exposed to Viktor as caller tools (see [`crate::tools`]): Viktor's call
//! comes back on the stream, runs through the editor (asking the user first for writes and
//! commands), and the result goes back on the same thread.
//!
//! Text streaming, content mapping and error reporting are ported from the TypeScript
//! `viktor-acp` agent, so an editor that only chats cannot tell the two apart on the wire.

use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use agent_client_protocol::{self as acp, Client as _};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::client::ViktorClient;
use crate::error::{ViktorError, ViktorErrorCode};
use crate::mcp::McpHub;
use crate::store::{Entry, OwedOutput, SessionStore, StoredSession};
use crate::tools::{self, EditorTools, FunctionCall, Request};

/// The id of the only authentication method: a Viktor API key from the environment or the
/// `vktr login` store.
pub const AUTH_METHOD_ID: &str = "viktor-api-key";

const AUTH_METHOD_DESCRIPTION: &str = "Set VIKTOR_API_KEY to a Viktor API key (zt_live_sk_…) with \
     the chat:completions scope, or save one with `vktr login`.";

/// Tool rounds allowed in one prompt turn before the agent gives up on a looping run.
const MAX_TOOL_ROUNDS: usize = 64;

/// How long a turn waits for a thread that is still busy (HTTP 409 `conversation_busy`), which
/// is what the next prompt meets for about 15 s after a cancelled turn. Past this, Viktor's own
/// message is shown.
const BUSY_WAIT: Duration = Duration::from_secs(120);

/// What Viktor is told about a call the user declined.
const REJECTED: &str = "The user rejected this call, so it did not run. Do not retry it \
     unchanged; ask the user how to proceed if you need it.";

/// What Viktor is told about calls left unanswered when the user cancelled the turn.
const CANCELLED: &str = "The user cancelled the turn before this call ran.";

/// A queued `session/update` plus the ack that keeps updates ordered ahead of the turn's result.
pub type UpdateSender = mpsc::UnboundedSender<(acp::SessionNotification, oneshot::Sender<()>)>;

/// How the agent is set up by the command that serves it.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// Where sessions are persisted; `None` keeps them in memory only (no `session/load`).
    pub store: Option<SessionStore>,
    /// Offer the editor's file-system and terminal capabilities to Viktor as tools.
    pub editor_tools: bool,
    /// Connect to the MCP servers the editor lists and offer their tools to Viktor.
    pub mcp_servers: bool,
    /// The version reported to the editor in `agentInfo`: the vktr release, not this crate's.
    pub version: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            store: None,
            editor_tools: true,
            mcp_servers: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

struct SessionState {
    /// Thread id, workspace root and transcript; what the store keeps.
    stored: StoredSession,
    /// Cancels the turn currently in flight, if any.
    pending: Option<Rc<CancellationToken>>,
    /// Tools the user chose "always allow" for, for the rest of this session.
    always_allow: HashSet<String>,
    /// The editor's MCP servers for this session, offered to Viktor as tools.
    mcp: Option<Rc<McpHub>>,
    /// MCP servers that failed to start; told to the user at the start of the next turn.
    mcp_warnings: Vec<String>,
    /// Held for the whole of a turn. A prompt that supersedes a running one cancels it and then
    /// waits here, so the two never interleave on the Viktor thread.
    turn_lock: Rc<tokio::sync::Mutex<()>>,
}

impl SessionState {
    fn new(stored: StoredSession) -> Self {
        Self {
            stored,
            pending: None,
            always_allow: HashSet::new(),
            mcp: None,
            mcp_warnings: Vec::new(),
            turn_lock: Rc::new(tokio::sync::Mutex::new(())),
        }
    }
}

/// The ACP agent. Holds one Viktor client and the live sessions.
pub struct ViktorAcpAgent {
    client: Rc<ViktorClient>,
    config: AgentConfig,
    sessions: RefCell<HashMap<acp::SessionId, SessionState>>,
    updates: UpdateSender,
    /// The connection back to the editor, for file, terminal and permission requests. Set once
    /// by [`crate::serve`] right after the connection is built around this agent.
    connection: OnceCell<Rc<acp::AgentSideConnection>>,
    /// What the editor said it can do, from `initialize`.
    client_capabilities: RefCell<acp::ClientCapabilities>,
}

impl ViktorAcpAgent {
    pub fn new(client: Rc<ViktorClient>, config: AgentConfig, updates: UpdateSender) -> Self {
        Self {
            client,
            config,
            sessions: RefCell::new(HashMap::new()),
            updates,
            connection: OnceCell::new(),
            client_capabilities: RefCell::new(acp::ClientCapabilities::default()),
        }
    }

    /// Give the agent its connection to the editor. Called once by [`crate::serve`].
    pub fn set_connection(&self, connection: Rc<acp::AgentSideConnection>) {
        self.connection.set(connection).ok();
    }

    fn connection(&self) -> Result<Rc<acp::AgentSideConnection>, acp::Error> {
        self.connection
            .get()
            .cloned()
            .ok_or_else(acp::Error::internal_error)
    }

    /// Send one `session/update` and wait for it to reach the client, so the whole reply is
    /// delivered before `session/prompt` returns its stop reason.
    async fn notify(
        &self,
        session_id: &acp::SessionId,
        update: acp::SessionUpdate,
    ) -> Result<(), acp::Error> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.updates
            .send((
                acp::SessionNotification::new(session_id.clone(), update),
                ack_tx,
            ))
            .map_err(|_| acp::Error::internal_error())?;
        ack_rx.await.map_err(|_| acp::Error::internal_error())
    }

    async fn update_tool(
        &self,
        session_id: &acp::SessionId,
        call_id: &str,
        fields: acp::ToolCallUpdateFields,
    ) -> Result<(), TurnError> {
        self.notify(
            session_id,
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                call_id.to_owned(),
                fields,
            )),
        )
        .await
        .map_err(TurnError::Acp)
    }

    fn not_found(session_id: &acp::SessionId) -> acp::Error {
        acp::Error::new(
            acp::ErrorCode::InvalidParams.into(),
            format!("Session {session_id} not found"),
        )
    }

    /// Run `f` on a live session. Never hold the borrow across an await.
    fn with_session<T>(
        &self,
        session_id: &acp::SessionId,
        f: impl FnOnce(&mut SessionState) -> T,
    ) -> Result<T, acp::Error> {
        self.sessions
            .borrow_mut()
            .get_mut(session_id)
            .map(f)
            .ok_or_else(|| Self::not_found(session_id))
    }

    /// Start a turn: supersede any turn still running in this session and take a fresh token.
    /// Returns the turn's token and the session's turn lock, which the caller must hold for the
    /// whole turn.
    fn begin_turn(
        &self,
        session_id: &acp::SessionId,
    ) -> Result<(Rc<CancellationToken>, Rc<tokio::sync::Mutex<()>>), acp::Error> {
        self.with_session(session_id, |session| {
            let token = Rc::new(CancellationToken::new());
            if let Some(previous) = session.pending.replace(Rc::clone(&token)) {
                previous.cancel();
            }
            (token, Rc::clone(&session.turn_lock))
        })
    }

    /// Drop the turn's token unless a newer turn already replaced it.
    fn end_turn(&self, session_id: &acp::SessionId, token: &Rc<CancellationToken>) {
        self.with_session(session_id, |session| {
            if session
                .pending
                .as_ref()
                .is_some_and(|t| Rc::ptr_eq(t, token))
            {
                session.pending = None;
            }
        })
        .ok();
    }

    fn thread_id(&self, session_id: &acp::SessionId) -> Option<String> {
        self.with_session(session_id, |s| s.stored.thread_id.clone())
            .ok()
            .flatten()
    }

    fn record(&self, session_id: &acp::SessionId, entry: Entry) {
        self.with_session(session_id, |s| s.stored.push(entry)).ok();
    }

    /// Write the session to the store, if there is one. A failure is logged, not fatal: the
    /// turn itself succeeded, only a later `session/load` would miss it.
    fn persist(&self, session_id: &acp::SessionId) {
        let Some(store) = &self.config.store else {
            return;
        };
        let Ok(mut stored) = self.with_session(session_id, |s| s.stored.clone()) else {
            return;
        };
        if let Err(error) = store.save(&mut stored) {
            tracing::warn!(%error, "vktr acp: could not save the session");
        }
    }

    /// The editor tools this session offers Viktor.
    fn offered_tools(&self) -> EditorTools {
        if self.config.editor_tools {
            EditorTools::from_capabilities(&self.client_capabilities.borrow())
        } else {
            EditorTools::default()
        }
    }

    /// One prompt turn: stream Viktor's run, and while it asks for editor tools, run them and
    /// send the results back on the same thread.
    ///
    /// Every tool output is recorded as owed the moment it exists, and dropped only when Viktor
    /// accepts a request carrying it, so no exit path (an error, a cancel, the round cap, an
    /// agent restart) can leave the thread with a call it never gets an answer to.
    async fn run_turn(
        &self,
        session_id: &acp::SessionId,
        prompt: &[acp::ContentBlock],
        token: &CancellationToken,
    ) -> Result<acp::StopReason, TurnError> {
        let cwd = self
            .with_session(session_id, |s| s.stored.cwd.clone())
            .map_err(TurnError::Acp)?;
        let offered = self.offered_tools();
        let mut definitions = offered.definitions(&cwd);
        let (hub, warnings) = self
            .with_session(session_id, |s| {
                (s.mcp.clone(), std::mem::take(&mut s.mcp_warnings))
            })
            .map_err(TurnError::Acp)?;
        if let Some(hub) = &hub {
            definitions.extend(hub.definitions());
        }
        for warning in warnings {
            self.notify(
                session_id,
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    acp::ContentBlock::Text(acp::TextContent::new(warning)),
                )),
            )
            .await
            .map_err(TurnError::Acp)?;
        }

        self.record(
            session_id,
            Entry::User {
                text: prompt_text(prompt),
            },
        );
        // The user's message goes out once, after whatever Viktor is still owed.
        let mut message = Some(json!({
            "role": "user",
            "content": crate::content::to_responses_input(prompt),
        }));

        for _ in 0..MAX_TOOL_ROUNDS {
            let owed = self
                .with_session(session_id, |s| s.stored.owed_outputs.clone())
                .map_err(TurnError::Acp)?;
            let mut input: Vec<Value> = owed
                .iter()
                .map(|o| function_call_output(&o.call_id, &o.output))
                .collect();
            input.extend(message.take());
            let run = self
                .stream_run(session_id, input, &owed, &definitions, token)
                .await?;
            let calls = match run {
                RunOutcome::Cancelled => return Ok(acp::StopReason::Cancelled),
                RunOutcome::Finished { stop_reason, calls } if calls.is_empty() => {
                    return Ok(stop_reason);
                }
                RunOutcome::Finished { calls, .. } => calls,
            };
            if self.thread_id(session_id).is_none() {
                return Err(TurnError::Viktor(ViktorError::new(
                    ViktorErrorCode::ServerError,
                    "Viktor asked for a tool without naming its thread, so the result cannot \
                     be sent back",
                )));
            }
            for (index, call) in calls.iter().enumerate() {
                let outcome = self.run_tool(session_id, call, offered, &cwd, token).await;
                if let Ok(ToolOutcome::Output(output)) = outcome {
                    self.owe(session_id, &call.call_id, output);
                    continue;
                }
                // Cancelled, or the editor connection failed: every call still unrun is
                // answered as cancelled when the thread next moves.
                for left in calls.iter().skip(index) {
                    self.owe(session_id, &left.call_id, CANCELLED.to_owned());
                }
                return outcome.map(|_| acp::StopReason::Cancelled);
            }
        }
        Err(TurnError::Viktor(ViktorError::new(
            ViktorErrorCode::ServerError,
            format!(
                "Viktor was still calling tools after {MAX_TOOL_ROUNDS} rounds; the turn was \
                 stopped (the last results go to Viktor with your next message)"
            ),
        )))
    }

    fn owe(&self, session_id: &acp::SessionId, call_id: &str, output: String) {
        self.with_session(session_id, |s| {
            s.stored.owed_outputs.push(OwedOutput {
                call_id: call_id.to_owned(),
                output,
            });
        })
        .ok();
    }

    /// Start a run, waiting out a thread that is still busy with the run of a cancelled turn.
    /// `None` means the turn was cancelled while waiting.
    async fn open_stream(
        &self,
        session_id: &acp::SessionId,
        body: Value,
        token: &CancellationToken,
    ) -> Result<Option<crate::client::ResponsesStream>, TurnError> {
        let deadline = tokio::time::Instant::now() + BUSY_WAIT;
        let mut delay = Duration::from_secs(2);
        let mut told = false;
        loop {
            let result = tokio::select! {
                biased;
                () = token.cancelled() => return Ok(None),
                result = self.client.responses_stream(body.clone()) => result,
            };
            match result {
                Ok(stream) => return Ok(Some(stream)),
                Err(error)
                    if error.is_conversation_busy()
                        && tokio::time::Instant::now() + delay < deadline =>
                {
                    if !told {
                        told = true;
                        self.notify(
                            session_id,
                            acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                                acp::ContentBlock::Text(acp::TextContent::new(
                                    "Viktor is still stopping the previous run; waiting for it.",
                                )),
                            )),
                        )
                        .await
                        .map_err(TurnError::Acp)?;
                    }
                    tokio::select! {
                        biased;
                        () = token.cancelled() => return Ok(None),
                        () = tokio::time::sleep(delay) => {}
                    }
                    delay = (delay * 3 / 2).min(Duration::from_secs(8));
                }
                Err(error) => return Err(TurnError::Viktor(error)),
            }
        }
    }

    /// Stream one Viktor run: its text becomes `agent_message_chunk` updates, its tool calls are
    /// collected for the caller.
    async fn stream_run(
        &self,
        session_id: &acp::SessionId,
        input: Vec<Value>,
        owed: &[OwedOutput],
        tools: &[Value],
        token: &CancellationToken,
    ) -> Result<RunOutcome, TurnError> {
        let mut body = json!({ "input": input });
        if let Some(object) = body.as_object_mut() {
            if let Some(thread_id) = self.thread_id(session_id) {
                object.insert("previous_response_id".to_owned(), json!(thread_id));
            }
            if !tools.is_empty() {
                object.insert("tools".to_owned(), json!(tools));
            }
        }

        let Some(mut stream) = self.open_stream(session_id, body, token).await? else {
            return Ok(RunOutcome::Cancelled);
        };
        // Viktor accepted the request, so the outputs it carried are delivered.
        self.with_session(session_id, |s| {
            s.stored.owed_outputs.retain(|o| !owed.contains(o));
        })
        .ok();

        let mut stop_reason = acp::StopReason::EndTurn;
        let mut calls: Vec<FunctionCall> = Vec::new();
        let mut text = String::new();
        let outcome = loop {
            let event = tokio::select! {
                biased;
                () = token.cancelled() => break Ok(RunOutcome::Cancelled),
                event = stream.next_event() => event,
            };
            let Some(event) = event else {
                break Ok(RunOutcome::Finished {
                    stop_reason,
                    calls: std::mem::take(&mut calls),
                });
            };
            let event = match event {
                Ok(event) => event,
                Err(error) => break Err(TurnError::Viktor(error)),
            };
            match event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "response.output_text.delta" => {
                    if let Some(delta) = event.get("delta").and_then(Value::as_str)
                        && !delta.is_empty()
                    {
                        text.push_str(delta);
                        if let Err(error) = self
                            .notify(
                                session_id,
                                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                                    acp::ContentBlock::Text(acp::TextContent::new(delta)),
                                )),
                            )
                            .await
                        {
                            break Err(TurnError::Acp(error));
                        }
                    }
                }
                "response.output_item.done" => {
                    if let Some(call) = event.get("item").and_then(FunctionCall::from_item) {
                        add_call(&mut calls, call);
                    }
                }
                kind @ ("response.completed" | "response.incomplete") => {
                    let response = event.get("response");
                    if let Some(id) = response.and_then(|r| r.get("id")).and_then(Value::as_str) {
                        let id = id.to_owned();
                        self.with_session(session_id, |s| s.stored.thread_id = Some(id))
                            .ok();
                    }
                    // A call only listed in the final response still has to be answered.
                    for item in response
                        .and_then(|r| r.get("output"))
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(call) = FunctionCall::from_item(item) {
                            add_call(&mut calls, call);
                        }
                    }
                    if kind == "response.incomplete" {
                        // Viktor's 600 s run cap; the closest ACP stop reason.
                        stop_reason = acp::StopReason::MaxTokens;
                    }
                }
                _ => {}
            }
        };
        if !text.is_empty() {
            self.record(session_id, Entry::Agent { text });
        }
        outcome
    }

    /// Run one editor tool call and produce the output Viktor gets back.
    async fn run_tool(
        &self,
        session_id: &acp::SessionId,
        call: &FunctionCall,
        offered: EditorTools,
        cwd: &Path,
        token: &CancellationToken,
    ) -> Result<ToolOutcome, TurnError> {
        let raw_input: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
        if call.name.starts_with(crate::mcp::PREFIX)
            && let Some(hub) = self
                .with_session(session_id, |s| s.mcp.clone())
                .ok()
                .flatten()
            && hub.describe(&call.name).is_some()
        {
            return self
                .run_mcp_tool(session_id, call, &hub, raw_input, token)
                .await;
        }
        let request = match Request::parse(call, offered, cwd) {
            Ok(request) => request,
            Err(message) => {
                let failed = acp::ToolCall::new(call.call_id.clone(), call.name.clone())
                    .kind(acp::ToolKind::Other)
                    .status(acp::ToolCallStatus::Failed)
                    .raw_input(raw_input)
                    .content(vec![text_content(&message)]);
                self.report_call(session_id, failed).await?;
                return Ok(ToolOutcome::Output(message));
            }
        };

        let call_id = call.call_id.as_str();
        let summary = acp::ToolCall::new(call_id.to_owned(), request.title())
            .kind(request.kind())
            .locations(request.locations());
        self.notify(
            session_id,
            acp::SessionUpdate::ToolCall(
                summary
                    .clone()
                    .status(acp::ToolCallStatus::Pending)
                    .raw_input(raw_input),
            ),
        )
        .await
        .map_err(TurnError::Acp)?;

        // A write shows its diff both in the approval prompt and in the finished call.
        let diff = match &request {
            Request::Write { path, content } => {
                let old_text = if offered.read {
                    self.read_file(session_id, path, None, None).await.ok()
                } else {
                    None
                };
                Some(acp::ToolCallContent::Diff(
                    acp::Diff::new(path.clone(), content.clone()).old_text(old_text),
                ))
            }
            _ => None,
        };

        if request.needs_permission() {
            match self
                .ask_permission(
                    session_id,
                    call_id,
                    Ask {
                        key: request.permission_key().to_owned(),
                        title: request.title(),
                        kind: request.kind(),
                    },
                    diff.clone(),
                    token,
                )
                .await?
            {
                Decision::Allow => {}
                Decision::Reject(reason) => {
                    self.finish_tool(
                        session_id,
                        &summary,
                        acp::ToolCallStatus::Failed,
                        vec![text_content(&reason)],
                    )
                    .await?;
                    return Ok(ToolOutcome::Output(format!("{REJECTED} ({reason})")));
                }
                Decision::Cancelled => {
                    self.finish_tool(session_id, &summary, acp::ToolCallStatus::Failed, vec![])
                        .await?;
                    return Ok(ToolOutcome::Cancelled);
                }
            }
        }

        self.update_tool(
            session_id,
            call_id,
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::InProgress),
        )
        .await?;

        let (status, content, output) = match request {
            Request::Read {
                path, line, limit, ..
            } => match self.read_file(session_id, &path, line, limit).await {
                Ok(text) => {
                    let lines = text.lines().count();
                    (
                        acp::ToolCallStatus::Completed,
                        vec![text_content(&format!("Read {lines} lines"))],
                        tools::cap_read(text),
                    )
                }
                Err(error) => failed(format!("error: could not read {}: {error}", path.display())),
            },
            Request::Write { path, content } => {
                let written = content.len();
                match self.write_file(session_id, &path, content).await {
                    Ok(()) => (
                        acp::ToolCallStatus::Completed,
                        diff.into_iter().collect(),
                        format!("Wrote {written} bytes to {}", path.display()),
                    ),
                    Err(error) => failed(format!(
                        "error: could not write {}: {error}",
                        path.display()
                    )),
                }
            }
            Request::Run { command, cwd } => {
                match self
                    .run_command(session_id, call_id, &command, &cwd, token)
                    .await?
                {
                    CommandOutcome::Finished {
                        terminal_id,
                        output,
                    } => (
                        acp::ToolCallStatus::Completed,
                        vec![acp::ToolCallContent::Terminal(acp::Terminal::new(
                            terminal_id,
                        ))],
                        output,
                    ),
                    CommandOutcome::Failed(message) => failed(message),
                    CommandOutcome::Cancelled => {
                        self.finish_tool(session_id, &summary, acp::ToolCallStatus::Failed, vec![])
                            .await?;
                        return Ok(ToolOutcome::Cancelled);
                    }
                }
            }
        };
        self.finish_tool(session_id, &summary, status, content)
            .await?;
        Ok(ToolOutcome::Output(output))
    }

    /// Run a tool of one of the editor's MCP servers, after asking the user.
    async fn run_mcp_tool(
        &self,
        session_id: &acp::SessionId,
        call: &FunctionCall,
        hub: &McpHub,
        raw_input: Value,
        token: &CancellationToken,
    ) -> Result<ToolOutcome, TurnError> {
        let (server, tool) = hub
            .describe(&call.name)
            .map(|(s, t)| (s.to_owned(), t.to_owned()))
            .unwrap_or_default();
        let title = format!("{server}: {tool}");
        let call_id = call.call_id.as_str();
        let summary =
            acp::ToolCall::new(call_id.to_owned(), title.clone()).kind(acp::ToolKind::Other);
        self.notify(
            session_id,
            acp::SessionUpdate::ToolCall(
                summary
                    .clone()
                    .status(acp::ToolCallStatus::Pending)
                    .raw_input(raw_input),
            ),
        )
        .await
        .map_err(TurnError::Acp)?;
        let ask = Ask {
            key: call.name.clone(),
            title,
            kind: acp::ToolKind::Other,
        };
        match self
            .ask_permission(session_id, call_id, ask, None, token)
            .await?
        {
            Decision::Allow => {}
            Decision::Reject(reason) => {
                self.finish_tool(
                    session_id,
                    &summary,
                    acp::ToolCallStatus::Failed,
                    vec![text_content(&reason)],
                )
                .await?;
                return Ok(ToolOutcome::Output(format!("{REJECTED} ({reason})")));
            }
            Decision::Cancelled => {
                self.finish_tool(session_id, &summary, acp::ToolCallStatus::Failed, vec![])
                    .await?;
                return Ok(ToolOutcome::Cancelled);
            }
        }
        self.update_tool(
            session_id,
            call_id,
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::InProgress),
        )
        .await?;
        let result = tokio::select! {
            biased;
            () = token.cancelled() => None,
            result = hub.call(&call.name, &call.arguments) => Some(result),
        };
        let Some(result) = result else {
            self.finish_tool(session_id, &summary, acp::ToolCallStatus::Failed, vec![])
                .await?;
            return Ok(ToolOutcome::Cancelled);
        };
        let (status, output) = match result {
            Ok((text, false)) => (acp::ToolCallStatus::Completed, text),
            Ok((text, true)) => (acp::ToolCallStatus::Failed, format!("error: {text}")),
            Err(message) => (acp::ToolCallStatus::Failed, message),
        };
        let preview: String = output.chars().take(2_000).collect();
        self.finish_tool(session_id, &summary, status, vec![text_content(&preview)])
            .await?;
        Ok(ToolOutcome::Output(output))
    }

    /// Connect the session's MCP servers, if the editor listed any and they are enabled.
    async fn attach_mcp(
        &self,
        session_id: &acp::SessionId,
        servers: &[acp::McpServer],
        cwd: &Path,
    ) {
        if !self.config.mcp_servers || servers.is_empty() {
            return;
        }
        let (hub, warnings) = McpHub::connect(servers, cwd).await;
        for warning in &warnings {
            tracing::warn!("{warning}");
            eprintln!("vktr acp: {warning}");
        }
        self.with_session(session_id, |s| {
            s.mcp = (!hub.is_empty()).then(|| Rc::new(hub));
            s.mcp_warnings = warnings;
        })
        .ok();
    }

    /// Report a tool call that never ran (bad arguments, unknown tool) and record it.
    async fn report_call(
        &self,
        session_id: &acp::SessionId,
        call: acp::ToolCall,
    ) -> Result<(), TurnError> {
        self.record(
            session_id,
            Entry::Tool {
                call: Box::new(
                    acp::ToolCall::new(call.tool_call_id.clone(), call.title.clone())
                        .kind(call.kind)
                        .status(call.status),
                ),
            },
        );
        self.notify(session_id, acp::SessionUpdate::ToolCall(call))
            .await
            .map_err(TurnError::Acp)
    }

    /// Send a tool call's final status and record it (without its content) in the transcript.
    async fn finish_tool(
        &self,
        session_id: &acp::SessionId,
        summary: &acp::ToolCall,
        status: acp::ToolCallStatus,
        content: Vec<acp::ToolCallContent>,
    ) -> Result<(), TurnError> {
        self.record(
            session_id,
            Entry::Tool {
                call: Box::new(summary.clone().status(status)),
            },
        );
        let mut fields = acp::ToolCallUpdateFields::new().status(status);
        if !content.is_empty() {
            fields = fields.content(content);
        }
        self.update_tool(session_id, &summary.tool_call_id.0, fields)
            .await
    }

    async fn ask_permission(
        &self,
        session_id: &acp::SessionId,
        call_id: &str,
        ask: Ask,
        preview: Option<acp::ToolCallContent>,
        token: &CancellationToken,
    ) -> Result<Decision, TurnError> {
        let Ask { key, title, kind } = ask;
        if self
            .with_session(session_id, |s| s.always_allow.contains(&key))
            .unwrap_or(false)
        {
            return Ok(Decision::Allow);
        }
        let connection = self.connection().map_err(TurnError::Acp)?;
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(title)
            .kind(kind)
            .status(acp::ToolCallStatus::Pending);
        if let Some(preview) = preview {
            fields = fields.content(vec![preview]);
        }
        let ask = connection.request_permission(acp::RequestPermissionRequest::new(
            session_id.clone(),
            acp::ToolCallUpdate::new(call_id.to_owned(), fields),
            vec![
                acp::PermissionOption::new(
                    "allow_once",
                    "Allow",
                    acp::PermissionOptionKind::AllowOnce,
                ),
                acp::PermissionOption::new(
                    "allow_always",
                    "Always allow in this session",
                    acp::PermissionOptionKind::AllowAlways,
                ),
                acp::PermissionOption::new(
                    "reject_once",
                    "Reject",
                    acp::PermissionOptionKind::RejectOnce,
                ),
            ],
        ));
        let response = tokio::select! {
            biased;
            () = token.cancelled() => return Ok(Decision::Cancelled),
            response = ask => response,
        };
        Ok(match response {
            Ok(response) => match response.outcome {
                acp::RequestPermissionOutcome::Cancelled => Decision::Cancelled,
                acp::RequestPermissionOutcome::Selected(selected) => match &*selected.option_id.0 {
                    "allow_once" => Decision::Allow,
                    "allow_always" => {
                        self.with_session(session_id, |s| s.always_allow.insert(key))
                            .ok();
                        Decision::Allow
                    }
                    _ => Decision::Reject("rejected by the user".to_owned()),
                },
                // Future outcome kinds: never run a call the user did not clearly allow.
                _ => Decision::Reject("the editor gave no clear answer".to_owned()),
            },
            // An editor that cannot ask must not get an unapproved write or command.
            Err(error) => Decision::Reject(format!(
                "the editor could not ask the user for permission: {}",
                error.message
            )),
        })
    }

    async fn read_file(
        &self,
        session_id: &acp::SessionId,
        path: &Path,
        line: Option<u32>,
        limit: Option<u32>,
    ) -> Result<String, String> {
        let connection = self.connection().map_err(|e| e.message)?;
        connection
            .read_text_file(
                acp::ReadTextFileRequest::new(session_id.clone(), path.to_path_buf())
                    .line(line)
                    .limit(limit),
            )
            .await
            .map(|r| r.content)
            .map_err(|e| e.message)
    }

    async fn write_file(
        &self,
        session_id: &acp::SessionId,
        path: &Path,
        content: String,
    ) -> Result<(), String> {
        let connection = self.connection().map_err(|e| e.message)?;
        connection
            .write_text_file(acp::WriteTextFileRequest::new(
                session_id.clone(),
                path.to_path_buf(),
                content,
            ))
            .await
            .map(|_| ())
            .map_err(|e| e.message)
    }

    /// Run a command in an editor terminal, showing the terminal in the tool call while it runs.
    async fn run_command(
        &self,
        session_id: &acp::SessionId,
        call_id: &str,
        command: &str,
        cwd: &Path,
        token: &CancellationToken,
    ) -> Result<CommandOutcome, TurnError> {
        let connection = self.connection().map_err(TurnError::Acp)?;
        let created = connection
            .create_terminal(
                acp::CreateTerminalRequest::new(session_id.clone(), "sh")
                    .args(vec!["-c".to_owned(), command.to_owned()])
                    .cwd(Some(cwd.to_path_buf()))
                    .output_byte_limit(Some(tools::COMMAND_OUTPUT_LIMIT)),
            )
            .await;
        let terminal_id = match created {
            Ok(created) => created.terminal_id,
            Err(error) => {
                return Ok(CommandOutcome::Failed(format!(
                    "error: the editor could not start the command: {}",
                    error.message
                )));
            }
        };
        self.update_tool(
            session_id,
            call_id,
            acp::ToolCallUpdateFields::new().content(vec![acp::ToolCallContent::Terminal(
                acp::Terminal::new(terminal_id.clone()),
            )]),
        )
        .await?;

        let exit = tokio::select! {
            biased;
            () = token.cancelled() => None,
            exit = connection.wait_for_terminal_exit(acp::WaitForTerminalExitRequest::new(
                session_id.clone(),
                terminal_id.clone(),
            )) => Some(exit),
        };
        let outcome = match exit {
            None => {
                connection
                    .kill_terminal(acp::KillTerminalRequest::new(
                        session_id.clone(),
                        terminal_id.clone(),
                    ))
                    .await
                    .ok();
                CommandOutcome::Cancelled
            }
            Some(Err(error)) => CommandOutcome::Failed(format!(
                "error: waiting for the command failed: {}",
                error.message
            )),
            Some(Ok(exit)) => match connection
                .terminal_output(acp::TerminalOutputRequest::new(
                    session_id.clone(),
                    terminal_id.clone(),
                ))
                .await
            {
                Ok(output) => CommandOutcome::Finished {
                    terminal_id: terminal_id.clone(),
                    output: tools::command_output(
                        &exit.exit_status,
                        &output.output,
                        output.truncated,
                    ),
                },
                Err(error) => CommandOutcome::Failed(format!(
                    "error: could not read the command's output: {}",
                    error.message
                )),
            },
        };
        // The editor keeps showing a released terminal that is embedded in a tool call.
        connection
            .release_terminal(acp::ReleaseTerminalRequest::new(
                session_id.clone(),
                terminal_id,
            ))
            .await
            .ok();
        Ok(outcome)
    }

    /// Bring a stored session back to life, for `session/load` and `session/resume`.
    fn restore(&self, session_id: &acp::SessionId, cwd: PathBuf) -> Result<Vec<Entry>, acp::Error> {
        let stored = self
            .config
            .store
            .as_ref()
            .and_then(|store| store.load(&session_id.0))
            .ok_or_else(|| {
                acp::Error::resource_not_found(Some(format!("vktr acp session {session_id}")))
            })?;
        let entries = stored.entries.clone();
        let mut state = SessionState::new(stored);
        // The editor's workspace wins: it may have moved since the session was saved.
        state.stored.cwd = cwd;
        let mut sessions = self.sessions.borrow_mut();
        if let Some(previous) = sessions.insert(session_id.clone(), state)
            && let Some(pending) = previous.pending
        {
            pending.cancel();
        }
        Ok(entries)
    }
}

enum RunOutcome {
    Finished {
        stop_reason: acp::StopReason,
        calls: Vec<FunctionCall>,
    },
    Cancelled,
}

enum ToolOutcome {
    Output(String),
    Cancelled,
}

/// What a permission prompt is about, and what "always allow" remembers it by.
struct Ask {
    key: String,
    title: String,
    kind: acp::ToolKind,
}

enum Decision {
    Allow,
    Reject(String),
    Cancelled,
}

enum CommandOutcome {
    Finished {
        terminal_id: acp::TerminalId,
        output: String,
    },
    Failed(String),
    Cancelled,
}

fn failed(message: String) -> (acp::ToolCallStatus, Vec<acp::ToolCallContent>, String) {
    (
        acp::ToolCallStatus::Failed,
        vec![text_content(&message)],
        message,
    )
}

fn text_content(text: &str) -> acp::ToolCallContent {
    acp::ToolCallContent::Content(acp::Content::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    )))
}

fn function_call_output(call_id: &str, output: &str) -> Value {
    json!({"type": "function_call_output", "call_id": call_id, "output": output})
}

/// Keep the first sighting of each call: the same call is listed by `output_item.done` and
/// again in the completed response.
fn add_call(calls: &mut Vec<FunctionCall>, call: FunctionCall) {
    if !calls.iter().any(|c| c.call_id == call.call_id) {
        calls.push(call);
    }
}

/// The prompt as transcript text: its text parts, with a marker for what text cannot show.
fn prompt_text(prompt: &[acp::ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            acp::ContentBlock::Text(text) => Some(text.text.clone()),
            acp::ContentBlock::Image(_) => Some("[image]".to_owned()),
            acp::ContentBlock::ResourceLink(link) => Some(format!("[{}]({})", link.name, link.uri)),
            acp::ContentBlock::Resource(_) => Some("[attached file]".to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A turn can fail because Viktor failed, or because the client connection did.
enum TurnError {
    Viktor(ViktorError),
    Acp(acp::Error),
}

/// Report a failed Viktor run as a JSON-RPC error carrying Viktor's own message and request id.
/// The editor shows it as an error, never as assistant text.
fn to_acp_error(error: ViktorError) -> acp::Error {
    acp::Error::new(acp::ErrorCode::InternalError.into(), error.message.clone()).data(json!({
        "code": error.code.as_str(),
        "detailCode": error.detail_code,
        "requestId": error.request_id,
    }))
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for ViktorAcpAgent {
    async fn initialize(
        &self,
        arguments: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, acp::Error> {
        *self.client_capabilities.borrow_mut() = arguments.client_capabilities;
        let persistent = self.config.store.is_some();
        let mut session_capabilities = acp::SessionCapabilities::new();
        if persistent {
            session_capabilities = session_capabilities
                .list(acp::SessionListCapabilities::new())
                .resume(acp::SessionResumeCapabilities::new());
        }
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    // The Viktor thread lives server-side; the store keeps the mapping to it
                    // and the transcript to replay.
                    .load_session(persistent)
                    .session_capabilities(session_capabilities)
                    .mcp_capabilities(acp::McpCapabilities::new().http(self.config.mcp_servers))
                    .prompt_capabilities(
                        acp::PromptCapabilities::new()
                            .image(true)
                            .embedded_context(true),
                    ),
            )
            .auth_methods(vec![acp::AuthMethod::Agent(
                acp::AuthMethodAgent::new(AUTH_METHOD_ID, "Viktor API key")
                    .description(AUTH_METHOD_DESCRIPTION.to_owned()),
            )])
            .agent_info(
                acp::Implementation::new("vktr", self.config.version.clone()).title("Viktor"),
            ))
    }

    async fn authenticate(
        &self,
        _arguments: acp::AuthenticateRequest,
    ) -> Result<acp::AuthenticateResponse, acp::Error> {
        // Listing models is the cheapest request that proves the key and its scope.
        self.client.list_models().await.map_err(to_acp_error)?;
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        arguments: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, acp::Error> {
        let session_id = acp::SessionId::from(uuid::Uuid::new_v4().to_string());
        self.sessions.borrow_mut().insert(
            session_id.clone(),
            SessionState::new(StoredSession::new(
                session_id.0.to_string(),
                arguments.cwd.clone(),
            )),
        );
        self.attach_mcp(&session_id, &arguments.mcp_servers, &arguments.cwd)
            .await;
        Ok(acp::NewSessionResponse::new(session_id))
    }

    async fn load_session(
        &self,
        arguments: acp::LoadSessionRequest,
    ) -> Result<acp::LoadSessionResponse, acp::Error> {
        let session_id = arguments.session_id;
        let entries = self.restore(&session_id, arguments.cwd.clone())?;
        self.attach_mcp(&session_id, &arguments.mcp_servers, &arguments.cwd)
            .await;
        // Replay the conversation before answering, as session/load requires.
        for entry in entries {
            let update = match entry {
                Entry::User { text } => acp::SessionUpdate::UserMessageChunk(
                    acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text))),
                ),
                Entry::Agent { text } => acp::SessionUpdate::AgentMessageChunk(
                    acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new(text))),
                ),
                Entry::Tool { call } => acp::SessionUpdate::ToolCall(*call),
            };
            self.notify(&session_id, update).await?;
        }
        Ok(acp::LoadSessionResponse::new())
    }

    async fn resume_session(
        &self,
        arguments: acp::ResumeSessionRequest,
    ) -> Result<acp::ResumeSessionResponse, acp::Error> {
        self.restore(&arguments.session_id, arguments.cwd.clone())?;
        self.attach_mcp(
            &arguments.session_id,
            &arguments.mcp_servers,
            &arguments.cwd,
        )
        .await;
        Ok(acp::ResumeSessionResponse::new())
    }

    async fn list_sessions(
        &self,
        arguments: acp::ListSessionsRequest,
    ) -> Result<acp::ListSessionsResponse, acp::Error> {
        let sessions = self
            .config
            .store
            .as_ref()
            .map(|store| store.list(arguments.cwd.as_deref()))
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                acp::SessionInfo::new(s.session_id, s.cwd)
                    .title(s.title)
                    .updated_at(s.updated_at)
            })
            .collect();
        Ok(acp::ListSessionsResponse::new(sessions))
    }

    async fn prompt(
        &self,
        arguments: acp::PromptRequest,
    ) -> Result<acp::PromptResponse, acp::Error> {
        let session_id = arguments.session_id.clone();
        let (token, turn_lock) = self.begin_turn(&session_id)?;
        // A superseded turn has been cancelled; wait for it to wind down before touching the
        // thread, and give up at once if a newer prompt superseded this one meanwhile.
        let _turn = turn_lock.lock().await;
        if token.is_cancelled() {
            self.end_turn(&session_id, &token);
            return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
        }
        let result = self.run_turn(&session_id, &arguments.prompt, &token).await;
        self.end_turn(&session_id, &token);
        self.persist(&session_id);
        match result {
            Ok(stop_reason) => Ok(acp::PromptResponse::new(stop_reason)),
            // A cancelled turn reports `cancelled`, not the error the aborted request raised.
            Err(_) if token.is_cancelled() => {
                Ok(acp::PromptResponse::new(acp::StopReason::Cancelled))
            }
            Err(TurnError::Viktor(error)) => Err(to_acp_error(error)),
            Err(TurnError::Acp(error)) => Err(error),
        }
    }

    async fn cancel(&self, arguments: acp::CancelNotification) -> Result<(), acp::Error> {
        if let Some(session) = self.sessions.borrow().get(&arguments.session_id)
            && let Some(pending) = &session.pending
        {
            pending.cancel();
        }
        Ok(())
    }
}
