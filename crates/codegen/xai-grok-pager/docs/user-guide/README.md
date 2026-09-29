# vktr User Guide

Learn how to install, configure, and extend vktr, a terminal coding agent whose model provider is Viktor. vktr is a rebranded fork of xAI's Grok Build (Apache-2.0).

---

## Tier 1: Essential User Docs

Start here. These guides cover what you need on your first day.

| # | Document | Description |
|---|----------|-------------|
| 1 | [Getting Started](01-getting-started.md) | Installation, the Viktor API key, first launch, basic interaction, and key concepts |
| 2 | [Authentication](02-authentication.md) | Viktor API keys: `VIKTOR_API_KEY`, `vktr login` / `vktr logout`, keys for custom models, and `vktr doctor` |
| 3 | [Keyboard Shortcuts](03-keyboard-shortcuts.md) | Reference for every key binding and mouse action in the TUI |
| 4 | [Slash Commands](04-slash-commands.md) | Every `/` command, including goals and workflow run management |
| 5 | [Configuration](05-configuration.md) | `config.toml`, `pager.toml`, the Viktor endpoint and protocol, environment variables, and file locations |

---

## Tier 2: Core Feature Docs

Customize and extend vktr.

| # | Document | Description |
|---|----------|-------------|
| 6 | [Theming and Appearance](06-theming.md) | Themes, the `/theme` command, `pager.toml`, and color-support detection |
| 7 | [MCP Servers](07-mcp-servers.md) | External tool integrations through the Model Context Protocol |
| 8 | [Skills](08-skills.md) | Reusable prompt packages in the SKILL.md format |
| 9 | [Plugins](09-plugins.md) | Bundle and share skills, commands, agents, hooks, and MCP servers; install from, author, and govern marketplaces |
| 10 | [Hooks](10-hooks.md) | Lifecycle scripts and HTTP callbacks for pre- and post-tool-use events |
| 11 | [Custom Models](11-custom-models.md) | The built-in `viktor` model, plus Ollama and other OpenAI-compatible endpoints as `[model.<name>]` |
| 12 | [Project Rules (AGENTS.md)](12-project-rules.md) | Per-directory AGENTS.md instructions and their precedence |
| 13 | [Memory](13-memory.md) | Cross-session knowledge persistence with `/flush`, `/dream`, and hybrid search |

---

## Tier 3: Advanced Usage Docs

Automate, script, and integrate vktr with other systems.

| # | Document | Description |
|---|----------|-------------|
| 14 | [Headless Mode and Scripting](14-headless-mode.md) | `vktr -p`, `--json`, piped input, output formats, and CI/CD |
| 15 | [Agent Mode and IDE Integration](15-agent-mode.md) | `vktr acp` (Viktor in Zed and JetBrains IDEs), `vktr agent stdio`, and `vktr launch` |
| 16 | [Subagents and Personas](16-subagents.md) | Parallel child sessions, agent types, personas, and capability modes (off by default in vktr) |
| 17 | [Session Management](17-sessions.md) | Save, load, resume, rewind, compact, Viktor thread continuation, and the session format |
| 18 | [Sandbox Mode](18-sandbox.md) | OS-level filesystem and network isolation profiles |
| 19 | [Plan Mode](19-plan-mode.md) | Structured planning, plan-file edits, and approval before coding |
| 20 | [Background Tasks and Monitoring](20-background-tasks.md) | `background: true`, `/loop`, `monitor`, and `Ctrl+B` to demote |
| 21 | [Terminal Support and Troubleshooting](21-terminal-support.md) | tmux, SSH, truecolor, clipboard, and OSC 52 |
| 22 | [Permissions and Safety](22-permissions-and-safety.md) | Modes (always-approve, auto, ask), rules, matching, hooks, and examples |
| 23 | [Agent Dashboard](23-dashboard.md) | Central overview of local sessions and forks |
| 24 | [Monitoring Usage](24-monitoring-usage.md) | Optional export of usage metrics and events to your own OpenTelemetry collector (vktr has no telemetry of its own) |
| 25 | [Status Line](25-status-line.md) | The bottom status row: built-in segments, command scripts, and the stdin JSON contract |
| 26 | [Configuration Reference](26-config-reference.md) | Field list for `config.toml` and `requirements.toml` |
| 27 | [vktr clone](27-grok-clone.md) | Not available in vktr (upstream's Grove-backed clone) |
