//! The Viktor error taxonomy `vktr acp` needs, ported from the TypeScript `viktor-acp`
//! agent's `@viktor/integrations-core` (`packages/core/src/errors.ts`).
//!
//! Only the distinctions that change ACP behaviour are kept: a failed run must reach the
//! editor as a JSON-RPC error carrying Viktor's own message and request id, never as
//! assistant text, and an auth failure must say how to fix the key.

use std::fmt;

/// Error classes that Viktor's compat API distinguishes on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViktorErrorCode {
    /// The run failed server-side: HTTP 502 `run_failed`, an opaque 502, or a terminal
    /// `response.failed` frame. Billed, and never worth retrying automatically.
    RunFailed,
    Auth,
    RateLimit,
    RequestTooLarge,
    InvalidRequest,
    ServerError,
}

impl ViktorErrorCode {
    /// The stable string reported to the editor in the JSON-RPC error `data`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RunFailed => "run_failed",
            Self::Auth => "auth",
            Self::RateLimit => "rate_limit",
            Self::RequestTooLarge => "request_too_large",
            Self::InvalidRequest => "invalid_request",
            Self::ServerError => "server_error",
        }
    }
}

/// A failure from the Viktor compat API.
#[derive(Clone, Debug)]
pub struct ViktorError {
    pub code: ViktorErrorCode,
    pub message: String,
    pub status: Option<u16>,
    /// `x-request-id`, so a failure can be traced back to the run that produced it.
    pub request_id: Option<String>,
    /// Viktor's own error code (`identity_denied`, `missing_scope`, ...).
    pub detail_code: Option<String>,
}

impl ViktorError {
    pub fn new(code: ViktorErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            request_id: None,
            detail_code: None,
        }
    }

    pub fn run_failed(message: impl fmt::Display) -> Self {
        Self::new(
            ViktorErrorCode::RunFailed,
            format!("Viktor run failed: {message}"),
        )
    }

    #[must_use]
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    #[must_use]
    pub fn with_detail_code(mut self, detail_code: Option<String>) -> Self {
        self.detail_code = detail_code;
        self
    }

    /// HTTP 409 `conversation_busy`: the thread still has a run in progress. After the editor
    /// cancels a turn, Viktor takes about 15 s to stop the run it was streaming (measured live
    /// 2026-09-23), so the next prompt meets this unless it waits.
    pub fn is_conversation_busy(&self) -> bool {
        self.status == Some(409) && self.detail_code.as_deref() == Some("conversation_busy")
    }
}

impl fmt::Display for ViktorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ViktorError {}

/// What `parse_error_body` could recover from an error envelope.
#[derive(Default)]
pub(crate) struct ParsedErrorBody {
    pub message: Option<String>,
    pub detail_code: Option<String>,
}

fn non_empty(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Understands the three envelopes Viktor uses: REST `{detail}`, OpenAI `{error}`, and
/// Anthropic `{type: "error", error}`.
pub(crate) fn parse_error_body(body: &serde_json::Value) -> ParsedErrorBody {
    if let Some(text) = body.as_str() {
        return ParsedErrorBody {
            message: (!text.is_empty()).then(|| text.to_owned()),
            detail_code: None,
        };
    }
    if let Some(detail) = body.get("detail") {
        if let Some(text) = detail.as_str() {
            let detail_code = if text.contains("scope required") {
                Some("missing_scope".to_owned())
            } else {
                Some(text.to_owned())
            };
            return ParsedErrorBody {
                message: Some(text.to_owned()),
                detail_code,
            };
        }
        if let Some(items) = detail.as_array() {
            let messages: Vec<String> = items
                .iter()
                .filter_map(|item| non_empty(item.get("msg")))
                .collect();
            let message = if messages.is_empty() {
                "Request validation failed".to_owned()
            } else {
                messages.join("; ")
            };
            return ParsedErrorBody {
                message: Some(message),
                detail_code: Some("validation_error".to_owned()),
            };
        }
        if detail.is_object() {
            return ParsedErrorBody {
                message: non_empty(detail.get("message")),
                detail_code: non_empty(detail.get("error")),
            };
        }
    }
    if let Some(error) = body.get("error").filter(|e| e.is_object()) {
        return ParsedErrorBody {
            message: non_empty(error.get("message")),
            detail_code: non_empty(error.get("code")).or_else(|| non_empty(error.get("type"))),
        };
    }
    ParsedErrorBody::default()
}

/// Actionable hints for the auth failures a user can actually fix.
fn auth_hint(detail_code: Option<&str>) -> Option<&'static str> {
    Some(match detail_code? {
        "invalid_api_key" => {
            "Check VIKTOR_API_KEY. Keys look like zt_live_sk_… and are shown once when created."
        }
        "api_key_inactive" => "The API key was deactivated. Create a new key in Viktor settings.",
        "api_key_expired" => "The API key expired. Create a new key in Viktor settings.",
        "missing_scope" => {
            "The API key lacks the scope named above; the chat model needs chat:completions."
        }
        "identity_denied" => {
            "The key's owner has no linked Slack or Teams identity, so Viktor cannot run as them."
        }
        "identity_unsupported_platform" => {
            "The key owner's chat platform is not supported for API runs."
        }
        "compat_api_not_enabled" => "This Viktor environment does not serve the compatibility API.",
        _ => return None,
    })
}

