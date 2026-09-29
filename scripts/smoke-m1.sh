#!/usr/bin/env bash
# M1 smoke test: drive `vktr -p` against a Viktor-compat mock endpoint in every failure mode.
#
#   scripts/smoke-m1.sh [path/to/vktr]
#
# Starts scripts/mock_viktor.py on a free port, runs the built-in `viktor` model on all three
# backends (responses, chat completions, messages) with VIKTOR_API_KEY + VIKTOR_BASE_URL, and asserts that a
# healthy run prints the reply while every failure mode exits non-zero and names the failure.
set -u
BIN="${1:-target/release/vktr}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# A free port unless MOCK_PORT pins one: a stale server on a fixed port would silently absorb the test traffic.
PORT="${MOCK_PORT:-$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')}"
WORK="$(mktemp -d "${TMPDIR_SMOKE:-$HOME}/.vktr-smoke.XXXXXX")"
PASS=0; FAIL=0

python3 "$ROOT/scripts/mock_viktor.py" "$PORT" >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
trap 'kill $MOCK_PID 2>/dev/null; rm -rf "$WORK"' EXIT
"$ROOT/scripts/wait-for-mock.sh" "$PORT" || exit 1

# run <name> <mode> <backend> <expect-exit: zero|nonzero> <expect-substring>
run() {
  local name="$1" mode="$2" backend="$3" want="$4" needle="$5"
  local home="$WORK/home-$name"; mkdir -p "$home"
  # The mock picks its failure shape from a header; the backend is the documented [model.viktor] override.
  printf '[model.viktor]\napi_backend = "%s"\nextra_headers = { "X-Mock-Mode" = "%s" }\n' "$backend" "$mode" >"$home/config.toml"
  local out; out="$(cd "$WORK" && HOME="$WORK" VKTR_HOME="$home" VIKTOR_API_KEY=zt_test_sk_mock \
      VIKTOR_BASE_URL="http://127.0.0.1:$PORT/v1" timeout 90 "$ROOT/$BIN" -p "Reply with the single word hello." 2>&1)"
  local code=$?
  local ok=1
  if [ "$want" = zero ] && [ $code -ne 0 ]; then ok=0; fi
  if [ "$want" = nonzero ] && [ $code -eq 0 ]; then ok=0; fi
  printf '%s' "$out" | grep -qi -- "$needle" || ok=0
  if ! grep -q "POST /v1/$(echo "$backend" | sed 's#chat_completions#chat/completions#') mode=$mode" "$WORK/mock.log"; then ok=0; out="$out
(mock never saw a $backend request in mode $mode)"; fi
  if [ $ok -eq 1 ]; then PASS=$((PASS+1)); echo "PASS $name (exit $code)"; else FAIL=$((FAIL+1)); echo "FAIL $name (exit $code, wanted $want + '$needle')"; printf '%s\n' "$out" | tail -5 | sed 's/^/     | /'; fi
}

# env-only run: no config file at all, backend chosen by VIKTOR_API_BACKEND
run_env() {
  local name="$1" backend="$2" path="$3"
  local home="$WORK/home-$name"; mkdir -p "$home"
  local before; before=$(wc -l <"$WORK/mock.log")
  local out; out="$(cd "$WORK" && HOME="$WORK" VKTR_HOME="$home" VIKTOR_API_KEY=zt_test_sk_mock VIKTOR_API_BACKEND="$backend" \
      VIKTOR_BASE_URL="http://127.0.0.1:$PORT/v1" timeout 90 "$ROOT/$BIN" -p "Reply with the single word hello." 2>&1)"
  local code=$?
  # every inference POST this run made must have gone to the expected backend path
  local posts; posts="$(tail -n +"$((before+1))" "$WORK/mock.log" | grep -o 'POST /v1/[a-z/]*' | sort -u)"
  # and there must be exactly one: title, summary and suggestion side calls would each be a billed Viktor run
  local npost; npost="$(tail -n +"$((before+1))" "$WORK/mock.log" | grep -c 'POST /v1/')"
  if [ $code -eq 0 ] && printf '%s' "$out" | grep -q MOCK_OK && [ "$posts" = "POST $path" ] && [ "$npost" -eq 1 ]; then
    PASS=$((PASS+1)); echo "PASS $name (exit $code)"
  else FAIL=$((FAIL+1)); echo "FAIL $name (exit $code, $npost inference calls to: $posts)"; printf '%s\n' "$out" | tail -5 | sed 's/^/     | /'; fi
}

