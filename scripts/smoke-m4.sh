#!/usr/bin/env bash
# M4 smoke test: `vktr launch`.
#
#   scripts/smoke-m4.sh [path/to/vktr]
#
# The Viktor-compat mock doubles as a local OpenAI-compatible backend. Real host tools that are
# installed (codex, opencode, pi, claude) are launched for real; missing ones are skipped.
set -u
BIN="${1:-target/release/vktr}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
V="$ROOT/$BIN"
# A free port unless MOCK_PORT pins one: a stale server on a fixed port would silently absorb the test traffic.
PORT="${MOCK_PORT:-$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')}"
WORK="$(mktemp -d "$HOME/.vktr-smoke4.XXXXXX")"
PASS=0; FAIL=0; SKIP=0
B="http://127.0.0.1:$PORT"
export PATH="$PATH:$HOME/.local/share/npm/bin"
export VKTR_HOME="$WORK/vktr-home" PI_CODING_AGENT_DIR="$WORK/pi" VKTR_LAUNCH_GROK_CONFIG="$WORK/grok/config.toml"
# Opt out of Harbor for the whole run so the native wiring is what gets tested. Without this,
# every local launch is delegated on a host that happens to have Harbor installed, and the
# delegation cases below re-enable it explicitly against their own fake `harbor`.
export VKTR_LAUNCH_NO_HARBOR=1

python3 "$ROOT/scripts/mock_viktor.py" "$PORT" >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null; rm -rf "$WORK"' EXIT
"$ROOT/scripts/wait-for-mock.sh" "$PORT" || exit 1
mkdir -p "$WORK/proj" "$WORK/fakebin"; cd "$WORK/proj" && git init -q .

ok()   { PASS=$((PASS+1)); echo "PASS $1"; }
bad()  { FAIL=$((FAIL+1)); echo "FAIL $1"; shift; printf '%s\n' "$@" | tail -6 | cut -c1-220 | sed 's/^/     | /'; }
skip() { SKIP=$((SKIP+1)); echo "SKIP $1 ($2 not installed)"; }
# expect <name> <needle...> -- <command...>: output must contain every needle
expect() {
  local name="$1"; shift; local needles=()
  while [ "$1" != "--" ]; do needles+=("$1"); shift; done; shift
  local out; out="$("$@" 2>&1 </dev/null)"; local miss=""
  for n in "${needles[@]}"; do printf '%s' "$out" | grep -qF -- "$n" || miss="$miss [$n]"; done
  if [ -z "$miss" ]; then ok "$name"; else bad "$name" "missing:$miss" "$out"; fi
}

# --- --config prints the wiring without starting anything --------------------------------------
expect config-codex 'tool=codex' "base_url=$B/v1" 'model=viktor' 'OPENAI_API_KEY=sk-local' \
  "model_providers.vktr_launch.base_url=\"$B/v1\"" 'wire_api="responses"' ' -m viktor --sandbox workspace-write' \
  -- "$V" launch --backend "$B" --config codex --sandbox workspace-write
expect config-explicit-model 'model=my-model' 'opencode -m vktr-custom/my-model' '"my-model":{' \
  -- "$V" launch --backend "$B/v1" --model my-model --config opencode
expect config-user-model-wins 'codex -c' -- "$V" launch --backend "$B" --config codex -m other
if "$V" launch --backend "$B" --config codex -m other 2>&1 | grep -q -- '-m viktor'; then bad config-user-model-not-overridden "launch added its own -m"; else ok config-user-model-not-overridden; fi
expect config-pi "wrote $WORK/pi/models.json" 'pi --provider vktr-custom --model viktor --session-dir' -- "$V" launch --backend "$B" --config pi
if [ ! -e "$WORK/pi/settings.json" ] && grep -q '"vktr-custom"' "$WORK/pi/models.json"; then ok pi-provider-registered-defaults-untouched; else bad pi-provider-registered-defaults-untouched "$(ls "$WORK/pi")"; fi
expect config-grok "GROK_MODELS_BASE_URL=$B/v1" 'grok -m vktr-custom' -- "$V" launch --backend "$B" --config grok
if grep -q '\[model.vktr-custom\]' "$WORK/grok/config.toml"; then ok grok-model-entry-written; else bad grok-model-entry-written "$(cat "$WORK/grok/config.toml" 2>&1)"; fi
expect config-env-tools 'OPENAI_BASE_URL=' 'HERMES_MODEL=viktor' 'hermes chat' -- "$V" launch --backend "$B" --config hermes
expect config-copilot 'COPILOT_PROVIDER_WIRE_API=responses' 'COPILOT_MODEL=viktor' -- "$V" launch --backend "$B" --config copilot

# --- --viktor ---------------------------------------------------------------------------------
export VIKTOR_BASE_URL="$B/v1"
expect viktor-config-claude "ANTHROPIC_BASE_URL=$B" 'ANTHROPIC_SMALL_FAST_MODEL=viktor' 'unset ANTHROPIC_AUTH_TOKEN' 'claude --model viktor' 'zt_test_… (masked)' \
  -- env VIKTOR_API_KEY=zt_test_sk_mock_secret "$V" launch --viktor --config claude
