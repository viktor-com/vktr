#!/usr/bin/env bash
# M3 smoke test: sessions, resume, Viktor thread continuation, and image input.
#
#   scripts/smoke-m3.sh [path/to/vktr]
#
# Uses the stateful mock: a Responses request without previous_response_id opens a new thread, one with
# a known id continues it, one with an unknown id gets 404 (as the Viktor compat API does).
set -u
BIN="${1:-target/release/vktr}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# A free port unless MOCK_PORT pins one: a stale server on a fixed port would silently absorb the test traffic.
PORT="${MOCK_PORT:-$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')}"
LOSSY_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
WORK="$(mktemp -d "$HOME/.vktr-smoke3.XXXXXX")"
PASS=0; FAIL=0

MOCK_DUMP_DIR="$WORK/dump" python3 "$ROOT/scripts/mock_viktor.py" "$PORT" >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
MOCK_LOSE_THREADS=1 python3 "$ROOT/scripts/mock_viktor.py" "$LOSSY_PORT" >"$WORK/lossy.log" 2>&1 &
LOSSY_PID=$!
trap 'kill $MOCK_PID $LOSSY_PID 2>/dev/null; rm -rf "$WORK"' EXIT
"$ROOT/scripts/wait-for-mock.sh" "$PORT" "$LOSSY_PORT" || exit 1

ok()   { PASS=$((PASS+1)); echo "PASS $1"; }
bad()  { FAIL=$((FAIL+1)); echo "FAIL $1"; shift; printf '     | %s\n' "$@"; }
# threads <log> <from-line>: the "responses thread=" lines a run produced
threads() { tail -n +"$(( $2 + 1 ))" "$1" | grep -a 'responses thread=' ; }
fresh() { HOME_DIR="$WORK/home-$1"; PROJ="$WORK/proj-$1"; mkdir -p "$HOME_DIR" "$PROJ"; (cd "$PROJ" && git init -q . && echo 'print("hello from main")' >main.py); }
vk() { (cd "$PROJ" && HOME="$WORK" VKTR_HOME="$HOME_DIR" VIKTOR_API_KEY=zt_test_sk_mock VIKTOR_BASE_URL="http://127.0.0.1:${VK_PORT:-$PORT}/v1" timeout 120 "$ROOT/$BIN" "$@" 2>&1); }

# 1. A tool round trip continues the thread: request 2 carries previous_response_id and only the tool result.
fresh cont; printf '[model.viktor]\nextra_headers = { "X-Mock-Mode" = "tool_read" }\n' >"$HOME_DIR/config.toml"
before=$(wc -l <"$WORK/mock.log"); out="$(vk -p "read it")"; t="$(threads "$WORK/mock.log" "$before")"
if echo "$out" | grep -q MOCK_TOOL_OK && [ "$(echo "$t" | wc -l)" -eq 2 ] && echo "$t" | sed -n 2p | grep -q 'continued=True items_in_request=1 '; then ok continuation-tool-round-trip; else bad continuation-tool-round-trip "$t" "$out"; fi

# 2. VKTR_RESPONSES_CONTINUATION=0 restores stateless replay: two fresh threads, full history each time.
fresh nocont; printf '[model.viktor]\nextra_headers = { "X-Mock-Mode" = "tool_read" }\n' >"$HOME_DIR/config.toml"
before=$(wc -l <"$WORK/mock.log"); out="$(VKTR_RESPONSES_CONTINUATION=0 vk -p "read it")"; t="$(threads "$WORK/mock.log" "$before")"
if echo "$out" | grep -q MOCK_TOOL_OK && [ "$(echo "$t" | grep -c 'continued=False')" -eq 2 ]; then ok continuation-can-be-disabled; else bad continuation-can-be-disabled "$t"; fi

