#!/usr/bin/env bash
# Live checks against the real Viktor compat API. Every passing case below is a billed Viktor run.
#
#   VIKTOR_API_KEY=... scripts/live-check.sh [path/to/vktr]
#
# Key hygiene: the key is read from the environment only, is never echoed, and is scrubbed from all
# captured output before anything is printed or kept. Steps that would persist the key are skipped on
# purpose: `vktr login --api-key` (saves to config.toml) and `vktr launch --viktor pi` (pi keeps provider
# keys in models.json). Throwaway VKTR_HOME directories are scanned for the key and removed at the end.
set -u
BIN="${1:-target/release/vktr}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
V="$ROOT/$BIN"
[ -n "${VIKTOR_API_KEY:-}" ] || { echo "live-check: VIKTOR_API_KEY is not set" >&2; exit 2; }
KEY="$VIKTOR_API_KEY"
WORK="$(mktemp -d "$HOME/.vktr-live.XXXXXX")"
PASS=0; FAIL=0; SKIP=0
export PATH="$PATH:$HOME/.local/share/npm/bin"
trap 'rm -rf "$WORK"' EXIT

scrub() { sed "s/${KEY//\//\\/}/<redacted>/g"; }
ok()   { PASS=$((PASS+1)); echo "PASS $1 ${2:-}"; }
bad()  { FAIL=$((FAIL+1)); echo "FAIL $1"; shift; printf '%s\n' "$@" | scrub | tail -8 | cut -c1-300 | sed 's/^/     | /'; }
skip() { SKIP=$((SKIP+1)); echo "SKIP $1: $2"; }
fresh() { HOME_DIR="$WORK/home-$1"; PROJ="$WORK/proj-$1"; mkdir -p "$HOME_DIR" "$PROJ"; (cd "$PROJ" && git init -q .); }
# vk <args...>: run vktr in $PROJ with a throwaway home; stdout+stderr captured in $OUT, exit code in $CODE
vk() { local t0; t0=$(date +%s); OUT="$(cd "$PROJ" && VKTR_HOME="$HOME_DIR" timeout "${LIVE_TIMEOUT:-420}" "$V" "$@" 2>&1 </dev/null)"; CODE=$?; SECS=$(( $(date +%s) - t0 )); }

# ---------------------------------------------------------------- M1: boot, chat, errors
for backend in responses chat_completions messages; do
  fresh "chat-$backend"
  VIKTOR_API_BACKEND=$backend vk -p "Reply with exactly the word PONG and nothing else."
  if [ $CODE -eq 0 ] && echo "$OUT" | grep -qi "pong"; then ok "m1-chat-$backend" "(${SECS}s)"; else bad "m1-chat-$backend" "exit=$CODE" "$OUT"; fi
done

fresh json
vk -p "Reply with exactly the word PONG and nothing else." --json
if [ $CODE -eq 0 ] && echo "$OUT" | python3 -c 'import sys,json; d=json.load(sys.stdin); assert "pong" in d["text"].lower() and d["stopReason"]=="end_turn" and d["usage"]["total_tokens"]>0' 2>/dev/null; then ok m1-json-output "(${SECS}s)"; else bad m1-json-output "exit=$CODE" "$OUT"; fi

fresh badkey
OUT="$(cd "$PROJ" && VKTR_HOME="$HOME_DIR" VIKTOR_API_KEY="zt_live_sk_this_key_does_not_exist" timeout 120 "$V" -p "hi" 2>&1 </dev/null)"; CODE=$?
if [ $CODE -ne 0 ] && echo "$OUT" | grep -qiE "401|403|unauthor|invalid|api key|forbidden"; then ok m1-bad-key-is-reported; else bad m1-bad-key-is-reported "exit=$CODE" "$OUT"; fi

fresh loginbad
OUT="$(VKTR_HOME="$HOME_DIR" "$V" login --api-key "zt_live_sk_this_key_does_not_exist" 2>&1 </dev/null)"; CODE=$?
if [ $CODE -ne 0 ] && echo "$OUT" | grep -q "Key check against" && [ ! -e "$HOME_DIR/config.toml" ]; then ok m1-login-rejects-bad-key-and-saves-nothing; else bad m1-login-rejects-bad-key-and-saves-nothing "exit=$CODE" "$OUT"; fi
skip m1-login-saves-good-key "would write the live key to disk"

# ---------------------------------------------------------------- M2: local tools over the live API
fresh read
TOKEN="vktr-live-$(date +%s)-$RANDOM"
echo "$TOKEN" >"$PROJ/secret_note.txt"
vk -p "Use your read_file tool to read the file secret_note.txt in the current directory, then reply with exactly its content and nothing else."
if [ $CODE -eq 0 ] && echo "$OUT" | grep -q "$TOKEN"; then ok m2-read-file-round-trip "(${SECS}s)"; else bad m2-read-file-round-trip "exit=$CODE" "$OUT"; fi
if grep -rqs "viktor continuation: resuming" "$HOME_DIR/logs" 2>/dev/null; then ok m3-continuation-used-on-tool-round-trip; else skip m3-continuation-used-on-tool-round-trip "no continuation line in the log (tracing level), see m3-resume"; fi

fresh edit
printf 'def greet():\n    print("hello from main")\n' >"$PROJ/main.py"
vk -p "Use your file editing tool to replace the word hello with goodbye in main.py. Do not run any shell commands." --always-approve
if [ $CODE -eq 0 ] && grep -q "goodbye from main" "$PROJ/main.py"; then ok m2-edit-with-approval "(${SECS}s)"; else bad m2-edit-with-approval "exit=$CODE" "$(cat "$PROJ/main.py")" "$OUT"; fi

