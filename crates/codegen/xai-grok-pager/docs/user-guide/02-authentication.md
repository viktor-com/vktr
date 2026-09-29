# Authentication

vktr authenticates with a Viktor API key. There is no browser login, no OIDC/SSO, no device-code flow, and no xAI account: `vktr login --oauth` and `vktr login --device-auth` are refused with a message saying so.

---

## Get a key

Create a Viktor public API key with the `chat:completions` scope. Keys start with `zt_live_sk_`. vktr sends it as a bearer token to the Viktor compat API (default `https://api.viktor.com/api/compat/v1`, see [Configuration](05-configuration.md)).

---

## `vktr login`

```bash
vktr login                       # prompts for the key with input hidden
vktr login < key.txt             # or reads the first line of piped stdin
vktr login --api-key zt_live_sk_...   # or takes it as an argument (ends up in shell history)
```

`vktr login` checks the key with a `GET /models` request against the Viktor endpoint. On success it prints the endpoint and the models the key can use, and saves the key to `~/.vktr/config.toml` as:

```toml
[model.viktor]
api_key = "zt_live_sk_..."
```

The file is created with owner-only permissions on Unix, and the rest of it is left as it was. If the check fails, vktr prints Viktor's own error message (for example a wrong key or a missing `chat:completions` scope) and saves nothing.

`vktr login --api-key` can also read the key from the `VKTR_LOGIN_API_KEY` environment variable.

## Signing in from the TUI

Starting `vktr` without a key opens a sign-in screen with a box for the key (the input is masked).
Paste it and press Enter: vktr checks it with the same `GET /models` request, saves it the same way
as `vktr login`, and continues into the session. A key that does not work shows Viktor's error and
saves nothing; press `l` to try again. `/login` inside a session opens the same screen, for
switching to another key. No browser opens at any point.

## `vktr logout`

```bash
vktr logout
```

Removes `[model.viktor] api_key` from `~/.vktr/config.toml`. If `VIKTOR_API_KEY` is still set in your shell, `vktr logout` says so; unset it to sign out completely.

---

## `VIKTOR_API_KEY`

Instead of saving the key, export it:

```bash
export VIKTOR_API_KEY="zt_live_sk_..."
vktr
```

No login and no config file are needed. `vktr acp`, `vktr launch --viktor`, and `vktr doctor` read `VIKTOR_API_KEY` first and fall back to the key saved by `vktr login`, so an editor started without your shell environment still works once you have logged in. `XAI_API_KEY` is still accepted as a legacy fallback name.

---

## Keys for custom models

A model you define yourself under `[model.<name>]` in `config.toml` carries its own key, either inline or from an environment variable:

```toml
[model.local]
model = "qwen3.5:4b"
base_url = "http://localhost:11434/v1"
env_key = "OPENAI_API_KEY"      # or: api_key = "..."
```

A model's own `api_key` / `env_key` is used for requests to that model. See [Custom Models](11-custom-models.md).

---

## Checking your setup: `vktr doctor`

```bash
vktr doctor
```

After the terminal checks, `vktr doctor` prints a Viktor section:

| Row | Shows |
|-----|-------|
| endpoint | the base URL and where it came from (`default`, `VIKTOR_BASE_URL`, or `config.toml`) |
| api key | the key's source, masked as `zt_live_sk_…` plus its last four characters |
| connection | a live `GET /models` check with its latency, or Viktor's error |
| protocol | `responses`, `chat_completions`, or `messages`, and whether sessions continue one Viktor thread |
| toolset | lean (default) or full (`VKTR_FULL_TOOLSET=1`) |
| acp | saved `vktr acp` editor sessions and whether editor tools are on |

It ends with `Viktor: ready` or the number of problems found.

---

## Credential storage

The saved key lives in `~/.vktr/config.toml` (owner-only). MCP OAuth tokens live in `~/.vktr/mcp_credentials.json`, also owner-only. Anyone with filesystem access to those files can use the credentials, so:

- Prefer full-disk encryption (FileVault, BitLocker, LUKS, or equivalent).
- Do not copy `config.toml` or `mcp_credentials.json` into shared directories, tickets, or chat.
- On multi-user hosts, keep `$HOME` / `$VKTR_HOME` private to your account.

---

## Related settings

vktr has no telemetry and no feedback service. Usage export to your own OpenTelemetry collector is optional and off by default; see [Monitoring Usage](24-monitoring-usage.md).

---

## Troubleshooting

### Debug logging

Set `RUST_LOG` to control the verbosity of the file log and headless stderr output. (The TUI's on-screen tracing pane uses a fixed filter and ignores `RUST_LOG`.) In the TUI, file logging defaults to `DEBUG`; in headless mode (`-p`), `RUST_LOG` defaults to `off` so only the answer is printed — set `RUST_LOG=error` (or broader) to see logs on stderr.

In the TUI, set `VKTR_LOG_FILE` to an absolute path to write logs to that file:

```bash
VKTR_LOG_FILE=/tmp/vktr.log RUST_LOG=debug vktr
tail -f /tmp/vktr.log
```

`VKTR_LOG_FILE` is treated as a literal file path. A relative value such as `1` writes a file named `1` in the current directory.

In headless mode, logs go to stderr. Redirect them to a file:

```bash
RUST_LOG=debug vktr -p "hello" 2> /tmp/vktr.log
```

### Common fixes

- **No key found** -- run `vktr login`, or export `VIKTOR_API_KEY`. `vktr doctor` shows which key vktr sees.
- **Authentication failed / HTTP 401** -- the key is wrong or revoked. Run `vktr login` again with a fresh key.
- **HTTP 403 about a scope** -- the key lacks `chat:completions`. Create a key with that scope.
- **Cannot reach Viktor** -- check `VIKTOR_BASE_URL` and `[endpoints] viktor_base_url`; `vktr doctor` shows the endpoint in use.
