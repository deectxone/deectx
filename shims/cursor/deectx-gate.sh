#!/usr/bin/env bash
# deeCtx Cursor gate. Fails closed: when the masking proxy isn't reachable,
# the hook denies the action. Cursor treats a non-zero exit or invalid JSON as
# a block when failClosed:true, so any unexpected error also denies.
set -euo pipefail
url="${DEECTX_URL:-http://127.0.0.1:8787/healthz}"
if curl -fsS --max-time 2 "$url" >/dev/null 2>&1; then
  echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permission":{"type":"allow"}}}'
else
  echo '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permission":{"type":"deny","reason":"deeCtx proxy is not running; masking cannot be guaranteed."}}}'
fi