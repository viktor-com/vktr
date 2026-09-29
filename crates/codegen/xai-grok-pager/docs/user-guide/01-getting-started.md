# Getting Started

vktr is a terminal coding agent whose model provider is Viktor. It runs as a TUI (Terminal User Interface) that understands your codebase, executes shell commands, edits files, fetches web pages, and manages tasks. The model is Viktor, reached through the Viktor compat API; each prompt you send becomes a Viktor run.

You can use it interactively as a full-screen TUI, run it headlessly for scripting and CI/CD, or serve Viktor to an editor over the Agent Client Protocol (ACP) with `vktr acp`.

vktr is a rebranded fork of xAI's Grok Build (Apache-2.0). It does not contact xAI services and has no telemetry.

---

## Installation

Releases are installed with `install.sh` from `viktor-com/vktr`:

```bash
curl -fsSL https://raw.githubusercontent.com/viktor-com/vktr/main/install.sh | sh
```

From a checkout of the source:

```bash
sh install.sh --tarball dist/vktr-<version>-<target>.tar.gz   # a tarball built by scripts/dist.sh
sh install.sh --from-source                                   # build here
```

The binary goes to `~/.vktr/bin` with a link in `~/.local/bin`; no root is needed. Release downloads are checked against their `.sha256`, and a release that cannot be verified is refused. `VKTR_RELEASE_REPO` (or `VKTR_RELEASE_BASE_URL` for a plain file server) points the installer at another location.

Builds exist for Linux x86_64 and aarch64 and need glibc 2.28 or newer (RHEL 8, Debian 10, Ubuntu 20.04 and later). macOS needs a build from source on a Mac.

Verify the installation:

```bash
vktr --version
```

vktr does not update itself. `vktr update` only explains how to update: rerun the installer you used, or `sh install.sh --from-source` in a checkout.

---

## First Launch

vktr needs a Viktor API key (it starts with `zt_live_sk_` and needs the `chat:completions` scope). Save it once:

```bash
vktr login          # paste the key; input is hidden
vktr doctor         # check the key, endpoint and connection
vktr                # start the TUI in the current directory
```

`vktr login` verifies the key against Viktor and saves it in `~/.vktr/config.toml` as `[model.viktor] api_key`, readable only by you. If you start `vktr` without a key, it opens a sign-in screen instead: paste the key there and it is checked and saved the same way. Alternatively, set the key in the environment; it wins over the saved key:

```bash
export VIKTOR_API_KEY="zt_live_sk_..."
vktr
```

There is no browser login, SSO, or device-code flow. See [Authentication](02-authentication.md) for details.

---

## Basic Interaction

Once authenticated, vktr presents a full-screen TUI with two main areas:

- **Scrollback** -- the conversation history showing your prompts, vktr's responses, tool calls, file edits, and more.
- **Prompt** -- the input area at the bottom where you type messages.

Type a message and press `Enter` to send it. vktr reads files, runs commands, and edits code as needed. Each tool run streams into the scrollback in real time.

