//! `previous_response_id` continuation for the Viktor compat API (Responses backend).
//!
//! Viktor's Responses surface is stateful: a request without `previous_response_id` runs in a fresh
//! ephemeral thread, and a request that carries one resumes the durable thread it names. The id Viktor
//! returns for a response *is* that thread id. When resuming, Viktor appends every inbound item to the
//! thread, so a continuing client must send only what is new.
//!
//! vktr always holds the full conversation, so continuation is an optimisation layered on stateless
//! replay, never a requirement. Per conversation id we remember the last response id plus a fingerprint
//! of the input that produced it. The next request continues only when its input starts with exactly
//! that input; anything else (compaction, rewind, an edited history, a restart) falls back to a full
//! replay in a fresh thread. A failed continuation forgets the entry, so the retry replays in full.
//!
//! Disable with `VKTR_RESPONSES_CONTINUATION=0`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};

use async_openai::types::responses as rs;

/// The only model served by the Viktor compat API.
const VIKTOR_MODEL: &str = "viktor";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Entry {
    response_id: String,
    /// Number of input items in the request that produced `response_id`.
    sent_items: usize,
    /// Fingerprint of those items.
    fingerprint: u64,
    /// Unix seconds of the last update; the disk copy keeps the newest entries.
    #[serde(default)]
    updated_at: u64,
}

/// Seeded from `~/.vktr/viktor_threads.json` so a resumed session (`vktr --continue`) keeps its Viktor thread.
static ENTRIES: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(load_from_disk()));

/// Newest entries kept on disk; older conversations simply replay in full when resumed.
const MAX_PERSISTED: usize = 200;

fn store_path() -> Option<std::path::PathBuf> {
    xai_dirs::user_grok_home().map(|home| home.join("viktor_threads.json"))
}

fn load_from_disk() -> HashMap<String, Entry> {
    store_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Vec<(String, Entry)>>(&text).ok())
        .map(|pairs| pairs.into_iter().collect())
        .unwrap_or_default()
}

/// Best effort: a failed write only costs a full replay after the next restart.
fn save_to_disk(map: &HashMap<String, Entry>) {
    let Some(path) = store_path() else { return };
    let mut pairs: Vec<(&String, &Entry)> = map.iter().collect();
    pairs.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.updated_at));
    pairs.truncate(MAX_PERSISTED);
    if let Ok(text) = serde_json::to_string(&pairs) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What to record once the response completes: the full (untrimmed) input of this request.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    conv_id: String,
    sent_items: usize,
    fingerprint: u64,
    /// True when this request was rewritten to continue a thread (as opposed to a full replay).
    continued: bool,
}

fn enabled() -> bool {
    !matches!(
        std::env::var("VKTR_RESPONSES_CONTINUATION")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("0" | "false" | "off" | "no")
    )
}

fn item_json(item: &rs::InputItem) -> serde_json::Value {
    serde_json::to_value(item).unwrap_or(serde_json::Value::Null)
}

