//! End-to-end protocol tests for `vktr acp`.
//!
//! A real ACP client drives the agent over in-memory pipes, exactly as an editor would over
//! stdio, while a mock Viktor compat API serves canned Responses streams. This mirrors the
//! TypeScript `viktor-acp` suite, so the two agents can be compared case by case.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use agent_client_protocol::{self as acp, Agent as _};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------------------------
// Mock Viktor compat API
// ---------------------------------------------------------------------------------------------

/// What the mock recorded about one inbound request.
#[derive(Clone, Debug)]
struct RecordedRequest {
    target: String,
    authorization: Option<String>,
    body: Value,
}

/// A throwaway Viktor server that replays scripted responses, one per request.
struct MockViktor {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockViktor {
    /// `bodies` are served in order: each entry is the raw HTTP response for the next request.
    async fn start(bodies: Vec<String>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        tokio::task::spawn(async move {
            let mut remaining = bodies.into_iter();
            while let Ok((stream, _)) = listener.accept().await {
                let Some(response) = remaining.next() else {
                    break;
                };
                let recorded = Arc::clone(&recorded);
                tokio::task::spawn(async move {
                    serve_one(stream, response, recorded).await;
                });
            }
        });
        Self { addr, requests }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock").clone()
    }
}

/// Read one HTTP/1.1 request, record it, write the scripted response.
async fn serve_one(
    stream: tokio::net::TcpStream,
    response: String,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = stream;
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    // Read headers, then exactly Content-Length bytes of body.
    let (head_end, content_length) = loop {
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            return;
        }
        raw.extend_from_slice(buffer.get(..read).unwrap_or_default());
        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while raw.len() < head_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(buffer.get(..read).unwrap_or_default());
    }

    let text = String::from_utf8_lossy(&raw).to_string();
    let head = text.get(..head_end).unwrap_or_default();
    let body = text.get(head_end..).unwrap_or_default();
    let target = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_owned();
    let authorization = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_owned())
    });
    recorded.lock().expect("lock").push(RecordedRequest {
        target,
        authorization,
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    });

    stream.write_all(response.as_bytes()).await.ok();
    stream.flush().await.ok();
    // A response with no content-length is an open-ended stream: hold the socket open so the
    // client sees a run still in progress (what cancellation needs to act on).
    if response.to_lowercase().contains("content-length") {
        stream.shutdown().await.ok();
    } else {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}

/// An HTTP response carrying an SSE body built from `event: data` pairs.
fn sse_response(events: &[Value]) -> String {
    let mut body = String::new();
    // A keep-alive comment: Viktor sends these while it works and they must never be content.
    body.push_str(": keep-alive\n\n");
    for event in events {
        let _ = write!(body, "data: {event}\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-request-id: req-test-1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The `responses-stream-text` fixture: Viktor answers, then completes the thread.
fn fixture_text() -> String {
    sse_response(&[
        json!({"type": "response.output_text.delta", "delta": "Hello from "}),
        json!({"type": "response.output_text.delta", "delta": "Viktor."}),
        json!({"type": "response.completed", "response": {"id": "zwKTTPTKCc9TVsSMgJuGh"}}),
    ])
}

/// The `responses-stream-failed` fixture: Viktor leaks the failure as text, then fails the run.
fn fixture_failed() -> String {
    sse_response(&[
        json!({"type": "response.output_text.delta", "delta": "[Stream error: empty response twice]"}),
        json!({
            "type": "response.failed",
            "response": {"error": {"message": "Viktor returned an empty response twice", "code": "server_error"}},
        }),
    ])
}

// ---------------------------------------------------------------------------------------------
// A real ACP client
// ---------------------------------------------------------------------------------------------

/// A command the fake terminal was asked to run: program, arguments, working directory.
type StartedCommand = (String, Vec<String>, Option<std::path::PathBuf>);

/// A scripted editor: an in-memory file system, a fake terminal and canned permission answers.
#[derive(Default)]
struct Editor {
    files: RefCell<std::collections::HashMap<std::path::PathBuf, String>>,
    /// Option ids to answer permission requests with, in order; "cancelled" cancels.
    answers: RefCell<std::collections::VecDeque<&'static str>>,
    permission_requests: RefCell<Vec<acp::RequestPermissionRequest>>,
    /// Commands started in the fake terminal, with their cwd.
    commands: RefCell<Vec<StartedCommand>>,
}

#[derive(Default)]
struct TestClient {
    updates: Rc<RefCell<Vec<acp::SessionNotification>>>,
    editor: Rc<Editor>,
}

#[async_trait::async_trait(?Send)]
impl acp::Client for TestClient {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> Result<acp::RequestPermissionResponse, acp::Error> {
        self.editor.permission_requests.borrow_mut().push(args);
        let answer = self.editor.answers.borrow_mut().pop_front();
        match answer {
            None => Err(acp::Error::method_not_found()),
            Some("cancelled") => Ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled,
            )),
            Some(option) => Ok(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    option,
                )),
            )),
        }
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> Result<(), acp::Error> {
        self.updates.borrow_mut().push(args);
        Ok(())
    }

    async fn read_text_file(
        &self,
        args: acp::ReadTextFileRequest,
    ) -> Result<acp::ReadTextFileResponse, acp::Error> {
        self.editor
            .files
            .borrow()
            .get(&args.path)
            .map(|text| acp::ReadTextFileResponse::new(text.clone()))
            .ok_or_else(|| acp::Error::resource_not_found(Some(args.path.display().to_string())))
    }

    async fn write_text_file(
        &self,
        args: acp::WriteTextFileRequest,
    ) -> Result<acp::WriteTextFileResponse, acp::Error> {
        self.editor
            .files
            .borrow_mut()
            .insert(args.path, args.content);
        Ok(acp::WriteTextFileResponse::new())
    }

    async fn create_terminal(
        &self,
        args: acp::CreateTerminalRequest,
    ) -> Result<acp::CreateTerminalResponse, acp::Error> {
        self.editor
            .commands
            .borrow_mut()
            .push((args.command, args.args, args.cwd));
        Ok(acp::CreateTerminalResponse::new("term-1"))
    }

    async fn wait_for_terminal_exit(
        &self,
        _args: acp::WaitForTerminalExitRequest,
    ) -> Result<acp::WaitForTerminalExitResponse, acp::Error> {
        Ok(acp::WaitForTerminalExitResponse::new(
            acp::TerminalExitStatus::new().exit_code(Some(0)),
        ))
    }

    async fn terminal_output(
        &self,
        _args: acp::TerminalOutputRequest,
    ) -> Result<acp::TerminalOutputResponse, acp::Error> {
        Ok(acp::TerminalOutputResponse::new(
            "hello from the editor terminal\n",
            false,
        ))
    }

    async fn release_terminal(
        &self,
        _args: acp::ReleaseTerminalRequest,
    ) -> Result<acp::ReleaseTerminalResponse, acp::Error> {
        Ok(acp::ReleaseTerminalResponse::new())
    }
}

