# vktr

`vktr` is a terminal coding agent with Viktor as its native model provider. It is a
rebranded fork of [Grok Build](https://github.com/xai-org/grok-build) (Apache-2.0), see
`NOTICE` and `docs/adr/0001-base.md`.

## Install

```bash
sh install.sh --tarball dist/vktr-<version>-<target>.tar.gz     # from a tarball built by scripts/dist.sh
curl -fsSL "$VKTR_RELEASE_BASE_URL/install.sh" | sh             # from a release location, once one exists
sh install.sh --from-source                                     # build in this checkout
```

The binary goes to `~/.vktr/bin` with a link in `~/.local/bin`; no root needed. Release downloads are
checked against their `.sha256`.

## Quick start

```bash
export VIKTOR_API_KEY=zt_live_sk_...        # a Viktor public API key with the chat:completions scope
vktr                                        # interactive TUI in the current directory
vktr -p "Explain this repository"           # headless, prints the answer and exits
```

Configuration lives in `~/.vktr/config.toml` (override the directory with `VKTR_HOME`).
The Viktor endpoint defaults to `https://api.viktor.com/api/compat/v1`; set `VIKTOR_BASE_URL`
for another deployment.

Any OpenAI-compatible backend can be added as a custom model:

```toml
[model.local]
model = "qwen3.5:4b"
base_url = "http://localhost:11434/v1"
env_key = "OPENAI_API_KEY"
```

## Launching other tools

`vktr launch` points an installed coding tool at a local OpenAI-compatible backend, or at Viktor,
without editing that tool's provider config:

```bash
vktr launch codex                                  # first reachable local backend, discovered model
vktr launch --backend ollama --model qwen3.5:4b opencode
vktr launch --backend http://host:8000 pi -p "explain this repo"
vktr launch --config opencode                      # print the wiring, start nothing
vktr launch --viktor claude                        # Claude Code against the Viktor compat API
```

Supported tools: claude, codex, copilot, grok, hermes, mi, opencode, pi, pool, vktr. Launch options go
before the tool name and everything after it is passed through. If [Harbor](https://github.com/av/harbor)
is installed, local launches are delegated to `harbor launch`, which can also start a backend.

## Building from source

Requirements: Rust (pinned in `rust-toolchain.toml`, rustup installs it), and
[DotSlash](https://dotslash-cli.com) on `PATH` for the hermetic `protoc`.

```bash
cargo build -p xai-grok-pager-bin --release   # -> target/release/vktr
```

## Layout

| Path | Contents |
| --- | --- |
| `crates/codegen/xai-grok-pager-bin` | Composition root; builds the `vktr` binary |
| `crates/codegen/xai-grok-pager` | The TUI |
| `crates/codegen/xai-grok-shell` | Agent runtime, headless and ACP entry points |
| `crates/codegen/xai-grok-tools` | Tool implementations |
| `crates/codegen/xai-grok-sampler` | Model backends (chat completions, responses, messages) |
| `crates/codegen/xai-grok-models` | Built-in model catalog (`viktor`) |
| `docs/` | ADRs, plan, recordings |
| `.facts` | Behavioural spec (`facts check`); `scripts/test.sh` runs the tests behind it |

Upstream crate names are kept as-is so diffs against upstream stay reviewable.

## License

Apache License 2.0. See `LICENSE`, `NOTICE` and `THIRD-PARTY-NOTICES`.