Press `Tab` to move focus between the prompt and the scrollback. While a turn is running, `Ctrl+C` cancels it once the composer is empty — with a draft, the first press only clears it. `Esc` never cancels a turn; mid-turn it shows a reminder to use `Ctrl+C`. Idle, press `Esc` twice within 800ms to clear a non-empty prompt, or (with an empty prompt and conversation messages) to open rewind — see [Keyboard Shortcuts](03-keyboard-shortcuts.md#escape). With the scrollback focused, use the arrow keys to select entries and to collapse or expand them. To navigate with `j`/`k` and fold with `h`/`l` instead, enable Vim mode.

### File References

Use `@` in your prompt to attach files:

```
@src/main.rs              # Attach a file
@src/main.rs:10-50        # Attach lines 10-50
@src/                     # Browse a directory
```

The `@` operator opens a fuzzy file picker. By default it respects `.gitignore` and hides dotfiles. Prefix with `!` to search hidden files:

```
@!.github                 # Search hidden files
@!.env                    # Attach a .env file
```

### Permissions

By default, vktr asks for permission before executing shell commands or editing files. You can approve individually or toggle always-approve mode:

- Press `Ctrl+O` to toggle always-approve mode
- Use the `--yolo` flag at launch: `vktr --yolo`
- Type `/always-approve` in the prompt to toggle the mode

---

## Key Concepts

### Sessions

Every conversation is a **session**. Sessions are automatically saved under `~/.vktr/sessions/<encoded working directory>/` and can be resumed later. Each session tracks the full conversation history, tool calls, file edits, and task state. On Viktor's default (responses) protocol a session is one Viktor thread, so Viktor keeps its context and sandbox state across turns and across `--continue` / `--resume`.

- Start a new session: `Ctrl+N` or `/new`
- Resume a previous session: `/resume` in the TUI, or `--resume <ID>` from the CLI
- Continue the most recent session: `vktr -c`

### Scrollback

The scrollback is the main display area. It shows:

- **User prompts** -- your messages, rendered as sticky headers
- **Agent messages** -- vktr's responses with full markdown rendering and syntax highlighting
- **Thinking blocks** -- vktr's reasoning process (collapsible)
- **Tool calls** -- file edits (with inline diffs), command executions, search results, and more
- **Task lists** -- TODO items tracking progress

Collapse or expand the selected entry with the `Left`/`Right` arrow keys (or `h`/`l` and `e` in Vim mode). In Vim mode, press `y` to copy its content and `Y` to copy its metadata (for example, the command that ran). Press `Enter` to open it in the fullscreen viewer (in any mode).

### Tools

vktr has built-in tools that run on your machine and return their results to Viktor:

| Tool | Description |
|------|-------------|
| `read_file` / `search_replace` / `write` | Read and edit files with line-precise changes |
| `grep` | Regex search across your codebase (powered by ripgrep) |
| `list_dir` | List directory contents |
| `run_terminal_command` | Execute shell commands |
| `web_fetch` | Fetch URLs |
| `todo_write` | Create and manage task lists |
| `memory_search` | Search cross-session memory |

The default toolset is lean, because every tool schema is sent with every request. Workflows, subagents, the scheduler, `monitor`, and goal tools are off unless you set `VKTR_FULL_TOOLSET=1` (or `VKTR_WORKFLOWS=1` / `VKTR_SUBAGENTS=1` for one family). Tools that only work against xAI's backend (`web_search`, image and video generation, `send_feedback`) are not offered; `VKTR_XAI_BACKED_TOOLS=1` turns them back on, but they do not work against Viktor.

Viktor also has its own tools, which run in its cloud sandbox. The compat API does not report their names yet, so they are not shown as tool calls.

Tools can be extended with [MCP servers](05-configuration.md#mcp-servers) for integrations like GitHub, databases, and more.

### Slash Commands

Type `/` in the prompt to access commands. These provide quick actions without writing a full prompt:

```
/model viktor                   # Switch model
/compact                          # Compress conversation history
/always-approve                   # Toggle always-approve mode
/new                              # Start a new session
```

See [Slash Commands](04-slash-commands.md) for the complete reference.

---

## Common Launch Options

```bash
# Launch the interactive TUI and submit an initial prompt as the first turn
vktr "fix the failing auth test and run it"

# Initial prompt in a new git worktree. Use --worktree=<name> (with `=`) so the
# prompt isn't swallowed as the worktree name — `vktr -w "refactor module X"`
# would treat "refactor module X" as the worktree label, not the prompt.
vktr --worktree=feat "refactor module X"

# Base the worktree on a specific branch (e.g. main) instead of the current HEAD:
vktr -w --ref main "implement feature from main"


# Start in a specific project directory
vktr --cwd ~/projects/my-app

# Add project-specific rules
vktr --rules "Always use TypeScript. Prefer functional components."

# Auto-approve all tool executions
vktr --yolo

# Use a specific model (viktor, or a [model.<name>] you defined)
vktr -m viktor

# Resume a previous session
vktr --resume <session-id>

# Continue the most recent session
vktr -c

# Experimental scrollback-native render mode. Sticky: plain `vktr` reopens in
# the mode last chosen via --minimal/--fullscreen (or /minimal//fullscreen).
vktr --minimal

# Back to the standard fullscreen TUI (and make it sticky again)
vktr --fullscreen

# Headless mode (for scripts)
vktr -p "Explain this codebase"
```

---

## Headless Mode

Run vktr non-interactively for scripting, CI/CD, and automation:

```bash
vktr -p "Your prompt here"
```

Output formats:

| Format | Flag | Description |
|--------|------|-------------|
| `plain` | (default) | Human-readable text |
| `json` | `--output-format json` or `--json` | Single JSON object with `text`, `stopReason`, `sessionId`, `requestId`, and usage |
| `streaming-json` | `--output-format streaming-json` | NDJSON event stream for real-time processing |

Example CI/CD usage:

```bash
git diff | vktr -p "Review this change for bugs" --json | jq -r '.text'
```

Piped input is appended to the prompt; a bare `vktr -p` takes piped input as the whole prompt.

---

## Project Rules (AGENTS.md)

Add per-project instructions by creating an `AGENTS.md` file in your repository. vktr reads these files and injects their contents as a project-instructions message at the start of the conversation:

```
~/.vktr/AGENTS.md           # Global rules (apply to all projects)
<repo-root>/AGENTS.md       # Repository-level rules
<cwd>/AGENTS.md             # Directory-level rules (highest priority)
```

Deeper files take precedence. vktr also reads `CLAUDE.md` files for compatibility.

---

## Where to Go Next

| Document | What You Will Learn |
|----------|-------------------|
| [Authentication](02-authentication.md) | Viktor API keys, `vktr login`, `vktr doctor` |
| [Keyboard Shortcuts](03-keyboard-shortcuts.md) | Complete reference for all key bindings |
| [Slash Commands](04-slash-commands.md) | All available `/` commands |
| [Configuration](05-configuration.md) | config.toml, pager.toml, environment variables |