# 3. The server lost the thread: the 404 is absorbed and the history is replayed in a new thread.
fresh lost; printf '[model.viktor]\nextra_headers = { "X-Mock-Mode" = "tool_read" }\n' >"$HOME_DIR/config.toml"
out="$(VK_PORT=$LOSSY_PORT vk -p "read it")"; code=$?
if [ $code -eq 0 ] && echo "$out" | grep -q MOCK_TOOL_OK && grep -a -q 'prev=thread_mock_1' "$WORK/lossy.log" && [ "$(grep -a -c 'continued=False' "$WORK/lossy.log")" -eq 2 ]; then ok lost-thread-replays-in-full; else bad lost-thread-replays-in-full "exit=$code" "$(grep -a 'thread' "$WORK/lossy.log")" "$out"; fi

# 4. --continue in a new process resumes the session AND its Viktor thread (one new item per turn).
fresh resume
before=$(wc -l <"$WORK/mock.log"); vk -p "first question" >/dev/null; vk -c -p "second question" >/dev/null; out="$(vk -c -p "third question")"; t="$(threads "$WORK/mock.log" "$before")"
ids="$(echo "$t" | grep -o 'responses thread=[a-z_0-9]*' | sort -u | wc -l)"
if echo "$out" | grep -q MOCK_OK && [ "$(echo "$t" | wc -l)" -eq 3 ] && [ "$ids" -eq 1 ] && [ "$(echo "$t" | grep -c 'continued=True items_in_request=1 ')" -eq 2 ]; then ok continue-resumes-session-and-thread; else bad continue-resumes-session-and-thread "$t"; fi

# 5. Sessions are stored per workspace under VKTR_HOME/sessions/<encoded cwd>/.
enc="$(python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.argv[1],safe=""))' "$PROJ")"
if [ -d "$HOME_DIR/sessions/$enc" ] && [ "$(find "$HOME_DIR/sessions/$enc" -mindepth 1 -maxdepth 1 -type d | wc -l)" -eq 1 ]; then ok sessions-are-workspace-scoped; else bad sessions-are-workspace-scoped "$(ls "$HOME_DIR/sessions")"; fi

# 6. --resume <session id> picks a specific session; without -c/-r a run starts a new session and thread.
sid="$(basename "$(find "$HOME_DIR/sessions/$enc" -mindepth 1 -maxdepth 1 -type d | head -1)")"
before=$(wc -l <"$WORK/mock.log"); vk -r "$sid" -p "fourth question" >/dev/null; vk -p "unrelated" >/dev/null; t="$(threads "$WORK/mock.log" "$before")"
if echo "$t" | sed -n 1p | grep -q 'continued=True' && echo "$t" | sed -n 2p | grep -qE 'continued=False items_in_request=[34] '; then ok resume-by-id-and-new-session; else bad resume-by-id-and-new-session "$t"; fi

# 7. Images reach the wire as image content on every backend.
fresh image
python3 - "$PROJ/red.png" <<'EOF'
import sys, zlib, struct
def chunk(t, d):
    c = struct.pack('>I', len(d)) + t + d
    return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = b''.join(b'\x00' + b'\xff\x00\x00' * 64 for _ in range(64))
open(sys.argv[1], 'wb').write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 64, 64, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
EOF
pj="[{\"type\":\"text\",\"text\":\"What colour is this image?\"},{\"type\":\"image\",\"data\":\"$(base64 <"$PROJ/red.png" | tr -d '\n')\",\"mimeType\":\"image/png\"}]"
for backend in responses chat_completions messages; do
  rm -rf "$WORK/dump"; VIKTOR_API_BACKEND=$backend vk --prompt-json "$pj" >/dev/null
  case $backend in responses) want='input_image';; chat_completions) want='image_url';; messages) want='"type": "image"';; esac
  if grep -q -- "$want" "$WORK"/dump/req-*.json 2>/dev/null; then ok "image-input-$backend"; else bad "image-input-$backend" "no $want in the request"; fi
done

echo "---"; echo "$PASS passed, $FAIL failed"
[ $FAIL -eq 0 ]
