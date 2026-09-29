# `vktr acp`: Viktor in Zed, JetBrains IDEs and other ACP clients

`vktr acp` serves [Viktor](https://viktor.com) to an editor over the
[Agent Client Protocol](https://agentclientprotocol.com) on stdio. Pick Viktor in your editor's
agent panel and ask it to do work in your team's systems without leaving the editor.

Viktor runs in its own cloud sandbox. When your editor lets agents use its files and terminals
(Zed and the JetBrains IDEs do), Viktor can also read your workspace, write files and run
commands **through the editor**, and the editor asks you before every write or command. One
editor session is one Viktor thread, so follow-up prompts keep Viktor's context and sandbox
state, and a session survives restarting the editor or the agent.

> `vktr acp` is not `vktr agent stdio`. That command serves vktr's **own local coding agent**
> over the same protocol. Here the agent is Viktor, working in its cloud sandbox with your
> team's tools, and reaching your machine only through the editor.

## Status: this replaces the TypeScript `viktor-acp`

`vktr acp` is the **canonical Viktor ACP agent**. The standalone TypeScript package
`viktor-acp` (in `viktor-integrations/acp`) is **deprecated**: it stays where it is and still
works, but it will not gain features, and editor configs should move to `vktr acp`.

Why the move: one native binary with no Node or `npx` on the critical path, the same key
resolution and TLS policy as the rest of vktr, and one codebase to keep at protocol parity.

Behaviour was ported case by case, not transliterated. `crates/codegen/vktr-acp/tests/acp_protocol.rs`
drives a **real ACP client** against a mock Viktor, mirroring the TypeScript suite, so the two can
be compared test by test.

## Set up

Give it a Viktor API key (scope `chat:completions`), either way:

```bash
export VIKTOR_API_KEY=zt_live_sk_...   # environment
vktr login                             # or paste it once (input hidden); saved to ~/.vktr/config.toml
```

`vktr acp` reads the environment variable first and falls back to the saved key, so an editor
launched from the desktop (with no shell environment) still works once you have logged in.

`vktr acp --print-config zed` (or `jetbrains`) prints the snippet below with the full path of your
`vktr`, ready to paste.

### Zed

In `settings.json`:

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

If your editor does not inherit your shell environment and you have not run `vktr login`, add the
key explicitly:

```json
{
  "agent_servers": {
    "Viktor": {
      "type": "custom",
      "command": "vktr",
      "args": ["acp"],
      "env": { "VIKTOR_API_KEY": "zt_live_sk_..." }
    }
  }
}
```

Use the absolute path (`~/.local/bin/vktr`) if Zed cannot find `vktr` on its `PATH`.

### JetBrains IDEs

In `~/.jetbrains/acp.json`:

```json
{
  "agent_servers": {
    "Viktor": { "command": "vktr", "args": ["acp"] }
  }
}
```

### Any other ACP client

Run `vktr acp` as the agent command over stdio. stdout carries the protocol and nothing else;
diagnostics go to stderr.

## What it supports

| ACP feature | `vktr acp` | TypeScript `viktor-acp` |
| --- | --- | --- |
| Protocol | v1 over stdio | v1 over stdio |
| `initialize`, `authenticate`, `session/new`, `session/prompt`, `session/cancel` | yes | yes |
| Streaming | `agent_message_chunk` as Viktor writes | same |
| Prompt content | text, images, embedded text resources, resource links | same |
| Session continuity | one session = one Viktor thread (`previous_response_id`) | same |
| Editor tools | Viktor reads, writes and runs commands through the editor's `fs/*` and `terminal/*`, when offered | no |
| Permission requests | every write and command, with the diff; "always allow" per session | no |
| The editor's MCP servers | stdio and HTTP servers become Viktor tools (`mcp__<server>__<tool>`), each call asked first | ignored |
| `session/load`, `session/resume`, `session/list` | yes: sessions are kept in `~/.vktr/acp/sessions/` | no |
| Stop reasons | `end_turn`, `cancelled`, `max_tokens` (Viktor's 600 s run cap) | same |
| Prompt right after a cancel | waits for Viktor to stop the cancelled run (~15 s) | fails with `conversation_busy` |
| Failed run | JSON-RPC error with Viktor's message, error class and request id | same |
| API key source | `VIKTOR_API_KEY` **or** the `vktr login` store | `VIKTOR_API_KEY` only |
| Endpoint override | `VIKTOR_BASE_URL`: bare host, `/api/compat`, or any `/v1` root | `VIKTOR_BASE_URL`, bare host only |
| Runtime | one static binary | Node ≥ 22 plus `npx` |
| TLS | vktr's policy; survives a broken OS trust store | Node default |

An editor that offers neither file-system nor terminal capabilities sees exactly the TypeScript
agent's wire: no tools are sent, only text streams back.

## Editor tools

When the editor's `initialize` request offers them, Viktor gets these tools, each only if the
matching capability is there:

| Tool | Editor method | Asks first | Shown as |
| --- | --- | --- | --- |
| `editor_read_file` | `fs/read_text_file` (includes unsaved changes) | only outside the workspace | a read tool call with the file's location |
| `editor_write_file` | `fs/write_text_file` | yes, with the diff | an edit tool call with the diff |
| `editor_run_command` | `terminal/create` running `sh -c <command>` | yes | an execute tool call embedding the live terminal |

Relative paths resolve against the session's workspace root, and `.` / `..` are collapsed before
anything runs, so the path you approve is the path that is touched. Viktor's call arrives on the
Responses stream, runs through the editor, and the result goes back to Viktor on the same thread,
so the run carries on where it paused. A rejected call is not retried blindly: Viktor is told the
user said no. Cancelling while the editor is asking leaves the call unrun and answers it as
cancelled at the start of the next turn. Results Viktor has not yet received are kept in the
session file until it does, so a network error or a restart never leaves the thread waiting for
an answer.

Viktor's own tools still run in its sandbox and are not shown as tool calls, because the compat
API does not put their names on the wire yet.

Set `VKTR_ACP_EDITOR_TOOLS=0` to keep Viktor out of the workspace entirely.

## MCP servers

Editors pass their configured MCP servers (Zed's context servers, a JetBrains IDE's MCP settings) in
`session/new`. Viktor runs in the cloud and cannot reach them, so `vktr acp` connects to them on
your machine and offers their tools to Viktor as `mcp__<server>__<tool>`. A call runs here and its
result goes back to Viktor on the same thread; the editor asks you before every call, since an MCP
tool can do anything ("always allow" remembers one tool for the session).

- stdio servers start in the session's workspace, in their own process group, and are killed with
  the session (grandchildren included); HTTP servers get the editor's headers. SSE is not offered.
- A server that fails to start is named in the first turn ("MCP server `x` did not start: …");
  the others still work.
- `VKTR_ACP_MCP=0` leaves the editor's MCP servers unconnected.

## Sessions

Each session is saved to `~/.vktr/acp/sessions/<session id>.json` (owner-only): the Viktor thread
it maps to, its workspace, a title from the first prompt, and the last 400 transcript entries
(tool calls without their content). Editors can list sessions (`session/list`), reload one with
its transcript replayed (`session/load`), or reattach without a replay (`session/resume`); the
next prompt continues the same Viktor thread either way.

Audio blocks and embedded binary blobs are dropped rather than guessed at, because Viktor's
Responses surface has no part type for them. The TypeScript agent did the same.

A failed Viktor run never appears as assistant text. Viktor emits `[Stream error: …]` as output
just before it fails the run; that text is dropped and the failure is reported as a protocol
error carrying Viktor's own message.

## Troubleshooting

`vktr acp` writes two lines to stderr on start: whether it found a key, and which endpoint it is
serving. Most editors surface an agent's stderr in a log panel.

- **"no Viktor API key"** — the handshake still succeeds so the editor can show the auth method,
  but prompts fail. Set `VIKTOR_API_KEY` or run `vktr login`.
- **Auth errors** carry Viktor's own message plus a hint for the fixable cases (wrong key,
  expired key, missing `chat:completions` scope, no linked chat identity).
- **A wrong endpoint** shows as `Could not reach Viktor at …`; check `VIKTOR_BASE_URL`.
- **"Viktor is still stopping the previous run"** right after you cancel: Viktor needs about 15 s
  to stop a run; the prompt waits (up to two minutes) and then goes through.
- **Viktor says it cannot see your files**: your editor did not offer file-system capabilities,
  or `VKTR_ACP_EDITOR_TOOLS=0` is set.
