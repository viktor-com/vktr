#!/usr/bin/env bash
# Everything a release binary must pass before it is published, with no API key and no network
# (the Viktor side is scripts/mock_viktor.py). The release workflow runs it on every target OS
# against the binary install.sh fetched from the staged draft release.
#
#   scripts/smoke-release.sh <path/to/vktr relative to the checkout> [expected --version line]
#
# Needs bash, python3 (3.8+), curl, file and GNU-compatible `timeout` on PATH.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2
BIN="${1:?usage: scripts/smoke-release.sh <path/to/vktr> [expected version line]}"
WANT="${2:-}"
# The M1-M4 scripts resolve the binary as "$ROOT/$BIN".
case "$BIN" in "$ROOT"/*) BIN="${BIN#"$ROOT"/}" ;; /*) echo "smoke-release: $BIN must be inside $ROOT" >&2; exit 2 ;; esac
[ -x "$BIN" ] || { echo "smoke-release: $BIN is not an executable" >&2; exit 2; }

failed=()
step() { # step <name> <command...>
  local name="$1"; shift
  echo "::group::$name"
  local start=$SECONDS
  if "$@"; then echo "ok: $name ($((SECONDS - start))s)"; else echo "FAILED: $name"; failed+=("$name"); fi
  echo "::endgroup::"
}

version_line() {
  local got; got="$("$BIN" --version)" || return 1
  echo "$got"
  [ -z "$WANT" ] || [ "$got" = "$WANT" ] || { echo "expected: $WANT" >&2; return 1; }
}

step "version" version_line
step "help" "$BIN" --help
step "help text names no upstream" scripts/check-help-text.sh "$BIN"
step "M1 headless runs and failure modes" scripts/smoke-m1.sh "$BIN"
step "M2 local tools and approvals" scripts/smoke-m2.sh "$BIN"
step "M3 sessions, resume, images" scripts/smoke-m3.sh "$BIN"
step "M4 vktr launch" scripts/smoke-m4.sh "$BIN"
step "ACP over stdio" python3 scripts/smoke-acp.py "$BIN"

if [ "${#failed[@]}" -gt 0 ]; then
  echo "smoke-release: ${#failed[@]} failed: ${failed[*]}" >&2
  exit 1
fi
echo "smoke-release: all checks passed on $(uname -sm)"