/// The agent and a connected client, wired through in-memory pipes.
struct Harness {
    connection: acp::ClientSideConnection,
    updates: Rc<RefCell<Vec<acp::SessionNotification>>>,
    editor: Rc<Editor>,
}

impl Harness {
    fn start(base_url: &str) -> Self {
        Self::start_with(base_url, vktr_acp::AgentConfig::default())
    }

    fn start_with(base_url: &str, config: vktr_acp::AgentConfig) -> Self {
        let client =
            Rc::new(vktr_acp::ViktorClient::new("zt_test_sk_fixture", base_url).expect("client"));
        let (client_to_agent_rx, client_to_agent_tx) = piper::pipe(64 * 1024);
        let (agent_to_client_rx, agent_to_client_tx) = piper::pipe(64 * 1024);

        tokio::task::spawn_local(vktr_acp::serve(
            client,
            config,
            agent_to_client_tx,
            client_to_agent_rx,
        ));

        let updates = Rc::new(RefCell::new(Vec::new()));
        let editor = Rc::new(Editor::default());
        let test_client = TestClient {
            updates: Rc::clone(&updates),
            editor: Rc::clone(&editor),
        };
        let (connection, io) = acp::ClientSideConnection::new(
            test_client,
            client_to_agent_tx,
            agent_to_client_rx,
            |fut| {
                tokio::task::spawn_local(fut);
            },
        );
        tokio::task::spawn_local(io);
        Self {
            connection,
            updates,
            editor,
        }
    }

    /// Initialize as an editor that offers file reads and writes and terminals.
    async fn initialize_as_editor(&self) -> acp::InitializeResponse {
        self.connection
            .initialize(
                acp::InitializeRequest::new(acp::ProtocolVersion::V1).client_capabilities(
                    acp::ClientCapabilities::new()
                        .fs(acp::FileSystemCapabilities::new()
                            .read_text_file(true)
                            .write_text_file(true))
                        .terminal(true),
                ),
            )
            .await
            .expect("initialize")
    }

    fn tool_updates(&self) -> Vec<acp::SessionUpdate> {
        self.updates
            .borrow()
            .iter()
            .filter(|n| {
                matches!(
                    n.update,
                    acp::SessionUpdate::ToolCall(_) | acp::SessionUpdate::ToolCallUpdate(_)
                )
            })
            .map(|n| n.update.clone())
            .collect()
    }

    async fn initialize(&self) -> acp::InitializeResponse {
        self.connection
            .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
            .await
            .expect("initialize")
    }

    async fn new_session(&self) -> acp::SessionId {
        self.connection
            .new_session(acp::NewSessionRequest::new("/tmp/project"))
            .await
            .expect("session/new")
            .session_id
    }

    /// Send a prompt and return the streamed text with the turn's stop reason.
    async fn prompt(
        &self,
        session_id: &acp::SessionId,
        blocks: Vec<acp::ContentBlock>,
    ) -> Result<(String, acp::StopReason), acp::Error> {
        let before = self.updates.borrow().len();
        let response = self
            .connection
            .prompt(acp::PromptRequest::new(session_id.clone(), blocks))
            .await?;
        let text = self
            .updates
            .borrow()
            .get(before..)
            .unwrap_or_default()
            .iter()
            .filter_map(|notification| match &notification.update {
                acp::SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    acp::ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        Ok((text, response.stop_reason))
    }
}

fn text(message: &str) -> Vec<acp::ContentBlock> {
    vec![acp::ContentBlock::Text(acp::TextContent::new(message))]
}

/// Run one test body on a current-thread runtime with a `LocalSet`, as the CLI does.
fn run<F: std::future::Future<Output = ()>>(body: impl FnOnce() -> F + 'static) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, body());
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[test]
fn completes_the_acp_v1_handshake_and_advertises_the_api_key_auth_method() {
    run(|| async {
        let viktor = MockViktor::start(vec![]).await;
        let harness = Harness::start(&viktor.base_url());
        let init = harness.initialize().await;

        assert_eq!(init.protocol_version, acp::ProtocolVersion::V1);
        assert_eq!(
            init.auth_methods.first().map(acp::AuthMethod::id),
            Some(&acp::AuthMethodId::from(vktr_acp::AUTH_METHOD_ID))
        );
        assert!(init.agent_capabilities.prompt_capabilities.image);
        assert!(init.agent_capabilities.prompt_capabilities.embedded_context);
        // Without a session store (the binary always has one) there is nothing to reload.
        assert!(!init.agent_capabilities.load_session);
    });
}

#[test]
fn streams_viktors_reply_as_agent_message_chunks_and_ends_the_turn() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let (reply, stop_reason) = harness
            .prompt(&session, text("Say hello"))
            .await
            .expect("prompt");
        assert_eq!(reply, "Hello from Viktor.");
        assert_eq!(stop_reason, acp::StopReason::EndTurn);

        let requests = viktor.requests();
        let request = requests.first().expect("one request");
        assert_eq!(request.target, "/api/compat/v1/responses");
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer zt_test_sk_fixture")
        );
        assert_eq!(
            request.body.get("model").and_then(Value::as_str),
            Some("viktor")
        );
        assert_eq!(
            request.body.get("stream").and_then(Value::as_bool),
            Some(true)
        );
        // A first prompt opens a fresh thread.
        assert!(request.body.get("previous_response_id").is_none());
        assert_eq!(
            request.body.pointer("/input/0/content/0"),
            Some(&json!({"type": "input_text", "text": "Say hello"}))
        );
    });
}

