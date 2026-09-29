//! Headless CLI parsing: output format, prompt sources, permission rules, and agent args.

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Plain,
    Json,
    /// NDJSON: one ACP session update per line, the agent's native format.
    #[value(name = "streaming-json")]
    StreamingJson,
    /// NDJSON in the Anthropic Messages API wire format.
    #[value(name = "streaming-messages-json")]
    StreamingMessagesJson,
}

pub fn parse_json_schema(input: &str) -> anyhow::Result<serde_json::Value> {
    let schema: serde_json::Value = serde_json::from_str(input)
        .map_err(|e| anyhow::anyhow!("--json-schema: invalid JSON: {e}"))?;
    if !schema.is_object() {
        anyhow::bail!("--json-schema: must be a JSON object describing a JSON Schema");
    }
    Ok(schema)
}

#[derive(Debug, Clone)]
pub enum HeadlessPrompt {
    Text(String),
    Blocks(Vec<acp::ContentBlock>),
}

impl HeadlessPrompt {
    /// Build from mutually-exclusive CLI prompt args. `None` means interactive mode.
    ///
    /// `stdin` is piped input (see [`read_piped_stdin`]): with `-p <text>` it is appended as a
    /// `<stdin>` block, and a bare `-p` takes it as the whole prompt.
    pub fn from_args(
        single: Option<&str>,
        prompt_json: Option<&str>,
        prompt_file: Option<&Path>,
        stdin: Option<&str>,
    ) -> anyhow::Result<Option<Self>> {
        if let Some(text) = single {
            let text = text.trim();
            let stdin = stdin.map(str::trim).filter(|s| !s.is_empty());
            let combined = match stdin {
                Some(input) if text.is_empty() || text == "-" => input.to_owned(),
                Some(input) => format!("{text}\n\n<stdin>\n{input}\n</stdin>"),
                None if text == "-" => {
                    anyhow::bail!(
                        "-p -: nothing was piped in; pipe the prompt, e.g. `echo hi | vktr -p`"
                    )
                }
                None => text.to_owned(),
            };
            if combined.is_empty() {
                anyhow::bail!(
                    "-p needs a prompt: `vktr -p \"question\"`, or pipe one in: `echo question | vktr -p`"
                );
            }
            Self::from_text(&combined)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("--single: {e}"))
        } else if let Some(json_str) = prompt_json {
            Self::from_json(json_str)
                .map(Some)
                .map_err(|e| anyhow::anyhow!("--prompt-json: {e}"))
        } else if let Some(path) = prompt_file {
            Self::from_file(path).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `.json` files are parsed as content blocks, everything else as text.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {e}", path.display()))?;

        let context = |e| anyhow::anyhow!("'{}': {e}", path.display());
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            Self::from_json(&content).map_err(context)
        } else {
            Self::from_text(&content).map_err(context)
        }
    }

    fn from_text(text: &str) -> anyhow::Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            anyhow::bail!("prompt is empty");
        }
        Ok(Self::Text(trimmed.to_string()))
    }

    fn from_json(json_str: &str) -> anyhow::Result<Self> {
        let blocks = parse_prompt_json(json_str)?;
        Ok(Self::Blocks(blocks))
    }

    pub fn into_content_blocks(self) -> Vec<acp::ContentBlock> {
        match self {
            Self::Text(text) => vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
            Self::Blocks(blocks) => blocks,
        }
    }
}

/// Parse ACP content blocks from an array (`[...]`) or typed wrapper (`{"type":"acp","content":[...]}`).
fn parse_prompt_json(json_str: &str) -> anyhow::Result<Vec<acp::ContentBlock>> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| anyhow::anyhow!("Invalid JSON: {e}"))?;

    let blocks: Vec<acp::ContentBlock> = match value {
        serde_json::Value::Array(_) => serde_json::from_value(value)
            .map_err(|e| anyhow::anyhow!("Invalid ACP content blocks: {e}"))?,

        serde_json::Value::Object(ref map) => {
            let format_type = map.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
                anyhow::anyhow!(
                    "JSON object must have a \"type\" field \
                         (e.g., {{\"type\": \"acp\", \"content\": [...]}})"
                )
            })?;
            let content = map
                .get("content")
                .ok_or_else(|| anyhow::anyhow!("JSON object must have a \"content\" field"))?;

            match format_type {
                "acp" => serde_json::from_value(content.clone()).map_err(|e| {
                    anyhow::anyhow!("Invalid ACP content blocks in \"content\": {e}")
                })?,
                other => anyhow::bail!(
                    "Unsupported prompt format type: \"{other}\". Supported types: \"acp\""
                ),
            }
        }

        _ => {
            anyhow::bail!("Expected JSON array or {{\"type\": \"...\", \"content\": [...]}} object")
        }
    };

    if blocks.is_empty() {
        anyhow::bail!("content blocks array is empty");
    }
    Ok(blocks)
}