/// Map a non-2xx Viktor response onto the matching error class.
pub(crate) fn error_from_response(
    status: u16,
    request_id: Option<String>,
    body: &serde_json::Value,
) -> ViktorError {
    let parsed = parse_error_body(body);
    let message = parsed
        .message
        .clone()
        .unwrap_or_else(|| format!("Viktor API returned HTTP {status}"));
    let detail_code = parsed.detail_code.clone();

    let error = match status {
        401 | 403 => {
            let message = match auth_hint(detail_code.as_deref()) {
                Some(hint) => format!("{message}. {hint}"),
                None => message,
            };
            ViktorError::new(ViktorErrorCode::Auth, message)
        }
        429 => ViktorError::new(ViktorErrorCode::RateLimit, message),
        413 => ViktorError::new(ViktorErrorCode::RequestTooLarge, message),
        502 if detail_code.as_deref() == Some("run_failed") => ViktorError::run_failed(message),
        // In production the CDN replaces Viktor's JSON 502 body with its own HTML page, so a
        // failed run often arrives opaque. It still counts as a failed (billed) run.
        502 => ViktorError::run_failed(
            "HTTP 502 from Viktor. The run most likely failed; a proxy replaced Viktor's error \
             detail. Stream the request to see Viktor's own message.",
        )
        .with_detail_code(Some(
            detail_code
                .clone()
                .unwrap_or_else(|| "run_failed_opaque".to_owned()),
        )),
        s if s >= 500 => ViktorError::new(ViktorErrorCode::ServerError, message),
        _ => ViktorError::new(ViktorErrorCode::InvalidRequest, message),
    };
    let detail_code = error.detail_code.clone().or(detail_code);
    error
        .with_status(status)
        .with_request_id(request_id)
        .with_detail_code(detail_code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rest_openai_and_anthropic_envelopes_are_all_understood() {
        assert_eq!(
            parse_error_body(&json!({"detail": "chat:completions scope required"})).detail_code,
            Some("missing_scope".to_owned())
        );
        let openai = parse_error_body(&json!({"error": {"message": "boom", "code": "run_failed"}}));
        assert_eq!(openai.message, Some("boom".to_owned()));
        assert_eq!(openai.detail_code, Some("run_failed".to_owned()));
        let anthropic = parse_error_body(
            &json!({"type": "error", "error": {"message": "no", "type": "overloaded"}}),
        );
        assert_eq!(anthropic.detail_code, Some("overloaded".to_owned()));
        let validation = parse_error_body(&json!({"detail": [{"msg": "field required"}]}));
        assert_eq!(validation.message, Some("field required".to_owned()));
    }

    #[test]
    fn an_opaque_502_is_still_a_failed_run_not_a_server_error() {
        let err = error_from_response(502, None, &json!("<html>bad gateway</html>"));
        assert_eq!(err.code, ViktorErrorCode::RunFailed);
        assert_eq!(err.detail_code.as_deref(), Some("run_failed_opaque"));
    }

    #[test]
    fn an_auth_failure_carries_the_hint_that_fixes_it() {
        let err = error_from_response(
            401,
            Some("req-1".to_owned()),
            &json!({"error": {"message": "Invalid key", "code": "invalid_api_key"}}),
        );
        assert_eq!(err.code, ViktorErrorCode::Auth);
        assert!(err.message.contains("zt_live_sk_"));
        assert_eq!(err.request_id.as_deref(), Some("req-1"));
    }
}
