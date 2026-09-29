//! The narrow Viktor compat-API client `vktr acp` needs: list the models (to verify a key) and
//! stream one Responses run.
//!
//! Ported from `createViktorClient` and `readResponsesStream` in the TypeScript agent. The full
//! vktr sampler is deliberately not reused here: `vktr acp` speaks to Viktor's own sandbox and
//! runs no local tools, so it needs a raw event stream rather than the agent runtime's
//! conversation machinery.

use std::time::Duration;

use futures::StreamExt as _;
use serde_json::{Value, json};

use crate::error::{ViktorError, ViktorErrorCode, error_from_response, parse_error_body};

/// The only model the Viktor compat API serves.
pub const VIKTOR_MODEL_ID: &str = "viktor";

/// Viktor caps a compat run at 600 s; stay above it so the server, not the client, ends the run.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(660);

/// Viktor emits `[Stream error: …]` as assistant text just before `response.failed`. The editor
/// must see the failure as a protocol error, not as words in the reply, so the text is dropped.
const STREAM_ERROR_PREFIX: &str = "[Stream error: ";

/// Reduce any accepted spelling of the Viktor endpoint to its OpenAI-style `/v1` root.
///
/// vktr configures Viktor as `https://api.viktor.com/api/compat/v1` and treats that value as the
/// `/v1` root everywhere else (`vktr login` asks it for `/models`), while the TypeScript agent
/// took the bare host and appended the compat path itself. Both spellings are accepted, so
/// `VIKTOR_BASE_URL` means the same thing to `vktr`, to `vktr acp` and to the old TS agent —
/// including the plain `/v1` roots that the smoke tests and local proxies serve.
pub fn resolve_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else if trimmed.ends_with("/api/compat") {
        format!("{trimmed}/v1")
    } else {
        format!("{trimmed}/api/compat/v1")
    }
}

/// A client for one Viktor deployment.
pub struct ViktorClient {
    /// The OpenAI-style `/v1` root; request paths hang directly off it.
    root: String,
    api_key: String,
    http: reqwest::Client,
}

impl ViktorClient {
    /// `base_url` may be the bare host or include the compat path.
    pub fn new(api_key: impl Into<String>, base_url: &str) -> Result<Self, ViktorError> {
        // The shared builder applies vktr's TLS policy and survives a broken OS trust store.
        let http = xai_grok_extra_ca::build_reqwest_client(|b| b.timeout(REQUEST_TIMEOUT))
            .map_err(|e| {
                ViktorError::new(
                    ViktorErrorCode::ServerError,
                    format!("build HTTP client: {e}"),
                )
            })?;
        Ok(Self {
            root: resolve_base_url(base_url),
            api_key: api_key.into(),
            http,
        })
    }

    /// The compat root, as shown to the user in diagnostics.
    pub fn compat_base_url(&self) -> &str {
        &self.root
    }

    /// `GET /api/compat/v1/models`. Used to verify a key during `authenticate`.
    pub async fn list_models(&self) -> Result<Vec<String>, ViktorError> {
        let url = format!("{}/models", self.root);
        let response = self.send(self.http.get(&url), &url).await?;
        let request_id = header(&response, "x-request-id");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let body = parse_json_or_text(&text);
        if !(200..300).contains(&status) {
            return Err(error_from_response(status, request_id, &body));
        }
        Ok(body
            .get("data")
            .and_then(Value::as_array)
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `POST /api/compat/v1/responses` with `stream: true`.
    pub async fn responses_stream(&self, body: Value) -> Result<ResponsesStream, ViktorError> {
        let url = format!("{}/responses", self.root);
        let mut payload = body;
        if let Some(object) = payload.as_object_mut() {
            object.insert("model".to_owned(), json!(VIKTOR_MODEL_ID));
            object.insert("stream".to_owned(), json!(true));
        }
        let request = self
            .http
            .post(&url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&payload);
        let response = self.send(request, &url).await?;
        let request_id = header(&response, "x-request-id");
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let text = response.text().await.unwrap_or_default();
            return Err(error_from_response(
                status,
                request_id,
                &parse_json_or_text(&text),
            ));
        }
        Ok(ResponsesStream {
            bytes: Box::new(response.bytes_stream()),
            buffer: String::new(),
            data: Vec::new(),
            event: None,
            request_id,
            finished: false,
        })
    }

    /// Send a request with the API key attached, turning a transport failure into a
    /// `server_error` that names the endpoint (a wrong `VIKTOR_BASE_URL` is a common mistake).
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        url: &str,
    ) -> Result<reqwest::Response, ViktorError> {
        request
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| {
                ViktorError::new(
                    ViktorErrorCode::ServerError,
                    format!("Could not reach Viktor at {url}: {e}"),
                )
            })
    }
}

fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn parse_json_or_text(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

/// A Responses API event stream.
///
/// Yields the run's events as they arrive. A terminal `response.failed` becomes an error rather
/// than an event, so a failed run can never be mistaken for a finished one.
pub struct ResponsesStream {
    bytes: Box<dyn futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin>,
    buffer: String,
    data: Vec<String>,
    event: Option<String>,
    request_id: Option<String>,
    finished: bool,
}

impl ResponsesStream {
    /// The next event, or `None` once the run is over.
    pub async fn next_event(&mut self) -> Option<Result<Value, ViktorError>> {
        loop {
            if self.finished {
                return None;
            }
            // Drain whatever complete SSE frames the buffer already holds.
            while let Some(frame) = self.take_frame() {
                match self.interpret(frame) {
                    Frame::Done => {
                        self.finished = true;
                        return None;
                    }
                    Frame::Event(event) => return Some(Ok(event)),
                    Frame::Failed(error) => {
                        self.finished = true;
                        return Some(Err(error));
                    }
                    Frame::Ignore => {}
                }
            }
            match self.bytes.next().await {
                Some(Ok(chunk)) => self.buffer.push_str(&String::from_utf8_lossy(&chunk)),
                Some(Err(e)) => {
                    self.finished = true;
                    return Some(Err(ViktorError::new(
                        ViktorErrorCode::ServerError,
                        format!("Viktor stream ended early: {e}"),
                    )
                    .with_request_id(self.request_id.clone())));
                }
                None => {
                    // End of stream: flush a frame the server did not terminate with a blank line.
                    self.finished = true;
                    let trailing = std::mem::take(&mut self.buffer);
                    if let Some(rest) = trailing.strip_prefix("data:") {
                        self.data
                            .push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
                    }
                    if !self.data.is_empty() {
                        let frame = self.take_pending();
                        if let Frame::Event(event) = self.interpret(frame) {
                            return Some(Ok(event));
                        }
                    }
                    return None;
                }
            }
        }
    }

    /// Pull one complete SSE frame out of the buffer, if there is one.
    ///
    /// Comment lines (Viktor sends `: keep-alive` every 15 s while it works) and `retry:` lines
    /// are dropped and never surface as content.
    fn take_frame(&mut self) -> Option<PendingFrame> {
        while let Some(end) = self.buffer.find('\n') {
            let line = self
                .buffer
                .get(..end)
                .unwrap_or_default()
                .trim_end_matches('\r')
                .to_owned();
            self.buffer.drain(..=end);
            if line.is_empty() {
                if !self.data.is_empty() {
                    return Some(self.take_pending());
                }
            } else if line.starts_with(':') {
                // comment / keep-alive
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
            } else if let Some(rest) = line.strip_prefix("event:") {
                self.event = Some(rest.trim().to_owned());
            }
        }
        None
    }

    fn take_pending(&mut self) -> PendingFrame {
        PendingFrame {
            data: std::mem::take(&mut self.data).join("\n"),
            event: self.event.take(),
        }
    }

    fn interpret(&self, frame: PendingFrame) -> Frame {
        if frame.data == "[DONE]" {
            return Frame::Done;
        }
        let Ok(mut payload) = serde_json::from_str::<Value>(&frame.data) else {
            return Frame::Ignore; // tolerate non-JSON frames
        };
        let kind = payload
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(frame.event)
            .unwrap_or_default();

        if kind == "response.failed" {
            let error = payload.get("response").and_then(|r| r.get("error"));
            let message = error
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            let detail_code = error
                .and_then(|e| e.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            return Frame::Failed(
                ViktorError::run_failed(message)
                    .with_status(200)
                    .with_request_id(self.request_id.clone())
                    .with_detail_code(detail_code),
            );
        }
        // An in-stream `{"error": …}` frame is the other shape a failed run takes.
        if kind.is_empty() && payload.get("error").is_some_and(Value::is_object) {
            let parsed = parse_error_body(&payload);
            return Frame::Failed(
                ViktorError::run_failed(parsed.message.as_deref().unwrap_or("unknown error"))
                    .with_status(200)
                    .with_request_id(self.request_id.clone())
                    .with_detail_code(parsed.detail_code),
            );
        }
        if kind == "response.output_text.delta"
            && payload
                .get("delta")
                .and_then(Value::as_str)
                .is_some_and(|d| d.trim_start().starts_with(STREAM_ERROR_PREFIX))
        {
            return Frame::Ignore;
        }
        if let Some(object) = payload.as_object_mut() {
            object.insert("type".to_owned(), json!(kind));
        }
        Frame::Event(payload)
    }
}

struct PendingFrame {
    data: String,
    event: Option<String>,
}

enum Frame {
    Event(Value),
    Failed(ViktorError),
    Done,
    Ignore,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_accepted_spelling_of_the_endpoint_reduces_to_the_same_v1_root() {
        for input in [
            // The bare host, which is the spelling the TypeScript agent took.
            "https://api.viktor.com",
            "https://api.viktor.com/",
            "https://api.viktor.com/api/compat",
            // vktr's own configured value, and what `vktr login` verifies against.
            "https://api.viktor.com/api/compat/v1",
            "https://api.viktor.com/api/compat/v1/",
        ] {
            assert_eq!(
                resolve_base_url(input),
                "https://api.viktor.com/api/compat/v1",
                "{input}"
            );
        }
    }

    #[test]
    fn a_plain_v1_root_is_used_as_given() {
        // What the smoke tests and local proxies serve: an OpenAI-style root that is not under
        // /api/compat. Appending the compat path to it would miss the endpoint entirely.
        assert_eq!(
            resolve_base_url("http://127.0.0.1:8765/v1"),
            "http://127.0.0.1:8765/v1"
        );
    }
}