#[test]
fn maps_a_session_to_one_viktor_thread_and_continues_it_with_previous_response_id() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text(), fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        harness
            .prompt(&session, text("first"))
            .await
            .expect("first");
        harness
            .prompt(&session, text("second"))
            .await
            .expect("second");

        let requests = viktor.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests
                .get(1)
                .and_then(|r| r.body.get("previous_response_id")),
            Some(&json!("zwKTTPTKCc9TVsSMgJuGh"))
        );
    });
}

#[test]
fn a_new_session_starts_a_new_thread() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text(), fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;

        let first = harness.new_session().await;
        harness.prompt(&first, text("one")).await.expect("one");
        let second = harness.new_session().await;
        assert_ne!(first, second);
        harness.prompt(&second, text("two")).await.expect("two");

        let requests = viktor.requests();
        assert!(
            requests
                .get(1)
                .is_some_and(|r| r.body.get("previous_response_id").is_none()),
            "a second session must not inherit the first session's Viktor thread"
        );
    });
}

#[test]
fn reports_a_failed_viktor_run_as_a_request_error_without_leaking_the_stream_error_text() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_failed()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let error = harness
            .prompt(&session, text("x"))
            .await
            .expect_err("run failed");
        assert!(
            error.message.contains("empty response twice"),
            "the editor must see Viktor's own message, got: {}",
            error.message
        );
        let data = error.data.expect("error data");
        assert_eq!(data.get("code").and_then(Value::as_str), Some("run_failed"));
        assert_eq!(
            data.get("requestId").and_then(Value::as_str),
            Some("req-test-1")
        );

        // The "[Stream error: …]" delta must never reach the editor as assistant text.
        let leaked = harness.updates.borrow().iter().any(|notification| {
            matches!(&notification.update, acp::SessionUpdate::AgentMessageChunk(chunk)
                if matches!(&chunk.content, acp::ContentBlock::Text(t) if t.text.contains("Stream error")))
        });
        assert!(!leaked, "the stream-error text leaked into the transcript");
    });
}

#[test]
fn an_http_error_from_viktor_is_reported_with_the_servers_own_message() {
    run(|| async {
        let body =
            json!({"error": {"message": "Invalid key", "code": "invalid_api_key"}}).to_string();
        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let viktor = MockViktor::start(vec![response]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let error = harness
            .prompt(&session, text("x"))
            .await
            .expect_err("auth failure");
        assert!(
            error.message.contains("Invalid key"),
            "got: {}",
            error.message
        );
        let data = error.data.expect("error data");
        assert_eq!(data.get("code").and_then(Value::as_str), Some("auth"));
    });
}

#[test]
fn an_image_block_reaches_viktor_as_an_input_image_data_url() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let blocks = vec![
            acp::ContentBlock::Text(acp::TextContent::new("what is this?")),
            acp::ContentBlock::Image(acp::ImageContent::new("AAAA", "image/png")),
        ];
        harness.prompt(&session, blocks).await.expect("prompt");

        let requests = viktor.requests();
        let content = requests
            .first()
            .and_then(|r| r.body.pointer("/input/0/content"))
            .cloned()
            .expect("content parts");
        assert_eq!(
            content,
            json!([
                {"type": "input_text", "text": "what is this?"},
                {"type": "input_image", "image_url": "data:image/png;base64,AAAA"},
            ])
        );
    });
}

#[test]
fn an_embedded_text_resource_reaches_viktor_as_named_file_text() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let blocks = vec![acp::ContentBlock::Resource(acp::EmbeddedResource::new(
            acp::EmbeddedResourceResource::TextResourceContents(acp::TextResourceContents::new(
                "const a = 1;",
                "file:///a.ts",
            )),
        ))];
        harness.prompt(&session, blocks).await.expect("prompt");

        let requests = viktor.requests();
        assert_eq!(
            requests
                .first()
                .and_then(|r| r.body.pointer("/input/0/content/0")),
            Some(&json!({"type": "input_text", "text": "File file:///a.ts:\n\nconst a = 1;"}))
        );
    });
}

#[test]
fn a_v1_root_endpoint_is_used_as_given_rather_than_having_the_compat_path_appended() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        // vktr's smoke tests and local proxies serve an OpenAI-style root that is not under
        // /api/compat; VIKTOR_BASE_URL is spelled that way for the rest of vktr too.
        let harness = Harness::start(&format!("{}/v1", viktor.base_url()));
        harness.initialize().await;
        let session = harness.new_session().await;
        harness.prompt(&session, text("hi")).await.expect("prompt");

        let requests = viktor.requests();
        assert_eq!(
            requests.first().map(|r| r.target.as_str()),
            Some("/v1/responses")
        );
    });
}