if env VIKTOR_API_KEY=zt_test_sk_mock_secret "$V" launch --viktor --config opencode 2>&1 | grep -q 'mock_secret'; then bad viktor-key-masked-in-config "key printed in clear"; else ok viktor-key-masked-in-config; fi
expect viktor-needs-key 'no Viktor API key' -- env -u VIKTOR_API_KEY -u XAI_API_KEY "$V" launch --viktor --config codex
expect viktor-excludes-backend 'mutually exclusive' -- env VIKTOR_API_KEY=k "$V" launch --viktor --backend ollama --config codex
mkdir -p "$VKTR_HOME"; printf '[model.viktor]\napi_key = "zt_saved_key_from_login"\n' >"$VKTR_HOME/config.toml"
expect viktor-uses-saved-login-key 'OPENAI_API_KEY=zt_saved… (masked)' -- env -u VIKTOR_API_KEY -u XAI_API_KEY "$V" launch --viktor --config codex
rm -f "$VKTR_HOME/config.toml"

# --- errors -----------------------------------------------------------------------------------
expect error-no-backend 'no running OpenAI-compatible backend found' 'install Harbor' -- "$V" launch --backend http://127.0.0.1:9 --config codex
expect error-claude-needs-anthropic 'Anthropic Messages API' -- "$V" launch --backend "$B" --config claude
expect error-unknown-tool 'unknown tool `notatool`' -- "$V" launch --backend "$B" --config notatool
expect error-harbor-only-tool 'configured by Harbor only' -- "$V" launch --backend "$B" --config droid

# --- a fake tool proves cwd, env and argument pass-through ---------------------------------------
cat >"$WORK/fakebin/mi" <<'EOF'
#!/bin/sh
echo "cwd=$(pwd)"; echo "base=$OPENAI_BASE_URL model=$MODEL key=$OPENAI_API_KEY"; echo "args=$*"
EOF
chmod +x "$WORK/fakebin/mi"
expect tool-runs-in-invoking-dir "cwd=$WORK/proj" "base=$B model=viktor key=sk-local" 'args=-p say hello --flag' \
  -- env PATH="$WORK/fakebin:$PATH" "$V" launch --backend "$B" mi -p "say hello" --flag

# --- Harbor delegation --------------------------------------------------------------------------
cat >"$WORK/fakebin/harbor" <<'EOF'
#!/bin/sh
echo "HARBOR_CALLED $*"
EOF
chmod +x "$WORK/fakebin/harbor"
expect harbor-delegation 'HARBOR_CALLED launch --backend ollama --model qwen3.5:4b --config codex --sandbox workspace-write' \
  -- env -u VKTR_LAUNCH_NO_HARBOR PATH="$WORK/fakebin:$PATH" "$V" launch --backend ollama --model qwen3.5:4b --config codex --sandbox workspace-write
expect harbor-not-used-for-viktor 'tool=codex' 'backend=viktor' -- env -u VKTR_LAUNCH_NO_HARBOR PATH="$WORK/fakebin:$PATH" VIKTOR_API_KEY=k "$V" launch --viktor --config codex
expect harbor-opt-out 'backend=custom' -- env PATH="$WORK/fakebin:$PATH" VKTR_LAUNCH_NO_HARBOR=1 "$V" launch --backend "$B" --config codex

# --- real host tools ----------------------------------------------------------------------------
real() { # real <name> <binary> <command...>
  local name="$1" bin="$2"; shift 2
  command -v "$bin" >/dev/null 2>&1 || { skip "$name" "$bin"; return; }
  local out; out="$(timeout 150 "$@" 2>&1 </dev/null)"
  if printf '%s' "$out" | grep -q 'MOCK_OK'; then ok "$name"; else bad "$name" "$out"; fi
}
real real-vktr-local      "$V"      "$V" launch --backend "$B" vktr -p "hi"
real real-opencode-local  opencode  "$V" launch --backend "$B" opencode run "hi"
real real-codex-local     codex     "$V" launch --backend "$B" codex exec --skip-git-repo-check "hi"
real real-pi-local        pi        "$V" launch --backend "$B" pi -p "hi"
export VIKTOR_API_KEY=zt_test_sk_mock
real real-vktr-viktor     "$V"      "$V" launch --viktor vktr -p "hi"
real real-codex-viktor    codex     "$V" launch --viktor codex exec --skip-git-repo-check "hi"
real real-opencode-viktor opencode  "$V" launch --viktor opencode run "hi"
real real-pi-viktor       pi        "$V" launch --viktor pi -p "hi"
real real-claude-viktor   claude    "$V" launch --viktor claude -p "hi"

echo "---"; echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ $FAIL -eq 0 ]
