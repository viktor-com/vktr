//! The editor's MCP servers, offered to Viktor as tools.
//!
//! An ACP editor lists its configured MCP servers in `session/new` (Zed's context servers, a
//! JetBrains IDE's MCP settings). Viktor runs in the cloud and cannot reach them, so `vktr acp`
//! connects to them on the user's machine and passes their tools to Viktor as caller tools named
//! `mcp__<server>__<tool>`. A call comes back on the Responses stream, runs against the server here,
//! and the result goes back on the same Viktor thread. The editor asks the user before each call,
//! since an MCP tool can do anything.
//!
//! stdio servers run in their own process group and are killed with the session; HTTP servers
//! use the streamable-HTTP transport with the editor's headers. SSE servers are not supported
//! (the capability is not advertised, so editors do not send them).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use agent_client_protocol as acp;
use serde_json::{Value, json};
use xai_grok_mcp::rmcp;
use xai_grok_mcp::rmcp::ServiceExt as _;

/// Prefix of every tool name that belongs to an MCP server.
pub const PREFIX: &str = "mcp__";

/// Longest function name the Responses API accepts.
const MAX_NAME: usize = 64;

/// How long one server gets to start and list its tools.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long one tool call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest tool result sent back to Viktor.
const MAX_OUTPUT_CHARS: usize = 200_000;

type Client = rmcp::service::RunningService<rmcp::RoleClient, ()>;

struct Server {
    name: String,
    client: Client,
    /// A stdio server's process and its enrolled group; the hub's scope kills the group with the
    /// session, grandchildren included.
    _child: Option<(
        tokio::process::Child,
        std::sync::Arc<xai_tty_utils::ProcessGroup>,
    )>,
}

/// One MCP tool as Viktor sees it.
#[derive(Clone, Debug)]
struct Offered {
    server: usize,
    tool: String,
    definition: Value,
}

/// The MCP servers of one ACP session.
pub struct McpHub {
    servers: Vec<Server>,
    tools: HashMap<String, Offered>,
    order: Vec<String>,
    scope: xai_tty_utils::ProcessScope,
}

impl Default for McpHub {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            tools: HashMap::new(),
            order: Vec::new(),
            scope: xai_tty_utils::ProcessScope::new(),
        }
    }
}

impl Drop for McpHub {
    fn drop(&mut self) {
        self.scope.kill_all();
    }
}

