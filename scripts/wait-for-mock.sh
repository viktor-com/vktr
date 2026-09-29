#!/usr/bin/env bash
# Wait until the mock Viktor (scripts/mock_viktor.py) answers on 127.0.0.1:<port>, for up to
# VKTR_MOCK_WAIT seconds (default 60). Exits 1 and says so if it never does, so a slow runner fails
# with the real cause instead of with "Could not reach Viktor" a few checks later.
#
#   scripts/wait-for-mock.sh <port>...
set -u
deadline=$(( $(date +%s) + ${VKTR_MOCK_WAIT:-60} ))
for port in "$@"; do
  until curl -s -o /dev/null "http://127.0.0.1:$port/v1/models"; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      echo "mock Viktor on 127.0.0.1:$port did not answer within ${VKTR_MOCK_WAIT:-60}s" >&2
      exit 1
    fi
    sleep 0.1
  done
done