/// Parse a comma-separated list into a vec, or None if empty.
pub(crate) fn parse_comma_list(s: Option<&str>) -> Option<Vec<String>> {
    s.and_then(|s| {
        let v: Vec<String> = s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if v.is_empty() { None } else { Some(v) }
    })
}

pub fn parse_permission_rules_strict(
    allow: &[String],
    deny: &[String],
) -> anyhow::Result<Vec<xai_grok_workspace::permission::types::PermissionRule>> {
    let (rules, errors) = parse_permission_rules_inner(allow, deny);
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .into_iter()
            .map(|(flag, rule, err)| format!("{flag} \"{rule}\": {err}"))
            .collect();
        anyhow::bail!("{}", msgs.join("; "));
    }
    Ok(rules)
}

pub fn parse_permission_rules_lenient(
    allow: &[String],
    deny: &[String],
) -> Vec<xai_grok_workspace::permission::types::PermissionRule> {
    let (rules, errors) = parse_permission_rules_inner(allow, deny);
    for (flag, rule, err) in errors {
        eprintln!("warning: {flag} \"{rule}\": {err}, skipping");
    }
    rules
}

// Deny before allow is cosmetic: the policy evaluator is order-independent (deny beats ask, ask beats allow)
pub(crate) fn parse_permission_rules_inner(
    allow: &[String],
    deny: &[String],
) -> (
    Vec<xai_grok_workspace::permission::types::PermissionRule>,
    Vec<(&'static str, String, String)>,
) {
    use xai_grok_workspace::permission::rules::parse_permission_rule;
    use xai_grok_workspace::permission::types::RuleAction;

    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for rule_str in deny {
        match parse_permission_rule(rule_str, RuleAction::Deny) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(("--deny", rule_str.clone(), e.to_string())),
        }
    }
    for rule_str in allow {
        match parse_permission_rule(rule_str, RuleAction::Allow) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(("--allow", rule_str.clone(), e.to_string())),
        }
    }
    (rules, errors)
}

pub(crate) enum ResolvedAgent {
    FilePath(PathBuf),
    Name(String),
}

