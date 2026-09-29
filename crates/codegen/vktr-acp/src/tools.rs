//! Editor-side tools Viktor can call during an ACP session.
//!
//! Viktor works in its own cloud sandbox, so on its own it cannot see the files open in the
//! editor. When the editor offers them, `vktr acp` exposes the editor's own ACP capabilities to
//! Viktor as caller tools over the compat API: reading a file (`fs/read_text_file`), writing one
//! (`fs/write_text_file`) and running a command in an editor terminal (`terminal/create`). The
//! calls come back on the Responses stream, run through the editor, and their results go back
//! to Viktor on the same thread, as for any other caller tool.
//!
//! The names carry an `editor_` prefix so Viktor never confuses them with the tools of its own
//! sandbox, and a caller tool can never shadow one of Viktor's (a caller name wins on collision).

use std::path::{Path, PathBuf};

use agent_client_protocol as acp;
use serde_json::{Value, json};

pub const READ_FILE: &str = "editor_read_file";
pub const WRITE_FILE: &str = "editor_write_file";
pub const RUN_COMMAND: &str = "editor_run_command";

/// Longest file text sent back to Viktor from one read.
pub const MAX_READ_CHARS: usize = 200_000;

/// Terminal output the editor keeps for one command (ACP truncates from the start).
pub const COMMAND_OUTPUT_LIMIT: u64 = 64 * 1024;

/// Which editor tools a session offers, from the client's capabilities.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorTools {
    pub read: bool,
    pub write: bool,
    pub terminal: bool,
}

impl EditorTools {
    pub fn from_capabilities(capabilities: &acp::ClientCapabilities) -> Self {
        Self {
            read: capabilities.fs.read_text_file,
            write: capabilities.fs.write_text_file,
            terminal: capabilities.terminal,
        }
    }

    /// Responses API function-tool definitions for the tools this editor supports.
    pub fn definitions(self, cwd: &Path) -> Vec<Value> {
        let root = cwd.display();
        let mut tools = Vec::new();
        if self.read {
            tools.push(function(
                READ_FILE,
                &format!(
                    "Read a text file from the user's local workspace, open in their editor \
                     (not your own sandbox). The workspace root is {root}; a relative path is \
                     resolved against it. Returns the file's text, including unsaved editor \
                     changes."
                ),
                json!({
                    "path": {"type": "string", "description": "Absolute path, or relative to the workspace root"},
                    "line": {"type": "integer", "minimum": 1, "description": "1-based line to start reading at"},
                    "limit": {"type": "integer", "minimum": 1, "description": "Maximum number of lines to read"},
                }),
                &["path"],
            ));
        }
        if self.write {
            tools.push(function(
                WRITE_FILE,
                &format!(
                    "Create or overwrite a text file in the user's local workspace, open in their \
                     editor (not your own sandbox). The workspace root is {root}; a relative path \
                     is resolved against it. The user is asked to approve every write. Send the \
                     complete new file content."
                ),
                json!({
                    "path": {"type": "string", "description": "Absolute path, or relative to the workspace root"},
                    "content": {"type": "string", "description": "The complete new file content"},
                }),
                &["path", "content"],
            ));
        }
        if self.terminal {
            tools.push(function(
                RUN_COMMAND,
                &format!(
                    "Run a shell command on the user's machine, in a terminal of their editor \
                     (not your own sandbox), and return its exit status and output. Runs with \
                     `sh -c` in the workspace root {root} unless cwd is given. The user is asked \
                     to approve every command. Output is capped at the last 64 KiB."
                ),
                json!({
                    "command": {"type": "string", "description": "The shell command line"},
                    "cwd": {"type": "string", "description": "Working directory: absolute, or relative to the workspace root"},
                }),
                &["command"],
            ));
        }
        tools
    }
}

