#!/usr/bin/env bash
# Fail if any `vktr [<cmd> [<sub>]] --help` names grok or x.ai. Hidden commands and flags are
# not listed by help, so they are not checked. `vktr launch` can start the third-party grok CLI,
# so its tool list is the one line allowed to name it.
#
#   scripts/check-help-text.sh [path/to/vktr]
set -u
V="${1:-target/release/vktr}"
commands() { "$@" --help 2>/dev/null | awk '/^Commands:/{f=1;next} /^[^ ]/{f=0} f && NF{print $1}' | grep -v '^help$'; }
bad=0
check() {
  local hits
  hits=$("$V" "$@" --help 2>&1 | grep -v 'copilot, grok, hermes' | grep -inE 'grok|x\.ai')
  if [ -n "$hits" ]; then echo "vktr $* --help:"; echo "$hits" | head -3; bad=1; fi
}
check
for c in $(commands "$V"); do
  check "$c"
  for s in $(commands "$V" "$c"); do check "$c" "$s"; done
done
exit $bad
