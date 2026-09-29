# Agent mode (ACP) and IDE integration

vktr speaks the [Agent Client Protocol](https://agentclientprotocol.com) (ACP, JSON-RPC over stdio) in two different ways:

| Command | What the editor gets |
|---------|---------------------|
| `vktr acp` | **Viktor**, working in its own cloud sandbox with your team's tools, reaching your machine only through the editor. Use this to put Viktor in Zed or a JetBrains IDE. |
| `vktr agent stdio` | **vktr's own local coding agent** (the same one as the TUI), running tools on your machine. |

For a one-shot prompt that prints and exits, use `vktr -p` instead ([headless mode](14-headless-mode.md)). To point another coding tool at Viktor or a local model, see [`vktr launch`](#vktr-launch).

---

## `vktr acp`: Viktor in your editor

`vktr acp` serves Viktor to an editor over ACP on stdio. It replaces the standalone TypeScript `viktor-acp` package, which is deprecated.

### Set up

It needs a Viktor API key: `VIKTOR_API_KEY`, or the key saved by `vktr login` (used when the environment variable is not set, so an editor started from the desktop still works).

`vktr acp --print-config zed` (or `jetbrains`) prints the editor snippet with the full path of your `vktr`, ready to paste. It does not touch editor files.

**Zed** (`settings.json`):

```json
{
  "agent_servers": {
    "Viktor": {
      "type": "custom",
      "command": "vktr",
      "args": ["acp"]
    }
  }
}
```

Add `"env": { "VIKTOR_API_KEY": "zt_live_sk_..." }` if you have not run `vktr login` and Zed does not inherit your shell environment. Use the absolute path (`~/.local/bin/vktr`) if Zed cannot find `vktr` on its `PATH`.

**JetBrains IDEs** (`~/.jetbrains/acp.json`):

```json
{
  "agent_servers": {
    "Viktor": { "command": "vktr", "args": ["acp"] }
  }
}
```

Any other ACP client: run `vktr acp` as the agent command. stdout carries only the protocol; diagnostics go to stderr.

### What it does

- **One editor session is one Viktor thread.** Follow-up prompts keep Viktor's context and sandbox state.
- **Editor tools.** When the editor offers file-system or terminal capabilities, Viktor gets `editor_read_file`, `editor_write_file`, and `editor_run_command`, which run through the editor. Writes and commands always ask first (allow once, always allow in this session, or reject), with the diff for writes. Reads inside the workspace run without asking; reads outside it ask. Paths are resolved against the workspace root with `.` and `..` collapsed, so the path you approve is the path touched. `VKTR_ACP_EDITOR_TOOLS=0` turns editor tools off.
- **The editor's MCP servers.** Servers the editor lists in `session/new` (stdio and HTTP) are connected on your machine, and their tools are offered to Viktor as `mcp__<server>__<tool>`. Every call asks first. stdio servers run in the workspace and die with the session. A server that fails to start is named in the first turn. `VKTR_ACP_MCP=0` turns this off.
- **Sessions survive restarts.** Each session is saved to `~/.vktr/acp/sessions/<id>.json` (owner-only): the Viktor thread, workspace, title, and the last 400 transcript entries. Editors can list (`session/list`), reload with the transcript replayed (`session/load`), or reattach (`session/resume`); the next prompt continues the same Viktor thread.
- **Prompt content:** text, images, embedded text resources, and resource links. Audio and embedded binary blobs are dropped.
- **Cancel, then prompt again.** Viktor needs about 15 seconds to stop a cancelled run; the next prompt waits (up to two minutes, with a thought chunk saying why) instead of failing.
- **Failed runs** come back as a JSON-RPC error carrying Viktor's own message, error class, and request id, never as assistant text.

Viktor's own tools run in its sandbox and are not shown as tool calls, because the compat API does not report their names yet.

### Troubleshooting

`vktr acp` writes two lines to stderr on start: whether it found a key, and which endpoint it serves. Most editors show an agent's stderr in a log panel.

- **"no Viktor API key"**: the handshake still succeeds, but prompts fail. Set `VIKTOR_API_KEY` or run `vktr login`.
- **Auth errors** carry Viktor's message plus a hint (wrong or expired key, missing `chat:completions` scope, no linked chat identity).
- **`Could not reach Viktor at …`**: check `VIKTOR_BASE_URL`.
- **Viktor says it cannot see your files**: the editor did not offer file-system capabilities, or `VKTR_ACP_EDITOR_TOOLS=0` is set.

`vktr doctor` shows how many `vktr acp` sessions are saved and whether editor tools are on.

---

## `vktr launch`

`vktr launch` points another installed coding tool at a local OpenAI-compatible backend, or at Viktor, without editing that tool's provider config:

```bash
vktr launch codex                                  # first reachable local backend, discovered model
vktr launch --backend ollama --model qwen3.5:4b opencode
vktr launch --backend http://host:8000 pi -p "explain this repo"
vktr launch --config opencode                      # print the wiring (secrets masked), start nothing
vktr launch --viktor claude                        # Claude Code against the Viktor compat API
```

Supported tools: `claude`, `codex`, `copilot`, `grok` (xAI's own CLI), `hermes`, `mi`, `opencode`, `pi`, `pool`, `vktr`. Launch options go before the tool name; everything after it is passed to the tool unchanged, and the tool runs from the current directory.

- Without `--backend`, vktr probes the usual local ports (Ollama, llama.cpp, LM Studio, vLLM, SGLang, TabbyAPI, mistral.rs, Docker Model Runner, MLX, LiteLLM) and picks the first reachable one. Without `--model`, it uses the first non-embedding model the backend lists.
- `--viktor` uses `VIKTOR_API_KEY` or the key saved by `vktr login`. With `claude`, every Claude Code model slot is routed to Viktor, and vktr warns that each request becomes a Viktor run.
- `claude` is refused for backends that do not speak the Anthropic Messages API (anything but Ollama and Viktor).
- If [Harbor](https://github.com/av/harbor) is installed, local launches (other than `vktr` itself) are handed to `harbor launch`, which can also start a backend. Without Harbor, vktr cannot start a backend; it lists the endpoints it probed and says how to start one.

---

## `vktr agent stdio`: the local agent over ACP

The rest of this page covers `vktr agent`, which runs vktr's own agent as a long-lived ACP server for SDKs, eval harnesses, and custom apps. Its model is whatever vktr is configured with (Viktor by default).

## Automation and SDKs

For scripts, CI, evals, and agent servers, start with always-approve so tools run without interactive permission prompts. Deny rules and hooks still apply.

```bash
# stdio (local process / many SDKs)
vktr agent --always-approve stdio

# WebSocket server
vktr agent --always-approve serve --bind 127.0.0.1:2419 --secret <token>
```

You can also set always-approve per session on `session/new`:

```json
{
  "cwd": "/path/to/project",
  "mcpServers": [],
  "_meta": { "yoloMode": true }
}
```

Interactive TUI users typically leave the default ask mode (or use auto). See [Permissions and safety](22-permissions-and-safety.md).

---

## What is ACP?

The [Agent Client Protocol (ACP)](https://agentclientprotocol.com) defines how clients talk to coding agents over JSON-RPC. With vktr it covers:

- Sessions (create, load, resume)
- Prompts and streamed replies
- Tool call updates
- Reasoning / thought streams
- Permission prompts when the session is not always-approve

---

## stdio transport

stdio is the common local integration path. The agent speaks JSON-RPC on stdin and stdout:

```bash
vktr agent --always-approve stdio
```

Typical clients: IDE extensions (Zed, Neovim, Emacs), custom tools, and ACP SDKs.

### Options

Agent options apply to every transport (`stdio`, `serve`, `headless`, `leader`). They go after `agent` and before the mode name. Mode-specific flags go after the mode (for example `serve --bind`).

```bash
vktr agent --always-approve --model viktor stdio
vktr agent --always-approve serve --bind 127.0.0.1:2419 --secret <token>
```

| Flag | Description |
| ---- | ----------- |
| `-m, --model <MODEL>` | Model ID (for example `viktor`). |
| `--always-approve` | Run without interactive tool-permission prompts. Alias: `--yolo`. |
| `--agent-profile <PATH>` | Load an agent profile from a file. |
| `--leader` / `--no-leader` | Connect to a shared leader process, or force a local agent. When a non-`off` sandbox profile is requested, leader mode is refused so tools stay in-process (see [Sandbox Mode](18-sandbox.md)). |

---

## Server mode

```bash
vktr agent --always-approve serve --bind 127.0.0.1:2419 --secret <token>
```

Clients connect over WebSocket and authenticate with the secret token. If you omit `--secret`, the agent prints a generated token at startup, or set `VKTR_AGENT_SECRET`. The process keeps state across client reconnects. Permissions match other entry points; see [Permissions and safety](22-permissions-and-safety.md).

This is a server you run yourself.

---

## WebSocket relay

`vktr agent headless` is upstream's relay mode. Its default relay is an xAI service, and vktr provides no relay of its own, so this mode is not supported in vktr. Use `stdio` or `serve`.

---

## ACP protocol basics

Communication follows the JSON-RPC 2.0 format. A typical session lifecycle:

1. **Initialize** -- client sends `initialize` with capabilities
2. **Create session** -- client sends `session/new` with working directory
3. **Send prompts** -- client sends `session/prompt` with user messages
4. **Receive updates** -- agent sends `session/update` notifications with streamed content
5. **Handle permissions** -- agent may request tool execution approval (or allow or deny based on permission mode)

### Architecture

```
+------------------------------------------+
|           ACP Client                     |
|  (IDE, Editor, Custom Application)       |
+-------------------+----------------------+
                    | JSON-RPC over stdio
+-------------------v----------------------+
|           vktr agent stdio               |
|                                          |
|  +---------+  +---------+  +---------+   |
|  | Session |  |  Tools  |  |   MCP   |   |
|  | Manager |  | Registry|  | Servers |   |
|  +---------+  +---------+  +---------+   |
+------------------------------------------+
```

---

## Streaming updates

ACP streams structured events. Each `session/update` notification carries a `sessionUpdate` field that identifies the update type:

| `sessionUpdate` value | Description                                            |
| --------------------- | ----------------------------------------------------- |
| `agent_message_chunk` | A chunk of the agent's response text.                 |
| `agent_thought_chunk` | A chunk of the agent's internal reasoning.            |
| `tool_call`           | A new tool invocation (title, kind, status, input).   |
| `tool_call_update`    | A status or result update for an in-flight tool call. |
| `plan`                | The agent's execution plan.                           |

Each update names its type, so a client can render distinct panels for reasoning, tool calls, and response text.

---

## Extension methods

Beyond the base ACP protocol, `vktr agent` keeps upstream's extension methods. Their names start with the prefix `x.ai/` (a protocol namespace inherited from upstream, not a network address). Relative to that prefix:

| Category                   | Methods                                           |
| -------------------------- | ------------------------------------------------- |
| **Filesystem**             | `fs/list`, `fs/exists`, `fs/read_file`, `fs/write_file` |
| **Git**                    | `git/status`, `git/stage`, `git/commit`, `git/diffs`, `git/discard` |
| **Git Worktree**           | `git/worktree/create`, `remove`, `apply`, `list`, `gc` |
| **Search**                 | `search/fuzzy/open`, `search/fuzzy/change`, `search/content` |
| **Terminal**               | `terminal/create`, `kill`, `output`, `wait_for_exit` |
| **Session Management**     | `session/fork`, `session/resolve_local_for_worktree_resume` |
| **Conversation & History** | `prompt_history`, `rewind/*`, `compact_conversation` |

These are representative, not exhaustive; discover the available methods from the agent's `initialize` response. `vktr acp` does not use them.

### Notifications (agent to client)

| Notification (after the prefix) | Description                          |
| ------------------------------- | ------------------------------------ |
| `search/fuzzy/status`           | Fuzzy search results update          |
| `git/worktree/status`           | Worktree creation progress           |
| `fs_notify`                     | Filesystem change notification       |
| `fs/index`                      | Full file index update               |
| `fs/index/delta`                | Incremental file index update        |
| `session_notification`          | Session-specific updates (diff review, retry state, auto-compact) |
| `session/update`                | Session update (tool calls, content) |

---

## Session config options

`session/new` and `session/load` responses include a typed `configOptions` list (standard ACP, not an extension). Change a live option with `session/set_config_option`.

| `configId` | Category | Effect |
|------------|----------|--------|
| `model` | `model` | Switches the session model (subject to `allowed_models`). Value must be a string id. |
| `reasoning_effort` | `thought_level` | Applies effort to the current model without changing the model (no prompt rewrite, no `allowed_models` gate). Value must be a string id (`minimal`, `low`, `medium`, `high`, `xhigh`). Dropped with a warning when the model does not advertise `supportsReasoningEffort`. |

```json
{
  "sessionId": "…",
  "configId": "reasoning_effort",
  "value": { "value": "high" }
}
```

The response is the **complete, updated** option list. A `config_option_update` session notification mirrors it to every subscribed client. In leader mode the proxy snoops `configId: model` so each client's `default_model` stays in sync. Boolean values are rejected; exposing boolean options is not implemented yet.

---

## Session `_meta` options

Optional fields on `session/new`:

| Field | Description |
| ----- | ----------- |
| `rules` | Extra rules appended to the system prompt. |
| `systemPromptOverride` | Replacement system prompt. |
| `agentProfile` | Agent profile name or JSON object. |
| `yoloMode` | When `true`, always-approve for this session. |
| `autoMode` | When `true`, auto permission mode for this session. Superseded when always-approve is already on. |

```json
{
  "cwd": "/path/to/project",
  "mcpServers": [],
  "_meta": { "yoloMode": true }
}
```

---

## ACP SDKs

Official SDK libraries are available for multiple languages:

| Language   | Package                                                                                  |
| ---------- | ---------------------------------------------------------------------------------------- |
| TypeScript | [`@agentclientprotocol/sdk`](https://www.npmjs.com/package/@agentclientprotocol/sdk)     |
| Rust       | [`agent-client-protocol`](https://crates.io/crates/agent-client-protocol)                |
| Python     | [`agent-client-protocol-python`](https://github.com/PsiACE/agent-client-protocol-python) |
| Go         | [`acp-go-sdk`](https://github.com/coder/acp-go-sdk)                                     |
| Kotlin     | [`acp`](https://github.com/agentclientprotocol/kotlin-sdk)                               |

---

## Compatible clients

For Viktor in an editor, use `vktr acp` with Zed or a JetBrains IDE ([set up](#set-up)). Other ACP clients (for example Neovim plugins such as CodeCompanion or avante.nvim, Emacs agent-shell, marimo) can run either `vktr acp` or `vktr agent stdio` as their agent command.

---

## Integration example: a TypeScript ACP client

```typescript
import { spawn, ChildProcess } from "child_process";
import * as readline from "readline";

class VktrACPChat {
  private proc!: ChildProcess;
  private sessionId!: string;
  private rl!: readline.Interface;

  constructor(private cwd = ".") {}

  async init() {
    this.proc = spawn("vktr", ["agent", "--always-approve", "stdio"]);
    this.rl = readline.createInterface({ input: this.proc.stdout! });

    await this.request("initialize", {
      protocolVersion: 1,
      clientCapabilities: {
        fs: { readTextFile: true, writeTextFile: true },
        terminal: true,
      },
    });

    const { sessionId } = await this.request("session/new", {
      cwd: this.cwd,
      mcpServers: [],
      _meta: { yoloMode: true },
    });
    this.sessionId = sessionId;
    return this;
  }

  private async request(method: string, params: any): Promise<any> {
    return new Promise((resolve) => {
      const msg = JSON.stringify({ jsonrpc: "2.0", id: 1, method, params });
      this.proc.stdin!.write(msg + "\n");

      this.rl.once("line", (line) => {
        resolve(JSON.parse(line).result || {});
      });
    });
  }

  async *streamPrompt(text: string) {
    const msg = JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "session/prompt",
      params: {
        sessionId: this.sessionId,
        prompt: [{ type: "text", text }],
      },
    });
    this.proc.stdin!.write(msg + "\n");

    for await (const line of this.rl) {
      const data = JSON.parse(line);

      if (data.method === "session/update") {
        const update = data.params.update;
        yield update; // { sessionUpdate, content, title, ... }
      } else if (data.result) {
        break; // Final response
      }
    }
  }
}

// Usage
const client = await new VktrACPChat(".").init();

for await (const update of client.streamPrompt("List the files in this project")) {
  switch (update.sessionUpdate) {
    case "agent_message_chunk":
      process.stdout.write(update.content?.text || "");
      break;
    case "agent_thought_chunk":
      console.log(`\n[Thinking: ${update.content?.text}]`);
      break;
    case "tool_call":
      console.log(`\n[Tool: ${update.title}]`);
      break;
  }
}
```

---

## Resources

- [ACP Specification](https://agentclientprotocol.com/protocol/prompt-turn)
- [Protocol Introduction](https://agentclientprotocol.com/overview/introduction)