fn function(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

/// A tool call Viktor asked for, as it arrived on the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

impl FunctionCall {
    /// Read a `function_call` output item, if `item` is one.
    pub fn from_item(item: &Value) -> Option<Self> {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return None;
        }
        Some(Self {
            call_id: item.get("call_id").and_then(Value::as_str)?.to_owned(),
            name: item.get("name").and_then(Value::as_str)?.to_owned(),
            arguments: item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

/// A parsed, validated editor tool request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Read {
        path: PathBuf,
        line: Option<u32>,
        limit: Option<u32>,
        /// The path is not under the workspace root, so the user is asked first.
        outside: bool,
    },
    Write {
        path: PathBuf,
        content: String,
    },
    Run {
        command: String,
        cwd: PathBuf,
    },
}

impl Request {
    /// Parse `call` against the tools this session offers. The error is sent back to Viktor as
    /// the tool's output, so it says what to fix.
    pub fn parse(call: &FunctionCall, offered: EditorTools, cwd: &Path) -> Result<Self, String> {
        let args: Value = if call.arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&call.arguments)
                .map_err(|e| format!("error: the arguments are not valid JSON: {e}"))?
        };
        let string = |key: &str| args.get(key).and_then(Value::as_str);
        let number = |key: &str| {
            args.get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
        };
        let required = |key: &str| {
            string(key)
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| format!("error: `{key}` is required"))
        };
        match call.name.as_str() {
            READ_FILE if offered.read => {
                let path = resolve(cwd, required("path")?);
                Ok(Self::Read {
                    outside: !path.starts_with(normalize(cwd)),
                    path,
                    line: number("line"),
                    limit: number("limit"),
                })
            }
            WRITE_FILE if offered.write => Ok(Self::Write {
                path: resolve(cwd, required("path")?),
                content: string("content")
                    .ok_or("error: `content` is required")?
                    .to_owned(),
            }),
            RUN_COMMAND if offered.terminal => Ok(Self::Run {
                command: required("command")?.to_owned(),
                cwd: string("cwd").map_or_else(|| cwd.to_path_buf(), |dir| resolve(cwd, dir)),
            }),
            other => Err(format!("error: `{other}` is not a tool this editor offers")),
        }
    }

    /// Reads inside the workspace run freely; everything else asks the user first.
    pub fn needs_permission(&self) -> bool {
        match self {
            Self::Read { outside, .. } => *outside,
            Self::Write { .. } | Self::Run { .. } => true,
        }
    }

    /// What an "always allow" answer covers: a tool, with reads outside the workspace kept
    /// apart from the ones that never ask.
    pub fn permission_key(&self) -> &'static str {
        match self {
            Self::Read { .. } => "editor_read_file outside the workspace",
            Self::Write { .. } => WRITE_FILE,
            Self::Run { .. } => RUN_COMMAND,
        }
    }

    pub fn kind(&self) -> acp::ToolKind {
        match self {
            Self::Read { .. } => acp::ToolKind::Read,
            Self::Write { .. } => acp::ToolKind::Edit,
            Self::Run { .. } => acp::ToolKind::Execute,
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::Read {
                path,
                outside: true,
                ..
            } => format!("Read {} (outside the workspace)", path.display()),
            Self::Read { path, .. } => format!("Read {}", path.display()),
            Self::Write { path, .. } => format!("Write {}", path.display()),
            Self::Run { command, .. } => format!("Run `{}`", one_line(command, 120)),
        }
    }

    pub fn locations(&self) -> Vec<acp::ToolCallLocation> {
        match self {
            Self::Read { path, line, .. } => {
                vec![acp::ToolCallLocation::new(path.clone()).line(*line)]
            }
            Self::Write { path, .. } => vec![acp::ToolCallLocation::new(path.clone())],
            Self::Run { .. } => Vec::new(),
        }
    }
}

/// Resolve a path Viktor gave against the workspace root and collapse `.` and `..`, so the path
/// the user is shown (and the workspace check) is the path that is actually touched. ACP file
/// methods need absolute paths.
pub fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path.trim());
    normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    })
}