impl McpHub {
    /// Connect to every server the editor listed. A server that fails is left out and named in
    /// the returned warnings; the others still work.
    pub async fn connect(configs: &[acp::McpServer], cwd: &Path) -> (Self, Vec<String>) {
        let mut hub = Self::default();
        let mut warnings = Vec::new();
        for config in configs {
            let Some(name) = server_name(config) else {
                continue;
            };
            let connected =
                tokio::time::timeout(CONNECT_TIMEOUT, connect_one(config, cwd, &hub.scope)).await;
            let (client, child) = match connected {
                Ok(Ok(pair)) => pair,
                Ok(Err(error)) => {
                    warnings.push(format!("MCP server `{name}` did not start: {error}"));
                    continue;
                }
                Err(_) => {
                    warnings.push(format!(
                        "MCP server `{name}` did not answer within {} s",
                        CONNECT_TIMEOUT.as_secs()
                    ));
                    continue;
                }
            };
            let tools = match tokio::time::timeout(CONNECT_TIMEOUT, client.list_all_tools()).await {
                Ok(Ok(tools)) => tools,
                Ok(Err(error)) => {
                    warnings.push(format!("MCP server `{name}` could not list tools: {error}"));
                    continue;
                }
                Err(_) => {
                    warnings.push(format!("MCP server `{name}` timed out listing tools"));
                    continue;
                }
            };
            let index = hub.servers.len();
            for tool in tools {
                let exposed = hub.unique_name(&name, &tool.name);
                let description = format!(
                    "[MCP server `{name}` on the user's machine, not your sandbox] {}",
                    tool.description.as_deref().unwrap_or("")
                );
                let definition = json!({
                    "type": "function",
                    "name": exposed,
                    "description": description.trim_end(),
                    "parameters": flatten_schema(Value::Object((*tool.input_schema).clone())),
                });
                hub.order.push(exposed.clone());
                hub.tools.insert(
                    exposed,
                    Offered {
                        server: index,
                        tool: tool.name.to_string(),
                        definition,
                    },
                );
            }
            hub.servers.push(Server {
                name,
                client,
                _child: child,
            });
        }
        (hub, warnings)
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Function-tool definitions for the Responses request, in a stable order.
    pub fn definitions(&self) -> Vec<Value> {
        self.order
            .iter()
            .filter_map(|name| self.tools.get(name).map(|t| t.definition.clone()))
            .collect()
    }

    /// `(server, tool)` for an exposed name, if it is one of ours.
    pub fn describe(&self, exposed: &str) -> Option<(&str, &str)> {
        let offered = self.tools.get(exposed)?;
        let server = self.servers.get(offered.server)?;
        Some((server.name.as_str(), offered.tool.as_str()))
    }

    /// Call a tool. `Ok((text, is_error))`: an MCP-level error is still a result Viktor should see.
    pub async fn call(&self, exposed: &str, arguments: &str) -> Result<(String, bool), String> {
        let offered = self.tools.get(exposed).ok_or_else(|| {
            format!("error: `{exposed}` is not a tool of the editor's MCP servers")
        })?;
        let server = self
            .servers
            .get(offered.server)
            .ok_or_else(|| "error: the MCP server is gone".to_owned())?;
        let arguments = if arguments.trim().is_empty() {
            serde_json::Map::new()
        } else {
            match serde_json::from_str::<Value>(arguments) {
                Ok(Value::Object(map)) => map,
                Ok(_) => return Err("error: the arguments must be a JSON object".to_owned()),
                Err(e) => return Err(format!("error: the arguments are not valid JSON: {e}")),
            }
        };
        let request = rmcp::model::CallToolRequestParams::new(Cow::Owned(offered.tool.clone()))
            .with_arguments(arguments);
        let result = tokio::time::timeout(CALL_TIMEOUT, server.client.call_tool(request))
            .await
            .map_err(|_| {
                format!(
                    "error: the MCP tool did not finish within {} s",
                    CALL_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| format!("error: the MCP server failed the call: {e}"))?;
        let mut text = String::new();
        for block in &result.content {
            if let Some(t) = block.as_text() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&t.text);
            } else {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str("[non-text content omitted]");
            }
        }
        if text.is_empty()
            && let Some(structured) = &result.structured_content
        {
            text = structured.to_string();
        }
        if text.chars().count() > MAX_OUTPUT_CHARS {
            text = text.chars().take(MAX_OUTPUT_CHARS).collect();
            text.push_str("\n[truncated]");
        }
        Ok((text, result.is_error.unwrap_or(false)))
    }

    fn unique_name(&self, server: &str, tool: &str) -> String {
        let base = format!("{PREFIX}{}__{}", sanitize(server), sanitize(tool));
        let mut name: String = base.chars().take(MAX_NAME).collect();
        let mut n = 2;
        while self.tools.contains_key(&name) {
            let suffix = format!("_{n}");
            name = base
                .chars()
                .take(MAX_NAME - suffix.len())
                .chain(suffix.chars())
                .collect();
            n += 1;
        }
        name
    }
}

fn server_name(config: &acp::McpServer) -> Option<String> {
    match config {
        acp::McpServer::Stdio(s) => Some(s.name.clone()),
        acp::McpServer::Http(s) => Some(s.name.clone()),
        _ => None,
    }
}

type OwnedChild = (
    tokio::process::Child,
    std::sync::Arc<xai_tty_utils::ProcessGroup>,
);

async fn connect_one(
    config: &acp::McpServer,
    cwd: &Path,
    scope: &xai_tty_utils::ProcessScope,
) -> Result<(Client, Option<OwnedChild>), String> {
    match config {
        acp::McpServer::Stdio(stdio) => {
            let mut command = tokio::process::Command::new(&stdio.command);
            command
                .args(&stdio.args)
                .envs(
                    stdio
                        .env
                        .iter()
                        .map(|e| (e.name.as_str(), e.value.as_str())),
                )
                .current_dir(cwd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let (mut child, group) = scope
                .spawn(command)
                .map_err(|e| format!("{}: {e}", stdio.command.display()))?;
            let stdout = child.stdout.take().ok_or("no stdout")?;
            let stdin = child.stdin.take().ok_or("no stdin")?;
            let client =
                ().serve((stdout, stdin))
                    .await
                    .map_err(|e| format!("handshake failed: {e}"))?;
            Ok((client, Some((child, group))))
        }
        acp::McpServer::Http(http) => {
            let mut config =
                rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                    http.url.as_str(),
                );
            for header in &http.headers {
                let name = http::HeaderName::from_bytes(header.name.as_bytes())
                    .map_err(|e| format!("header {}: {e}", header.name))?;
                let value = http::HeaderValue::from_str(&header.value)
                    .map_err(|e| format!("header {}: {e}", header.name))?;
                config.custom_headers.insert(name, value);
            }
            let transport = rmcp::transport::StreamableHttpClientTransport::from_config(config);
            let client = ().serve(transport).await.map_err(|e| format!("handshake failed: {e}"))?;
            Ok((client, None))
        }
        _ => Err("only stdio and HTTP MCP servers are supported".to_owned()),
    }
}

/// Function names allow letters, digits, `_` and `-`.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "tool".to_owned()
    } else {
        cleaned
    }
}