#[test]
fn authenticate_verifies_the_key_against_the_models_endpoint() {
    run(|| async {
        let body = json!({"data": [{"id": "viktor"}]}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let viktor = MockViktor::start(vec![response]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;

        harness
            .connection
            .authenticate(acp::AuthenticateRequest::new(vktr_acp::AUTH_METHOD_ID))
            .await
            .expect("authenticate");

        let requests = viktor.requests();
        assert_eq!(
            requests.first().map(|r| r.target.as_str()),
            Some("/api/compat/v1/models")
        );
    });
}

#[test]
fn a_cancelled_turn_stops_with_the_cancelled_stop_reason() {
    run(|| async {
        // A response that never completes: headers only, body withheld until the socket closes.
        let never_finishes = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-request-id: req-test-1\r\n\r\n: keep-alive\n\n".to_owned();
        let viktor = MockViktor::start(vec![never_finishes]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let prompt = harness.connection.prompt(acp::PromptRequest::new(
            session.clone(),
            text("take your time"),
        ));
        let cancel = async {
            // Let the turn reach the server before cancelling it.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            harness
                .connection
                .cancel(acp::CancelNotification::new(session.clone()))
                .await
                .expect("cancel");
        };
        let (response, ()) = tokio::join!(prompt, cancel);
        assert_eq!(
            response
                .expect("prompt returns a result, not an error")
                .stop_reason,
            acp::StopReason::Cancelled
        );
    });
}

// ---------------------------------------------------------------------------------------------
// Editor tools: Viktor calls back into the editor, and the result resumes the same thread
// ---------------------------------------------------------------------------------------------

/// Viktor asks for one caller tool, the way the live compat API does (2026-09-23 capture).
fn fixture_call(call_id: &str, name: &str, arguments: &Value, response_id: &str) -> String {
    let item = json!({
        "type": "function_call",
        "id": call_id,
        "call_id": call_id,
        "name": name,
        "arguments": arguments.to_string(),
        "status": "completed",
    });
    sse_response(&[
        json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
        json!({"type": "response.completed", "response": {"id": response_id, "output": [item]}}),
    ])
}

fn fixture_answer(text: &str, response_id: &str) -> String {
    sse_response(&[
        json!({"type": "response.output_text.delta", "delta": text}),
        json!({"type": "response.completed", "response": {"id": response_id}}),
    ])
}

fn tool_names(request: &RecordedRequest) -> Vec<String> {
    request
        .body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn tool_output(request: &RecordedRequest) -> Option<String> {
    request
        .body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
        .and_then(|item| item.get("output").and_then(Value::as_str))
        .map(str::to_owned)
}

fn statuses(updates: &[acp::SessionUpdate]) -> Vec<acp::ToolCallStatus> {
    updates
        .iter()
        .filter_map(|u| match u {
            acp::SessionUpdate::ToolCall(call) => Some(call.status),
            acp::SessionUpdate::ToolCallUpdate(update) => update.fields.status,
            _ => None,
        })
        .collect()
}

#[test]
fn a_chat_only_client_gets_no_tools_and_an_editor_gets_exactly_what_it_offers() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text(), fixture_text()]).await;
        let chat_only = Harness::start(&viktor.base_url());
        chat_only.initialize().await;
        let session = chat_only.new_session().await;
        chat_only
            .prompt(&session, text("hi"))
            .await
            .expect("prompt");

        let editor = Harness::start(&viktor.base_url());
        editor.initialize_as_editor().await;
        let session = editor.new_session().await;
        editor.prompt(&session, text("hi")).await.expect("prompt");

        let requests = viktor.requests();
        assert!(
            requests
                .first()
                .is_some_and(|r| r.body.get("tools").is_none()),
            "a client without fs or terminal capabilities must see the TypeScript agent's wire"
        );
        assert_eq!(
            requests.get(1).map(tool_names),
            Some(vec![
                vktr_acp::READ_FILE.to_owned(),
                vktr_acp::WRITE_FILE.to_owned(),
                vktr_acp::RUN_COMMAND.to_owned(),
            ])
        );
    });
}

#[test]
fn the_tools_can_be_turned_off_even_for_an_editor_that_offers_them() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        let config = vktr_acp::AgentConfig {
            editor_tools: false,
            ..vktr_acp::AgentConfig::default()
        };
        let harness = Harness::start_with(&viktor.base_url(), config);
        harness.initialize_as_editor().await;
        let session = harness.new_session().await;
        harness.prompt(&session, text("hi")).await.expect("prompt");
        assert!(
            viktor
                .requests()
                .first()
                .is_some_and(|r| r.body.get("tools").is_none())
        );
    });
}

