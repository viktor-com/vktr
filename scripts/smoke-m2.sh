#!/usr/bin/env bash
# M2 smoke test: local tool round trips over the Viktor compat API, approvals, and edits.
#
#   scripts/smoke-m2.sh [path/to/vktr]
#
# The mock asks vktr to run one caller tool; vktr must execute it locally, send the result
# back on the same protocol, and print the final reply that quotes the result. Covered:
# read_file / run_terminal_command / search_replace / write on responses, chat completions and
# messages; headless denial without approval; --allow rules; --always-approve.
set -u
BIN="${1:-target/release/vktr}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# A free port unless MOCK_PORT pins one: a stale server on a fixed port would silently absorb the test traffic.
PORT="${MOCK_PORT:-$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')}"
WORK="$(mktemp -d "$HOME/.vktr-smoke2.XXXXXX")"
PASS=0; FAIL=0

python3 "$ROOT/scripts/mock_viktor.py" "$PORT" >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null; rm -rf "$WORK"' EXIT
"$ROOT/scripts/wait-for-mock.sh" "$PORT" || exit 1

# case <name> <mode> <backend> <expect: substring in output> <file-check: shell test run in project dir> [vktr flags...]
case_run() {
  local name="$1" mode="$2" backend="$3" needle="$4" check="$5"; shift 5
  local home="$WORK/home-$name" proj="$WORK/proj-$name"; mkdir -p "$home" "$proj"
  (cd "$proj" && git init -q . && echo 'print("hello from main")' >main.py)
  printf '[model.viktor]\napi_backend = "%s"\nextra_headers = { "X-Mock-Mode" = "%s" }\n' "$backend" "$mode" >"$home/config.toml"
  local out; out="$(cd "$proj" && HOME="$WORK" VKTR_HOME="$home" VIKTOR_API_KEY=zt_test_sk_mock \
      VIKTOR_BASE_URL="http://127.0.0.1:$PORT/v1" timeout 120 "$ROOT/$BIN" -p "do it" "$@" 2>&1)"
  local code=$? ok=1
  if [ "${WANT_EXIT:-zero}" = zero ] && [ $code -ne 0 ]; then ok=0; fi
  if [ "${WANT_EXIT:-zero}" = nonzero ] && [ $code -eq 0 ]; then ok=0; fi
  printf '%s' "$out" | grep -qF -- "$needle" || ok=0
  (cd "$proj" && eval "$check") >/dev/null 2>&1 || ok=0
  if [ $ok -eq 1 ]; then PASS=$((PASS+1)); echo "PASS $name (exit $code)"
  else FAIL=$((FAIL+1)); echo "FAIL $name (exit $code; wanted '$needle' and: $check)"; printf '%s\n' "$out" | tail -4 | cut -c1-240 | sed 's/^/     | /'; fi
}

for backend in responses chat_completions messages; do
  case_run "read-$backend"  tool_read  "$backend" 'MOCK_TOOL_OK: 1→print("hello from main")' 'true'
  case_run "bash-$backend"  tool_bash  "$backend" 'MOCK_TOOL_OK:' 'grep -q TOOL_RAN tool_ran.txt' --always-approve
  case_run "edit-$backend"  tool_edit  "$backend" 'MOCK_TOOL_OK:' 'grep -q "goodbye from main" main.py' --always-approve
  case_run "write-$backend" tool_write "$backend" 'MOCK_TOOL_OK:' 'grep -q "written by the mock" notes.txt' --always-approve
done

# Approvals: without a grant, calls that need approval are denied, nothing changes on disk, the operator is
# told why, and the run exits non-zero instead of pretending to succeed.
WANT_EXIT=nonzero case_run deny-bash-redirect tool_bash  responses 'headless mode cannot ask for approval' '! test -e tool_ran.txt'
WANT_EXIT=nonzero case_run deny-edit          tool_edit  responses 'headless mode cannot ask for approval' 'grep -q "hello from main" main.py'
WANT_EXIT=nonzero case_run deny-write         tool_write responses 'headless mode cannot ask for approval' '! test -e notes.txt'
# A plain workspace command runs inside the sandbox without a prompt (upstream's workspace-write model).
case_run plain-bash-no-prompt tool_bash_plain responses 'MOCK_TOOL_OK:' 'test -e tool_ran_plain.txt'
# An --allow rule grants exactly that tool family: edits are allowed, a redirecting shell command stays denied.
case_run allow-rule-edit tool_edit responses 'MOCK_TOOL_OK:' 'grep -q "goodbye from main" main.py' --allow 'Edit'
WANT_EXIT=nonzero case_run allow-rule-edit-does-not-grant-bash tool_bash responses 'headless mode cannot ask for approval' '! test -e tool_ran.txt' --allow 'Edit'

echo "---"; echo "$PASS passed, $FAIL failed"
[ $FAIL -eq 0 ]