pub(crate) fn resolve_agent_arg(agent: &str) -> ResolvedAgent {
    let path = std::path::Path::new(agent);
    if path.exists() && path.is_file() {
        ResolvedAgent::FilePath(dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
    } else {
        ResolvedAgent::Name(agent.to_string())
    }
}

pub(crate) fn parse_cli_agents(
    json: &str,
) -> anyhow::Result<Vec<xai_grok_shell::agent::config::AgentDefinition>> {
    let map: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(json).map_err(|e| anyhow::anyhow!("--agents: invalid JSON: {e}"))?;
    let mut agents = Vec::with_capacity(map.len());
    for (name, mut value) in map {
        if let serde_json::Value::Object(ref mut obj) = value {
            if !obj.contains_key("promptBody")
                && let Some(prompt) = obj.remove("prompt")
            {
                obj.insert("promptBody".to_string(), prompt);
            }
            obj.entry("name".to_string())
                .or_insert_with(|| serde_json::Value::String(name.clone()));
            obj.entry("description".to_string())
                .or_insert_with(|| serde_json::Value::String(name.clone()));
        }
        let mut def = xai_grok_shell::agent::config::AgentDefinition::from_json(&value)
            .map_err(|e| anyhow::anyhow!("--agents: failed to parse '{name}': {e}"))?;
        def.name = name;
        agents.push(def);
    }
    Ok(agents)
}

pub(crate) fn apply_agent_flag(
    agent: &Option<String>,
    config: &mut xai_grok_shell::agent::config::Config,
) {
    if let Some(agent) = agent {
        match resolve_agent_arg(agent) {
            ResolvedAgent::FilePath(path) => config.agent_profile_path = Some(path),
            ResolvedAgent::Name(name) => config.agent.name = Some(name),
        }
    }
}

/// Largest piped input sent to Viktor; the rest is cut with a note.
const MAX_STDIN_BYTES: usize = 512 * 1024;

/// How long `-p` waits for piped input to start before ignoring stdin.
const STDIN_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Read piped input for `-p`. Only a pipe or a regular file is read (`cmd | vktr -p`,
/// `vktr -p < file`); a terminal, `/dev/null` or a socket never is. A pipe that stays silent for
/// the first second is left alone, so a script whose stdin is an idle inherited pipe does not
/// hang.
pub fn read_piped_stdin() -> Option<String> {
    read_piped(libc::STDIN_FILENO, STDIN_GRACE)
}

fn read_piped(fd: libc::c_int, grace: std::time::Duration) -> Option<String> {
    use std::io::Read as _;
    // SAFETY: fstat on an fd we own for the life of the process, into a zeroed struct.
    let mode = unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &mut st) != 0 {
            return None;
        }
        st.st_mode & libc::S_IFMT
    };
    if mode != libc::S_IFIFO && mode != libc::S_IFREG {
        return None;
    }
    if mode == libc::S_IFIFO {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = libc::c_int::try_from(grace.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut pfd, 1, ms) };
        if ready <= 0 {
            return None;
        }
    }
    // SAFETY: borrow the fd without taking ownership; ManuallyDrop keeps it open.
    let file = std::mem::ManuallyDrop::new(unsafe {
        <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd)
    });
    let mut bytes = Vec::new();
    (&*file)
        .take((MAX_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let cut = bytes.len() > MAX_STDIN_BYTES;
    bytes.truncate(MAX_STDIN_BYTES);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut {
        text.push_str(&format!(
            "\n[vktr: input cut at {} KiB]",
            MAX_STDIN_BYTES / 1024
        ));
    }
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(all(test, unix))]
mod piped_stdin_tests {
    use super::*;
    use std::io::Write as _;
    use std::os::fd::AsRawFd as _;

    #[test]
    fn prompt_and_piped_input_combine() {
        let p = |single, stdin| match HeadlessPrompt::from_args(Some(single), None, None, stdin) {
            Ok(Some(HeadlessPrompt::Text(t))) => t,
            other => panic!("{:?}", other.map(|_| ())),
        };
        assert_eq!(
            p("explain", Some("E42\n")),
            "explain\n\n<stdin>\nE42\n</stdin>"
        );
        assert_eq!(p("", Some("just this")), "just this");
        assert_eq!(p("-", Some("just this")), "just this");
        assert_eq!(p("plain", None), "plain");
        assert!(HeadlessPrompt::from_args(Some(""), None, None, None).is_err());
        assert!(HeadlessPrompt::from_args(Some("-"), None, None, None).is_err());
    }

    #[test]
    fn a_regular_file_is_read_and_an_idle_pipe_is_not_waited_on() {
        let mut file = tempfile::tempfile().expect("tempfile");
        file.write_all(b"from a file").expect("write");
        use std::io::Seek as _;
        file.rewind().expect("rewind");
        assert_eq!(
            read_piped(file.as_raw_fd(), STDIN_GRACE).as_deref(),
            Some("from a file")
        );

        let (reader, _writer) = std::io::pipe().expect("pipe");
        let started = std::time::Instant::now();
        assert_eq!(
            read_piped(reader.as_raw_fd(), std::time::Duration::from_millis(100)),
            None
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        let devnull = std::fs::File::open("/dev/null").expect("devnull");
        assert_eq!(read_piped(devnull.as_raw_fd(), STDIN_GRACE), None);
    }

    #[test]
    fn huge_input_is_cut_with_a_note() {
        let (reader, mut writer) = std::io::pipe().expect("pipe");
        let feeder = std::thread::spawn(move || {
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..12 {
                if writer.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let text = read_piped(reader.as_raw_fd(), STDIN_GRACE).expect("text");
        drop(reader);
        feeder.join().ok();
        assert!(text.ends_with("[vktr: input cut at 512 KiB]"));
    }
}