fn fingerprint(items: &[serde_json::Value]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for item in items {
        item.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// True for items the *client* contributes (a user/system/developer message or a tool result). Everything
/// else after the previously sent prefix is the model's own output, which the Viktor thread already holds.
fn is_client_item(item: &serde_json::Value) -> bool {
    match item.get("type").and_then(|t| t.as_str()) {
        Some("function_call_output" | "custom_tool_call_output" | "mcp_approval_response") => true,
        Some("message") | None => {
            !matches!(item.get("role").and_then(|r| r.as_str()), Some("assistant"))
        }
        _ => false,
    }
}

/// Decide whether `request` can continue a Viktor thread. When it can, `request` is rewritten in place
/// (`previous_response_id` set, input trimmed to the new client items). Returns what to record when the
/// response completes, or `None` when continuation does not apply to this request at all.
pub(crate) fn plan(
    conv_id: Option<&str>,
    effective_model: &str,
    request: &mut rs::CreateResponse,
) -> Option<Pending> {
    if !enabled() || effective_model != VIKTOR_MODEL {
        return None;
    }
    let conv_id = conv_id.filter(|id| !id.is_empty())?;
    let rs::InputParam::Items(items) = &request.input else {
        return None;
    };
    let json: Vec<serde_json::Value> = items.iter().map(item_json).collect();
    let mut pending = Pending {
        conv_id: conv_id.to_owned(),
        sent_items: json.len(),
        fingerprint: fingerprint(&json),
        continued: false,
    };

    let entry = ENTRIES
        .lock()
        .ok()
        .and_then(|map| map.get(conv_id).cloned());
    if let Some(entry) = entry {
        let prefix_matches = json.len() > entry.sent_items
            && json
                .get(..entry.sent_items)
                .is_some_and(|prefix| fingerprint(prefix) == entry.fingerprint);
        let delta_start = prefix_matches
            .then(|| {
                json.iter()
                    .enumerate()
                    .skip(entry.sent_items)
                    .find(|(_, item)| is_client_item(item))
                    .map(|(idx, _)| idx)
            })
            .flatten();
        match delta_start {
            Some(start) => {
                let delta: Vec<rs::InputItem> = items.iter().skip(start).cloned().collect();
                tracing::info!(
                    conv_id,
                    previous_response_id = %entry.response_id,
                    full_items = json.len(),
                    delta_items = delta.len(),
                    "viktor continuation: resuming the durable thread with new items only"
                );
                request.previous_response_id = Some(entry.response_id);
                request.input = rs::InputParam::Items(delta);
                pending.continued = true;
            }
            None => {
                tracing::info!(
                    conv_id,
                    "viktor continuation: history diverged from the last request; replaying in full"
                );
                forget(conv_id);
            }
        }
    }
    Some(pending)
}

/// Record the thread id once a response completed.
pub(crate) fn record(pending: &Pending, response_id: &str) {
    if response_id.is_empty() {
        return;
    }
    if let Ok(mut map) = ENTRIES.lock() {
        map.insert(
            pending.conv_id.clone(),
            Entry {
                response_id: response_id.to_owned(),
                sent_items: pending.sent_items,
                fingerprint: pending.fingerprint,
                updated_at: now_secs(),
            },
        );
        save_to_disk(&map);
    }
}

/// Drop the entry so the next request replays the whole history in a fresh thread.
pub(crate) fn forget(conv_id: &str) {
    if let Ok(mut map) = ENTRIES.lock()
        && map.remove(conv_id).is_some()
    {
        save_to_disk(&map);
    }
}

impl Pending {
    pub(crate) fn conv_id(&self) -> &str {
        &self.conv_id
    }

    /// A continued request that Viktor refused because the thread is gone (expired, another deployment,
    /// another key) must not surface as an error: the entry is already forgotten, so reporting a
    /// retryable transport error makes the sampler resend, and the resend replays the full history.
    pub(crate) fn recover(
        &self,
        err: xai_grok_sampling_types::SamplingError,
    ) -> xai_grok_sampling_types::SamplingError {
        use xai_grok_sampling_types::SamplingError;
        let thread_gone = self.continued
            && match &err {
                SamplingError::Api {
                    status, error_code, ..
                } => {
                    status.as_u16() == 404
                        || error_code
                            .as_ref()
                            .is_some_and(|c| c.as_str() == "previous_response_not_found")
                }
                _ => false,
            };
        if thread_gone {
            tracing::warn!(
                conv_id = %self.conv_id,
                "viktor continuation: the thread is gone on the server; replaying the full history"
            );
            SamplingError::EventStreamError(
                "Viktor thread not found; replaying the conversation in a new thread".to_string(),
            )
        } else {
            err
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> serde_json::Value {
        serde_json::json!({"type": "message", "role": "user", "content": text})
    }
    fn assistant(text: &str) -> serde_json::Value {
        serde_json::json!({"type": "message", "role": "assistant", "content": text})
    }

    #[test]
    fn client_items_are_user_messages_and_tool_results() {
        assert!(is_client_item(&user("hi")));
        assert!(is_client_item(
            &serde_json::json!({"type": "function_call_output", "call_id": "c", "output": "x"})
        ));
        assert!(!is_client_item(&assistant("hello")));
        assert!(!is_client_item(
            &serde_json::json!({"type": "function_call", "call_id": "c", "name": "n", "arguments": "{}"})
        ));
        assert!(!is_client_item(
            &serde_json::json!({"type": "reasoning", "summary": []})
        ));
    }

    #[test]
    fn fingerprint_is_order_and_content_sensitive() {
        let a = vec![user("one"), assistant("two")];
        let b = vec![assistant("two"), user("one")];
        let c = vec![user("one"), assistant("TWO")];
        assert_eq!(fingerprint(&a), fingerprint(&a.clone()));
        assert_ne!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&c));
    }
}