#[test]
fn a_read_runs_through_the_editor_and_its_text_resumes_the_same_thread() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "call_1",
                vktr_acp::READ_FILE,
                &json!({"path": "README.md"}),
                "thread-A",
            ),
            fixture_answer("It starts with a heading.", "thread-A"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness.editor.files.borrow_mut().insert(
            "/tmp/project/README.md".into(),
            "# Pelican Orchard\n".to_owned(),
        );
        let session = harness.new_session().await;

        let (reply, stop) = harness
            .prompt(&session, text("what is in README.md?"))
            .await
            .expect("prompt");
        assert_eq!(reply, "It starts with a heading.");
        assert_eq!(stop, acp::StopReason::EndTurn);
        assert!(
            harness.editor.permission_requests.borrow().is_empty(),
            "reads do not ask the user"
        );

        let requests = viktor.requests();
        let second = requests.get(1).expect("the tool result went back");
        assert_eq!(
            second.body.get("previous_response_id"),
            Some(&json!("thread-A"))
        );
        assert_eq!(
            second.body.pointer("/input/0/call_id"),
            Some(&json!("call_1"))
        );
        assert_eq!(tool_output(second).as_deref(), Some("# Pelican Orchard\n"));
        assert!(
            second.body.get("tools").is_some(),
            "the tools stay offered while the run continues"
        );

        let updates = harness.tool_updates();
        assert_eq!(
            statuses(&updates),
            [
                acp::ToolCallStatus::Pending,
                acp::ToolCallStatus::InProgress,
                acp::ToolCallStatus::Completed,
            ]
        );
        assert!(
            matches!(updates.first(), Some(acp::SessionUpdate::ToolCall(call))
            if call.kind == acp::ToolKind::Read
                && call.title == "Read /tmp/project/README.md"
                && call.tool_call_id.0.as_ref() == "call_1")
        );
    });
}

#[test]
fn a_write_asks_the_user_with_its_diff_and_only_then_touches_the_file() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "call_w",
                vktr_acp::WRITE_FILE,
                &json!({"path": "notes.txt", "content": "new\n"}),
                "thread-W",
            ),
            fixture_answer("Done.", "thread-W"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness
            .editor
            .files
            .borrow_mut()
            .insert("/tmp/project/notes.txt".into(), "old\n".to_owned());
        harness.editor.answers.borrow_mut().push_back("allow_once");
        let session = harness.new_session().await;

        harness
            .prompt(&session, text("update notes"))
            .await
            .expect("prompt");

        assert_eq!(
            harness
                .editor
                .files
                .borrow()
                .get(std::path::Path::new("/tmp/project/notes.txt")),
            Some(&"new\n".to_owned())
        );
        let asked = harness.editor.permission_requests.borrow();
        let request = asked.first().expect("the user was asked");
        let options: Vec<_> = request.options.iter().map(|o| o.kind).collect();
        assert_eq!(
            options,
            [
                acp::PermissionOptionKind::AllowOnce,
                acp::PermissionOptionKind::AllowAlways,
                acp::PermissionOptionKind::RejectOnce,
            ]
        );
        assert!(matches!(
            request.tool_call.fields.content.as_deref(),
            Some([acp::ToolCallContent::Diff(diff)])
                if diff.old_text.as_deref() == Some("old\n") && diff.new_text == "new\n"
        ));
        assert_eq!(
            tool_output(viktor.requests().get(1).expect("result")).as_deref(),
            Some("Wrote 4 bytes to /tmp/project/notes.txt")
        );
    });
}

#[test]
fn a_rejected_write_leaves_the_file_alone_and_tells_viktor_so() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "call_w",
                vktr_acp::WRITE_FILE,
                &json!({"path": "/tmp/project/a.txt", "content": "x"}),
                "thread-R",
            ),
            fixture_answer("Understood.", "thread-R"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness.editor.answers.borrow_mut().push_back("reject_once");
        let session = harness.new_session().await;

        let (_, stop) = harness
            .prompt(&session, text("write a"))
            .await
            .expect("prompt");
        assert_eq!(stop, acp::StopReason::EndTurn);
        assert!(
            harness.editor.files.borrow().is_empty(),
            "nothing was written"
        );
        let output = tool_output(viktor.requests().get(1).expect("result")).expect("output");
        assert!(output.contains("rejected"), "{output}");
        assert_eq!(
            statuses(&harness.tool_updates()).last(),
            Some(&acp::ToolCallStatus::Failed)
        );
    });
}

#[test]
fn an_editor_that_cannot_ask_never_gets_an_unapproved_write() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "call_w",
                vktr_acp::WRITE_FILE,
                &json!({"path": "a.txt", "content": "x"}),
                "thread-N",
            ),
            fixture_answer("ok", "thread-N"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        // No scripted answer: the editor's request_permission fails.
        let session = harness.new_session().await;
        harness
            .prompt(&session, text("write a"))
            .await
            .expect("prompt");
        assert!(harness.editor.files.borrow().is_empty());
        assert!(
            tool_output(viktor.requests().get(1).expect("result"))
                .is_some_and(|o| o.contains("could not ask the user"))
        );
    });
}

#[test]
fn always_allow_is_remembered_for_the_rest_of_the_session() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "c1",
                vktr_acp::WRITE_FILE,
                &json!({"path": "a", "content": "1"}),
                "t",
            ),
            fixture_call(
                "c2",
                vktr_acp::WRITE_FILE,
                &json!({"path": "b", "content": "2"}),
                "t",
            ),
            fixture_answer("both written", "t"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness
            .editor
            .answers
            .borrow_mut()
            .push_back("allow_always");
        let session = harness.new_session().await;
        harness
            .prompt(&session, text("write two"))
            .await
            .expect("prompt");
        assert_eq!(harness.editor.permission_requests.borrow().len(), 1);
        assert_eq!(harness.editor.files.borrow().len(), 2);
    });
}

#[test]
fn a_command_runs_in_an_editor_terminal_and_its_output_goes_back() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "call_c",
                vktr_acp::RUN_COMMAND,
                &json!({"command": "echo hello", "cwd": "sub"}),
                "thread-C",
            ),
            fixture_answer("It printed hello.", "thread-C"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness.editor.answers.borrow_mut().push_back("allow_once");
        let session = harness.new_session().await;
        harness
            .prompt(&session, text("run it"))
            .await
            .expect("prompt");

        assert_eq!(
            harness.editor.commands.borrow().first().cloned(),
            Some((
                "sh".to_owned(),
                vec!["-c".to_owned(), "echo hello".to_owned()],
                Some("/tmp/project/sub".into()),
            ))
        );
        assert_eq!(
            tool_output(viktor.requests().get(1).expect("result")).as_deref(),
            Some("exit code 0\nhello from the editor terminal\n")
        );
        let embeds_terminal = harness.tool_updates().iter().any(|u| {
            matches!(u, acp::SessionUpdate::ToolCallUpdate(update)
                if matches!(update.fields.content.as_deref(), Some([acp::ToolCallContent::Terminal(t)]) if t.terminal_id.0.as_ref() == "term-1"))
        });
        assert!(
            embeds_terminal,
            "the editor shows the live terminal in the tool call"
        );
    });
}

