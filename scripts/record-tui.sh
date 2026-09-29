#!/usr/bin/env bash
# Record a short interactive vktr session against the Viktor-compat mock with asciinema + tmux.
#
#   scripts/record-tui.sh <cast-name> <mock-mode> "<prompt>" [path/to/vktr]
#
# Writes docs/recordings/<cast-name>.cast and prints the final screen. Needs tmux and asciinema.
set -eu
NAME="$1"; MODE="$2"; PROMPT="$3"; BIN="${4:-target/release/vktr}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${MOCK_PORT:-8792}"
WORK="$(mktemp -d "$HOME/.vktr-rec.XXXXXX")"
SESSION="vktr-rec-$$"
# A private tmux server with no config: the default one would load the user's plugins (session
# restore included) and outlive the recording.
TMUX_CMD=(tmux -L "vktr-rec-$$" -f /dev/null)
mkdir -p "$WORK/home" "$WORK/proj" "$ROOT/docs/recordings"
(cd "$WORK/proj" && git init -q . && echo 'print("hello")' >main.py)
printf '[model.viktor]\nextra_headers = { "X-Mock-Mode" = "%s" }\n' "$MODE" >"$WORK/home/config.toml"

python3 "$ROOT/scripts/mock_viktor.py" "$PORT" >"$WORK/mock.log" 2>&1 &
MOCK_PID=$!
cleanup() { "${TMUX_CMD[@]}" kill-server 2>/dev/null || true; kill "$MOCK_PID" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
"$ROOT/scripts/wait-for-mock.sh" "$PORT" || exit 1

"${TMUX_CMD[@]}" new-session -d -s "$SESSION" -x 110 -y 30 -c "$WORK/proj" \
  "asciinema rec -q --overwrite '$ROOT/docs/recordings/$NAME.cast' -c \"env VKTR_HOME='$WORK/home' VIKTOR_API_KEY=zt_test_sk_mock VIKTOR_BASE_URL=http://127.0.0.1:$PORT/v1 '$ROOT/$BIN'\""

wait_for() { for _ in $(seq 1 "${2:-60}"); do "${TMUX_CMD[@]}" capture-pane -t "$SESSION" -p | grep -qiE "$1" && return 0; sleep 0.5; done; return 1; }
wait_for "Logged in|Viktor" 60
sleep 1
"${TMUX_CMD[@]}" send-keys -t "$SESSION" "$PROMPT" Enter
wait_for "MOCK_OK|\([0-9]{3}\)|Empty response|error" 120 || true
sleep 2
"${TMUX_CMD[@]}" capture-pane -t "$SESSION" -p | grep -v '^\s*$' | head -20
"${TMUX_CMD[@]}" send-keys -t "$SESSION" C-q; sleep 0.25; "${TMUX_CMD[@]}" send-keys -t "$SESSION" C-q 2>/dev/null || true; sleep 1