run_env env-only-default      ""                 /v1/responses
run_env env-only-chat         chat_completions   /v1/chat/completions
run_env env-only-messages     messages           /v1/messages
run ok-responses          ok            responses         zero    "MOCK_OK"
run ok-chat               ok            chat_completions  zero    "MOCK_OK"
run ok-messages           ok            messages          zero    "MOCK_OK"
run run-failed-502        run_failed    responses         nonzero "closed two consecutive streams"
run run-failed-502-chat   run_failed    chat_completions  nonzero "closed two consecutive streams"
run run-failed-502-msgs   run_failed    messages          nonzero "closed two consecutive streams"
run stream-error-chat     stream_error  chat_completions  nonzero "closed two consecutive streams"
run response-failed       stream_error  responses         nonzero "closed two consecutive streams"
run error-event-messages  stream_error  messages          nonzero "closed two consecutive streams"
run empty-responses       empty         responses         nonzero "empty response"
run empty-chat            empty         chat_completions  nonzero "empty response"
run empty-messages        empty         messages          nonzero "empty response"
run unauthorized          unauthorized  responses         nonzero "Invalid API key"
run forbidden-scope       forbidden     responses         nonzero "chat:completions"

# The lean default toolset: the heavy upstream tools are not sent unless VKTR_FULL_TOOLSET=1.
DUMP_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
mkdir -p "$WORK/dump"
MOCK_DUMP_DIR="$WORK/dump" python3 "$ROOT/scripts/mock_viktor.py" "$DUMP_PORT" >"$WORK/dump-mock.log" 2>&1 &
DUMP_PID=$!
trap 'kill $MOCK_PID $DUMP_PID 2>/dev/null; rm -rf "$WORK"' EXIT
"$ROOT/scripts/wait-for-mock.sh" "$DUMP_PORT" || exit 1
toolset() {
  local name="$1" full="$2" want="$3"
  rm -f "$WORK"/dump/*; local home="$WORK/home-$name"; mkdir -p "$home"
  (cd "$WORK" && HOME="$WORK" VKTR_HOME="$home" VIKTOR_API_KEY=zt_test_sk_mock VKTR_FULL_TOOLSET="$full" \
      VIKTOR_BASE_URL="http://127.0.0.1:$DUMP_PORT/v1" timeout 90 "$ROOT/$BIN" -p "hello" >/dev/null 2>&1)
  local heavy; heavy="$(python3 -c '
import glob, json, sys
b = json.load(open(sorted(glob.glob(sys.argv[1] + "/*"))[-1])); b = b.get("body", b)
names = {t.get("name") for t in b.get("tools", [])}
print(len(names & {"workflow", "spawn_subagent", "scheduler_create", "scheduler_delete", "scheduler_list", "monitor"}))
' "$WORK/dump" 2>/dev/null)"
  if [ "$heavy" = "$want" ]; then PASS=$((PASS+1)); echo "PASS $name ($heavy heavy tools sent)"; else FAIL=$((FAIL+1)); echo "FAIL $name (sent ${heavy:-?} heavy tools, wanted $want)"; fi
}
toolset lean-toolset-by-default 0 0
toolset full-toolset-on-request 1 6

echo "---"; echo "$PASS passed, $FAIL failed"
[ $FAIL -eq 0 ]