#[test]
fn a_call_to_a_tool_the_editor_does_not_offer_is_answered_with_an_error_not_run() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call("c", "bash", &json!({"cmd": "ls"}), "t"),
            fixture_answer("ok", "t"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        let session = harness.new_session().await;
        harness.prompt(&session, text("x")).await.expect("prompt");
        assert!(harness.editor.commands.borrow().is_empty());
        assert!(
            tool_output(viktor.requests().get(1).expect("result"))
                .is_some_and(|o| o.contains("not a tool this editor offers"))
        );
    });
}

#[test]
fn cancelling_at_the_permission_prompt_answers_the_open_call_on_the_next_turn() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "c_open",
                vktr_acp::WRITE_FILE,
                &json!({"path": "a", "content": "1"}),
                "t",
            ),
            fixture_answer("fine", "t"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness.editor.answers.borrow_mut().push_back("cancelled");
        let session = harness.new_session().await;

        let (_, stop) = harness
            .prompt(&session, text("write"))
            .await
            .expect("first");
        assert_eq!(stop, acp::StopReason::Cancelled);
        assert!(harness.editor.files.borrow().is_empty());

        harness
            .prompt(&session, text("never mind"))
            .await
            .expect("second");
        let requests = viktor.requests();
        let next = requests.get(1).expect("second turn");
        // A thread with an unanswered call cannot continue, so it is answered first.
        assert_eq!(
            next.body.pointer("/input/0/type"),
            Some(&json!("function_call_output"))
        );
        assert_eq!(
            next.body.pointer("/input/0/call_id"),
            Some(&json!("c_open"))
        );
        assert_eq!(next.body.pointer("/input/1/role"), Some(&json!("user")));
        assert_eq!(next.body.get("previous_response_id"), Some(&json!("t")));
    });
}

// ---------------------------------------------------------------------------------------------
// Sessions survive an agent restart
// ---------------------------------------------------------------------------------------------

fn stored(dir: &std::path::Path) -> vktr_acp::AgentConfig {
    vktr_acp::AgentConfig {
        store: Some(vktr_acp::SessionStore::new(dir)),
        ..vktr_acp::AgentConfig::default()
    }
}