fresh deny
printf 'def greet():\n    print("hello from main")\n' >"$PROJ/main.py"
vk -p "Use your file editing tool to replace the word hello with goodbye in main.py. Do not run any shell commands."
if grep -q "hello from main" "$PROJ/main.py" && echo "$OUT" | grep -q "headless mode cannot ask for approval" && [ $CODE -ne 0 ]; then ok m2-edit-denied-without-approval "(${SECS}s)"; else bad m2-edit-denied-without-approval "exit=$CODE" "$(cat "$PROJ/main.py")" "$OUT"; fi

# ---------------------------------------------------------------- M3: resume, continuation, images
fresh resume
WORD="quokka$RANDOM"
thread_id() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[0][1]["response_id"])' "$HOME_DIR/viktor_threads.json" 2>/dev/null; }
vk -p "Remember this code word for later: $WORD. Reply with just OK."
first=$CODE; thread1="$(thread_id)"
vk -c -p "What was the code word I asked you to remember? Reply with just the word."
thread2="$(thread_id)"
if [ $first -eq 0 ] && [ $CODE -eq 0 ] && echo "$OUT" | grep -qi "$WORD"; then ok m3-resume-keeps-context "(${SECS}s)"; else bad m3-resume-keeps-context "first=$first exit=$CODE" "$OUT"; fi
# Viktor returns the thread id as the response id, so an unchanged id across the resumed turn proves the
# second request continued the same durable thread (it carried only the new message) instead of replaying.
if [ -n "$thread1" ] && [ "$thread1" = "$thread2" ]; then ok m3-resumed-turn-continues-the-same-viktor-thread; else bad m3-resumed-turn-continues-the-same-viktor-thread "turn1=$thread1 turn2=$thread2"; fi
if [ -s "$HOME_DIR/viktor_threads.json" ] && python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d and d[0][1]["response_id"]' "$HOME_DIR/viktor_threads.json" 2>/dev/null; then ok m3-viktor-thread-id-recorded; else bad m3-viktor-thread-id-recorded "$(ls "$HOME_DIR")"; fi

fresh stateless
vk -p "Remember this code word for later: $WORD. Reply with just OK."
VKTR_RESPONSES_CONTINUATION=0 vk -c -p "What was the code word I asked you to remember? Reply with just the word."
if [ $CODE -eq 0 ] && echo "$OUT" | grep -qi "$WORD"; then ok m3-resume-works-stateless-too "(${SECS}s)"; else bad m3-resume-works-stateless-too "exit=$CODE" "$OUT"; fi

fresh image
python3 - "$PROJ/red.png" <<'EOF'
import sys, zlib, struct
def chunk(t, d):
    c = struct.pack('>I', len(d)) + t + d
    return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = b''.join(b'\x00' + b'\xff\x00\x00' * 64 for _ in range(64))
open(sys.argv[1], 'wb').write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 64, 64, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
EOF
pj="[{\"type\":\"text\",\"text\":\"What single colour fills this image? Reply with one word.\"},{\"type\":\"image\",\"data\":\"$(base64 -w0 "$PROJ/red.png")\",\"mimeType\":\"image/png\"}]"
vk --prompt-json "$pj"
if [ $CODE -eq 0 ] && echo "$OUT" | grep -qi "red"; then ok m3-image-input "(${SECS}s)"; else bad m3-image-input "exit=$CODE" "$OUT"; fi

# ---------------------------------------------------------------- M4: vktr launch --viktor
fresh launch
OUT="$(cd "$PROJ" && VKTR_HOME="$HOME_DIR" "$V" launch --viktor --config codex 2>&1)"
if echo "$OUT" | grep -q "backend=viktor" && ! echo "$OUT" | grep -qF "$KEY"; then ok m4-config-masks-the-live-key; else bad m4-config-masks-the-live-key "$OUT"; fi
live_tool() { # live_tool <name> <binary> <args...>
  local name="$1" bin="$2"; shift 2
  # `vktr` as the tool is this binary itself (launch re-execs the running executable)
  [ "$bin" = vktr ] || command -v "$bin" >/dev/null 2>&1 || { skip "$name" "$bin not installed"; return; }
  local t0; t0=$(date +%s)
  OUT="$(cd "$PROJ" && VKTR_HOME="$HOME_DIR" timeout "${LIVE_TIMEOUT:-420}" "$V" launch --viktor "$bin" "$@" 2>&1 </dev/null)"; CODE=$?
  if [ $CODE -eq 0 ] && echo "$OUT" | grep -qi "pong"; then ok "$name" "($(( $(date +%s) - t0 ))s)"; else bad "$name" "exit=$CODE" "$OUT"; fi
}
live_tool m4-launch-viktor-vktr     vktr     -p "Reply with exactly the word PONG and nothing else."
live_tool m4-launch-viktor-codex    codex    exec --skip-git-repo-check "Reply with exactly the word PONG and nothing else."
live_tool m4-launch-viktor-opencode opencode run "Reply with exactly the word PONG and nothing else."
live_tool m4-launch-viktor-claude   claude   -p "Reply with exactly the word PONG and nothing else."
skip m4-launch-viktor-pi "pi stores provider keys in models.json, which would write the live key to disk"

# ---------------------------------------------------------------- key hygiene
leaks="$(grep -rlF "$KEY" "$WORK" 2>/dev/null | wc -l)"
if [ "$leaks" -eq 0 ]; then ok key-not-written-anywhere-under-the-test-homes; else bad key-not-written-anywhere-under-the-test-homes "$leaks file(s) contain the key: $(grep -rlF "$KEY" "$WORK" | sed "s#$WORK/##" | head -5 | tr '\n' ' ')"; fi

echo "---"; echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ $FAIL -eq 0 ]