/// Lexical normalisation: no file-system access, so it works for files that do not exist yet.
/// `..` never climbs above the root.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Cap a file's text for Viktor, saying so when it was cut.
pub fn cap_read(text: String) -> String {
    if text.chars().count() <= MAX_READ_CHARS {
        return text;
    }
    let mut capped: String = text.chars().take(MAX_READ_CHARS).collect();
    capped.push_str(&format!(
        "\n\n[truncated: only the first {MAX_READ_CHARS} characters are shown; read a line range \
         with `line` and `limit` for the rest]"
    ));
    capped
}

/// What Viktor is told about a finished command.
pub fn command_output(exit: &acp::TerminalExitStatus, output: &str, truncated: bool) -> String {
    let status = match (&exit.exit_code, &exit.signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "exit status unknown".to_owned(),
    };
    let note = if truncated {
        " (output truncated to its last 64 KiB)"
    } else {
        ""
    };
    format!("{status}{note}\n{output}")
}

fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    let mut short: String = line.chars().take(max).collect();
    if short.len() < text.len() {
        short.push('…');
    }
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: Value) -> FunctionCall {
        FunctionCall {
            call_id: "c1".to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_string(),
        }
    }

    const ALL: EditorTools = EditorTools {
        read: true,
        write: true,
        terminal: true,
    };

    #[test]
    fn only_the_capabilities_the_editor_offers_become_tools() {
        let caps = acp::ClientCapabilities::new()
            .fs(acp::FileSystemCapabilities::new().read_text_file(true));
        let offered = EditorTools::from_capabilities(&caps);
        let names: Vec<_> = offered
            .definitions(Path::new("/w"))
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect();
        assert_eq!(names, [READ_FILE]);
        assert_eq!(
            EditorTools::from_capabilities(&acp::ClientCapabilities::new()),
            EditorTools::default()
        );
    }

    #[test]
    fn every_definition_is_a_flat_object_schema_viktor_accepts() {
        // The Viktor API rejects a top-level oneOf/anyOf/allOf in a tool schema.
        for tool in ALL.definitions(Path::new("/w")) {
            let parameters = tool.get("parameters").expect("parameters");
            assert_eq!(parameters.get("type"), Some(&json!("object")));
            for combinator in ["oneOf", "anyOf", "allOf"] {
                assert!(parameters.get(combinator).is_none());
            }
            assert!(
                tool.get("description")
                    .and_then(Value::as_str)
                    .is_some_and(|d| d.contains("/w")),
                "the description names the workspace root"
            );
        }
    }

    #[test]
    fn relative_paths_resolve_against_the_workspace_root() {
        let cwd = Path::new("/work/project");
        assert_eq!(
            Request::parse(
                &call(READ_FILE, json!({"path": "src/main.rs", "line": 3})),
                ALL,
                cwd
            ),
            Ok(Request::Read {
                path: PathBuf::from("/work/project/src/main.rs"),
                line: Some(3),
                limit: None,
                outside: false,
            })
        );
        assert_eq!(
            Request::parse(&call(RUN_COMMAND, json!({"command": "ls"})), ALL, cwd),
            Ok(Request::Run {
                command: "ls".to_owned(),
                cwd: PathBuf::from("/work/project"),
            })
        );
        assert_eq!(
            Request::parse(
                &call(WRITE_FILE, json!({"path": "/abs/x.txt", "content": ""})),
                ALL,
                cwd
            ),
            Ok(Request::Write {
                path: PathBuf::from("/abs/x.txt"),
                content: String::new(),
            })
        );
    }

    #[test]
    fn bad_calls_become_errors_viktor_can_act_on() {
        let cwd = Path::new("/w");
        let read_only = EditorTools {
            read: true,
            ..EditorTools::default()
        };
        for (c, expected) in [
            (call(READ_FILE, json!({})), "`path` is required"),
            (
                call(WRITE_FILE, json!({"path": "a"})),
                "not a tool this editor offers",
            ),
            (call("bash", json!({})), "not a tool this editor offers"),
        ] {
            let error = Request::parse(&c, read_only, cwd).expect_err("rejected");
            assert!(error.contains(expected), "{error}");
        }
        let broken = FunctionCall {
            call_id: "c".to_owned(),
            name: READ_FILE.to_owned(),
            arguments: "{not json".to_owned(),
        };
        assert!(
            Request::parse(&broken, ALL, cwd)
                .expect_err("rejected")
                .contains("not valid JSON")
        );
    }

    #[test]
    fn dot_dot_is_collapsed_and_a_read_that_leaves_the_workspace_asks_first() {
        let cwd = Path::new("/work/project");
        let escape = Request::parse(
            &call(READ_FILE, json!({"path": "../../home/u/.ssh/id_ed25519"})),
            ALL,
            cwd,
        )
        .expect("ok");
        assert_eq!(
            escape,
            Request::Read {
                path: PathBuf::from("/home/u/.ssh/id_ed25519"),
                line: None,
                limit: None,
                outside: true,
            }
        );
        assert!(escape.needs_permission());
        assert!(escape.title().ends_with("(outside the workspace)"));
        let absolute =
            Request::parse(&call(READ_FILE, json!({"path": "/etc/passwd"})), ALL, cwd).expect("ok");
        assert!(absolute.needs_permission());
        let inside = Request::parse(
            &call(READ_FILE, json!({"path": "./src/../README.md"})),
            ALL,
            cwd,
        )
        .expect("ok");
        assert!(!inside.needs_permission());
        assert_eq!(inside.title(), "Read /work/project/README.md");
        // A sibling directory that shares the root's name as a prefix is still outside.
        let sibling = Request::parse(
            &call(READ_FILE, json!({"path": "/work/project2/x"})),
            ALL,
            cwd,
        )
        .expect("ok");
        assert!(sibling.needs_permission());
        let write = Request::parse(
            &call(
                WRITE_FILE,
                json!({"path": "a/../../../.bashrc", "content": ""}),
            ),
            ALL,
            cwd,
        )
        .expect("ok");
        assert_eq!(write.title(), "Write /.bashrc");
    }

    #[test]
    fn only_reads_run_without_asking_the_user() {
        let cwd = Path::new("/w");
        let read = Request::parse(&call(READ_FILE, json!({"path": "a"})), ALL, cwd).expect("ok");
        let write = Request::parse(
            &call(WRITE_FILE, json!({"path": "a", "content": "x"})),
            ALL,
            cwd,
        )
        .expect("ok");
        let run = Request::parse(&call(RUN_COMMAND, json!({"command": "rm -rf x"})), ALL, cwd)
            .expect("ok");
        assert!(!read.needs_permission());
        assert!(write.needs_permission());
        assert!(run.needs_permission());
    }

    #[test]
    fn a_function_call_item_is_read_from_the_stream_shape_viktor_sends() {
        // Captured from the live compat API on 2026-09-23.
        let item = json!({
            "type": "function_call",
            "id": "call_vk1_x",
            "call_id": "call_vk1_x",
            "name": "editor_read_text_file",
            "arguments": "{\"path\":\"/home/dev/project/README.md\"}",
            "status": "completed",
        });
        assert_eq!(
            FunctionCall::from_item(&item),
            Some(FunctionCall {
                call_id: "call_vk1_x".to_owned(),
                name: "editor_read_text_file".to_owned(),
                arguments: "{\"path\":\"/home/dev/project/README.md\"}".to_owned(),
            })
        );
        assert_eq!(FunctionCall::from_item(&json!({"type": "message"})), None);
    }

    #[test]
    fn command_output_names_the_exit_status_and_truncation() {
        let exit = acp::TerminalExitStatus::new().exit_code(Some(2));
        assert_eq!(
            command_output(&exit, "boom", true),
            "exit code 2 (output truncated to its last 64 KiB)\nboom"
        );
        let killed = acp::TerminalExitStatus::new().signal(Some("SIGKILL".to_owned()));
        assert!(command_output(&killed, "", false).starts_with("killed by signal SIGKILL"));
    }

    #[test]
    fn a_huge_file_is_capped_with_a_note() {
        let text = "x".repeat(MAX_READ_CHARS + 5);
        let capped = cap_read(text);
        assert!(capped.contains("[truncated"));
        assert_eq!(cap_read("small".to_owned()), "small");
    }
}