#[test]
fn a_restarted_agent_loads_a_session_replays_it_and_continues_the_same_thread() {
    run(|| async {
        let dir = tempfile::tempdir().expect("tempdir");
        let viktor = MockViktor::start(vec![fixture_text(), fixture_text()]).await;

        let first = Harness::start_with(&viktor.base_url(), stored(dir.path()));
        let init = first.initialize().await;
        assert!(init.agent_capabilities.load_session);
        let session = first.new_session().await;
        first
            .prompt(&session, text("remember 42"))
            .await
            .expect("prompt");

        // A fresh agent process: nothing in memory, only the store.
        let second = Harness::start_with(&viktor.base_url(), stored(dir.path()));
        second.initialize().await;
        let listed = second
            .connection
            .list_sessions(acp::ListSessionsRequest::new().cwd(Some("/tmp/project".into())))
            .await
            .expect("session/list");
        assert_eq!(listed.sessions.len(), 1);
        assert_eq!(
            listed.sessions.first().and_then(|s| s.title.as_deref()),
            Some("remember 42")
        );

        second
            .connection
            .load_session(acp::LoadSessionRequest::new(
                session.clone(),
                "/tmp/project",
            ))
            .await
            .expect("session/load");
        let replayed: Vec<String> = second
            .updates
            .borrow()
            .iter()
            .map(|n| match &n.update {
                acp::SessionUpdate::UserMessageChunk(c) => format!("user: {}", chunk_text(c)),
                acp::SessionUpdate::AgentMessageChunk(c) => format!("agent: {}", chunk_text(c)),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(replayed, ["user: remember 42", "agent: Hello from Viktor."]);

        second
            .prompt(&session, text("what did I say?"))
            .await
            .expect("continue");
        assert_eq!(
            viktor
                .requests()
                .get(1)
                .and_then(|r| r.body.get("previous_response_id"))
                .cloned(),
            Some(json!("zwKTTPTKCc9TVsSMgJuGh")),
            "the reloaded session continues its Viktor thread"
        );
    });
}

#[test]
fn resume_restores_the_thread_without_replaying_and_an_unknown_session_is_an_error() {
    run(|| async {
        let dir = tempfile::tempdir().expect("tempdir");
        let viktor = MockViktor::start(vec![fixture_text(), fixture_text()]).await;
        let first = Harness::start_with(&viktor.base_url(), stored(dir.path()));
        first.initialize().await;
        let session = first.new_session().await;
        first.prompt(&session, text("one")).await.expect("prompt");

        let second = Harness::start_with(&viktor.base_url(), stored(dir.path()));
        second.initialize().await;
        second
            .connection
            .resume_session(acp::ResumeSessionRequest::new(
                session.clone(),
                "/tmp/project",
            ))
            .await
            .expect("session/resume");
        assert!(second.updates.borrow().is_empty(), "resume does not replay");
        second
            .prompt(&session, text("two"))
            .await
            .expect("continue");
        assert_eq!(
            viktor
                .requests()
                .get(1)
                .and_then(|r| r.body.get("previous_response_id"))
                .cloned(),
            Some(json!("zwKTTPTKCc9TVsSMgJuGh"))
        );

        assert!(
            second
                .connection
                .load_session(acp::LoadSessionRequest::new(
                    "no-such-session",
                    "/tmp/project"
                ))
                .await
                .is_err()
        );
    });
}

fn chunk_text(chunk: &acp::ContentChunk) -> String {
    match &chunk.content {
        acp::ContentBlock::Text(t) => t.text.clone(),
        _ => String::new(),
    }
}

#[test]
fn a_prompt_right_after_a_cancel_waits_for_the_busy_thread_instead_of_failing() {
    run(|| async {
        // What the live API answers for ~15 s after a cancelled turn (measured 2026-09-23).
        let body = json!({"error": {
            "message": "This conversation has a response in progress. Wait for it to finish, then retry.",
            "type": "invalid_request_error",
            "code": "conversation_busy",
        }})
        .to_string();
        let busy = format!(
            "HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let viktor = MockViktor::start(vec![busy, fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let (reply, stop) = harness
            .prompt(&session, text("again"))
            .await
            .expect("prompt");
        assert_eq!(reply, "Hello from Viktor.");
        assert_eq!(stop, acp::StopReason::EndTurn);
        assert_eq!(
            viktor.requests().len(),
            2,
            "the busy request was retried once"
        );
        let waited = harness.updates.borrow().iter().any(|n| {
            matches!(&n.update, acp::SessionUpdate::AgentThoughtChunk(chunk) if chunk_text(chunk).contains("still stopping"))
        });
        assert!(waited, "the editor is told why the turn is waiting");
    });
}

#[test]
fn tool_outputs_survive_a_failed_request_and_go_out_before_the_next_message() {
    run(|| async {
        let body =
            json!({"error": {"message": "upstream hiccup", "code": "server_error"}}).to_string();
        let failure = format!(
            "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let viktor = MockViktor::start(vec![
            fixture_call(
                "c_read",
                vktr_acp::READ_FILE,
                &json!({"path": "a.txt"}),
                "t",
            ),
            failure,
            fixture_answer("recovered", "t"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness
            .editor
            .files
            .borrow_mut()
            .insert("/tmp/project/a.txt".into(), "AAA".to_owned());
        let session = harness.new_session().await;

        harness
            .prompt(&session, text("read a"))
            .await
            .expect_err("the request carrying the tool output failed");
        let (reply, _) = harness
            .prompt(&session, text("try again"))
            .await
            .expect("second");
        assert_eq!(reply, "recovered");

        let requests = viktor.requests();
        let retry = requests.get(2).expect("third request");
        assert_eq!(
            retry.body.pointer("/input/0/call_id"),
            Some(&json!("c_read"))
        );
        assert_eq!(retry.body.pointer("/input/0/output"), Some(&json!("AAA")));
        assert_eq!(retry.body.pointer("/input/1/role"), Some(&json!("user")));
    });
}

#[test]
fn owed_tool_outputs_are_stored_so_a_reloaded_session_still_answers_them() {
    run(|| async {
        let dir = tempfile::tempdir().expect("tempdir");
        let viktor = MockViktor::start(vec![
            fixture_call(
                "c_open",
                vktr_acp::WRITE_FILE,
                &json!({"path": "a", "content": "1"}),
                "t",
            ),
            fixture_answer("fine", "t"),
        ])
        .await;
        let first = Harness::start_with(&viktor.base_url(), stored(dir.path()));
        first.initialize_as_editor().await;
        first.editor.answers.borrow_mut().push_back("cancelled");
        let session = first.new_session().await;
        let (_, stop) = first.prompt(&session, text("write")).await.expect("first");
        assert_eq!(stop, acp::StopReason::Cancelled);

        let second = Harness::start_with(&viktor.base_url(), stored(dir.path()));
        second.initialize_as_editor().await;
        second
            .connection
            .resume_session(acp::ResumeSessionRequest::new(
                session.clone(),
                "/tmp/project",
            ))
            .await
            .expect("resume");
        second
            .prompt(&session, text("go on"))
            .await
            .expect("second");
        let requests = viktor.requests();
        let next = requests.get(1).expect("second request");
        assert_eq!(
            next.body.pointer("/input/0/call_id"),
            Some(&json!("c_open"))
        );
        assert_eq!(next.body.pointer("/input/1/role"), Some(&json!("user")));
    });
}

#[test]
fn a_read_outside_the_workspace_asks_first() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "c",
                vktr_acp::READ_FILE,
                &json!({"path": "../../etc/hosts"}),
                "t",
            ),
            fixture_answer("ok", "t"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize_as_editor().await;
        harness
            .editor
            .files
            .borrow_mut()
            .insert("/etc/hosts".into(), "127.0.0.1 localhost".to_owned());
        harness.editor.answers.borrow_mut().push_back("reject_once");
        let session = harness.new_session().await;
        harness
            .prompt(&session, text("peek"))
            .await
            .expect("prompt");

        let asked = harness.editor.permission_requests.borrow();
        assert_eq!(
            asked
                .first()
                .and_then(|r| r.tool_call.fields.title.clone())
                .as_deref(),
            Some("Read /etc/hosts (outside the workspace)")
        );
        let output = tool_output(viktor.requests().get(1).expect("result")).expect("output");
        assert!(
            output.contains("rejected") && !output.contains("localhost"),
            "{output}"
        );
    });
}

#[test]
fn a_prompt_that_supersedes_a_running_one_waits_for_it_to_stop() {
    run(|| async {
        let never_finishes = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-request-id: req-test-1\r\n\r\n: keep-alive\n\n".to_owned();
        let viktor = MockViktor::start(vec![never_finishes, fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let session = harness.new_session().await;

        let first = harness
            .connection
            .prompt(acp::PromptRequest::new(session.clone(), text("slow")));
        let second = async {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            harness
                .connection
                .prompt(acp::PromptRequest::new(session.clone(), text("fast")))
                .await
        };
        let (first, second) = tokio::join!(first, second);
        assert_eq!(
            first.expect("first").stop_reason,
            acp::StopReason::Cancelled
        );
        assert_eq!(
            second.expect("second").stop_reason,
            acp::StopReason::EndTurn
        );
    });
}

// ---------------------------------------------------------------------------------------------
// The editor's MCP servers become Viktor tools
// ---------------------------------------------------------------------------------------------

fn mock_mcp() -> acp::McpServer {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_mcp.py");
    acp::McpServer::Stdio(
        acp::McpServerStdio::new("mock", "python3").args(vec![script.display().to_string()]),
    )
}

impl Harness {
    async fn new_session_with(&self, servers: Vec<acp::McpServer>) -> acp::SessionId {
        self.connection
            .new_session(acp::NewSessionRequest::new("/tmp").mcp_servers(servers))
            .await
            .expect("session/new")
            .session_id
    }
}

#[test]
fn an_editor_mcp_server_is_offered_to_viktor_and_its_tool_round_trips() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call(
                "c_mcp",
                "mcp__mock__lookup",
                &json!({"code": "A7"}),
                "t-mcp",
            ),
            fixture_answer("The code means A7.", "t-mcp"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        let init = harness.initialize().await;
        assert!(
            init.agent_capabilities.mcp_capabilities.http,
            "HTTP MCP servers are accepted"
        );
        harness.editor.answers.borrow_mut().push_back("allow_once");
        let session = harness.new_session_with(vec![mock_mcp()]).await;

        let (reply, stop) = harness
            .prompt(&session, text("look up A7"))
            .await
            .expect("prompt");
        assert_eq!(reply, "The code means A7.");
        assert_eq!(stop, acp::StopReason::EndTurn);

        let requests = viktor.requests();
        let first = requests.first().expect("first request");
        assert!(
            tool_names(first).contains(&"mcp__mock__lookup".to_owned()),
            "{:?}",
            tool_names(first)
        );
        let description = first
            .body
            .get("tools")
            .and_then(Value::as_array)
            .and_then(|t| {
                t.iter()
                    .find(|t| t.get("name") == Some(&json!("mcp__mock__lookup")))
            })
            .and_then(|t| t.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(description.contains("user's machine"), "{description}");
        assert_eq!(
            tool_output(requests.get(1).expect("result")).as_deref(),
            Some("MCP_LOOKUP:A7")
        );
        let asked = harness.editor.permission_requests.borrow();
        assert_eq!(
            asked
                .first()
                .and_then(|r| r.tool_call.fields.title.clone())
                .as_deref(),
            Some("mock: lookup"),
            "the user is asked before an MCP tool runs"
        );
    });
}

#[test]
fn a_rejected_or_failing_mcp_call_is_reported_to_viktor() {
    run(|| async {
        let viktor = MockViktor::start(vec![
            fixture_call("c1", "mcp__mock__lookup", &json!({"code": "X"}), "t"),
            fixture_call("c2", "mcp__mock__fail", &json!({}), "t"),
            fixture_answer("ok", "t"),
        ])
        .await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        harness.editor.answers.borrow_mut().push_back("reject_once");
        harness.editor.answers.borrow_mut().push_back("allow_once");
        let session = harness.new_session_with(vec![mock_mcp()]).await;
        harness.prompt(&session, text("go")).await.expect("prompt");

        let requests = viktor.requests();
        let rejected = tool_output(requests.get(1).expect("second")).expect("output");
        assert!(rejected.contains("rejected"), "{rejected}");
        let failed = tool_output(requests.get(2).expect("third")).expect("output");
        assert_eq!(failed, "error: it broke");
    });
}

#[test]
fn an_mcp_server_that_cannot_start_is_named_and_the_session_still_works() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        let harness = Harness::start(&viktor.base_url());
        harness.initialize().await;
        let broken = acp::McpServer::Stdio(acp::McpServerStdio::new(
            "broken",
            "/nonexistent-dir-vktr/no-such-server",
        ));
        let session = harness.new_session_with(vec![broken]).await;
        let (reply, _) = harness.prompt(&session, text("hi")).await.expect("prompt");
        assert_eq!(reply, "Hello from Viktor.");
        let warned = harness.updates.borrow().iter().any(|n| {
            matches!(&n.update, acp::SessionUpdate::AgentThoughtChunk(c) if chunk_text(c).contains("`broken` did not start"))
        });
        assert!(warned, "the user is told which MCP server failed");
        assert!(
            viktor
                .requests()
                .first()
                .is_some_and(|r| r.body.get("tools").is_none())
        );
    });
}

#[test]
fn mcp_servers_can_be_turned_off() {
    run(|| async {
        let viktor = MockViktor::start(vec![fixture_text()]).await;
        let config = vktr_acp::AgentConfig {
            mcp_servers: false,
            ..vktr_acp::AgentConfig::default()
        };
        let harness = Harness::start_with(&viktor.base_url(), config);
        harness.initialize().await;
        let session = harness.new_session_with(vec![mock_mcp()]).await;
        harness.prompt(&session, text("hi")).await.expect("prompt");
        assert!(
            viktor
                .requests()
                .first()
                .is_some_and(|r| r.body.get("tools").is_none())
        );
    });
}
