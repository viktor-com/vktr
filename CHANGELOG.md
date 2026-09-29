# vktr changelog

## Unreleased

### Look and feel

- While Viktor works, the spinner cycles through the Viktor Hero Gradient and a soft highlight
  sweeps across the status label ("Thinking…", "Waiting for response…"); tool and approval states
  keep their own colours
- The welcome box's top edge draws in with the Hero Gradient (navy → violet → lilac → peach →
  yellow) when vktr starts, then holds; Viktor Day uses an amber end that reads on light terminals
- `vktr doctor` opens its Viktor section with the official Viktor avatar on truecolor terminals;
  piped output, `NO_COLOR` and other terminals keep the plain `Viktor` title
- Each theme has a five-stop brand gradient; 256- and 16-colour terminals get the nearest palette
  steps instead of banding

## 1.0.38-vktr.2 — 2026-09-26

First public release.

### Sign-in and setup

- Starting `vktr` without a key opens a sign-in screen: paste a Viktor API key and it is checked,
  saved and used; no browser login. `VIKTOR_API_KEY` now wins over the saved key everywhere
- `vktr login` asks for the key with input hidden (or reads it from stdin), so it stays out of shell
  history; `vktr logout` removes it
- `vktr doctor` checks your Viktor setup: key source, endpoint, a live key check, protocol and toolset
- `vktr --help` explains what vktr is and ends with a quick start; help screens no longer show
  xAI-only options
- The installer suggests `vktr login` instead of putting the key on the command line, and
  "latest" means the newest stable release, never a prerelease

### Editors (Agent Client Protocol)

- `vktr acp` serves Viktor to Zed, JetBrains IDEs and other ACP clients, replacing the
  standalone TypeScript `viktor-acp` agent (now deprecated); see `docs/acp.md`
- One editor session is one Viktor thread, and the key comes from `VIKTOR_API_KEY` or the
  `vktr login` store, so an editor launched without a shell environment still works
- Viktor can read your workspace, write files and run commands through the editor when it offers
  them; every write and command asks you first, with the diff
- `vktr acp` connects the MCP servers your editor lists and lets Viktor call their tools (asking
  you first), so Viktor can use your local context servers from the cloud
- Sessions survive a restart: `session/load` replays the conversation and the next prompt
  continues the same Viktor thread
- `vktr acp --print-config zed|jetbrains` prints the editor snippet with the right binary path

### Headless

- `vktr -p` takes piped input: `git diff | vktr -p "review this"` sends the diff along, and
  `echo question | vktr -p` uses it as the prompt (an idle inherited pipe is ignored after 1 s)
- Headless errors print once, as readable text

### TUI

- The welcome screen carries the Viktor wordmark as pixel art, two-tone dithered with a white
  glint, and shows it at 80x24 too; vktr Night and vktr Day use the Viktor brand colors
- An idle welcome screen stops redrawing once the wordmark settles (about 11 s), instead of
  repainting every 180 ms
- The sign-in hint wraps instead of being cut off in narrow windows
- The built-in guides (`/docs`) describe vktr instead of Grok Build
- A prompt sent right after cancelling one waits for Viktor to stop the old run instead of
  failing with "conversation has a response in progress", in the TUI and in `vktr acp`
- A leaner default toolset: workflows, subagents, scheduler, monitor and goal tools are off unless
  `VKTR_FULL_TOOLSET=1`, so every request is about a third smaller and replies come back steadier
- Long sessions stay on their Viktor thread: vktr no longer auto-compacts Viktor conversations
  (Viktor compacts its own threads), saving a billed summary run and Viktor's sandbox state

### Privacy and links

- `/privacy` says where your data goes (only the endpoint you configured; nothing to xAI), and the
  upstream xAI training opt-in is gone from settings; its banner can no longer be switched on
- Rate-limit, credit-limit and plan messages point to Viktor pricing and billing instead of
  grok.com and docs.x.ai; `vktr setup` no longer sends you to console.x.ai
- `vktr update` no longer consults Grok Build's release channel (it would have installed Grok over
  vktr); it tells you how to update. `vktr trace` always exports locally

### Build and packaging

- Prebuilt binaries for Linux (x86_64, aarch64; glibc 2.28 or newer) and macOS 11 or newer (Apple
  Silicon), built and smoke-tested on each OS by GitHub Actions before they are published
- `vktr launch --help` lists `grok` again (the rebrand had turned it into a second `vktr`)
- aarch64 Linux builds from source target any aarch64 CPU, not only Neoverse V2
- `cargo test --workspace` is a usable gate again: the upstream unit tests that still asserted
  removed xAI behaviour were updated or removed, five host-portability bugs they exposed were
  fixed in production code, and the PTY harness no longer wedges the run
- The README opens with a demo recording and covers editors, scripting and configuration; docs no
  longer suggest passing the key with `--api-key`

## 1.0.38-vktr.1

- Viktor is the built-in provider: set `VIKTOR_API_KEY` and chat, no login and no config file needed
- One Viktor thread per session via `previous_response_id`, kept across `--continue` and `--resume`
- Failed Viktor runs (`run_failed`, `response.failed`, empty streams) surface immediately with the server's message
- `vktr launch` points codex, opencode, pi, claude and other tools at a local backend or at Viktor
- Headless mode names a tool call it had to deny and exits non-zero instead of stopping silently
- One model request per prompt: LLM titles, turn summaries, recaps and prompt suggestions are opt-in
- No xAI traffic: no self-update, no telemetry, no vendor changelog fetch, no `x-grok-*` request headers

Forked from Grok Build 1.0.38 (Apache-2.0). See `NOTICE`.