/// The Viktor API rejects a top-level `oneOf`/`anyOf`/`allOf` in a tool schema, and
/// needs `type: object`. Keep the properties the combinators declare, drop the combinator.
fn flatten_schema(schema: Value) -> Value {
    let Value::Object(mut object) = schema else {
        return json!({"type": "object", "properties": {}});
    };
    let mut properties = object
        .remove("properties")
        .and_then(|p| p.as_object().cloned())
        .unwrap_or_default();
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(Value::Array(variants)) = object.remove(key) {
            for variant in variants {
                if let Some(props) = variant.get("properties").and_then(Value::as_object) {
                    for (k, v) in props {
                        properties.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
            // With the alternatives merged, no single variant's `required` holds for all.
            object.remove("required");
        }
    }
    object.insert("type".to_owned(), json!("object"));
    object.insert("properties".to_owned(), Value::Object(properties));
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_valid_function_names_and_stay_unique() {
        let mut hub = McpHub::default();
        let first = hub.unique_name("my server", "search.docs");
        assert_eq!(first, "mcp__my_server__search_docs");
        hub.tools.insert(
            first.clone(),
            Offered {
                server: 0,
                tool: "x".into(),
                definition: json!({}),
            },
        );
        assert_eq!(
            hub.unique_name("my server", "search docs"),
            "mcp__my_server__search_docs_2"
        );
        let long = hub.unique_name(&"s".repeat(80), "t");
        assert!(long.len() <= MAX_NAME);
    }

    #[test]
    fn a_combinator_schema_becomes_a_plain_object() {
        let flat = flatten_schema(json!({
            "oneOf": [
                {"properties": {"id": {"type": "string"}}, "required": ["id"]},
                {"properties": {"name": {"type": "string"}}},
            ],
        }));
        assert_eq!(flat.get("type"), Some(&json!("object")));
        assert!(flat.get("oneOf").is_none());
        assert!(flat.pointer("/properties/id").is_some());
        assert!(flat.pointer("/properties/name").is_some());
        assert_eq!(
            flatten_schema(json!("nonsense")).get("type"),
            Some(&json!("object"))
        );
    }
}
